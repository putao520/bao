// @trace TEST-CDS-011-COMPLIANCE [req:REQ-CDS-001,REQ-CDS-003,REQ-CDS-004] [level:unit]
// JSON-RPC 2.0 protocol compliance via public API: CdpMessage parse, CdpResponse serialize,
// DomainRegistry dispatch roundtrip, TargetInfo, ServerConfig

use cdp_server::{
    CdpMessage, CdpError, CdpResponse, CdpEvent, SessionError,
    DomainRegistry, ServerConfig, TargetInfo,
};
use serde_json::{Value, json};

// ---- CdpMessage deserialization (JSON-RPC 2.0 parsing) ----

#[test]
fn test_parse_valid_minimal_request() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"Page.navigate"}"#).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.navigate");
    assert!(msg.params.is_none());
    assert!(msg.session_id.is_none());
}

#[test]
fn test_parse_request_with_null_params() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":2,"method":"Page.enable","params":null}"#,
    ).unwrap();
    assert_eq!(msg.id, Some(2));
    assert!(msg.params.is_none());
}

#[test]
fn test_parse_request_with_empty_object_params() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":3,"method":"Page.disable","params":{}}"#,
    ).unwrap();
    assert_eq!(msg.params, Some(json!({})));
}

#[test]
fn test_parse_request_with_nested_params() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":4,"method":"Page.navigate","params":{"url":"https://example.com","referrer":"https://google.com"}}"#,
    ).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["url"], "https://example.com");
    assert_eq!(params["referrer"], "https://google.com");
}

#[test]
fn test_parse_request_with_session_id() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":5,"method":"Runtime.evaluate","params":{"expression":"1+1"},"session_id":"sess_abc123"}"#,
    ).unwrap();
    assert_eq!(msg.session_id, Some("sess_abc123".into()));
}

#[test]
fn test_parse_request_negative_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":-999,"method":"Test.ping"}"#).unwrap();
    assert_eq!(msg.id, Some(-999));
}

#[test]
fn test_parse_request_zero_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":0,"method":"Test.ping"}"#).unwrap();
    assert_eq!(msg.id, Some(0));
}

#[test]
fn test_parse_request_max_i64_id() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":9223372036854775807,"method":"Test.ping"}"#,
    ).unwrap();
    assert_eq!(msg.id, Some(i64::MAX));
}

#[test]
fn test_parse_notification_no_id() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"method":"Page.loadEventFired","params":{"timestamp":1234.5}}"#,
    ).unwrap();
    assert!(msg.id.is_none());
    assert_eq!(msg.method, "Page.loadEventFired");
}

#[test]
fn test_parse_empty_string_fails() {
    assert!(serde_json::from_str::<CdpMessage>("").is_err());
}

#[test]
fn test_parse_array_fails() {
    assert!(serde_json::from_str::<CdpMessage>("[]").is_err());
}

#[test]
fn test_parse_null_fails() {
    assert!(serde_json::from_str::<CdpMessage>("null").is_err());
}

#[test]
fn test_parse_number_fails() {
    assert!(serde_json::from_str::<CdpMessage>("42").is_err());
}

#[test]
fn test_parse_string_fails() {
    assert!(serde_json::from_str::<CdpMessage>(r#""hello""#).is_err());
}

#[test]
fn test_parse_missing_method_fails() {
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":1,"params":{}}"#).is_err());
}

#[test]
fn test_parse_array_method_fails() {
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":1,"method":[1,2,3]}"#).is_err());
}

#[test]
fn test_parse_number_method_fails() {
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":1,"method":42}"#).is_err());
}

#[test]
fn test_parse_unicode_method() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"Page.日本語テスト"}"#).unwrap();
    assert_eq!(msg.method, "Page.日本語テスト");
}

#[test]
fn test_parse_emoji_params() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":1,"method":"Page.navigate","params":{"url":"https://example.com/🎉"}}"#,
    ).unwrap();
    assert_eq!(msg.params.unwrap()["url"], "https://example.com/🎉");
}

#[test]
fn test_parse_deeply_nested_params() {
    let raw = r#"{"id":1,"method":"DOM.setAttributeValue","params":{"nodeId":1,"name":"class","value":"a b c"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["nodeId"], 1);
    assert_eq!(params["name"], "class");
    assert_eq!(params["value"], "a b c");
}

