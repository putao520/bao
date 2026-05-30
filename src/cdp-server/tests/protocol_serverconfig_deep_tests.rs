// @trace TEST-CDS-016 [req:REQ-CDS-001] [level:unit]
// @trace TEST-CDS-017 [req:REQ-CDS-005] [level:unit]
// cdp-server protocol helpers, ServerConfig builder, TargetInfo serialization,
// DomainRegistry dispatch edge cases, error code constants, CdpMessage fields.

use cdp_server::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---- Helpers ----

struct NopSender;
impl EventSender for NopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

struct CountingHandler {
    name: &'static str,
    count: Arc<AtomicUsize>,
}

impl DomainHandler for CountingHandler {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, cmd: &str, _params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"cmd": cmd, "domain": self.name}))
    }
    fn on_session_created(&self, _session_id: &str) {}
    fn on_session_destroyed(&self, _session_id: &str) {}
}

// ---- ServerConfig builder edge cases ----

#[test]
fn test_server_config_default_values() {
    let config = ServerConfig::default();
    assert_eq!(config.port, 9222);
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.max_sessions, 100);
    assert_eq!(config.protocol_version, "1.3");
    assert_eq!(config.browser_name, "Bao/0.1.0");
    assert!(config.user_agent.is_none());
    assert!(config.v8_version.is_none());
    assert!(config.webkit_version.is_none());
}

#[test]
fn test_server_config_builder_port_only() {
    let config = ServerConfig::builder().port(8080).build();
    assert_eq!(config.port, 8080);
    assert_eq!(config.host, "127.0.0.1"); // default preserved
}

#[test]
fn test_server_config_builder_host_only() {
    let config = ServerConfig::builder().host("0.0.0.0").build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9222);
}

#[test]
fn test_server_config_builder_max_sessions() {
    let config = ServerConfig::builder().max_sessions(50).build();
    assert_eq!(config.max_sessions, 50);
}

#[test]
fn test_server_config_builder_browser_name() {
    let config = ServerConfig::builder().browser_name("TestBrowser").build();
    assert_eq!(config.browser_name, "TestBrowser");
}

#[test]
fn test_server_config_builder_user_agent() {
    let config = ServerConfig::builder().user_agent("MyAgent/1.0").build();
    assert_eq!(config.user_agent.as_deref(), Some("MyAgent/1.0"));
}

#[test]
fn test_server_config_builder_v8_version() {
    let config = ServerConfig::builder().v8_version("SpiderMonkey").build();
    assert_eq!(config.v8_version.as_deref(), Some("SpiderMonkey"));
}

#[test]
fn test_server_config_builder_webkit_version() {
    let config = ServerConfig::builder().webkit_version("Servo").build();
    assert_eq!(config.webkit_version.as_deref(), Some("Servo"));
}

#[test]
fn test_server_config_builder_full() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(3000)
        .max_sessions(200)
        .browser_name("MyBrowser")
        .user_agent("Agent/2.0")
        .v8_version("V8")
        .webkit_version("WK")
        .http_timeout_seconds(60)
        .build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 3000);
    assert_eq!(config.max_sessions, 200);
    assert_eq!(config.browser_name, "MyBrowser");
    assert_eq!(config.user_agent.as_deref(), Some("Agent/2.0"));
    assert_eq!(config.v8_version.as_deref(), Some("V8"));
    assert_eq!(config.webkit_version.as_deref(), Some("WK"));
    assert_eq!(config.http_timeout_seconds, 60);
}

// ---- TargetInfo serialization ----

#[test]
fn test_target_info_serialization() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("\"id\":\"t-1\""));
    assert!(json.contains("\"type\":\"page\""));
    assert!(json.contains("\"title\":\"Test\""));
    assert!(json.contains("\"url\":\"https://example.com\""));
}

#[test]
fn test_target_info_deserialization() {
    let json = r#"{"id":"t-2","type":"iframe","title":"Sub","url":"about:blank","web_socket_debugger_url":"ws://127.0.0.1:9222/devtools/page/t-2"}"#;
    let info: TargetInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.id, "t-2");
    assert_eq!(info.target_type, "iframe");
    assert_eq!(info.title, "Sub");
}

