// @trace TEST-CDS-001 [req:REQ-CDS-001] [level:unit]
// @trace TEST-CDS-002 [req:REQ-CDS-004] [level:unit]
// @trace TEST-CDS-003 [req:REQ-CDS-001] [level:unit]
// @trace TEST-CDS-004 [req:REQ-CDS-002] [level:unit]
// @trace TEST-CDS-005 [req:REQ-CDS-005] [level:unit]

use cdp_server::{CdpError, DomainHandler, EventSender, DomainRegistry, ServerConfig, TargetInfo};
use cdp_server::{CdpMessage, CdpResponse, CdpEvent};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Stub EventSender for tests
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct NoopEventSender;

impl EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

// ---------------------------------------------------------------------------
// Stub DomainHandler for tests
// ---------------------------------------------------------------------------

struct EchoHandler;

impl DomainHandler for EchoHandler {
    fn domain_name(&self) -> &'static str { "Echo" }
    fn handle_command(&self, command: &str, params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Echo.ping" => Ok(json!({ "pong": true })),
            "Echo.reflect" => Ok(json!({ "echo": params })),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

struct StatefulHandler {
    name: &'static str,
}

impl DomainHandler for StatefulHandler {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, command: &str, _params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        Ok(json!({ "domain": self.name, "command": command }))
    }
    fn on_session_created(&self, session_id: &str) {
        let _ = session_id;
    }
    fn on_session_destroyed(&self, session_id: &str) {
        let _ = session_id;
    }
}

// ===========================================================================
// §1 Protocol parsing tests (TEST-CDS-001)
// ===========================================================================

fn parse_msg(raw: &str) -> Option<CdpMessage> {
    serde_json::from_str(raw).ok()
}

#[test]
fn test_parse_valid_message() {
    let msg = parse_msg(r#"{"id":1,"method":"Page.navigate","params":{"url":"https://example.com"}}"#).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.navigate");
    assert_eq!(msg.params.as_ref().unwrap().get("url").unwrap().as_str(), Some("https://example.com"));
}

#[test]
fn test_parse_message_without_params() {
    let msg = parse_msg(r#"{"id":42,"method":"Page.enable"}"#).unwrap();
    assert_eq!(msg.id, Some(42));
    assert_eq!(msg.method, "Page.enable");
    assert!(msg.params.is_none());
}

#[test]
fn test_parse_message_without_id() {
    let msg = parse_msg(r#"{"method":"Runtime.consoleAPICalled","params":{}}"#).unwrap();
    assert!(msg.id.is_none());
    assert_eq!(msg.method, "Runtime.consoleAPICalled");
}

#[test]
fn test_parse_invalid_json_returns_none() {
    assert!(parse_msg("not json at all").is_none());
    assert!(parse_msg("").is_none());
    assert!(parse_msg("{{{invalid").is_none());
}

#[test]
fn test_parse_with_session_id() {
    let msg = parse_msg(r#"{"id":1,"method":"Runtime.evaluate","session_id":"abc123"}"#).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("abc123"));
}

// ===========================================================================
// §2 DomainRegistry tests (TEST-CDS-002 / REQ-CDS-004)
// ===========================================================================

#[test]
fn test_registry_register_and_dispatch() {
    let registry = DomainRegistry::new();
    registry.register(Box::new(EchoHandler)).unwrap();

    let es = NoopEventSender;
    let result = registry.dispatch_command("Echo.ping", json!({}), &es);
    assert!(result.is_some());
    let value = result.unwrap().unwrap();
    assert_eq!(value["pong"], true);
}

#[test]
fn test_registry_duplicate_registration_fails() {
    let registry = DomainRegistry::new();
    registry.register(Box::new(EchoHandler)).unwrap();
    let err = registry.register(Box::new(EchoHandler));
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("already registered"));
}

#[test]
fn test_registry_unknown_domain_returns_none() {
    let registry = DomainRegistry::new();
    let es = NoopEventSender;
    assert!(registry.dispatch_command("Unknown.method", json!({}), &es).is_none());
}

#[test]
fn test_registry_has_domain() {
    let registry = DomainRegistry::new();
    assert!(!registry.has_domain("Echo"));
    registry.register(Box::new(EchoHandler)).unwrap();
    assert!(registry.has_domain("Echo"));
}

#[test]
fn test_registry_multiple_domains() {
    let registry = DomainRegistry::new();
    registry.register(Box::new(StatefulHandler { name: "Page" })).unwrap();
    registry.register(Box::new(StatefulHandler { name: "Runtime" })).unwrap();
    registry.register(Box::new(StatefulHandler { name: "DOM" })).unwrap();

    let es = NoopEventSender;

    let result = registry.dispatch_command("Page.navigate", json!({}), &es).unwrap().unwrap();
    assert_eq!(result["domain"], "Page");

    let result = registry.dispatch_command("Runtime.evaluate", json!({}), &es).unwrap().unwrap();
    assert_eq!(result["domain"], "Runtime");

    let result = registry.dispatch_command("DOM.getDocument", json!({}), &es).unwrap().unwrap();
    assert_eq!(result["domain"], "DOM");
}

#[test]
fn test_registry_command_not_found() {
    let registry = DomainRegistry::new();
    registry.register(Box::new(EchoHandler)).unwrap();

    let es = NoopEventSender;
    let result = registry.dispatch_command("Echo.nonexistent", json!({}), &es);
    assert!(result.is_some());
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_registry_dispatch_extracts_domain() {
    let registry = DomainRegistry::new();
    registry.register(Box::new(EchoHandler)).unwrap();

    let es = NoopEventSender;
    let result = registry.dispatch_command("Echo.reflect", json!({"key": "value"}), &es);
    let value = result.unwrap().unwrap();
    assert_eq!(value["echo"]["key"], "value");
}

// ===========================================================================
// §3 Transport parsing tests (TEST-CDS-003 / REQ-CDS-001)
// ===========================================================================

mod transport_tests {
    // Transport functions are private, so we test through exported types.

    #[test]
    fn target_info_serialization() {
        let info = cdp_server::TargetInfo {
            id: "abc123".into(),
            target_type: "page".into(),
            title: "Test Page".into(),
            url: "https://example.com".into(),
            web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/abc123".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(r#""id":"abc123""#));
        assert!(json.contains(r#""type":"page""#));
        assert!(json.contains(r#""title":"Test Page""#));
        assert!(json.contains(r#""url":"https://example.com""#));
    }

    #[test]
    fn target_info_deserialization() {
        let json = r#"{"id":"xyz","type":"page","title":"T","url":"U","web_socket_debugger_url":"W"}"#;
        let info: cdp_server::TargetInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.id, "xyz");
        assert_eq!(info.target_type, "page");
    }
}

// ===========================================================================
// §4 ServerConfig builder tests (TEST-CDS-004 / REQ-CDS-008)
// ===========================================================================

#[test]
fn test_server_config_default() {
    let config = ServerConfig::default();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 9222);
    assert_eq!(config.max_sessions, 100);
    assert_eq!(config.protocol_version, "1.3");
}

#[test]
fn test_server_config_builder() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(9333)
        .max_sessions(50)
        .browser_name("TestBrowser/1.0")
        .user_agent("TestAgent")
        .v8_version("SM")
        .webkit_version("Servo")
        .build();

    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9333);
    assert_eq!(config.max_sessions, 50);
    assert_eq!(config.browser_name, "TestBrowser/1.0");
    assert_eq!(config.user_agent.as_deref(), Some("TestAgent"));
    assert_eq!(config.v8_version.as_deref(), Some("SM"));
    assert_eq!(config.webkit_version.as_deref(), Some("Servo"));
}

#[test]
fn test_server_config_builder_partial() {
    let config = ServerConfig::builder()
        .port(8080)
        .build();
    assert_eq!(config.port, 8080);
    assert_eq!(config.host, "127.0.0.1"); // default preserved
}

// ===========================================================================
// §5 EventSender trait contract tests (TEST-CDS-005 / REQ-CDS-005)
// ===========================================================================

#[test]
fn test_noop_event_sender_satisfies_trait() {
    let sender = NoopEventSender;
    sender.send_event("Page.loadEventFired", json!({}));
    // No panic = pass
}

#[test]
fn test_event_serialization() {
    let event = CdpEvent {
        method: "Page.loadEventFired".to_string(),
        params: Some(json!({ "timestamp": 12345 })),
    };
    let json_str = serde_json::to_string(&event).unwrap();
    assert!(json_str.contains("Page.loadEventFired"));
    assert!(json_str.contains("12345"));
}

#[test]
fn test_response_serialization_success() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({ "value": 42 })),
        error: None,
    };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains(r#""result""#));
    assert!(!json_str.contains(r#""error""#));
}

#[test]
fn test_response_serialization_error() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains(r#""error""#));
    assert!(json_str.contains("-32601"));
    assert!(!json_str.contains(r#""result""#));
}

// ===========================================================================
// §6 Error code constants tests
// ===========================================================================

#[test]
fn test_cdp_error_codes() {
    let err = CdpError { code: -32601, message: "test".into() };
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "test");

    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("-32601"));
}

// ===========================================================================
// §7 DomainHandler lifecycle tests
// ===========================================================================

#[test]
fn test_handler_on_session_created_noop() {
    let handler = EchoHandler;
    handler.on_session_created("session-1");
    // No panic = pass (default impl is noop)
}

#[test]
fn test_handler_on_session_destroyed_noop() {
    let handler = EchoHandler;
    handler.on_session_destroyed("session-1");
    // No panic = pass
}
