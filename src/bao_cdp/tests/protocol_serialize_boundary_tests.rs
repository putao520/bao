// @trace TEST-CDP-015-SERIAL [req:REQ-CDP-001,REQ-CDP-005] [level:unit]
// CDPMessage parse/serialize boundary tests: empty method, large id,
// nested params, null params, session_id variants, CDPResponse success/error,
// CDPEvent with/without params, serialize_response edge cases,
// parse_message invalid inputs, roundtrip consistency.

use bao_cdp::{CDPMessage, CDPResponse, CDPError, CDPEvent, parse_message, serialize_response, serialize_event};
use serde_json::json;

// ---- parse_message valid inputs ----

#[test]
fn test_parse_minimal_message() {
    let raw = r#"{"id":1,"method":"Test.run"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 1);
    assert_eq!(msg.method, "Test.run");
    assert!(msg.params.is_none());
    assert!(msg.session_id.is_none());
}

#[test]
fn test_parse_message_with_params() {
    let raw = r#"{"id":2,"method":"Page.navigate","params":{"url":"https://example.com"}}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.params.unwrap()["url"], "https://example.com");
}

#[test]
fn test_parse_message_with_session_id() {
    let raw = r#"{"id":3,"method":"Runtime.evaluate","params":{"expression":"1+1"},"session_id":"sess-abc"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.session_id.unwrap(), "sess-abc");
}

#[test]
fn test_parse_message_empty_params_object() {
    let raw = r#"{"id":4,"method":"Page.enable","params":{}}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.params.unwrap().as_object().unwrap().is_empty());
}

#[test]
fn test_parse_message_null_params() {
    let raw = r#"{"id":5,"method":"Page.enable","params":null}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.params.is_none() || msg.params.as_ref().map_or(true, |p| p.is_null()));
}

#[test]
fn test_parse_message_large_id() {
    let raw = r#"{"id":9223372036854775807,"method":"Test.run"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, i64::MAX);
}

#[test]
fn test_parse_message_zero_id() {
    let raw = r#"{"id":0,"method":"Test.run"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 0);
}

#[test]
fn test_parse_message_negative_id() {
    let raw = r#"{"id":-1,"method":"Test.run"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, -1);
}

#[test]
fn test_parse_message_nested_params() {
    let raw = r#"{"id":6,"method":"Test.run","params":{"a":{"b":{"c":[1,2,3]}}}}"#;
    let msg = parse_message(raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["a"]["b"]["c"].as_array().unwrap().len(), 3);
}

#[test]
fn test_parse_message_array_params() {
    let raw = r#"{"id":7,"method":"Test.run","params":[1,"two",true,null]}"#;
    let msg = parse_message(raw).unwrap();
    let params = msg.params.unwrap();
    let arr = params.as_array().unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[0], 1);
    assert_eq!(arr[1], "two");
    assert_eq!(arr[2], true);
    assert!(arr[3].is_null());
}

#[test]
fn test_parse_message_empty_method() {
    let raw = r#"{"id":8,"method":""}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.method.is_empty());
}

#[test]
fn test_parse_message_unicode_method() {
    let raw = r#"{"id":9,"method":"ドメイン.実行"}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.method.contains("ドメイン"));
}

