// @trace TEST-CDP-016-TYPES [req:REQ-CDP-001,REQ-CDP-004] [level:unit]
// CdpMessage/CdpError/CdpResponse/CdpEvent deep edge cases:
// field validation, serialization, clone/debug, boundary values, parse errors,
// large inputs, unicode, determinism.

use bao_cdp::{
    parse_message, serialize_event, serialize_response, CdpError, CdpEvent, CdpMessage, CdpResponse,
};

const TID: &str = "test-target";
use serde_json::json;

// ---- CdpMessage field validation ----

#[test]
fn test_cdp_message_zero_id() {
    let raw = r#"{"id":0,"method":"Test.run"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(0));
}

#[test]
fn test_cdp_message_negative_id() {
    let raw = r#"{"id":-999,"method":"Test.run"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(-999));
}

#[test]
fn test_cdp_message_large_id() {
    let raw = r#"{"id":9223372036854775807,"method":"Test.run"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(i64::MAX));
}

#[test]
fn test_cdp_message_method_variants() {
    for method in [
        "Page.enable",
        "Runtime.evaluate",
        "DOM.getDocument",
        "a.b",
        ".",
    ] {
        let raw = format!(r#"{{"id":1,"method":"{}"}}"#, method);
        let msg = parse_message(&raw).unwrap();
        assert_eq!(msg.method, method);
    }
}

#[test]
fn test_cdp_message_empty_method() {
    let raw = r#"{"id":1,"method":""}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.method.is_empty());
}

#[test]
fn test_cdp_message_unicode_method() {
    let raw = r#"{"id":1,"method":"テスト.実行"}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.method.contains("テスト"));
}

#[test]
fn test_cdp_message_session_id_variants() {
    for sid in ["s-1", ""] {
        let raw = format!(r#"{{"id":1,"method":"Test.run","sessionId":"{}"}}"#, sid);
        let msg = parse_message(&raw).unwrap();
        assert_eq!(msg.session_id.unwrap(), sid);
    }
}

#[test]
fn test_cdp_message_session_id_long() {
    let sid = "a".repeat(500);
    let raw = format!(r#"{{"id":1,"method":"Test.run","sessionId":"{}"}}"#, sid);
    let msg = parse_message(&raw).unwrap();
    assert_eq!(msg.session_id.unwrap().len(), 500);
}

#[test]
fn test_cdp_message_null_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":null}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.params.is_none() || msg.params.as_ref().map_or(true, |p| p.is_null()));
}

#[test]
fn test_cdp_message_empty_params_object() {
    let raw = r#"{"id":1,"method":"Test.run","params":{}}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.params.unwrap().as_object().unwrap().is_empty());
}

#[test]
fn test_cdp_message_nested_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"a":{"b":{"c":[1,2,3]}}}}"#;
    let msg = parse_message(raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["a"]["b"]["c"].as_array().unwrap().len(), 3);
}

#[test]
fn test_cdp_message_array_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":[1,"two",true,null]}"#;
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
fn test_cdp_message_extra_fields_ignored() {
    let raw = r#"{"id":1,"method":"Test.run","extra":"ignored","another":42}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(1));
}

// ---- CdpMessage clone/debug ----

#[test]
fn test_cdp_message_clone() {
    let msg = CdpMessage {
        id: Some(1),
        method: "Page.enable".into(),
        params: Some(json!({"key": "val"})),
        session_id: Some("s-1".into()),
    };
    let cloned = msg.clone();
    assert_eq!(cloned.id, msg.id);
    assert_eq!(cloned.method, msg.method);
    assert_eq!(cloned.params, msg.params);
    assert_eq!(cloned.session_id, msg.session_id);
}

#[test]
fn test_cdp_message_debug() {
    let msg = CdpMessage {
        id: Some(42),
        method: "Test.run".into(),
        params: None,
        session_id: None,
    };
    let debug = format!("{:?}", msg);
    assert!(debug.contains("42"));
    assert!(debug.contains("Test.run"));
}

#[test]
fn test_cdp_message_clone_with_all_fields() {
    let msg = CdpMessage {
        id: Some(-1),
        method: "DOM.querySelector".into(),
        params: Some(json!({"nodeId": 1, "selector": "div"})),
        session_id: Some("sess-abc-def".into()),
    };
    let cloned = msg.clone();
    assert_eq!(cloned.params.unwrap()["selector"], "div");
}

// ---- CdpError ----

#[test]
fn test_cdp_error_code_message() {
    let err = CdpError {
        code: -32600,
        message: "invalid".into(),
    };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid");
}

#[test]
fn test_cdp_error_debug() {
    let err = CdpError {
        code: -32700,
        message: "parse".into(),
    };
    assert!(format!("{:?}", err).contains("-32700"));
}

#[test]
fn test_cdp_error_serialize() {
    let err = CdpError {
        code: -32000,
        message: "internal".into(),
    };
    let json_str = serde_json::to_string(&err).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["code"], -32000);
    assert_eq!(parsed["message"], "internal");
}

#[test]
fn test_cdp_error_empty_message() {
    let err = CdpError {
        code: -1,
        message: String::new(),
    };
    let json_str = serde_json::to_string(&err).unwrap();
    assert!(json_str.contains(r#""message":"""#));
}

#[test]
fn test_cdp_error_large_code() {
    let err = CdpError {
        code: i64::MIN,
        message: "min".into(),
    };
    assert_eq!(err.code, i64::MIN);
}

#[test]
fn test_cdp_error_positive_code() {
    let err = CdpError {
        code: 999,
        message: "custom".into(),
    };
    assert_eq!(err.code, 999);
}

// ---- CdpResponse ----

#[test]
fn test_cdp_response_success() {
    let resp = CdpResponse {
        id: Some(1),
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
fn test_cdp_response_error() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError {
            code: -32601,
            message: "not found".into(),
        }),
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("result").is_none());
    assert_eq!(parsed["error"]["code"], -32601);
}

#[test]
fn test_cdp_response_null_fields() {
    let resp = CdpResponse {
        id: Some(3),
        result: None,
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], 3);
}

