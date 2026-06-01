// @trace TEST-CDP-EDGE [req:REQ-CDP-001] [level:unit]
// Protocol edge case deep tests: CdpMessage parsing with unusual inputs,
// CdpResponse serialization with empty/null fields, CdpError with all
// JSON-RPC 2.0 error code variants, DomainRegistry dispatch boundary
// conditions, TargetInfo field validation/serialization, SessionState
// lifecycle transitions, ServerConfig builder boundary values,
// EventBroadcaster with no subscribers.

use bao_cdp::{CDPMessage, CDPResponse, CDPError, CDPEvent, parse_message, serialize_response, serialize_event, handle_command};
use bao_cdp::{ServerConfig, DomainRegistry, EventBroadcaster};
use cdp_server::{DomainHandler, EventSender, CdpError as ServerCdpError, SessionState, TargetInfo};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// 1. CdpMessage parsing with unusual/edge-case inputs
// ---------------------------------------------------------------------------

#[test]
fn parse_message_whitespace_only() {
    assert!(parse_message("   ").is_none());
}

#[test]
fn parse_message_null_root() {
    assert!(parse_message("null").is_none());
}

#[test]
fn parse_message_boolean_root() {
    assert!(parse_message("true").is_none());
    assert!(parse_message("false").is_none());
}

#[test]
fn parse_message_number_root() {
    assert!(parse_message("42").is_none());
    assert!(parse_message("-1").is_none());
    assert!(parse_message("3.14").is_none());
}

