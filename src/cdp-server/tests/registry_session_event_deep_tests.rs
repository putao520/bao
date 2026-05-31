// @trace TEST-CDS-009 [req:REQ-CDS-003,REQ-CDS-005,REQ-CDS-006] [level:unit]
// DomainRegistry register/dispatch/notify/has_domain,
// SessionState transitions, protocol types, CdpError/CdpResponse/CdpEvent.

use std::sync::{Arc, Mutex};

use cdp_server::{
    DomainHandler, DomainRegistry, EventSender, CdpError,
    CdpResponse, CdpEvent, SessionState,
};
use serde_json::{Value, json};

// ---- Test helpers: mock DomainHandler with interior mutability ----

struct MockHandler {
    domain: &'static str,
    response: Result<Value, CdpError>,
    session_created_count: Mutex<usize>,
    session_destroyed_count: Mutex<usize>,
    last_command: Mutex<String>,
}

impl MockHandler {
    fn new(domain: &'static str) -> Self {
        MockHandler {
            domain,
            response: Ok(json!({"ok": true})),
            session_created_count: Mutex::new(0),
            session_destroyed_count: Mutex::new(0),
            last_command: Mutex::new(String::new()),
        }
    }

    fn with_response(domain: &'static str, response: Result<Value, CdpError>) -> Self {
        MockHandler {
            domain,
            response,
            session_created_count: Mutex::new(0),
            session_destroyed_count: Mutex::new(0),
            last_command: Mutex::new(String::new()),
        }
    }

    fn created_count(&self) -> usize {
        *self.session_created_count.lock().unwrap()
    }

    fn destroyed_count(&self) -> usize {
        *self.session_destroyed_count.lock().unwrap()
    }

    fn last_command(&self) -> String {
        self.last_command.lock().unwrap().clone()
    }
}

impl DomainHandler for MockHandler {
    fn domain_name(&self) -> &'static str { self.domain }

    fn handle_command(
        &self,
        command: &str,
        _params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        *self.last_command.lock().unwrap() = command.to_string();
        self.response.clone()
    }

    fn on_session_created(&self, _session_id: &str) {
        let mut c = self.session_created_count.lock().unwrap();
        *c += 1;
    }

    fn on_session_destroyed(&self, _session_id: &str) {
        let mut c = self.session_destroyed_count.lock().unwrap();
        *c += 1;
    }
}

// Arc-wrapped MockHandler to observe side effects after registration
struct ObservedHandler {
    inner: Arc<MockHandler>,
}

impl ObservedHandler {
    fn new(inner: Arc<MockHandler>) -> Self {
        ObservedHandler { inner }
    }
}

impl DomainHandler for ObservedHandler {
    fn domain_name(&self) -> &'static str { self.inner.domain_name() }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        es: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        self.inner.handle_command(command, params, es)
    }

    fn on_session_created(&self, sid: &str) {
        self.inner.on_session_created(sid);
    }

    fn on_session_destroyed(&self, sid: &str) {
        self.inner.on_session_destroyed(sid);
    }
}

struct NoopEventSender;
impl EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

static SENDER: NoopEventSender = NoopEventSender;
fn noop_sender() -> &'static dyn EventSender { &SENDER }

// ---- DomainRegistry::new / default ----

#[test]
fn test_registry_new_empty() {
    let reg = DomainRegistry::new();
    assert!(!reg.has_domain("Page"));
    assert!(!reg.has_domain("Runtime"));
}

#[test]
fn test_registry_default_empty() {
    let reg = DomainRegistry::default();
    assert!(!reg.has_domain("Anything"));
}

// ---- DomainRegistry::register ----

#[test]
fn test_register_single_handler() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    assert!(reg.has_domain("Page"));
}

#[test]
fn test_register_multiple_handlers() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    assert!(reg.register(Box::new(MockHandler::new("Runtime"))).is_ok());
    assert!(reg.register(Box::new(MockHandler::new("DOM"))).is_ok());
    assert!(reg.has_domain("Page"));
    assert!(reg.has_domain("Runtime"));
    assert!(reg.has_domain("DOM"));
}

#[test]
fn test_register_duplicate_fails() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    let result = reg.register(Box::new(MockHandler::new("Page")));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already registered"));
}

