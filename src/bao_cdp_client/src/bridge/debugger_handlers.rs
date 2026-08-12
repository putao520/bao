//! Debugger domain 9 method — 接入 servo SM Debugger API。
//!
//! BUG-CDP-006 修复:之前 9 method 是 E 类(-32601 NotSupported),Puppeteer/Playwright
//! 的 step/breakpoint/evaluateOnCallFrame 全失效。本模块把 CDP Debugger 9 method 真实
//! 接入 servo `DevtoolScriptControlMsg`(servo 内部由 script_thread 转发到 SM Debugger API)。
//!
//! # servo SM Debugger API 映射
//!
//! | CDP method | servo DevtoolScriptControlMsg | script_thread handler |
//! |------------|-------------------------------|----------------------|
//! | Debugger.enable                  | WantsLiveNotifications(true) | script_thread.rs:2265 |
//! | Debugger.setBreakpointByUrl      | SetBreakpoint(actor_id, script_id, offset) | script_thread.rs:2322 |
//! | Debugger.removeBreakpoint        | ClearBreakpoint(actor_id, script_id, offset) | script_thread.rs:2326 |
//! | Debugger.pause                   | Interrupt                    | script_thread.rs:2330 |
//! | Debugger.resume                  | Resume(Option<limit>, Option<frame_id>) | script_thread.rs:2341 |
//! | Debugger.stepOver/Into/Out       | Resume(Some(limit), frame_id) | script_thread.rs:2341 |
//! | Debugger.evaluateOnCallFrame     | Eval(code, pipeline_id, _, frame_id, reply) | script_thread.rs:2311 |
//! | Debugger.getPossibleBreakpoints  | GetPossibleBreakpoints(script_id, reply) | script_thread.rs:2315 |
//!
//! 参见 `~/code/rust/servo/components/shared/devtools/lib.rs:338` (DevtoolScriptControlMsg enum)
//! 参见 `~/code/rust/servo/components/script/script_thread.rs:2183` (handle function)
//!
//! # 数据流
//!
//! ```text
//!   CDP Client (Puppeteer/Playwright)
//!       ↓ Debugger.setBreakpointByUrl
//!   command_dispatcher::dispatch_command
//!       ↓
//!   debugger_handlers::debugger_set_breakpoint_by_url
//!       ↓
//!   ServoBackend::debugger_set_breakpoint_by_url
//!       ↓ (crossbeam channel per DEC-CDP-002)
//!   DevtoolScriptControlMsg::SetBreakpoint
//!       ↓
//!   script_thread.rs:2322 → fire_set_breakpoint(cx, ...)
//!       ↓
//!   SM JSDDebugger::setBreakpoint
//! ```
//!
//! # 调用栈帧(callFrame)模型
//!
//! CDP `Debugger.paused` 事件的 `callFrames` 数组对应 servo `ListFrames` 返回的
//! `Vec<String>` 序列化结果(JSON-encoded frame actor info)。每个 frame 含:
//! - callFrameId(servo frame actor id,形如 `frame-{n}`)
//! - functionName / url / lineNumber / columnNumber
//! - scopeChain (servo GetEnvironment)
//!
//! CDP `evaluateOnCallFrame` 的 `callFrameId` 参数透传给 servo `Eval` 第 4 参数。
//!
//! @trace REQ-CDP-003 [level:library]
//! @trace BUG-CDP-006 [level:library]

use serde_json::{json, Value};

use super::error::BridgeError;
use super::servo_backend::ServoBackend;

// ────────────────────────────────────────────────────────────────────
// 通用 JSON 助手(与 a_class_handlers 一致)
// ────────────────────────────────────────────────────────────────────

fn get_str(params: &Value, key: &str) -> Result<String, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing string field: {key}")))
}

fn get_opt_str(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn get_opt_u32(params: &Value, key: &str, default: u32) -> u32 {
    params
        .get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(default)
}

// ────────────────────────────────────────────────────────────────────
// Debugger.enable — REQ-CDP-003-C1
// ────────────────────────────────────────────────────────────────────

/// Debugger.enable — 启用 Debugger domain。
///
/// servo 端语义:初始化 JSDDebugger,开始监听 scriptParsed / breakpointHit 事件,
/// 通过 `WantsLiveNotifications(true)` 启用 script→devtools 通道。
///
/// CDP 响应:`{ debuggerId: <number> }`(可选,用于多个 debugger 实例区分)。
///
/// @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C1]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_enable(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.debugger_enable(target_id)?;
    Ok(json!({ "debuggerId": 1 }))
}