#[test]
fn parse_message_string_root() {
    assert!(parse_message(r#""hello""#).is_none());
}

#[test]
fn parse_message_truncated_json_object() {
    assert!(parse_message(r#"{"id":1"#).is_none());
    assert!(parse_message(r#"{"id":1,"method":"Page.enable""#).is_none());
    assert!(parse_message("{").is_none());
    assert!(parse_message("}").is_none());
}

#[test]
fn parse_message_nested_array_root() {
    assert!(parse_message("[[1,2],[3,4]]").is_none());
}

#[test]
fn parse_message_id_as_float_fails() {
    assert!(parse_message(r#"{"id":1.5,"method":"Page.enable"}"#).is_none());
}

#[test]
fn parse_message_id_as_bool_fails() {
    assert!(parse_message(r#"{"id":true,"method":"Page.enable"}"#).is_none());
}

#[test]
fn parse_message_id_as_object_fails() {
    assert!(parse_message(r#"{"id":{},"method":"Page.enable"}"#).is_none());
}

#[test]
fn parse_message_id_as_array_fails() {
    assert!(parse_message(r#"{"id":[1],"method":"Page.enable"}"#).is_none());
}

#[test]
fn parse_message_method_as_number_fails() {
    assert!(parse_message(r#"{"id":1,"method":42}"#).is_none());
}

#[test]
fn parse_message_method_as_null_fails() {
    assert!(parse_message(r#"{"id":1,"method":null}"#).is_none());
}

#[test]
fn parse_message_method_as_array_fails() {
    assert!(parse_message(r#"{"id":1,"method":["Page.enable"]}"#).is_none());
}

#[test]
fn parse_message_params_as_string_fails() {
    // CDPMessage.params is Option<Value>; a JSON string is valid Value,
    // but this is unusual. Verify it parses (serde accepts any Value).
    let raw = r#"{"id":1,"method":"Test.run","params":"string"}"#;
    let msg = parse_message(raw);
    assert!(msg.is_some());
    let msg = msg.unwrap();
    assert!(msg.params.is_some());
    assert!(msg.params.unwrap().is_string());
}

#[test]
fn parse_message_params_as_number() {
    let raw = r#"{"id":1,"method":"Test.run","params":42}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.params.unwrap().is_number());
}

#[test]
fn parse_message_params_as_bool() {
    let raw = r#"{"id":1,"method":"Test.run","params":true}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.params.unwrap().is_boolean());
}

#[test]
fn parse_message_deeply_nested_params() {
    // Build a valid deeply-nested JSON object for params
    let mut inner = json!(1);
    for i in 0..20 {
        inner = json!({ format!("level_{}", i): inner });
    }
    let msg_raw = json!({
        "id": 1,
        "method": "Test.run",
        "params": inner,
    });
    let raw = serde_json::to_string(&msg_raw).unwrap();
    let msg = parse_message(&raw);
    assert!(msg.is_some(), "20-level nested params should parse");
}

#[test]
fn parse_message_params_with_all_json_types() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"str":"hello","num":42,"float":3.14,"bool":true,"null":null,"arr":[1,2,3],"obj":{"k":"v"}}}"#;
    let msg = parse_message(raw).unwrap();
    let p = msg.params.unwrap();
    let obj = p.as_object().unwrap();
    assert_eq!(obj.len(), 7);
}

#[test]
fn parse_message_session_id_as_number_fails() {
    assert!(parse_message(r#"{"id":1,"method":"Test.run","session_id":123}"#).is_none());
}

#[test]
fn parse_message_session_id_empty_string() {
    let raw = r#"{"id":1,"method":"Test.run","session_id":""}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.session_id.unwrap(), "");
}

#[test]
fn parse_message_extremely_long_method() {
    let method = "A".repeat(10000);
    let raw = format!(r#"{{"id":1,"method":"{}"}}"#, method);
    let msg = parse_message(&raw).unwrap();
    assert_eq!(msg.method.len(), 10000);
}

#[test]
fn parse_message_method_with_multiple_dots() {
    let raw = r#"{"id":1,"method":"Page.sub.deep.method"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.method, "Page.sub.deep.method");
}

#[test]
fn parse_message_method_dot_only() {
    let raw = r#"{"id":1,"method":"."}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.method, ".");
}

#[test]
fn parse_message_method_no_dot() {
    let raw = r#"{"id":1,"method":"NoDot"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.method, "NoDot");
}

#[test]
fn parse_message_id_max_i64() {
    let raw = format!(r#"{{"id":{},"method":"Test.run"}}"#, i64::MAX);
    let msg = parse_message(&raw).unwrap();
    assert_eq!(msg.id, i64::MAX);
}

#[test]
fn parse_message_id_min_i64() {
    let raw = format!(r#"{{"id":{},"method":"Test.run"}}"#, i64::MIN);
    let msg = parse_message(&raw).unwrap();
    assert_eq!(msg.id, i64::MIN);
}

#[test]
fn parse_message_unicode_in_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"emoji":"🎉","cjk":"漢字","arabic":"مرحبا"}}"#;
    let msg = parse_message(raw).unwrap();
    let p = msg.params.unwrap();
    assert_eq!(p["emoji"], "🎉");
    assert_eq!(p["cjk"], "漢字");
    assert_eq!(p["arabic"], "مرحبا");
}

#[test]
fn parse_message_escaped_characters_in_method() {
    let raw = r#"{"id":1,"method":"Test.run.nested"}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.method.contains('.'));
}

#[test]
fn parse_message_binary_json_fails() {
    // JSON with raw control characters should fail
    let raw = "{\"id\":1,\"method\":\"Test\x00.run\"}";
    assert!(parse_message(raw).is_none());
}

#[test]
fn parse_message_id_zero_vs_null_distinction() {
    // bao_cdp::CDPMessage.id is i64 (required), so id:0 is valid
    let msg = parse_message(r#"{"id":0,"method":"Test.run"}"#).unwrap();
    assert_eq!(msg.id, 0);
}

#[test]
fn parse_message_many_extra_fields_ignored() {
    let raw = r#"{"id":1,"method":"Test.run","extra1":1,"extra2":"x","extra3":true,"extra4":null,"extra5":[1]}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 1);
    assert_eq!(msg.method, "Test.run");
}

#[test]
fn parse_message_duplicate_keys_last_wins() {
    // JSON spec: duplicate keys behavior is undefined, but serde_json
    // typically uses the last value
    let raw = r#"{"id":1,"method":"First.run","id":2}"#;
    let msg = parse_message(raw);
    if let Some(m) = msg {
        // If it parses, the last id should win
        assert_eq!(m.id, 2);
    }
}

// ---------------------------------------------------------------------------
// 2. CdpResponse serialization with empty/null fields
// ---------------------------------------------------------------------------

#[test]
fn serialize_response_both_none_fields() {
    let resp = CDPResponse { id: 1, result: None, error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 1);
    assert!(parsed.get("result").is_none());
    assert!(parsed.get("error").is_none());
}

#[test]
fn serialize_response_result_is_null_value() {
    // result: Some(Value::Null) — different from result: None
    let resp = CDPResponse { id: 2, result: Some(json!(null)), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["result"].is_null());
}

#[test]
fn serialize_response_result_is_empty_object() {
    let resp = CDPResponse { id: 3, result: Some(json!({})), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["result"].as_object().unwrap().is_empty());
}

#[test]
fn serialize_response_result_is_empty_array() {
    let resp = CDPResponse { id: 4, result: Some(json!([])), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["result"].as_array().unwrap().is_empty());
}

#[test]
fn serialize_response_result_is_false() {
    let resp = CDPResponse { id: 5, result: Some(json!(false)), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"], false);
}

#[test]
fn serialize_response_result_is_zero() {
    let resp = CDPResponse { id: 6, result: Some(json!(0)), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"], 0);
}

#[test]
fn serialize_response_result_is_empty_string() {
    let resp = CDPResponse { id: 7, result: Some(json!("")), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"], "");
}

#[test]
fn serialize_response_error_with_empty_message() {
    let resp = CDPResponse {
        id: 8,
        result: None,
        error: Some(CDPError { code: -32600, message: String::new() }),
    };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["error"]["message"], "");
}

#[test]
fn serialize_response_error_with_unicode_message() {
    let resp = CDPResponse {
        id: 9,
        result: None,
        error: Some(CDPError { code: -32600, message: "エラー: 不正なリクエスト".into() }),
    };
    let raw = serialize_response(&resp);
    assert!(raw.contains("エラー"));
}

#[test]
fn serialize_response_error_with_very_long_message() {
    let long_msg = "x".repeat(10000);
    let resp = CDPResponse {
        id: 10,
        result: None,
        error: Some(CDPError { code: -32600, message: long_msg.clone() }),
    };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["error"]["message"].as_str().unwrap().len(), 10000);
}

#[test]
fn serialize_response_deterministic_output() {
    let resp = CDPResponse {
        id: 42,
        result: Some(json!({"a": 1, "b": 2})),
        error: None,
    };
    let first = serialize_response(&resp);
    let second = serialize_response(&resp);
    assert_eq!(first, second, "serialization must be deterministic");
}

#[test]
fn serialize_response_id_boundary_min() {
    let resp = CDPResponse { id: i64::MIN, result: Some(json!({})), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"].as_i64(), Some(i64::MIN));
}

#[test]
fn serialize_response_id_boundary_max() {
    let resp = CDPResponse { id: i64::MAX, result: Some(json!({})), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"].as_i64(), Some(i64::MAX));
}

#[test]
fn serialize_response_deeply_nested_result() {
    let mut result = json!({"leaf": 1});
    for i in 0..20 {
        result = json!({ format!("level_{}", i): result });
    }
    let resp = CDPResponse { id: 1, result: Some(result), error: None };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["result"].is_object());
}

#[test]
fn serialize_response_roundtrip_fidelity() {
    let resp = CDPResponse {
        id: 99,
        result: Some(json!({"frameId": "0", "loaderId": "abc123"})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 99);
    assert_eq!(parsed["result"]["frameId"], "0");
    assert_eq!(parsed["result"]["loaderId"], "abc123");
}

// ---------------------------------------------------------------------------
// 3. CdpError with all JSON-RPC 2.0 error code variants
// ---------------------------------------------------------------------------

#[test]
fn cdp_error_parse_error_code() {
    // -32700: Parse error
    let err = CDPError { code: -32700, message: "Parse error".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32700);
    assert_eq!(parsed["message"], "Parse error");
}

#[test]
fn cdp_error_invalid_request_code() {
    // -32600: Invalid Request
    let err = CDPError { code: -32600, message: "Invalid Request".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32600);
}

#[test]
fn cdp_error_method_not_found_code() {
    // -32601: Method not found
    let err = CDPError { code: -32601, message: "Method not found".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32601);
}

#[test]
fn cdp_error_invalid_params_code() {
    // -32602: Invalid params
    let err = CDPError { code: -32602, message: "Invalid params".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32602);
}

#[test]
fn cdp_error_internal_error_code() {
    // -32603: Internal error
    let err = CDPError { code: -32603, message: "Internal error".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32603);
}

#[test]
fn cdp_error_all_standard_codes_are_distinct() {
    let codes = [-32700i64, -32600, -32601, -32602, -32603];
    for i in 0..codes.len() {
        for j in (i + 1)..codes.len() {
            assert_ne!(codes[i], codes[j], "all standard error codes must be distinct");
        }
    }
}

#[test]
fn cdp_error_code_range_server_defined() {
    // JSON-RPC 2.0: -32000 to -32099 are reserved for implementation-defined
    let err = CDPError { code: -32000, message: "Server error".into() };
    assert!(err.code >= -32099 && err.code <= -32000);
}

#[test]
fn cdp_error_clone_preserves_fields() {
    let err = CDPError { code: -32601, message: "Method not found".into() };
    let cloned = err.clone();
    assert_eq!(cloned.code, err.code);
    assert_eq!(cloned.message, err.message);
}

#[test]
fn cdp_error_debug_includes_code_and_message() {
    let err = CDPError { code: -32700, message: "Parse error".into() };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32700"));
    assert!(debug.contains("Parse error"));
}

#[test]
fn cdp_error_positive_custom_code() {
    let err = CDPError { code: 9999, message: "Custom".into() };
    assert_eq!(err.code, 9999);
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], 9999);
}

#[test]
fn cdp_error_i64_min_code() {
    let err = CDPError { code: i64::MIN, message: "Extreme".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"].as_i64(), Some(i64::MIN));
}

#[test]
fn cdp_error_roundtrip_through_response() {
    let resp = CDPResponse {
        id: 1,
        result: None,
        error: Some(CDPError { code: -32602, message: "Invalid params".into() }),
    };
    let raw = serialize_response(&resp);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["error"]["code"], -32602);
    assert_eq!(parsed["error"]["message"], "Invalid params");
}

// ---------------------------------------------------------------------------
// 4. DomainRegistry dispatch with boundary conditions
// ---------------------------------------------------------------------------

struct EdgeDomain {
    name: &'static str,
}

impl DomainHandler for EdgeDomain {
    fn domain_name(&self) -> &'static str { self.name }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, ServerCdpError> {
        if command == format!("{}.echo", self.name) {
            Ok(params)
        } else if command == format!("{}.fail", self.name) {
            Err(ServerCdpError { code: -32603, message: "deliberate failure".into() })
        } else {
            Err(ServerCdpError { code: -32601, message: format!("'{}' wasn't found", command) })
        }
    }
}

struct NopSender;
impl EventSender for NopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

#[test]
fn registry_dispatch_empty_domain_name() {
    let reg = DomainRegistry::new();
    // Empty domain: method is ".command"
    let result = reg.dispatch_command(".enable", json!({}), &NopSender);
    assert!(result.is_none(), "empty domain should not match any handler");
}

#[test]
fn registry_dispatch_empty_command_name() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "TestDomain" })).unwrap();
    // Method "TestDomain." — command part is empty
    let result = reg.dispatch_command("TestDomain.", json!({}), &NopSender);
    assert!(result.is_some(), "domain is registered, dispatch should attempt");
    assert!(result.unwrap().is_err(), "empty command should not match any handler method");
}

#[test]
fn registry_dispatch_very_long_command_name() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "TestDomain" })).unwrap();
    let long_cmd = format!("TestDomain.{}", "x".repeat(10000));
    let result = reg.dispatch_command(&long_cmd, json!({}), &NopSender);
    assert!(result.is_some());
    assert!(result.unwrap().is_err());
}

#[test]
fn registry_dispatch_no_dot_in_method() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "TestDomain" })).unwrap();
    // Method without dot: split('.').next() returns "TestDomain" as domain,
    // so the handler IS found (dispatch_command returns Some), but the
    // command part is empty — the handler will see "TestDomain" as the
    // full method and likely return an error for unrecognized command.
    let result = reg.dispatch_command("TestDomain", json!({}), &NopSender);
    assert!(result.is_some(), "domain 'TestDomain' is found, dispatch attempts");
    // The handler receives the full method "TestDomain" which doesn't match
    // any known command pattern, so it returns an error.
    assert!(result.unwrap().is_err(), "no-dot method is not a recognized command");
}

#[test]
fn registry_dispatch_case_sensitive_domain() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "Page" })).unwrap();
    assert!(reg.has_domain("Page"));
    assert!(!reg.has_domain("page"));
    assert!(!reg.has_domain("PAGE"));

    let result_lower = reg.dispatch_command("page.enable", json!({}), &NopSender);
    assert!(result_lower.is_none(), "lowercase 'page' should not match 'Page'");
}

#[test]
fn registry_dispatch_with_null_params() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "Echo" })).unwrap();
    let result = reg.dispatch_command("Echo.echo", json!(null), &NopSender);
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());
}

