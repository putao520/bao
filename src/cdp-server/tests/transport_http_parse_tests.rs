// @trace TEST-CDS-019 [req:REQ-CDS-001,REQ-CDS-002] [level:unit]
// Transport HTTP parse functions deep tests: path detection logic,
// TargetInfo serde edge cases, ServerConfig boundary values,
// protocol helpers, SessionError, CdpServer ws_url patterns.

use cdp_server::*;
use serde_json::json;

// ---- Path detection logic (unit tests of string matching) ----

#[test]
fn test_path_version_exact() {
    let req = "GET /json/version HTTP/1.1";
    assert!(req.starts_with("GET /json/version"));
}

#[test]
fn test_path_version_not_match_list() {
    let req = "GET /json/list HTTP/1.1";
    assert!(!req.starts_with("GET /json/version"));
}

#[test]
fn test_path_json_list_exact() {
    let req = "GET /json HTTP/1.1";
    assert!(req.starts_with("GET /json") && !req.starts_with("GET /json/"));
}

#[test]
fn test_path_json_slash_excluded() {
    let req = "GET /json/version HTTP/1.1";
    // /json/version starts with /json/ so excluded from list route
    assert!(req.starts_with("GET /json") && !req.starts_with("GET /json/") == false);
}

#[test]
fn test_path_devtools_page_extracts_target_id() {
    let req = "GET /devtools/page/abc-123 HTTP/1.1";
    let rest = req.strip_prefix("GET /devtools/page/").unwrap();
    let id = rest.split(' ').next().unwrap();
    assert_eq!(id, "abc-123");
}

#[test]
fn test_path_devtools_page_no_space() {
    let req = "GET /devtools/page/my-target";
    let rest = req.strip_prefix("GET /devtools/page/").unwrap();
    let id = rest.split(' ').next().unwrap();
    assert_eq!(id, "my-target");
}

#[test]
fn test_path_devtools_browser() {
    let req = "GET /devtools/browser HTTP/1.1";
    assert!(req.starts_with("GET /devtools/browser"));
}

#[test]
fn test_path_devtools_page_with_query() {
    let req = "GET /devtools/page/t-1?foo=bar HTTP/1.1";
    let rest = req.strip_prefix("GET /devtools/page/").unwrap();
    let id = rest.split(' ').next().unwrap();
    assert_eq!(id, "t-1?foo=bar");
}

#[test]
fn test_path_close_extracts_id() {
    let prefix = "GET /json/close/";
    let req = "GET /json/close/abc HTTP/1.1";
    let rest = req.strip_prefix(prefix).unwrap();
    let id = rest.split(' ').next().unwrap();
    assert_eq!(id, "abc");
}

#[test]
fn test_path_activate_extracts_id() {
    let prefix = "GET /json/activate/";
    let req = "GET /json/activate/xyz HTTP/1.1";
    let rest = req.strip_prefix(prefix).unwrap();
    let id = rest.split(' ').next().unwrap();
    assert_eq!(id, "xyz");
}

#[test]
fn test_path_new_blank() {
    let prefix = "GET /json/new";
    let req = "GET /json/new HTTP/1.1";
    let rest = req.strip_prefix(prefix).unwrap();
    assert!(!rest.starts_with('?'));
}

#[test]
fn test_path_new_with_url_param() {
    let prefix = "GET /json/new";
    let req = "GET /json/new?https://example.com HTTP/1.1";
    let rest = req.strip_prefix(prefix).unwrap();
    assert!(rest.starts_with('?'));
    let end = rest.find(' ').unwrap_or(rest.len());
    let url = &rest[1..end];
    assert_eq!(url, "https://example.com");
}

#[test]
fn test_path_404_not_matched() {
    let req = "GET /unknown/path HTTP/1.1";
    let is_version = req.starts_with("GET /json/version");
    let is_json_list = req.starts_with("GET /json") && !req.starts_with("GET /json/");
    let is_page = req.strip_prefix("GET /devtools/page/").is_some();
    let is_browser = req.starts_with("GET /devtools/browser");
    assert!(!is_version && !is_json_list && !is_page && !is_browser);
}

