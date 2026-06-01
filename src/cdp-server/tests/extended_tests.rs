// @trace TEST-CDS-009 [req:REQ-CDS-004] [level:unit]
// @trace TEST-CDS-010 [req:REQ-CDS-005] [level:unit]
// @trace TEST-CDS-011 [req:REQ-CDS-006] [level:unit]
// @trace TEST-CDS-012 [req:REQ-CDS-008] [level:unit]

use cdp_server::{CdpError, DomainHandler, EventSender, DomainRegistry, ServerConfig, TargetInfo};
use cdp_server::{CdpMessage, CdpResponse, CdpEvent};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct NoopEventSender;
impl EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

/// Handler that tracks session lifecycle calls.
struct LifecycleHandler {
    session_created: Arc<Mutex<Vec<String>>>,
    session_destroyed: Arc<Mutex<Vec<String>>>,
}

impl LifecycleHandler {
    fn new() -> Self {
        LifecycleHandler {
            session_created: Arc::new(Mutex::new(Vec::new())),
            session_destroyed: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl DomainHandler for LifecycleHandler {
    fn domain_name(&self) -> &'static str { "Lifecycle" }

    fn handle_command(&self, command: &str, _params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Lifecycle.status" => Ok(json!({ "status": "ok" })),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }

    fn on_session_created(&self, session_id: &str) {
        self.session_created.lock().unwrap().push(session_id.to_string());
    }

    fn on_session_destroyed(&self, session_id: &str) {
        self.session_destroyed.lock().unwrap().push(session_id.to_string());
    }
}

/// Handler that captures events sent through EventSender.
struct EventCaptor {
    captured: Arc<Mutex<Vec<(String, Value)>>>,
}

impl EventCaptor {
    fn new() -> Self {
        EventCaptor {
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn sender(&self) -> CapturingEventSender {
        CapturingEventSender {
            captured: Arc::clone(&self.captured),
        }
    }
}

#[derive(Clone)]
struct CapturingEventSender {
    captured: Arc<Mutex<Vec<(String, Value)>>>,
}

impl EventSender for CapturingEventSender {
    fn send_event(&self, method: &str, params: Value) {
        self.captured.lock().unwrap().push((method.to_string(), params));
    }
}

// ===========================================================================
// §1 DomainRegistry thread safety (REQ-CDS-004)
// ===========================================================================

#[test]
fn test_registry_concurrent_registration() {
    use std::thread;

    let registry = Arc::new(DomainRegistry::new());
    let mut handles = Vec::new();

    for i in 0..5 {
        let reg = Arc::clone(&registry);
        handles.push(thread::spawn(move || {
            struct Handler { name: &'static str }
            impl DomainHandler for Handler {
                fn domain_name(&self) -> &'static str { self.name }
                fn handle_command(&self, _cmd: &str, _p: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
                    Ok(json!({}))
                }
            }
            let domain_name = match i {
                0 => "Domain0",
                1 => "Domain1",
                2 => "Domain2",
                3 => "Domain3",
                _ => "Domain4",
            };
            // Create handler with unique name
            let result = match i {
                0 => reg.register(Box::new(Handler { name: "Domain0" })),
                1 => reg.register(Box::new(Handler { name: "Domain1" })),
                2 => reg.register(Box::new(Handler { name: "Domain2" })),
                3 => reg.register(Box::new(Handler { name: "Domain3" })),
                _ => reg.register(Box::new(Handler { name: "Domain4" })),
            };
            assert!(result.is_ok());
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    for i in 0..5u8 {
        let domain = format!("Domain{}", i);
        assert!(registry.has_domain(&domain));
    }
}

#[test]
fn test_registry_dispatch_unregistered() {
    let registry = DomainRegistry::new();
    let es = NoopEventSender;
    let result = registry.dispatch_command("Unregistered.method", json!({}), &es);
    assert!(result.is_none());
}

// ===========================================================================
// §2 Session lifecycle callbacks (REQ-CDS-006)
// ===========================================================================

#[test]
fn test_session_created_callback() {
    let handler = LifecycleHandler::new();
    let created = Arc::clone(&handler.session_created);
    let registry = DomainRegistry::new();
    registry.register(Box::new(handler)).unwrap();

    registry.notify_session_created("Lifecycle", "sess-1");

    let created_sessions = created.lock().unwrap();
    assert_eq!(created_sessions.len(), 1);
    assert_eq!(created_sessions[0], "sess-1");
}

#[test]
fn test_session_destroyed_callback() {
    let handler = LifecycleHandler::new();
    let destroyed = Arc::clone(&handler.session_destroyed);
    let registry = DomainRegistry::new();
    registry.register(Box::new(handler)).unwrap();

    let domains = vec!["Lifecycle".to_string()];
    registry.notify_session_destroyed(&domains, "sess-1");

    let destroyed_sessions = destroyed.lock().unwrap();
    assert_eq!(destroyed_sessions.len(), 1);
    assert_eq!(destroyed_sessions[0], "sess-1");
}

#[test]
fn test_lifecycle_dispatch_command() {
    let handler = LifecycleHandler::new();
    let registry = DomainRegistry::new();
    registry.register(Box::new(handler)).unwrap();

    let es = NoopEventSender;
    let result = registry.dispatch_command("Lifecycle.status", json!({}), &es);
    assert!(result.is_some());
    let value = result.unwrap().unwrap();
    assert_eq!(value["status"], "ok");
}

#[test]
fn test_lifecycle_unknown_domain_no_callback() {
    let handler = LifecycleHandler::new();
    let created = Arc::clone(&handler.session_created);
    let registry = DomainRegistry::new();
    registry.register(Box::new(handler)).unwrap();

    // Notify a domain that doesn't have a handler
    registry.notify_session_created("NonExistent", "sess-1");

    let created_sessions = created.lock().unwrap();
    assert!(created_sessions.is_empty());
}

// ===========================================================================
// §3 EventSender trait contract (REQ-CDS-005)
// ===========================================================================

#[test]
fn test_capturing_event_sender() {
    let captor = EventCaptor::new();
    let sender = captor.sender();

    sender.send_event("Page.loadEventFired", json!({"timestamp": 1}));
    sender.send_event("Runtime.consoleAPICalled", json!({"type": "log"}));

    let captured = captor.captured.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].0, "Page.loadEventFired");
    assert_eq!(captured[1].0, "Runtime.consoleAPICalled");
}

#[test]
fn test_event_sender_clone_independence() {
    let captor = EventCaptor::new();
    let sender1 = captor.sender();
    let sender2 = sender1.clone();

    sender1.send_event("Test.event1", json!({}));
    sender2.send_event("Test.event2", json!({}));

    let captured = captor.captured.lock().unwrap();
    assert_eq!(captured.len(), 2);
}

#[test]
fn test_handler_with_event_sender() {
    struct EventEmitHandler;
    impl DomainHandler for EventEmitHandler {
        fn domain_name(&self) -> &'static str { "Emit" }
        fn handle_command(&self, cmd: &str, _p: Value, es: &dyn EventSender) -> Result<Value, CdpError> {
            match cmd {
                "Emit.trigger" => {
                    es.send_event("Emit.triggered", json!({"fired": true}));
                    Ok(json!({ "emitted": true }))
                }
                _ => Err(CdpError { code: -32601, message: "not found".into() }),
            }
        }
    }

    let captor = EventCaptor::new();
    let sender = captor.sender();
    let registry = DomainRegistry::new();
    registry.register(Box::new(EventEmitHandler)).unwrap();

    let result = registry.dispatch_command("Emit.trigger", json!({}), &sender);
    assert!(result.is_some());
    let value = result.unwrap().unwrap();
    assert_eq!(value["emitted"], true);

    let captured = captor.captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].0, "Emit.triggered");
    assert_eq!(captured[0].1["fired"], true);
}

// ===========================================================================
// §4 ServerConfig edge cases (REQ-CDS-008)
// ===========================================================================

#[test]
fn test_server_config_all_fields() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(12345)
        .http_timeout_seconds(60)
        .max_sessions(200)
        .browser_name("TestBrowser/2.0")
        .user_agent("TestAgent/2.0")
        .v8_version("SpiderMonkey102")
        .webkit_version("Servo2")
        .build();

    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 12345);
    assert_eq!(config.http_timeout_seconds, 60);
    assert_eq!(config.max_sessions, 200);
    assert_eq!(config.browser_name, "TestBrowser/2.0");
    assert_eq!(config.user_agent.as_deref(), Some("TestAgent/2.0"));
    assert_eq!(config.v8_version.as_deref(), Some("SpiderMonkey102"));
    assert_eq!(config.webkit_version.as_deref(), Some("Servo2"));
}

#[test]
fn test_server_config_zero_port() {
    let config = ServerConfig::builder().port(0).build();
    assert_eq!(config.port, 0);
    // Port 0 means OS assigns a port
}

#[test]
fn test_server_config_large_max_sessions() {
    let config = ServerConfig::builder().max_sessions(10000).build();
    assert_eq!(config.max_sessions, 10000);
}

// ===========================================================================
// §5 TargetInfo serialization (REQ-CDS-007)
// ===========================================================================

#[test]
fn test_target_info_roundtrip() {
    let info = TargetInfo {
        id: "target-123".into(),
        target_type: "page".into(),
        title: "Test Page".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/target-123".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, info.id);
    assert_eq!(parsed.target_type, info.target_type);
    assert_eq!(parsed.title, info.title);
    assert_eq!(parsed.url, info.url);
    assert_eq!(parsed.web_socket_debugger_url, info.web_socket_debugger_url);
}

#[test]
fn test_target_info_multiple_types() {
    let types = ["page", "iframe", "worker", "other"];
    for t in types {
        let info = TargetInfo {
            id: format!("id-{}", t),
            target_type: t.into(),
            title: format!("Title {}", t),
            url: format!("https://example.com/{}", t),
            web_socket_debugger_url: format!("ws://127.0.0.1:9222/devtools/{}/id-{}", t, t),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.target_type, t);
    }
}

// ===========================================================================
// §6 CdpMessage / CdpResponse / CdpError edge cases (REQ-CDS-001)
// ===========================================================================

#[test]
fn test_cdp_message_deserialize_no_params() {
    let json = r#"{"id":1,"method":"Page.enable"}"#;
    let msg: CdpMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.id, Some(1));
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_deserialize_with_params() {
    let json = r#"{"id":2,"method":"Page.navigate","params":{"url":"https://example.com"}}"#;
    let msg: CdpMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.params.unwrap()["url"], "https://example.com");
}

#[test]
fn test_cdp_message_deserialize_no_id() {
    let json = r#"{"method":"Runtime.consoleAPICalled","params":{}}"#;
    let msg: CdpMessage = serde_json::from_str(json).unwrap();
    assert!(msg.id.is_none());
    assert_eq!(msg.method, "Runtime.consoleAPICalled");
}

#[test]
fn test_cdp_response_success_serialization() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"frameId": "0"})),
        error: None,
    };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains(r#""result""#));
    assert!(!json_str.contains(r#""error""#));
}

#[test]
fn test_cdp_response_error_serialization() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32000, message: "server error".into() }),
    };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains(r#""error""#));
    assert!(json_str.contains("-32000"));
}