#[test]
fn test_parse_message_long_session_id() {
    let sid = "a".repeat(1000);
    let raw = format!(r#"{{"id":10,"method":"Test.run","session_id":"{}"}}"#, sid);
    let msg = parse_message(&raw).unwrap();
    assert_eq!(msg.session_id.unwrap().len(), 1000);
}

// ---- parse_message invalid inputs ----

#[test]
fn test_parse_message_empty_string() {
    assert!(parse_message("").is_none());
}

#[test]
fn test_parse_message_invalid_json() {
    assert!(parse_message("{not json}").is_none());
}

#[test]
fn test_parse_message_missing_id() {
    let raw = r#"{"method":"Test.run"}"#;
    assert!(parse_message(raw).is_none());
}

#[test]
fn test_parse_message_missing_method() {
    let raw = r#"{"id":1}"#;
    assert!(parse_message(raw).is_none());
}

#[test]
fn test_parse_message_string_id() {
    let raw = r#"{"id":"abc","method":"Test.run"}"#;
    assert!(parse_message(raw).is_none());
}

#[test]
fn test_parse_message_array_root() {
    assert!(parse_message(r#"[1,2,3]"#).is_none());
}

#[test]
fn test_parse_message_number_root() {
    assert!(parse_message("42").is_none());
}

#[test]
fn test_parse_message_null_root() {
    assert!(parse_message("null").is_none());
}

#[test]
fn test_parse_message_extra_fields() {
    // Extra fields should be ignored by serde
    let raw = r#"{"id":1,"method":"Test.run","extra":"ignored"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 1);
}

// ---- serialize_response ----

#[test]
fn test_serialize_response_success() {
    let resp = CDPResponse {
        id: 1,
        result: Some(json!({"ok": true})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 1);
    assert_eq!(parsed["result"]["ok"], true);
    assert!(parsed.get("error").is_none());
}

#[test]
fn test_serialize_response_error() {
    let resp = CDPResponse {
        id: 2,
        result: None,
        error: Some(CDPError { code: -32601, message: "not found".into() }),
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 2);
    assert!(parsed.get("result").is_none());
    assert_eq!(parsed["error"]["code"], -32601);
    assert_eq!(parsed["error"]["message"], "not found");
}

#[test]
fn test_serialize_response_null_result() {
    let resp = CDPResponse {
        id: 3,
        result: None,
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 3);
}

#[test]
fn test_serialize_response_empty_result() {
    let resp = CDPResponse {
        id: 4,
        result: Some(json!({})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"].as_object().unwrap().len(), 0);
}

#[test]
fn test_serialize_response_nested_result() {
    let resp = CDPResponse {
        id: 5,
        result: Some(json!({"data": {"nested": {"deep": true}}})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"]["data"]["nested"]["deep"], true);
}

#[test]
fn test_serialize_response_array_result() {
    let resp = CDPResponse {
        id: 6,
        result: Some(json!([1, 2, 3])),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"].as_array().unwrap().len(), 3);
}

#[test]
fn test_serialize_response_negative_id() {
    let resp = CDPResponse {
        id: -100,
        result: Some(json!({})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], -100);
}

// ---- serialize_event ----

#[test]
fn test_serialize_event_with_params() {
    let ev = CDPEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345})),
    };
    let raw = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["method"], "Page.loadEventFired");
    assert_eq!(parsed["params"]["timestamp"], 12345);
}

#[test]
fn test_serialize_event_without_params() {
    let ev = CDPEvent {
        method: "DOM.updated".into(),
        params: None,
    };
    let raw = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["method"], "DOM.updated");
    assert!(parsed.get("params").is_none());
}

#[test]
fn test_serialize_event_empty_params() {
    let ev = CDPEvent {
        method: "Network.requestWillBeSent".into(),
        params: Some(json!({})),
    };
    let raw = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["params"].as_object().unwrap().is_empty());
}

#[test]
fn test_serialize_event_complex_params() {
    let ev = CDPEvent {
        method: "Runtime.consoleAPICalled".into(),
        params: Some(json!({
            "type": "log",
            "args": [{"type": "string", "value": "hello"}],
            "timestamp": 999
        })),
    };
    let raw = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["params"]["args"][0]["value"], "hello");
}

// ---- CDPError ----

#[test]
fn test_cdp_error_code_message() {
    let err = CDPError { code: -32600, message: "invalid request".into() };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid request");
}

#[test]
fn test_cdp_error_debug() {
    let err = CDPError { code: -32700, message: "parse error".into() };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32700"));
    assert!(debug.contains("parse error"));
}

#[test]
fn test_cdp_error_serialize() {
    let err = CDPError { code: -32000, message: "internal".into() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["code"], -32000);
    assert_eq!(parsed["message"], "internal");
}

#[test]
fn test_cdp_error_empty_message() {
    let err = CDPError { code: -1, message: String::new() };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["message"], "");
}