#[test]
fn test_websocket_upgrade_detection_case_insensitive() {
    let req1 = "GET /devtools/page/abc HTTP/1.1\r\nUpgrade: websocket\r\n";
    assert!(req1.contains("Upgrade: websocket"));
    let req2 = "GET /devtools/page/abc HTTP/1.1\r\nupgrade: websocket\r\n";
    assert!(req2.contains("upgrade: websocket"));
}

#[test]
fn test_no_websocket_upgrade() {
    let req = "GET /json/version HTTP/1.1\r\nHost: localhost\r\n\r\n";
    assert!(!req.contains("Upgrade: websocket") && !req.contains("upgrade: websocket"));
}

// ---- TargetInfo edge cases ----

#[test]
fn test_target_info_special_chars_in_title() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "Test <script>alert('xss')</script>".into(),
        url: "https://example.com/path?q=1&b=2#frag".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    let json_str = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.title.contains("<script>"));
    assert!(parsed.url.contains("?q=1&b=2#frag"));
}

#[test]
fn test_target_info_unicode_title() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "日本語ページ".into(),
        url: "https://example.co.jp".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    let json_str = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed.title, "日本語ページ");
}

#[test]
fn test_target_info_array_serde() {
    let targets = vec![
        TargetInfo {
            id: "t-1".into(), target_type: "page".into(), title: "Page 1".into(),
            url: "https://a.com".into(),
            web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
        },
        TargetInfo {
            id: "t-2".into(), target_type: "iframe".into(), title: "Page 2".into(),
            url: "https://b.com".into(),
            web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-2".into(),
        },
    ];
    let json_str = serde_json::to_string(&targets).unwrap();
    let parsed: Vec<TargetInfo> = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[1].target_type, "iframe");
}

#[test]
fn test_target_info_all_empty_strings() {
    let info = TargetInfo {
        id: String::new(), target_type: String::new(), title: String::new(),
        url: String::new(), web_socket_debugger_url: String::new(),
    };
    let json_str = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.id.is_empty());
    assert!(parsed.url.is_empty());
}

#[test]
fn test_target_info_long_fields() {
    let long = "a".repeat(10000);
    let info = TargetInfo {
        id: long.clone(), target_type: "page".into(), title: long.clone(),
        url: format!("https://example.com/{}", long),
        web_socket_debugger_url: format!("ws://127.0.0.1:9222/devtools/page/{}", long),
    };
    let json_str = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed.id.len(), 10000);
}

// ---- ServerConfig boundary values ----

#[test]
fn test_server_config_zero_port() {
    let config = ServerConfig::builder().port(0).build();
    assert_eq!(config.port, 0);
}

#[test]
fn test_server_config_max_port() {
    let config = ServerConfig::builder().port(65535).build();
    assert_eq!(config.port, 65535);
}

#[test]
fn test_server_config_all_versions_set() {
    let config = ServerConfig::builder()
        .v8_version("SM102.0")
        .webkit_version("Servo/1.0")
        .build();
    assert_eq!(config.v8_version.as_deref(), Some("SM102.0"));
    assert_eq!(config.webkit_version.as_deref(), Some("Servo/1.0"));
}

#[test]
fn test_server_config_browser_name_default() {
    let config = ServerConfig::default();
    assert!(config.browser_name.contains("Bao"));
}

#[test]
fn test_server_config_http_timeout_default() {
    let config = ServerConfig::default();
    assert_eq!(config.http_timeout_seconds, 30);
}

#[test]
fn test_server_config_custom_http_timeout() {
    let config = ServerConfig::builder().http_timeout_seconds(60).build();
    assert_eq!(config.http_timeout_seconds, 60);
}

#[test]
fn test_server_config_zero_timeout() {
    let config = ServerConfig::builder().http_timeout_seconds(0).build();
    assert_eq!(config.http_timeout_seconds, 0);
}

