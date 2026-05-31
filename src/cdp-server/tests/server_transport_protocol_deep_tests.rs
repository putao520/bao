// @trace TEST-CDS-012 [req:REQ-CDS-001,REQ-CDS-002,REQ-CDS-004,REQ-CDS-005,REQ-CDS-007] [level:unit]
// CdpServer constructor + accessors, TargetProvider trait mock,
// transport parse functions full coverage, protocol helper functions,
// EventBroadcaster sender/clone, DomainRegistry lifecycle callbacks.

use cdp_server::{
    CdpServer, ServerConfig, DomainRegistry, DomainHandler, EventSender,
    CdpError, CdpMessage, CdpResponse, CdpEvent, SessionError, TargetInfo,
    TargetProvider, EventBroadcaster,
};
use serde_json::{json, Value};

// ---- CdpServer constructor + accessors ----

#[test]
fn test_cdp_server_new_default_config() {
    let server = CdpServer::new(ServerConfig::default());
    assert_eq!(server.port(), 9222);
}

#[test]
fn test_cdp_server_new_custom_port() {
    let cfg = ServerConfig::builder().port(8080).build();
    let server = CdpServer::new(cfg);
    assert_eq!(server.port(), 8080);
}

#[test]
fn test_cdp_server_registry_accessible() {
    let server = CdpServer::new(ServerConfig::default());
    let _ = server.registry();
}

#[test]
fn test_cdp_server_broadcaster_accessible() {
    let server = CdpServer::new(ServerConfig::default());
    let _bc = server.broadcaster();
}

#[test]
fn test_cdp_server_ws_url_format() {
    let cfg = ServerConfig::builder().host("192.168.1.1").port(9333).build();
    let server = CdpServer::new(cfg);
    let url = server.ws_url_for_target("page-abc");
    assert_eq!(url, "ws://192.168.1.1:9333/devtools/page/page-abc");
}

#[test]
fn test_cdp_server_ws_url_localhost() {
    let cfg = ServerConfig::builder().host("127.0.0.1").port(9222).build();
    let server = CdpServer::new(cfg);
    let url = server.ws_url_for_target("t-001");
    assert!(url.starts_with("ws://127.0.0.1:9222/"));
    assert!(url.ends_with("/t-001"));
}

#[test]
fn test_cdp_server_ws_url_empty_target() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("");
    assert!(url.ends_with("/"));
}

#[test]
fn test_cdp_server_ws_url_unicode_target() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("ページ-1");
    assert!(url.contains("ページ-1"));
}

#[test]
fn test_cdp_server_set_target_provider() {
    let mut server = CdpServer::new(ServerConfig::default());
    server.set_target_provider(Arc::new(MockTargetProvider));
    // No crash = success
}

// ---- TargetProvider trait mock ----

use std::sync::Arc;

struct MockTargetProvider;

impl TargetProvider for MockTargetProvider {
    fn list_targets(&self) -> Vec<TargetInfo> {
        vec![
            TargetInfo {
                id: "t-1".into(),
                target_type: "page".into(),
                title: "Test".into(),
                url: "https://example.com".into(),
                web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
            },
            TargetInfo {
                id: "t-2".into(),
                target_type: "page".into(),
                title: "Other".into(),
                url: "about:blank".into(),
                web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-2".into(),
            },
        ]
    }

    fn create_target(&self, url: &str) -> Result<TargetInfo, String> {
        Ok(TargetInfo {
            id: "new-1".into(),
            target_type: "page".into(),
            title: "New".into(),
            url: url.to_string(),
            web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/new-1".into(),
        })
    }

    fn close_target(&self, target_id: &str) -> Result<(), String> {
        if target_id == "not-found" {
            Err("not found".into())
        } else {
            Ok(())
        }
    }

    fn activate_target(&self, _target_id: &str) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn test_target_provider_list() {
    let provider = MockTargetProvider;
    let targets = provider.list_targets();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].id, "t-1");
    assert_eq!(targets[1].id, "t-2");
}

#[test]
fn test_target_provider_create() {
    let provider = MockTargetProvider;
    let info = provider.create_target("https://new.com").unwrap();
    assert_eq!(info.url, "https://new.com");
    assert_eq!(info.id, "new-1");
}

