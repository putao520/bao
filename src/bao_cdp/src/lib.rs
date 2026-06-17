// REQ-CDP-003: CDP module public API — server entry + WS / JSON-RPC codec
// @trace REQ-CDP-001 [entity:CdpRouter] [entity:CdpServer]
// @trace REQ-PURE-009 [level:library] [entity:HttpServer,HttpServerConfig]
// @trace REQ-IMPL-06
//
// TASK-6 (DEC-CDP-001): evaluate_js 注入式 domain handler 已删除,
// CDP 命令分发由 bao_cdp_client::CDPRdpBridge 接管。本 crate 退化为
// 对外 CDP server 入口(Playwright 兼容)+ 基础设施(RFC 6455 codec、
// JSON-RPC 编解码、Target 路由),被 bao_cdp_client 复用。
//
// TASK-18 (REQ-CDP-UWS-001): RFC 6455 codec / handshake / masking 已迁移
// 至 `bun_uws`(ws_codec / ws_handshake / ws_client / ws_server)。本 crate
// 通过 `pub use bun_uws::*` 重导出,删除自写 ws_codec/ws_handshake/ws。
// 所有 WebSocket 表面入口统一在 `bun_uws`(bao_cdp / bao_cdp_client 仅依赖
// bun_uws,无 tungstenite)。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};

// ---------------------------------------------------------------------------
// §1 CdpServer public re-exports (from cdp-server crate)
// ---------------------------------------------------------------------------

pub use cdp_server::{BaoEvent, ConsoleMessage};

// WebSocket surface — re-exported from `bun_uws` (REQ-CDP-UWS-001).
// Removed: bao_cdp::{ws, ws_codec, ws_handshake} self-written modules.
pub use bun_uws::ws_codec::{self, FrameDecoder, FrameEncoder, FrameHeader, Message, Opcode};
pub use bun_uws::ws_handshake::{
    self, client_handshake, compute_accept, generate_sec_websocket_key, server_handshake,
    HandshakeError,
};
pub use bun_uws::ws_server::{self, ReplayStream as UwsReplayStream, WsServerConnection};
// `ws::read_message` / `ws::write_message` legacy helpers — replaced by
// `WsServerConnection::{read, write_text}` from bun_uws. Kept API-compatible
// via the new `ws` shim module below.

mod protocol;
mod backend;
mod router;
pub mod servo_bridge;
pub mod domains;

pub use protocol::{parse_message, handle_command, serialize_response, serialize_event};
pub use protocol::{CDPError, CDPEvent, CDPMessage, CDPResponse};
pub use router::{BackendKind, CdpRouter, CdpSession, ExternalBrowser};
pub use servo_bridge::{bridge_channel, BridgeCommand, BridgeReceiver, BridgeResponse, BridgeSender};

// ---------------------------------------------------------------------------
// §2 ReplayStream + WebSocketConnection — shared with backend.rs
//
// REQ-CDP-UWS-001: `ReplayStream` is re-exported from `bun_uws::ws_server`
// (same drain-peeked-buffer-then-stream semantics). The local alias keeps
// the legacy `bao_cdp::ReplayStream` path working for backend.rs and tests.
// ---------------------------------------------------------------------------

pub use bun_uws::ws_server::ReplayStream;

/// WebSocket connection using the `bun_uws` frame codec.
pub struct WebSocketConnection {
    pub stream: UwsReplayStream,
    pub decoder: FrameDecoder,
    pub encoder: FrameEncoder,
}

// ---------------------------------------------------------------------------
// §3 CDPServer — legacy synchronous CDP server entry point (Playwright compat)
//
// This is the original synchronous TCP server. It listens on 127.0.0.1:port,
// serves /json/* HTTP discovery endpoints, performs the WebSocket upgrade,
// and dispatches incoming CDP commands via `protocol::handle_command` +
// the optional servo `BridgeSender`. Domain commands that need real servo
// state (Page.navigate, Runtime.evaluate, DOM.getDocument, ...) are routed
// through the bridge; everything else returns a stub response.
//
// Note: cdp-server crate's async `CdpServer` is the new Playwright-compatible
// entry. Both coexist during the migration; this synchronous server remains
// for tests / integration that don't need cdp-server's full registry.
// ---------------------------------------------------------------------------

pub struct CDPServer {
    port: u16,
    target_id: String,
    sessions: HashMap<String, CDPSession>,
    cmd_tx: Sender<CDPCommand>,
    cmd_rx: Receiver<CDPCommand>,
    bridge: Option<BridgeSender>,
}

#[derive(Debug)]
pub enum CDPCommand {
    SendEvent(CDPEvent),
    Shutdown,
}

pub struct CDPSession {
    id: String,
    target_id: String,
    pub ws: WsServerConnection,
    bridge: Option<BridgeSender>,
    #[allow(dead_code)]
    cmd_tx: Sender<CDPCommand>,
}

impl CDPServer {
    pub fn new(port: u16) -> Self {
        let (cmd_tx, cmd_rx) = channel();
        CDPServer {
            port,
            target_id: format!("{:016x}", rand_id()),
            sessions: HashMap::new(),
            cmd_tx,
            cmd_rx,
            bridge: None,
        }
    }