#[test]
fn test_register_duplicate_preserves_original() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::with_response("Page", Ok(json!({"v": 1}))))).is_ok());
    let _ = reg.register(Box::new(MockHandler::new("Page")));
    let result = reg.dispatch_command("Page.navigate", json!({}), noop_sender());
    assert!(result.is_some());
    let inner = result.unwrap();
    assert!(inner.is_ok());
    assert_eq!(inner.unwrap()["v"], 1);
}

#[test]
fn test_register_many_domains() {
    let reg = DomainRegistry::new();
    let domains = ["Page", "Runtime", "DOM", "Network", "CSS", "Emulation",
                   "Input", "Overlay", "Debugger", "Log", "Fetch"];
    for d in &domains {
        assert!(reg.register(Box::new(MockHandler::new(d))).is_ok());
    }
    for d in &domains {
        assert!(reg.has_domain(d));
    }
}

// ---- DomainRegistry::dispatch_command ----

#[test]
fn test_dispatch_known_domain() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    let result = reg.dispatch_command("Page.navigate", json!({"url": "http://test"}), noop_sender());
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());
}

#[test]
fn test_dispatch_unknown_domain() {
    let reg = DomainRegistry::new();
    let result = reg.dispatch_command("UnknownDomain.method", json!({}), noop_sender());
    assert!(result.is_none());
}

#[test]
fn test_dispatch_extracts_domain_correctly() {
    let mock = Arc::new(MockHandler::new("Runtime"));
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&mock)))).is_ok());

    let _ = reg.dispatch_command("Runtime.evaluate", json!({"expression": "1+1"}), noop_sender());
    assert_eq!(mock.last_command(), "Runtime.evaluate");
}

#[test]
fn test_dispatch_handler_error() {
    let reg = DomainRegistry::new();
    let handler = MockHandler::with_response("Debugger", Err(CdpError {
        code: -32601,
        message: "not found".into(),
    }));
    assert!(reg.register(Box::new(handler)).is_ok());
    let result = reg.dispatch_command("Debugger.invalidMethod", json!({}), noop_sender());
    assert!(result.is_some());
    let inner = result.unwrap();
    assert!(inner.is_err());
    assert_eq!(inner.unwrap_err().code, -32601);
}

#[test]
fn test_dispatch_multiple_domains() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    assert!(reg.register(Box::new(MockHandler::new("Runtime"))).is_ok());

    let r1 = reg.dispatch_command("Page.enable", json!({}), noop_sender());
    let r2 = reg.dispatch_command("Runtime.evaluate", json!({}), noop_sender());
    assert!(r1.is_some());
    assert!(r2.is_some());
    assert!(r1.unwrap().is_ok());
    assert!(r2.unwrap().is_ok());
}

#[test]
fn test_dispatch_no_dot_in_method() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    let result = reg.dispatch_command("Page", json!({}), noop_sender());
    assert!(result.is_some());
}

#[test]
fn test_dispatch_empty_method() {
    let reg = DomainRegistry::new();
    let result = reg.dispatch_command("", json!({}), noop_sender());
    assert!(result.is_none());
}

// ---- DomainRegistry::has_domain ----

#[test]
fn test_has_domain_false_before_register() {
    assert!(!DomainRegistry::new().has_domain("Page"));
}

#[test]
fn test_has_domain_true_after_register() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Network"))).is_ok());
    assert!(reg.has_domain("Network"));
}

#[test]
fn test_has_domain_case_sensitive() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    assert!(reg.has_domain("Page"));
    assert!(!reg.has_domain("page"));
    assert!(!reg.has_domain("PAGE"));
}

// ---- DomainRegistry::notify_session_created ----

#[test]
fn test_notify_session_created_calls_handler() {
    let mock = Arc::new(MockHandler::new("Page"));
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&mock)))).is_ok());

    reg.notify_session_created("Page", "sess-1");
    assert_eq!(mock.created_count(), 1);
}

#[test]
fn test_notify_session_created_unknown_domain() {
    let reg = DomainRegistry::new();
    reg.notify_session_created("Unknown", "sess-1");
    // No panic
}

#[test]
fn test_notify_session_created_multiple() {
    let mock = Arc::new(MockHandler::new("Runtime"));
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&mock)))).is_ok());

    reg.notify_session_created("Runtime", "s1");
    reg.notify_session_created("Runtime", "s2");
    reg.notify_session_created("Runtime", "s3");
    assert_eq!(mock.created_count(), 3);
}

