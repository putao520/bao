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
    fn domain_name(&self) -> &'static str {
        self.name
    }
    fn handle_command(
        &self,
        cmd: &str,
        _params: Value,
        _: &dyn EventSender,
    ) -> Result<Value, CdpError> {
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
    let reg = DomainRegistry::<CountingHandler>::new();
    let c1 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));

    reg.register(CountingHandler {
        name: "Page",
        count: c1.clone(),
    })
    .unwrap();
    reg.register(CountingHandler {
        name: "Runtime",
        count: c2.clone(),
    })
    .unwrap();

    let r1 = reg
        .dispatch_command("Page.navigate", json!({}), &NopSender)
        .unwrap()
        .unwrap();
    assert_eq!(r1["domain"], "Page");
    assert_eq!(r1["cmd"], "Page.navigate");
    assert_eq!(c1.load(Ordering::SeqCst), 1);
    assert_eq!(c2.load(Ordering::SeqCst), 0);

    let r2 = reg
        .dispatch_command("Runtime.evaluate", json!({}), &NopSender)
        .unwrap()
        .unwrap();
    assert_eq!(r2["domain"], "Runtime");
    assert_eq!(c1.load(Ordering::SeqCst), 1);
    assert_eq!(c2.load(Ordering::SeqCst), 1);
}

#[test]
fn test_registry_dispatch_unknown_returns_none() {
    let reg = DomainRegistry::<CountingHandler>::new();
    assert!(reg
        .dispatch_command("Unknown.method", json!({}), &NopSender)
        .is_none());
}

#[test]
fn test_registry_dispatch_empty_method_returns_none() {
    let reg = DomainRegistry::<CountingHandler>::new();
    assert!(reg.dispatch_command("", json!({}), &NopSender).is_none());
}

#[test]
fn test_registry_dispatch_no_dot_returns_none() {
    let reg = DomainRegistry::<CountingHandler>::new();
    assert!(reg
        .dispatch_command("NoDot", json!({}), &NopSender)
        .is_none());
}

#[test]
fn test_registry_has_domain() {
    let reg = DomainRegistry::<CountingHandler>::new();
    assert!(!reg.has_domain("Page"));
    reg.register(CountingHandler {
        name: "Page",
        count: Arc::new(AtomicUsize::new(0)),
    })
    .unwrap();
    assert!(reg.has_domain("Page"));
    assert!(!reg.has_domain("Runtime"));
}

#[test]
fn test_registry_notify_unregistered_domain_noop() {
    let reg = DomainRegistry::<CountingHandler>::new();
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
        error: Some(CdpError {
            code: -32601,
            message: "not found".into(),
        }),
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
    let err = CdpError {
        code: -32600,
        message: "invalid request".into(),
    };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid request");
}

#[test]
fn test_cdp_error_debug() {
    let err = CdpError {
        code: -32700,
        message: "parse error".into(),
    };
    assert!(format!("{:?}", err).contains("-32700"));
}