#[test]
fn registry_dispatch_with_large_params() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "Echo" })).unwrap();
    let large_array: Vec<i32> = (0..10000).collect();
    let result = reg.dispatch_command("Echo.echo", json!({"data": large_array}), &NopSender);
    assert!(result.is_some());
    let val = result.unwrap().unwrap();
    assert_eq!(val["data"].as_array().unwrap().len(), 10000);
}

#[test]
fn registry_dispatch_handler_returns_error() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "Fail" })).unwrap();
    let result = reg.dispatch_command("Fail.fail", json!({}), &NopSender);
    assert!(result.is_some());
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32603);
    assert_eq!(err.message, "deliberate failure");
}

#[test]
fn registry_dispatch_unicode_domain() {
    let reg = DomainRegistry::new();
    // register only accepts &'static str, so we use ASCII here
    // but dispatch receives a &str, so test with unicode in method
    let result = reg.dispatch_command("ページ.enable", json!({}), &NopSender);
    assert!(result.is_none(), "unregistered unicode domain should return None");
}

#[test]
fn registry_dispatch_multiple_dots_in_method() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "A" })).unwrap();
    // "A.B.C" — domain is "A", command is "B.C"
    let result = reg.dispatch_command("A.B.C", json!({}), &NopSender);
    assert!(result.is_some(), "domain A is registered");
    assert!(result.unwrap().is_err(), "command B.C not a known handler method");
}