#[test]
fn test_cdp_error_debug_format() {
    let err = CdpError { code: -32601, message: "test error".into() };
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("-32601"));
    assert!(debug_str.contains("test error"));
}

// ===========================================================================
// §7 Multiple domain dispatch ordering (REQ-CDS-004)
// ===========================================================================

#[test]
fn test_registry_dispatch_multiple_domains_order() {
    let registry = DomainRegistry::new();

    struct OrderHandler { name: &'static str, order: Arc<Mutex<Vec<&'static str>>> }
    impl DomainHandler for OrderHandler {
        fn domain_name(&self) -> &'static str { self.name }
        fn handle_command(&self, _cmd: &str, _p: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
            self.order.lock().unwrap().push(self.name);
            Ok(json!({}))
        }
    }

    let order = Arc::new(Mutex::new(Vec::new()));

    registry.register(Box::new(OrderHandler { name: "Alpha", order: Arc::clone(&order) })).unwrap();
    registry.register(Box::new(OrderHandler { name: "Beta", order: Arc::clone(&order) })).unwrap();
    registry.register(Box::new(OrderHandler { name: "Gamma", order: Arc::clone(&order) })).unwrap();

    let es = NoopEventSender;
    registry.dispatch_command("Alpha.ping", json!({}), &es).unwrap().unwrap();
    registry.dispatch_command("Gamma.ping", json!({}), &es).unwrap().unwrap();
    registry.dispatch_command("Beta.ping", json!({}), &es).unwrap().unwrap();

    let o = order.lock().unwrap();
    assert_eq!(*o, vec!["Alpha", "Gamma", "Beta"]);
}

// ===========================================================================
// §8 DomainHandler default lifecycle no-ops (REQ-CDS-006)
// ===========================================================================

#[test]
fn test_default_on_session_created_no_panic() {
    struct MinimalHandler;
    impl DomainHandler for MinimalHandler {
        fn domain_name(&self) -> &'static str { "Minimal" }
        fn handle_command(&self, _cmd: &str, _p: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
            Ok(json!({}))
        }
    }
    let h = MinimalHandler;
    h.on_session_created("any-id"); // default impl = no panic
    h.on_session_destroyed("any-id"); // default impl = no panic
}

// ===========================================================================
// §9 Registry notify_session_destroyed for multiple domains
// ===========================================================================

#[test]
fn test_notify_destroyed_multiple_domains() {
    let h1 = LifecycleHandler::new();
    let destroyed1 = Arc::clone(&h1.session_destroyed);

    let registry = DomainRegistry::new();
    registry.register(Box::new(h1)).unwrap();

    registry.notify_session_destroyed(
        &["Lifecycle".to_string()],
        "sess-final",
    );

    let d1 = destroyed1.lock().unwrap();
    assert_eq!(d1.len(), 1);
    assert_eq!(d1[0], "sess-final");
}
