//! Debugger domain SM API 接入集成测试 — BUG-CDP-006。
//!
//! 验证 9 个 Debugger method 全部从 E 类(-32601)迁移到真实 servo SM Debugger
//! API 接入(DevtoolScriptControlMsg)。每个 method 一个测试,聚焦 CDP command →
//! MockServoBackend 调用记录的映射(servo 端语义验证)。
//!
//! # 覆盖矩阵
//!
//! | CDP method | servo DevtoolScriptControlMsg | backend 调用断言 |
//! |------------|-------------------------------|------------------|
//! | Debugger.enable                  | WantsLiveNotifications(true) | debugger_enable |
//! | Debugger.disable                 | WantsLiveNotifications(false) | debugger_disable |
//! | Debugger.setBreakpoint           | SetBreakpoint | debugger_set_breakpoint_by_url |
//! | Debugger.setBreakpointByUrl      | SetBreakpoint | debugger_set_breakpoint_by_url |
//! | Debugger.removeBreakpoint        | ClearBreakpoint | debugger_remove_breakpoint |
//! | Debugger.pause                   | Interrupt | debugger_pause |
//! | Debugger.resume                  | Resume(None, _) | debugger_resume |
//! | Debugger.stepOver                | Resume(Some(next), _) | debugger_resume |
//! | Debugger.stepInto                | Resume(Some(step), _) | debugger_resume |
//! | Debugger.stepOut                 | Resume(Some(finish), _) | debugger_resume |
//! | Debugger.evaluateOnCallFrame     | Eval(code, pipeline, _, frame, reply) | debugger_evaluate_on_call_frame |
//! | Debugger.getPossibleBreakpoints  | GetPossibleBreakpoints | debugger_get_possible_breakpoints |
//! | Debugger.getScriptSource         | SourceInfo actor | debugger_get_script_source |
//! | Debugger.setBreakpointsActive    | (no-op, validate) | (params 校验) |
//!
//! @trace REQ-CDP-003 [domain:Debugger] [level:integration]
//! @trace BUG-CDP-006 [domain:Debugger] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend};
use serde_json::json;

// ─────────────────────────────────────────────────────────────────────────
// 9 method — 全部不再 E 类,真实路由到 backend
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn sm_api_enable_records_wants_live_notifications() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C1]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    let r = dispatch_command(&b, "Debugger.enable", json!({}), "1").unwrap();
    assert_eq!(r["debuggerId"].as_i64(), Some(1));
    let log = b.call_log.lock().unwrap();
    assert!(log.iter().any(|(_, m, _)| m == "debugger_enable"));
}

#[test]
fn sm_api_disable_records_disable() {
    // @trace REQ-CDP-003 [domain:Debugger]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.disable", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log.iter().any(|(_, m, _)| m == "debugger_disable"));
}

#[test]
fn sm_api_set_breakpoint_by_url_returns_breakpoint_id() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C2]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    let r = dispatch_command(
        &b,
        "Debugger.setBreakpointByUrl",
        json!({"url":"app.js","lineNumber":7,"columnNumber":3}),
        "1",
    )
    .unwrap();
    assert!(r["breakpointId"].is_string());
    assert!(r["breakpointId"].as_str().unwrap().contains("7"));
    let locs = r["locations"].as_array().unwrap();
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0]["lineNumber"].as_i64(), Some(7));
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_set_breakpoint_by_url" && p.contains("app.js")));
}

#[test]
fn sm_api_set_breakpoint_with_location_routes() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C2]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    let r = dispatch_command(
        &b,
        "Debugger.setBreakpoint",
        json!({"location":{"scriptId":"42","lineNumber":3}}),
        "1",
    )
    .unwrap();
    assert!(r["breakpointId"].is_string());
    assert_eq!(r["actualLocation"]["scriptId"].as_str(), Some("42"));
}

#[test]
fn sm_api_remove_breakpoint_routes_to_clear() {
    // @trace REQ-CDP-003 [domain:Debugger]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(
        &b,
        "Debugger.removeBreakpoint",
        json!({"breakpointId":"bp:1:10:5"}),
        "1",
    )
    .unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_remove_breakpoint" && p.contains("1:10:5")));
}