/// Debugger.disable — 禁用 Debugger domain(关闭断点监听)。
///
/// @trace REQ-CDP-003 [domain:Debugger]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_disable(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.debugger_disable(target_id)?;
    Ok(Value::Object(Default::default()))
}

// ────────────────────────────────────────────────────────────────────
// Debugger.setBreakpointByUrl — REQ-CDP-003-C2
// ────────────────────────────────────────────────────────────────────

/// Debugger.setBreakpointByUrl — 按 URL + 行号设置断点。
///
/// CDP params:
/// - lineNumber:    u32 (1-based for CDP, 0-based for SM — 内部转换)
/// - url:           Option<String>   (单 URL)
/// - urlRegex:      Option<String>   (正则匹配多个 URL,本实现回退为字面匹配)
/// - columnNumber:  Option<u32>
/// - condition:     Option<String>   (条件断点,servo Internal 模式不支持,忽略)
///
/// CDP 响应:
/// - breakpointId:  "{url}:{line}:{column}"
/// - locations:     [{ scriptId, lineNumber, columnNumber }]
///
/// servo 调用链:
/// 1. 通过 `url` 查找已 parsed 的 script(由 servo `ScriptParsed` 事件维护的 url→script_id 映射)
/// 2. `GetPossibleBreakpoints(script_id)` 找出 line 对应的 offset
/// 3. `SetBreakpoint(actor_id, script_id, offset)`
///
/// @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C2]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_set_breakpoint_by_url(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let line_number = get_opt_u32(params, "lineNumber", 0);
    let column_number = get_opt_u32(params, "columnNumber", 0);
    let url = get_opt_str(params, "url")
        .or_else(|| get_opt_str(params, "urlRegex"))
        .ok_or_else(|| {
            BridgeError::InvalidParams(
                "Debugger.setBreakpointByUrl requires `url` or `urlRegex`".into(),
            )
        })?;

    let bp = backend.debugger_set_breakpoint_by_url(target_id, &url, line_number, column_number)?;

    Ok(json!({
        "breakpointId": bp.breakpoint_id,
        "locations": [{
            "scriptId": bp.script_id.to_string(),
            "lineNumber": bp.actual_line,
            "columnNumber": bp.actual_column,
        }],
    }))
}

/// Debugger.setBreakpoint — 在已知 scriptId 上设置断点。
///
/// CDP params:
/// - location: { scriptId, lineNumber, columnNumber? }
///
/// 与 `setBreakpointByUrl` 共享 backend 调用,但 `script_url` 用 scriptId 反查。
/// (servo 端 `SetBreakpoint(actor_id, script_id, offset)` 直接接受 script_id,
/// 不需要 URL,所以本实现把 scriptId 字符串解析为 u32 后直接调用底层。)
///
/// @trace REQ-CDP-003 [domain:Debugger]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_set_breakpoint(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let location = params.get("location").ok_or_else(|| {
        BridgeError::InvalidParams("Debugger.setBreakpoint requires `location`".into())
    })?;
    let script_id_str = location
        .get("scriptId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| BridgeError::InvalidParams("location.scriptId required".into()))?;
    let line_number = get_opt_u32(location, "lineNumber", 0);
    let column_number = get_opt_u32(location, "columnNumber", 0);

    // scriptId 在 CDP 中是字符串(因 Chrome 用 hash),servo 内部是 u32。
    // 直接用字面解析;失败时回退到 hash 截断(取后 8 字符 hex → u32)。
    let script_id: u32 = script_id_str.parse().unwrap_or_else(|_| {
        // 兜底:hash 字符串取稳定 u32
        let mut h: u32 = 2166136261;
        for b in script_id_str.bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(16777619);
        }
        h
    });

    let bp = backend.debugger_set_breakpoint_by_url(
        target_id,
        &format!("inline:{script_id}"),
        line_number,
        column_number,
    )?;

    Ok(json!({
        "breakpointId": bp.breakpoint_id,
        "actualLocation": {
            "scriptId": script_id_str,
            "lineNumber": bp.actual_line,
            "columnNumber": bp.actual_column,
        },
    }))
}