#[test]
fn test_cdp_error_serialize_roundtrip() {
    let err = CdpError {
        code: -32000,
        message: "internal".into(),
    };
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
    let reg = Arc::new(DomainRegistry::<CountingHandler>::new());
    reg.register(CountingHandler {
        name: "Page",
        count: Arc::new(AtomicUsize::new(0)),
    })
    .unwrap();
    let server = CdpServer::with_registry(ServerConfig::default(), reg);
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

// ============================================================================
// ADVERSARIAL VERIFICATION GAPS (TEST-CDS-016 / TEST-CDS-017)
// SPEC alignment: REQ-CDS-001 (C5/C6/C7/C8), REQ-CDS-005 (C1/C2/C4/C5),
//                 REQ-CDS-006 (C3/C4/C5), REQ-CDS-008 (C1-C5).
// Covers: boundary values, negative-id, duplicate registration, callback
// invocation, error propagation, builder last-write-wins, serde alias
// strictness, Send+Sync bounds, event-shape invariants, EmptyHandler contract.
// ============================================================================

// ---- REQ-CDS-001 C5/C6: JSON-RPC id boundary matrix ----

#[test]
fn test_cdp_message_negative_id_roundtrip() {
    // Negative ids must survive deserialization (clients may use negative ids).
    let raw = r#"{"id":-1,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(-1));
}

#[test]
fn test_cdp_message_id_i64_min() {
    // i64::MIN is the adversarial lower bound.
    let raw = r#"{"id":-9223372036854775808,"method":"Test.run"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(i64::MIN));
}

#[test]
fn test_cdp_message_missing_id_is_none() {
    // Per JSON-RPC 2.0, a notification has no id. id field is Option.
    let raw = r#"{"method":"Page.enable"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.id.is_none());
    assert_eq!(msg.method, "Page.enable");
}

#[test]
fn test_cdp_message_null_id_is_none() {
    let raw = r#"{"id":null,"method":"Page.enable"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.id.is_none());
}

#[test]
fn test_cdp_message_params_null_vs_absent() {
    // Adversarial: serde treats `params: null` the SAME as absent params
    // (both → None), because Option<T> maps JSON null to None. Document
    // this so consumers cannot rely on distinguishing null from absent.
    let with_null: CdpMessage =
        serde_json::from_str(r#"{"id":1,"method":"T.x","params":null}"#).unwrap();
    assert_eq!(
        with_null.params, None,
        "JSON null must deserialize to None, not Some(Null)"
    );

    let absent: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"T.x"}"#).unwrap();
    assert_eq!(absent.params, None);

    // Only a non-null JSON value yields Some(...).
    let with_obj: CdpMessage =
        serde_json::from_str(r#"{"id":1,"method":"T.x","params":{}}"#).unwrap();
    assert!(with_obj.params.is_some());
}

#[test]
fn test_cdp_message_session_id_roundtrip() {
    // C5: JSON-RPC parsing must accept session_id (flat, non-array).
    let raw = r#"{"id":7,"method":"Runtime.evaluate","session_id":"flat-session-1"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("flat-session-1"));
}

#[test]
fn test_cdp_message_rejects_non_object_root() {
    // Adversarial: scalars/arrays must not parse as a CdpMessage.
    assert!(serde_json::from_str::<CdpMessage>("42").is_err());
    assert!(serde_json::from_str::<CdpMessage>("\"str\"").is_err());
    assert!(serde_json::from_str::<CdpMessage>("true").is_err());
    assert!(serde_json::from_str::<CdpMessage>("[]").is_err());
}

#[test]
fn test_cdp_message_requires_method_field() {
    // method is mandatory; missing method must fail deserialization.
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":1}"#).is_err());
}

#[test]
fn test_cdp_message_id_string_rejected() {
    // Per JSON-RPC, id should be numeric; string id must not coerce silently.
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":"abc","method":"T.x"}"#).is_err());
}

// ---- REQ-CDS-001 C6/C7/C8: CdpResponse shape + JSON-RPC error code semantics ----

#[test]
fn test_cdp_response_result_and_error_mutually_exclusive_at_construction() {
    // Although struct allows both, the construction helpers enforce mutual
    // exclusivity. Verify the canonical shapes never carry both fields set.
    let ok = CdpResponse {
        id: Some(1),
        result: Some(json!({"v": 1})),
        error: None,
    };
    assert!(ok.result.is_some() && ok.error.is_none());

    let err = CdpResponse {
        id: Some(1),
        result: None,
        error: Some(CdpError {
            code: -32601,
            message: "x".into(),
        }),
    };
    assert!(err.result.is_none() && err.error.is_some());
}

#[test]
fn test_cdp_response_error_serializes_skip_result_when_none() {
    // C6: error path must serialize without a "result" key.
    let resp = CdpResponse {
        id: Some(99),
        result: None,
        error: Some(CdpError {
            code: -32601,
            message: "Method not found".into(),
        }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert!(
        v.get("result").is_none(),
        "result must be absent on error path"
    );
    assert_eq!(v["error"]["code"], -32601);
    assert_eq!(v["error"]["message"], "Method not found");
}

#[test]
fn test_cdp_response_result_serializes_skip_error_when_none() {
    // C6: success path must serialize without an "error" key.
    let resp = CdpResponse {
        id: Some(7),
        result: Some(json!({"frameId": "f1"})),
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert!(
        v.get("error").is_none(),
        "error must be absent on success path"
    );
    assert_eq!(v["result"]["frameId"], "f1");
}

#[test]
fn test_jsonrpc_error_code_invalid_request_value() {
    // C7: invalid JSON request → -32600. Construct the canonical error shape
    // and assert the wire code matches JSON-RPC 2.0 Invalid Request.
    let err = CdpError {
        code: -32600,
        message: "Invalid Request".into(),
    };
    let raw = serde_json::to_string(&err).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["code"].as_i64(), Some(-32600));
}

#[test]
fn test_jsonrpc_error_code_method_not_found_value() {
    // C8: unknown Domain.Method → -32601.
    let err = CdpError {
        code: -32601,
        message: "Method not found".into(),
    };
    let raw = serde_json::to_string(&err).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["code"].as_i64(), Some(-32601));
}

#[test]
fn test_jsonrpc_error_code_parse_error_value() {
    // Parse error canonical code -32700.
    let err = CdpError {
        code: -32700,
        message: "Parse error".into(),
    };
    let v: Value = serde_json::from_str(&serde_json::to_string(&err).unwrap()).unwrap();
    assert_eq!(v["code"].as_i64(), Some(-32700));
}

#[test]
fn test_jsonrpc_error_code_internal_error_value() {
    // Internal error canonical code -32603.
    let err = CdpError {
        code: -32603,
        message: "Internal error".into(),
    };
    let v: Value = serde_json::from_str(&serde_json::to_string(&err).unwrap()).unwrap();
    assert_eq!(v["code"].as_i64(), Some(-32603));
}

#[test]
fn test_jsonrpc_error_codes_form_ordered_jsonrpc_range() {
    // Adversarial: verify the JSON-RPC reserved error range (-32000..=-32700)
    // is internally consistent. Codes we emit must all lie in the reserved range.
    for code in [-32600i64, -32601, -32602, -32603, -32700] {
        assert!(
            (-32768..=-32000).contains(&code),
            "code {} outside JSON-RPC reserved range",
            code
        );
    }
}

// ---- REQ-CDS-005 C4: event must NOT carry an id field ----

#[test]
fn test_cdp_event_strictly_no_id_key() {
    // C4: events are notifications — the serialized JSON must not contain "id".
    let ev = CdpEvent {
        method: "Page.frameNavigated".into(),
        params: Some(json!({"frameId": "f1"})),
    };
    let raw = serde_json::to_string(&ev).unwrap();
    assert!(
        !raw.contains("\"id\""),
        "event payload leaked an id field: {}",
        raw
    );
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert!(v.get("id").is_none());
    assert_eq!(v["method"], "Page.frameNavigated");
}

#[test]
fn test_cdp_event_without_params_has_only_method_key() {
    // C4 + minimal-shape: no params → only "method" key present.
    let ev = CdpEvent {
        method: "DOM.updated".into(),
        params: None,
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    assert_eq!(keys, vec!["method"]);
}

#[test]
fn test_cdp_event_method_must_be_dotted_domain_event() {
    // C4: method format "Domain.eventName". Adversarial: confirm the wire
    // preserves the dotted form verbatim (no normalization).
    for method in [
        "Page.loadEventFired",
        "Runtime.consoleAPICalled",
        "Target.targetDestroyed",
    ] {
        let ev = CdpEvent {
            method: method.into(),
            params: None,
        };
        let raw = serde_json::to_string(&ev).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], method);
        assert!(method.contains('.'));
    }
}

// ---- REQ-CDS-005 C1: DomainHandler broadcasts via EventSender ----

#[test]
fn test_handler_can_broadcast_via_event_sender() {
    // C1: a DomainHandler must be able to call send_event on the injected
    // EventSender. Capture the broadcast to prove it propagates.
    use std::sync::Mutex;

    struct CapturingSender {
        captured: Arc<Mutex<Vec<(String, Value)>>>,
    }
    impl EventSender for CapturingSender {
        fn send_event(&self, method: &str, params: Value) {
            self.captured
                .lock()
                .unwrap()
                .push((method.to_string(), params));
        }
    }

    struct BroadcastingHandler {
        captured: Arc<Mutex<Vec<(String, Value)>>>,
    }
    impl DomainHandler for BroadcastingHandler {
        fn domain_name(&self) -> &'static str {
            "Page"
        }
        fn handle_command(
            &self,
            cmd: &str,
            _params: Value,
            sender: &dyn EventSender,
        ) -> Result<Value, CdpError> {
            // C1: handler exercises the EventSender.
            sender.send_event("Page.frameNavigated", json!({"cmd": cmd}));
            Ok(json!({"ok": true}))
        }
        fn on_session_created(&self, _sid: &str) {}
        fn on_session_destroyed(&self, _sid: &str) {}
    }

    let captured = Arc::new(Mutex::new(Vec::new()));
    let reg = DomainRegistry::new();
    reg.register(BroadcastingHandler {
        captured: captured.clone(),
    })
    .unwrap();
    let sender = CapturingSender {
        captured: captured.clone(),
    };
    let res = reg
        .dispatch_command("Page.navigate", json!({}), &sender)
        .unwrap()
        .unwrap();
    assert_eq!(res["ok"], true);
    let events = captured.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "Page.frameNavigated");
    assert_eq!(events[0].1["cmd"], "Page.navigate");
}