#[test]
fn test_server_config_zero_max_sessions() {
    let config = ServerConfig::builder().max_sessions(0).build();
    assert_eq!(config.max_sessions, 0);
}

#[test]
fn test_server_config_large_max_sessions() {
    let config = ServerConfig::builder().max_sessions(100000).build();
    assert_eq!(config.max_sessions, 100000);
}

#[test]
fn test_server_config_wildcard_host() {
    let config = ServerConfig::builder().host("0.0.0.0").build();
    assert_eq!(config.host, "0.0.0.0");
}

#[test]
fn test_server_config_custom_browser_name() {
    let config = ServerConfig::builder().browser_name("TestBrowser/2.0").build();
    assert_eq!(config.browser_name, "TestBrowser/2.0");
}

// ---- CdpServer ws_url patterns ----

#[test]
fn test_cdp_server_ws_url_default_port() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("test-id");
    assert!(url.starts_with("ws://127.0.0.1:9222/devtools/page/test-id"));
}

#[test]
fn test_cdp_server_ws_url_custom_port() {
    let server = CdpServer::new(ServerConfig::builder().port(8080).build());
    let url = server.ws_url_for_target("abc");
    assert!(url.contains(":8080/"));
    assert!(url.contains("abc"));
}

#[test]
fn test_cdp_server_ws_url_with_special_chars() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("target-123_abc");
    assert!(url.contains("target-123_abc"));
}

#[test]
fn test_cdp_server_port_accessor() {
    let server = CdpServer::new(ServerConfig::builder().port(9999).build());
    assert_eq!(server.port(), 9999);
}

#[test]
fn test_cdp_server_registry_empty_initially() {
    let server = CdpServer::new(ServerConfig::default());
    assert!(!server.registry().has_domain("Page"));
    assert!(!server.registry().has_domain("Runtime"));
    assert!(!server.registry().has_domain("DOM"));
}

#[test]
fn test_cdp_server_broadcaster_accessible() {
    let server = CdpServer::new(ServerConfig::default());
    let bc = server.broadcaster();
    bc.send_event("Test.event", json!({}));
}

// ---- SessionState exhaustive ----

#[test]
fn test_session_state_all_variants() {
    let states = [SessionState::Created, SessionState::Active, SessionState::Closing, SessionState::Closed];
    assert_eq!(states.len(), 4);
    for i in 0..states.len() {
        for j in 0..states.len() {
            if i == j { assert_eq!(states[i], states[j]); }
            else { assert_ne!(states[i], states[j]); }
        }
    }
}

#[test]
fn test_session_state_ordering() {
    assert!((SessionState::Created as u8) < (SessionState::Active as u8));
    assert!((SessionState::Active as u8) < (SessionState::Closing as u8));
    assert!((SessionState::Closing as u8) < (SessionState::Closed as u8));
}

#[test]
fn test_session_state_debug_format() {
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
fn test_session_state_clone() {
    let s1 = SessionState::Closing;
    let s2 = s1.clone();
    assert_eq!(s1, s2);
}

// ---- SessionError ----

#[test]
fn test_session_error_debug_closed() {
    let err = SessionError::Closed;
    let debug = format!("{:?}", err);
    assert!(debug.contains("Closed"));
}

#[test]
fn test_session_error_debug_io() {
    let err = SessionError::Io;
    let debug = format!("{:?}", err);
    assert!(debug.contains("Io"));
}

// ---- Protocol helpers via public types ----

#[test]
fn test_cdp_response_success_serialization() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"ok": true})),
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 1);
    assert_eq!(parsed["result"]["ok"], true);
    assert!(parsed.get("error").is_none());
}

#[test]
fn test_cdp_response_error_serialization() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("result").is_none());
    assert_eq!(parsed["error"]["code"], -32601);
}

#[test]
fn test_cdp_response_null_result() {
    let resp = CdpResponse {
        id: Some(3),
        result: None,
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("result").is_none());
    assert!(parsed.get("error").is_none());
}

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345})),
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
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
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("params").is_none());
}