#[test]
fn test_notify_session_created_only_matching_domain() {
    let m_page = Arc::new(MockHandler::new("Page"));
    let m_runtime = Arc::new(MockHandler::new("Runtime"));
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&m_page)))).is_ok());
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&m_runtime)))).is_ok());

    reg.notify_session_created("Page", "s1");
    assert_eq!(m_page.created_count(), 1);
    assert_eq!(m_runtime.created_count(), 0);
}

// ---- DomainRegistry::notify_session_destroyed ----

#[test]
fn test_notify_session_destroyed_calls_matching() {
    let m_page = Arc::new(MockHandler::new("Page"));
    let m_runtime = Arc::new(MockHandler::new("Runtime"));
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&m_page)))).is_ok());
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&m_runtime)))).is_ok());

    reg.notify_session_destroyed(&["Page".to_string()], "s1");
    assert_eq!(m_page.destroyed_count(), 1);
    assert_eq!(m_runtime.destroyed_count(), 0);
}

#[test]
fn test_notify_session_destroyed_multiple_domains() {
    let m1 = Arc::new(MockHandler::new("Page"));
    let m2 = Arc::new(MockHandler::new("Runtime"));
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&m1)))).is_ok());
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&m2)))).is_ok());

    reg.notify_session_destroyed(&["Page".to_string(), "Runtime".to_string()], "s1");
    assert_eq!(m1.destroyed_count(), 1);
    assert_eq!(m2.destroyed_count(), 1);
}

#[test]
fn test_notify_session_destroyed_empty_list() {
    let m = Arc::new(MockHandler::new("Page"));
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&m)))).is_ok());

    reg.notify_session_destroyed(&[], "s1");
    assert_eq!(m.destroyed_count(), 0);
}

#[test]
fn test_notify_session_destroyed_unknown_domain() {
    let reg = DomainRegistry::new();
    reg.notify_session_destroyed(&["Fake".to_string()], "s1");
    // No panic
}

#[test]
fn test_notify_session_destroyed_repeated() {
    let m = Arc::new(MockHandler::new("DOM"));
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(ObservedHandler::new(Arc::clone(&m)))).is_ok());

    reg.notify_session_destroyed(&["DOM".to_string()], "s1");
    reg.notify_session_destroyed(&["DOM".to_string()], "s2");
    assert_eq!(m.destroyed_count(), 2);
}

// ---- DomainRegistry::get (placeholder, returns None) ----

#[test]
fn test_get_returns_none_placeholder() {
    let reg = DomainRegistry::new();
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    assert!(reg.get("Page").is_none());
}

// ---- SessionState enum ----

#[test]
fn test_session_state_variants_differ() {
    let states = [SessionState::Created, SessionState::Active,
                  SessionState::Closing, SessionState::Closed];
    for i in 0..states.len() {
        for j in 0..states.len() {
            if i != j { assert_ne!(states[i], states[j]); }
        }
    }
}

#[test]
fn test_session_state_copy() {
    let s1 = SessionState::Active;
    let s2 = s1;
    assert_eq!(s1, s2);
}

#[test]
fn test_session_state_clone() {
    let s1 = SessionState::Closing;
    let s2 = s1.clone();
    assert_eq!(s1, s2);
}

#[test]
fn test_session_state_debug() {
    assert!(format!("{:?}", SessionState::Created).contains("Created"));
    assert!(format!("{:?}", SessionState::Active).contains("Active"));
    assert!(format!("{:?}", SessionState::Closing).contains("Closing"));
    assert!(format!("{:?}", SessionState::Closed).contains("Closed"));
}

#[test]
fn test_session_state_eq() {
    assert_eq!(SessionState::Created, SessionState::Created);
    assert_eq!(SessionState::Active, SessionState::Active);
    assert_eq!(SessionState::Closing, SessionState::Closing);
    assert_eq!(SessionState::Closed, SessionState::Closed);
}

// ---- SharedRegistry (Arc<DomainRegistry>) ----