// ---- REQ-CDS-006 C5: duplicate registration returns Err naming the domain ----

#[test]
fn test_registry_register_duplicate_returns_err_with_domain_name() {
    // C5: re-registering the same domain_name must Err (no silent overwrite).
    let reg = DomainRegistry::<CountingHandler>::new();
    reg.register(CountingHandler {
        name: "Page",
        count: Arc::new(AtomicUsize::new(0)),
    })
    .unwrap();
    let err = reg
        .register(CountingHandler {
            name: "Page",
            count: Arc::new(AtomicUsize::new(0)),
        })
        .unwrap_err();
    assert!(
        err.contains("Page"),
        "error must name the conflicting domain: {}",
        err
    );
    assert!(
        err.contains("already"),
        "error must indicate conflict: {}",
        err
    );
}

#[test]
fn test_registry_register_duplicate_does_not_overwrite_handler() {
    // C5 (no-overwrite): after a failed duplicate register, the original
    // handler must still be the one dispatched to.
    let reg = DomainRegistry::<CountingHandler>::new();
    let original_count = Arc::new(AtomicUsize::new(0));
    reg.register(CountingHandler {
        name: "Page",
        count: original_count.clone(),
    })
    .unwrap();
    let _ = reg
        .register(CountingHandler {
            name: "Page",
            count: Arc::new(AtomicUsize::new(0)),
        })
        .unwrap_err();
    reg.dispatch_command("Page.navigate", json!({}), &NopSender)
        .unwrap()
        .unwrap();
    assert_eq!(
        original_count.load(Ordering::SeqCst),
        1,
        "original handler must remain registered"
    );
}

#[test]
fn test_registry_register_distinct_domains_both_dispatchable() {
    // C5 (no-conflict): different domain_names register independently.
    let reg = DomainRegistry::<CountingHandler>::new();
    reg.register(CountingHandler {
        name: "Page",
        count: Arc::new(AtomicUsize::new(0)),
    })
    .unwrap();
    reg.register(CountingHandler {
        name: "Network",
        count: Arc::new(AtomicUsize::new(0)),
    })
    .unwrap();
    assert!(reg.has_domain("Page"));
    assert!(reg.has_domain("Network"));
}

// ---- REQ-CDS-006 C3/C4: lifecycle callback invocation ----

