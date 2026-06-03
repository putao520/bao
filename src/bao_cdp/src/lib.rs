// REQ-CDP-003: CDP module public API and domain registry  @trace REQ-CDP-001 [entity:CdpServer]
// @trace REQ-IMPL-06
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use tungstenite::accept;
use tungstenite::protocol::WebSocket;

/// EventSender that discards all events. Used during command dispatch
/// since domain handlers currently ignore the event_sender parameter.
struct NoopEventSender;
impl cdp_server::EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: serde_json::Value) {}
}

mod ws;
mod protocol;
mod backend;
mod router;
pub mod servo_bridge;
pub mod domains;

pub use protocol::{CDPMessage, CDPResponse, CDPError, CDPEvent};
pub use protocol::{parse_message, handle_command, serialize_response, serialize_event};
pub use router::{CdpRouter, CdpSession, ExternalBrowser, BackendKind};
pub use servo_bridge::{BridgeSender, BridgeReceiver, BridgeCommand, BridgeResponse, bridge_channel};

// cdp-server integration — new domain-handler architecture
pub use cdp_server::{CdpServer, ServerConfig, DomainRegistry, EventBroadcaster};

pub struct CDPServer {
    port: u16,
    target_id: String,
    sessions: HashMap<String, CDPSession>,
    cmd_tx: Sender<CDPCommand>,
    cmd_rx: Receiver<CDPCommand>,
    bridge: Option<BridgeSender>,
    registry: Option<Arc<DomainRegistry>>,
}

pub enum CDPCommand {
    SendEvent(CDPEvent),
    Shutdown,
}

pub struct CDPSession {
    id: String,
    target_id: String,
    ws: WebSocket<ReplayStream>,
    bridge: Option<BridgeSender>,
    registry: Option<Arc<DomainRegistry>>,
}

/// Wraps a TcpStream with pre-read bytes, replaying them on the first reads
/// so tungstenite sees the full HTTP upgrade request.
struct ReplayStream {
    stream: TcpStream,
    replay: Cursor<Vec<u8>>,
}

impl ReplayStream {
    fn new(stream: TcpStream, peeked: Vec<u8>) -> Self {
        ReplayStream {
            stream,
            replay: Cursor::new(peeked),
        }
    }
}

impl Read for ReplayStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.replay.position() < self.replay.get_ref().len() as u64 {
            return self.replay.read(buf);
        }
        self.stream.read(buf)
    }
}