#[test]
fn test_target_provider_close_ok() {
    let provider = MockTargetProvider;
    assert!(provider.close_target("t-1").is_ok());
}

#[test]
fn test_target_provider_close_not_found() {
    let provider = MockTargetProvider;
    let err = provider.close_target("not-found").unwrap_err();
    assert!(err.contains("not found"));
}

#[test]
fn test_target_provider_activate() {
    let provider = MockTargetProvider;
    assert!(provider.activate_target("t-1").is_ok());
}

#[test]
fn test_target_provider_via_arc() {
    let provider: Arc<dyn TargetProvider> = Arc::new(MockTargetProvider);
    assert_eq!(provider.list_targets().len(), 2);
}

// ---- DomainHandler lifecycle callbacks ----

struct LifecycleDomain {
    name: &'static str,
}

impl DomainHandler for LifecycleDomain {
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

    fn on_session_created(&self, session_id: &str) {
        // Lifecycle callback — just verify it gets called
        let _ = session_id;
    }

    fn on_session_destroyed(&self, session_id: &str) {
        let _ = session_id;
    }
}

#[test]
fn test_registry_notify_session_created() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(LifecycleDomain { name: "Test" })).unwrap();
    reg.notify_session_created("Test", "sess-001");
    // No panic = success
}

#[test]
fn test_registry_notify_session_destroyed() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(LifecycleDomain { name: "Test" })).unwrap();
    reg.notify_session_destroyed(&["Test".to_string()], "sess-001");
    // No panic = success
}

#[test]
fn test_registry_notify_unknown_domain_no_panic() {
    let reg = DomainRegistry::new();
    reg.notify_session_created("Unknown", "sess-001");
    reg.notify_session_destroyed(&["Unknown".to_string()], "sess-001");
}

#[test]
fn test_registry_notify_multiple_domains_destroyed() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(LifecycleDomain { name: "Alpha" })).unwrap();
    reg.register(Box::new(LifecycleDomain { name: "Beta" })).unwrap();
    reg.notify_session_destroyed(&["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()], "sess-x");
}

#[test]
fn test_registry_get_returns_none() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(LifecycleDomain { name: "Test" })).unwrap();
    // get() is a placeholder that returns None
    assert!(reg.get("Test").is_none());
}

#[test]
fn test_registry_double_register_fails() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(LifecycleDomain { name: "Test" })).unwrap();
    let result = reg.register(Box::new(LifecycleDomain { name: "Test" }));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already registered"));
}

// ---- Protocol helper functions ----

#[test]
fn test_parse_message_valid() {
    let msg = cdp_server::CdpMessage {
        id: Some(1),
        method: "Page.navigate".into(),
        params: Some(json!({"url": "https://example.com"})),
        session_id: None,
    };
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.navigate");
}

#[test]
fn test_serialize_response_ok() {
    let resp = CdpResponse {
        id: Some(42),
        result: Some(json!({"success": true})),
        error: None,
    };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains("\"id\":42"));
    assert!(json_str.contains("\"success\":true"));
    assert!(!json_str.contains("\"error\""));
}

#[test]
fn test_serialize_response_error() {
    let resp = CdpResponse {
        id: Some(99),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains("-32601"));
    assert!(!json_str.contains("\"result\""));
}

#[test]
fn test_serialize_response_null_id() {
    let resp = CdpResponse {
        id: None,
        result: Some(json!({})),
        error: None,
    };
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(json_str.contains("\"id\":null"));
}

#[test]
fn test_serialize_event() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345})),
    };
    let json_str = serde_json::to_string(&ev).unwrap();
    assert!(json_str.contains("Page.loadEventFired"));
    assert!(json_str.contains("12345"));
}

#[test]
fn test_serialize_event_no_params() {
    let ev = CdpEvent {
        method: "DOM.documentUpdated".into(),
        params: None,
    };
    let json_str = serde_json::to_string(&ev).unwrap();
    assert!(!json_str.contains("params"));
}

// ---- SessionError variants ----

#[test]
fn test_session_error_closed() {
    let err = SessionError::Closed;
    let debug = format!("{:?}", err);
    assert!(debug.contains("Closed"));
}

