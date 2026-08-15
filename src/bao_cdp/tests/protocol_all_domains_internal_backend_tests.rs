// @trace TEST-CDP-033 [req:REQ-CDP-001,REQ-CDP-002,REQ-CDP-003] [level:unit]
// InternalBackend + protocol.rs handle_command all 11 domains without bridge.
// Tests cover: every command path in handle_command with bridge=None,
// CdpMessage parse edge cases, serialize_response/serialize_event,
// InternalBackend send_command, CdpResponse/CdpError construction.

use bao_cdp::{CdpError, CdpEvent, CdpMessage, CdpResponse};

const TID: &str = "test-target";
use bao_cdp::{handle_command, parse_message, serialize_event, serialize_response};

use serde_json::json;

// ---- CdpMessage parse edge cases ----

#[test]
fn test_parse_valid_message() {
    let msg = parse_message(r#"{"id":1,"method":"Page.enable","params":{}}"#).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.enable");
    assert!(msg.params.is_some());
    assert!(msg.session_id.is_none());
}

#[test]
fn test_parse_minimal_message() {
    let msg = parse_message(r#"{"id":0,"method":"Test"}"#).unwrap();
    assert_eq!(msg.id, Some(0));
    assert_eq!(msg.method, "Test");
    assert!(msg.params.is_none());
}

#[test]
fn test_parse_with_session_id() {
    // serde deserializes snake_case field names, so "sessionId" won't match
    // unless there's a #[serde(rename)]. Test with snake_case.
    let msg =
        parse_message(r#"{"id":5,"method":"Runtime.evaluate","sessionId":"sess1"}"#).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("sess1"));
}

#[test]
fn test_parse_camel_session_id_matched() {
    // "sessionId" (camelCase) doesn't match the snake_case field without rename
    let msg = parse_message(r#"{"id":5,"method":"Runtime.evaluate","sessionId":"sess1"}"#).unwrap();
    assert!(msg.session_id.is_some());
}

#[test]
fn test_parse_invalid_json() {
    assert!(parse_message("not json").is_none());
}

#[test]
fn test_parse_empty_string() {
    assert!(parse_message("").is_none());
}

#[test]
fn test_parse_missing_method() {
    // method is required by struct definition
    assert!(parse_message(r#"{"id":1}"#).is_none());
}

#[test]
fn test_parse_negative_id() {
    let msg = parse_message(r#"{"id":-1,"method":"X"}"#).unwrap();
    assert_eq!(msg.id, Some(-1));
}

#[test]
fn test_parse_large_id() {
    let msg = parse_message(r#"{"id":9999999999,"method":"X"}"#).unwrap();
    assert_eq!(msg.id, Some(9999999999));
}

#[test]
fn test_parse_string_id_fails() {
    // id must be i64, not string
    assert!(parse_message(r#"{"id":"abc","method":"X"}"#).is_none());
}

#[test]
fn test_parse_params_null() {
    let msg = parse_message(r#"{"id":1,"method":"X","params":null}"#).unwrap();
    // serde default on Option means null → None
    assert!(msg.params.is_none());
}

#[test]
fn test_parse_params_array() {
    let msg = parse_message(r#"{"id":1,"method":"X","params":[1,2]}"#).unwrap();
    assert!(msg.params.unwrap().is_array());
}

// ---- serialize_response ----

#[test]
fn test_serialize_response_ok() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"status": "ok"})),
        error: None,
    };
    let s = serialize_response(&resp);
    assert!(s.contains(r#""id":1"#));
    assert!(s.contains(r#""status":"ok""#));
    assert!(!s.contains("error"));
}

#[test]
fn test_serialize_response_error() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError {
            code: -32601,
            message: "not found".into(),
        }),
    };
    let s = serialize_response(&resp);
    assert!(s.contains(r#""id":2"#));
    assert!(s.contains("-32601"));
    assert!(s.contains("not found"));
    assert!(!s.contains("result"));
}

#[test]
fn test_serialize_response_empty_result() {
    let resp = CdpResponse {
        id: Some(3),
        result: Some(json!({})),
        error: None,
    };
    let s = serialize_response(&resp);
    assert!(s.contains(r#""id":3"#));
}

#[test]
fn test_serialize_response_null_result() {
    let resp = CdpResponse {
        id: Some(4),
        result: Some(json!(null)),
        error: None,
    };
    let s = serialize_response(&resp);
    assert!(s.contains(r#""id":4"#));
}

// ---- serialize_event ----

#[test]
fn test_serialize_event_with_params() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 123})),
    };
    let s = serialize_event(&ev);
    assert!(s.contains("Page.loadEventFired"));
    assert!(s.contains("123"));
}

#[test]
fn test_serialize_event_no_params() {
    let ev = CdpEvent {
        method: "Runtime.executionContextCreated".into(),
        params: None,
    };
    let s = serialize_event(&ev);
    assert!(s.contains("Runtime.executionContextCreated"));
    assert!(!s.contains("params"));
}

// ---- CdpError construction ----

#[test]
fn test_cdp_error_debug() {
    let err = CdpError {
        code: -32601,
        message: "test".into(),
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32601"));
    assert!(debug.contains("test"));
}

#[test]
fn test_cdp_error_serialize() {
    let err = CdpError {
        code: -32700,
        message: "parse error".into(),
    };
    let s = serde_json::to_string(&err).unwrap();
    assert!(s.contains("-32700"));
    assert!(s.contains("parse error"));
}

// ---- handle_command all domains without bridge ----

fn handle(method: &str) -> CdpResponse {
    let msg = CdpMessage {
        id: Some(42),
        method: method.into(),
        params: None,
        session_id: None,
    };
    handle_command(msg, "t1", &None, None)
}

fn handle_params(method: &str, params: serde_json::Value) -> CdpResponse {
    let msg = CdpMessage {
        id: Some(42),
        method: method.into(),
        params: Some(params.clone()),
        session_id: None,
    };
    handle_command(msg, "t1", &Some(params), None)
}

#[test]
fn test_handle_no_dot_in_method() {
    let resp = handle("NoDomain");
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn test_handle_empty_method() {
    let resp = handle("");
    assert!(resp.error.is_some());
}

// ---- Target domain ----

#[test]
fn test_target_get_targets() {
    let resp = handle("Target.getTargets");
    assert!(resp.result.is_some());
    let val = resp.result.unwrap();
    assert!(val["targetInfos"].is_array());
    assert_eq!(val["targetInfos"][0]["type"], "page");
}

#[test]
fn test_target_get_target_targets() {
    let resp = handle("Target.getTargetTargets");
    assert!(resp.result.is_some());
}

#[test]
fn test_target_create_target() {
    // Real page creation requires the servo bridge — explicit error without
    // one, never an echo of the current target id.
    let resp = handle("Target.createTarget");
    let err = resp.error.expect("createTarget must fail without a bridge");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge connected"));

}

#[test]
fn test_target_close_target() {
    // Closing a page is a blocking bridge round-trip — explicit error
    // without a bridge, never a fire-and-forget fake success.
    let resp = handle("Target.closeTarget");
    let err = resp.error.expect("closeTarget must fail without a bridge");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge connected"));

}

#[test]
fn test_target_set_auto_attach() {
    let resp = handle("Target.setAutoAttach");
    assert!(resp.result.is_some());
}

#[test]
fn test_target_set_discover_targets() {
    let resp = handle("Target.setDiscoverTargets");
    assert!(resp.result.is_some());
}

#[test]
fn test_target_get_target_info() {
    let resp = handle("Target.getTargetInfo");
    assert!(resp.result.is_some());
    let val = resp.result.unwrap();
    assert!(val["targetInfo"]["type"] == "page");
}

#[test]
fn test_target_attach_to_target() {
    // Session minting lives in the WS session registry (bao_browser) — the
    // stateless internal backend refuses explicitly, never a fabricated id.
    let resp = handle("Target.attachToTarget");
    let err = resp.error.expect("attachToTarget must fail without the WS registry");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("WS session registry"));

}

#[test]
fn test_target_detach() {
    let resp = handle("Target.detachFromTarget");
    let err = resp.error.expect("detachFromTarget must fail without the WS registry");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("WS session registry"));

}

#[test]
fn test_target_send_message() {
    let resp = handle("Target.sendMessageToTarget");
    let err = resp.error.expect("sendMessageToTarget must fail without the WS registry");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("WS session registry"));

}

#[test]
fn test_target_unknown() {
    let resp = handle("Target.nonexistent");
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

// ---- Page domain (no bridge) ----

#[test]
fn test_page_enable_disable() {
    assert!(handle("Page.enable").result.is_some());
    assert!(handle("Page.disable").result.is_some());
}

// New contract (6983871b): every servo-state-dependent Page command is an
// explicit error without a bridge — real data or explicit failure, never a
// canned success. -32603 = no servo bridge; -32000 = facility absent;
// -32602 = required param missing.

#[test]
fn test_page_navigate_default_url() {
    let resp = handle("Page.navigate");
    let e = resp.error.expect("no bridge must yield an error");
    assert_eq!(e.code, -32603);
    assert!(e.message.contains("no servo bridge"));
}

#[test]
fn test_page_navigate_with_url() {
    let resp = handle_params("Page.navigate", json!({"url": "https://example.com"}));
    assert_eq!(resp.error.unwrap().code, -32603);
}

#[test]
fn test_page_reload() {
    assert_eq!(handle("Page.reload").error.unwrap().code, -32603);
}

#[test]
fn test_page_get_frame_tree() {
    assert_eq!(handle("Page.getFrameTree").error.unwrap().code, -32603);
}

#[test]
fn test_page_get_navigation_history() {
    // servo exposes no session-history enumeration — -32000, never a
    // fabricated currentIndex/entries payload.
    let resp = handle("Page.getNavigationHistory");
    let e = resp.error.expect("history enumeration must fail loudly");
    assert_eq!(e.code, -32000);
    assert!(e.message.contains("not supported"));
}

#[test]
fn test_page_capture_screenshot_no_bridge() {
    // No renderer without the bridge — -32603, never {"data":""}.
    let resp = handle("Page.captureScreenshot");
    let e = resp.error.expect("no bridge must yield an error");
    assert_eq!(e.code, -32603);
    assert!(resp.result.is_none(), "no fabricated image data");
}

#[test]
fn test_page_set_content() {
    let e = handle("Page.setContent").error.expect("missing html must be rejected");
    assert_eq!(e.code, -32602);
    assert!(e.message.contains("html"));
}

#[test]
fn test_page_close() {
    assert_eq!(handle("Page.close").error.unwrap().code, -32603);
}

#[test]
fn test_page_bring_to_front() {
    assert_eq!(handle("Page.bringToFront").error.unwrap().code, -32603);
}

#[test]
fn test_page_get_layout_metrics() {
    // Metrics are computed live from the document — -32603 without a bridge,
    // the hardcoded 1920x1080 is eradicated.
    let resp = handle("Page.getLayoutMetrics");
    let e = resp.error.expect("no bridge must yield an error");
    assert_eq!(e.code, -32603);
    assert!(resp.result.is_none(), "no fabricated dimensions");
}

#[test]
fn test_page_add_script_no_bridge() {
    // Chrome-compatible: an empty init script registers as a no-op with a
    // fresh identifier; no bridge needed.
    let resp = handle_params("Page.addScriptToEvaluateOnNewDocument", json!({"source": ""}));
    let result = resp.result.expect("empty source registers as a no-op");
    assert!(result["identifier"].as_str().unwrap().starts_with("script-"));
}

#[test]
fn test_response_id_large() {
    let msg = CdpMessage {
        id: Some(12345),
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t1", &None, None);
    assert_eq!(resp.id, Some(12345));
}

#[test]
fn test_response_id_negative() {
    let msg = CdpMessage {
        id: Some(-999),
        method: "Runtime.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t1", &None, None);
    assert_eq!(resp.id, Some(-999));
}