#[test]
fn test_cdp_response_nested_result() {
    let resp = CdpResponse {
        id: Some(4),
        result: Some(json!({"root": {"nodeId": 1, "children": [{"nodeId": 2}]}})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"]["root"]["children"][0]["nodeId"], 2);
}

#[test]
fn test_cdp_response_array_result() {
    let resp = CdpResponse {
        id: Some(5),
        result: Some(json!([1, "two", true, null])),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["result"].as_array().unwrap().len(), 4);
}

#[test]
fn test_cdp_response_negative_id() {
    let resp = CdpResponse {
        id: Some(-100),
        result: Some(json!({})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], -100);
}

#[test]
fn test_cdp_response_zero_id() {
    let resp = CdpResponse {
        id: Some(0),
        result: Some(json!({})),
        error: None,
    };
    let raw = serialize_response(&resp);
    assert!(serde_json::from_str::<serde_json::Value>(&raw).unwrap()["id"] == 0);
}

#[test]
fn test_cdp_response_deterministic() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"a": 1})),
        error: None,
    };
    assert_eq!(serialize_response(&resp), serialize_response(&resp));
}

#[test]
fn test_cdp_response_empty_result() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["result"].as_object().unwrap().is_empty());
}

#[test]
fn test_cdp_response_large_result() {
    let data: Vec<i32> = (0..500).collect();
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"data": data})),
        error: None,
    };
    let raw = serialize_response(&resp);
    assert!(raw.len() > 1000);
}

// ---- CdpEvent ----

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.load".into(),
        params: Some(json!({"ts": 1})),
    };
    let raw = serialize_event(&ev);
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
    let raw = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("params").is_none());
}

#[test]
fn test_cdp_event_clone() {
    let ev = CdpEvent {
        method: "Test.ev".into(),
        params: Some(json!({"x": 1})),
    };
    let cloned = ev.clone();
    assert_eq!(cloned.method, ev.method);
    assert_eq!(cloned.params, ev.params);
}

#[test]
fn test_cdp_event_debug() {
    let ev = CdpEvent {
        method: "Test.ev".into(),
        params: None,
    };
    assert!(format!("{:?}", ev).contains("Test.ev"));
}

#[test]
fn test_cdp_event_deterministic() {
    let ev = CdpEvent {
        method: "Test.ev".into(),
        params: Some(json!({"x": 1})),
    };
    assert_eq!(serialize_event(&ev), serialize_event(&ev));
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
    // JSON-RPC 2.0 permits notifications (no id). With CdpMessage.id as
    // Option<i64> (TASK-4-CDP), a missing id deserializes to None and the
    // message parses successfully (it is a valid notification).
    let msg = parse_message(r#"{"method":"Test.run"}"#).unwrap();
    assert_eq!(msg.id, None);
    assert_eq!(msg.method, "Test.run");
}

#[test]
fn test_parse_message_missing_method() {
    assert!(parse_message(r#"{"id":1}"#).is_none());
}

#[test]
fn test_parse_message_array_root() {
    assert!(parse_message("[1,2,3]").is_none());
}

#[test]
fn test_parse_message_null_root() {
    assert!(parse_message("null").is_none());
}

#[test]
fn test_parse_message_number_root() {
    assert!(parse_message("42").is_none());
}

#[test]
fn test_parse_message_string_id() {
    assert!(parse_message(r#"{"id":"abc","method":"Test.run"}"#).is_none());
}

#[test]
fn test_parse_message_large_params() {
    let arr: Vec<i32> = (0..1000).collect();
    let raw = format!(
        r#"{{"id":1,"method":"Test.run","params":{}}}"#,
        serde_json::to_string(&arr).unwrap()
    );
    let msg = parse_message(&raw).unwrap();
    assert_eq!(msg.params.unwrap().as_array().unwrap().len(), 1000);
}

#[test]
fn test_serialize_response_unicode_in_error() {
    let resp = CdpResponse {
        id: Some(1),
        result: None,
        error: Some(CdpError {
            code: -32000,
            message: "エラー発生".into(),
        }),
    };
    let raw = serialize_response(&resp);
    assert!(raw.contains("エラー"));
}

#[test]
fn test_parse_message_special_chars_in_params() {
    let raw = r#"{"id":1,"method":"Test.run","params":{"url":"https://example.com/path?q=a%20b&c=d#frag"}}"#;
    let msg = parse_message(raw).unwrap();
    let params = msg.params.unwrap();
    assert!(params["url"].as_str().unwrap().contains("%20"));
}

// ---- Roundtrip: parse → serialize ----

#[test]
fn test_roundtrip_response_serialize() {
    let resp = CdpResponse {
        id: Some(42),
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
    let resp = CdpResponse {
        id: Some(99),
        result: None,
        error: Some(CdpError {
            code: -32601,
            message: "'Foo.bar' wasn't found".into(),
        }),
    };
    let raw = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Foo.bar"));
}

#[test]
fn test_roundtrip_event_serialize() {
    let ev = CdpEvent {
        method: "Network.requestWillBeSent".into(),
        params: Some(json!({"requestId": "r-1", "url": "https://test.com"})),
    };
    let raw = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["params"]["requestId"], "r-1");
}