#[test]
fn test_target_info_roundtrip() {
    let info = TargetInfo {
        id: "abc".into(),
        target_type: "worker".into(),
        title: "SW".into(),
        url: "sw.js".into(),
        web_socket_debugger_url: "ws://localhost:9222/devtools/page/abc".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, info.id);
    assert_eq!(parsed.target_type, info.target_type);
    assert_eq!(parsed.title, info.title);
    assert_eq!(parsed.url, info.url);
}

#[test]
fn test_target_info_empty_fields() {
    let info = TargetInfo {
        id: String::new(),
        target_type: String::new(),
        title: String::new(),
        url: String::new(),
        web_socket_debugger_url: String::new(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert!(parsed.id.is_empty());
}

#[test]
fn test_target_info_clone_independence() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: String::new(),
    };
    let mut cloned = info.clone();
    cloned.id = "t-2".into();
    assert_eq!(info.id, "t-1");
    assert_eq!(cloned.id, "t-2");
}

// ---- CdpMessage edge cases ----

#[test]
fn test_cdp_message_with_large_id() {
    let raw = r#"{"id":9223372036854775807,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(i64::MAX));
}

#[test]
fn test_cdp_message_with_zero_id() {
    let raw = r#"{"id":0,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(0));
}

#[test]
fn test_cdp_message_with_nested_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"a":{"b":{"c":42}}}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.as_ref().unwrap()["a"]["b"]["c"], 42);
}

#[test]
fn test_cdp_message_with_array_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":[1,2,3]}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.as_ref().unwrap().as_array().unwrap().len(), 3);
}

#[test]
fn test_cdp_message_with_empty_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":{}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.as_ref().unwrap().as_object().unwrap().len(), 0);
}

// ---- DomainRegistry dispatch with multiple handlers ----

#[test]
fn test_registry_dispatch_routes_correctly() {
    let reg = DomainRegistry::new();
    let c1 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));

    reg.register(Box::new(CountingHandler { name: "Page", count: c1.clone() })).unwrap();
    reg.register(Box::new(CountingHandler { name: "Runtime", count: c2.clone() })).unwrap();

    let r1 = reg.dispatch_command("Page.navigate", json!({}), &NopSender).unwrap().unwrap();
    assert_eq!(r1["domain"], "Page");
    assert_eq!(r1["cmd"], "Page.navigate");
    assert_eq!(c1.load(Ordering::SeqCst), 1);
    assert_eq!(c2.load(Ordering::SeqCst), 0);

    let r2 = reg.dispatch_command("Runtime.evaluate", json!({}), &NopSender).unwrap().unwrap();
    assert_eq!(r2["domain"], "Runtime");
    assert_eq!(c1.load(Ordering::SeqCst), 1);
    assert_eq!(c2.load(Ordering::SeqCst), 1);
}

#[test]
fn test_registry_dispatch_unknown_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("Unknown.method", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_dispatch_empty_method_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_dispatch_no_dot_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("NoDot", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_has_domain() {
    let reg = DomainRegistry::new();
    assert!(!reg.has_domain("Page"));
    reg.register(Box::new(CountingHandler { name: "Page", count: Arc::new(AtomicUsize::new(0)) })).unwrap();
    assert!(reg.has_domain("Page"));
    assert!(!reg.has_domain("Runtime"));
}

#[test]
fn test_registry_notify_unregistered_domain_noop() {
    let reg = DomainRegistry::new();
    // Should not panic
    reg.notify_session_created("NonExistent", "s-1");
    reg.notify_session_destroyed(&["NonExistent".to_string()], "s-1");
}

// ---- CdpResponse serialization ----

#[test]
fn test_cdp_response_null_id_serializes() {
    let resp = CdpResponse {
        id: None,
        result: Some(json!({"ok": true})),
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["id"].is_null());
    assert_eq!(parsed["result"]["ok"], true);
}

#[test]
fn test_cdp_response_error_serializes() {
    let resp = CdpResponse {
        id: Some(42),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 42);
    assert!(parsed.get("result").is_none());
    assert_eq!(parsed["error"]["code"], -32601);
}

// ---- CdpError construction ----

#[test]
fn test_cdp_error_fields() {
    let err = CdpError { code: -32600, message: "invalid request".into() };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid request");
}

#[test]
fn test_cdp_error_debug() {
    let err = CdpError { code: -32700, message: "parse error".into() };
    assert!(format!("{:?}", err).contains("-32700"));
}

#[test]
fn test_cdp_error_serialize_roundtrip() {
    let err = CdpError { code: -32000, message: "internal".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32000);
    assert_eq!(parsed["message"], "internal");
}

// ---- CdpEvent serialization ----

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345})),
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["method"], "Page.loadEventFired");
    assert_eq!(parsed["params"]["timestamp"], 12345);
}

