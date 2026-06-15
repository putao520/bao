//! # bao_cdp_client
//!
//! 统一浏览器控制客户端,基于 CDP (Chrome DevTools Protocol)。
//!
//! 通过 URL scheme 自动路由到内置 servo 或外部 Chrome:
//!
//! | Scheme               | Transport          | 适用场景                       |
//! |----------------------|--------------------|--------------------------------|
//! | `memory://bao`       | InMemoryTransport  | 同进程 servo,零网络往返       |
//! | `ws://host:port`     | WebSocketTransport | 外部 Chrome / Chromium         |
//! | `http://host:port`   | WebSocketTransport | HTTP discover → 自动转 ws://   |
//!
//! ## 设计目标
//!
//! - **统一 API**:不论连内嵌 servo 还是远端 Chrome,顶层 API 一致
//! - **Playwright 风格**:Browser/BrowserContext/Page/Frame/ElementHandle 等高层抽象
//! - **零 tokio**:用 `bun_event_loop` 调度收发,与 Bao 运行栈一致
//! - **完整类型**:所有公共 API 含 doc comment + 示例 + 错误码
//!
//! ## 快速开始
//!
//! 连接内嵌 servo 并打开页面:
//!
//! ```no_run
//! use bao_cdp_client::Browser;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 同进程 servo(memory:// scheme 路由到 InMemoryTransport)
//!     let browser = Browser::connect("memory://bao")?;
//!     assert!(browser.is_in_memory());
//!     Ok(())
//! }
//! ```
//!
//! 连接外部 Chrome:
//!
//! ```no_run
//! use bao_cdp_client::Browser;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let browser = Browser::connect("ws://127.0.0.1:9222")?;
//!     assert!(browser.is_websocket());
//!     Ok(())
//! }
//! ```
//!
//! ## 模块组织
//!
//! - [`browser`]: `Browser::connect(url)` 入口
//! - [`transport`]: Transport trait + InMemoryTransport / WebSocketTransport
//! - [`connection`]: Connection 配置与 URL 解析
//! - [`bridge`]: servo RDP 桥接(CDPRdpBridge + servo 7 事件 → CDP event)
//! - [`api`]: 高层 API 类(Page/Frame/ElementHandle/...)
//! - [`types`]: 公共类型(ScreenshotFormat/Cookie/Viewport/...)
//! - [`error`]: ConnectError + CdpError
//!
//! @trace REQ-BAO-API-001 [level:library]
//! @trace REQ-BAO-API-008 [level:library]

pub mod api;
pub mod bridge;
pub mod browser;
pub mod connection;
pub mod error;
pub mod transport;
pub mod types;

// ─── 顶层入口 ─────────────────────────────────────────────────────────────
//
// `Browser::connect(url)` 是用户与 bao_cdp_client 交互的起点。它通过 URL
// scheme 路由到对应的 transport(memory:// / ws:// / http://),返回一个
// [`Browser`] 句柄,后续可 `browser.new_page()` / `browser.contexts()` 等。
//
// @trace REQ-BAO-API-001 [level:library]
pub use browser::Browser;

// ─── 错误类型 ─────────────────────────────────────────────────────────────
//
// 公共错误类型分两层:
// - ConnectError: 连接阶段(URL 解析 / scheme 路由 / TCP 握手)
// - CdpError:     通信阶段(JSON-RPC 协议 / I/O / Timeout)
// - BridgeError:  servo RDP 桥接层错误
//
// 所有错误都实现 `std::error::Error + Display`,可用 `?` 在用户代码中传播。
//
// @trace REQ-BAO-API-001 [level:library]
pub use error::{CdpError, ConnectError, Result};
pub use bridge::BridgeError;