    pub fn with_bridge(port: u16, bridge: BridgeSender) -> Self {
        let (cmd_tx, cmd_rx) = channel();
        CDPServer {
            port,
            target_id: format!("{:016x}", rand_id()),
            sessions: HashMap::new(),
            cmd_tx,
            cmd_rx,
            bridge: Some(bridge),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/devtools/page/{}", self.port, self.target_id)
    }

    pub fn json_url(&self) -> String {
        format!("http://127.0.0.1:{}/json", self.port)
    }

    pub fn event_sender(&self) -> Sender<CDPCommand> {
        self.cmd_tx.clone()
    }

    pub fn send_event(&self, method: &str, params: serde_json::Value) {
        let ev = CDPEvent {
            method: method.to_string(),
            params: Some(params),
        };
        let _ = self.cmd_tx.send(CDPCommand::SendEvent(ev));
    }

    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(CDPCommand::Shutdown);
    }

    #[allow(unreachable_code)]
    pub fn run(&mut self) -> Result<(), CDPServerError> {
        let listener = TcpListener::bind(("127.0.0.1", self.port))
            .map_err(|e| CDPServerError::Bind(e.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| CDPServerError::Io(e.to_string()))?;

        log::info!("CDP listening on ws://127.0.0.1:{}", self.port);
        log::info!("DevTools: {}", self.ws_url());

        loop {
            // Drain command channel without dropping Shutdown.
            // The previous `while let Ok(SendEvent)` pattern consumed *any* message
            // (including Shutdown) and silently dropped it when the pattern didn't match,
            // making graceful shutdown impossible.
            loop {
                match self.cmd_rx.try_recv() {
                    Ok(CDPCommand::SendEvent(ev)) => self.broadcast_event(&ev),
                    Ok(CDPCommand::Shutdown) => {
                        log::info!("[server] run loop exiting");
                        return Ok(());
                    }
                    Err(_) => break,
                }
            }

            match listener.accept() {
                Ok((stream, _addr)) => {
                    if let Some(session) = self.handle_connection(stream) {
                        self.sessions.insert(session.id.clone(), session);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => log::info!("CDP accept error: {}", e),
            }

            let mut to_remove = Vec::new();
            for (id, session) in &mut self.sessions {
                if session.process().is_err() {
                    to_remove.push(id.clone());
                }
            }
            for id in to_remove {
                self.sessions.remove(&id);
            }

            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Outer loop only exits via `return Ok(())` on Shutdown.
        unreachable!("run loop exited without Shutdown")
    }

    fn handle_connection(&self, mut stream: TcpStream) -> Option<CDPSession> {
        let mut buf = [0u8; 8192];
        stream.set_nonblocking(false).ok()?;
        let n = stream.read(&mut buf).ok()?;
        let request = std::str::from_utf8(&buf[..n]).ok()?;

        // HTTP JSON discovery endpoints
        if request.starts_with("GET /json/version") {
            respond_json(
                &mut stream,
                &serde_json::json!({
                    "Browser": "Bao/0.1.0",
                    "Protocol-Version": "1.3",
                    "User-Agent": "Bao/0.1.0",
                    "V8-Version": "SpiderMonkey",
                    "WebKit-Version": "Servo",
                    "webSocketDebuggerUrl": self.ws_url()
                }),
            );
            return None;
        }

        if request.starts_with("GET /json/new") {
            let url = request
                .split_whitespace().nth(1)
                .and_then(|p| p.strip_prefix("/json/new?"))
                .unwrap_or("about:blank");
            let entry = serde_json::json!({
                "id": self.target_id,
                "type": "page",
                "title": "Bao",
                "url": url,
                "webSocketDebuggerUrl": format!("ws://127.0.0.1:{}/devtools/page/{}", self.port, self.target_id)
            });
            respond_json(&mut stream, &entry);
            return None;
        }

        if request.starts_with("GET /json/close/") {
            respond_json(&mut stream, &serde_json::json!("Target is closing"));
            return None;
        }

        if request.starts_with("GET /json/activate/") {
            respond_json(&mut stream, &serde_json::json!("Target activated"));
            return None;
        }

        if request.starts_with("GET /json/list") || request.starts_with("GET /json ") || request == "GET /json" {
            let entry = serde_json::json!({
                "id": self.target_id,
                "type": "page",
                "title": "Bao",
                "url": "about:blank",
                "webSocketDebuggerUrl": format!("ws://127.0.0.1:{}/devtools/page/{}", self.port, self.target_id)
            });
            respond_json(&mut stream, &serde_json::json!([entry]));
            return None;
        }

        // WebSocket upgrade — perform handshake using bun_uws codec
        if request.starts_with("GET /devtools/page/") {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(1000)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(5000)));

            let peeked = buf[..n].to_vec();
            match WsServerConnection::accept(stream, peeked) {
                Ok(ws_connection) => {
                    return Some(CDPSession {
                        id: format!("{:016x}", rand_id()),
                        target_id: self.target_id.clone(),
                        ws: ws_connection,
                        bridge: self.bridge.clone(),
                        cmd_tx: self.cmd_tx.clone(),
                    });
                }
                Err(e) => {
                    log::info!("CDP WebSocket handshake failed: {:?}", e);
                    return None;
                }
            }
        }

        respond_raw(&mut stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        None
    }

    fn broadcast_event(&mut self, ev: &CDPEvent) {
        for session in self.sessions.values_mut() {
            let _ = session.send_event(ev);
        }
    }
}

impl CDPSession {
    #[allow(clippy::result_unit_err)]
    pub fn process(&mut self) -> Result<(), ()> {
        // bun_uws::WsServerConnection::read returns ReadOutcome, which already
        // skips Ping/Pong control frames internally (replaces the old
        // `ws::read_message` retry loop).
        let outcome = self.ws.read();
        let msg = match outcome {
            ws_server::ReadOutcome::Text(t) => t,
            ws_server::ReadOutcome::Binary(d) => String::from_utf8_lossy(&d).into_owned(),
            ws_server::ReadOutcome::Control | ws_server::ReadOutcome::Pending => return Ok(()),
            ws_server::ReadOutcome::Closed => return Err(()),
        };

        let cdp_msg: CDPMessage = match protocol::parse_message(&msg) {
            Some(m) => m,
            None => return Ok(()),
        };

        let response = protocol::handle_command(
            cdp_msg.clone(),
            &self.target_id,
            &cdp_msg.params,
            self.bridge.as_ref(),
        );
        let response_json = protocol::serialize_response(&response);
        let _ = self.ws.write_text(&response_json);

        Ok(())
    }

    #[allow(clippy::result_unit_err)]
    pub fn send_event(&mut self, ev: &CDPEvent) -> Result<(), ()> {
        let json = protocol::serialize_event(ev);
        self.ws.write_text(&json)
    }
}

fn respond_json(stream: &mut TcpStream, value: &serde_json::Value) {
    let body = value.to_string();
    respond_raw(
        stream,
        &format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        ),
    );
}

fn respond_raw(stream: &mut TcpStream, response: &str) {
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn rand_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    d.as_nanos() as u64 ^ (d.as_nanos() as u64).wrapping_shr(17)
}

#[derive(Debug)]
pub enum CDPServerError {
    Bind(String),
    Io(String),
    WebSocket(String),
    Protocol(String),
}

impl std::fmt::Display for CDPServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            CDPServerError::Bind(msg) => write!(f, "Bind error: {}", msg),
            CDPServerError::Io(msg) => write!(f, "IO error: {}", msg),
            CDPServerError::WebSocket(msg) => write!(f, "WebSocket error: {}", msg),
            CDPServerError::Protocol(msg) => write!(f, "Protocol error: {}", msg),
        }
    }
}

