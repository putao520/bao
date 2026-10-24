//! Bridge 模块 — servo RDP 桥接核心。
//!
//! 子模块:
//! - [`cdp_rdp_bridge`]:CDPRdpBridge 结构 + InMemoryBridge 实现
//! - [`command_dispatcher`]:match (domain, method) 分发框架
//! - [`a_class_handlers`]:A 类 48 method handler
//! - [`b_class_handlers`]:B 类 52 method handler(IIFE Eval 合成 + 多步合成)
//! - [`debugger_handlers`]:Debugger domain 14 method handler(BUG-CDP-006 接入 servo SM Debugger API)
//! - [`eval_synthesizer`]:IIFE 安全封装 + JSON.stringify 参数化
//! - [`e_class`]:E 类 31+ method servo 不支持标记
//! - [`servo_backend`]:ServoBackend trait + MockServoBackend + 数据结构
//! - [`bridge_sender_backend`]:ServoBackend 的生产通道实现(BridgeCommand 挂接,M1 接线 REQ-CDP-001)
//! - [`event_translator`]:servo 7 类事件 → CDP event 转换 + EventSubscriber + CdpEventTap(WS 事件等待面,M1 P1)
//! - [`error`]:BridgeError + CDP error code 映射
//!
//! # 数据流
//!
//! ```text
//!   CDP Client
//!       ↓ (JSON-RPC command)
//!   InMemoryTransport::send_command
//!       ↓
//!   InMemoryBridge::dispatch_command
//!       ↓
//!   CDPRdpBridge::dispatch_command
//!       ↓
//!   command_dispatcher::dispatch_command (match domain.method)
//!       ↓                  ↓                ↓
//!   A 类 48 handler  B 类 52 handler  E 类 31+ → NotSupported -32601
//!       ↓                  ↓                ↓
//!   ServoBackend    eval_synthesizer   debugger_handlers (BUG-CDP-006)
//!       ↓                  ↓                ↓
//!   MockServoBackend / PagePoolBackend → servo SM Debugger API
//! ```
//!
//! @trace REQ-BAO-API-004 [level:library]
//! @trace REQ-BAO-API-005 [level:library]
//! @trace REQ-BAO-API-007 [level:library]
//! @trace REQ-CDP-003 [level:library]
//! @trace BUG-CDP-006 [level:library]

pub mod a_class_handlers;
pub mod b_class_handlers;
pub mod bridge_sender_backend;
pub mod cdp_rdp_bridge;
pub mod command_dispatcher;
pub mod debugger_handlers;
pub mod e_class;
pub mod error;
pub mod eval_synthesizer;
pub mod event_translator;
pub mod servo_backend;

pub use bridge_sender_backend::BridgeSenderBackend;
pub use cdp_rdp_bridge::CDPRdpBridge;
pub use command_dispatcher::dispatch_command;
pub use error::{
    BridgeError, CDP_ERR_INVALID_PARAMS, CDP_ERR_METHOD_NOT_FOUND, CDP_ERR_SERVER_ERROR,
};
pub use event_translator::{
    from_console_message, translate, CdpEventTap, ConsoleLevel, EventSubscriber, ServoEvent,
};
pub use servo_backend::{
    BoxModel, BreakpointResult, BridgeScreenshotFormat as ScreenshotFormat,
    CSSComputedStyleProperty, CSSProperty, CSSStyle, DebugStepAction, DebuggerEvalResult,
    DebuggerRemoteObject, DeviceMetrics, EvaluateResult, ExceptionDetails, Frame, FrameTree,
    KeyEvent, LayoutMetrics, MatchedRule, MatchedStyles, MockServoBackend, MouseEvent,
    NavigateResult, NavigationEntry, NavigationHistory, NodeDescriptor, PossibleBreakpoint,
    PropertyDescriptor, RemoteObject, ResponseBody, ServoBackend, TargetInfo, TouchPoint,
};

// ────────────────────────────────────────────────────────────────────
// 通用 JSON 参数助手 —— bridge 层共享单一实现。
//
// REQ-ARCH-001 卫生波次:原 `a_class_handlers` / `b_class_handlers` /
// `debugger_handlers` 三处逐字节重复的副本已收敛到本模块单一
// `pub(crate)` 位置,各 handler 模块经 `use super::{...}` 引用。
// 纯结构变换,行为零变化(函数体逐字节等价)。
// ────────────────────────────────────────────────────────────────────

use serde_json::Value;

/// 从 params 抽取必填字符串字段,缺失时返回 InvalidParams。
pub(crate) fn get_str(params: &Value, key: &str) -> Result<String, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing string field: {key}")))
}

/// 从 params 抽取可选字符串字段(允许缺失)。
pub(crate) fn get_opt_str(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 从 params 抽取可选 i64 字段(允许缺失,缺失返回 default)。
pub(crate) fn get_opt_i64(params: &Value, key: &str, default: i64) -> i64 {
    params.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

/// 从 params 抽取可选 f64 字段。
pub(crate) fn get_opt_f64(params: &Value, key: &str, default: f64) -> f64 {
    params.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

/// 从 params 抽取可选 bool 字段。
pub(crate) fn get_opt_bool(params: &Value, key: &str, default: bool) -> bool {
    params.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// 从 params 抽取必填 i64。
pub(crate) fn get_i64(params: &Value, key: &str) -> Result<i64, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing int field: {key}")))
}

/// 从 params 抽取必填 f64。
#[allow(dead_code)]
pub(crate) fn get_f64(params: &Value, key: &str) -> Result<f64, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing float field: {key}")))
}

/// 从 params 抽取 array 字段(返回 Value::Array 借用)。
pub(crate) fn get_array<'a>(params: &'a Value, key: &str) -> Result<&'a Vec<Value>, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing array field: {key}")))
}

/// enable/disable/dispose 类 handler 共享体 —— backend 单元操作成功后返回
/// 空对象 CDP 响应(`Value::Object({})`),失败则传播 `BridgeError`。
///
/// REQ-ARCH-001:多个 handler 逐字节同形(`op()?; Ok(Value::Object(Default::default()))`),
/// 提取为单一 helper(行为零变化)。
pub(crate) fn unit_ok(r: Result<(), BridgeError>) -> Result<Value, BridgeError> {
    r?;
    Ok(Value::Object(Default::default()))
}