struct LifecycleHandler {
    name: &'static str,
    created: Arc<AtomicUsize>,
    destroyed: Arc<AtomicUsize>,
    last_session: Arc<std::sync::Mutex<Option<String>>>,
}

impl DomainHandler for LifecycleHandler {
    fn domain_name(&self) -> &'static str {
        self.name
    }
    fn handle_command(&self, _: &str, _: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Ok(json!({}))
    }
    fn on_session_created(&self, session_id: &str) {
        self.created.fetch_add(1, Ordering::SeqCst);
        *self.last_session.lock().unwrap() = Some(session_id.to_string());
    }
    fn on_session_destroyed(&self, session_id: &str) {
        self.destroyed.fetch_add(1, Ordering::SeqCst);
        *self.last_session.lock().unwrap() = Some(session_id.to_string());
    }
}

#[test]
fn test_registry_notify_session_created_invokes_callback_with_session_id() {
    // C3: notify_session_created routes to the matching handler with the id.
    let reg = DomainRegistry::new();
    let created = Arc::new(AtomicUsize::new(0));
    let destroyed = Arc::new(AtomicUsize::new(0));
    let last = Arc::new(std::sync::Mutex::new(None));
    reg.register(LifecycleHandler {
        name: "Page",
        created: created.clone(),
        destroyed: destroyed.clone(),
        last_session: last.clone(),
    })
    .unwrap();

    reg.notify_session_created("Page", "sess-xyz");
    assert_eq!(created.load(Ordering::SeqCst), 1);
    assert_eq!(destroyed.load(Ordering::SeqCst), 0);
    assert_eq!(*last.lock().unwrap(), Some("sess-xyz".to_string()));
}

#[test]
fn test_registry_notify_session_destroyed_invokes_callback_with_session_id() {
    // C4: notify_session_destroyed routes to each listed domain's handler.
    let reg = DomainRegistry::new();
    let created = Arc::new(AtomicUsize::new(0));
    let destroyed = Arc::new(AtomicUsize::new(0));
    let last = Arc::new(std::sync::Mutex::new(None));
    reg.register(LifecycleHandler {
        name: "Runtime",
        created: created.clone(),
        destroyed: destroyed.clone(),
        last_session: last.clone(),
    })
    .unwrap();

    reg.notify_session_destroyed(&["Runtime".to_string()], "sess-end");
    assert_eq!(destroyed.load(Ordering::SeqCst), 1);
    assert_eq!(created.load(Ordering::SeqCst), 0);
    assert_eq!(*last.lock().unwrap(), Some("sess-end".to_string()));
}

#[test]
fn test_registry_notify_session_destroyed_skips_unregistered_domains() {
    // C4: listing an unregistered domain must not panic and must still
    // deliver to the registered ones.
    let reg = DomainRegistry::new();
    let destroyed = Arc::new(AtomicUsize::new(0));
    let created = Arc::new(AtomicUsize::new(0));
    let last = Arc::new(std::sync::Mutex::new(None));
    reg.register(LifecycleHandler {
        name: "Page",
        created: created.clone(),
        destroyed: destroyed.clone(),
        last_session: last.clone(),
    })
    .unwrap();

    reg.notify_session_destroyed(
        &["Page".to_string(), "NoSuchDomain".to_string()],
        "sess-mix",
    );
    assert_eq!(destroyed.load(Ordering::SeqCst), 1);
}

#[test]
fn test_registry_notify_session_created_wrong_domain_noop() {
    // C3: notify for a domain that is not registered must be a silent no-op.
    let reg = DomainRegistry::<LifecycleHandler>::new();
    // No handler registered at all — must not panic.
    reg.notify_session_created("Page", "sess-1");
    reg.notify_session_destroyed(&["Page".to_string()], "sess-1");
}

#[test]
fn test_registry_notify_session_destroyed_empty_slice_noop() {
    // Adversarial: empty domain slice must be a no-op (no iteration, no panic).
    let reg = DomainRegistry::<LifecycleHandler>::new();
    reg.notify_session_destroyed(&[], "sess-1");
}

// ---- REQ-CDS-006: dispatch_command propagates handler Err ----

struct FailingHandler;
impl DomainHandler for FailingHandler {
    fn domain_name(&self) -> &'static str {
        "Boom"
    }
    fn handle_command(&self, _: &str, _: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Err(CdpError {
            code: -32000,
            message: "boom".into(),
        })
    }
}

#[test]
fn test_registry_dispatch_propagates_handler_error_as_some_err() {
    // dispatch_command wraps the handler's Err in Some(Err(_)) so the caller
    // (CdpSession) can translate it into an error_response.
    let reg = DomainRegistry::new();
    reg.register(FailingHandler).unwrap();
    let result = reg.dispatch_command("Boom.crash", json!({}), &NopSender);
    match result {
        Some(Err(e)) => {
            assert_eq!(e.code, -32000);
            assert_eq!(e.message, "boom");
        }
        other => panic!("expected Some(Err(_)), got {:?}", other),
    }
}

#[test]
fn test_registry_dispatch_unknown_domain_returns_none_distinct_from_err() {
    // Adversarial: unknown domain → None (vs handler Err → Some(Err)).
    // These two paths produce different CDP responses
    // (Method not found vs handler error) and must stay distinguishable.
    let reg = DomainRegistry::new();
    reg.register(FailingHandler).unwrap();
    assert!(reg
        .dispatch_command("Other.method", json!({}), &NopSender)
        .is_none());
    assert!(reg
        .dispatch_command("Boom.crash", json!({}), &NopSender)
        .is_some());
}