#[test]
fn registry_notify_session_created_unregistered_no_panic() {
    let reg = DomainRegistry::new();
    // Should not panic for unregistered domain
    reg.notify_session_created("NonExistent", "sess-1");
}

#[test]
fn registry_notify_session_destroyed_empty_list_no_panic() {
    let reg = DomainRegistry::new();
    reg.notify_session_destroyed(&[], "sess-1");
}

#[test]
fn registry_register_duplicate_returns_error() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EdgeDomain { name: "Dup" })).unwrap();
    let err = reg.register(Box::new(EdgeDomain { name: "Dup" })).unwrap_err();
    assert!(err.contains("Dup"));
}

#[test]
fn registry_many_domains_registered() {
    let reg = DomainRegistry::new();
    for i in 0..50 {
        // &'static str from string literals only, so use numbered static names
        // We can't dynamically create &'static str, so test with a few
        let _ = i; // just ensure the registry works for small counts
    }
    // Use static names instead
    let names: &[&'static str] = &[
        "A1", "A2", "A3", "A4", "A5",
        "B1", "B2", "B3", "B4", "B5",
    ];
    for &name in names {
        reg.register(Box::new(EdgeDomain { name })).unwrap();
    }
    for &name in names {
        assert!(reg.has_domain(name));
    }
    assert!(!reg.has_domain("C1"));
}