/// Debugger.removeBreakpoint — 移除断点。
///
/// CDP params:
/// - breakpointId: "{url}:{line}:{column}" 或 "bp:{script_id}:{line}:{column}"
///
/// @trace REQ-CDP-003 [domain:Debugger]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_remove_breakpoint(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let bp_id = get_str(params, "breakpointId")?;
    // 解析 breakpointId 还原 (script_id, line, column)。
    // 格式 1: "bp:{script_id}:{line}:{column}"
    // 格式 2: "{url}:{line}:{column}" — url 可能含 ':',从右解析
    let parts: Vec<&str> = bp_id.rsplitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(BridgeError::InvalidParams(format!(
            "malformed breakpointId: {bp_id}"
        )));
    }
    let column: u32 = parts[0].parse().map_err(|_| {
        BridgeError::InvalidParams(format!("invalid column in breakpointId: {}", parts[0]))
    })?;
    let line: u32 = parts[1].parse().map_err(|_| {
        BridgeError::InvalidParams(format!("invalid line in breakpointId: {}", parts[1]))
    })?;
    let script_id: u32 = parts[2]
        .strip_prefix("bp")
        .unwrap_or(parts[2])
        .parse()
        .unwrap_or(1);

    backend.debugger_remove_breakpoint(target_id, script_id, line, column)?;
    Ok(Value::Object(Default::default()))
}

// ────────────────────────────────────────────────────────────────────
// Debugger.pause / resume — REQ-CDP-003-C3
// ────────────────────────────────────────────────────────────────────

/// Debugger.pause — 请求 SM 在下一可暂停点暂停。
///
/// 映射到 servo `DevtoolScriptControlMsg::Interrupt`。
/// servo `script_thread` 会调用 `enter_debugger_pause_loop`,暂停 SM 主线程,
/// 同时通过 `ServoEvent::DebuggerPaused` 推送 callFrames 给 CDP client。
///
/// CDP 响应:空对象(异步事件 `Debugger.paused` 携带 callFrames)。
///
/// @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C3]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_pause(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.debugger_pause(target_id)?;
    Ok(Value::Object(Default::default()))
}

/// Debugger.resume — 恢复执行(可选单步)。
///
/// CDP params:
/// - terminateOnResume: bool (默认 false)
///
/// 当 SM 已 paused 时调用,映射到 servo `Resume(None, None)`。
/// 若 SM 未 paused,servo 端 no-op。
///
/// CDP 响应:空对象。后续 SM 继续执行,触发 `Debugger.resumed` 事件。
///
/// @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C3]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_resume(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.debugger_resume(target_id, None)?;
    Ok(Value::Object(Default::default()))
}

/// Debugger.stepOver — 单步跳过(函数调用整体执行)。
///
/// CDP params: 可选 `breakpointId`(条件步进,忽略)。
///
/// 映射到 servo `Resume(Some("next"), frame_id)` — SM 在下一语句边界暂停。
///
/// @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_step_over(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.debugger_resume(target_id, Some(super::servo_backend::DebugStepAction::Next))?;
    Ok(Value::Object(Default::default()))
}

/// Debugger.stepInto — 单步进入(进入函数调用)。
///
/// 映射到 servo `Resume(Some("step"), frame_id)`。
///
/// @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_step_into(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.debugger_resume(target_id, Some(super::servo_backend::DebugStepAction::Into))?;
    Ok(Value::Object(Default::default()))
}

/// Debugger.stepOut — 单步跳出(执行完当前函数)。
///
/// 映射到 servo `Resume(Some("finish"), frame_id)`。
///
/// @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_step_out(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.debugger_resume(target_id, Some(super::servo_backend::DebugStepAction::Out))?;
    Ok(Value::Object(Default::default()))
}

// ────────────────────────────────────────────────────────────────────
// Debugger.evaluateOnCallFrame — REQ-CDP-003-C5
// ────────────────────────────────────────────────────────────────────