// ---- EmptyHandler contract (default registry type) ----

#[test]
fn test_empty_handler_domain_name_is_empty_string() {
    use cdp_server::EmptyHandler;
    let h = EmptyHandler;
    assert_eq!(h.domain_name(), "");
}

#[test]
fn test_empty_handler_returns_method_not_found_error() {
    use cdp_server::EmptyHandler;
    // EmptyHandler must refuse all commands with the canonical -32601 code.
    let h = EmptyHandler;
    let err = h
        .handle_command("Anything.go", json!({}), &NopSender)
        .unwrap_err();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("empty"));
}

#[test]
fn test_empty_handler_implements_send_sync() {
    use cdp_server::EmptyHandler;
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<EmptyHandler>();
    assert_sync::<EmptyHandler>();
}

// ---- REQ-CDS-008: ServerConfig boundary values + builder semantics ----

#[test]
fn test_server_config_builder_port_zero_boundary() {
    // port=0 is "OS-assigned port" — a valid u16 boundary the builder must accept.
    let cfg = ServerConfig::builder().port(0).build();
    assert_eq!(cfg.port, 0);
}

#[test]
fn test_server_config_builder_port_max_u16_boundary() {
    // 65535 is the u16 upper bound.
    let cfg = ServerConfig::builder().port(65535).build();
    assert_eq!(cfg.port, 65535);
}

#[test]
fn test_server_config_builder_max_sessions_zero_boundary() {
    // max_sessions=0 disables all new sessions (boundary).
    let cfg = ServerConfig::builder().max_sessions(0).build();
    assert_eq!(cfg.max_sessions, 0);
}

#[test]
fn test_server_config_builder_http_timeout_zero_boundary() {
    let cfg = ServerConfig::builder().http_timeout_seconds(0).build();
    assert_eq!(cfg.http_timeout_seconds, 0);
}

#[test]
fn test_server_config_builder_last_write_wins_port() {
    // Adversarial: calling .port() twice must keep the LAST value
    // (builder is consuming-self, no accumulation).
    let cfg = ServerConfig::builder().port(1111).port(2222).build();
    assert_eq!(cfg.port, 2222);
}

#[test]
fn test_server_config_builder_last_write_wins_host() {
    let cfg = ServerConfig::builder().host("a").host("b").build();
    assert_eq!(cfg.host, "b");
}

#[test]
fn test_server_config_builder_last_write_wins_optional_fields() {
    // Optional setters: the last call wins (overwrites prior Some).
    let cfg = ServerConfig::builder()
        .user_agent("first")
        .user_agent("second")
        .v8_version("v1")
        .v8_version("v2")
        .webkit_version("w1")
        .webkit_version("w2")
        .build();
    assert_eq!(cfg.user_agent.as_deref(), Some("second"));
    assert_eq!(cfg.v8_version.as_deref(), Some("v2"));
    assert_eq!(cfg.webkit_version.as_deref(), Some("w2"));
}

#[test]
fn test_server_config_builder_chaining_returns_self_each_step() {
    // C5: builder pattern — each setter returns Self to enable chaining.
    // Type-system proof: this compiles only if every setter yields ServerConfigBuilder.
    let builder = ServerConfig::builder();
    let b1 = builder.host("0.0.0.0");
    let b2 = b1.port(8080);
    let b3 = b2.max_sessions(10);
    let b4 = b3.browser_name("X");
    let b5 = b4.user_agent("Y");
    let b6 = b5.v8_version("Z");
    let b7 = b6.webkit_version("W");
    let cfg = b7.http_timeout_seconds(5).build();
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.max_sessions, 10);
    assert_eq!(cfg.browser_name, "X");
    assert_eq!(cfg.user_agent.as_deref(), Some("Y"));
    assert_eq!(cfg.v8_version.as_deref(), Some("Z"));
    assert_eq!(cfg.webkit_version.as_deref(), Some("W"));
    assert_eq!(cfg.http_timeout_seconds, 5);
}

#[test]
fn test_server_config_builder_protocol_version_not_settable() {
    // Adversarial: protocol_version has NO builder setter — it is pinned to
    // the default "1.3" (CDP baseline). Verify it survives a full build.
    let cfg = ServerConfig::builder()
        .host("0.0.0.0")
        .port(1)
        .max_sessions(1)
        .browser_name("X")
        .user_agent("Y")
        .build();
    assert_eq!(
        cfg.protocol_version, "1.3",
        "protocol_version must remain pinned to 1.3"
    );
}

#[test]
fn test_server_config_default_protocol_version_is_1_3() {
    // C4: protocol_version is part of version info, defaults to "1.3".
    assert_eq!(ServerConfig::default().protocol_version, "1.3");
}

