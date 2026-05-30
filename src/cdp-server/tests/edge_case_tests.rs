// @trace TEST-CDS-013 [req:REQ-CDS-001] [level:unit]
// @trace TEST-CDS-014 [req:REQ-CDS-003] [level:unit]
// @trace TEST-CDS-015 [req:REQ-CDS-004] [level:unit]
// @trace TEST-CDS-016 [req:REQ-CDS-008] [level:unit]
// Edge case tests: dispatch, config validation, handler registration, empty/null params.

use cdp_server::{CdpError, DomainHandler, EventSender, DomainRegistry, ServerConfig, CdpMessage};
use serde_json::{json, Value};

#[derive(Clone)]
struct CaptureEventSender;

impl EventSender for CaptureEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

struct EchoHandler {
    domain: &'static str,
}

impl DomainHandler for EchoHandler {
    fn domain_name(&self) -> &'static str { self.domain }

    fn handle_command(&self, command: &str, params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Ok(json!({ "command": command, "params": params }))
    }
}

struct ErrorHandler;

impl DomainHandler for ErrorHandler {
    fn domain_name(&self) -> &'static str { "Error" }

    fn handle_command(&self, command: &str, _: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Err(CdpError { code: -32000, message: format!("command '{}' failed", command) })
    }
}

struct MultiCmdHandler {
    domain: &'static str,
}

impl DomainHandler for MultiCmdHandler {
    fn domain_name(&self) -> &'static str { self.domain }

    fn handle_command(&self, command: &str, params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            d if command.ends_with("enable") => Ok(json!({"state": "enabled"})),
            d if command.ends_with("disable") => Ok(json!({"state": "disabled"})),
            _ => Ok(json!({"echo": command, "params": params})),
        }
    }
}

// --- CdpMessage construction ---

#[test]
fn test_cdp_message_fields() {
    let msg = CdpMessage {
        id: Some(1),
        method: "Page.navigate".into(),
        params: Some(json!({"url": "https://example.com"})),
        session_id: None,
    };
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.navigate");
    assert_eq!(msg.params.as_ref().unwrap()["url"], "https://example.com");
}

#[test]
fn test_cdp_message_notification() {
    let msg = CdpMessage {
        id: None,
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 1234.5})),
        session_id: None,
    };
    assert!(msg.id.is_none());
    assert_eq!(msg.method, "Page.loadEventFired");
}

#[test]
fn test_cdp_message_with_session_id() {
    let msg = CdpMessage {
        id: Some(10),
        method: "Runtime.evaluate".into(),
        params: Some(json!({})),
        session_id: Some("sess-abc".into()),
    };
    assert_eq!(msg.session_id.as_deref(), Some("sess-abc"));
}

#[test]
fn test_cdp_message_null_params() {
    let msg = CdpMessage {
        id: Some(5),
        method: "Test.cmd".into(),
        params: None,
        session_id: None,
    };
    assert!(msg.params.is_none());
}

// --- DomainRegistry dispatch ---

#[test]
fn test_registry_dispatch_echo() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { domain: "Test" })).unwrap();

    let sender = CaptureEventSender;
    let result = reg.dispatch_command("Test.echo", json!({"key": "val"}), &sender);
    assert!(result.is_some());
}

#[test]
fn test_registry_dispatch_unknown_domain() {
    let reg = DomainRegistry::new();
    let sender = CaptureEventSender;
    let result = reg.dispatch_command("Unknown.cmd", json!({}), &sender);
    assert!(result.is_none());
}

#[test]
fn test_registry_dispatch_handler_error() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(ErrorHandler)).unwrap();

    let sender = CaptureEventSender;
    let result = reg.dispatch_command("Error.fail", json!({}), &sender);
    match result {
        Some(Err(e)) => assert_eq!(e.code, -32000),
        _ => panic!("expected error response from ErrorHandler"),
    }
}