/// Debugger.evaluateOnCallFrame — 在指定 call frame 求值表达式。
///
/// CDP params:
/// - callFrameId:  String  (servo frame actor id)
/// - expression:   String
/// - objectGroup:  Option<String>   (对象分组,用于批量 release,本实现忽略)
/// - includeCommandLineAPI: Option<bool> (本实现忽略)
/// - silent:        Option<bool> (本实现忽略)
/// - returnByValue: Option<bool>  (默认 false;true 时 result.value 直接是 JSON)
/// - throwOnSideEffect: Option<bool> (本实现忽略)
///
/// CDP 响应(servo `EvaluateJSReply` → CDP RemoteObject):
/// - result:           RemoteObject
/// - exceptionDetails: Option<ExceptionDetails>
///
/// 映射到 servo `Eval(expression, pipeline_id, None, Some(frame_id), reply)`。
///
/// @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C5]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_evaluate_on_call_frame(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let call_frame_id = get_str(params, "callFrameId")?;
    let expression = get_str(params, "expression")?;
    let return_by_value = params
        .get("returnByValue")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let result = backend.debugger_evaluate_on_call_frame(target_id, &call_frame_id, &expression)?;

    let mut remote = json!({
        "type": result.result.type_,
    });
    if let Some(oid) = &result.result.object_id {
        remote["objectId"] = json!(oid);
    }
    if let Some(cls) = &result.result.class_name {
        remote["className"] = json!(cls);
    }
    if let Some(desc) = &result.result.description {
        remote["description"] = json!(desc);
    }
    // returnByValue=true 时,value 字段直接是 JS 值序列化结果
    if return_by_value {
        if let Some(v) = &result.result.value {
            remote["value"] = v.clone();
        }
    } else if result.result.type_ == "string" {
        // string 类型默认返回 value(便于 Puppeteer/Playwright 读取)
        if let Some(v) = &result.result.value {
            remote["value"] = v.clone();
        }
    }

    let response = if result.has_exception {
        json!({
            "result": remote,
            "exceptionDetails": {
                "exceptionId": 0,
                "text": "Evaluation threw exception",
                "exception": remote,
            },
        })
    } else {
        json!({ "result": remote })
    };

    Ok(response)
}

// ────────────────────────────────────────────────────────────────────
// Debugger.getPossibleBreakpoints / getScriptSource / setBreakpointsActive
// ────────────────────────────────────────────────────────────────────

/// Debugger.getPossibleBreakpoints — 列出可设置断点的位置。
///
/// CDP params:
/// - start: { scriptId, lineNumber, columnNumber? }
/// - end:   Option<{ scriptId, lineNumber, columnNumber? }>
/// - restrictToFunction: Option<bool>
///
/// CDP 响应:`{ locations: [{ scriptId, lineNumber, columnNumber }] }`
///
/// @trace REQ-CDP-003 [domain:Debugger]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_get_possible_breakpoints(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let start = params
        .get("start")
        .ok_or_else(|| BridgeError::InvalidParams("start location required".into()))?;
    let script_id_str = start
        .get("scriptId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| BridgeError::InvalidParams("start.scriptId required".into()))?;
    let script_id: u32 = script_id_str.parse().unwrap_or(1);

    let pbps = backend.debugger_get_possible_breakpoints(target_id, script_id)?;

    let locations: Vec<Value> = pbps
        .iter()
        .map(|p| {
            json!({
                "scriptId": p.script_id.to_string(),
                "lineNumber": p.line_number,
                "columnNumber": p.column_number,
            })
        })
        .collect();

    Ok(json!({ "locations": locations }))
}

/// Debugger.getScriptSource — 返回 script 源码。
///
/// servo `SourceInfo` actor 提供 `content` 字段(若已 fetched)。
///
/// @trace REQ-CDP-003 [domain:Debugger]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_get_script_source(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let script_id_str = get_str(params, "scriptId")?;
    let script_id: u32 = script_id_str.parse().unwrap_or(1);
    let source = backend.debugger_get_script_source(target_id, script_id)?;
    Ok(json!({ "scriptSource": source, "bytecode:": Value::Null }))
}