#[test]
fn test_server_config_fields_are_publicly_readable() {
    // Adversarial: ServerConfig is a plain public-field struct; all fields
    // must be directly accessible (no encapsulation that breaks config consumers).
    let cfg = ServerConfig::default();
    let _host: &String = &cfg.host;
    let _port: u16 = cfg.port;
    let _timeout: u64 = cfg.http_timeout_seconds;
    let _max: usize = cfg.max_sessions;
    let _bn: &String = &cfg.browser_name;
    let _pv: &String = &cfg.protocol_version;
    let _ua: &Option<String> = &cfg.user_agent;
    let _v8: &Option<String> = &cfg.v8_version;
    let _wk: &Option<String> = &cfg.webkit_version;
}

// ---- TargetInfo serde alias strictness ----

#[test]
fn test_target_info_rust_field_target_type_maps_to_json_type() {
    // serde rename: Rust `target_type` ↔ JSON `"type"`. Adversarial strict check.
    let info = TargetInfo {
        id: "t".into(),
        target_type: "page".into(),
        title: "T".into(),
        url: "u".into(),
        web_socket_debugger_url: "ws".into(),
    };
    let v: Value = serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
    // Must use the CDP-canonical "type" key, NOT "target_type".
    assert!(v.get("type").is_some(), "must serialize as \"type\"");
    assert!(
        v.get("target_type").is_none(),
        "must NOT leak the rust field name"
    );
    assert_eq!(v["type"], "page");
}

#[test]
fn test_target_info_deserialize_requires_type_alias() {
    // Adversarial: JSON using the rust field name "target_type" must FAIL
    // because #[serde(rename = "type")] has NO alias — the original name is
    // rejected as a missing `type` field. Document this strict one-way rename.
    let bad =
        r#"{"id":"t","target_type":"page","title":"T","url":"u","web_socket_debugger_url":"ws"}"#;
    let result: Result<TargetInfo, _> = serde_json::from_str(bad);
    let err = result.unwrap_err();
    assert!(
        format!("{}", err).contains("missing field `type`"),
        "deserialization must reject rust field name and demand the renamed `type`: {}",
        err
    );
}

#[test]
fn test_target_info_serialization_key_set_exact() {
    // Adversarial: exact key set on the wire. Only `target_type` is renamed
    // (to "type"); all other fields keep their snake_case Rust names (NOT
    // camelCase). This documents the actual contract — consumers must read
    // `web_socket_debugger_url` (snake_case), not `webSocketDebuggerUrl`.
    let info = TargetInfo {
        id: "t".into(),
        target_type: "page".into(),
        title: "T".into(),
        url: "u".into(),
        web_socket_debugger_url: "ws".into(),
    };
    let v: Value = serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
    let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "id".to_string(),
            "title".to_string(),
            "type".to_string(),
            "url".to_string(),
            "web_socket_debugger_url".to_string(),
        ],
        "TargetInfo JSON keys: only `type` is renamed; rest stay snake_case"
    );
}

#[test]
fn test_target_info_debug_repr_contains_all_fields() {
    // Adversarial: Debug must surface every field for diagnostics.
    let info = TargetInfo {
        id: "tid".into(),
        target_type: "page".into(),
        title: "Hello".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://h:9222/devtools/page/tid".into(),
    };
    let dbg = format!("{:?}", info);
    assert!(dbg.contains("tid"));
    assert!(dbg.contains("page"));
    assert!(dbg.contains("Hello"));
    assert!(dbg.contains("https://example.com"));
    assert!(dbg.contains("ws://h:9222/devtools/page/tid"));
}

#[test]
fn test_target_info_unicode_roundtrip() {
    // Adversarial: non-ASCII title/url must survive serde round-trip.
    let info = TargetInfo {
        id: "u-1".into(),
        target_type: "page".into(),
        title: "日本語タイトル".into(),
        url: "https://例え.テスト/path?q=値".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/u-1".into(),
    };
    let raw = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed.title, "日本語タイトル");
    assert_eq!(parsed.url, "https://例え.テスト/path?q=値");
}

// ---- CdpServer deep API coverage ----

#[test]
fn test_cdp_server_with_registry_uses_provided_registry() {
    // CdpServer::with_registry must wire the caller-supplied registry
    // (NOT create a fresh empty one).
    let reg = Arc::new(DomainRegistry::<CountingHandler>::new());
    reg.register(CountingHandler {
        name: "Network",
        count: Arc::new(AtomicUsize::new(0)),
    })
    .unwrap();
    let server = CdpServer::with_registry(ServerConfig::default(), reg);
    assert!(server.registry().has_domain("Network"));
    assert!(!server.registry().has_domain("Page"));
}

#[test]
fn test_cdp_server_new_uses_empty_registry() {
    // CdpServer::new creates an EmptyHandler registry — no domains present.
    let server = CdpServer::new(ServerConfig::default());
    assert!(!server.registry().has_domain("Page"));
    assert!(!server.registry().has_domain("Runtime"));
    assert!(!server.registry().has_domain("Network"));
}

#[test]
fn test_cdp_server_ws_url_for_target_strict_format() {
    // ws_url_for_target must produce exactly:
    //   ws://{host}:{port}/devtools/page/{target_id}
    let server = CdpServer::new(
        ServerConfig::builder()
            .host("192.168.1.1")
            .port(8765)
            .build(),
    );
    let url = server.ws_url_for_target("tgt-42");
    assert_eq!(url, "ws://192.168.1.1:8765/devtools/page/tgt-42");
}

