// @trace TEST-CDS-010 [req:REQ-CDS-001] [level:unit]
// ServerConfig + ServerConfigBuilder deep tests:
// default values, builder chaining, all builder methods, build output,
// clone/debug, boundary values.

use cdp_server::ServerConfig;

// ---- ServerConfig default values ----

#[test]
fn test_default_host() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.host, "127.0.0.1");
}

#[test]
fn test_default_port() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.port, 9222);
}

#[test]
fn test_default_http_timeout() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.http_timeout_seconds, 30);
}

#[test]
fn test_default_max_sessions() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.max_sessions, 100);
}

#[test]
fn test_default_browser_name() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.browser_name, "Bao/0.1.0");
}

#[test]
fn test_default_protocol_version() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.protocol_version, "1.3");
}

#[test]
fn test_default_user_agent_none() {
    let cfg = ServerConfig::default();
    assert!(cfg.user_agent.is_none());
}

#[test]
fn test_default_v8_version_none() {
    let cfg = ServerConfig::default();
    assert!(cfg.v8_version.is_none());
}

#[test]
fn test_default_webkit_version_none() {
    let cfg = ServerConfig::default();
    assert!(cfg.webkit_version.is_none());
}

// ---- ServerConfig::builder() ----

#[test]
fn test_builder_returns_default() {
    let cfg = ServerConfig::builder().build();
    let default = ServerConfig::default();
    assert_eq!(cfg.host, default.host);
    assert_eq!(cfg.port, default.port);
}

// ---- Builder methods ----

#[test]
fn test_builder_host() {
    let cfg = ServerConfig::builder().host("0.0.0.0").build();
    assert_eq!(cfg.host, "0.0.0.0");
}

#[test]
fn test_builder_host_custom() {
    let cfg = ServerConfig::builder().host("192.168.1.1").build();
    assert_eq!(cfg.host, "192.168.1.1");
}

#[test]
fn test_builder_port() {
    let cfg = ServerConfig::builder().port(8080).build();
    assert_eq!(cfg.port, 8080);
}

#[test]
fn test_builder_port_zero() {
    let cfg = ServerConfig::builder().port(0).build();
    assert_eq!(cfg.port, 0);
}

#[test]
fn test_builder_port_max() {
    let cfg = ServerConfig::builder().port(65535).build();
    assert_eq!(cfg.port, 65535);
}

#[test]
fn test_builder_http_timeout() {
    let cfg = ServerConfig::builder().http_timeout_seconds(60).build();
    assert_eq!(cfg.http_timeout_seconds, 60);
}

#[test]
fn test_builder_http_timeout_zero() {
    let cfg = ServerConfig::builder().http_timeout_seconds(0).build();
    assert_eq!(cfg.http_timeout_seconds, 0);
}

#[test]
fn test_builder_max_sessions() {
    let cfg = ServerConfig::builder().max_sessions(200).build();
    assert_eq!(cfg.max_sessions, 200);
}

#[test]
fn test_builder_max_sessions_one() {
    let cfg = ServerConfig::builder().max_sessions(1).build();
    assert_eq!(cfg.max_sessions, 1);
}

#[test]
fn test_builder_browser_name() {
    let cfg = ServerConfig::builder().browser_name("CustomBrowser/1.0").build();
    assert_eq!(cfg.browser_name, "CustomBrowser/1.0");
}

#[test]
fn test_builder_user_agent() {
    let cfg = ServerConfig::builder().user_agent("Mozilla/5.0").build();
    assert_eq!(cfg.user_agent.as_deref(), Some("Mozilla/5.0"));
}

#[test]
fn test_builder_user_agent_empty() {
    let cfg = ServerConfig::builder().user_agent("").build();
    assert_eq!(cfg.user_agent.as_deref(), Some(""));
}

#[test]
fn test_builder_v8_version() {
    let cfg = ServerConfig::builder().v8_version("12.3.4").build();
    assert_eq!(cfg.v8_version.as_deref(), Some("12.3.4"));
}

#[test]
fn test_builder_webkit_version() {
    let cfg = ServerConfig::builder().webkit_version("537.36").build();
    assert_eq!(cfg.webkit_version.as_deref(), Some("537.36"));
}

// ---- Builder chaining ----

#[test]
fn test_builder_full_chain() {
    let cfg = ServerConfig::builder()
        .host("10.0.0.1")
        .port(3000)
        .http_timeout_seconds(120)
        .max_sessions(50)
        .browser_name("TestBrowser")
        .user_agent("TestUA")
        .v8_version("v8")
        .webkit_version("wk")
        .build();
    assert_eq!(cfg.host, "10.0.0.1");
    assert_eq!(cfg.port, 3000);
    assert_eq!(cfg.http_timeout_seconds, 120);
    assert_eq!(cfg.max_sessions, 50);
    assert_eq!(cfg.browser_name, "TestBrowser");
    assert_eq!(cfg.user_agent.as_deref(), Some("TestUA"));
    assert_eq!(cfg.v8_version.as_deref(), Some("v8"));
    assert_eq!(cfg.webkit_version.as_deref(), Some("wk"));
}

#[test]
fn test_builder_partial_chain() {
    let cfg = ServerConfig::builder()
        .port(9999)
        .max_sessions(10)
        .build();
    assert_eq!(cfg.port, 9999);
    assert_eq!(cfg.max_sessions, 10);
    // Non-set fields should be defaults
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.http_timeout_seconds, 30);
}

#[test]
fn test_builder_override_order() {
    let cfg = ServerConfig::builder()
        .port(1111)
        .port(2222)
        .build();
    assert_eq!(cfg.port, 2222);
}