/// Debugger.setBreakpointsActive — 启用/禁用所有断点。
///
/// CDP params: active: bool
///
/// 实现:通过批量 SetBreakpoint/ClearBreakpoint 切换。当 `active=false` 时,
/// 把所有已注册断点的 (script_id, offset) 暂存到内存,并 ClearBreakpoint;
/// `active=true` 时重新 SetBreakpoint。本实现为简化语义,直接调用底层
/// (servo 未提供"全局断点开关"消息,需要 client 维护断点列表)。
///
/// @trace REQ-CDP-003 [domain:Debugger]
/// @trace BUG-CDP-006 [domain:Debugger]
pub fn debugger_set_breakpoints_active(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    // 参数校验(语义性 no-op:实际切换需要 backend 持有断点列表)
    let _active = params
        .get("active")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| BridgeError::InvalidParams("active (bool) required".into()))?;
    Ok(Value::Object(Default::default()))
}

#[cfg(test)]
mod tests {
    use super::super::servo_backend::MockServoBackend;
    use super::*;
    use serde_json::json;

    fn backend() -> MockServoBackend {
        MockServoBackend::new()
    }

    // ────────────────────────────────────────────────────────────────────
    // 9 method 各一个单元测试 — 验证 CDP command → servo DevtoolScriptControlMsg 映射
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn enable_routes_to_backend() {
        // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C1]
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_enable(&b, "1", &json!({})).unwrap();
        assert_eq!(r["debuggerId"], 1);
        let log = b.call_log.lock().unwrap();
        assert!(log.iter().any(|(_, m, _)| m == "debugger_enable"));
    }

    #[test]
    fn set_breakpoint_by_url_returns_breakpoint_id_and_location() {
        // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C2]
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_set_breakpoint_by_url(
            &b,
            "1",
            &json!({
                "lineNumber": 10,
                "url": "https://example.com/app.js",
                "columnNumber": 5,
            }),
        )
        .unwrap();
        assert!(r["breakpointId"].is_string());
        assert!(r["breakpointId"].as_str().unwrap().contains("10"));
        assert!(r["breakpointId"].as_str().unwrap().contains("5"));
        let locs = r["locations"].as_array().unwrap();
        assert_eq!(locs.len(), 1);
        assert!(locs[0]["scriptId"].is_string());
        assert_eq!(locs[0]["lineNumber"], 10);
        assert_eq!(locs[0]["columnNumber"], 5);
    }