// ---- Roundtrip: parse → serialize ----

#[test]
fn test_roundtrip_response_serialize() {
    let resp = CDPResponse {
        id: 42,
        result: Some(json!({"frameId": "main", "loaderId": "l-1"})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 42);
    assert_eq!(parsed["result"]["frameId"], "main");
}

#[test]
fn test_roundtrip_error_response() {
    let resp = CDPResponse {
        id: 99,
        result: None,
        error: Some(CDPError { code: -32601, message: "'Foo.bar' wasn't found".into() }),
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["error"]["message"].as_str().unwrap().contains("Foo.bar"));
}

// ---- CDPMessage clone ----

#[test]
fn test_cdp_message_clone() {
    let msg = CDPMessage {
        id: 1,
        method: "Page.enable".into(),
        params: Some(json!({"key": "val"})),
        session_id: Some("sess-1".into()),
    };
    let cloned = msg.clone();
    assert_eq!(cloned.id, msg.id);
    assert_eq!(cloned.method, msg.method);
    assert_eq!(cloned.params, msg.params);
    assert_eq!(cloned.session_id, msg.session_id);
}

#[test]
fn test_cdp_message_debug() {
    let msg = CDPMessage {
        id: 1,
        method: "Test.run".into(),
        params: None,
        session_id: None,
    };
    let debug = format!("{:?}", msg);
    assert!(debug.contains("Test.run"));
}

// ---- CDPEvent clone ----

#[test]
fn test_cdp_event_clone() {
    let ev = CDPEvent {
        method: "Page.load".into(),
        params: Some(json!({"ts": 1})),
    };
    let cloned = ev.clone();
    assert_eq!(cloned.method, ev.method);
    assert_eq!(cloned.params, ev.params);
}

// ---- serialize determinism ----

#[test]
fn test_serialize_response_deterministic() {
    let resp = CDPResponse {
        id: 1,
        result: Some(json!({"a": 1})),
        error: None,
    };
    let r1 = serialize_response(&resp);
    let r2 = serialize_response(&resp);
    assert_eq!(r1, r2);
}

#[test]
fn test_serialize_event_deterministic() {
    let ev = CDPEvent {
        method: "Test.ev".into(),
        params: Some(json!({"x": 1})),
    };
    let r1 = serialize_event(&ev);
    let r2 = serialize_event(&ev);
    assert_eq!(r1, r2);
}

// ---- Boundary: large params ----

#[test]
fn test_parse_message_large_array_params() {
    let arr: Vec<i32> = (0..1000).collect();
    let raw = format!(r#"{{"id":1,"method":"Test.run","params":{}}}"#, serde_json::to_string(&arr).unwrap());
    let msg = parse_message(&raw).unwrap();
    assert_eq!(msg.params.unwrap().as_array().unwrap().len(), 1000);
}

#[test]
fn test_serialize_response_large_result() {
    let data: Vec<i32> = (0..500).collect();
    let resp = CDPResponse {
        id: 1,
        result: Some(json!({"data": data})),
        error: None,
    };
    let raw = serialize_response(&resp);
    assert!(raw.len() > 1000);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"]["data"].as_array().unwrap().len(), 500);
}

// ---- Special characters in method/params ----

#[test]
fn test_parse_message_special_chars_in_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"url":"https://example.com/path?q=a%20b&c=d#frag"}}"#;
    let msg = parse_message(raw).unwrap();
    let params = msg.params.unwrap();
    let url = params["url"].as_str().unwrap();
    assert!(url.contains("%20"));
    assert!(url.contains("#frag"));
}

#[test]
fn test_serialize_response_unicode_in_error() {
    let resp = CDPResponse {
        id: 1,
        result: None,
        error: Some(CDPError { code: -32000, message: "エラーが発生しました".into() }),
    };
    let raw = serialize_response(&resp);
    assert!(raw.contains("エラー"));
}
