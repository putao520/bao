//! Bao CDP Client — Chrome DevTools Protocol client for Bao browser.
//!
//! Fork of chromiumoxide,适配 Bao 的"servo + SpiderMonkey + bun_event_loop"运行栈:
//!
//! - **零 tokio**:用 `bun_event_loop` 调度收发
//! - **双 transport**:`memory://`(同进程 servo RDP) / `ws://`/`http://`(外部 Chrome)
//! - **统一 URL 入口**:`Browser::connect(url)` 通过 scheme 自动路由(DEC-URL-001)
//! - **统一 CDP 协议层**:复用 `bao_cdp` 的 JSON-RPC 编解码
//!
//! # 模块组织
//! - [`browser`]: `Browser::connect(url)` 入口,URL scheme 路由
//! - [`transport`]: Transport trait 抽象 + InMemoryTransport / WebSocketTransport 双实现
//! - [`connection`]: Connection 配置与 URL 解析结果
//! - [`error`]: `ConnectError` + `CdpError` 错误类型
//!
//! # 示例
//! ```
//! use bao_cdp_client::Browser;
//!
//! // InMemory servo
//! let b = Browser::connect("memory://bao").unwrap();
//! assert!(b.is_in_memory());
//!
//! // 外部 Chrome(直连)
//! let b = Browser::connect("ws://127.0.0.1:9222").unwrap();
//! assert!(b.is_websocket());
//!
//! // 外部 Chrome(HTTP 自动发现 ws endpoint)
//! let b = Browser::connect("http://127.0.0.1:9222").unwrap();
//! assert!(b.is_websocket());
//! ```
//!
//! @trace REQ-BAO-API-001 [level:library]

pub mod api;
pub mod bridge;
pub mod browser;
pub mod connection;
pub mod error;
pub mod transport;

// 顶层 re-export — 公共 API 表面(REQ-BAO-API-008 会进一步扩展)。
pub use browser::Browser;
pub use bridge::{
    dispatch_command, BridgeError, BoxModel, CDPRdpBridge, CSSProperty,
    CSSStyle, DeviceMetrics, EvaluateResult, ExceptionDetails, Frame, FrameTree, KeyEvent,
    LayoutMetrics, MatchedRule, MatchedStyles, MockServoBackend, MouseEvent, NavigateResult,
    NavigationEntry, NavigationHistory, NodeDescriptor, PropertyDescriptor, RemoteObject,
    ResponseBody, ScreenshotFormat, ServoBackend, TargetInfo, TouchPoint,
};
// TASK-4: servo 7 事件 → CDP event(REQ-BAO-API-003)
pub use bridge::{
    from_console_message, translate as translate_servo_event, ConsoleLevel, EventSubscriber,
    ServoEvent,
};
pub use connection::{Connection, ConnectionConfig, ParsedConnectUrl};
pub use error::{CdpError, ConnectError, Result};
pub use transport::{
    CdpEvent, InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport, Transport, TransportKind,
    WebSocketTransport,
};

/// bao_cdp_client 当前版本(来自 Cargo.toml)。
///
/// @trace REQ-BAO-API-001 [level:library]
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty());
        assert!(VERSION.contains('.'));
    }

    #[test]
    fn reexport_browser_callable() {
        let b = Browser::connect("memory://bao").unwrap();
        assert!(b.is_in_memory());
    }

    #[test]
    fn reexport_connect_error_matches() {
        let err = Browser::connect("ftp://x").unwrap_err();
        assert!(matches!(err, ConnectError::InvalidScheme(_)));
    }

    #[test]
    fn reexport_transport_kind() {
        let b = Browser::connect("ws://127.0.0.1:9222").unwrap();
        assert_eq!(b.transport_kind(), TransportKind::WebSocket);
    }
}
