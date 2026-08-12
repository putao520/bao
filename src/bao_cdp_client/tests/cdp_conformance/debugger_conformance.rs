//! Debugger domain conformance 审计 — BUG-CDP-006 接入 servo SM Debugger API + scriptParsed 事件。
//!
//! 对照 CDP 官方规范(Debugger domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Debugger/
//!
//! # 覆盖
//!
//! BUG-CDP-006: Debugger 9 method 已接入 servo SM Debugger API,不再返回 -32601。
//! 本文件验证:
//! - 9 method 全部成功路由(不再 NotSupported)
//! - 返回 CDP-compliant JSON 响应结构(breakpointId / locations / RemoteObject)
//! - Debugger.scriptParsed 事件(servo SourceInfo 翻译)
//!
//! @trace REQ-CDP-001 [domain:Debugger] [level:integration]
//! @trace REQ-CDP-003 [domain:Debugger] [level:integration]
//! @trace BUG-CDP-006 [domain:Debugger] [level:integration]

use bao_cdp_client::{
    dispatch_command, translate_event, BridgeError, MockServoBackend, ServoBackend, ServoEvent,
};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Debugger 9 method — BUG-CDP-006 已接入 servo SM Debugger API
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn debugger_set_breakpoint_routes_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Debugger.setBreakpoint",
        json!({"location":{"scriptId":"1","lineNumber":0}}),
        "1",
    );
    assert!(
        !matches!(result, Err(BridgeError::NotSupported(_))),
        "setBreakpoint 不应再返回 NotSupported (BUG-CDP-006)"
    );
}

#[test]
fn debugger_set_breakpoint_by_url_returns_breakpoint_id() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(
        &*b,
        "Debugger.setBreakpointByUrl",
        json!({"url":"x.js","lineNumber":5,"columnNumber":0}),
        "1",
    )
    .unwrap();
    // CDP spec: { breakpointId, locations: [{scriptId, lineNumber, columnNumber}] }
    assert!(r["breakpointId"].is_string(), "breakpointId must be string");
    assert!(
        r["breakpointId"].as_str().unwrap().contains('5'),
        "breakpointId should encode line"
    );
    let locs = r["locations"].as_array().expect("locations must be array");
    assert_eq!(locs.len(), 1);
    assert!(locs[0]["scriptId"].is_string());
}

#[test]
fn debugger_remove_breakpoint_routes_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(
        &*b,
        "Debugger.removeBreakpoint",
        json!({"breakpointId":"bp:1:10:5"}),
        "1",
    )
    .unwrap();
    assert!(r.as_object().unwrap().is_empty());
}

#[test]
fn debugger_pause_returns_empty_object() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(&*b, "Debugger.pause", json!({}), "1").unwrap();
    // CDP spec: Debugger.pause 响应为空对象,异步事件 Debugger.paused 携带 callFrames
    assert!(r.as_object().unwrap().is_empty());
}

#[test]
fn debugger_resume_returns_empty_object() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(&*b, "Debugger.resume", json!({}), "1").unwrap();
    assert!(r.as_object().unwrap().is_empty());
}

#[test]
fn debugger_step_over_routes_to_resume_next() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.stepOver", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(next)"));
}

#[test]
fn debugger_step_into_routes_to_resume_step() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.stepInto", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(step)"));
}

#[test]
fn debugger_step_out_routes_to_resume_finish() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.stepOut", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(finish)"));
}

#[test]
fn debugger_evaluate_on_call_frame_returns_remote_object() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(
        &*b,
        "Debugger.evaluateOnCallFrame",
        json!({"callFrameId":"frame-0","expression":"1+1"}),
        "1",
    )
    .unwrap();
    // CDP spec: { result: RemoteObject, exceptionDetails?: ExceptionDetails }
    assert!(
        r["result"]["type"].is_string(),
        "result.type must be string"
    );
}

#[test]
fn debugger_enable_returns_debugger_id() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(&*b, "Debugger.enable", json!({}), "1").unwrap();
    assert!(r["debuggerId"].is_number());
}

#[test]
fn debugger_get_possible_breakpoints_returns_locations() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(
        &*b,
        "Debugger.getPossibleBreakpoints",
        json!({"start":{"scriptId":"1","lineNumber":0}}),
        "1",
    )
    .unwrap();
    let arr = r["locations"].as_array().expect("locations must be array");
    assert!(!arr.is_empty());
    assert!(arr[0]["scriptId"].is_string());
}

#[test]
fn debugger_get_script_source_returns_string() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(
        &*b,
        "Debugger.getScriptSource",
        json!({"scriptId":"1"}),
        "1",
    )
    .unwrap();
    assert!(r["scriptSource"].is_string());
}

#[test]
fn debugger_set_breakpoints_active_validates_bool() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = backend();
    let r = dispatch_command(
        &*b,
        "Debugger.setBreakpointsActive",
        json!({"active":false}),
        "1",
    )
    .unwrap();
    assert!(r.as_object().unwrap().is_empty());
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
    assert_eq!(
        events.len(),
        1,
        "ScriptParsed → exactly 1 Debugger.scriptParsed"
    );
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
