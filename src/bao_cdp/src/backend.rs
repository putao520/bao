// REQ-CDP-004: CDP backend abstraction (internal/external)  @trace REQ-CDP-001
// @trace REQ-CDP-004 [entity:InternalBackend,ExternalBackend]
// @trace REQ-LIB-003
//
// REQ-CDP-UWS-001: ExternalBackend now uses `bun_uws::ws_client::WebSocketClient`
// (full RFC 6455 client handshake + masking) instead of the previous
// `crate::WebSocketConnection` + hand-rolled inline handshake.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bun_uws::ws_client::{RecvOutcome, WebSocketClient, WsClientError};

use crate::protocol::CdpError;

pub trait CdpBackend: Send + Sync {
    fn send_command(
        &self,
        method: &str,
        params: &Option<serde_json::Value>,
        target_id: &str,
    ) -> Result<serde_json::Value, CdpError>;
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
    ) -> Result<serde_json::Value, CdpError> {
        let msg = crate::protocol::CdpMessage {
            id: Some(0),
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

/// Map a [`WsClientError`] to the CDP error code set.
fn ws_err_to_cdp(e: WsClientError) -> CdpError {
    let (code, message) = match e {
        WsClientError::InvalidUrl => (-32602, "invalid ws URL".to_string()),
        WsClientError::Connect(io) => (-32603, format!("connect failed: {io}")),
        WsClientError::Handshake(h) => (-32603, format!("handshake failed: {h:?}")),
        WsClientError::Io(io) => (-32603, format!("io: {io}")),
        WsClientError::Closed => (-32603, "connection closed".to_string()),
    };
    CdpError { code, message }
}

pub struct ExternalBackend {
    endpoint: String,
    ws: std::sync::Mutex<Option<WebSocketClient>>,
}

impl ExternalBackend {
    pub fn new(endpoint: &str) -> Result<Self, CdpError> {
        Ok(ExternalBackend {
            endpoint: endpoint.to_string(),
            ws: std::sync::Mutex::new(None),
        })
    }

    fn ensure_connected(&self) -> Result<(), CdpError> {
        let mut guard = self.ws.lock().map_err(|_| CdpError {
            code: -32603,
            message: "lock poisoned".into(),
        })?;

        if guard.is_none() {
            // bun_uws::ws_client::WebSocketClient performs the full RFC 6455
            // client handshake (Sec-WebSocket-Key + SHA1 GUID verification) and
            // wires up masking on every outbound frame.
            let client = WebSocketClient::connect(&self.endpoint).map_err(ws_err_to_cdp)?;
            *guard = Some(client);
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
    ) -> Result<serde_json::Value, CdpError> {
        self.ensure_connected()?;

        let mut guard = self.ws.lock().map_err(|_| CdpError {
            code: -32603,
            message: "lock poisoned".into(),
        })?;

        let ws = guard.as_mut().ok_or_else(|| CdpError {
            code: -32603,
            message: "not connected".into(),
        })?;

        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let mut msg_obj = serde_json::json!({
            "id": id,
            "method": method,
        });
        if let Some(p) = params {
            msg_obj["params"] = p.clone();
        }
        let msg_str = serde_json::to_string(&msg_obj).map_err(|e| CdpError {
            code: -32700,
            message: format!("serialize error: {e}"),
        })?;

        ws.send_text(&msg_str).map_err(ws_err_to_cdp)?;

        // Poll for the matching response id (up to ~1s at 10ms cadence).
        let deadline = SystemTime::now() + Duration::from_secs(1);
        while SystemTime::now() < deadline {
            match ws.recv().map_err(ws_err_to_cdp)? {
                RecvOutcome::Message(_op, payload) => {
                    let resp: serde_json::Value =
                        serde_json::from_slice(&payload).map_err(|e| CdpError {
                            code: -32700,
                            message: format!("parse error: {e}"),
                        })?;
                    if resp.get("id").and_then(|v| v.as_i64()) == Some(id) {
                        if let Some(error) = resp.get("error") {
                            return Err(CdpError {
                                code: error["code"].as_i64().unwrap_or(-32603),
                                message: error["message"]
                                    .as_str()
                                    .unwrap_or("unknown error")
                                    .into(),
                            });
                        }
                        return Ok(resp.get("result").cloned().unwrap_or(serde_json::json!({})));
                    }
                    // Different id or an event — keep polling.
                }
                RecvOutcome::Timeout => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                RecvOutcome::Closed => {
                    return Err(CdpError {
                        code: -32603,
                        message: "connection closed".into(),
                    });
                }
            }
        }

        Err(CdpError {
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
        let result = backend
            .send_command("Page.enable", &None, "test-target")
            .unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 3. Runtime.enable returns ok empty (Chrome semantics: no
    //    executionContextId in the enable response)
    #[test]
    fn internal_backend_send_command_runtime_enable_returns_ok_empty() {
        let backend = InternalBackend::new();
        let result = backend
            .send_command("Runtime.enable", &None, "test-target")
            .unwrap();
        assert!(result.get("executionContextId").is_none());
        assert_eq!(result, serde_json::json!({}));
    }

    // 4. DOM.getDocument returns ok with root node
    #[test]
    fn internal_backend_send_command_dom_get_document_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend
            .send_command("DOM.getDocument", &None, "test-target")
            .unwrap();
        assert!(result.get("root").is_some());
        assert_eq!(result["root"]["nodeId"], 1);
        assert_eq!(result["root"]["nodeType"], 9);
    }

    // 5. Network.enable returns ok
    #[test]
    fn internal_backend_send_command_network_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend
            .send_command("Network.enable", &None, "test-target")
            .unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 6. Debugger.enable returns ok
    #[test]
    fn internal_backend_send_command_debugger_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend
            .send_command("Debugger.enable", &None, "test-target")
            .unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 7. Unknown domain/command returns error code -32601
    #[test]
    fn internal_backend_send_command_unknown_returns_error_32601() {
        let backend = InternalBackend::new();
        let err = backend
            .send_command("Foo.bar", &None, "test-target")
            .unwrap_err();
        assert_eq!(err.code, -32601);
    }

    // 8. Page.getLayoutMetrics without a servo bridge → explicit error
    //    (real layout dimensions require the live document; never 1920×1080)
    #[test]
    fn internal_backend_send_command_page_get_layout_metrics_returns_dimensions() {
        let backend = InternalBackend::new();
        let err = backend
            .send_command("Page.getLayoutMetrics", &None, "test-target")
            .unwrap_err();
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"));
    }

    // 9. target_id is passed through to the command handler
    #[test]
    fn internal_backend_send_command_with_target_id_passed_through() {
        let backend = InternalBackend::new();
        let result = backend
            .send_command("Target.getTargets", &None, "my-custom-target")
            .unwrap();
        let infos = result["targetInfos"].as_array().unwrap();
        assert_eq!(infos[0]["targetId"], "my-custom-target");
    }

    // 10. CSS.enable returns ok
    #[test]
    fn internal_backend_send_command_css_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend
            .send_command("CSS.enable", &None, "test-target")
            .unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 11. Log.enable returns ok
    #[test]
    fn internal_backend_send_command_log_enable_returns_ok() {
        let backend = InternalBackend::new();
        let result = backend
            .send_command("Log.enable", &None, "test-target")
            .unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    // 12. Fetch.enable → explicit error (no request interception facility —
    //     never a canned enabled/patternCount success)
    #[test]
    fn internal_backend_send_command_fetch_enable_returns_ok() {
        let backend = InternalBackend::new();
        let params = Some(serde_json::json!({"patterns": [{"urlPattern": "*"}]}));
        let err = backend
            .send_command("Fetch.enable", &params, "test-target")
            .unwrap_err();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
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
