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
//! - [`event_translator`]:servo 7 类事件 → CDP event 转换 + EventSubscriber
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
pub mod cdp_rdp_bridge;
pub mod command_dispatcher;
pub mod debugger_handlers;
pub mod e_class;
pub mod error;
pub mod eval_synthesizer;
pub mod servo_backend;
pub mod event_translator;

pub use cdp_rdp_bridge::CDPRdpBridge;
pub use command_dispatcher::dispatch_command;
pub use error::{
    BridgeError, CDP_ERR_INVALID_PARAMS, CDP_ERR_METHOD_NOT_FOUND, CDP_ERR_SERVER_ERROR,
};
pub use servo_backend::{
    BoxModel, CSSComputedStyleProperty, CSSProperty, CSSStyle, DeviceMetrics, EvaluateResult,
    ExceptionDetails, Frame, FrameTree, KeyEvent, LayoutMetrics, MatchedRule, MatchedStyles,
    MouseEvent, NavigateResult, NavigationEntry, NavigationHistory, NodeDescriptor,
    PropertyDescriptor, RemoteObject, ResponseBody, ServoBackend, TargetInfo, TouchPoint,
    BridgeScreenshotFormat as ScreenshotFormat, MockServoBackend,
    BreakpointResult, DebuggerEvalResult, DebuggerRemoteObject, DebugStepAction, PossibleBreakpoint,
};
pub use event_translator::{
    from_console_message, translate, ConsoleLevel, EventSubscriber, ServoEvent,
};