impl Write for ReplayStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
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
            registry: None,
        }
    }

    pub fn with_bridge(port: u16, bridge: BridgeSender) -> Self {
        let (cmd_tx, cmd_rx) = channel();
        let target_id = format!("{:016x}", rand_id());
        let registry = DomainRegistry::new();
        domains::register_all_domains_with_target(bridge.clone(), target_id.clone(), &registry);
        CDPServer {
            port,
            target_id,
            sessions: HashMap::new(),
            cmd_tx,
            cmd_rx,
            bridge: Some(bridge),
            registry: Some(Arc::new(registry)),
        }
    }

    pub fn port(&self) -> u16 { self.port }
    pub fn target_id(&self) -> &str { &self.target_id }

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

        eprintln!("CDP listening on ws://127.0.0.1:{}", self.port);
        eprintln!("DevTools: {}", self.ws_url());

        loop {
            // Drain command channel without dropping Shutdown.
            // The previous `while let Ok(SendEvent)` pattern consumed *any* message
            // (including Shutdown) and silently dropped it when the pattern didn't match,
            // making graceful shutdown impossible.
            loop {
                match self.cmd_rx.try_recv() {
                    Ok(CDPCommand::SendEvent(ev)) => self.broadcast_event(&ev),
                    Ok(CDPCommand::Shutdown) => {
                        eprintln!("[server] run loop exiting");
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
                Err(e) => eprintln!("CDP accept error: {}", e),
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

        if request.starts_with("GET /json") {
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

        // WebSocket upgrade — replay already-read bytes to tungstenite
        if request.starts_with("GET /devtools/page/") {
            // Set short read timeout so session.process() doesn't block the event loop.
            // The server is single-threaded; without a timeout, ws.read() would hang
            // forever waiting for data, freezing accept() and Shutdown handling.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(1000)));
            let replay = ReplayStream::new(stream, buf[..n].to_vec());
            match accept(replay) {
                Ok(ws) => {
                    return Some(CDPSession {
                        id: format!("{:016x}", rand_id()),
                        target_id: self.target_id.clone(),
                        ws,
                        bridge: self.bridge.clone(),
                        registry: self.registry.clone(),
                    });
                }
                Err(e) => {
                    eprintln!("CDP WebSocket accept error: {}", e);
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
        let msg = match ws::read_message(&mut self.ws) {
            Ok(Some(msg)) => msg,
            Ok(None) => return Ok(()),
            Err(_) => return Err(()),
        };

        let cdp_msg: CDPMessage = match protocol::parse_message(&msg) {
            Some(m) => m,
            None => return Ok(()),
        };

        let response = match self.dispatch_cdp(&cdp_msg) {
            Some(resp) => resp,
            None => protocol::handle_command(
                cdp_msg.clone(), &self.target_id, &cdp_msg.params, self.bridge.as_ref(),
            ),
        };
        let response_json = protocol::serialize_response(&response);
        let _ = ws::write_message(&mut self.ws, &response_json);

        Ok(())
    }

    /// Try dispatching via DomainRegistry. Returns Some(CDPResponse) if the domain
    /// was found in the registry, None to fall back to old protocol routing.
    fn dispatch_cdp(&self, cdp_msg: &CDPMessage) -> Option<CDPResponse> {
        let registry = self.registry.as_ref()?;
        let params = cdp_msg.params.clone().unwrap_or_default();
        registry.dispatch_command(&cdp_msg.method, params, &NoopEventSender).map(|result| {
            match result {
                Ok(value) => CDPResponse {
                    id: cdp_msg.id,
                    result: Some(value),
                    error: None,
                },
                Err(err) => CDPResponse {
                    id: cdp_msg.id,
                    result: None,
                    error: Some(CDPError { code: err.code, message: err.message }),
                },
            }
        })
    }

    #[allow(clippy::result_unit_err)]
    pub fn send_event(&mut self, ev: &CDPEvent) -> Result<(), ()> {
        let json = protocol::serialize_event(ev);
        ws::write_message(&mut self.ws, &json)
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
    use std::time::Duration;

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
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
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

    // --- Registry integration tests (Wave 2) ---

    #[test]
    fn cdp_server_new_has_no_registry() {
        let server = CDPServer::new(9222);
        assert!(server.registry.is_none());
    }

    #[test]
    fn cdp_server_with_bridge_has_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        assert!(server.registry.is_some());
        let registry = server.registry.as_ref().unwrap();
        assert!(registry.has_domain("Page"));
        assert!(registry.has_domain("Runtime"));
        assert!(registry.has_domain("DOM"));
        assert!(registry.has_domain("Network"));
    }

    #[test]
    fn cdp_server_with_bridge_registry_has_all_12_domains() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();
        let expected = [
            "Page", "Runtime", "DOM", "Network", "Debugger",
            "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch", "Target",
        ];
        for domain in &expected {
            assert!(registry.has_domain(domain), "domain '{}' should be registered", domain);
        }
    }

    #[test]
    fn cdp_server_with_bridge_target_is_in_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();
        assert!(registry.has_domain("Target"), "Target should be in registry");
    }

    #[test]
    fn dispatch_cdp_with_registry_returns_page_enable() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let cdp_msg = CDPMessage {
            id: 1,
            method: "Page.enable".into(),
            params: None,
            session_id: None,
        };

        // Simulate what dispatch_cdp does
        let params = cdp_msg.params.clone().unwrap_or_default();
        let result = registry.dispatch_command(&cdp_msg.method, params, &NoopEventSender);
        assert!(result.is_some());
        let response = result.unwrap();
        assert!(response.is_ok());
        assert_eq!(response.unwrap(), serde_json::json!({}));
    }

    #[test]
    fn dispatch_cdp_with_registry_returns_runtime_enable() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("Runtime.enable", serde_json::json!({}), &NoopEventSender);
        assert!(result.is_some());
        let response = result.unwrap();
        assert!(response.is_ok());
        assert_eq!(response.unwrap()["executionContextId"], 1);
    }

    #[test]
    fn dispatch_cdp_unregistered_domain_returns_none() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        // HeapProfiler is NOT registered — should return None
        let result = registry.dispatch_command("HeapProfiler.takeHeapSnapshot", serde_json::json!({}), &NoopEventSender);
        assert!(result.is_none());
    }

    #[test]
    fn dispatch_cdp_unknown_method_returns_error() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("Page.nonExistentMethod", serde_json::json!({}), &NoopEventSender);
        assert!(result.is_some());
        let err = result.unwrap().unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn dispatch_cdp_page_get_layout_metrics_via_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("Page.getLayoutMetrics", serde_json::json!({}), &NoopEventSender);
        assert!(result.is_some());
        let response = result.unwrap().unwrap();
        assert_eq!(response["contentSize"]["width"], 1920);
        assert_eq!(response["contentSize"]["height"], 1080);
    }

    #[test]
    fn dispatch_cdp_network_enable_via_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("Network.enable", serde_json::json!({}), &NoopEventSender);
        assert!(result.is_some());
        assert_eq!(result.unwrap().unwrap(), serde_json::json!({}));
    }

    #[test]
    fn dispatch_cdp_css_stub_via_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("CSS.enable", serde_json::json!({}), &NoopEventSender);
        assert!(result.is_some());
        assert_eq!(result.unwrap().unwrap(), serde_json::json!({}));
    }

    #[test]
    fn dispatch_cdp_fetch_enable_via_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command(
            "Fetch.enable",
            serde_json::json!({"patterns": [{"urlPattern": "*"}]}),
            &NoopEventSender,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().unwrap()["patternCount"], 1);
    }

    #[test]
    fn dispatch_target_get_targets_via_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("Target.getTargets", serde_json::json!({}), &NoopEventSender);
        assert!(result.is_some());
        let response = result.unwrap();
        assert!(response.is_ok());
        let result_val = response.unwrap();
        let infos = result_val["targetInfos"].as_array().unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0]["targetId"], server.target_id());
    }

    #[test]
    fn dispatch_target_create_target_via_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("Target.createTarget", serde_json::json!({ "url": "https://example.com" }), &NoopEventSender);
        assert!(result.is_some());
        let response = result.unwrap();
        assert!(response.is_ok());
        assert_eq!(response.unwrap()["targetId"], server.target_id());
    }

    #[test]
    fn dispatch_target_close_target_via_registry() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("Target.closeTarget", serde_json::json!({ "targetId": "abc" }), &NoopEventSender);
        assert!(result.is_some());
        let response = result.unwrap();
        assert!(response.is_ok());
        assert_eq!(response.unwrap()["success"], true);
    }

    #[test]
    fn dispatch_target_unknown_returns_error() {
        let (sender, _rx) = crate::servo_bridge::bridge_channel(Duration::from_millis(100));
        let server = CDPServer::with_bridge(9333, sender);
        let registry = server.registry.as_ref().unwrap();

        let result = registry.dispatch_command("Target.nonExistent", serde_json::json!({}), &NoopEventSender);
        assert!(result.is_some());
        let err = result.unwrap().unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn cdp_error_conversion_between_types() {
        // Verify cdp_server::CdpError → protocol::CDPError conversion
        let server_err = cdp_server::CdpError { code: -32601, message: "not found".into() };
        let protocol_err = CDPError { code: server_err.code, message: server_err.message.clone() };
        assert_eq!(protocol_err.code, -32601);
        assert_eq!(protocol_err.message, "not found");
    }

    #[test]
    fn rand_id_unique() {
        assert_ne!(rand_id(), rand_id(), "two consecutive rand_id calls should differ");
    }
}