#[test]
fn test_parse_large_params_array() {
    let items: Vec<Value> = (0..5000).map(|i| json!({"idx": i})).collect();
    let raw = json!({"id": 1, "method": "Test.bulk", "params": {"items": items}}).to_string();
    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    assert_eq!(msg.params.unwrap()["items"].as_array().unwrap().len(), 5000);
}

// ---- CdpResponse serialization ----

#[test]
fn test_serialize_ok_response() {
    let resp = CdpResponse {
        id: Some(42),
        result: Some(json!({"value": true})),
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 42);
    assert_eq!(parsed["result"]["value"], true);
    assert!(parsed.get("error").is_none());
}

#[test]
fn test_serialize_error_response() {
    let resp = CdpResponse {
        id: Some(10),
        result: None,
        error: Some(CdpError { code: -32601, message: "Method not found".into() }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 10);
    assert!(parsed.get("result").is_none());
    assert_eq!(parsed["error"]["code"], -32601);
    assert_eq!(parsed["error"]["message"], "Method not found");
}

#[test]
fn test_serialize_response_null_id() {
    let resp = CdpResponse {
        id: None,
        result: None,
        error: Some(CdpError { code: -32700, message: "Parse error".into() }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["id"].is_null());
    assert_eq!(parsed["error"]["code"], -32700);
}

#[test]
fn test_serialize_empty_result() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({})),
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"], json!({}));
}

// ---- CdpEvent serialization ----

#[test]
fn test_serialize_event_with_params() {
    let ev = CdpEvent {
        method: "Page.frameNavigated".into(),
        params: Some(json!({"frameId": "main"})),
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["method"], "Page.frameNavigated");
    assert_eq!(parsed["params"]["frameId"], "main");
    assert!(parsed.get("id").is_none());
}

#[test]
fn test_serialize_event_without_params() {
    let ev = CdpEvent {
        method: "Page.domContentEventFired".into(),
        params: None,
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["method"], "Page.domContentEventFired");
    assert!(parsed.get("params").is_none());
}

// ---- SessionError variants ----

#[test]
fn test_session_error_debug_variants() {
    assert!(format!("{:?}", SessionError::Closed).contains("Closed"));
    assert!(format!("{:?}", SessionError::Io).contains("Io"));
}

// ---- CdpError serialization edge cases ----

#[test]
fn test_cdp_error_serialization() {
    let err = CdpError { code: -32600, message: "Invalid Request".into() };
    let serialized = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["code"], -32600);
    assert_eq!(parsed["message"], "Invalid Request");
}

#[test]
fn test_cdp_error_empty_message() {
    let err = CdpError { code: -1, message: String::new() };
    let serialized = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["message"], "");
}

#[test]
fn test_cdp_error_unicode_message() {
    let err = CdpError { code: -32000, message: "错误：无效的参数 🚫".into() };
    let serialized = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["message"], "错误：无效的参数 🚫");
}

// ---- TargetInfo roundtrip ----

#[test]
fn test_target_info_roundtrip() {
    let info = TargetInfo {
        id: "target-123".into(),
        target_type: "page".into(),
        title: "Test Page".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/target-123".into(),
    };
    let serialized = serde_json::to_string(&info).unwrap();
    let parsed: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["id"], "target-123");
    assert_eq!(parsed["type"], "page");
    assert_eq!(parsed["title"], "Test Page");
    assert_eq!(parsed["url"], "https://example.com");
    assert_eq!(parsed["web_socket_debugger_url"], "ws://127.0.0.1:9222/devtools/page/target-123");
}

#[test]
fn test_target_info_deserialize() {
    let raw = r#"{"id":"abc","type":"iframe","title":"Inner","url":"about:blank","web_socket_debugger_url":"ws://localhost:9222/devtools/page/abc"}"#;
    let info: TargetInfo = serde_json::from_str(raw).unwrap();
    assert_eq!(info.id, "abc");
    assert_eq!(info.target_type, "iframe");
    assert_eq!(info.title, "Inner");
    assert_eq!(info.url, "about:blank");
}

// ---- DomainRegistry full dispatch roundtrip ----

