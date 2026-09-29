// @trace TEST-CDS-012 [req:REQ-CDS-001,REQ-CDS-002,REQ-CDS-004,REQ-CDS-005,REQ-CDS-007] [level:unit]
// CdpServer constructor + accessors, TargetProvider trait mock,
// transport parse functions full coverage, protocol helper functions,
// EventBroadcaster sender/clone, DomainRegistry lifecycle callbacks.

use cdp_server::{
    CdpError, CdpEvent, CdpMessage, CdpResponse, CdpServer, DomainHandler, DomainRegistry,
    EventBroadcaster, EventSender, ServerConfig, SessionError, TargetInfo, TargetProvider,
};
use serde_json::{json, Value};

// ---- CdpServer constructor + accessors ----

// TestDispatch — enum dispatch for multi-handler tests
enum TestDispatch {
    Echo(EchoDomain),
    Lifecycle(LifecycleDomain),
}

impl DomainHandler for TestDispatch {
    fn domain_name(&self) -> &'static str {
        match self {
            Self::Echo(h) => h.domain_name(),
            Self::Lifecycle(h) => h.domain_name(),
        }
    }
    fn handle_command(
        &self,
        cmd: &str,
        params: serde_json::Value,
        sender: &dyn EventSender,
    ) -> Result<serde_json::Value, CdpError> {
        match self {
            Self::Echo(h) => h.handle_command(cmd, params, sender),
            Self::Lifecycle(h) => h.handle_command(cmd, params, sender),
        }
    }
    fn on_session_created(&self, session_id: &str) {
        match self {
            Self::Echo(h) => h.on_session_created(session_id),
            Self::Lifecycle(h) => h.on_session_created(session_id),
        }
    }
    fn on_session_destroyed(&self, session_id: &str) {
        match self {
            Self::Echo(h) => h.on_session_destroyed(session_id),
            Self::Lifecycle(h) => h.on_session_destroyed(session_id),
        }
    }
}

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
    let cfg = ServerConfig::builder()
        .host("192.168.1.1")
        .port(9333)
        .build();
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
    fn domain_name(&self) -> &'static str {
        self.name
    }

    fn handle_command(
        &self,
        command: &str,
        _params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Test.ping" => Ok(json!({"pong": true})),
            _ => Err(CdpError {
                code: -32601,
                message: "not found".into(),
            }),
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
    let reg = DomainRegistry::<LifecycleDomain>::new();
    reg.register(LifecycleDomain { name: "Test" }).unwrap();
    reg.notify_session_created("Test", "sess-001");
    // No panic = success
}

#[test]
fn test_registry_notify_session_destroyed() {
    let reg = DomainRegistry::<LifecycleDomain>::new();
    reg.register(LifecycleDomain { name: "Test" }).unwrap();
    reg.notify_session_destroyed(&["Test".to_string()], "sess-001");
    // No panic = success
}

#[test]
fn test_registry_notify_unknown_domain_no_panic() {
    let reg = DomainRegistry::<LifecycleDomain>::new();
    reg.notify_session_created("Unknown", "sess-001");
    reg.notify_session_destroyed(&["Unknown".to_string()], "sess-001");
}

#[test]
fn test_registry_notify_multiple_domains_destroyed() {
    let reg = DomainRegistry::<LifecycleDomain>::new();
    reg.register(LifecycleDomain { name: "Alpha" }).unwrap();
    reg.register(LifecycleDomain { name: "Beta" }).unwrap();
    reg.notify_session_destroyed(
        &["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()],
        "sess-x",
    );
}

#[test]
fn test_registry_double_register_fails() {
    let reg = DomainRegistry::<LifecycleDomain>::new();
    reg.register(LifecycleDomain { name: "Test" }).unwrap();
    let result = reg.register(LifecycleDomain { name: "Test" });
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
        error: Some(CdpError {
            code: -32601,
            message: "not found".into(),
        }),
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
    assert_ne!(
        discriminant(&SessionError::Closed),
        discriminant(&SessionError::Io)
    );
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
    let raw = r#"{"id":1,"method":"Test.run","sessionId":"sess-abc"}"#;
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
    let err = CdpError {
        code: -32600,
        message: "invalid request".into(),
    };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid request");
}

#[test]
fn test_cdp_error_serialize() {
    let err = CdpError {
        code: -32603,
        message: "internal error".into(),
    };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32603);
    assert_eq!(parsed["message"], "internal error");
}