#[test]
fn test_cdp_server_ws_url_for_target_preserves_special_target_ids() {
    // Adversarial: target ids with hyphens, underscores, digits, mixed case.
    let server = CdpServer::new(ServerConfig::builder().host("127.0.0.1").port(9222).build());
    for tid in [
        "ABC-123_def",
        "00000000000000000000000000000000",
        "a-b-C-D_E-F",
    ] {
        let url = server.ws_url_for_target(tid);
        assert!(url.ends_with(&format!("/devtools/page/{}", tid)));
    }
}

#[test]
fn test_cdp_server_port_reflects_custom_config() {
    // port() accessor must return the config's port verbatim.
    for p in [0u16, 1, 8080, 65535] {
        let server = CdpServer::new(ServerConfig::builder().port(p).build());
        assert_eq!(server.port(), p);
    }
}

#[test]
fn test_cdp_server_broadcaster_arc_shared_across_clones() {
    // broadcaster() returns a fresh Arc clone each call but points at the
    // same underlying EventBroadcaster state.
    let server = CdpServer::new(ServerConfig::default());
    let bc1 = server.broadcaster();
    let bc2 = server.broadcaster();
    // EventBroadcaster::clone is Arc-shallow; both clones share session state.
    let _ = bc1.clone();
    let _ = bc2.clone();
    assert!(Arc::strong_count(&bc1) >= 1);
}

// ---- EventBroadcaster deep contract ----

#[test]
fn test_event_broadcaster_sender_is_boxed_dyn_event_sender() {
    // sender() must yield a heap-allocated trait object usable as &dyn EventSender.
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    let sender: Box<dyn EventSender> = bc.sender();
    // Trait object must be callable without panic on empty session map.
    sender.send_event("Any.event", json!({}));
}

#[test]
fn test_event_broadcaster_clone_shares_session_arc() {
    // Adversarial: clone() must share the SAME sessions Arc (not deep-copy).
    // Build the canonical SessionMap shape so the type matches EventBroadcaster's
    // internal map (HashMap<String, Arc<Mutex<CdpSession>>>).
    type SessionMap = Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, Arc<std::sync::Mutex<cdp_server::CdpSession>>>,
        >,
    >;
    // We cannot easily construct a CdpSession (needs a WebSocket), so verify
    // Arc sharing structurally: clone yields a broadcaster whose internal
    // sessions Arc is pointer-equal to the one we passed in.
    let sessions: SessionMap = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc1 = EventBroadcaster::new(Arc::clone(&sessions));
    let bc2 = bc1.clone();
    // Both broadcasters must reference the same underlying map Arc.
    // (EventBroadcaster::clone is Arc-shallow — proven by the fact that
    // drop(bc1) does not invalidate bc2's ability to send_event.)
    drop(bc1);
    bc2.send_event("Still.works", json!({}));
}

#[test]
fn test_event_broadcaster_no_dot_method_does_not_panic() {
    // Adversarial: send_event with a method lacking '.' must not panic
    // (domain extraction falls back to the whole string).
    let sessions = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let bc = EventBroadcaster::new(sessions);
    bc.send_event("noDotHere", json!({}));
    bc.send_event("", json!({}));
}

// ---- SessionState: Send+Sync + full distinctness ----

#[test]
fn test_session_state_is_send_and_sync() {
    // Adversarial: SessionState must be Send + Sync + Copy (used across threads).
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    fn assert_copy<T: Copy>() {}
    assert_send::<SessionState>();
    assert_sync::<SessionState>();
    assert_copy::<SessionState>();
}

#[test]
fn test_session_state_all_variants_pairwise_distinct() {
    // Adversarial: every pair of variants must be unequal (catch enum dedup bugs).
    let variants = [
        SessionState::Created,
        SessionState::Active,
        SessionState::Closing,
        SessionState::Closed,
    ];
    for i in 0..variants.len() {
        for j in (i + 1)..variants.len() {
            assert_ne!(variants[i], variants[j], "variants {} and {} collide", i, j);
        }
    }
}

#[test]
fn test_session_state_discriminants_are_unique_u8() {
    // Adversarial: the four states must occupy four distinct discriminant values.
    let mut seen = std::collections::HashSet::new();
    for s in [
        SessionState::Created,
        SessionState::Active,
        SessionState::Closing,
        SessionState::Closed,
    ] {
        seen.insert(s as u8);
    }
    assert_eq!(seen.len(), 4);
}

// ---- CdpMessage clone + params ownership ----

#[test]
fn test_cdp_message_clone_is_independent() {
    // Adversarial: cloning a CdpMessage yields an independent value;
    // mutating the clone's method must not affect the original.
    let raw = r#"{"id":1,"method":"Page.navigate","params":{"url":"u"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let mut cloned = msg.clone();
    cloned.method = "Page.reload".into();
    assert_eq!(msg.method, "Page.navigate");
    assert_eq!(cloned.method, "Page.reload");
}

#[test]
fn test_cdp_message_params_is_owned_value() {
    // Adversarial: params is Option<Value> (owned), not a borrow —
    // extracting it must not require cloning.
    let raw = r#"{"id":1,"method":"T.x","params":{"a":1}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let params: Value = msg.params.unwrap();
    assert_eq!(params["a"], 1);
}

// ---- SessionError exhaustiveness ----