impl std::error::Error for CDPServerError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_server_new_creates_server() {
        let server = CDPServer::new(9222);
        assert_eq!(server.port(), 9222);
        assert!(!server.target_id().is_empty());
    }

    #[test]
    fn cdp_server_ws_url_format() {
        let server = CDPServer::new(9222);
        let ws_url = server.ws_url();
        assert!(ws_url.starts_with("ws://127.0.0.1:9222/devtools/page/"));
    }

    #[test]
    fn cdp_server_json_url_format() {
        let server = CDPServer::new(9222);
        assert_eq!(server.json_url(), "http://127.0.0.1:9222/json");
    }

    #[test]
    fn cdp_server_with_bridge() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(std::time::Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        assert_eq!(server.port(), 9333);
    }

    #[test]
    fn cdp_server_event_sender() {
        let server = CDPServer::new(9222);
        let _tx = server.event_sender();
    }

    #[test]
    fn cdp_server_error_display_bind() {
        let err = CDPServerError::Bind("port in use".into());
        assert!(err.to_string().contains("Bind error"));
        assert!(err.to_string().contains("port in use"));
    }

    #[test]
    fn cdp_server_error_display_io() {
        let err = CDPServerError::Io("broken pipe".into());
        assert!(err.to_string().contains("IO error"));
    }

    #[test]
    fn cdp_server_error_display_ws() {
        let err = CDPServerError::WebSocket("handshake failed".into());
        assert!(err.to_string().contains("WebSocket error"));
    }

    #[test]
    fn cdp_server_error_display_protocol() {
        let err = CDPServerError::Protocol("invalid frame".into());
        assert!(err.to_string().contains("Protocol error"));
    }

    #[test]
    fn cdp_command_send_event() {
        let server = CDPServer::new(9222);
        server.send_event("Page.loadEventFired", serde_json::json!({"timestamp": 12345.0}));
    }

    #[test]
    fn cdp_command_shutdown() {
        let server = CDPServer::new(9222);
        server.shutdown();
    }

    #[test]
    fn rand_id_is_nonzero() {
        let id = rand_id();
        assert_ne!(id, 0);
    }

    #[test]
    fn cdp_server_error_is_std_error() {
        let err = CDPServerError::Bind("test".into());
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn rand_id_unique() {
        assert_ne!(rand_id(), rand_id(), "two consecutive rand_id calls should differ");
    }
}
