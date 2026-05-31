// @trace REQ-CDS-003 [entity:CdpSessionGeneric] [sm:SM-CDP-SESSION]
// CDP Session lifecycle management.

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::net::TcpStream;

use tungstenite::protocol::WebSocket;

use crate::protocol::{self, CdpMessage, CdpResponse, SessionError};
use crate::registry::SharedRegistry;
use crate::EventSender;

/// Session lifecycle states (SM-CDP-SESSION).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Created,
    Active,
    Closing,
    Closed,
}

/// ReplayStream replays pre-read bytes to tungstenite on first reads.
pub struct ReplayStream {
    stream: TcpStream,
    replay: Cursor<Vec<u8>>,
}

impl ReplayStream {
    pub fn new(stream: TcpStream, peeked: Vec<u8>) -> Self {
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

/// CDP client session. Holds a WebSocket connection and tracks enabled domains.
pub struct CdpSession {
    session_id: String,
    target_id: String,
    ws: WebSocket<ReplayStream>,
    enabled_domains: HashSet<String>,
    state: SessionState,
    is_browser_session: bool,
    first_domain_enabled: HashSet<String>,
}

impl CdpSession {
    pub fn new(
        session_id: String,
        target_id: String,
        ws: WebSocket<ReplayStream>,
        is_browser_session: bool,
    ) -> Self {
        CdpSession {
            session_id,
            target_id,
            ws,
            enabled_domains: HashSet::new(),
            state: SessionState::Created,
            is_browser_session,
            first_domain_enabled: HashSet::new(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn is_browser_session(&self) -> bool {
        self.is_browser_session
    }

    pub fn has_domain_enabled(&self, domain: &str) -> bool {
        self.enabled_domains.contains(domain)
    }

    /// Process one incoming WebSocket message. Returns Err on disconnect.
    pub fn process(
        &mut self,
        registry: &SharedRegistry,
        event_sender: &dyn EventSender,
    ) -> Result<(), SessionError> {
        let msg = match read_ws_message(&mut self.ws) {
            Ok(Some(msg)) => msg,
            Ok(None) => return Ok(()),
            Err(e) => {
                self.state = SessionState::Closing;
                return Err(e);
            }
        };

        let cdp_msg: CdpMessage = match protocol::parse_message(&msg) {
            Some(m) => m,
            None => {
                let resp = protocol::error_response(
                    None,
                    protocol::ERR_INVALID_REQUEST,
                    "Invalid JSON",
                );
                let _ = self.send_text(&protocol::serialize_response(&resp));
                return Ok(());
            }
        };

        let response = self.route_command(cdp_msg, registry, event_sender);
        let _ = self.send_text(&protocol::serialize_response(&response));
        Ok(())
    }

    /// Route a CDP command: handle enable/disable internally, dispatch
    /// everything else to DomainRegistry.
    fn route_command(
        &mut self,
        msg: CdpMessage,
        registry: &SharedRegistry,
        event_sender: &dyn EventSender,
    ) -> CdpResponse {
        let parts: Vec<&str> = msg.method.splitn(2, '.').collect();
        let domain = parts.first().copied().unwrap_or("");
        let command = parts.get(1).copied().unwrap_or("");

        // Domain.enable / Domain.disable are handled internally.
        match command {
            "enable" => {
                self.enabled_domains.insert(domain.to_string());
                if self.state == SessionState::Created {
                    self.state = SessionState::Active;
                }
                // Notify handler on first enable for this domain in this session.
                if !self.first_domain_enabled.contains(domain) {
                    self.first_domain_enabled.insert(domain.to_string());
                    registry.notify_session_created(domain, &self.session_id);
                }
                return protocol::ok_empty(msg.id);
            }
            "disable" => {
                self.enabled_domains.remove(domain);
                return protocol::ok_empty(msg.id);
            }
            _ => {}
        }

        // Dispatch to DomainHandler.
        match registry.dispatch_command(&msg.method, msg.params.unwrap_or_default(), event_sender) {
            Some(Ok(result)) => protocol::ok_response(msg.id, result),
            Some(Err(err)) => CdpResponse {
                id: msg.id,
                result: None,
                error: Some(err),
            },
            None => protocol::error_response(
                msg.id,
                protocol::ERR_METHOD_NOT_FOUND,
                format!("'{}' wasn't found", msg.method),
            ),
        }
    }

    /// Send raw text over WebSocket.
    pub fn send_text(&mut self, data: &str) -> Result<(), SessionError> {
        use tungstenite::Message;
        self.ws.send(Message::Text(data.into())).map_err(|_| SessionError::Io)
    }

    /// Get all enabled domain names (for on_session_destroyed notification).
    pub fn enabled_domains(&self) -> Vec<String> {
        self.enabled_domains.iter().cloned().collect()
    }

    /// Transition to Closing state.
    pub fn begin_close(&mut self) {
        self.state = SessionState::Closing;
    }

    /// Transition to Closed state.
    pub fn finalize(&mut self) {
        self.state = SessionState::Closed;
    }
}

fn read_ws_message(ws: &mut WebSocket<ReplayStream>) -> Result<Option<String>, SessionError> {
    use tungstenite::Message;
    match ws.read() {
        Ok(Message::Text(text)) => Ok(Some(text.to_string())),
        Ok(Message::Binary(data)) => Ok(Some(String::from_utf8_lossy(&data).into_owned())),
        Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => Ok(None),
        Ok(Message::Close(_)) => Err(SessionError::Closed),
        Ok(Message::Frame(_)) => Ok(None),
        Err(_) => Err(SessionError::Io),
    }
}

// @trace REQ-CDS-003 [req:REQ-CDS-003] [level:unit]
#[cfg(test)]
mod tests {
    use super::SessionState;

    #[test]
    fn session_state_equality_same_variant() {
        assert_eq!(SessionState::Created, SessionState::Created);
    }

    #[test]
    fn session_state_equality_different_variants() {
        assert_ne!(SessionState::Created, SessionState::Closed);
    }

    #[test]
    fn session_state_clone() {
        let original = SessionState::Active;
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn session_state_copy() {
        let original = SessionState::Closing;
        let copied = original; // Copy, not move
        assert_eq!(original, copied);
    }

    #[test]
    fn session_state_debug_format() {
        assert!(format!("{:?}", SessionState::Created).contains("Created"));
        assert!(format!("{:?}", SessionState::Active).contains("Active"));
        assert!(format!("{:?}", SessionState::Closing).contains("Closing"));
        assert!(format!("{:?}", SessionState::Closed).contains("Closed"));
    }

    #[test]
    fn session_state_all_variants_distinct() {
        let variants = [
            SessionState::Created,
            SessionState::Active,
            SessionState::Closing,
            SessionState::Closed,
        ];
        for i in 0..variants.len() {
            for j in (i + 1)..variants.len() {
                assert_ne!(variants[i], variants[j]);
            }
        }
    }

    #[test]
    fn session_state_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<SessionState>();
        assert_sync::<SessionState>();
    }
}
