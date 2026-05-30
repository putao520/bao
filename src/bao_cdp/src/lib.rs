// REQ-CDP-003: CDP module public API and domain registry  @trace REQ-CDP-001 [entity:CdpServer]
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};

use tungstenite::accept;
use tungstenite::protocol::WebSocket;

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

    pub fn run(&mut self) -> Result<(), CDPServerError> {
        let listener = TcpListener::bind(("127.0.0.1", self.port))
            .map_err(|e| CDPServerError::Bind(e.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| CDPServerError::Io(e.to_string()))?;

        eprintln!("CDP listening on ws://127.0.0.1:{}", self.port);
        eprintln!("DevTools: {}", self.ws_url());

        loop {
            while let Ok(CDPCommand::SendEvent(ev)) = self.cmd_rx.try_recv() {
                self.broadcast_event(&ev);
            }
            if let Ok(CDPCommand::Shutdown) = self.cmd_rx.try_recv() {
                break;
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

        Ok(())
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
            let replay = ReplayStream::new(stream, buf[..n].to_vec());
            match accept(replay) {
                Ok(ws) => {
                    return Some(CDPSession {
                        id: format!("{:016x}", rand_id()),
                        target_id: self.target_id.clone(),
                        ws,
                        bridge: self.bridge.clone(),
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

        let response = protocol::handle_command(
            cdp_msg.clone(), &self.target_id, &cdp_msg.params, self.bridge.as_ref(),
        );
        let response_json = protocol::serialize_response(&response);
        let _ = ws::write_message(&mut self.ws, &response_json);

        Ok(())
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