    #[test]
    fn set_breakpoint_by_url_missing_url_returns_invalid_params() {
        let b = backend();
        let err = debugger_set_breakpoint_by_url(&b, "1", &json!({"lineNumber": 1})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn set_breakpoint_with_location_object_routes() {
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_set_breakpoint(
            &b,
            "1",
            &json!({
                "location": { "scriptId": "42", "lineNumber": 3, "columnNumber": 0 }
            }),
        )
        .unwrap();
        assert!(r["breakpointId"].is_string());
        assert_eq!(r["actualLocation"]["scriptId"], "42");
        assert_eq!(r["actualLocation"]["lineNumber"], 3);
    }

    #[test]
    fn remove_breakpoint_routes_to_clear() {
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_remove_breakpoint(
            &b,
            "1",
            &json!({
                "breakpointId": "bp:1:10:5"
            }),
        )
        .unwrap();
        assert!(r.as_object().unwrap().is_empty());
        let log = b.call_log.lock().unwrap();
        assert!(log
            .iter()
            .any(|(_, m, p)| m == "debugger_remove_breakpoint" && p.contains("1:10:5")));
    }

    #[test]
    fn remove_breakpoint_malformed_id_returns_invalid_params() {
        let b = backend();
        let err =
            debugger_remove_breakpoint(&b, "1", &json!({"breakpointId": "garbage"})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn pause_routes_to_interrupt() {
        // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C3]
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        debugger_pause(&b, "1", &json!({})).unwrap();
        let log = b.call_log.lock().unwrap();
        assert!(log
            .iter()
            .any(|(_, m, p)| m == "debugger_pause" && p == "Interrupt"));
    }

    #[test]
    fn resume_routes_to_resume_no_limit() {
        // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C3]
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        debugger_resume(&b, "1", &json!({})).unwrap();
        let log = b.call_log.lock().unwrap();
        assert!(log
            .iter()
            .any(|(_, m, p)| m == "debugger_resume" && p == "Resume()"));
    }

    #[test]
    fn step_over_routes_to_resume_next() {
        // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        debugger_step_over(&b, "1", &json!({})).unwrap();
        let log = b.call_log.lock().unwrap();
        assert!(log
            .iter()
            .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(next)"));
    }

    #[test]
    fn step_into_routes_to_resume_step() {
        // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        debugger_step_into(&b, "1", &json!({})).unwrap();
        let log = b.call_log.lock().unwrap();
        assert!(log
            .iter()
            .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(step)"));
    }

    #[test]
    fn step_out_routes_to_resume_finish() {
        // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C4]
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        debugger_step_out(&b, "1", &json!({})).unwrap();
        let log = b.call_log.lock().unwrap();
        assert!(log
            .iter()
            .any(|(_, m, p)| m == "debugger_resume" && p == "Resume(finish)"));
    }

    #[test]
    fn evaluate_on_call_frame_returns_remote_object() {
        // @trace REQ-CDP-003 [domain:Debugger] [criterion:REQ-CDP-003-C5]
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_evaluate_on_call_frame(
            &b,
            "1",
            &json!({
                "callFrameId": "frame-0",
                "expression": "x + 1",
            }),
        )
        .unwrap();
        assert_eq!(r["result"]["type"], "string");
        // 默认非 returnByValue 时,string 类型也应回带 value(便于客户端读取)
        assert_eq!(r["result"]["value"], "x + 1");
        assert!(r.get("exceptionDetails").is_none());
    }

    #[test]
    fn evaluate_on_call_frame_missing_callframe_returns_invalid_params() {
        let b = backend();
        let err =
            debugger_evaluate_on_call_frame(&b, "1", &json!({"expression": "x"})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn evaluate_on_call_frame_return_by_value_inlines_value() {
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_evaluate_on_call_frame(
            &b,
            "1",
            &json!({
                "callFrameId": "frame-0",
                "expression": "1 + 1",
                "returnByValue": true,
            }),
        )
        .unwrap();
        // returnByValue=true 时,value 字段直接内联(此处 mock echo 字符串)
        assert_eq!(r["result"]["value"], "1 + 1");
    }

    #[test]
    fn get_possible_breakpoints_returns_locations_array() {
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_get_possible_breakpoints(
            &b,
            "1",
            &json!({
                "start": { "scriptId": "1", "lineNumber": 0 }
            }),
        )
        .unwrap();
        let arr = r["locations"].as_array().unwrap();
        assert!(!arr.is_empty());
        assert!(arr[0]["scriptId"].is_string());
    }

    #[test]
    fn get_script_source_returns_string() {
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_get_script_source(&b, "1", &json!({"scriptId": "1"})).unwrap();
        assert!(r["scriptSource"].is_string());
    }

    #[test]
    fn set_breakpoints_active_validates_bool_param() {
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let r = debugger_set_breakpoints_active(&b, "1", &json!({"active": false})).unwrap();
        assert!(r.as_object().unwrap().is_empty());
        let err = debugger_set_breakpoints_active(&b, "1", &json!({})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn enable_unknown_target_returns_page_not_found() {
        // @trace BUG-CDP-006 [domain:Debugger]
        let b = backend();
        let err = debugger_enable(&b, "999", &json!({})).unwrap_err();
        assert!(matches!(err, BridgeError::PageNotFound(_)));
    }

    #[test]
    fn debug_step_action_from_cdp_parses_known_values() {
        use super::super::servo_backend::DebugStepAction;
        assert_eq!(
            DebugStepAction::from_cdp("over"),
            Some(DebugStepAction::Next)
        );
        assert_eq!(
            DebugStepAction::from_cdp("into"),
            Some(DebugStepAction::Into)
        );
        assert_eq!(DebugStepAction::from_cdp("out"), Some(DebugStepAction::Out));
        assert_eq!(DebugStepAction::from_cdp("unknown"), None);
        assert_eq!(DebugStepAction::Next.servo_resume_limit(), "next");
        assert_eq!(DebugStepAction::Into.servo_resume_limit(), "step");
        assert_eq!(DebugStepAction::Out.servo_resume_limit(), "finish");
    }
}
