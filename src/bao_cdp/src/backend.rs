// REQ-CDP-004: CDP backend abstraction (internal/external)  @trace REQ-CDP-001
use std::net::TcpStream;

use tungstenite::client;
use tungstenite::protocol::WebSocket;

use crate::protocol::CDPError;

pub trait CdpBackend: Send + Sync {
    fn send_command(
        &self,
        method: &str,
        params: &Option<serde_json::Value>,
        target_id: &str,
    ) -> Result<serde_json::Value, CDPError>;
}

pub struct InternalBackend;

impl InternalBackend {
    pub fn new() -> Self {
        InternalBackend
    }
}

impl CdpBackend for InternalBackend {
    fn send_command(
        &self,
        method: &str,
        params: &Option<serde_json::Value>,
        target_id: &str,
    ) -> Result<serde_json::Value, CDPError> {
        let msg = crate::protocol::CDPMessage {
            id: 0,
            method: method.to_string(),
            params: params.clone(),
            session_id: None,
        };
        let response = crate::protocol::handle_command(msg, target_id, params, None);
        match (response.result, response.error) {
            (Some(result), _) => Ok(result),
            (None, Some(err)) => Err(err),
            (None, None) => Ok(serde_json::json!({})),
        }
    }
}

pub struct ExternalBackend {
    endpoint: String,
    ws: std::sync::Mutex<Option<WebSocket<TcpStream>>>,
}

impl ExternalBackend {
    pub fn new(endpoint: &str) -> Result<Self, CDPError> {
        Ok(ExternalBackend {
            endpoint: endpoint.to_string(),
            ws: std::sync::Mutex::new(None),
        })
    }

    fn ensure_connected(&self) -> Result<(), CDPError> {
        let mut guard = self.ws.lock().map_err(|_| CDPError {
            code: -32603,
            message: "lock poisoned".into(),
        })?;

        if guard.is_none() {
            let tcp_url = self.endpoint.trim_start_matches("ws://");
            let stream = TcpStream::connect(tcp_url).map_err(|e| CDPError {
                code: -32603,
                message: format!("connect failed: {e}"),
            })?;
            stream.set_nonblocking(false).map_err(|e| CDPError {
                code: -32603,
                message: format!("nonblocking failed: {e}"),
            })?;

            let (websocket, _response) = client(&self.endpoint, stream).map_err(|e| CDPError {
                code: -32603,
                message: format!("websocket handshake failed: {e}"),
            })?;
            *guard = Some(websocket);
        }
        Ok(())
    }
}

impl CdpBackend for ExternalBackend {
    fn send_command(
        &self,
        method: &str,
        params: &Option<serde_json::Value>,
        _target_id: &str,
    ) -> Result<serde_json::Value, CDPError> {
        self.ensure_connected()?;

        let mut guard = self.ws.lock().map_err(|_| CDPError {
            code: -32603,
            message: "lock poisoned".into(),
        })?;

        let ws = guard.as_mut().ok_or_else(|| CDPError {
            code: -32603,
            message: "not connected".into(),
        })?;

        let id = {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64
        };

        let mut msg_obj = serde_json::json!({
            "id": id,
            "method": method,
        });
        if let Some(p) = params {
            msg_obj["params"] = p.clone();
        }
        let msg_str = serde_json::to_string(&msg_obj).map_err(|e| CDPError {
            code: -32700,
            message: format!("serialize error: {e}"),
        })?;

        crate::ws::write_message(ws, &msg_str).map_err(|_| CDPError {
            code: -32603,
            message: "websocket write failed".into(),
        })?;

        for _ in 0..100 {
            match crate::ws::read_message(ws) {
                Ok(Some(response_str)) => {
                    let resp: serde_json::Value =
                        serde_json::from_str(&response_str).map_err(|e| CDPError {
                            code: -32700,
                            message: format!("parse error: {e}"),
                        })?;
                    if resp.get("id").and_then(|v| v.as_i64()) == Some(id) {
                        if let Some(error) = resp.get("error") {
                            return Err(CDPError {
                                code: error["code"].as_i64().unwrap_or(-32603),
                                message: error["message"]
                                    .as_str()
                                    .unwrap_or("unknown error")
                                    .into(),
                            });
                        }
                        return Ok(resp.get("result").cloned().unwrap_or(serde_json::json!({})));
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }

        Err(CDPError {
            code: -32603,
            message: "response timeout".into(),
        })
    }
}
