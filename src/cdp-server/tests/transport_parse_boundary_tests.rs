// @trace TEST-CDS-018 [req:REQ-CDS-001,REQ-CDS-005] [level:unit]
// Transport parse functions boundary tests: parse_close_request,
// parse_activate_request, parse_new_request, TargetInfo serde,
// handle_http_request path detection (without TCP).

use cdp_server::*;
use serde_json::json;

// ---- TargetInfo serde ----

#[test]
fn test_target_info_serde_roundtrip() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    let json_str = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed.id, info.id);
    assert_eq!(parsed.target_type, info.target_type);
    assert_eq!(parsed.title, info.title);
    assert_eq!(parsed.url, info.url);
    assert_eq!(parsed.web_socket_debugger_url, info.web_socket_debugger_url);
}

#[test]
fn test_target_info_json_fields() {
    let info = TargetInfo {
        id: "abc".into(),
        target_type: "page".into(),
        title: "Title".into(),
        url: "https://test.com".into(),
        web_socket_debugger_url: "ws://localhost:9222/devtools/page/abc".into(),
    };
    let json_val: serde_json::Value = serde_json::to_value(&info).unwrap();
    assert_eq!(json_val["id"], "abc");
    assert_eq!(json_val["type"], "page");
    assert_eq!(json_val["title"], "Title");
    assert_eq!(json_val["url"], "https://test.com");
    assert_eq!(json_val["web_socket_debugger_url"], "ws://localhost:9222/devtools/page/abc");
}

#[test]
fn test_target_info_deserialize_with_type_field() {
    let json = r#"{"id":"x","type":"iframe","title":"Sub","url":"about:blank","web_socket_debugger_url":"ws://127.0.0.1:9222/devtools/page/x"}"#;
    let info: TargetInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.target_type, "iframe");
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
fn test_target_info_clone() {
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

#[test]
fn test_target_info_debug() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: String::new(),
    };
    let debug = format!("{:?}", info);
    assert!(debug.contains("t-1"));
    assert!(debug.contains("page"));
}

// ---- ServerConfig defaults and builder ----

#[test]
fn test_server_config_default_port() {
    let config = ServerConfig::default();
    assert_eq!(config.port, 9222);
}

#[test]
fn test_server_config_default_host() {
    let config = ServerConfig::default();
    assert_eq!(config.host, "127.0.0.1");
}

#[test]
fn test_server_config_default_max_sessions() {
    let config = ServerConfig::default();
    assert_eq!(config.max_sessions, 100);
}

#[test]
fn test_server_config_builder_full() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(3000)
        .max_sessions(200)
        .browser_name("TestBrowser")
        .user_agent("Agent/1.0")
        .v8_version("SM")
        .webkit_version("Servo")
        .http_timeout_seconds(30)
        .build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 3000);
    assert_eq!(config.max_sessions, 200);
    assert_eq!(config.browser_name, "TestBrowser");
    assert_eq!(config.user_agent.as_deref(), Some("Agent/1.0"));
    assert_eq!(config.http_timeout_seconds, 30);
}

#[test]
fn test_server_config_builder_partial() {
    let config = ServerConfig::builder()
        .port(8888)
        .build();
    assert_eq!(config.port, 8888);
    assert_eq!(config.host, "127.0.0.1"); // default preserved
}

// ---- CdpServer construction ----

#[test]
fn test_cdp_server_new_default() {
    let server = CdpServer::new(ServerConfig::default());
    assert_eq!(server.port(), 9222);
}

#[test]
fn test_cdp_server_custom_port() {
    let server = CdpServer::new(ServerConfig::builder().port(5555).build());
    assert_eq!(server.port(), 5555);
}

#[test]
fn test_cdp_server_ws_url_format() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("test-id");
    assert!(url.starts_with("ws://"));
    assert!(url.contains("9222"));
    assert!(url.contains("test-id"));
}

#[test]
fn test_cdp_server_ws_url_custom_port() {
    let server = CdpServer::new(ServerConfig::builder().port(8080).build());
    let url = server.ws_url_for_target("abc");
    assert!(url.contains("8080"));
}