#[test]
fn test_shared_registry_arc() {
    let reg = Arc::new(DomainRegistry::new());
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());

    let reg2 = Arc::clone(&reg);
    assert!(reg2.has_domain("Page"));
    let result = reg2.dispatch_command("Page.enable", json!({}), noop_sender());
    assert!(result.is_some());
}

#[test]
fn test_shared_registry_thread_safety() {
    use std::thread;
    let reg = Arc::new(DomainRegistry::new());
    assert!(reg.register(Box::new(MockHandler::new("Page"))).is_ok());
    assert!(reg.register(Box::new(MockHandler::new("Runtime"))).is_ok());

    let reg1 = Arc::clone(&reg);
    let reg2 = Arc::clone(&reg);

    let t1 = thread::spawn(move || {
        assert!(reg1.has_domain("Page"));
        let _ = reg1.dispatch_command("Page.enable", json!({}), noop_sender());
    });
    let t2 = thread::spawn(move || {
        assert!(reg2.has_domain("Runtime"));
        let _ = reg2.dispatch_command("Runtime.evaluate", json!({}), noop_sender());
    });

    t1.join().unwrap();
    t2.join().unwrap();
}

// ---- CdpError ----

#[test]
fn test_cdp_error_fields() {
    let err = CdpError { code: -32601, message: "test error".into() };
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "test error");
}

#[test]
fn test_cdp_error_debug() {
    let err = CdpError { code: -32600, message: "invalid".into() };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32600"));
    assert!(debug.contains("invalid"));
}

#[test]
fn test_cdp_error_serialize() {
    let err = CdpError { code: -32601, message: "not found".into() };
    let json_str = serde_json::to_string(&err).unwrap();
    assert!(json_str.contains("-32601"));
    assert!(json_str.contains("not found"));
}

// ---- CdpResponse ----

#[test]
fn test_cdp_response_ok_fields() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"data": 42})),
        error: None,
    };
    assert_eq!(resp.id, Some(1));
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_cdp_response_error_fields() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "err".into() }),
    };
    assert!(resp.result.is_none());
    assert!(resp.error.is_some());
}

#[test]
fn test_cdp_response_debug() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({})),
        error: None,
    };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("1"));
}

#[test]
fn test_cdp_response_none_id() {
    let resp = CdpResponse {
        id: None,
        result: Some(json!({})),
        error: None,
    };
    assert!(resp.id.is_none());
}

// ---- CdpEvent ----

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.load".into(),
        params: Some(json!({"ts": 1})),
    };
    assert_eq!(ev.method, "Page.load");
    assert!(ev.params.is_some());
}

#[test]
fn test_cdp_event_no_params() {
    let ev = CdpEvent {
        method: "Runtime.consoleAPICalled".into(),
        params: None,
    };
    assert!(ev.params.is_none());
}

#[test]
fn test_cdp_event_debug() {
    let ev = CdpEvent {
        method: "Test.event".into(),
        params: None,
    };
    let debug = format!("{:?}", ev);
    assert!(debug.contains("Test.event"));
}

#[test]
fn test_cdp_event_clone() {
    let ev = CdpEvent {
        method: "Page.load".into(),
        params: Some(json!({"x": 1})),
    };
    let cloned = ev.clone();
    assert_eq!(cloned.method, ev.method);
}

// ---- DomainHandler trait ----

#[test]
fn test_handler_domain_name() {
    let h = MockHandler::new("CustomDomain");
    assert_eq!(h.domain_name(), "CustomDomain");
}

#[test]
fn test_handler_records_command() {
    let h = MockHandler::new("Test");
    let _ = h.handle_command("Test.doSomething", json!({"a": 1}), noop_sender());
    assert_eq!(h.last_command(), "Test.doSomething");
}

#[test]
fn test_handler_session_lifecycle_callbacks() {
    let h = MockHandler::new("Test");
    assert_eq!(h.created_count(), 0);
    assert_eq!(h.destroyed_count(), 0);

    h.on_session_created("s1");
    assert_eq!(h.created_count(), 1);

    h.on_session_created("s2");
    assert_eq!(h.created_count(), 2);

    h.on_session_destroyed("s1");
    assert_eq!(h.destroyed_count(), 1);
}

// ---- ServerConfig ----

#[test]
fn test_server_config_default_host() {
    let cfg = cdp_server::ServerConfig::default();
    assert_eq!(cfg.host, "127.0.0.1");
}

