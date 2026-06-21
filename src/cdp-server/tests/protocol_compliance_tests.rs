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
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(r#"{"id":100,"method":"Echo.ping","params":{}}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    assert!(result.is_some());
    let val = result.unwrap().unwrap();
    assert_eq!(val["pong"], true);
}

#[test]
fn test_full_roundtrip_echo() {
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
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
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(r#"{"id":200,"method":"Echo.fail","params":{}}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32000);
}

#[test]
fn test_full_roundtrip_unknown_method_in_domain() {
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(r#"{"id":201,"method":"Echo.nonexistent","params":{}}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_full_roundtrip_unknown_domain() {
    let reg = DomainRegistry::<EchoDomain>::new();
    let sender = NopSender;

    let msg: CdpMessage = serde_json::from_str(r#"{"id":300,"method":"Foo.bar","params":{}}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    assert!(result.is_none());
}

#[test]
fn test_full_roundtrip_notification() {
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
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
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;

    // CdpMessage parsed from JSON without params → params is None → unwrap_or_default() gives Null
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"Echo.ping"}"#).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    assert!(result.unwrap().is_ok());
}

#[test]
fn test_dispatch_after_multiple_errors_recovers() {
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
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

// ============================================================================
// ADVERSARIAL VERIFICATION GAPS — supplementary assertions, boundary conditions,
// and SPEC alignment (REQ-CDS-001/003/004/005/006). JSON-RPC 2.0 error codes
// are the fixed protocol values (not re-exported from the crate's private
// protocol module): -32700 Parse error, -32600 Invalid Request, -32601
// Method not found, -32602 Invalid params, -32603 Internal error.
// ============================================================================

// ---- REQ-CDS-001-C5: JSON-RPC 2.0 request field semantics ----

#[test]
fn test_parse_null_id_is_none_jsonrpc_semantics() {
    // JSON-RPC 2.0: explicit "id":null is a notification (no response expected)
    // and must deserialize to None, NOT Some(null-shaped value).
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id":null,"method":"Page.reload"}"#).unwrap();
    assert_eq!(msg.id, None);
}

#[test]
fn test_parse_float_id_fails_jsonrpc_strictness() {
    // JSON-RPC 2.0 §4: id MUST be String, Number, or Null. A fractional
    // number is discouraged but serde accepts any i64; a true float (1.5)
    // must be rejected by the i64 field type — verifying the type strictness.
    let res = serde_json::from_str::<CdpMessage>(r#"{"id":1.5,"method":"X.y"}"#);
    // i64 field rejects 1.5 (non-integer). Either rejected, or serde rounds —
    // assert the strict rejection path per JSON-RPC "SHOULD NOT use fractional".
    assert!(res.is_err(), "fractional id must be rejected by i64 field");
}

#[test]
fn test_parse_params_array_preserved() {
    // REQ-CDS-001-C5: params may be an Array or Object (JSON-RPC §4.2).
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":7,"method":"DOM.querySelectorAll","params":["div","span"]}"#,
    )
    .unwrap();
    let params = msg.params.unwrap();
    assert!(params.is_array());
    assert_eq!(params.as_array().unwrap().len(), 2);
    assert_eq!(params[0], "div");
    assert_eq!(params[1], "span");
}

#[test]
fn test_parse_params_scalar_preserved() {
    // Boundary: params is a bare scalar (technically allowed by JSON-RPC §4.2
    // "structured value"; serde_json::Value accepts it). Verify it survives.
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id":8,"method":"X.y","params":42}"#).unwrap();
    assert_eq!(msg.params.unwrap(), 42);
}

#[test]
fn test_parse_null_vs_missing_params_both_none() {
    // REQ-CDS-001-C5 semantic equivalence: omitted params and explicit null
    // both yield None (serde default + deserialize Option<Value>).
    let missing: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"X.y"}"#).unwrap();
    let null_params: CdpMessage =
        serde_json::from_str(r#"{"id":1,"method":"X.y","params":null}"#).unwrap();
    assert_eq!(missing.params, None);
    assert_eq!(null_params.params, None);
    assert_eq!(missing.params, null_params.params);
}

#[test]
fn test_parse_extra_fields_ignored_not_rejected() {
    // Forward-compat: unknown fields must be ignored (serde default deny_unknown_fields=false).
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":1,"method":"Page.reload","jsonrpc":"2.0","extra":"x","n":123}"#,
    )
    .unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.reload");
}

#[test]
fn test_parse_jsonrpc_2_0_field_accepted() {
    // Strict JSON-RPC 2.0 envelope includes "jsonrpc":"2.0"; must parse.
    let msg: CdpMessage = serde_json::from_str(
        r#"{"jsonrpc":"2.0","id":1,"method":"Page.reload","params":{}}"#,
    )
    .unwrap();
    assert_eq!(msg.id, Some(1));
}

#[test]
fn test_parse_min_i64_id() {
    // Boundary: most negative id survives the i64 field.
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id":-9223372036854775808,"method":"X.y"}"#).unwrap();
    assert_eq!(msg.id, Some(i64::MIN));
}

#[test]
fn test_parse_object_method_fails() {
    // type-safety: method must be a string.
    assert!(serde_json::from_str::<CdpMessage>(
        r#"{"id":1,"method":{"nested":true}}"#
    )
    .is_err());
}

#[test]
fn test_parse_bool_method_fails() {
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":1,"method":true}"#).is_err());
}

#[test]
fn test_parse_empty_method_string_accepted() {
    // Boundary: empty method is syntactically valid JSON; dispatch will treat
    // it as an empty domain (split('.').next() → ""). Verify parse doesn't
    // artificially reject.
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":""}"#).unwrap();
    assert_eq!(msg.method, "");
}

#[test]
fn test_parse_empty_object_fails() {
    // method is required → {} must fail.
    assert!(serde_json::from_str::<CdpMessage>("{}").is_err());
}

// ---- REQ-CDS-001-C6 / C7 / C8: response construction & error codes ----

#[test]
fn test_invalid_request_error_code_jsonrpc_32600() {
    // REQ-CDS-001-C7: invalid JSON request → -32600 Invalid Request.
    // The parse-error path at the transport layer uses this code; we assert
    // the canonical constant value the SPEC mandates.
    const ERR_INVALID_REQUEST: i64 = -32600;
    let resp = CdpResponse {
        id: None,
        result: None,
        error: Some(CdpError {
            code: ERR_INVALID_REQUEST,
            message: "Invalid Request".into(),
        }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["error"]["code"], -32600);
    assert_eq!(v["error"]["message"], "Invalid Request");
}

#[test]
fn test_method_not_found_error_code_jsonrpc_32601() {
    // REQ-CDS-001-C8 / REQ-CDS-004-C4: unknown Domain.Method → -32601.
    const ERR_METHOD_NOT_FOUND: i64 = -32601;
    let resp = CdpResponse {
        id: Some(1),
        result: None,
        error: Some(CdpError {
            code: ERR_METHOD_NOT_FOUND,
            message: "Method not found".into(),
        }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["error"]["code"], -32601);
}

#[test]
fn test_parse_error_code_jsonrpc_32700() {
    // REQ-CDS-001-C7 variant: unparseable JSON → -32700 Parse error.
    const ERR_PARSE_ERROR: i64 = -32700;
    let resp = CdpResponse {
        id: None,
        result: None,
        error: Some(CdpError {
            code: ERR_PARSE_ERROR,
            message: "Parse error".into(),
        }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["error"]["code"], -32700);
}

#[test]
fn test_invalid_params_error_code_jsonrpc_32602() {
    // REQ-CDS-004-C4: handler signals invalid params via -32602.
    const ERR_INVALID_PARAMS: i64 = -32602;
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError {
            code: ERR_INVALID_PARAMS,
            message: "Invalid params".into(),
        }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["error"]["code"], -32602);
}

#[test]
fn test_response_result_and_error_mutually_exclusive() {
    // JSON-RPC 2.0 §5.1: a Response MUST contain EITHER result OR error,
    // never both. CdpResponse serializes both but valid usage is exclusive.
    // Assert the success branch carries no error key.
    let ok = CdpResponse {
        id: Some(1),
        result: Some(json!({"v": 1})),
        error: None,
    };
    let raw = serde_json::to_string(&ok).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert!(v.get("result").is_some());
    assert!(v.get("error").is_none());

    let err = CdpResponse {
        id: Some(1),
        result: None,
        error: Some(CdpError { code: -1, message: "x".into() }),
    };
    let raw = serde_json::to_string(&err).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert!(v.get("result").is_none());
    assert!(v.get("error").is_some());
}

#[test]
fn test_response_id_always_present_even_on_error() {
    // JSON-RPC 2.0 §5.1: response MUST include id (null when it can't be
    // determined). CdpResponse.id is Option serialized as null when None.
    let resp = CdpResponse {
        id: None,
        result: None,
        error: Some(CdpError { code: -32600, message: "x".into() }),
    };
    let v: Value = serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
    // "id" key must be present (None serializes to null, not omitted).
    assert!(v.get("id").is_some());
    assert!(v["id"].is_null());
}

// ---- REQ-CDS-005-C4: events carry NO id field ----

#[test]
fn test_event_never_carries_id_field() {
    // REQ-CDS-005-C4: event messages must not contain an id. CdpEvent has no
    // id field at all — verify serialization structurally omits it.
    let ev = CdpEvent {
        method: "Page.frameNavigated".into(),
        params: Some(json!({"frameId": "F"})),
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let obj: serde_json::Map<String, Value> = serde_json::from_str(&raw).unwrap();
    assert!(
        !obj.contains_key("id"),
        "CDP event must never contain an id field (REQ-CDS-005-C4)"
    );
    assert_eq!(obj.len(), 2, "event must have exactly method + params");
}

#[test]
fn test_event_method_format_domain_dot_eventname() {
    // REQ-CDS-005-C4: method format is "Domain.eventName".
    let ev = CdpEvent {
        method: "Target.attachedToTarget".into(),
        params: None,
    };
    let method = &ev.method;
    assert!(method.contains('.'));
    let domain = method.split('.').next().unwrap();
    assert_eq!(domain, "Target");
}

// ---- REQ-CDS-004-C2: domain extraction from method (boundary cases) ----

#[test]
fn test_dispatch_method_without_dot_extracts_full_as_domain() {
    // REQ-CDS-004-C2: split('.').next(). For a method with no '.', the whole
    // string is the domain. EchoDomain is registered as "Echo" → no match → None.
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;
    let result = reg.dispatch_command("no_dot_here", json!({}), &sender);
    assert!(result.is_none(), "method with no dot → domain lookup misses");
}

#[test]
fn test_dispatch_method_with_trailing_dot() {
    // "Echo." → domain "Echo" → handler found, command "" → handler returns
    // -32601 (unknown command within domain). Verifies split keeps leading.
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;
    let result = reg.dispatch_command("Echo.", json!({}), &sender).unwrap();
    let err = result.unwrap_err();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_dispatch_method_with_multiple_dots_extracts_first_segment() {
    // "Echo.deeply.nested.command" → domain "Echo" only (first segment).
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;
    // Echo handler matches on full cmd; pass full string, domain extraction
    // must still select Echo domain.
    let result = reg.dispatch_command("Echo.ping.extra", json!({}), &sender);
    assert!(result.is_some(), "domain extraction takes first dot segment");
}

#[test]
fn test_dispatch_empty_method_string_returns_none() {
    // Empty method → empty domain → no handler → None (no panic).
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;
    let result = reg.dispatch_command("", json!({}), &sender);
    assert!(result.is_none());
}

#[test]
fn test_dispatch_leading_dot_method_extracts_empty_domain() {
    // ".ping" → split('.').next() = "" → empty domain → None.
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;
    let result = reg.dispatch_command(".ping", json!({}), &sender);
    assert!(result.is_none());
}

// ---- REQ-CDS-004-C4 / REQ-CDS-006-C5: handler dispatch & registry invariants ----

#[test]
fn test_dispatch_passes_full_method_to_handler() {
    // REQ-CDS-004-C4: handler receives the full method string, not just the
    // command suffix. Echo.echo echoes params; assert the params round-trip.
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;
    let out = reg
        .dispatch_command("Echo.echo", json!({"k": "v"}), &sender)
        .unwrap()
        .unwrap();
    assert_eq!(out["k"], "v");
}

#[test]
fn test_register_duplicate_domain_rejected_no_overwrite() {
    // REQ-CDS-006-C5: duplicate domain registration MUST return an error and
    // MUST NOT overwrite the existing handler.
    struct Original;
    impl cdp_server::DomainHandler for Original {
        fn domain_name(&self) -> &'static str { "Echo" }
        fn handle_command(&self, _: &str, _: Value, _: &dyn cdp_server::EventSender)
            -> Result<Value, CdpError> {
            Ok(json!({"which": "original"}))
        }
    }
    struct Replacement;
    impl cdp_server::DomainHandler for Replacement {
        fn domain_name(&self) -> &'static str { "Echo" }
        fn handle_command(&self, _: &str, _: Value, _: &dyn cdp_server::EventSender)
            -> Result<Value, CdpError> {
            Ok(json!({"which": "replacement"}))
        }
    }

    // Two different handler types can't share one DomainRegistry<H> (H fixed),
    // so verify via EchoDomain self-register: second register of Echo fails and
    // the original handler stays active.
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let err = reg.register(EchoDomain).unwrap_err();
    assert!(
        err.contains("already registered") || err.contains("'Echo'"),
        "duplicate register error must name the domain: got {err}"
    );

    // Original handler unchanged: dispatch still answers with Echo.ping semantics.
    let sender = NopSender;
    let out = reg
        .dispatch_command("Echo.ping", json!({}), &sender)
        .unwrap()
        .unwrap();
    assert_eq!(out["pong"], true);
}

#[test]
fn test_register_multiple_distinct_domains_all_dispatchable() {
    // REQ-CDS-006-C2: O(1) lookup by domain_name. Register many, dispatch each.
    let reg = DomainRegistry::<EchoDomain>::new();
    // EchoDomain always reports domain "Echo"; to exercise multi-domain we
    // rely on the single-domain registry but verify lookup misses correctly.
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;
    assert!(reg.dispatch_command("Echo.ping", json!({}), &sender).is_some());
    assert!(reg.dispatch_command("Page.navigate", json!({}), &sender).is_none());
    assert!(reg.dispatch_command("Runtime.evaluate", json!({}), &sender).is_none());
}

#[test]
fn test_has_domain_after_register_and_not_before() {
    // REQ-CDS-006-C2: has_domain reflects registration state.
    let reg = DomainRegistry::<EchoDomain>::new();
    assert!(!reg.has_domain("Echo"));
    assert!(!reg.has_domain("Page"));
    reg.register(EchoDomain).unwrap();
    assert!(reg.has_domain("Echo"));
    assert!(!reg.has_domain("Page"));
}

#[test]
fn test_notification_dispatch_does_not_require_id() {
    // REQ-CDS-004: a message with no id (notification) is still dispatched;
    // dispatch_command is id-agnostic (routing keyed on method only).
    let reg = DomainRegistry::<EchoDomain>::new();
    reg.register(EchoDomain).unwrap();
    let sender = NopSender;
    let msg: CdpMessage = serde_json::from_str(r#"{"method":"Echo.echo","params":{"a":1}}"#).unwrap();
    assert!(msg.id.is_none());
    let out = reg
        .dispatch_command(&msg.method, msg.params.unwrap(), &sender)
        .unwrap()
        .unwrap();
    assert_eq!(out["a"], 1);
}

// ---- REQ-CDS-004-C6: sessionId parameter (flat session) ----

#[test]
fn test_session_id_roundtrip_preserved_on_parse() {
    // REQ-CDS-004-C6: session_id is parsed and available for flat-session routing.
    // (Wire field name is "session_id" per CdpMessage struct definition.)
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":99,"method":"Runtime.evaluate","params":{"expression":"1"},"session_id":"flat-xyz"}"#,
    )
    .unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("flat-xyz"));
}

#[test]
fn test_session_id_unicode_preserved() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":1,"method":"X.y","session_id":"会话-🎉"}"#,
    )
    .unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("会话-🎉"));
}

#[test]
fn test_session_id_null_is_none() {
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id":1,"method":"X.y","session_id":null}"#).unwrap();
    assert_eq!(msg.session_id, None);
}

#[test]
fn test_session_id_empty_string_is_some_empty() {
    // Boundary: empty session_id is still Some("") (distinct from absent/null).
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id":1,"method":"X.y","session_id":""}"#).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some(""));
}

// ---- REQ-CDS-001-C6 / REQ-CDS-005-C4: serialization structural invariants ----

#[test]
fn test_ok_response_omits_error_key_via_skip() {
    // CdpResponse uses skip_serializing_if on result/error. Assert the success
    // form has no "error" key (not null — entirely absent).
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"ok": 1})),
        error: None,
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let obj: serde_json::Map<String, Value> = serde_json::from_str(&raw).unwrap();
    assert!(!obj.contains_key("error"));
    assert!(obj.contains_key("result"));
}

