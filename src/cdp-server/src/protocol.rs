// @trace REQ-CDS-001 [entity:CdpServer] [api:GET /json/version]
// @trace REQ-CDS-004 [entity:DomainRegistry]
// JSON-RPC 2.0 message types and serialization for CDP.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Incoming CDP/JSON-RPC 2.0 request.
#[derive(Debug, Clone, Deserialize)]
pub struct CdpMessage {
    pub id: Option<i64>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Outgoing CDP/JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct CdpResponse {
    pub id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CdpError>,
}

/// CDP error object.
#[derive(Debug, Clone, Serialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
}

/// Session-level I/O error (replaces unit `()` error types).
#[derive(Debug)]
pub enum SessionError {
    /// WebSocket connection closed by peer.
    Closed,
    /// Underlying I/O failure (read/write error, unexpected frame, etc.).
    Io,
}

/// CDP event notification (no id field).
#[derive(Debug, Clone, Serialize)]
pub struct CdpEvent {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

// JSON-RPC 2.0 error codes.
pub const ERR_INVALID_REQUEST: i64 = -32600;
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
#[allow(dead_code)]
pub const ERR_INVALID_PARAMS: i64 = -32602;
#[allow(dead_code)]
pub const ERR_INTERNAL: i64 = -32603;
#[allow(dead_code)]
pub const ERR_PARSE_ERROR: i64 = -32700;

pub fn parse_message(raw: &str) -> Option<CdpMessage> {
    serde_json::from_str(raw).ok()
}

pub fn serialize_response(resp: &CdpResponse) -> String {
    serde_json::to_string(resp)
        .unwrap_or_else(|_| r#"{"id":null,"error":{"code":-32700,"message":"serialize error"}}"#.into())
}

pub fn serialize_event(ev: &CdpEvent) -> String {
    serde_json::to_string(ev).unwrap_or_else(|_| "{}".into())
}

pub fn ok_response(id: Option<i64>, result: Value) -> CdpResponse {
    CdpResponse {
        id,
        result: Some(result),
        error: None,
    }
}

pub fn error_response(id: Option<i64>, code: i64, message: impl Into<String>) -> CdpResponse {
    CdpResponse {
        id,
        result: None,
        error: Some(CdpError {
            code,
            message: message.into(),
        }),
    }
}

pub fn ok_empty(id: Option<i64>) -> CdpResponse {
    ok_response(id, serde_json::json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 1. parse_message valid JSON → Some
    #[test]
    fn parse_valid_json_returns_some() {
        let msg = parse_message(r#"{"id":1,"method":"Page.navigate"}"#);
        assert!(msg.is_some());
        let m = msg.unwrap();
        assert_eq!(m.id, Some(1));
        assert_eq!(m.method, "Page.navigate");
    }

    // 2. parse_message invalid JSON → None
    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_message("{not json}").is_none());
        assert!(parse_message("").is_none());
    }

    // 3. parse_message with null id → id is None
    #[test]
    fn parse_null_id_is_none() {
        let msg = parse_message(r#"{"id":null,"method":"Page.reload"}"#).unwrap();
        assert_eq!(msg.id, None);
    }

    // 4. parse_message with id 0 → id is Some(0)
    #[test]
    fn parse_zero_id_is_some_zero() {
        let msg = parse_message(r#"{"id":0,"method":"Page.reload"}"#).unwrap();
        assert_eq!(msg.id, Some(0));
    }

    // 5. parse_message with session_id
    #[test]
    fn parse_session_id() {
        let msg = parse_message(
            r#"{"id":5,"method":"Runtime.evaluate","session_id":"sess-abc"}"#,
        )
        .unwrap();
        assert_eq!(msg.session_id, Some("sess-abc".into()));
    }

    // 6. parse_message with nested params object
    #[test]
    fn parse_nested_params() {
        let msg = parse_message(
            r#"{"id":2,"method":"Page.navigate","params":{"url":"https://example.com","transitionType":"link"}}"#,
        )
        .unwrap();
        let params = msg.params.unwrap();
        assert_eq!(params["url"], "https://example.com");
        assert_eq!(params["transitionType"], "link");
    }

    // 7. parse_message with array params
    #[test]
    fn parse_array_params() {
        let msg = parse_message(
            r#"{"id":3,"method":"DOM.querySelectorAll","params":["div","span"]}"#,
        )
        .unwrap();
        let params = msg.params.unwrap();
        assert!(params.is_array());
        assert_eq!(params.as_array().unwrap().len(), 2);
    }

    // 8. serialize_response with result only
    #[test]
    fn serialize_response_result_only() {
        let resp = ok_response(Some(1), json!({"frameId": "f1"}));
        let s = serialize_response(&resp);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("result").is_some());
        assert!(v.get("error").is_none());
        assert_eq!(v["id"], 1);
    }

    // 9. serialize_response with error only
    #[test]
    fn serialize_response_error_only() {
        let resp = error_response(Some(2), ERR_METHOD_NOT_FOUND, "not found");
        let s = serialize_response(&resp);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("error").is_some());
        assert!(v.get("result").is_none());
    }

    // 10. serialize_response is valid JSON
    #[test]
    fn serialize_response_produces_valid_json() {
        let resp = ok_response(Some(42), json!({"data": true}));
        let s = serialize_response(&resp);
        assert!(serde_json::from_str::<Value>(&s).is_ok());
    }

    // 11. serialize_event with params
    #[test]
    fn serialize_event_with_params() {
        let ev = CdpEvent {
            method: "Page.frameNavigated".into(),
            params: Some(json!({"frameId": "f1"})),
        };
        let s = serialize_event(&ev);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["method"], "Page.frameNavigated");
        assert!(v.get("params").is_some());
    }

    // 12. serialize_event without params (method only)
    #[test]
    fn serialize_event_method_only() {
        let ev = CdpEvent {
            method: "Page.domContentEventFired".into(),
            params: None,
        };
        let s = serialize_event(&ev);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["method"], "Page.domContentEventFired");
        assert!(v.get("params").is_none());
    }