#[test]
fn test_registry_multiple_domains() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { domain: "Page" })).unwrap();
    reg.register(Box::new(EchoHandler { domain: "Runtime" })).unwrap();
    reg.register(Box::new(EchoHandler { domain: "DOM" })).unwrap();

    assert!(reg.has_domain("Page"));
    assert!(reg.has_domain("Runtime"));
    assert!(reg.has_domain("DOM"));
    assert!(!reg.has_domain("Network"));

    let sender = CaptureEventSender;
    assert!(reg.dispatch_command("Page.reload", json!({}), &sender).is_some());
    assert!(reg.dispatch_command("Runtime.evaluate", json!({}), &sender).is_some());
    assert!(reg.dispatch_command("DOM.getDocument", json!({}), &sender).is_some());
    assert!(reg.dispatch_command("Network.enable", json!({}), &sender).is_none());
}

#[test]
fn test_registry_duplicate_registration() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { domain: "Test" })).unwrap();
    let result = reg.register(Box::new(EchoHandler { domain: "Test" }));
    assert!(result.is_err());
}

#[test]
fn test_registry_multi_cmd_handler() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(MultiCmdHandler { domain: "Page" })).unwrap();

    let sender = CaptureEventSender;
    let enable = reg.dispatch_command("Page.enable", json!({}), &sender);
    assert!(enable.is_some());
    match enable {
        Some(Ok(v)) => assert_eq!(v["state"], "enabled"),
        _ => panic!("expected ok from enable"),
    }

    let navigate = reg.dispatch_command("Page.navigate", json!({"url": "http://test.com"}), &sender);
    assert!(navigate.is_some());
}

#[test]
fn test_registry_empty_domain_name() {
    // Ensure has_domain works for empty string
    let reg = DomainRegistry::new();
    assert!(!reg.has_domain(""));
}

// --- ServerConfig builder ---

#[test]
fn test_config_builder_all_fields() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(9333)
        .http_timeout_seconds(60)
        .max_sessions(200)
        .browser_name("Bao/1.0")
        .user_agent("Bao/1.0")
        .v8_version("11.0")
        .webkit_version("605.1")
        .build();

    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9333);
    assert_eq!(config.http_timeout_seconds, 60);
    assert_eq!(config.max_sessions, 200);
    assert_eq!(config.browser_name, "Bao/1.0");
    assert_eq!(config.user_agent.as_deref(), Some("Bao/1.0"));
    assert_eq!(config.v8_version.as_deref(), Some("11.0"));
    assert_eq!(config.webkit_version.as_deref(), Some("605.1"));
}

#[test]
fn test_config_default_values() {
    let config = ServerConfig::default();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 9222);
    assert_eq!(config.http_timeout_seconds, 30);
    assert_eq!(config.max_sessions, 100);
    assert_eq!(config.browser_name, "Bao/0.1.0");
    assert_eq!(config.protocol_version, "1.3");
    assert!(config.user_agent.is_none());
    assert!(config.v8_version.is_none());
    assert!(config.webkit_version.is_none());
}

#[test]
fn test_config_builder_zero_port() {
    let config = ServerConfig::builder().port(0).build();
    assert_eq!(config.port, 0);
}

#[test]
fn test_config_builder_high_port() {
    let config = ServerConfig::builder().port(65535).build();
    assert_eq!(config.port, 65535);
}

#[test]
fn test_config_builder_partial() {
    let config = ServerConfig::builder()
        .port(8080)
        .max_sessions(50)
        .build();
    assert_eq!(config.port, 8080);
    assert_eq!(config.max_sessions, 50);
    assert_eq!(config.host, "127.0.0.1"); // default
}

// --- CdpError ---

#[test]
fn test_cdp_error_fields() {
    let err = CdpError { code: -32601, message: "method not found".into() };
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "method not found");
}

#[test]
fn test_cdp_error_clone() {
    let err = CdpError { code: -32000, message: "test".into() };
    let cloned = err.clone();
    assert_eq!(cloned.code, err.code);
    assert_eq!(cloned.message, err.message);
}
