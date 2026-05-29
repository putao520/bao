// REQ-CDP-004: CDP backend abstraction (internal/external)
use serde_json::Value;

use crate::protocol::CDPError;

pub trait CdpBackend: Send + Sync {
    fn send_command(
        &self,
        method: &str,
        params: &Option<Value>,
        target_id: &str,
    ) -> Result<Value, CDPError>;
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
        params: &Option<Value>,
        target_id: &str,
    ) -> Result<Value, CDPError> {
        let msg = crate::protocol::CDPMessage {
            id: 0,
            method: method.to_string(),
            params: params.clone(),
            session_id: None,
        };
        let response = crate::protocol::handle_command(msg, target_id, params);
        match (response.result, response.error) {
            (Some(result), _) => Ok(result),
            (None, Some(err)) => Err(err),
            (None, None) => Ok(serde_json::json!({})),
        }
    }
}

pub struct ExternalBackend {
    endpoint: String,
    stream: std::sync::Mutex<Option<std::net::TcpStream>>,
}

impl ExternalBackend {
    pub fn new(endpoint: &str) -> Result<Self, CDPError> {
        Ok(ExternalBackend {
            endpoint: endpoint.to_string(),
            stream: std::sync::Mutex::new(None),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn ensure_connected(&self) -> Result<(), CDPError> {
        let mut guard = self.stream.lock().map_err(|_| CDPError {
            code: -32603,
            message: "lock poisoned".into(),
        })?;

        if guard.is_none() {
            let ws_url = self.endpoint.trim_start_matches("ws://");
            let stream = std::net::TcpStream::connect(ws_url).map_err(|e| CDPError {
                code: -32603,
                message: format!("connect failed: {e}"),
            })?;
            stream.set_nonblocking(true).map_err(|e| CDPError {
                code: -32603,
                message: format!("nonblocking failed: {e}"),
            })?;
            *guard = Some(stream);
        }
        Ok(())
    }
}

impl CdpBackend for ExternalBackend {
    fn send_command(
        &self,
        method: &str,
        params: &Option<Value>,
        _target_id: &str,
    ) -> Result<Value, CDPError> {
        self.ensure_connected()?;

        let mut guard = self.stream.lock().map_err(|_| CDPError {
            code: -32603,
            message: "lock poisoned".into(),
        })?;

        let stream = guard.as_mut().ok_or_else(|| CDPError {
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

        crate::ws::write_message(stream, &msg_str).map_err(|_| CDPError {
            code: -32603,
            message: "websocket write failed".into(),
        })?;

        for _ in 0..100 {
            match crate::ws::read_message(stream) {
                Ok(Some(response_str)) => {
                    let resp: Value =
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