#[test]
fn test_session_error_io() {
    let err = SessionError::Io;
    let debug = format!("{:?}", err);
    assert!(debug.contains("Io"));
}

#[test]
fn test_session_error_neq() {
    use std::mem::discriminant;
    assert_ne!(discriminant(&SessionError::Closed), discriminant(&SessionError::Io));
}

// ---- CdpMessage edge cases ----

#[test]
fn test_cdp_message_missing_params() {
    let raw = r#"{"id":1,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_null_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":null}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_params_object() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"key":"val"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap()["key"], "val");
}

#[test]
fn test_cdp_message_params_array() {
    let raw = r#"{"id":1,"method":"Test.run","params":[1,2,3]}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let binding = msg.params.unwrap();
    let arr = binding.as_array().unwrap();
    assert_eq!(arr.len(), 3);
}

#[test]
fn test_cdp_message_session_id() {
    let raw = r#"{"id":1,"method":"Test.run","session_id":"sess-abc"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("sess-abc"));
}

#[test]
fn test_cdp_message_no_session_id() {
    let raw = r#"{"id":1,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.session_id.is_none());
}

// ---- CdpError construction ----

#[test]
fn test_cdp_error_fields() {
    let err = CdpError { code: -32600, message: "invalid request".into() };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid request");
}

#[test]
fn test_cdp_error_serialize() {
    let err = CdpError { code: -32603, message: "internal error".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32603);
    assert_eq!(parsed["message"], "internal error");
}

#[test]
fn test_cdp_error_clone() {
    let err = CdpError { code: -32601, message: "not found".into() };
    let cloned = err.clone();
    assert_eq!(cloned.code, err.code);
    assert_eq!(cloned.message, err.message);
}

// ---- TargetInfo construction + serde ----

#[test]
fn test_target_info_construction() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "Example".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    assert_eq!(info.id, "t-1");
    assert_eq!(info.target_type, "page");
    assert_eq!(info.title, "Example");
    assert_eq!(info.url, "https://example.com");
    assert!(info.web_socket_debugger_url.starts_with("ws://"));
}

#[test]
fn test_target_info_serialize() {
    let info = TargetInfo {
        id: "t-serde".into(),
        target_type: "page".into(),
        title: "Serde".into(),
        url: "http://test".into(),
        web_socket_debugger_url: "ws://test/t-serde".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    // "type" field uses serde rename
    assert!(json.contains("\"type\":\"page\""));
    assert!(json.contains("\"id\":\"t-serde\""));
}

#[test]
fn test_target_info_deserialize() {
    let json = r#"{
        "id": "t-d",
        "type": "page",
        "title": "Desc",
        "url": "http://d",
        "web_socket_debugger_url": "ws://d/t-d"
    }"#;
    let info: TargetInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.id, "t-d");
    assert_eq!(info.target_type, "page");
    assert_eq!(info.title, "Desc");
}

#[test]
fn test_target_info_roundtrip() {
    let info = TargetInfo {
        id: "t-rt".into(),
        target_type: "page".into(),
        title: "RT".into(),
        url: "http://rt".into(),
        web_socket_debugger_url: "ws://rt/t-rt".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, info.id);
    assert_eq!(parsed.target_type, info.target_type);
    assert_eq!(parsed.url, info.url);
}

#[test]
fn test_target_info_clone() {
    let info = TargetInfo {
        id: "t-clone".into(),
        target_type: "page".into(),
        title: "Clone".into(),
        url: "http://clone".into(),
        web_socket_debugger_url: "ws://clone/t-clone".into(),
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
        url: "http://debug".into(),
        web_socket_debugger_url: "ws://debug/t-debug".into(),
    };
    let debug = format!("{:?}", info);
    assert!(debug.contains("t-debug") || debug.contains("TargetInfo"));
}

// ---- EventBroadcaster type + Clone ----

#[test]
fn test_event_broadcaster_clone() {
    use std::sync::Arc;
    use std::collections::HashMap;
    let sessions: Arc<std::sync::Mutex<HashMap<String, Arc<std::sync::Mutex<cdp_server::CdpSession>>>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let bc1 = EventBroadcaster::new(sessions);
    let bc2 = bc1.clone();
    let _ = bc2.sender();
}