#[test]
fn test_cdp_event_without_params() {
    let ev = CdpEvent {
        method: "DOM.updated".into(),
        params: None,
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("params").is_none());
}

// ---- SessionError debug ----

#[test]
fn test_session_error_variants() {
    assert!(format!("{:?}", SessionError::Closed).contains("Closed"));
    assert!(format!("{:?}", SessionError::Io).contains("Io"));
}

// ---- SessionState ----

#[test]
fn test_session_state_ordering() {
    assert!((SessionState::Created as u8) < SessionState::Active as u8);
    assert!((SessionState::Active as u8) < SessionState::Closing as u8);
    assert!((SessionState::Closing as u8) < SessionState::Closed as u8);
}

#[test]
fn test_session_state_equality() {
    assert_eq!(SessionState::Created, SessionState::Created);
    assert_ne!(SessionState::Active, SessionState::Closed);
}

#[test]
fn test_session_state_copy() {
    let s1 = SessionState::Active;
    let s2 = s1;
    assert_eq!(s1, s2);
}

#[test]
fn test_session_state_debug_names() {
    assert_eq!(format!("{:?}", SessionState::Created), "Created");
    assert_eq!(format!("{:?}", SessionState::Active), "Active");
    assert_eq!(format!("{:?}", SessionState::Closing), "Closing");
    assert_eq!(format!("{:?}", SessionState::Closed), "Closed");
}

// ---- CdpServer construction ----

#[test]
fn test_cdp_server_default_port() {
    let server = CdpServer::new(ServerConfig::default());
    assert_eq!(server.port(), 9222);
}

#[test]
fn test_cdp_server_custom_port() {
    let server = CdpServer::new(ServerConfig::builder().port(3333).build());
    assert_eq!(server.port(), 3333);
}

#[test]
fn test_cdp_server_ws_url_format() {
    let server = CdpServer::new(ServerConfig::builder().port(9222).build());
    let url = server.ws_url_for_target("abc123");
    assert!(url.starts_with("ws://"));
    assert!(url.contains("9222"));
    assert!(url.contains("abc123"));
}

#[test]
fn test_cdp_server_registry_empty_initially() {
    let server = CdpServer::new(ServerConfig::default());
    assert!(!server.registry().has_domain("Page"));
    assert!(!server.registry().has_domain("Runtime"));
}

#[test]
fn test_cdp_server_broadcaster_exists() {
    let server = CdpServer::new(ServerConfig::default());
    let bc = server.broadcaster();
    assert!(Arc::strong_count(&bc) >= 1);
}

#[test]
fn test_cdp_server_register_and_check() {
    let server = CdpServer::new(ServerConfig::default());
    server.registry().register(Box::new(CountingHandler {
        name: "Page",
        count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();
    assert!(server.registry().has_domain("Page"));
}

// ---- EventBroadcaster with no sessions ----

#[test]
fn test_event_broadcaster_no_sessions_no_panic() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    bc.send_event("Page.loadEventFired", json!({}));
    bc.send_event("Runtime.consoleAPICalled", json!({}));
    bc.send_event("DOM.childNodeInserted", json!({}));
}

#[test]
fn test_event_broadcaster_sender_sends() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    let sender = bc.sender();
    sender.send_event("Test.event", json!({"key": "val"}));
}

#[test]
fn test_event_broadcaster_clone_shares_state() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc1 = EventBroadcaster::new(sessions);
    let bc2 = bc1.clone();
    bc1.send_event("Test.a", json!({}));
    bc2.send_event("Test.b", json!({}));
}
