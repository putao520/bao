//! Bridge 模块 — servo RDP 桥接核心。
//!
//! 子模块:
//! - [`cdp_rdp_bridge`]:CDPRdpBridge 结构 + InMemoryBridge 实现
//! - [`command_dispatcher`]:match (domain, method) 分发框架
//! - [`a_class_handlers`]:A 类 48 method handler
//! - [`e_class`]:E 类 31+ method servo 不支持标记
//! - [`servo_backend`]:ServoBackend trait + MockServoBackend + 数据结构
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
//!       ↓                                ↓
//!   A 类 48 handler               E 类 31+ → NotSupported -32601
//!       ↓
//!   ServoBackend::page_navigate / runtime_evaluate / ...
//!       ↓
//!   MockServoBackend (test) | PagePoolBackend (TASK-3b+)
//! ```
//!
//! @trace REQ-BAO-API-004 [level:library]
//! @trace REQ-BAO-API-007 [level:library]

pub mod a_class_handlers;
pub mod cdp_rdp_bridge;
pub mod command_dispatcher;
pub mod e_class;
pub mod error;
pub mod servo_backend;

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
};