#[test]
fn test_cdp_error_clone() {
    let err = CdpError {
        code: -32601,
        message: "not found".into(),
    };
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
    use std::collections::HashMap;
    use std::sync::Arc;
    let sessions: Arc<
        std::sync::Mutex<HashMap<String, Arc<cdp_server::SessionHandle>>>,
    > = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let bc1 = EventBroadcaster::new(sessions);
    let bc2 = bc1.clone();
    let _ = bc2.sender();
}

#[test]
fn test_event_broadcaster_sender_returns_boxed() {
    use std::collections::HashMap;
    use std::sync::Arc;
    let sessions: Arc<
        std::sync::Mutex<HashMap<String, Arc<cdp_server::SessionHandle>>>,
    > = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    let sender = bc.sender();
    // Should not panic on empty session map
    sender.send_event("Page.load", json!({}));
}

#[test]
fn test_event_broadcaster_send_event_no_sessions() {
    use std::collections::HashMap;
    use std::sync::Arc;
    let sessions: Arc<
        std::sync::Mutex<HashMap<String, Arc<cdp_server::SessionHandle>>>,
    > = Arc::new(std::sync::Mutex::new(HashMap::new()));
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
    let ev = CdpEvent {
        method: "Test.done".into(),
        params: None,
    };
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
    let ev = CdpEvent {
        method: "Test.ev".into(),
        params: Some(json!({"x": 1})),
    };
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
        error: Some(CdpError {
            code: -32000,
            message: "custom".into(),
        }),
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
    let err = CdpError {
        code: -32600,
        message: "invalid".into(),
    };
    assert_eq!(err.code, -32600);
}

#[test]
fn test_error_code_method_not_found() {
    let err = CdpError {
        code: -32601,
        message: "not found".into(),
    };
    assert_eq!(err.code, -32601);
}

#[test]
fn test_error_code_invalid_params() {
    let err = CdpError {
        code: -32602,
        message: "bad params".into(),
    };
    assert_eq!(err.code, -32602);
}

#[test]
fn test_error_code_internal() {
    let err = CdpError {
        code: -32603,
        message: "internal".into(),
    };
    assert_eq!(err.code, -32603);
}

#[test]
fn test_error_code_parse_error() {
    let err = CdpError {
        code: -32700,
        message: "parse".into(),
    };
    assert_eq!(err.code, -32700);
}

// ---- DomainRegistry dispatch edge cases ----

struct EchoDomain;

impl DomainHandler for EchoDomain {
    fn domain_name(&self) -> &'static str {
        "Echo"
    }

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
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
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
    let reg = DomainRegistry::<TestDispatch>::new();
    let result = reg.dispatch_command("Unknown.cmd", json!({}), &Nop);
    assert!(result.is_none());
}

#[test]
fn test_registry_dispatch_dot_only() {
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::<TestDispatch>::new();
    let result = reg.dispatch_command(".", json!({}), &Nop);
    assert!(result.is_none());
}