#[test]
fn test_server_config_default_port() {
    assert_eq!(cdp_server::ServerConfig::default().port, 9222);
}

#[test]
fn test_server_config_default_timeout() {
    assert_eq!(cdp_server::ServerConfig::default().http_timeout_seconds, 30);
}

#[test]
fn test_server_config_default_max_sessions() {
    assert_eq!(cdp_server::ServerConfig::default().max_sessions, 100);
}

#[test]
fn test_server_config_default_browser_name() {
    assert_eq!(cdp_server::ServerConfig::default().browser_name, "Bao/0.1.0");
}

#[test]
fn test_server_config_default_protocol_version() {
    assert_eq!(cdp_server::ServerConfig::default().protocol_version, "1.3");
}

#[test]
fn test_server_config_default_optional_none() {
    let cfg = cdp_server::ServerConfig::default();
    assert!(cfg.user_agent.is_none());
    assert!(cfg.v8_version.is_none());
    assert!(cfg.webkit_version.is_none());
}

#[test]
fn test_server_config_builder_full() {
    let cfg = cdp_server::ServerConfig::builder()
        .host("0.0.0.0")
        .port(8080)
        .http_timeout_seconds(60)
        .max_sessions(50)
        .browser_name("TestBrowser")
        .user_agent("TestAgent")
        .v8_version("11.0")
        .webkit_version("537.36")
        .build();
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.http_timeout_seconds, 60);
    assert_eq!(cfg.max_sessions, 50);
    assert_eq!(cfg.browser_name, "TestBrowser");
    assert_eq!(cfg.user_agent.unwrap(), "TestAgent");
    assert_eq!(cfg.v8_version.unwrap(), "11.0");
    assert_eq!(cfg.webkit_version.unwrap(), "537.36");
}

#[test]
fn test_server_config_builder_partial() {
    let cfg = cdp_server::ServerConfig::builder()
        .port(9999)
        .build();
    assert_eq!(cfg.port, 9999);
    assert_eq!(cfg.host, "127.0.0.1"); // default
}

// ---- TargetInfo ----

#[test]
fn test_target_info_fields() {
    let ti = cdp_server::TargetInfo {
        id: "target-1".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "http://test".into(),
        web_socket_debugger_url: "ws://localhost:9222/devtools/page/target-1".into(),
    };
    assert_eq!(ti.id, "target-1");
    assert_eq!(ti.target_type, "page");
    assert_eq!(ti.title, "Test");
    assert_eq!(ti.url, "http://test");
}

#[test]
fn test_target_info_serde_roundtrip() {
    let ti = cdp_server::TargetInfo {
        id: "abc".into(),
        target_type: "page".into(),
        title: "T".into(),
        url: "http://x".into(),
        web_socket_debugger_url: "ws://x".into(),
    };
    let json_str = serde_json::to_string(&ti).unwrap();
    let parsed: cdp_server::TargetInfo = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed.id, "abc");
    assert_eq!(parsed.target_type, "page");
}

#[test]
fn test_target_info_clone() {
    let ti = cdp_server::TargetInfo {
        id: "1".into(),
        target_type: "page".into(),
        title: "A".into(),
        url: "http://a".into(),
        web_socket_debugger_url: "ws://a".into(),
    };
    let cloned = ti.clone();
    assert_eq!(cloned.id, ti.id);
    assert_eq!(cloned.url, ti.url);
}

// ---- NoopEventSender ----

#[test]
fn test_noop_event_sender() {
    let sender = NoopEventSender;
    sender.send_event("Page.load", json!({}));
    sender.send_event("Runtime.consoleAPICalled", json!({"args": []}));
}

// ---- ObservedHandler ----

#[test]
fn test_observed_handler_delegates() {
    let mock = Arc::new(MockHandler::new("Test"));
    let obs = ObservedHandler::new(Arc::clone(&mock));
    assert_eq!(obs.domain_name(), "Test");

    let _ = obs.handle_command("Test.run", json!({}), noop_sender());
    assert_eq!(mock.last_command(), "Test.run");

    obs.on_session_created("s1");
    assert_eq!(mock.created_count(), 1);

    obs.on_session_destroyed("s1");
    assert_eq!(mock.destroyed_count(), 1);
}
