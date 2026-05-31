// REQ-CDP-004: CDP backend abstraction (internal/external)  @trace REQ-CDP-001
// @trace REQ-LIB-003
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

// @trace TEST-CDP-004 [req:REQ-CDP-001] [level:unit] [nfr:TMG-CDP-01]
#[cfg(test)]
mod tests {
    use super::*;

    // 1. InternalBackend::new() constructs without panic
    #[test]
    fn internal_backend_new_creates_without_panic() {
        let _backend = InternalBackend::new();
    }

    // 2. Page.enable via InternalBackend returns ok
    #[test]
    fn internal_backend_send_command_page_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend.send_command("Page.enable", &None, "test-target").unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 3. Runtime.enable returns ok with executionContextId
    #[test]
    fn internal_backend_send_command_runtime_enable_returns_ok_with_execution_context_id() {
        let backend = InternalBackend::new();
        let result = backend.send_command("Runtime.enable", &None, "test-target").unwrap();
        assert!(result.get("executionContextId").is_some());
        assert_eq!(result["executionContextId"], 1);
    }

    // 4. DOM.getDocument returns ok with root node
    #[test]
    fn internal_backend_send_command_dom_get_document_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend.send_command("DOM.getDocument", &None, "test-target").unwrap();
        assert!(result.get("root").is_some());
        assert_eq!(result["root"]["nodeId"], 1);
        assert_eq!(result["root"]["nodeType"], 9);
    }

    // 5. Network.enable returns ok
    #[test]
    fn internal_backend_send_command_network_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend.send_command("Network.enable", &None, "test-target").unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 6. Debugger.enable returns ok
    #[test]
    fn internal_backend_send_command_debugger_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend.send_command("Debugger.enable", &None, "test-target").unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 7. Unknown domain/command returns error code -32601
    #[test]
    fn internal_backend_send_command_unknown_returns_error_32601() {
        let backend = InternalBackend::new();
        let err = backend.send_command("Foo.bar", &None, "test-target").unwrap_err();
        assert_eq!(err.code, -32601);
    }

    // 8. Page.getLayoutMetrics returns dimensions
    #[test]
    fn internal_backend_send_command_page_get_layout_metrics_returns_dimensions() {
        let backend = InternalBackend::new();
        let result = backend.send_command("Page.getLayoutMetrics", &None, "test-target").unwrap();
        assert!(result.get("contentSize").is_some());
        assert_eq!(result["contentSize"]["width"], 1920);
        assert_eq!(result["contentSize"]["height"], 1080);
    }

    // 9. target_id is passed through to the command handler
    #[test]
    fn internal_backend_send_command_with_target_id_passed_through() {
        let backend = InternalBackend::new();
        let result = backend.send_command("Target.getTargets", &None, "my-custom-target").unwrap();
        let infos = result["targetInfos"].as_array().unwrap();
        assert_eq!(infos[0]["targetId"], "my-custom-target");
    }

    // 10. CSS.enable returns ok
    #[test]
    fn internal_backend_send_command_css_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend.send_command("CSS.enable", &None, "test-target").unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 11. Log.enable returns ok
    #[test]
    fn internal_backend_send_command_log_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend.send_command("Log.enable", &None, "test-target").unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 12. Fetch.enable returns ok with patternCount
    #[test]
    fn internal_backend_send_command_fetch_enable_returns_ok() {
        let backend = InternalBackend::new();
        let params = Some(serde_json::json!({"patterns": [{"urlPattern": "*"}]}));
        let result = backend.send_command("Fetch.enable", &params, "test-target").unwrap();
        assert_eq!(result["patternCount"], 1);
    }

    // 13. ExternalBackend::new with invalid endpoint still constructs (connects lazily)
    #[test]
    fn external_backend_new_with_invalid_endpoint_still_constructs() {
        let backend = ExternalBackend::new("ws://127.0.0.1:1").unwrap();
        assert_eq!(backend.endpoint, "ws://127.0.0.1:1");
    }

    // 14. InternalBackend is Send + Sync (CdpBackend trait requirement)
    #[test]
    fn internal_backend_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<InternalBackend>();
    }
}