struct EchoDomain;
impl cdp_server::DomainHandler for EchoDomain {
    fn domain_name(&self) -> &'static str { "Echo" }
    fn handle_command(&self, cmd: &str, params: Value, _: &dyn cdp_server::EventSender) -> Result<Value, CdpError> {
        match cmd {
            "Echo.ping" => Ok(json!({"pong": true})),
            "Echo.echo" => Ok(params),
            "Echo.fail" => Err(CdpError { code: -32000, message: "deliberate failure".into() }),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", cmd) }),
        }
    }
}

struct NopSender;
impl cdp_server::EventSender for NopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

#[test]
fn test_full_roundtrip_success() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoDomain)).unwrap();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(r#"{"id":100,"method":"Echo.ping","params":{}}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    assert!(result.is_some());
    let val = result.unwrap().unwrap();
    assert_eq!(val["pong"], true);
}

#[test]
fn test_full_roundtrip_echo() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoDomain)).unwrap();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":101,"method":"Echo.echo","params":{"hello":"world","n":42}}"#,
    ).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    let val = result.unwrap().unwrap();
    assert_eq!(val["hello"], "world");
    assert_eq!(val["n"], 42);
}

#[test]
fn test_full_roundtrip_handler_error() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoDomain)).unwrap();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(r#"{"id":200,"method":"Echo.fail","params":{}}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32000);
}

#[test]
fn test_full_roundtrip_unknown_method_in_domain() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoDomain)).unwrap();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(r#"{"id":201,"method":"Echo.nonexistent","params":{}}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_full_roundtrip_unknown_domain() {
    let reg = DomainRegistry::new();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(r#"{"id":300,"method":"Foo.bar","params":{}}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    assert!(result.is_none());
}

#[test]
fn test_full_roundtrip_notification() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoDomain)).unwrap();
    let sender = NopSender;

    // Notification (no id) — dispatch still works
    let msg: CdpMessage = serde_json::from_str(r#"{"method":"Echo.ping"}"#).unwrap();
    assert!(msg.id.is_none());
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    assert!(result.unwrap().is_ok());
}

// ---- ServerConfig ----

#[test]
fn test_server_config_default_values() {
    let config = ServerConfig::default();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 9222);
    assert_eq!(config.http_timeout_seconds, 30);
    assert_eq!(config.max_sessions, 100);
    assert_eq!(config.browser_name, "Bao/0.1.0");
    assert_eq!(config.protocol_version, "1.3");
    assert!(config.user_agent.is_none());
    assert!(config.v8_version.is_none());
    assert!(config.webkit_version.is_none());
}

#[test]
fn test_server_config_builder_full() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(9333)
        .http_timeout_seconds(60)
        .max_sessions(50)
        .browser_name("Chrome/120")
        .user_agent("Mozilla/5.0")
        .v8_version("12.0")
        .webkit_version("537.36")
        .build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9333);
    assert_eq!(config.http_timeout_seconds, 60);
    assert_eq!(config.max_sessions, 50);
    assert_eq!(config.browser_name, "Chrome/120");
    assert_eq!(config.user_agent, Some("Mozilla/5.0".into()));
    assert_eq!(config.v8_version, Some("12.0".into()));
    assert_eq!(config.webkit_version, Some("537.36".into()));
}

#[test]
fn test_server_config_builder_partial() {
    let config = ServerConfig::builder().port(8080).build();
    assert_eq!(config.port, 8080);
    assert_eq!(config.host, "127.0.0.1");
}

// ---- Registry protocol-level edge cases ----

#[test]
fn test_dispatch_with_missing_params_uses_default() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoDomain)).unwrap();
    let sender = NopSender;

    // CdpMessage parsed from JSON without params → params is None → unwrap_or_default() gives Null
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"Echo.ping"}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    assert!(result.unwrap().is_ok());
}

#[test]
fn test_dispatch_after_multiple_errors_recovers() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoDomain)).unwrap();
    let sender = NopSender;

    // Multiple errors
    for _ in 0..5 {
        let result = reg.dispatch_command("Echo.fail", json!({}), &sender);
        assert!(result.unwrap().is_err());
    }
    // Recovery
    let result = reg.dispatch_command("Echo.ping", json!({}), &sender);
    assert!(result.unwrap().is_ok());
}
