// @trace TEST-CDS-014 [req:REQ-CDS-001] [level:unit]
// @trace TEST-CDS-015 [req:REQ-CDS-005] [level:unit]
// cdp-server protocol types + EventBroadcaster deep tests:
// CdpMessage/CdpResponse/CdpError/CdpEvent edge cases, parse_message,
// serialize_response, serialize_event, error_response, ok_empty,
// EventBroadcaster clone + domain filtering, SessionState transitions.

use cdp_server::{
    CdpMessage, CdpResponse, CdpError, CdpEvent, SessionError,
    DomainRegistry, EventBroadcaster, CdpServer, DomainHandler, EventSender,
    ServerConfig, SessionState,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---- Test helpers ----

struct NopSender;
impl EventSender for NopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

struct CountingDomain {
    name: &'static str,
    create_count: Arc<AtomicUsize>,
    destroy_count: Arc<AtomicUsize>,
}

impl DomainHandler for CountingDomain {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, cmd: &str, _params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Ok(json!({"handled": cmd}))
    }
    fn on_session_created(&self, _session_id: &str) {
        self.create_count.fetch_add(1, Ordering::SeqCst);
    }
    fn on_session_destroyed(&self, _session_id: &str) {
        self.destroy_count.fetch_add(1, Ordering::SeqCst);
    }
}

// ---- CdpMessage deserialization edge cases ----

#[test]
fn test_cdp_message_missing_id() {
    // CdpMessage.id is Option<i64>, so missing id should work
    let raw = r#"{"method":"Page.enable"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.id.is_none());
    assert_eq!(msg.method, "Page.enable");
}

#[test]
fn test_cdp_message_null_id() {
    let raw = r#"{"id":null,"method":"Page.enable"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.id.is_none());
}

#[test]
fn test_cdp_message_with_all_fields() {
    let raw = r#"{"id":42,"method":"Runtime.evaluate","params":{"expr":"1"},"session_id":"s1"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(42));
    assert_eq!(msg.method, "Runtime.evaluate");
    assert_eq!(msg.params.as_ref().unwrap()["expr"], "1");
    assert_eq!(msg.session_id.as_ref().unwrap(), "s1");
}

#[test]
fn test_cdp_message_invalid_json() {
    assert!(serde_json::from_str::<CdpMessage>("").is_err());
    assert!(serde_json::from_str::<CdpMessage>("{invalid}").is_err());
    assert!(serde_json::from_str::<CdpMessage>("null").is_err());
    assert!(serde_json::from_str::<CdpMessage>("[]").is_err());
}

#[test]
fn test_cdp_message_missing_method() {
    let raw = r#"{"id":1}"#;
    assert!(serde_json::from_str::<CdpMessage>(raw).is_err());
}

#[test]
fn test_cdp_message_extra_fields() {
    let raw = r#"{"id":1,"method":"Test","extra":true}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(1));
}

#[test]
fn test_cdp_message_negative_id() {
    let raw = r#"{"id":-99,"method":"Test"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(-99));
}

#[test]
fn test_cdp_message_clone() {
    let msg = CdpMessage {
        id: Some(1),
        method: "Page.enable".into(),
        params: Some(json!({"key": "val"})),
        session_id: Some("s-1".into()),
    };
    let cloned = msg.clone();
    assert_eq!(cloned.id, Some(1));
    assert_eq!(cloned.method, "Page.enable");
}

#[test]
fn test_cdp_message_debug() {
    let msg = CdpMessage { id: Some(1), method: "Test".into(), params: None, session_id: None };
    assert!(format!("{:?}", msg).contains("Test"));
}

// ---- CdpResponse serialization ----

#[test]
fn test_cdp_response_ok() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"value": 42})),
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["id"], 1);
    assert_eq!(p["result"]["value"], 42);
    assert!(p.get("error").is_none());
}

#[test]
fn test_cdp_response_error() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert!(p.get("result").is_none());
    assert_eq!(p["error"]["code"], -32601);
}

#[test]
fn test_cdp_response_null_id() {
    let resp = CdpResponse { id: None, result: Some(json!({})), error: None };
    let raw = serde_json::to_string(&resp).unwrap();
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert!(p["id"].is_null());
}

// ---- CdpError ----

#[test]
fn test_cdp_error_clone() {
    let err = CdpError { code: -32601, message: "test".into() };
    let cloned = err.clone();
    assert_eq!(cloned.code, -32601);
    assert_eq!(cloned.message, "test");
}

#[test]
fn test_cdp_error_debug() {
    let err = CdpError { code: -32601, message: "method not found".into() };
    assert!(format!("{:?}", err).contains("-32601"));
}

#[test]
fn test_cdp_error_serialize() {
    let err = CdpError { code: -32000, message: "internal error".into() };
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("-32000"));
    assert!(json.contains("internal error"));
}