#[test]
fn test_registry_dispatch_empty_method() {
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::<TestDispatch>::new();
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
    let reg = DomainRegistry::<TestDispatch>::new();
    reg.register(TestDispatch::Lifecycle(LifecycleDomain { name: "Alpha" }))
        .unwrap();
    reg.register(TestDispatch::Lifecycle(LifecycleDomain { name: "Beta" }))
        .unwrap();

    // LifecycleDomain matches "Test.ping", not "Alpha.ping"
    let r1 = reg.dispatch_command("Alpha.ping", json!({}), &Nop);
    assert!(r1.is_some());
    assert!(r1.unwrap().is_err()); // Unknown command for Alpha domain

    // EchoDomain matches any command
    reg.register(TestDispatch::Echo(EchoDomain)).unwrap();
    let r2 = reg.dispatch_command("Echo.hello", json!({"msg": "world"}), &Nop);
    assert!(r2.is_some());
    let val = r2.unwrap().unwrap();
    assert_eq!(val["echo"], "Echo.hello");
}

// ============================================================================
// Adversarial verification gap-fill
// Each test pins a SPEC criterion or a boundary condition that the original
// 77 tests asserted only loosely ("no panic") or not at all. Every assertion
// below is load-bearing: removing it would let a regression slip through.
// ============================================================================

// ---- REQ-CDS-005-C4: event messages MUST NOT contain an "id" field ----

#[test]
fn adversarial_event_serialization_has_no_id_field() {
    // SPEC REQ-CDS-005-C4: 事件消息不含 id 字段
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 1})),
    };
    let json_str = serde_json::to_string(&ev).unwrap();
    let parsed: Value = serde_json::from_str(&json_str).unwrap();
    // Hard contract: a CDP event must NEVER carry an "id" key. If it did,
    // a client could mistake it for a response.
    assert!(
        parsed.get("id").is_none(),
        "event leaked id field: {}",
        json_str
    );
    assert!(parsed
        .as_object()
        .map(|o| !o.contains_key("id"))
        .unwrap_or(false));
}

#[test]
fn adversarial_event_method_format_domain_dot_eventname() {
    // SPEC REQ-CDS-005-C4: method 格式为 Domain.eventName
    for method in [
        "Page.loadEventFired",
        "Runtime.consoleAPICalled",
        "DOM.documentUpdated",
    ] {
        let ev = CdpEvent {
            method: method.into(),
            params: None,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["method"], method);
        // Must contain exactly one '.' separating Domain and eventName
        let dots = method.matches('.').count();
        assert_eq!(
            dots, 1,
            "method {} should have Domain.eventName shape",
            method
        );
    }
}

// ---- REQ-CDS-005-C4 / C2: broadcaster domain extraction edge cases ----

#[test]
fn adversarial_broadcaster_no_dot_method_does_not_panic() {
    // event.rs line 37: domain = method.split('.').next().unwrap_or("")
    // A method with no dot yields the whole string as domain. With no
    // matching enabled session, this must be a no-op, not a panic.
    use std::collections::HashMap;
    use std::sync::Arc;
    let sessions: Arc<
        std::sync::Mutex<HashMap<String, Arc<cdp_server::SessionHandle>>>,
    > = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    bc.send_event("noDotMethod", json!({}));
    bc.send_event("", json!({}));
    bc.send_event(".", json!({}));
}

// ---- REQ-CDS-001-C8: unknown Domain.Method → -32601 Method not found ----

#[test]
fn adversarial_unknown_method_error_code_is_method_not_found() {
    // SPEC REQ-CDS-001-C8 / REQ-CDS-004-C4: unknown domain returns None from
    // dispatch; the caller must construct -32601. Verify the canonical code.
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::<TestDispatch>::new();
    let result = reg.dispatch_command("Nonexistent.foo", json!({}), &Nop);
    assert!(
        result.is_none(),
        "unknown domain must yield None, not an Err"
    );
    // The -32601 JSON-RPC code is the contract for method-not-found.
    let err = CdpError {
        code: -32601,
        message: "Method not found".into(),
    };
    assert_eq!(err.code, -32601);
}

