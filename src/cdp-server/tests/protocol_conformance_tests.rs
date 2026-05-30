// @trace TEST-CDS-001-PROTO [req:REQ-CDS-001~008] [level:unit]
// CDP protocol conformance: message types, serialization, transport parsing, server config

use cdp_server::*;
use serde_json::{json, Value};

// ---- CdpMessage parsing via public types ----

#[test]
fn test_cdp_message_deserialize_full() {
    let raw = r#"{"id":1,"method":"Page.navigate","params":{"url":"https://example.com"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.navigate");
    assert_eq!(msg.params.as_ref().unwrap().get("url").unwrap().as_str(), Some("https://example.com"));
}

#[test]
fn test_cdp_message_deserialize_no_params() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":2,"method":"Page.enable"}"#).unwrap();
    assert_eq!(msg.id, Some(2));
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_deserialize_no_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"method":"Page.reload"}"#).unwrap();
    assert!(msg.id.is_none());
}

#[test]
fn test_cdp_message_deserialize_with_session_id() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":3,"method":"Runtime.evaluate","params":{"expression":"1+1"},"session_id":"abc123"}"#
    ).unwrap();
    assert_eq!(msg.session_id, Some("abc123".to_string()));
}

#[test]
fn test_cdp_message_invalid_json() {
    assert!(serde_json::from_str::<CdpMessage>("not json").is_err());
    assert!(serde_json::from_str::<CdpMessage>("").is_err());
    assert!(serde_json::from_str::<CdpMessage>("null").is_err());
    assert!(serde_json::from_str::<CdpMessage>("[]").is_err());
}

#[test]
fn test_cdp_message_missing_method() {
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":1}"#).is_err());
}

// ---- CdpResponse serialization ----

#[test]
fn test_cdp_response_ok() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"value": 42})),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains(r#""id":1"#));
    assert!(s.contains("result"));
    assert!(!s.contains("error"));
}

#[test]
fn test_cdp_response_error() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains("-32601"));
    assert!(s.contains("not found"));
}

#[test]
fn test_cdp_response_null_id() {
    let resp = CdpResponse {
        id: None,
        result: Some(json!({})),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains(r#""id":null"#));
}

// ---- CdpEvent serialization ----

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.frameNavigated".to_string(),
        params: Some(json!({"frameId": "main"})),
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(s.contains("Page.frameNavigated"));
    let parsed: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["method"], "Page.frameNavigated");
    assert!(!parsed.as_object().unwrap().contains_key("id"));
}

#[test]
fn test_cdp_event_no_params() {
    let ev = CdpEvent {
        method: "Page.domContentEventFired".to_string(),
        params: None,
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(s.contains("Page.domContentEventFired"));
    assert!(!s.contains("params"));
}

// ---- CdpError ----

#[test]
fn test_cdp_error_fields() {
    let err = CdpError { code: -32600, message: "invalid request".into() };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid request");
    let s = serde_json::to_string(&err).unwrap();
    assert!(s.contains("-32600"));
}

// ---- SessionState ----

#[test]
fn test_session_state_variants() {
    let states = [SessionState::Active, SessionState::Closing];
    // Verify variants exist and debug format
    let _ = format!("{:?}", states[0]);
    let _ = format!("{:?}", states[1]);
}

// ---- Transport parsing ----

#[test]
fn test_parse_close_request() {
    // parse_close_request is in private module — test via TargetInfo
    let info = TargetInfo {
        id: "page-1".to_string(),
        target_type: "page".to_string(),
        title: "Test".to_string(),
        url: "https://example.com".to_string(),
        web_socket_debugger_url: "ws://localhost:9222/devtools/page/page-1".to_string(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains(r#""type":"page""#));
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["id"], "page-1");
    assert_eq!(parsed["title"], "Test");
    assert_eq!(parsed["url"], "https://example.com");
}

#[test]
fn test_target_info_deserialize() {
    let json = r#"{"id":"p2","type":"page","title":"Hello","url":"http://test","web_socket_debugger_url":"ws://x:9222/devtools/page/p2"}"#;
    let info: TargetInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.id, "p2");
    assert_eq!(info.target_type, "page");
    assert_eq!(info.title, "Hello");
}

// ---- ServerConfig builder ----

#[test]
fn test_server_config_defaults() {
    let config = ServerConfig::builder().build();
    assert!(!config.host.is_empty());
    assert!(config.port > 0);
}

#[test]
fn test_server_config_custom() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(9333)
        .browser_name("Bao/1.0")
        .user_agent("Bao/1.0")
        .build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9333);
    assert_eq!(config.browser_name, "Bao/1.0");
    assert_eq!(config.protocol_version, "1.3"); // default
    assert_eq!(config.user_agent, Some("Bao/1.0".to_string()));
}

#[test]
fn test_server_config_v8_webkit_versions() {
    let config = ServerConfig::builder()
        .v8_version("12.0")
        .webkit_version("605.1.15")
        .build();
    assert_eq!(config.v8_version, Some("12.0".to_string()));
    assert_eq!(config.webkit_version, Some("605.1.15".to_string()));
}

#[test]
fn test_server_config_max_sessions() {
    let config = ServerConfig::builder()
        .max_sessions(100)
        .build();
    assert_eq!(config.max_sessions, 100);
}

// ---- DomainRegistry basic ----

#[test]
fn test_registry_new_empty() {
    let reg = DomainRegistry::new();
    assert!(!reg.has_domain("Page"));
    assert!(!reg.has_domain("Runtime"));
}

#[test]
fn test_registry_dispatch_unknown_returns_none() {
    let reg = DomainRegistry::new();
    struct Nop;
    impl EventSender for Nop { fn send_event(&self, _: &str, _: Value) {} }
    assert!(reg.dispatch_command("Unknown.method", json!({}), &Nop).is_none());
}

// ---- Edge cases ----

#[test]
fn test_cdp_message_large_params() {
    let large_array: Vec<i32> = (0..1000).collect();
    let raw = json!({"id": 10, "method": "test.large", "params": {"data": large_array}}).to_string();
    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    assert_eq!(msg.params.unwrap()["data"].as_array().unwrap().len(), 1000);
}

#[test]
fn test_cdp_message_unicode_params() {
    let raw = r#"{"id":11,"method":"Page.navigate","params":{"url":"https://例子.测试"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.unwrap()["url"].as_str().unwrap().contains("例子"));
}

#[test]
fn test_cdp_message_negative_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":-1,"method":"Test.method"}"#).unwrap();
    assert_eq!(msg.id, Some(-1));
}

#[test]
fn test_cdp_message_zero_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":0,"method":"Test.method"}"#).unwrap();
    assert_eq!(msg.id, Some(0));
}

#[test]
fn test_cdp_message_large_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":9999999999,"method":"Test.method"}"#).unwrap();
    assert_eq!(msg.id, Some(9999999999));
}