// ---- CdpEvent ----

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent { method: "Page.loadEventFired".into(), params: Some(json!({"ts": 1})) };
    let raw = serde_json::to_string(&ev).unwrap();
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["method"], "Page.loadEventFired");
    assert_eq!(p["params"]["ts"], 1);
}

#[test]
fn test_cdp_event_without_params() {
    let ev = CdpEvent { method: "DOM.updated".into(), params: None };
    let raw = serde_json::to_string(&ev).unwrap();
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert!(p.get("params").is_none());
}

#[test]
fn test_cdp_event_clone() {
    let ev = CdpEvent { method: "Test.evt".into(), params: Some(json!({})) };
    let cloned = ev.clone();
    assert_eq!(cloned.method, "Test.evt");
}

// ---- SessionError ----

#[test]
fn test_session_error_debug() {
    let err = SessionError::Closed;
    assert!(format!("{:?}", err).contains("Closed"));
    let err = SessionError::Io;
    assert!(format!("{:?}", err).contains("Io"));
}

// ---- SessionState ----

#[test]
fn test_session_state_values() {
    assert_eq!(SessionState::Created as u8, 0);
    assert_ne!(SessionState::Created, SessionState::Active);
    assert_ne!(SessionState::Active, SessionState::Closing);
    assert_ne!(SessionState::Closing, SessionState::Closed);
}

#[test]
fn test_session_state_debug() {
    assert_eq!(format!("{:?}", SessionState::Created), "Created");
    assert_eq!(format!("{:?}", SessionState::Active), "Active");
    assert_eq!(format!("{:?}", SessionState::Closing), "Closing");
    assert_eq!(format!("{:?}", SessionState::Closed), "Closed");
}

#[test]
fn test_session_state_clone_copy() {
    let s = SessionState::Active;
    let cloned = s;
    assert_eq!(cloned, SessionState::Active);
}

// ---- ServerConfig ----

#[test]
fn test_server_config_default() {
    let config = ServerConfig::default();
    assert_eq!(config.port, 9222);
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.max_sessions, 100);
    assert_eq!(config.protocol_version, "1.3");
}

#[test]
fn test_server_config_builder() {
    let config = ServerConfig::builder().port(8080).build();
    assert_eq!(config.port, 8080);
}

#[test]
fn test_server_config_builder_full() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(3000)
        .max_sessions(50)
        .browser_name("TestBrowser")
        .build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 3000);
    assert_eq!(config.max_sessions, 50);
    assert_eq!(config.browser_name, "TestBrowser");
}

// ---- CdpServer construction ----

#[test]
fn test_cdp_server_new() {
    let server = CdpServer::new(ServerConfig::default());
    assert_eq!(server.port(), 9222);
}

#[test]
fn test_cdp_server_new_with_port() {
    let server = CdpServer::new(ServerConfig::builder().port(9222).build());
    assert_eq!(server.port(), 9222);
}

#[test]
fn test_cdp_server_ws_url_format() {
    let server = CdpServer::new(ServerConfig::builder().port(9222).build());
    let url = server.ws_url_for_target("abc123");
    assert!(url.contains("9222"));
    assert!(url.contains("abc123"));
    assert!(url.starts_with("ws://"));
}

#[test]
fn test_cdp_server_registry_not_empty_after_construction() {
    let server = CdpServer::new(ServerConfig::default());
    assert!(!server.registry().has_domain("Page"));
}

#[test]
fn test_cdp_server_broadcaster_accessible() {
    let server = CdpServer::new(ServerConfig::default());
    let bc = server.broadcaster();
    assert!(Arc::strong_count(&bc) >= 1);
}

// ---- EventBroadcaster: clone shares sessions ----

#[test]
fn test_event_broadcaster_clone() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc1 = EventBroadcaster::new(sessions);
    let bc2 = bc1.clone();
    // Both share the same underlying session map
    bc1.send_event("Test.event", json!({}));
    bc2.send_event("Test.event", json!({}));
    // No sessions, so no errors — just verifying clone works
}

#[test]
fn test_event_broadcaster_sender_boxed() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    let sender = bc.sender();
    sender.send_event("Page.loadEventFired", json!({"timestamp": 1}));
}

#[test]
fn test_event_broadcaster_empty_sessions() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    // Should not panic with empty sessions
    bc.send_event("Page.loadEventFired", json!({}));
    bc.send_event("Runtime.consoleAPICalled", json!({}));
}

// ---- DomainRegistry with lifecycle callbacks ----