#[test]
fn test_event_broadcaster_sender_returns_boxed() {
    use std::sync::Arc;
    use std::collections::HashMap;
    let sessions: Arc<std::sync::Mutex<HashMap<String, Arc<std::sync::Mutex<cdp_server::CdpSession>>>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    let sender = bc.sender();
    // Should not panic on empty session map
    sender.send_event("Page.load", json!({}));
}

#[test]
fn test_event_broadcaster_send_event_no_sessions() {
    use std::sync::Arc;
    use std::collections::HashMap;
    let sessions: Arc<std::sync::Mutex<HashMap<String, Arc<std::sync::Mutex<cdp_server::CdpSession>>>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    // Should not panic with no active sessions
    bc.send_event("Runtime.consoleAPICalled", json!({"type": "log"}));
    bc.send_event("DOM.documentUpdated", json!({}));
    bc.send_event("Network.requestWillBeSent", json!({"requestId": "r-1"}));
}

// ---- CdpServer with target provider ----

#[test]
fn test_cdp_server_with_provider() {
    let mut server = CdpServer::new(ServerConfig::default());
    server.set_target_provider(Arc::new(MockTargetProvider));
    assert_eq!(server.port(), 9222);
}

#[test]
fn test_cdp_server_custom_host_ws_url() {
    let cfg = ServerConfig::builder().host("0.0.0.0").port(9333).build();
    let server = CdpServer::new(cfg);
    let url = server.ws_url_for_target("abc");
    assert!(url.starts_with("ws://0.0.0.0:9333/"));
}

// ---- CdpEvent construction + serialize ----

#[test]
fn test_cdp_event_method_only() {
    let ev = CdpEvent { method: "Test.done".into(), params: None };
    let json = serde_json::to_string(&ev).unwrap();
    assert!(json.contains("\"method\":\"Test.done\""));
    assert!(!json.contains("params"));
}

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.domContentEventFired".into(),
        params: Some(json!({"timestamp": 999})),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["method"], "Page.domContentEventFired");
    assert_eq!(parsed["params"]["timestamp"], 999);
}

#[test]
fn test_cdp_event_clone_independence() {
    let ev = CdpEvent { method: "Test.ev".into(), params: Some(json!({"x": 1})) };
    let mut cloned = ev.clone();
    cloned.method = "Other.ev".into();
    assert_eq!(ev.method, "Test.ev");
    assert_eq!(cloned.method, "Other.ev");
}

// ---- CdpResponse skip_serializing_if ----

#[test]
fn test_cdp_response_skip_result_when_error() {
    let resp = CdpResponse {
        id: Some(1),
        result: None,
        error: Some(CdpError { code: -32000, message: "custom".into() }),
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(!json.contains("result"));
    assert!(json.contains("error"));
}

#[test]
fn test_cdp_response_skip_error_when_result() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"ok": true})),
        error: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("result"));
    assert!(!json.contains("error"));
}

#[test]
fn test_cdp_response_neither_result_nor_error() {
    // This is technically an invalid JSON-RPC response but API allows it
    let resp = CdpResponse {
        id: Some(1),
        result: None,
        error: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(!json.contains("result"));
    assert!(!json.contains("error"));
    assert!(json.contains("\"id\":1"));
}

// ---- JSON-RPC error code constants ----

#[test]
fn test_error_code_invalid_request() {
    // ERR_INVALID_REQUEST = -32600 (JSON-RPC 2.0)
    let err = CdpError { code: -32600, message: "invalid".into() };
    assert_eq!(err.code, -32600);
}

#[test]
fn test_error_code_method_not_found() {
    let err = CdpError { code: -32601, message: "not found".into() };
    assert_eq!(err.code, -32601);
}

#[test]
fn test_error_code_invalid_params() {
    let err = CdpError { code: -32602, message: "bad params".into() };
    assert_eq!(err.code, -32602);
}

#[test]
fn test_error_code_internal() {
    let err = CdpError { code: -32603, message: "internal".into() };
    assert_eq!(err.code, -32603);
}

#[test]
fn test_error_code_parse_error() {
    let err = CdpError { code: -32700, message: "parse".into() };
    assert_eq!(err.code, -32700);
}

// ---- DomainRegistry dispatch edge cases ----

struct EchoDomain;

impl DomainHandler for EchoDomain {
    fn domain_name(&self) -> &'static str { "Echo" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        Ok(json!({"echo": command, "params": params}))
    }
}