// ─── Transport 抽象 ──────────────────────────────────────────────────────
//
// [`Transport`] trait 抽象三种操作:send_command / recv_event / close。
// 公开 trait 允许用户实现自己的 Transport(如 Unix domain socket / TLS / 自定义协议)。
//
// - InMemoryTransport: servo 同进程桥接
// - WebSocketTransport: 外部 Chrome
//
// @trace REQ-BAO-API-002 [interface:Transport]
pub use transport::{
    CdpEvent, InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport, Transport, TransportKind,
    WebSocketTransport,
};

// ─── 高层 API 类 ─────────────────────────────────────────────────────────
//
// Playwright 风格的高层 API:
//
// - Browser:           顶层浏览器实例
// - BrowserContext:    隔离的 cookie / 缓存上下文(类似 incognito window)
// - Page:              一个 tab(顶层 frame)
// - Frame:             iframe / 主 frame(执行 JS、点击、查询)
// - ElementHandle:     DOM 元素引用
// - JSHandle:          任意 JS 对象引用
// - Request:           HTTP 请求
// - Response:          HTTP 响应
// - Dialog:            alert/prompt/confirm
// - ConsoleMessage:    console.log 等消息
// - Keyboard/Mouse/Touchscreen: 输入设备
// - Coverage/Tracing/Accessibility: 性能 / 调试工具
//
// 注:`api::Browser` 与顶层 `Browser` 不同 — 前者是高层 API 类(多个 Page 共享的
// 浏览器实例,包含版本/连接状态/disconnect 等),后者是 connection 入口(URL 路由)。
// 我们将高层 Browser 重命名为 `HighLevelBrowser`,顶层入口保留 `Browser`。
//
// @trace REQ-BAO-API-006 [level:library]
pub use api::browser::{Browser as HighLevelBrowser, BrowserOptions, Pid};
pub use api::browser_context::{BrowserContext, ContextOptions, PermissionOverride};
pub use api::page::{Page, TargetInfo as PageTargetInfo, Viewport as PageViewport, Worker};
pub use api::frame::{ExecutionContext, Frame};
pub use api::element_handle::{BoundingBox, ElementHandle};
pub use api::js_handle::JSHandle;
pub use api::request::Request;
pub use api::response::Response;
pub use api::dialog::{Dialog, DialogType};
pub use api::console_message::ConsoleMessage;
pub use api::keyboard::Keyboard;
pub use api::mouse::{MouseButton, Mouse};
pub use api::touchscreen::Touchscreen;
pub use api::coverage::Coverage;
pub use api::tracing::Tracing;
pub use api::accessibility::{Accessibility, AXNode};

// ─── EventEmitter 共享 trait ─────────────────────────────────────────────
//
// [`EventEmitter`] trait 让 Page / BrowserContext / Browser 等都支持
// `on/once/off/emit` 风格的事件订阅,与 Node.js EventEmitter 兼容。
//
// @trace REQ-BAO-API-003 [level:library]
pub use api::event_emitter::{
    EventHandler, EventEmitter, EventEmitterInner, HandlerId, SubscriptionResult,
};
// `delegate_event_emitter!` 是 #[macro_export],挂在 crate 根。

// ─── Bridge 类型 ─────────────────────────────────────────────────────────
//
// servo RDP 桥接层暴露的类型(用户通常不直接用,但供 advanced 用户):
// - InMemoryBridge trait:    允许用户实现自己的 servo 后端
// - CDPRdpBridge:            默认 servo → CDP 实现
// - ServoBackend trait:      抽象 servo 操作
// - ServoEvent:              servo 7 类原始事件
// - EventSubscriber:         订阅 servo 事件
// - 翻译函数 translate / from_console_message: 把 servo 事件转为 CDP event
//
// @trace REQ-BAO-API-004 [level:library]
// @trace REQ-BAO-API-005 [level:library]
// @trace REQ-BAO-API-007 [level:library]
pub use bridge::{
    dispatch_command, translate as translate_event, ConsoleLevel, EventSubscriber, ServoEvent,
};
/// 与 [`translate_event`] 等价 — servo 事件 → CDP event 翻译函数别名。
///
/// @trace REQ-BAO-API-003 [level:library]
pub fn translate_servo_event(event: ServoEvent) -> Vec<crate::CdpEvent> {
    translate_event(event)
}
pub use bridge::{
    BoxModel, CDPRdpBridge, CSSProperty, CSSStyle, DeviceMetrics, EvaluateResult, ExceptionDetails,
    Frame as BridgeFrame, FrameTree, KeyEvent, LayoutMetrics, MatchedRule, MatchedStyles,
    MockServoBackend, MouseEvent, NavigateResult, NavigationEntry, NavigationHistory,
    NodeDescriptor, PropertyDescriptor, RemoteObject, ResponseBody, ServoBackend, TargetInfo,
    TouchPoint,
};
// 截图格式:bridge 内部用 `BridgeScreenshotFormat`,公共 API 别名为 `ScreenshotFormat`。
pub use bridge::ScreenshotFormat as BridgeScreenshotFormat;
pub use connection::{Connection, ConnectionConfig, ParsedConnectUrl};