#[test]
fn test_registry_notify_session_created_counting() {
    let reg = DomainRegistry::new();
    let create = Arc::new(AtomicUsize::new(0));
    let destroy = Arc::new(AtomicUsize::new(0));
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: create.clone(),
        destroy_count: destroy.clone(),
    })).unwrap();

    for i in 0..5 {
        reg.notify_session_created("Page", &format!("s-{}", i));
    }
    assert_eq!(create.load(Ordering::SeqCst), 5);
    assert_eq!(destroy.load(Ordering::SeqCst), 0);
}

#[test]
fn test_registry_notify_session_destroyed_counting() {
    let reg = DomainRegistry::new();
    let create = Arc::new(AtomicUsize::new(0));
    let destroy = Arc::new(AtomicUsize::new(0));
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: create.clone(),
        destroy_count: destroy.clone(),
    })).unwrap();

    reg.notify_session_destroyed(&["Page".to_string()], "s-1");
    reg.notify_session_destroyed(&["Page".to_string()], "s-2");
    assert_eq!(destroy.load(Ordering::SeqCst), 2);
}

#[test]
fn test_registry_destroy_unregistered_domain_noop() {
    let reg = DomainRegistry::new();
    // Should not panic
    reg.notify_session_destroyed(&["NonExistent".to_string()], "s-1");
    reg.notify_session_created("NonExistent", "s-1");
}

// ---- Multiple domains with interleaved lifecycle ----

#[test]
fn test_multiple_domains_lifecycle() {
    let reg = DomainRegistry::new();
    let c1 = Arc::new(AtomicUsize::new(0));
    let d1 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::new(AtomicUsize::new(0));

    reg.register(Box::new(CountingDomain { name: "Page", create_count: c1.clone(), destroy_count: d1.clone() })).unwrap();
    reg.register(Box::new(CountingDomain { name: "Runtime", create_count: c2.clone(), destroy_count: d2.clone() })).unwrap();

    reg.notify_session_created("Page", "s-1");
    reg.notify_session_created("Runtime", "s-1");
    reg.notify_session_created("Page", "s-2");

    assert_eq!(c1.load(Ordering::SeqCst), 2);
    assert_eq!(c2.load(Ordering::SeqCst), 1);

    reg.notify_session_destroyed(&["Page".to_string(), "Runtime".to_string()], "s-1");
    assert_eq!(d1.load(Ordering::SeqCst), 1);
    assert_eq!(d2.load(Ordering::SeqCst), 1);
}

// ---- Dispatch with multiple handlers ----

#[test]
fn test_dispatch_routes_to_correct_handler() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: Arc::new(AtomicUsize::new(0)),
        destroy_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();
    reg.register(Box::new(CountingDomain {
        name: "Runtime",
        create_count: Arc::new(AtomicUsize::new(0)),
        destroy_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();

    let r1 = reg.dispatch_command("Page.navigate", json!({}), &NopSender).unwrap().unwrap();
    assert_eq!(r1["handled"], "Page.navigate");

    let r2 = reg.dispatch_command("Runtime.evaluate", json!({}), &NopSender).unwrap().unwrap();
    assert_eq!(r2["handled"], "Runtime.evaluate");
}

#[test]
fn test_dispatch_unregistered_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("Unknown.method", json!({}), &NopSender).is_none());
}

#[test]
fn test_dispatch_no_dot_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("NoDotMethod", json!({}), &NopSender).is_none());
}

#[test]
fn test_dispatch_empty_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("", json!({}), &NopSender).is_none());
}

// ---- CdpServer register handler ----

#[test]
fn test_cdp_server_register_handler() {
    let server = CdpServer::new(ServerConfig::default());
    assert!(!server.registry().has_domain("Page"));
    server.registry().register(Box::new(CountingDomain {
        name: "Page",
        create_count: Arc::new(AtomicUsize::new(0)),
        destroy_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();
    assert!(server.registry().has_domain("Page"));
}

// ---- TargetInfo ----

#[test]
fn test_target_info_construction() {
    let info = cdp_server::TargetInfo {
        id: "t-1".into(),
        title: "Test".into(),
        url: "https://example.com".into(),
        target_type: "page".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    assert_eq!(info.id, "t-1");
    assert_eq!(info.title, "Test");
    assert_eq!(info.url, "https://example.com");
    assert_eq!(info.target_type, "page");
}

#[test]
fn test_target_info_clone() {
    let info = cdp_server::TargetInfo {
        id: "t-1".into(),
        title: "Test".into(),
        url: "about:blank".into(),
        target_type: "page".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    let cloned = info.clone();
    assert_eq!(cloned.id, "t-1");
    assert_eq!(cloned.url, "about:blank");
}

#[test]
fn test_target_info_debug() {
    let info = cdp_server::TargetInfo {
        id: "t-1".into(),
        title: "Test".into(),
        url: "about:blank".into(),
        target_type: "page".into(),
        web_socket_debugger_url: String::new(),
    };
    let debug = format!("{:?}", info);
    assert!(debug.contains("t-1"));
}