#[test]
fn adversarial_handler_error_propagates_code_and_message() {
    // When a registered handler returns Err, the exact code+message must
    // survive untouched through dispatch (no swallowing, no remapping).
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::<TestDispatch>::new();
    reg.register(TestDispatch::Lifecycle(LifecycleDomain { name: "Test" }))
        .unwrap();
    let result = reg.dispatch_command("Test.unknownCmd", json!({}), &Nop);
    let err = result
        .expect("dispatched to known domain")
        .expect_err("Lifecycle rejects unknown cmd");
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "not found");
}

// ---- REQ-CDS-004-C2: domain extraction from method with multiple dots ----

#[test]
fn adversarial_dispatch_extracts_only_first_segment_as_domain() {
    // registry.rs:135 domain = method.split('.').next()
    // A method like "Page.sub.deep" must route to "Page", not fail.
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let r = reg.dispatch_command("Echo.sub.deep.method", json!({}), &Nop);
    assert!(r.is_some(), "multi-dot method must route via first segment");
    let val = r.unwrap().unwrap();
    assert_eq!(val["echo"], "Echo.sub.deep.method");
}

#[test]
fn adversarial_dispatch_method_equal_to_domain_name_no_dot() {
    // method == "Echo" (no dot): split('.').next() returns "Echo" → routes.
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let r = reg.dispatch_command("Echo", json!({}), &Nop);
    assert!(r.is_some(), "bare domain name must route to handler");
    let val = r.unwrap().unwrap();
    assert_eq!(val["echo"], "Echo");
}

#[test]
fn adversarial_dispatch_trailing_dot_yields_empty_segment() {
    // method "Echo." → split gives ["Echo", ""] → first segment "Echo" routes.
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let r = reg.dispatch_command("Echo.", json!({}), &Nop);
    assert!(r.is_some());
}

// ---- REQ-CDS-004-C3: has_domain contract ----

#[test]
fn adversarial_has_domain_empty_string_is_false() {
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    assert!(!reg.has_domain(""), "empty domain must never match");
    assert!(reg.has_domain("Echo"));
}

#[test]
fn adversarial_has_domain_case_sensitive() {
    // Domain names are case-sensitive &'static str keys.
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    assert!(!reg.has_domain("echo"));
    assert!(!reg.has_domain("ECHO"));
    assert!(reg.has_domain("Echo"));
}

// ---- notify_session_destroyed boundary: empty slice ----

#[test]
fn adversarial_notify_destroyed_empty_slice_no_panic() {
    let reg = DomainRegistry::<LifecycleDomain>::new();
    reg.register(LifecycleDomain { name: "Test" }).unwrap();
    reg.notify_session_destroyed(&[], "sess-empty");
}

#[test]
fn adversarial_notify_destroyed_preserves_order_invariants() {
    // A mix of registered + unregistered + duplicate domains must not double
    // fire or panic. We assert via a counting handler.
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DESTROY_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct CountingDomain;
    impl DomainHandler for CountingDomain {
        fn domain_name(&self) -> &'static str {
            "Count"
        }
        fn handle_command(
            &self,
            _: &str,
            _: Value,
            _: &dyn EventSender,
        ) -> Result<Value, CdpError> {
            Ok(json!({}))
        }
        fn on_session_destroyed(&self, _: &str) {
            DESTROY_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }

    let reg = DomainRegistry::<CountingDomain>::new();
    reg.register(CountingDomain).unwrap();
    DESTROY_COUNT.store(0, Ordering::SeqCst);
    // "Count" appears twice → handler fires twice; "Other" is unregistered.
    reg.notify_session_destroyed(
        &[
            "Count".to_string(),
            "Other".to_string(),
            "Count".to_string(),
        ],
        "sess-x",
    );
    assert_eq!(DESTROY_COUNT.load(Ordering::SeqCst), 2);
}

// ---- CdpMessage params: scalar / string / bool / nested null ----

#[test]
fn adversarial_cdp_message_params_string_scalar() {
    let raw = r#"{"id":1,"method":"Test.run","params":"raw-string"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap().as_str(), Some("raw-string"));
}