#[test]
fn sm_api_pause_routes_to_interrupt() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C3]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.pause", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_pause" && p == "Interrupt"));
}

#[test]
fn sm_api_resume_routes_to_resume_no_limit() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C3]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.resume", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_resume" && p == "Resume()"));
}

#[test]
fn sm_api_step_over_routes_to_resume_next() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.stepOver", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(next)"));
}

#[test]
fn sm_api_step_into_routes_to_resume_step() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.stepInto", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(step)"));
}

#[test]
fn sm_api_step_out_routes_to_resume_finish() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    dispatch_command(&b, "Debugger.stepOut", json!({}), "1").unwrap();
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(finish)"));
}

#[test]
fn sm_api_evaluate_on_call_frame_routes_to_eval() {
    // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C5]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    let r = dispatch_command(
        &b,
        "Debugger.evaluateOnCallFrame",
        json!({"callFrameId":"frame-0","expression":"x + 1"}),
        "1",
    )
    .unwrap();
    assert_eq!(r["result"]["type"].as_str(), Some("string"));
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, p)| m == "debugger_evaluate_on_call_frame" && p.contains("frame-0")));
}

#[test]
fn sm_api_get_possible_breakpoints_routes_to_backend() {
    // @trace REQ-CDP-003 [domain:Debugger]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    let r = dispatch_command(
        &b,
        "Debugger.getPossibleBreakpoints",
        json!({"start":{"scriptId":"1","lineNumber":0}}),
        "1",
    )
    .unwrap();
    let arr = r["locations"].as_array().unwrap();
    assert!(!arr.is_empty());
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, _)| m == "debugger_get_possible_breakpoints"));
}

#[test]
fn sm_api_get_script_source_routes_to_backend() {
    // @trace REQ-CDP-003 [domain:Debugger]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    let r = dispatch_command(
        &b,
        "Debugger.getScriptSource",
        json!({"scriptId":"1"}),
        "1",
    )
    .unwrap();
    assert!(r["scriptSource"].is_string());
    let log = b.call_log.lock().unwrap();
    assert!(log
        .iter()
        .any(|(_, m, _)| m == "debugger_get_script_source"));
}

#[test]
fn sm_api_set_breakpoints_active_validates_bool_param() {
    // @trace REQ-CDP-003 [domain:Debugger]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    let r = dispatch_command(
        &b,
        "Debugger.setBreakpointsActive",
        json!({"active":true}),
        "1",
    )
    .unwrap();
    assert!(r.as_object().unwrap().is_empty());
    // 缺 active 参数 → InvalidParams(不是 NotSupported,确认已路由)
    let err = dispatch_command(&b, "Debugger.setBreakpointsActive", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn sm_api_unknown_target_returns_page_not_found() {
    // 验证 Debugger method 同样走 ensure_target 校验
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    let err = dispatch_command(&b, "Debugger.enable", json!({}), "999").unwrap_err();
    assert!(matches!(err, BridgeError::PageNotFound(_)));
}

#[test]
fn sm_api_no_debugger_method_remains_e_class() {
    // 回归断言:所有 9 method 已脱离 E 类,无任何 Debugger.* 返回 -32601。
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    let b = MockServoBackend::new();
    for method in [
        "Debugger.enable",
        "Debugger.disable",
        "Debugger.setBreakpoint",
        "Debugger.setBreakpointByUrl",
        "Debugger.removeBreakpoint",
        "Debugger.pause",
        "Debugger.resume",
        "Debugger.stepOver",
        "Debugger.stepInto",
        "Debugger.stepOut",
        "Debugger.evaluateOnCallFrame",
        "Debugger.getPossibleBreakpoints",
        "Debugger.getScriptSource",
        "Debugger.setBreakpointsActive",
    ] {
        let result = dispatch_command(&b, method, json!({}), "1");
        assert!(
            !matches!(result, Err(BridgeError::NotSupported(_))),
            "{method} should NOT be E-class after BUG-CDP-006"
        );
    }
}
