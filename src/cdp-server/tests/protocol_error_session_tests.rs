// @trace TEST-CDS-011 [req:REQ-CDS-001,REQ-CDS-002] [level:unit]
// CdpError, SessionError, CdpMessage parse edge cases,
// CdpResponse serialize edge cases, CdpEvent edge cases.

use cdp_server::{CdpError, CdpMessage, CdpResponse, CdpEvent, SessionError};
use serde_json::{json, Value};

// ---- CdpError construction + Serialize ----

#[test]
fn test_cdp_error_new() {
    let err = CdpError { code: -32601, message: "not found".into() };
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "not found");
}

#[test]
fn test_cdp_error_debug() {
    let err = CdpError { code: -32600, message: "invalid".into() };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32600"));
    assert!(debug.contains("invalid"));
}

#[test]
fn test_cdp_error_serialize_roundtrip() {
    let err = CdpError { code: -32700, message: "parse error".into() };
    let json_str = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["code"], -32700);
    assert_eq!(parsed["message"], "parse error");
}

#[test]
fn test_cdp_error_empty_message() {
    let err = CdpError { code: 0, message: String::new() };
    let json_str = serde_json::to_string(&err).unwrap();
    assert!(json_str.contains(r#""message":"""#));
}

#[test]
fn test_cdp_error_unicode_message() {
    let err = CdpError { code: -1, message: "エラー発生".into() };
    let json_str = serde_json::to_string(&err).unwrap();
    assert!(json_str.contains("エラー"));
}

#[test]
fn test_cdp_error_large_code() {
    let err = CdpError { code: i64::MIN, message: "min".into() };
    let json_str = serde_json::to_string(&err).unwrap();
    assert!(json_str.contains(&i64::MIN.to_string()));
}

#[test]
fn test_cdp_error_positive_code() {
    let err = CdpError { code: 999, message: "custom".into() };
    let json_str = serde_json::to_string(&err).unwrap();
    assert!(json_str.contains("999"));
}

#[test]
fn test_cdp_error_clone() {
    let err = CdpError { code: -32601, message: "test".into() };
    let cloned = err.clone();
    assert_eq!(cloned.code, err.code);
    assert_eq!(cloned.message, err.message);
}

// ---- SessionError ----

#[test]
fn test_session_error_closed_debug() {
    let err = SessionError::Closed;
    let debug = format!("{:?}", err);
    assert!(debug.contains("Closed"));
}

#[test]
fn test_session_error_io_debug() {
    let err = SessionError::Io;
    let debug = format!("{:?}", err);
    assert!(debug.contains("Io"));
}

#[test]
fn test_session_error_variants_differ() {
    let d1 = format!("{:?}", SessionError::Closed);
    let d2 = format!("{:?}", SessionError::Io);
    assert_ne!(d1, d2);
}

// ---- CdpMessage parse edge cases (Deserialize) ----

fn parse(raw: &str) -> Option<CdpMessage> {
    serde_json::from_str::<CdpMessage>(raw).ok()
}

#[test]
fn test_parse_message_basic() {
    let msg = parse(r#"{"id":1,"method":"Test.run"}"#).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Test.run");
}

#[test]
fn test_parse_message_with_session() {
    let msg = parse(r#"{"id":1,"method":"Test.run","session_id":"abc"}"#).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("abc"));
}

#[test]
fn test_parse_message_null_method() {
    let msg = parse(r#"{"id":1,"method":null}"#);
    assert!(msg.is_none());
}

#[test]
fn test_parse_message_array_method() {
    let msg = parse(r#"{"id":1,"method":[]}"#);
    assert!(msg.is_none());
}

#[test]
fn test_parse_message_bool_id() {
    let msg = parse(r#"{"id":true,"method":"Test.run"}"#);
    assert!(msg.is_none());
}

#[test]
fn test_parse_message_missing_id_defaults_to_none() {
    let msg = parse(r#"{"method":"Test.run"}"#).unwrap();
    assert!(msg.id.is_none());
}

#[test]
fn test_parse_message_missing_method() {
    assert!(parse(r#"{"id":1}"#).is_none());
}

#[test]
fn test_parse_message_empty_string() {
    assert!(parse("").is_none());
}

#[test]
fn test_parse_message_invalid_json() {
    assert!(parse("{broken}").is_none());
}

#[test]
fn test_parse_message_array_root() {
    assert!(parse("[1,2,3]").is_none());
}

#[test]
fn test_parse_message_null_root() {
    assert!(parse("null").is_none());
}

#[test]
fn test_parse_message_number_root() {
    assert!(parse("42").is_none());
}

#[test]
fn test_parse_message_nested_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"nested":{"deep":[1,{"x":2}]}}}"#;
    let msg = parse(raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["nested"]["deep"][1]["x"], 2);
}

#[test]
fn test_parse_message_large_array_params() {
    let data: Vec<i32> = (0..500).collect();
    let raw = format!(r#"{{"id":1,"method":"Test.run","params":{}}}"#, serde_json::to_string(&data).unwrap());
    let msg = parse(&raw).unwrap();
    assert_eq!(msg.params.unwrap().as_array().unwrap().len(), 500);
}

#[test]
fn test_parse_message_unicode_in_method() {
    let msg = parse(r#"{"id":1,"method":"テスト.実行"}"#).unwrap();
    assert!(msg.method.contains("テスト"));
}

#[test]
fn test_parse_message_extra_fields_ignored() {
    let msg = parse(r#"{"id":1,"method":"Test.run","extra":"ignored"}"#).unwrap();
    assert_eq!(msg.id, Some(1));
}

#[test]
fn test_parse_message_zero_id() {
    let msg = parse(r#"{"id":0,"method":"Test.run"}"#).unwrap();
    assert_eq!(msg.id, Some(0));
}

#[test]
fn test_parse_message_negative_id() {
    let msg = parse(r#"{"id":-42,"method":"Test.run"}"#).unwrap();
    assert_eq!(msg.id, Some(-42));
}

#[test]
fn test_parse_message_max_id() {
    let msg = parse(r#"{"id":9223372036854775807,"method":"Test.run"}"#).unwrap();
    assert_eq!(msg.id, Some(i64::MAX));
}

// ---- CdpResponse serialize edge cases (Serialize only) ----

#[test]
fn test_serialize_response_success() {
    let resp = CdpResponse { id: Some(1), result: Some(json!({"ok": true})), error: None };
    let json_str = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["id"], 1);
    assert_eq!(parsed["result"]["ok"], true);
    assert!(parsed.get("error").is_none());
}

#[test]
fn test_serialize_response_error() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let json_str = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.get("result").is_none());
    assert_eq!(parsed["error"]["code"], -32601);
}

#[test]
fn test_serialize_response_with_null_result() {
    let resp = CdpResponse { id: Some(1), result: Some(Value::Null), error: None };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains(r#""result":null"#));
}

#[test]
fn test_serialize_response_deterministic() {
    let resp = CdpResponse { id: Some(1), result: Some(json!({"a":1})), error: None };
    let j1 = serde_json::to_string(&resp).unwrap();
    let j2 = serde_json::to_string(&resp).unwrap();
    assert_eq!(j1, j2);
}

#[test]
fn test_serialize_response_negative_id() {
    let resp = CdpResponse { id: Some(-100), result: Some(json!({})), error: None };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains("-100"));
}

#[test]
fn test_serialize_response_large_result() {
    let data: Vec<i64> = (0..2000).collect();
    let resp = CdpResponse { id: Some(1), result: Some(json!({"data": data})), error: None };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.len() > 5000);
}

#[test]
fn test_serialize_response_empty_result() {
    let resp = CdpResponse { id: Some(1), result: Some(json!({})), error: None };
    let json_str = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed["result"].as_object().unwrap().is_empty());
}

// ---- CdpEvent edge cases (Serialize + Clone) ----

#[test]
fn test_cdp_event_serialize_with_params() {
    let ev = CdpEvent { method: "Page.load".into(), params: Some(json!({"ts": 1})) };
    let json_str = serde_json::to_string(&ev).unwrap();
    let parsed: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["method"], "Page.load");
    assert_eq!(parsed["params"]["ts"], 1);
}

#[test]
fn test_cdp_event_serialize_without_params() {
    let ev = CdpEvent { method: "DOM.updated".into(), params: None };
    let json_str = serde_json::to_string(&ev).unwrap();
    let parsed: Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.get("params").is_none());
}

#[test]
fn test_cdp_event_empty_method() {
    let ev = CdpEvent { method: String::new(), params: None };
    let json_str = serde_json::to_string(&ev).unwrap();
    assert!(json_str.contains(r#""method":"""#));
}

#[test]
fn test_cdp_event_unicode_method() {
    let ev = CdpEvent { method: "テスト.イベント".into(), params: Some(json!({"key": "値"})) };
    let json_str = serde_json::to_string(&ev).unwrap();
    assert!(json_str.contains("テスト"));
}

#[test]
fn test_cdp_event_large_params() {
    let data: Vec<i64> = (0..3000).collect();
    let ev = CdpEvent { method: "Test.big".into(), params: Some(json!({"data": data})) };
    let json_str = serde_json::to_string(&ev).unwrap();
    assert!(json_str.len() > 10000);
}

#[test]
fn test_cdp_event_clone_independent() {
    let ev = CdpEvent { method: "Test.ev".into(), params: Some(json!({"x": 1})) };
    let mut cloned = ev.clone();
    cloned.method = "Modified".into();
    assert_eq!(ev.method, "Test.ev");
    assert_eq!(cloned.method, "Modified");
}

#[test]
fn test_cdp_event_debug() {
    let ev = CdpEvent { method: "Test.ev".into(), params: None };
    let debug = format!("{:?}", ev);
    assert!(debug.contains("Test.ev"));
}

#[test]
fn test_cdp_event_deterministic() {
    let ev = CdpEvent { method: "Test.ev".into(), params: Some(json!({"x": 1})) };
    let j1 = serde_json::to_string(&ev).unwrap();
    let j2 = serde_json::to_string(&ev).unwrap();
    assert_eq!(j1, j2);
}

// ---- JSON-RPC standard error codes validation ----

#[test]
fn test_jsonrpc_error_codes_standard() {
    // JSON-RPC 2.0 standard error codes
    let standard_codes = [-32700i64, -32600, -32601, -32602, -32603];
    for code in &standard_codes {
        let err = CdpError { code: *code, message: "test".into() };
        let json_str = serde_json::to_string(&err).unwrap();
        assert!(json_str.contains(&code.to_string()));
    }
}

#[test]
fn test_jsonrpc_error_codes_range() {
    // JSON-RPC 2.0: -32768 to -32000 are reserved
    let err = CdpError { code: -32000, message: "server error".into() };
    assert!(err.code >= -32768);
    assert!(err.code <= -32000);
}