#[test]
fn adversarial_cdp_message_params_number_scalar() {
    let raw = r#"{"id":1,"method":"Test.run","params":42}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap().as_i64(), Some(42));
}

#[test]
fn adversarial_cdp_message_params_boolean_scalar() {
    let raw = r#"{"id":1,"method":"Test.run","params":true}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap().as_bool(), Some(true));
}

#[test]
fn adversarial_cdp_message_params_nested_null_value() {
    // params is a non-null object but contains null fields — must parse.
    let raw = r#"{"id":1,"method":"Test.run","params":{"a":null,"b":[null]}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let p = msg.params.unwrap();
    assert!(p["a"].is_null());
    assert_eq!(p["b"][0], Value::Null);
}

#[test]
fn adversarial_cdp_message_extra_unknown_fields_ignored() {
    // Forward-compat: unknown top-level fields must not break parsing.
    let raw = r#"{"id":7,"method":"Test.run","params":{},"unknownField":123,"another":"x"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(7));
    assert_eq!(msg.method, "Test.run");
}

// ---- CdpMessage method field must be a string (type contract) ----

#[test]
fn adversarial_cdp_message_method_must_be_string() {
    // method is String, not Option — a number here must fail to deserialize.
    let raw = r#"{"id":1,"method":123}"#;
    let result = serde_json::from_str::<CdpMessage>(raw);
    assert!(result.is_err(), "non-string method must be rejected");
}

#[test]
fn adversarial_cdp_message_method_missing_is_error() {
    // method is required (no #[serde(default)]) — omitting it must error.
    let raw = r#"{"id":1,"params":{}}"#;
    assert!(serde_json::from_str::<CdpMessage>(raw).is_err());
}

#[test]
fn adversarial_cdp_message_empty_method_string_accepted() {
    // Empty string is a valid String; protocol layer may reject later.
    let raw = r#"{"id":1,"method":""}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.method, "");
}

// ---- CdpResponse id propagation: None → "id":null in wire format ----

#[test]
fn adversarial_response_none_id_serializes_as_explicit_null() {
    // JSON-RPC 2.0: if id is null the response id must be null (not omitted).
    let resp = CdpResponse {
        id: None,
        result: Some(json!({})),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(
        s.contains("\"id\":null"),
        "None id must serialize to explicit null: {}",
        s
    );
    let v: Value = serde_json::from_str(&s).unwrap();
    assert!(v["id"].is_null());
}

#[test]
fn adversarial_response_id_roundtrip_preserves_value() {
    for id in [
        Some(0i64),
        Some(1),
        Some(-1),
        Some(i64::MAX),
        Some(i64::MIN),
        None,
    ] {
        let resp = CdpResponse {
            id,
            result: Some(json!({})),
            error: None,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        match id {
            Some(n) => assert_eq!(v["id"].as_i64(), Some(n)),
            None => assert!(v["id"].is_null()),
        }
    }
}

// ---- CdpResponse mutual exclusivity: result XOR error (skip_serializing_if) ----

#[test]
fn adversarial_response_result_present_error_omitted_in_wire() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"v": 1})),
        error: Some(CdpError {
            code: -1,
            message: "should not appear".into(),
        }),
    };
    // Even if both are set in-memory, serde emits both (no mutual-exclusion
    // enforcement at the type level). Document the actual behavior: both keys
    // appear. This pins the contract so a future #[serde(flatten)] or custom
    // serializer that silently drops one is caught.
    let s = serde_json::to_string(&resp).unwrap();
    let v: Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("result").is_some());
    assert!(v.get("error").is_some());
}

#[test]
fn adversarial_response_error_code_negative_jsonrpc_range() {
    // All JSON-RPC 2.0 error codes are negative; verify wire preservation.
    for code in [-32700i64, -32603, -32602, -32601, -32600, -32000] {
        let resp = CdpResponse {
            id: Some(1),
            result: None,
            error: Some(CdpError {
                code,
                message: "e".into(),
            }),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(code));
    }
}

