//! Debugger domain conformance 审计 — E 类(servo Internal 模式不支持)+ Debugger.scriptParsed 事件。
//!
//! 对照 CDP 官方规范(Debugger domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Debugger/
//!
//! # 覆盖
//!
//! - E 类 method(全部 -32601):setBreakpoint, setBreakpointByUrl, removeBreakpoint,
//!   pause, resume, stepOver, stepInto, stepOut, evaluateOnCallFrame
//! - Debugger.scriptParsed 事件(servo SourceInfo 翻译)
//!
//! @trace REQ-CDP-001 [domain:Debugger] [level:integration]
//! @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]

use bao_cdp_client::{dispatch_command, translate_event, BridgeError, MockServoBackend, ServoBackend, ServoEvent};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Debugger E 类 method — servo Internal 模式不支持,全部返回 -32601
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn debugger_set_breakpoint_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err =
        dispatch_command(&*b, "Debugger.setBreakpoint", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn debugger_set_breakpoint_by_url_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err =
        dispatch_command(&*b, "Debugger.setBreakpointByUrl", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn debugger_remove_breakpoint_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err =
        dispatch_command(&*b, "Debugger.removeBreakpoint", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn debugger_pause_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Debugger.pause", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn debugger_resume_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Debugger.resume", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn debugger_step_over_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Debugger.stepOver", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn debugger_step_into_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Debugger.stepInto", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn debugger_step_out_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Debugger.stepOut", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn debugger_evaluate_on_call_frame_e_class_returns_32601() {
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    let b = backend();
    let err = dispatch_command(
        &*b,
        "Debugger.evaluateOnCallFrame",
        json!({}),
        "1",
    )
    .unwrap_err();
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

// ─────────────────────────────────────────────────────────────────────────
// Debugger.scriptParsed 事件 — CDP spec:
// {scriptId, url, startLine, startColumn, endLine, endColumn, ...}
// https://chromedevtools.github.io/devtools-protocol/tot/Debugger/#event-scriptParsed
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn debugger_script_parsed_event_schema_conformance() {
    // Arrange — servo SourceInfo 事件 → Debugger.scriptParsed
    // @trace REQ-CDP-001 [domain:Debugger] [level:integration]
    // @trace REQ-BAO-API-003 [event:Debugger.scriptParsed] [level:integration]
    let servo_event = ServoEvent::ScriptParsed {
        target_id: "1".to_string(),
        script_id: "42".to_string(),
        url: "https://x/page.js".to_string(),
        start_line: 0,
        start_column: 0,
        end_line: 100,
        end_column: 0,
        source_map_url: None,
    };

    // Act
    let events = translate_event(servo_event);

    // Assert — exactly one CdpEvent
    assert_eq!(events.len(), 1, "ScriptParsed → exactly 1 Debugger.scriptParsed");
    let ev = &events[0];

    // CDP spec: method = "Debugger.scriptParsed"
    assert_eq!(ev.method, "Debugger.scriptParsed");

    // CDP spec: required fields
    assert!(
        ev.params["scriptId"].is_string(),
        "CDP spec: scriptId must be string, got: {:?}",
        ev.params["scriptId"]
    );
    assert!(ev.params["url"].is_string(), "CDP spec: url must be string");
    assert!(
        ev.params["startLine"].is_i64() || ev.params["startLine"].is_u64(),
        "CDP spec: startLine must be integer"
    );
    assert!(
        ev.params["startColumn"].is_i64() || ev.params["startColumn"].is_u64(),
        "CDP spec: startColumn must be integer"
    );
    assert!(
        ev.params["endLine"].is_i64() || ev.params["endLine"].is_u64(),
        "CDP spec: endLine must be integer"
    );
    assert!(
        ev.params["endColumn"].is_i64() || ev.params["endColumn"].is_u64(),
        "CDP spec: endColumn must be integer"
    );
}

#[test]
fn debugger_script_parsed_optional_source_map_url() {
    // Arrange — CDP 规范: sourceMapURL 可选
    // @trace REQ-CDP-001 [domain:Debugger] [level:integration]
    let servo_event = ServoEvent::ScriptParsed {
        target_id: "1".into(),
        script_id: "1".into(),
        url: "https://x.js".into(),
        start_line: 0,
        start_column: 0,
        end_line: 0,
        end_column: 0,
        source_map_url: Some("https://x.js.map".into()),
    };
    let events = translate_event(servo_event);

    // Assert
    assert_eq!(
        events[0].params["sourceMapURL"].as_str(),
        Some("https://x.js.map"),
        "CDP spec: sourceMapURL when present must be string"
    );
}

#[test]
fn debugger_script_parsed_carries_session_id() {
    // @trace REQ-BAO-API-003 [event:Debugger.scriptParsed] [level:integration]
    let servo_event = ServoEvent::ScriptParsed {
        target_id: "page-7".into(),
        script_id: "1".into(),
        url: "x".into(),
        start_line: 0,
        start_column: 0,
        end_line: 0,
        end_column: 0,
        source_map_url: None,
    };
    let events = translate_event(servo_event);
    assert_eq!(
        events[0].session_id.as_deref(),
        Some("page-7"),
        "bao convention: target_id as sessionId"
    );
}