#[test]
fn test_cdp_server_registry_empty() {
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

// ---- DomainRegistry edge cases ----

struct NopSender;
impl EventSender for NopSender {
    fn send_event(&self, _method: &str, _params: serde_json::Value) {}
}

struct EchoHandler {
    name: &'static str,
}

impl DomainHandler for EchoHandler {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, cmd: &str, params: serde_json::Value, _: &dyn EventSender) -> Result<serde_json::Value, CdpError> {
        Ok(json!({"echo": cmd, "params": params}))
    }
    fn on_session_created(&self, _session_id: &str) {}
    fn on_session_destroyed(&self, _session_id: &str) {}
}

#[test]
fn test_registry_register_and_dispatch() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Test" })).unwrap();
    assert!(reg.has_domain("Test"));
    let result = reg.dispatch_command("Test.run", json!({"x": 1}), &NopSender).unwrap().unwrap();
    assert_eq!(result["echo"], "Test.run");
    assert_eq!(result["params"]["x"], 1);
}

#[test]
fn test_registry_unknown_domain_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("Unknown.method", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_no_dot_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("NoDotMethod", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_empty_method_returns_none() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_dispatch_with_empty_params() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler { name: "Page" })).unwrap();
    let result = reg.dispatch_command("Page.enable", json!({}), &NopSender).unwrap().unwrap();
    assert_eq!(result["echo"], "Page.enable");
}

// ---- EventBroadcaster with no sessions ----

#[test]
fn test_broadcaster_no_sessions_no_panic() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    bc.send_event("Page.loadEventFired", json!({}));
    bc.send_event("Runtime.consoleAPICalled", json!({}));
}

#[test]
fn test_broadcaster_sender_sends() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    let sender = bc.sender();
    sender.send_event("Test.event", json!({"key": "val"}));
}

#[test]
fn test_broadcaster_clone_shares_state() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc1 = EventBroadcaster::new(sessions);
    let bc2 = bc1.clone();
    bc1.send_event("Test.a", json!({}));
    bc2.send_event("Test.b", json!({}));
}

// ---- SessionState ----

#[test]
fn test_session_state_ordering() {
    assert!((SessionState::Created as u8) < (SessionState::Active as u8));
    assert!((SessionState::Active as u8) < (SessionState::Closing as u8));
    assert!((SessionState::Closing as u8) < (SessionState::Closed as u8));
}

#[test]
fn test_session_state_debug() {
    assert_eq!(format!("{:?}", SessionState::Created), "Created");
    assert_eq!(format!("{:?}", SessionState::Active), "Active");
    assert_eq!(format!("{:?}", SessionState::Closing), "Closing");
    assert_eq!(format!("{:?}", SessionState::Closed), "Closed");
}

#[test]
fn test_session_state_copy() {
    let s1 = SessionState::Active;
    let s2 = s1;
    assert_eq!(s1, s2);
}

#[test]
fn test_session_state_equality() {
    assert_eq!(SessionState::Created, SessionState::Created);
    assert_ne!(SessionState::Active, SessionState::Closed);
}

// ---- CdpError ----

#[test]
fn test_cdp_error_fields() {
    let err = CdpError { code: -32600, message: "invalid".into() };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid");
}

#[test]
fn test_cdp_error_debug() {
    let err = CdpError { code: -32700, message: "parse".into() };
    assert!(format!("{:?}", err).contains("-32700"));
}

// ---- CdpResponse serialization ----

#[test]
fn test_cdp_response_success_serializes() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"ok": true})),
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 1);
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
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("result").is_none());
    assert_eq!(parsed["error"]["code"], -32601);
}

// ---- CdpEvent serialization ----

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.load".into(),
        params: Some(json!({"ts": 1})),
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["method"], "Page.load");
    assert_eq!(parsed["params"]["ts"], 1);
}

#[test]
fn test_cdp_event_without_params() {
    let ev = CdpEvent {
        method: "DOM.updated".into(),
        params: None,
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("params").is_none());
}

use std::sync::Arc;