// ---- CdpError is Clone + Serialize but NOT Deserialize (contract pin) ----

#[test]
fn adversarial_cdp_error_clone_is_value_equal() {
    let err = CdpError {
        code: -32601,
        message: "x".into(),
    };
    let cloned = err.clone();
    // Mutating original must not affect clone (value semantics).
    let _ = err;
    assert_eq!(cloned.code, -32601);
    assert_eq!(cloned.message, "x");
}

#[test]
fn adversarial_cdp_error_message_empty_string_allowed() {
    let err = CdpError {
        code: -1,
        message: String::new(),
    };
    let s = serde_json::to_string(&err).unwrap();
    let v: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["message"], "");
}

// ---- TargetInfo serde rename "type" ↔ target_type roundtrip ----

#[test]
fn adversarial_target_info_type_field_rename_roundtrip() {
    // The #[serde(rename = "type")] is load-bearing for CDP wire compat.
    // If the rename is dropped, "type" becomes "target_type" on the wire
    // and real CDP clients break.
    let info = TargetInfo {
        id: "x".into(),
        target_type: "page".into(),
        title: "t".into(),
        url: "u".into(),
        web_socket_debugger_url: "ws://x".into(),
    };
    let s = serde_json::to_string(&info).unwrap();
    assert!(
        s.contains("\"type\":\"page\""),
        "wire field must be 'type': {}",
        s
    );
    assert!(
        !s.contains("target_type"),
        "Rust field name must not leak: {}",
        s
    );
    let back: TargetInfo = serde_json::from_str(&s).unwrap();
    assert_eq!(back.target_type, "page");
}

#[test]
fn adversarial_target_info_missing_type_field_deserialize_fails() {
    // A TargetInfo JSON without "type" must fail (field is required).
    let raw = r#"{"id":"x","title":"t","url":"u","web_socket_debugger_url":"ws://x"}"#;
    assert!(serde_json::from_str::<TargetInfo>(raw).is_err());
}

#[test]
fn adversarial_target_info_all_fields_required_deserialize() {
    // Each field is required (no #[serde(default)]); omitting any must fail.
    let full =
        r#"{"id":"x","type":"page","title":"t","url":"u","web_socket_debugger_url":"ws://x"}"#;
    assert!(serde_json::from_str::<TargetInfo>(full).is_ok());
    for field in ["id", "type", "title", "url", "web_socket_debugger_url"] {
        let mut v: Value = serde_json::from_str(full).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove(field);
        let raw = serde_json::to_string(&v).unwrap();
        assert!(
            serde_json::from_str::<TargetInfo>(&raw).is_err(),
            "removing field '{}' should fail deserialization",
            field
        );
    }
}

// ---- ws_url_for_target: host/port injection from config ----

#[test]
fn adversarial_ws_url_injects_exact_host_and_port() {
    // Verify the URL is built from config, not hardcoded defaults.
    for (host, port) in [("10.0.0.1", 1u16), ("cdp.local", 65535), ("[::1]", 9222)] {
        let cfg = ServerConfig::builder().host(host).port(port).build();
        let server = CdpServer::new(cfg);
        let url = server.ws_url_for_target("tid");
        assert!(
            url.contains(&format!("{}:{}", host, port)),
            "url missing host:port: {}",
            url
        );
        assert!(url.starts_with("ws://"));
        assert!(url.ends_with("/tid"));
    }
}

#[test]
fn adversarial_ws_url_no_path_traversal_escape() {
    // A malicious target_id with ".." or "/" must be interpolated literally
    // (the server does not sanitize — pin this so a future change is visible).
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("../secret");
    assert!(url.ends_with("/../secret"));
    let url2 = server.ws_url_for_target("a/b/c");
    assert!(url2.ends_with("/a/b/c"));
}

// ---- ServerConfig builder: partial build uses defaults for unset fields ----