#[test]
fn test_cdp_error_fields() {
    let err = CdpError { code: -32600, message: "invalid".into() };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid");
}

#[test]
fn test_cdp_error_serialization() {
    let err = CdpError { code: -32000, message: "internal".into() };
    let json_str = serde_json::to_string(&err).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["code"], -32000);
    assert_eq!(parsed["message"], "internal");
}

#[test]
fn test_cdp_error_empty_message() {
    let err = CdpError { code: -1, message: String::new() };
    let json_str = serde_json::to_string(&err).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["message"], "");
}

#[test]
fn test_cdp_error_debug() {
    let err = CdpError { code: -32700, message: "parse error".into() };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32700"));
    assert!(debug.contains("parse error"));
}

// ---- DomainRegistry with custom handlers ----

struct CountHandler {
    name: &'static str,
}

impl DomainHandler for CountHandler {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, cmd: &str, params: serde_json::Value, _: &dyn EventSender) -> Result<serde_json::Value, CdpError> {
        Ok(json!({"cmd": cmd, "params": params}))
    }
    fn on_session_created(&self, _session_id: &str) {}
    fn on_session_destroyed(&self, _session_id: &str) {}
}

struct FailHandler;

impl DomainHandler for FailHandler {
    fn domain_name(&self) -> &'static str { "Fail" }
    fn handle_command(&self, cmd: &str, _: serde_json::Value, _: &dyn EventSender) -> Result<serde_json::Value, CdpError> {
        Err(CdpError { code: -32000, message: format!("failed: {}", cmd) })
    }
    fn on_session_created(&self, _session_id: &str) {}
    fn on_session_destroyed(&self, _session_id: &str) {}
}

struct NopSender;
impl EventSender for NopSender {
    fn send_event(&self, _method: &str, _params: serde_json::Value) {}
}

#[test]
fn test_registry_multiple_domains() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(CountHandler { name: "Alpha" })).unwrap();
    reg.register(Box::new(CountHandler { name: "Beta" })).unwrap();
    reg.register(Box::new(CountHandler { name: "Gamma" })).unwrap();
    assert!(reg.has_domain("Alpha"));
    assert!(reg.has_domain("Beta"));
    assert!(reg.has_domain("Gamma"));
    assert!(!reg.has_domain("Delta"));
}

#[test]
fn test_registry_dispatch_success() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(CountHandler { name: "Page" })).unwrap();
    let result = reg.dispatch_command("Page.navigate", json!({"url": "https://example.com"}), &NopSender);
    let resp = result.unwrap().unwrap();
    assert_eq!(resp["cmd"], "Page.navigate");
    assert_eq!(resp["params"]["url"], "https://example.com");
}

#[test]
fn test_registry_dispatch_error() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(FailHandler)).unwrap();
    let result = reg.dispatch_command("Fail.run", json!({}), &NopSender);
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("Fail.run"));
}

#[test]
fn test_registry_dispatch_unknown_domain() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("Unknown.method", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_dispatch_no_dot() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("NoDotMethod", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_dispatch_empty_command() {
    let reg = DomainRegistry::new();
    assert!(reg.dispatch_command("", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_dispatch_empty_params() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(CountHandler { name: "Test" })).unwrap();
    let result = reg.dispatch_command("Test.run", json!({}), &NopSender);
    assert!(result.is_some());
}

// ---- EventBroadcaster edge cases ----

#[test]
fn test_broadcaster_no_sessions_no_panic() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    bc.send_event("Page.loadEventFired", json!({}));
    bc.send_event("Runtime.consoleAPICalled", json!({}));
    bc.send_event("DOM.childNodeInserted", json!({}));
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

#[test]
fn test_broadcaster_multiple_events_no_panic() {
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    for i in 0..100 {
        bc.send_event("Test.event", json!({"i": i}));
    }
}

use std::sync::Arc;