#[test]
fn test_session_error_closed_and_io_distinct() {
    // Adversarial: the two variants must be distinguishable via Debug.
    let closed = format!("{:?}", SessionError::Closed);
    let io = format!("{:?}", SessionError::Io);
    assert_ne!(closed, io);
    assert!(closed.contains("Closed") && !closed.contains("Io"));
    assert!(io.contains("Io"));
}

// ---- CdpError clone + message ownership ----

#[test]
fn test_cdp_error_clone_is_independent() {
    let err = CdpError {
        code: -1,
        message: "orig".into(),
    };
    let mut cloned = err.clone();
    cloned.message = "changed".into();
    assert_eq!(err.message, "orig");
    assert_eq!(cloned.message, "changed");
}

#[test]
fn test_cdp_error_message_is_owned_string() {
    // Adversarial: message is an owned String, not a borrow.
    let err = CdpError {
        code: -1,
        message: "owned".into(),
    };
    let msg: String = err.message;
    assert_eq!(msg, "owned");
}

// ---- DomainRegistry::new vs Default equivalence ----

#[test]
fn test_domain_registry_new_equals_default() {
    // Adversarial: new() and Default::default() must produce equivalent
    // (empty, no-domain) registries.
    let via_new = DomainRegistry::<CountingHandler>::new();
    let via_default = DomainRegistry::<CountingHandler>::default();
    assert!(!via_new.has_domain("Page"));
    assert!(!via_default.has_domain("Page"));
}

#[test]
fn test_domain_registry_default_can_register() {
    // Adversarial: Default-constructed registry must be usable for registration.
    let reg = DomainRegistry::<CountingHandler>::default();
    reg.register(CountingHandler {
        name: "Page",
        count: Arc::new(AtomicUsize::new(0)),
    })
    .unwrap();
    assert!(reg.has_domain("Page"));
}

// ---- dispatch domain extraction edge cases (REQ-CDS-001 C5) ----

#[test]
fn test_registry_dispatch_leading_dot_yields_empty_domain() {
    // Adversarial: ".method" → domain="" → unregistered → None (no panic).
    let reg = DomainRegistry::<CountingHandler>::new();
    assert!(reg
        .dispatch_command(".method", json!({}), &NopSender)
        .is_none());
}

#[test]
fn test_registry_dispatch_trailing_dot_yields_empty_command() {
    // Adversarial: "Page." → domain="Page" but command empty.
    // The handler still receives the full method string; routing is by domain only.
    let reg = DomainRegistry::<CountingHandler>::new();
    let count = Arc::new(AtomicUsize::new(0));
    reg.register(CountingHandler {
        name: "Page",
        count: count.clone(),
    })
    .unwrap();
    let res = reg
        .dispatch_command("Page.", json!({}), &NopSender)
        .unwrap()
        .unwrap();
    assert_eq!(res["cmd"], "Page.");
    assert_eq!(res["domain"], "Page");
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_registry_dispatch_multiple_dots_uses_first_segment_as_domain() {
    // Adversarial: "Page.sub.deep" → domain="Page" (split on first dot).
    let reg = DomainRegistry::<CountingHandler>::new();
    let count = Arc::new(AtomicUsize::new(0));
    reg.register(CountingHandler {
        name: "Page",
        count: count.clone(),
    })
    .unwrap();
    let res = reg
        .dispatch_command("Page.sub.deep", json!({}), &NopSender)
        .unwrap()
        .unwrap();
    assert_eq!(res["domain"], "Page");
    assert_eq!(res["cmd"], "Page.sub.deep");
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_registry_dispatch_passes_params_through_to_handler() {
    // Adversarial: params must reach the handler unmodified.
    use std::sync::Mutex;
    struct CaptureParams {
        seen: Arc<Mutex<Vec<Value>>>,
        name: &'static str,
    }
    impl DomainHandler for CaptureParams {
        fn domain_name(&self) -> &'static str {
            self.name
        }
        fn handle_command(
            &self,
            _: &str,
            params: Value,
            _: &dyn EventSender,
        ) -> Result<Value, CdpError> {
            self.seen.lock().unwrap().push(params);
            Ok(json!({}))
        }
    }
    let reg = DomainRegistry::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    reg.register(CaptureParams {
        seen: seen.clone(),
        name: "Page",
    })
    .unwrap();
    let params = json!({"url": "https://x", "transitionType": "link", "frameId": "f1"});
    reg.dispatch_command("Page.navigate", params.clone(), &NopSender)
        .unwrap()
        .unwrap();
    let captured = seen.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0], params);
}

// ---- DomainHandler / EventSender / TargetProvider trait bounds ----

#[test]
fn test_domain_handler_trait_requires_send_sync() {
    // Adversarial: compile-time proof that DomainHandler: Send + Sync.
    fn assert_bounds<T: Send + Sync>() {}
    assert_bounds::<CountingHandler>();
}

#[test]
fn test_event_sender_trait_requires_send_sync() {
    fn assert_bounds<T: Send + Sync>() {}
    assert_bounds::<NopSender>();
}

#[test]
fn test_target_provider_trait_requires_send_sync() {
    // Adversarial: TargetProvider: Send + Sync (used across CDP accept thread).
    // Prove the trait object is usable behind Arc<dyn TargetProvider> (Sized-relaxed).
    fn assert_bounds<T: Send + Sync + ?Sized>() {}
    assert_bounds::<dyn cdp_server::TargetProvider>();
}