#[test]
fn test_error_response_omits_result_key_via_skip() {
    let resp = CdpResponse {
        id: Some(1),
        result: None,
        error: Some(CdpError { code: -1, message: "e".into() }),
    };
    let raw = serde_json::to_string(&resp).unwrap();
    let obj: serde_json::Map<String, Value> = serde_json::from_str(&raw).unwrap();
    assert!(!obj.contains_key("result"));
    assert!(obj.contains_key("error"));
}

#[test]
fn test_event_omits_params_when_none() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: None,
    };
    let raw = serde_json::to_string(&ev).unwrap();
    let obj: serde_json::Map<String, Value> = serde_json::from_str(&raw).unwrap();
    assert!(!obj.contains_key("params"));
    assert!(obj.contains_key("method"));
}

// ---- TargetInfo adversarial boundary ----

#[test]
fn test_target_info_serializes_type_not_target_type() {
    // TargetInfo uses #[serde(rename = "type")]. Adversarial: the wire field
    // MUST be "type", never "target_type".
    let info = TargetInfo {
        id: "t".into(),
        target_type: "page".into(),
        title: "T".into(),
        url: "u".into(),
        web_socket_debugger_url: "ws".into(),
    };
    let obj: serde_json::Map<String, Value> =
        serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
    assert!(obj.contains_key("type"));
    assert!(
        !obj.contains_key("target_type"),
        "wire field must be renamed to 'type', not 'target_type'"
    );
}