// ─── 公共类型(types 模块顶层 re-export) ────────────────────────────────
//
// 让用户可以直接 `use bao_cdp_client::{Cookie, Viewport, ...}` 而不必记模块路径。
//
// @trace REQ-BAO-API-008 [level:library]
pub use types::{Cookie, DeviceDescriptor, ScreenshotFormat, Viewport, WaitUntilState};

/// bao_cdp_client 当前版本(来自 Cargo.toml)。
///
/// @trace REQ-BAO-API-001 [level:library]
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// bao_cdp_client 当前版本字符串,等价于 [`VERSION`]。
///
/// 兼容 `Bun.version` 风格的 API 设计。
pub fn version() -> &'static str {
    VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty());
        assert!(VERSION.contains('.'));
        assert_eq!(version(), VERSION);
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

    /// 编译时验证所有公共 API 类型可被外部代码引用(防止 accidental private)。
    /// 这是一种"API 表面完整性"测试 — 如果任何类型变成私有,test 不再编译。
    #[test]
    fn public_api_surface_compiles() {
        // 顶层入口
        let _ = Browser::connect;
        // 错误类型
        let _: Option<CdpError> = None;
        let _: Option<ConnectError> = None;
        let _: Option<BridgeError> = None;
        // Transport
        let _: Option<Box<dyn Transport>> = None;
        let _: TransportKind = TransportKind::InMemory;
        let _: Option<CdpEvent> = None;
        // 高层 API 类型 — 仅类型签名检查(不实例化,因构造需 transport)
        fn _assert_types(
            _: HighLevelBrowser,
            _: BrowserOptions,
            _: Pid,
            _: BrowserContext,
            _: ContextOptions,
            _: PermissionOverride,
            _: Page,
            _: PageTargetInfo,
            _: PageViewport,
            _: Worker,
            _: Frame,
            _: ExecutionContext,
            _: ElementHandle,
            _: BoundingBox,
            _: JSHandle,
            _: Request,
            _: Response,
            _: Dialog,
            _: DialogType,
            _: ConsoleMessage,
            _: Keyboard,
            _: MouseButton,
            _: Mouse,
            _: Touchscreen,
            _: Coverage,
            _: Tracing,
            _: Accessibility,
            _: AXNode,
            _: HandlerId,
            _: Cookie,
            _: Viewport,
            _: DeviceDescriptor,
            _: ScreenshotFormat,
            _: WaitUntilState,
        ) {
        }
        let _ = _assert_types;
        // 常量
        let _: &'static str = VERSION;
    }

    /// Cookie / ScreenshotFormat / WaitUntilState 的构造 + serde 往返。
    #[test]
    fn public_types_basic_roundtrip() {
        let c = Cookie::new("name", "value").with_domain("example.com");
        let json = serde_json::to_string(&c).unwrap();
        let back: Cookie = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);

        assert_eq!(ScreenshotFormat::Png.as_cdp_str(), "png");
        assert_eq!(WaitUntilState::Load.as_str(), "load");
    }
}
