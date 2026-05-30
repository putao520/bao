// @trace TEST-CDS-009-ROBUST [req:REQ-CDS-001,REQ-CDS-004,REQ-CDS-006] [level:unit]
// Protocol robustness: edge cases for message parsing, dispatch, registry lifecycle

use cdp_server::{CdpMessage, CdpError, CdpResponse, CdpEvent, DomainRegistry, EventSender};
use serde_json::{Value, json};

// ---- NoopEventSender for testing ----

#[derive(Clone)]
struct NoopSender;

impl EventSender for NoopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

// ---- CdpMessage parsing edge cases ----

#[test]
fn test_cdp_message_parse_missing_method() {
    let raw = r#"{"id": 1}"#;
    let result = serde_json::from_str::<CdpMessage>(raw);
    assert!(result.is_err(), "Missing method should fail deserialization");
}

#[test]
fn test_cdp_message_parse_empty_method() {
    let raw = r#"{"id": 1, "method": ""}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.method, "");
}

#[test]
fn test_cdp_message_parse_null_params() {
    let raw = r#"{"id": 1, "method": "Page.navigate", "params": null}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_parse_missing_params() {
    let raw = r#"{"id": 1, "method": "Page.navigate"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_parse_empty_object_params() {
    let raw = r#"{"id": 1, "method": "Page.navigate", "params": {}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params, Some(json!({})));
}

#[test]
fn test_cdp_message_parse_nested_params() {
    let raw = r#"{"id": 1, "method": "DOM.setAttributeValue", "params": {"nodeId": 1, "attributes": {"class": "test", "data-x": "[1,2,3]"}}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["nodeId"], 1);
    assert_eq!(params["attributes"]["class"], "test");
}

#[test]
fn test_cdp_message_parse_large_array_params() {
    let nums: Vec<i64> = (0..1000).collect();
    let raw = json!({"id": 1, "method": "test", "params": {"data": nums}}).to_string();
    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    let params = msg.params.unwrap();
    let arr = params["data"].as_array().unwrap();
    assert_eq!(arr.len(), 1000);
}

#[test]
fn test_cdp_message_parse_string_id() {
    // JSON-RPC spec says id should be string/number, but we parse as i64
    let raw = r#"{"id": "abc", "method": "Page.navigate"}"#;
    let result = serde_json::from_str::<CdpMessage>(raw);
    // String id should fail (CdpMessage.id is Option<i64>)
    assert!(result.is_err());
}

#[test]
fn test_cdp_message_parse_negative_id() {
    let raw = r#"{"id": -999, "method": "Page.navigate"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(-999));
}

#[test]
fn test_cdp_message_parse_zero_id() {
    let raw = r#"{"id": 0, "method": "Page.enable"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(0));
}

#[test]
fn test_cdp_message_parse_large_id() {
    let raw = r#"{"id": 9007199254740991, "method": "Page.enable"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(9007199254740991)); // Number.MAX_SAFE_INTEGER
}

#[test]
fn test_cdp_message_notification_no_id() {
    let raw = r#"{"method": "Page.loadEventFired"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.id.is_none());
    assert_eq!(msg.method, "Page.loadEventFired");
}

#[test]
fn test_cdp_message_with_session_id() {
    let raw = r#"{"id": 1, "method": "Runtime.evaluate", "session_id": "sess-abc123"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.session_id, Some("sess-abc123".into()));
}

// ---- DomainRegistry dispatch edge cases ----

struct EchoHandler {
    name: &'static str,
}

impl cdp_server::DomainHandler for EchoHandler {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, cmd: &str, params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Ok(json!({"command": cmd, "params": params}))
    }
}

#[test]
fn test_dispatch_no_dot_in_method() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    let result = reg.dispatch_command("Page", json!({}), &NoopSender);
    // "Page" has no dot, split('.').next() gives "Page" → handler found
    assert!(result.is_some());
}

#[test]
fn test_dispatch_multiple_dots_in_method() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    let result = reg.dispatch_command("Page.navigate.to.url", json!({}), &NoopSender);
    // split('.').next() gives "Page" → handler found
    assert!(result.is_some());
    let inner = result.unwrap().unwrap();
    assert_eq!(inner["command"], "Page.navigate.to.url");
}

#[test]
fn test_dispatch_empty_method() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    let result = reg.dispatch_command("", json!({}), &NoopSender);
    assert!(result.is_none(), "Empty method should not dispatch");
}

#[test]
fn test_dispatch_unregistered_domain() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    let result = reg.dispatch_command("Network.enable", json!({}), &NoopSender);
    assert!(result.is_none(), "Unregistered domain should return None");
}