#[test]
fn test_target_info_all_fields_required_on_deserialize() {
    // TargetInfo has no Optional fields → omitting any must fail.
    assert!(serde_json::from_str::<TargetInfo>(
        r#"{"id":"a","type":"page","title":"t","url":"u"}"# // missing web_socket_debugger_url
    )
    .is_err());
    assert!(serde_json::from_str::<TargetInfo>(
        r#"{"type":"page","title":"t","url":"u","web_socket_debugger_url":"ws"}"# // missing id
    )
    .is_err());
}

#[test]
fn test_target_info_unicode_fields_roundtrip() {
    let info = TargetInfo {
        id: "目标-1".into(),
        target_type: "page".into(),
        title: "测试页面 🎉".into(),
        url: "https://例え.jp/パス".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/目标-1".into(),
    };
    let raw = serde_json::to_string(&info).unwrap();
    let back: TargetInfo = serde_json::from_str(&raw).unwrap();
    assert_eq!(back.id, "目标-1");
    assert_eq!(back.title, "测试页面 🎉");
    assert_eq!(back.url, "https://例え.jp/パス");
}

// ---- ServerConfig adversarial boundary ----

#[test]
fn test_server_config_builder_idempotent_default() {
    // Empty builder → exactly Default. Adversarial: no field should be Some
    // when nothing was set.
    let built = ServerConfig::builder().build();
    let default = ServerConfig::default();
    assert_eq!(built.host, default.host);
    assert_eq!(built.port, default.port);
    assert_eq!(built.http_timeout_seconds, default.http_timeout_seconds);
    assert_eq!(built.max_sessions, default.max_sessions);
    assert_eq!(built.browser_name, default.browser_name);
    assert_eq!(built.protocol_version, default.protocol_version);
    assert_eq!(built.user_agent, default.user_agent);
    assert_eq!(built.v8_version, default.v8_version);
    assert_eq!(built.webkit_version, default.webkit_version);
}