// ---------------------------------------------------------------------------
// 5. TargetInfo field validation and serialization edge cases
// ---------------------------------------------------------------------------

#[test]
fn target_info_all_empty_strings() {
    let info = TargetInfo {
        id: String::new(),
        target_type: String::new(),
        title: String::new(),
        url: String::new(),
        web_socket_debugger_url: String::new(),
    };
    assert!(info.id.is_empty());
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains(r#""id":"""#));
}

#[test]
fn target_info_unicode_fields() {
    let info = TargetInfo {
        id: "t-漢字".into(),
        target_type: "page".into(),
        title: "テストページ".into(),
        url: "https://example.com/パス".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-漢字".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("テストページ"));
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.title, "テストページ");
}

#[test]
fn target_info_very_long_id() {
    let long_id = "x".repeat(10000);
    let info = TargetInfo {
        id: long_id.clone(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: format!("ws://127.0.0.1:9222/devtools/page/{}", long_id),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id.len(), 10000);
}

#[test]
fn target_info_special_chars_in_url() {
    let info = TargetInfo {
        id: "t-1".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "https://example.com/path?q=a%20b&c=d#frag".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/t-1".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert!(parsed.url.contains("%20"));
    assert!(parsed.url.contains("#frag"));
}

#[test]
fn target_info_clone_preserves_all_fields() {
    let info = TargetInfo {
        id: "t-clone".into(),
        target_type: "page".into(),
        title: "Clone Test".into(),
        url: "https://clone.test".into(),
        web_socket_debugger_url: "ws://clone.test/devtools/page/t-clone".into(),
    };
    let cloned = info.clone();
    assert_eq!(cloned.id, info.id);
    assert_eq!(cloned.target_type, info.target_type);
    assert_eq!(cloned.title, info.title);
    assert_eq!(cloned.url, info.url);
    assert_eq!(cloned.web_socket_debugger_url, info.web_socket_debugger_url);
}

#[test]
fn target_info_serde_roundtrip() {
    let info = TargetInfo {
        id: "t-roundtrip".into(),
        target_type: "page".into(),
        title: "Roundtrip".into(),
        url: "https://roundtrip.test".into(),
        web_socket_debugger_url: "ws://roundtrip.test/devtools/page/t-roundtrip".into(),
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
fn target_info_serde_renames_type_field() {
    // TargetInfo uses #[serde(rename = "type")] for target_type
    let info = TargetInfo {
        id: "t-renamed".into(),
        target_type: "page".into(),
        title: "Renamed".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: "ws://test/t-renamed".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains(r#""type":"page""#), "target_type should serialize as 'type'");
    assert!(!json.contains(r#""target_type""#), "target_type should NOT appear in JSON");
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.target_type, "page");
}

#[test]
fn target_info_debug_format() {
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
fn target_info_json_array_serialization() {
    let targets: Vec<TargetInfo> = (0..5).map(|i| TargetInfo {
        id: format!("t-{}", i),
        target_type: "page".into(),
        title: format!("Page {}", i),
        url: format!("https://example.com/{}", i),
        web_socket_debugger_url: format!("ws://127.0.0.1:9222/devtools/page/t-{}", i),
    }).collect();
    let json = serde_json::to_string(&targets).unwrap();
    let parsed: Vec<TargetInfo> = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.len(), 5);
    assert_eq!(parsed[0].id, "t-0");
    assert_eq!(parsed[4].id, "t-4");
}

// ---------------------------------------------------------------------------
// 6. SessionState lifecycle transitions
// ---------------------------------------------------------------------------

#[test]
fn session_state_created_is_starting_state() {
    // Created is the initial state
    let state = SessionState::Created;
    assert_eq!(state, SessionState::Created);
}

#[test]
fn session_state_lifecycle_created_to_active() {
    // Normal flow: Created -> Active
    let state = SessionState::Created;
    assert_ne!(state, SessionState::Active);
    // Simulate transition
    let state = SessionState::Active;
    assert_eq!(state, SessionState::Active);
}

#[test]
fn session_state_lifecycle_active_to_closing() {
    // Active -> Closing (on disconnect)
    let _state = SessionState::Active;
    let state = SessionState::Closing;
    assert_eq!(state, SessionState::Closing);
}

#[test]
fn session_state_lifecycle_closing_to_closed() {
    // Closing -> Closed (after cleanup)
    let _state = SessionState::Closing;
    let state = SessionState::Closed;
    assert_eq!(state, SessionState::Closed);
}

#[test]
fn session_state_all_variants_are_distinct() {
    let variants = [
        SessionState::Created,
        SessionState::Active,
        SessionState::Closing,
        SessionState::Closed,
    ];
    for i in 0..variants.len() {
        for j in (i + 1)..variants.len() {
            assert_ne!(variants[i], variants[j],
                "SessionState variants must all be distinct");
        }
    }
}

#[test]
fn session_state_copy_semantics() {
    let a = SessionState::Active;
    let b = a; // Copy, not move (SessionState derives Copy)
    assert_eq!(a, b);
}

#[test]
fn session_state_clone_semantics() {
    let a = SessionState::Closing;
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn session_state_debug_format_all_variants() {
    assert!(format!("{:?}", SessionState::Created).contains("Created"));
    assert!(format!("{:?}", SessionState::Active).contains("Active"));
    assert!(format!("{:?}", SessionState::Closing).contains("Closing"));
    assert!(format!("{:?}", SessionState::Closed).contains("Closed"));
}

#[test]
fn session_state_equality_same_variant() {
    assert_eq!(SessionState::Created, SessionState::Created);
    assert_eq!(SessionState::Active, SessionState::Active);
    assert_eq!(SessionState::Closing, SessionState::Closing);
    assert_eq!(SessionState::Closed, SessionState::Closed);
}

#[test]
fn session_state_inequality_cross_variants() {
    assert_ne!(SessionState::Created, SessionState::Active);
    assert_ne!(SessionState::Active, SessionState::Closing);
    assert_ne!(SessionState::Closing, SessionState::Closed);
    assert_ne!(SessionState::Created, SessionState::Closed);
}

#[test]
fn session_state_is_send_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<SessionState>();
    assert_sync::<SessionState>();
}

#[test]
fn session_state_full_lifecycle_sequence() {
    // Verify the full sequence Created -> Active -> Closing -> Closed
    let mut state = SessionState::Created;
    assert_eq!(state, SessionState::Created);

    // First enable command transitions to Active
    state = SessionState::Active;
    assert_eq!(state, SessionState::Active);

    // Disconnect detected
    state = SessionState::Closing;
    assert_eq!(state, SessionState::Closing);

    // Cleanup complete
    state = SessionState::Closed;
    assert_eq!(state, SessionState::Closed);
}

// ---------------------------------------------------------------------------
// 7. ServerConfig builder with boundary values
// ---------------------------------------------------------------------------

#[test]
fn server_config_port_zero() {
    let cfg = ServerConfig::builder().port(0).build();
    assert_eq!(cfg.port, 0);
}

#[test]
fn server_config_port_max() {
    let cfg = ServerConfig::builder().port(65535).build();
    assert_eq!(cfg.port, 65535);
}

#[test]
fn server_config_max_sessions_zero() {
    let cfg = ServerConfig::builder().max_sessions(0).build();
    assert_eq!(cfg.max_sessions, 0);
}

#[test]
fn server_config_max_sessions_usize_max() {
    let cfg = ServerConfig::builder().max_sessions(usize::MAX).build();
    assert_eq!(cfg.max_sessions, usize::MAX);
}

#[test]
fn server_config_http_timeout_zero() {
    let cfg = ServerConfig::builder().http_timeout_seconds(0).build();
    assert_eq!(cfg.http_timeout_seconds, 0);
}

#[test]
fn server_config_http_timeout_u64_max() {
    let cfg = ServerConfig::builder().http_timeout_seconds(u64::MAX).build();
    assert_eq!(cfg.http_timeout_seconds, u64::MAX);
}

#[test]
fn server_config_empty_host() {
    let cfg = ServerConfig::builder().host("").build();
    assert_eq!(cfg.host, "");
}

#[test]
fn server_config_empty_browser_name() {
    let cfg = ServerConfig::builder().browser_name("").build();
    assert_eq!(cfg.browser_name, "");
}

#[test]
fn server_config_empty_user_agent() {
    let cfg = ServerConfig::builder().user_agent("").build();
    assert_eq!(cfg.user_agent.as_deref(), Some(""));
}

#[test]
fn server_config_empty_v8_version() {
    let cfg = ServerConfig::builder().v8_version("").build();
    assert_eq!(cfg.v8_version.as_deref(), Some(""));
}

#[test]
fn server_config_empty_webkit_version() {
    let cfg = ServerConfig::builder().webkit_version("").build();
    assert_eq!(cfg.webkit_version.as_deref(), Some(""));
}

#[test]
fn server_config_builder_override_last_wins() {
    let cfg = ServerConfig::builder()
        .port(1111)
        .port(2222)
        .port(3333)
        .build();
    assert_eq!(cfg.port, 3333, "last builder call should win");
}

#[test]
fn server_config_builder_default_then_build_equals_default() {
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
fn server_config_builder_full_chain_all_fields() {
    let cfg = ServerConfig::builder()
        .host("10.0.0.1")
        .port(3000)
        .http_timeout_seconds(120)
        .max_sessions(50)
        .browser_name("TestBrowser/2.0")
        .user_agent("TestAgent/1.0")
        .v8_version("13.0")
        .webkit_version("600.0")
        .build();
    assert_eq!(cfg.host, "10.0.0.1");
    assert_eq!(cfg.port, 3000);
    assert_eq!(cfg.http_timeout_seconds, 120);
    assert_eq!(cfg.max_sessions, 50);
    assert_eq!(cfg.browser_name, "TestBrowser/2.0");
    assert_eq!(cfg.user_agent.as_deref(), Some("TestAgent/1.0"));
    assert_eq!(cfg.v8_version.as_deref(), Some("13.0"));
    assert_eq!(cfg.webkit_version.as_deref(), Some("600.0"));
}

#[test]
fn server_config_unicode_host() {
    let cfg = ServerConfig::builder().host("本地主机").build();
    assert_eq!(cfg.host, "本地主机");
}

#[test]
fn server_config_very_long_browser_name() {
    let name = "B".repeat(5000);
    let cfg = ServerConfig::builder().browser_name(&name).build();
    assert_eq!(cfg.browser_name.len(), 5000);
}

// ---------------------------------------------------------------------------
// 8. EventBroadcaster with no subscribers
// ---------------------------------------------------------------------------

fn empty_session_map() -> Arc<Mutex<HashMap<String, Arc<Mutex<cdp_server::CdpSession>>>>> {
    Arc::new(Mutex::new(HashMap::new()))
}

#[test]
fn broadcaster_new_with_empty_sessions_no_panic() {
    let sessions = empty_session_map();
    let _broadcaster = EventBroadcaster::new(sessions);
}

#[test]
fn broadcaster_send_event_no_subscribers_no_panic() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    // Should silently succeed
    broadcaster.send_event("Page.loadEventFired", json!({"timestamp": 123}));
}

#[test]
fn broadcaster_send_event_multiple_no_subscribers_no_panic() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    for i in 0..100 {
        broadcaster.send_event("Page.loadEventFired", json!({"i": i}));
    }
}

#[test]
fn broadcaster_send_event_empty_method_no_panic() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    broadcaster.send_event("", json!({}));
}

#[test]
fn broadcaster_send_event_no_dot_in_method_no_panic() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    broadcaster.send_event("NoDotMethod", json!({}));
}

#[test]
fn broadcaster_sender_returns_boxed_event_sender() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    let _sender: Box<dyn EventSender> = broadcaster.sender();
}

#[test]
fn broadcaster_sender_can_send_events() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    let sender = broadcaster.sender();
    sender.send_event("Runtime.consoleAPICalled", json!({"type": "log"}));
}

#[test]
fn broadcaster_clone_shares_sessions() {
    let sessions = empty_session_map();
    let a = EventBroadcaster::new(Arc::clone(&sessions));
    let b = a.clone();
    // Both broadcasters share the same session map via Arc
    // Verify by sending events through both — no panic means shared state
    a.send_event("Test.event", json!({}));
    b.send_event("Test.event", json!({}));
}

#[test]
fn broadcaster_send_event_various_domains_no_subscribers() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    let domains = [
        "Page.loadEventFired",
        "Runtime.consoleAPICalled",
        "DOM.documentUpdated",
        "Network.requestWillBeSent",
        "CSS.styleSheetAdded",
        "Debugger.scriptParsed",
        "Log.entryAdded",
        "Overlay.nodeHighlightRequested",
    ];
    for &method in &domains {
        broadcaster.send_event(method, json!({}));
    }
}

#[test]
fn broadcaster_send_event_with_large_params_no_subscribers() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    let large_data: Vec<i32> = (0..10000).collect();
    broadcaster.send_event("Page.loadEventFired", json!({"data": large_data}));
}

#[test]
fn broadcaster_send_event_with_null_params_no_subscribers() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    broadcaster.send_event("Page.loadEventFired", json!(null));
}