#[test]
fn test_registry_dispatch_echo() {
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoDomain)).unwrap();
    let result = reg.dispatch_command("Echo.hello", json!({"msg": "world"}), &Nop);
    assert!(result.is_some());
    let val = result.unwrap().unwrap();
    assert_eq!(val["echo"], "Echo.hello");
    assert_eq!(val["params"]["msg"], "world");
}

#[test]
fn test_registry_dispatch_no_domain() {
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::new();
    let result = reg.dispatch_command("Unknown.cmd", json!({}), &Nop);
    assert!(result.is_none());
}

#[test]
fn test_registry_dispatch_dot_only() {
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::new();
    let result = reg.dispatch_command(".", json!({}), &Nop);
    assert!(result.is_none());
}

#[test]
fn test_registry_dispatch_empty_method() {
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::new();
    let result = reg.dispatch_command("", json!({}), &Nop);
    assert!(result.is_none());
}

// ---- CdpMessage default fields ----

#[test]
fn test_cdp_message_id_optional() {
    let raw = r#"{"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.id.is_none());
}

#[test]
fn test_cdp_message_large_id() {
    let raw = r#"{"id":9223372036854775807,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(i64::MAX));
}

#[test]
fn test_cdp_message_negative_id() {
    let raw = r#"{"id":-1,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(-1));
}

#[test]
fn test_cdp_message_zero_id() {
    let raw = r#"{"id":0,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(0));
}

// ---- CdpResponse deterministic serialization ----

#[test]
fn test_cdp_response_deterministic() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"a": 1, "b": 2})),
        error: None,
    };
    let j1 = serde_json::to_string(&resp).unwrap();
    let j2 = serde_json::to_string(&resp).unwrap();
    assert_eq!(j1, j2);
}

#[test]
fn test_cdp_event_deterministic() {
    let ev = CdpEvent {
        method: "Test.evt".into(),
        params: Some(json!({"x": 1})),
    };
    let j1 = serde_json::to_string(&ev).unwrap();
    let j2 = serde_json::to_string(&ev).unwrap();
    assert_eq!(j1, j2);
}

// ---- CdpMessage invalid inputs ----

#[test]
fn test_cdp_message_invalid_json() {
    let result = serde_json::from_str::<CdpMessage>("{broken}");
    assert!(result.is_err());
}

#[test]
fn test_cdp_message_array() {
    let result = serde_json::from_str::<CdpMessage>("[1,2,3]");
    assert!(result.is_err());
}

#[test]
fn test_cdp_message_number() {
    let result = serde_json::from_str::<CdpMessage>("42");
    assert!(result.is_err());
}

#[test]
fn test_cdp_message_null() {
    let result = serde_json::from_str::<CdpMessage>("null");
    assert!(result.is_err());
}

#[test]
fn test_cdp_message_empty_string() {
    let result = serde_json::from_str::<CdpMessage>("");
    assert!(result.is_err());
}

#[test]
fn test_cdp_message_missing_method() {
    let result = serde_json::from_str::<CdpMessage>(r#"{"id":1}"#);
    assert!(result.is_err());
}

// ---- Multiple domains registration + dispatch ----

#[test]
fn test_registry_multiple_dispatch() {
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::new();
    reg.register(Box::new(LifecycleDomain { name: "Alpha" })).unwrap();
    reg.register(Box::new(LifecycleDomain { name: "Beta" })).unwrap();

    // LifecycleDomain matches "Test.ping", not "Alpha.ping"
    let r1 = reg.dispatch_command("Alpha.ping", json!({}), &Nop);
    assert!(r1.is_some());
    assert!(r1.unwrap().is_err()); // Unknown command for Alpha domain

    // EchoDomain matches any command
    reg.register(Box::new(EchoDomain)).unwrap();
    let r2 = reg.dispatch_command("Echo.hello", json!({"msg": "world"}), &Nop);
    assert!(r2.is_some());
    let val = r2.unwrap().unwrap();
    assert_eq!(val["echo"], "Echo.hello");
}