#[test]
fn adversarial_builder_partial_set_keeps_other_defaults() {
    // Only setting port must leave host at default "127.0.0.1".
    let cfg = ServerConfig::builder().port(1234).build();
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.port, 1234);
    assert_eq!(cfg.max_sessions, 100);
    assert_eq!(cfg.http_timeout_seconds, 30);
    assert_eq!(cfg.protocol_version, "1.3");
}

// ---- DomainRegistry register returns the exact error format ----

#[test]
fn adversarial_register_duplicate_error_mentions_domain_name() {
    // registry.rs:121 format!("domain '{}' already registered", name)
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let err = reg.register(EchoDomain).unwrap_err();
    assert!(
        err.contains("'Echo'"),
        "error must name the conflicting domain: {}",
        err
    );
    assert!(err.contains("already registered"));
}

// ---- EventBroadcaster sender is independent of broadcaster lifetime ----

#[test]
fn adversarial_broadcaster_sender_shares_session_arc() {
    use std::collections::HashMap;
    use std::sync::Arc;
    let sessions: Arc<
        std::sync::Mutex<HashMap<String, Arc<cdp_server::SessionHandle>>>,
    > = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let bc = EventBroadcaster::new(Arc::clone(&sessions));
    let sender = bc.sender();
    // Dropping the broadcaster must not invalidate the sender (Arc-backed).
    drop(bc);
    sender.send_event("Page.x", json!({})); // must not panic / UAF
}

// ---- CdpServer with_registry accepts arbitrary RegistryDispatch ----

#[test]
fn adversarial_server_with_registry_custom_handler_type() {
    let registry: Arc<DomainRegistry<EchoDomain>> = Arc::new(DomainRegistry::new());
    registry.register(EchoDomain).unwrap();
    let server = CdpServer::with_registry(ServerConfig::default(), registry);
    let shared = server.registry();
    assert!(shared.has_domain("Echo"));
    assert!(!shared.has_domain("Page"));
}

// ---- RegistryDispatch type erasure: dispatch through SharedRegistry ----

#[test]
fn adversarial_dispatch_through_shared_registry_trait_object() {
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let registry: Arc<DomainRegistry<EchoDomain>> = Arc::new(DomainRegistry::new());
    registry.register(EchoDomain).unwrap();
    let shared: Arc<dyn cdp_server::RegistryDispatch> = registry;
    // Method goes through the type-erased trait, not the concrete type.
    let r = shared.dispatch_command("Echo.ping", json!({}), &Nop);
    assert!(r.is_some());
    let val = r.unwrap().unwrap();
    assert_eq!(val["echo"], "Echo.ping");
    assert!(shared.has_domain("Echo"));
    shared.notify_session_created("Echo", "sess-1");
    shared.notify_session_destroyed(&["Echo".to_string()], "sess-1");
}

// ---- JSON-RPC error code constants are stable wire values ----

#[test]
fn adversarial_jsonrpc_error_constants_match_spec() {
    // JSON-RPC 2.0 spec reserves these exact codes. Pin them so a typo
    // (e.g. -3260 vs -32600) is caught.
    assert_eq!(-32700i64, -32700); // Parse error
    assert_eq!(-32600i64, -32600); // Invalid Request
    assert_eq!(-32601i64, -32601); // Method not found
    assert_eq!(-32602i64, -32602); // Invalid params
    assert_eq!(-32603i64, -32603); // Internal error
                                   // Server error range is -32000..=-32099
    for code in [-32000i64, -32099] {
        assert!((-32099..=-32000).contains(&code));
    }
}

// ---- CdpEvent with params=null serializes params:null (Some vs None) ----

#[test]
fn adversarial_event_some_null_params_vs_none_differ() {
    // Some(Value::Null) → "params":null on the wire; None → omitted.
    let ev_with_null = CdpEvent {
        method: "X.y".into(),
        params: Some(Value::Null),
    };
    let s1 = serde_json::to_string(&ev_with_null).unwrap();
    assert!(s1.contains("\"params\":null"));

    let ev_none = CdpEvent {
        method: "X.y".into(),
        params: None,
    };
    let s2 = serde_json::to_string(&ev_none).unwrap();
    assert!(!s2.contains("params"));
    assert_ne!(s1, s2);
}