    // 13. ok_response has result, no error
    #[test]
    fn ok_response_has_result_no_error() {
        let resp = ok_response(Some(1), json!({"ok": true}));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    // 14. error_response has error, no result
    #[test]
    fn error_response_has_error_no_result() {
        let resp = error_response(Some(1), -32600, "invalid");
        assert!(resp.error.is_some());
        assert!(resp.result.is_none());
    }

    // 15. ok_empty returns empty object result
    #[test]
    fn ok_empty_returns_empty_object() {
        let resp = ok_empty(Some(1));
        assert_eq!(resp.result, Some(json!({})));
        assert!(resp.error.is_none());
    }

    // 16. CdpError debug format
    #[test]
    fn cdp_error_debug_format() {
        let err = CdpError {
            code: -32601,
            message: "Method not found".into(),
        };
        let dbg = format!("{:?}", err);
        assert!(dbg.contains("-32601"));
        assert!(dbg.contains("Method not found"));
    }

    // 17. CdpResponse with large id (i64::MAX)
    #[test]
    fn cdp_response_large_id() {
        let resp = ok_response(Some(i64::MAX), json!({}));
        let s = serialize_response(&resp);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"].as_i64(), Some(i64::MAX));
    }

    // 18. CdpResponse with negative id
    #[test]
    fn cdp_response_negative_id() {
        let resp = ok_response(Some(-100), json!({}));
        let s = serialize_response(&resp);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"].as_i64(), Some(-100));
    }

    // 19. parse_message with unicode in method
    #[test]
    fn parse_unicode_method() {
        let msg = parse_message(r#"{"id":1,"method":"Page.日本語テスト"}"#).unwrap();
        assert_eq!(msg.method, "Page.日本語テスト");
    }

    // 20. parse_message extra fields ignored
    #[test]
    fn parse_extra_fields_ignored() {
        let msg = parse_message(
            r#"{"id":1,"method":"Page.reload","extra":"ignored","another":123}"#,
        )
        .unwrap();
        assert_eq!(msg.id, Some(1));
        assert_eq!(msg.method, "Page.reload");
    }

    // 21. SessionError variants (Closed, Io) debug format
    #[test]
    fn session_error_debug_format() {
        let closed_dbg = format!("{:?}", SessionError::Closed);
        assert!(closed_dbg.contains("Closed"));
        let io_dbg = format!("{:?}", SessionError::Io);
        assert!(io_dbg.contains("Io"));
    }

    // 22. ERR_INVALID_REQUEST / ERR_METHOD_NOT_FOUND constants
    #[test]
    fn error_code_constants() {
        assert_eq!(ERR_INVALID_REQUEST, -32600);
        assert_eq!(ERR_METHOD_NOT_FOUND, -32601);
    }

    // 23. ok_response serialize → contains 'result' key
    #[test]
    fn ok_response_serialize_contains_result() {
        let resp = ok_response(Some(1), json!({"value": 42}));
        let s = serialize_response(&resp);
        assert!(s.contains("\"result\""));
    }

    // 24. error_response serialize → contains 'error' key
    #[test]
    fn error_response_serialize_contains_error() {
        let resp = error_response(Some(1), ERR_INVALID_REQUEST, "bad");
        let s = serialize_response(&resp);
        assert!(s.contains("\"error\""));
    }
}