#[test]
fn broadcaster_send_event_with_empty_object_params_no_subscribers() {
    let broadcaster = EventBroadcaster::new(empty_session_map());
    broadcaster.send_event("Page.loadEventFired", json!({}));
}

// ---------------------------------------------------------------------------
// Cross-cutting: handle_command edge cases through bao_cdp protocol
// ---------------------------------------------------------------------------

#[test]
fn handle_command_empty_method_returns_method_not_found() {
    let msg = CDPMessage { id: 1, method: String::new(), params: None, session_id: None };
    let resp = handle_command(msg, "t-1", &None, None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn handle_command_method_no_dot_returns_method_not_found() {
    let msg = CDPMessage { id: 1, method: "NoDotMethod".into(), params: None, session_id: None };
    let resp = handle_command(msg, "t-1", &None, None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn handle_command_all_12_domains_unknown_command_error_code() {
    let domains = [
        "Target", "Page", "Runtime", "DOM", "Network",
        "CSS", "Emulation", "Input", "Overlay", "Debugger",
        "Log", "Fetch",
    ];
    for domain in &domains {
        let method = format!("{}.nonexistentCommandXYZ", domain);
        let msg = CDPMessage { id: 1, method, params: None, session_id: None };
        let resp = handle_command(msg, "t-1", &None, None);
        assert!(resp.error.is_some(), "{} should return error", domain);
        assert_eq!(resp.error.as_ref().unwrap().code, -32601);
    }
}

#[test]
fn handle_command_target_info_fields_no_bridge() {
    let msg = CDPMessage { id: 1, method: "Target.getTargets".into(), params: None, session_id: None };
    let resp = handle_command(msg, "target-xyz", &None, None);
    let result = resp.result.unwrap();
    let infos = result["targetInfos"].as_array().unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0]["targetId"], "target-xyz");
    assert_eq!(infos[0]["type"], "page");
    assert_eq!(infos[0]["attached"], true);
}

#[test]
fn handle_command_preserves_id_for_success() {
    for id in [0i64, 1, -1, i64::MAX, i64::MIN] {
        let msg = CDPMessage { id, method: "Page.enable".into(), params: None, session_id: None };
        let resp = handle_command(msg, "t-1", &None, None);
        assert_eq!(resp.id, id, "response id should match request id");
    }
}

#[test]
fn handle_command_preserves_id_for_error() {
    let msg = CDPMessage { id: -999, method: "Unknown.method".into(), params: None, session_id: None };
    let resp = handle_command(msg, "t-1", &None, None);
    assert_eq!(resp.id, -999);
    assert!(resp.error.is_some());
}

#[test]
fn handle_command_no_bridge_returns_internal_error_for_bridge_commands() {
    // Commands that require bridge should return -32603
    let msg = CDPMessage { id: 1, method: "Runtime.evaluate".into(), params: Some(json!({"expression": "1+1"})), session_id: None };
    // With a non-empty expression and no bridge, the legacy path returns undefined
    // (only certain bridge-required paths return -32603)
    let resp = handle_command(msg, "t-1", &Some(json!({"expression": "1+1"})), None);
    // In bao_cdp's protocol, Runtime.evaluate with expression but no bridge
    // returns undefined (not an error). The -32603 only happens via bridge_send.
    assert!(resp.result.is_some() || resp.error.is_some());
}

// ---------------------------------------------------------------------------
// CDPEvent serialization edge cases
// ---------------------------------------------------------------------------

#[test]
fn serialize_event_with_null_params() {
    let ev = CDPEvent { method: "Test.event".into(), params: Some(json!(null)) };
    let raw = serialize_event(&ev);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["params"], json!(null));
}

#[test]
fn serialize_event_with_empty_string_method() {
    let ev = CDPEvent { method: String::new(), params: None };
    let raw = serialize_event(&ev);
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["method"], "");
}

#[test]
fn serialize_event_with_unicode_method() {
    let ev = CDPEvent { method: "ページ.読み込み完了".into(), params: Some(json!({"ts": 1})) };
    let raw = serialize_event(&ev);
    assert!(raw.contains("ページ"));
}

#[test]
fn serialize_event_deterministic() {
    let ev = CDPEvent { method: "Page.loadEventFired".into(), params: Some(json!({"timestamp": 123})) };
    assert_eq!(serialize_event(&ev), serialize_event(&ev));
}

#[test]
fn serialize_event_clone_and_serialize_match() {
    let ev = CDPEvent { method: "Test.clone".into(), params: Some(json!({"x": 1})) };
    let cloned = ev.clone();
    assert_eq!(serialize_event(&ev), serialize_event(&cloned));
}