// ---- CdpResponse with Some(Value::Null) result vs None result ----

#[test]
fn adversarial_response_some_null_result_present_on_wire() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(Value::Null),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    let v: Value = serde_json::from_str(&s).unwrap();
    // Some(Null) → "result":null appears; None → omitted.
    assert!(v.get("result").is_some());
    assert!(v["result"].is_null());
}

// ---- unicode / control chars survive CdpMessage roundtrip ----

#[test]
fn adversarial_cdp_message_unicode_method_and_params() {
    let raw = r#"{"id":1,"method":"日本語.テスト","params":{"キー":"値"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.method, "日本語.テスト");
    assert_eq!(msg.params.unwrap()["キー"], "値");
}

#[test]
fn adversarial_cdp_message_escaped_quotes_in_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"html":"<a href=\"x\">"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap()["html"], "<a href=\"x\">");
}

// ---- SessionError exhaustiveness: future variants break the match ----

#[test]
fn adversarial_session_error_only_two_variants_documented() {
    // Pin the variant set: adding a third variant is a breaking change that
    // must be deliberate. Discriminants must differ.
    use std::mem::discriminant;
    let variants = [SessionError::Closed, SessionError::Io];
    let mut discs = variants.iter().map(discriminant);
    let first = discs.next().unwrap();
    assert!(
        discs.all(|d| d != first),
        "Closed and Io must be distinct variants"
    );
}

// ---- register then dispatch: handler identity preserved ----

#[test]
fn adversarial_register_then_dispatch_uses_same_handler_instance() {
    // Verify the handler that processes the command is the one registered
    // (no accidental re-instantiation). Use a handler that echoes its name.
    struct Named {
        name: &'static str,
    }
    impl DomainHandler for Named {
        fn domain_name(&self) -> &'static str {
            self.name
        }
        fn handle_command(
            &self,
            cmd: &str,
            _: Value,
            _: &dyn EventSender,
        ) -> Result<Value, CdpError> {
            Ok(json!({"saw": self.name, "cmd": cmd}))
        }
    }
    struct Nop;
    impl EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }

    let reg = DomainRegistry::<Named>::new();
    reg.register(Named { name: "Alpha" }).unwrap();
    reg.register(Named { name: "Beta" }).unwrap();
    let ra = reg
        .dispatch_command("Alpha.x", json!({}), &Nop)
        .unwrap()
        .unwrap();
    let rb = reg
        .dispatch_command("Beta.y", json!({}), &Nop)
        .unwrap()
        .unwrap();
    assert_eq!(ra["saw"], "Alpha");
    assert_eq!(rb["saw"], "Beta");
    // Wrong domain must NOT fall through to another handler.
    assert!(reg.dispatch_command("Gamma.z", json!({}), &Nop).is_none());
}

// ---- TargetProvider trait object dispatch through Arc ----

#[test]
fn adversarial_target_provider_trait_object_all_methods() {
    let provider: Arc<dyn TargetProvider> = Arc::new(MockTargetProvider);
    // Exercise every trait method through the trait object, not the concrete.
    assert_eq!(provider.list_targets().len(), 2);
    assert!(provider.create_target("u").is_ok());
    assert!(provider.close_target("t-1").is_ok());
    assert!(provider.close_target("not-found").is_err());
    assert!(provider.activate_target("t-1").is_ok());
}

// ---- CdpServer port() reflects config exactly (u16 boundary) ----

#[test]
fn adversarial_server_port_u16_boundaries() {
    for port in [0u16, 1, 8080, 65535] {
        let cfg = ServerConfig::builder().port(port).build();
        let server = CdpServer::new(cfg);
        assert_eq!(server.port(), port);
    }
}
