//! Input domain conformance 审计 — 4 method。
//!
//! 对照 CDP 官方规范(Input domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Input/
//!
//! # 覆盖 method
//!
//! dispatchMouseEvent, dispatchKeyEvent, dispatchTouchEvent, setIgnoreInputEvents
//!
//! @trace REQ-CDP-001 [domain:Input] [level:integration]
//! @trace REQ-BAO-API-004 [domain:Input] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Input.dispatchMouseEvent — CDP spec: empty return {}
// params: {type: "mousePressed"|"mouseReleased"|"mouseMoved"|"mouseWheel", x, y, ...}
// https://chromedevtools.github.io/devtools-protocol/tot/Input/#method-dispatchMouseEvent
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn input_dispatch_mouse_event_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Input] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Input.dispatchMouseEvent",
        json!({"type":"mousePressed", "x":10, "y":20, "button":"left", "clickCount":1}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: dispatchMouseEvent returns empty object"
    );
}

#[test]
fn input_dispatch_mouse_event_missing_type_returns_32602() {
    // Arrange — CDP 规范: type 必填
    // @trace REQ-BAO-API-007 [domain:Input] [level:integration]
    let b = backend();
    let err = dispatch_command(
        &*b,
        "Input.dispatchMouseEvent",
        json!({"x":10, "y":20}),
        "1",
    )
    .unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn input_dispatch_mouse_event_all_types_accepted() {
    // Arrange — CDP 规范: type ∈ {mousePressed, mouseReleased, mouseMoved, mouseWheel}
    // @trace REQ-CDP-001 [domain:Input] [level:integration]
    let b = backend();
    for t in ["mousePressed", "mouseReleased", "mouseMoved", "mouseWheel"] {
        let result = dispatch_command(
            &*b,
            "Input.dispatchMouseEvent",
            json!({"type":t, "x":0, "y":0}),
            "1",
        )
        .unwrap();
        assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Input.dispatchKeyEvent — CDP spec: empty return {}
// params: {type: "keyDown"|"keyUp"|"rawKeyDown"|"char", ...}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn input_dispatch_key_event_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Input] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown", "key":"a", "code":"KeyA"}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: dispatchKeyEvent returns empty object"
    );
}

#[test]
fn input_dispatch_key_event_missing_type_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Input] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Input.dispatchKeyEvent", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn input_dispatch_key_event_all_types_accepted() {
    // Arrange — CDP 规范: type ∈ {keyDown, keyUp, rawKeyDown, char}
    // @trace REQ-CDP-001 [domain:Input] [level:integration]
    let b = backend();
    for t in ["keyDown", "keyUp", "rawKeyDown", "char"] {
        let result =
            dispatch_command(&*b, "Input.dispatchKeyEvent", json!({"type":t}), "1").unwrap();
        assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Input.dispatchTouchEvent — CDP spec: empty return {}
// params: {type: "touchStart"|"touchEnd"|"touchMove"|"touchCancel", touchPoints: [TouchPoint]}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn input_dispatch_touch_event_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Input] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Input.dispatchTouchEvent",
        json!({
            "type":"touchStart",
            "touchPoints":[{"x":10, "y":20}]
        }),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: dispatchTouchEvent returns empty object"
    );
}

#[test]
fn input_dispatch_touch_event_missing_type_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Input] [level:integration]
    let b = backend();
    let err = dispatch_command(
        &*b,
        "Input.dispatchTouchEvent",
        json!({"touchPoints":[]}),
        "1",
    )
    .unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn input_dispatch_touch_event_missing_touch_points_returns_32602() {
    // Arrange — CDP 规范: touchPoints 必填数组
    // @trace REQ-BAO-API-007 [domain:Input] [level:integration]
    let b = backend();
    let err = dispatch_command(
        &*b,
        "Input.dispatchTouchEvent",
        json!({"type":"touchStart"}),
        "1",
    )
    .unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// Input.setIgnoreInputEvents — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn input_set_ignore_input_events_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Input] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Input.setIgnoreInputEvents",
        json!({"ignore":true}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setIgnoreInputEvents returns empty object"
    );
}

#[test]
fn input_set_ignore_input_events_default_param_accepted() {
    // @trace REQ-CDP-001 [domain:Input] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Input.setIgnoreInputEvents", json!({}), "1").unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}