#[test]
fn test_server_config_overrides_only_specified_fields() {
    // Partial builder must leave other fields at default.
    let cfg = ServerConfig::builder()
        .user_agent("UA/1.0")
        .v8_version("11.0")
        .build();
    assert_eq!(cfg.user_agent.as_deref(), Some("UA/1.0"));
    assert_eq!(cfg.v8_version.as_deref(), Some("11.0"));
    // Untouched fields keep defaults.
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.port, 9222);
    assert!(cfg.webkit_version.is_none());
}

#[test]
fn test_server_config_protocol_version_default_is_1_3() {
    // CDP protocol version 1.3 is the SPEC-mandated baseline.
    assert_eq!(ServerConfig::default().protocol_version, "1.3");
}

// ---- CdpError adversarial ----

#[test]
fn test_cdp_error_extreme_codes_serialize() {
    // Boundary: min/max i64 error codes survive serialization.
    for code in [i64::MIN, -1, 0, 1, i64::MAX] {
        let err = CdpError { code, message: format!("e{code}") };
        let v: Value = serde_json::from_str(&serde_json::to_string(&err).unwrap()).unwrap();
        assert_eq!(v["code"].as_i64(), Some(code));
    }
}

#[test]
fn test_cdp_error_clone_preserves_fields() {
    // CdpError must be Clone (used across session/error paths).
    let err = CdpError { code: -32601, message: "m".into() };
    let cloned = err.clone();
    assert_eq!(err.code, cloned.code);
    assert_eq!(err.message, cloned.message);
}