#[test]
fn test_builder_host_unicode() {
    let cfg = ServerConfig::builder().host("本地主机").build();
    assert_eq!(cfg.host, "本地主机");
}

#[test]
fn test_builder_browser_name_long() {
    let name = "A".repeat(500);
    let cfg = ServerConfig::builder().browser_name(&name).build();
    assert_eq!(cfg.browser_name.len(), 500);
}

// ---- DomainRegistry edge cases ----

use cdp_server::{DomainRegistry, DomainHandler, EventSender, CdpError};
use serde_json::{json, Value};

struct TestDomain {
    name: &'static str,
}

impl DomainHandler for TestDomain {
    fn domain_name(&self) -> &'static str { self.name }

    fn handle_command(
        &self,
        command: &str,
        _params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Test.ping" => Ok(json!({"pong": true})),
            _ => Err(CdpError { code: -32601, message: "not found".into() }),
        }
    }
}

#[test]
fn test_registry_new() {
    let reg = DomainRegistry::new();
    assert!(!reg.has_domain("Test"));
}

#[test]
fn test_registry_register_and_has() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(TestDomain { name: "Test" })).unwrap();
    assert!(reg.has_domain("Test"));
}

#[test]
fn test_registry_dispatch_registered() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(TestDomain { name: "Test" })).unwrap();
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let result = reg.dispatch_command("Test.ping", json!({}), &Nop);
    assert!(result.is_some());
    let val = result.unwrap();
    assert!(val.is_ok());
    assert_eq!(val.unwrap()["pong"], true);
}

#[test]
fn test_registry_dispatch_unknown_domain() {
    let reg = DomainRegistry::new();
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let result = reg.dispatch_command("Unknown.ping", json!({}), &Nop);
    assert!(result.is_none());
}

#[test]
fn test_registry_dispatch_unknown_command() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(TestDomain { name: "Test" })).unwrap();
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let result = reg.dispatch_command("Test.nonexistent", json!({}), &Nop);
    assert!(result.is_some());
    assert!(result.unwrap().is_err());
}

#[test]
fn test_registry_multiple_domains() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(TestDomain { name: "Alpha" })).unwrap();
    reg.register(Box::new(TestDomain { name: "Beta" })).unwrap();
    reg.register(Box::new(TestDomain { name: "Gamma" })).unwrap();
    assert!(reg.has_domain("Alpha"));
    assert!(reg.has_domain("Beta"));
    assert!(reg.has_domain("Gamma"));
    assert!(!reg.has_domain("Delta"));
}

#[test]
fn test_registry_case_sensitive() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(TestDomain { name: "Test" })).unwrap();
    assert!(reg.has_domain("Test"));
    assert!(!reg.has_domain("test"));
    assert!(!reg.has_domain("TEST"));
}

// ---- EventBroadcaster ----
// EventBroadcaster requires Arc<Mutex<HashMap<String, Arc<Mutex<CdpSession>>>>>
// which needs real CdpSession objects. Test only that the type is publicly accessible.

#[test]
fn test_event_broadcaster_type_accessible() {
    // Verify EventBroadcaster is publicly accessible
    let _ = std::marker::PhantomData::<cdp_server::EventBroadcaster>;
}

// ---- TargetInfo ----

use cdp_server::TargetInfo;

#[test]
fn test_target_info_fields() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    assert_eq!(info.id, "t-1");
    assert_eq!(info.target_type, "page");
    assert_eq!(info.title, "Test");
    assert_eq!(info.url, "https://example.com");
    assert!(info.web_socket_debugger_url.contains("ws://"));
}

#[test]
fn test_target_info_clone() {
    let info = TargetInfo {
        id: "t-2".into(),
        target_type: "page".into(),
        title: "Clone".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: "ws://localhost/t-2".into(),
    };
    let cloned = info.clone();
    assert_eq!(cloned.id, info.id);
    assert_eq!(cloned.url, info.url);
}

#[test]
fn test_target_info_debug() {
    let info = TargetInfo {
        id: "t-debug".into(),
        target_type: "page".into(),
        title: "Debug".into(),
        url: "http://test".into(),
        web_socket_debugger_url: "ws://test/t-debug".into(),
    };
    let debug = format!("{:?}", info);
    assert!(debug.contains("t-debug") || debug.contains("TargetInfo"));
}

#[test]
fn test_target_info_serde() {
    let info = TargetInfo {
        id: "t-serde".into(),
        target_type: "page".into(),
        title: "Serde".into(),
        url: "http://serde".into(),
        web_socket_debugger_url: "ws://serde/t-serde".into(),
    };
    let json_str = serde_json::to_string(&info).unwrap();
    assert!(json_str.contains("t-serde"));
    let parsed: TargetInfo = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed.id, "t-serde");
}

// ---- SessionState enum ----

use cdp_server::SessionState;

#[test]
fn test_session_state_variants() {
    let states = [SessionState::Created, SessionState::Active, SessionState::Closing, SessionState::Closed];
    // All variants are distinct
    for i in 0..states.len() {
        for j in (i+1)..states.len() {
            assert_ne!(states[i], states[j]);
        }
    }
}

#[test]
fn test_session_state_clone() {
    let state = SessionState::Active;
    let cloned = state;
    assert_eq!(state, cloned);
}

#[test]
fn test_session_state_debug() {
    let debug = format!("{:?}", SessionState::Active);
    assert!(debug.contains("Active"));
}

#[test]
fn test_session_state_eq() {
    assert_eq!(SessionState::Created, SessionState::Created);
    assert_ne!(SessionState::Created, SessionState::Closed);
}