#[test]
fn test_dispatch_case_sensitive_domain() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    let result = reg.dispatch_command("page.navigate", json!({}), &NoopSender);
    assert!(result.is_none(), "Domain names are case-sensitive");
}

#[test]
fn test_registry_empty_name_handler() {
    let reg = DomainRegistry::new();
    let result = reg.register(Box::new(EchoHandler { name: "" }));
    // Empty domain name should register (no validation against it)
    assert!(result.is_ok());
}

// ---- CdpResponse serialization ----

#[test]
fn test_cdp_response_success_serialization() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"value": 42})),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains("\"result\""));
    assert!(!s.contains("\"error\""), "error should be skipped when None");
}

#[test]
fn test_cdp_response_error_serialization() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains("\"error\""));
    assert!(!s.contains("\"result\""), "result should be skipped when None");
    assert!(s.contains("-32601"));
    assert!(s.contains("not found"));
}

// ---- CdpEvent serialization ----

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345.0})),
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(s.contains("\"method\""));
    assert!(s.contains("\"params\""));
    assert!(s.contains("Page.loadEventFired"));
}

#[test]
fn test_cdp_event_without_params() {
    let ev = CdpEvent {
        method: "DOM.documentUpdated".into(),
        params: None,
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(!s.contains("\"params\""), "params should be skipped when None");
}

// ---- Multi-handler registry ----

#[test]
fn test_registry_multiple_handlers_independent() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    reg.register(Box::new(EchoHandler { name: "Runtime" })).unwrap();
    reg.register(Box::new(EchoHandler { name: "DOM" })).unwrap();

    let p = reg.dispatch_command("Page.enable", json!({}), &NoopSender);
    assert!(p.is_some());
    let r = reg.dispatch_command("Runtime.evaluate", json!({"expr": "1"}), &NoopSender);
    assert!(r.is_some());
    let d = reg.dispatch_command("DOM.getDocument", json!({}), &NoopSender);
    assert!(d.is_some());

    // Unregistered
    assert!(reg.dispatch_command("Network.enable", json!({}), &NoopSender).is_none());
}

#[test]
fn test_registry_duplicate_rejected() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    let dup = reg.register(Box::new(EchoHandler { name: "Page" }));
    assert!(dup.is_err());
    assert!(dup.unwrap_err().contains("already registered"));
}

// ---- Session lifecycle notifications ----

#[test]
fn test_notify_session_created_unknown_domain() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    // Should not panic for unknown domain
    reg.notify_session_created("UnknownDomain", "sess-1");
}

#[test]
fn test_notify_session_destroyed_empty_list() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    reg.notify_session_destroyed(&[], "sess-1");
}

#[test]
fn test_notify_session_destroyed_multiple_domains() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    reg.register(Box::new(EchoHandler { name: "Runtime" })).unwrap();
    let domains: Vec<String> = vec!["Page".into(), "Runtime".into(), "Unknown".into()];
    reg.notify_session_destroyed(&domains, "sess-1");
}

// ---- Handler returning error ----

struct ErrorAlwaysHandler;

impl cdp_server::DomainHandler for ErrorAlwaysHandler {
    fn domain_name(&self) -> &'static str { "ErrorDomain" }
    fn handle_command(&self, _cmd: &str, _params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Err(CdpError { code: -32000, message: "intentional error".into() })
    }
}

#[test]
fn test_handler_error_propagates() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(ErrorAlwaysHandler)).unwrap();
    let result = reg.dispatch_command("ErrorDomain.doStuff", json!({}), &NoopSender);
    assert!(result.is_some());
    let inner = result.unwrap();
    assert!(inner.is_err());
    let err = inner.unwrap_err();
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("intentional error"));
}

// ---- ServerConfig builder edge cases ----

#[test]
fn test_server_config_builder_minimal() {
    let config = cdp_server::ServerConfig::builder().build();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 9222);
}

#[test]
fn test_server_config_builder_custom_host() {
    let config = cdp_server::ServerConfig::builder()
        .host("0.0.0.0")
        .port(8080)
        .build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 8080);
}

// ---- CdpError traits ----

#[test]
fn test_cdp_error_clone() {
    let e1 = CdpError { code: -32601, message: "test".into() };
    let e2 = e1.clone();
    assert_eq!(e1.code, e2.code);
    assert_eq!(e1.message, e2.message);
}

#[test]
fn test_cdp_error_debug() {
    let e = CdpError { code: -32601, message: "not found".into() };
    let d = format!("{:?}", e);
    assert!(d.contains("-32601"));
    assert!(d.contains("not found"));
}
