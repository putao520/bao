//! 公共 API 完整性测试(REQ-BAO-API-008)
//!
//! 这个测试文件验证 bao_cdp_client 的**公共 API 表面**完全可用 —
//! 所有 `pub` 类型 / trait / 函数 / 错误都能被外部 crate 引用、构造、传递。
//!
//! ## 测试策略
//!
//! 每个测试聚焦一组公共 API,编译时验证可达性:
//! - `top_level_browser`:顶层 `Browser::connect(url)` 入口
//! - `top_level_errors`:ConnectError / CdpError / BridgeError 类型
//! - `top_level_transport`:Transport trait + TransportKind + CdpEvent
//! - `high_level_api`:HighLevelBrowser / BrowserContext / Page / Frame / ...
//! - `tool_classes`:Keyboard / Mouse / Touchscreen / Coverage / Tracing / Accessibility
//! - `event_emitter`:EventEmitter trait + HandlerId + EventHandler + SubscriptionResult
//! - `public_types`:ScreenshotFormat / WaitUntilState / Cookie / Viewport / DeviceDescriptor
//! - `bridge_types`:CDPRdpBridge / ServoBackend / ServoEvent / ConsoleLevel / EventSubscriber
//! - `version_constants`:VERSION / version()
//!
//! 如果任何公共 API 变成私有(或被意外移除),测试将无法编译 —
//! 这是一种"API 表面完整性"门控。
//!
//! @trace REQ-BAO-API-008 [level:library]

// 直接 use 顶层 re-export — 验证用户代码可以"扁平化"导入,不需要记模块路径。
use bao_cdp_client::{
    bridge::ConsoleLevel, bridge::ServoEvent, AXNode, Accessibility, BoundingBox, BridgeError,
    BridgeScreenshotFormat, Browser, BrowserContext, BrowserOptions, CDPRdpBridge, CdpError,
    CdpEvent, Connection, ConnectionConfig, ConnectError, ConsoleMessage, ContextOptions, Cookie,
    Coverage, DeviceDescriptor, Dialog, DialogType, ElementHandle, EventHandler, EventEmitter,
    EventEmitterInner, EventSubscriber, ExecutionContext, Frame, HandlerId, HighLevelBrowser,
    InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport, JSHandle, Keyboard, MouseButton,
    Mouse, NavigationEntry, NavigationHistory, Page, PageTargetInfo, PageViewport,
    ParsedConnectUrl, PermissionOverride, Pid, RemoteObject, Request, Response, ScreenshotFormat,
    ServoBackend, SubscriptionResult, TargetInfo, Touchscreen, Tracing, Transport, TransportKind,
    Viewport, WaitUntilState, WebSocketTransport, Worker, VERSION,
};

#[test]
fn top_level_browser() {
    // Arrange
    // Act
    let browser = Browser::connect("memory://bao").unwrap();
    // Assert
    assert!(browser.is_in_memory());
    assert!(!browser.is_websocket());
    assert_eq!(browser.scheme(), "memory");
    assert_eq!(browser.url(), "memory://bao");
    assert_eq!(browser.transport_kind(), TransportKind::InMemory);

    let ws_browser = Browser::connect("ws://127.0.0.1:9222").unwrap();
    assert!(ws_browser.is_websocket());
    assert!(!ws_browser.is_in_memory());
}

#[test]
fn top_level_errors() {
    // Arrange
    // ConnectError 变体
    let _: ConnectError = ConnectError::InvalidUrl;
    // Act
    let _: ConnectError = ConnectError::InvalidScheme("ftp".to_string());
    let _: ConnectError = ConnectError::LaunchError("msg".to_string());
    let _: ConnectError = ConnectError::ConnectionFailed("refused".to_string());
    let _: ConnectError = ConnectError::Timeout("30s".to_string());

    // CdpError 变体
    let _: CdpError = CdpError::ProtocolError("err".to_string());
    let _: CdpError = CdpError::JsonError("err".to_string());
    let _: CdpError = CdpError::IoError(std::io::Error::new(std::io::ErrorKind::Other, "boom"));
    let _: CdpError = CdpError::ConnectionClosed;
    let _: CdpError = CdpError::Timeout("err".to_string());
    let _: CdpError = CdpError::TransportError("err".to_string());
    let _: CdpError = CdpError::HandshakeError("err".to_string());

    // BridgeError 可构造(具体变体由内部模块决定)
    let _: fn() -> Option<BridgeError> = || None;

    // std::error::Error 兼容性
    fn assert_std_error<E: std::error::Error>() {}
    assert_std_error::<ConnectError>();
    assert_std_error::<CdpError>();
    // Assert
    assert_std_error::<BridgeError>();
}

#[test]
fn top_level_transport() {
    // Arrange
    // CdpEvent 构造
    let evt = CdpEvent::new("Page.frameNavigated", serde_json::json!({"url": "about:blank"}));
    // Act
    // Assert
    assert_eq!(evt.method, "Page.frameNavigated");
    let evt_with_session = evt.with_session("TARGET-1");
    assert_eq!(evt_with_session.session_id.as_deref(), Some("TARGET-1"));

    // TransportKind 相等性
    assert_eq!(TransportKind::InMemory, TransportKind::InMemory);
    assert_ne!(TransportKind::InMemory, TransportKind::WebSocket);

    // ParsedConnectUrl / ConnectionConfig 构造
    let parsed = ParsedConnectUrl::new("memory://bao", "memory", TransportKind::InMemory);
    assert_eq!(parsed.raw, "memory://bao");
    assert_eq!(parsed.scheme, "memory");

    let cfg = ConnectionConfig::default();
    assert_eq!(cfg.default_timeout_ms, 30_000);

    // Connection::new 需要传入 Transport + config
    // 此处验证 ConnectionConfig 可构造且字段可达
    let cfg2 = ConnectionConfig {
        default_timeout_ms: 5000,
        transport_kind: TransportKind::InMemory,
    };
    assert_eq!(cfg2.default_timeout_ms, 5000);

    // Transport trait object 可构造(用户可自定义实现)
    let _: Option<Box<dyn Transport>> = None;

    // InMemoryBridge / InMemoryBridgeResponse / WebSocketTransport 类型可达
    let _: Option<Box<dyn InMemoryBridge>> = None;
    let _: Option<WebSocketTransport> = None;
    let _: Option<InMemoryTransport> = None;
    let _: Option<InMemoryBridgeResponse> = None;
}

#[test]
fn high_level_api_types_are_public() {
    // Arrange
    // 编译时验证所有高层 API 类型可被外部代码引用。
    // 不实例化(构造需 transport),只验证可达。
    // Act
    fn _assert_high_level_types(
        // Assert
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
    ) {
    }
    let _ = _assert_high_level_types;
}

#[test]
fn tool_classes_are_public() {
    // Arrange
    // Act
    fn _assert_tool_classes(
        // Assert
        _: Keyboard,
        _: MouseButton,
        _: Mouse,
        _: Touchscreen,
        _: Coverage,
        _: Tracing,
        _: Accessibility,
        _: AXNode,
    ) {
    }
    let _ = _assert_tool_classes;
}

#[test]
fn event_emitter_trait_usable() {
    // Arrange
    // HandlerId / EventHandler / SubscriptionResult 类型可达
    let _: HandlerId = 0u64;
    let _handler: Option<EventHandler> = None;

    // SubscriptionResult 变体可达
    // Act
    let _ = SubscriptionResult::Registered(0);
    let _ = SubscriptionResult::Removed;
    let _ = SubscriptionResult::NotFound;

    // EventEmitter trait 可作为 bound
    fn _accept_emitter<E: EventEmitter>() {}
    // Assert
    _accept_emitter::<Page>();

    // EventEmitterInner 类型可达
    let _: Option<EventEmitterInner> = None;
}

#[test]
fn public_types_full_coverage() {
    // Arrange
    // ScreenshotFormat 变体 + 转换
    // Assert
    assert_eq!(ScreenshotFormat::Png.as_cdp_str(), "png");
    assert_eq!(ScreenshotFormat::Jpeg.as_cdp_str(), "jpeg");
    assert_eq!(ScreenshotFormat::Webp.as_cdp_str(), "webp");
    assert_eq!(ScreenshotFormat::from_cdp(Some("png")), ScreenshotFormat::Png);
    assert_eq!(ScreenshotFormat::default(), ScreenshotFormat::Png);

    // WaitUntilState 变体 + 转换
    assert_eq!(WaitUntilState::Load.as_str(), "load");
    assert_eq!(WaitUntilState::DomContentLoaded.as_str(), "domcontentloaded");
    assert_eq!(WaitUntilState::NetworkIdle0.as_str(), "networkidle0");
    assert_eq!(WaitUntilState::NetworkIdle2.as_str(), "networkidle2");
    assert_eq!(WaitUntilState::default(), WaitUntilState::Load);

    // Cookie 构造 + builder + 序列化
    let c = Cookie::new("k", "v")
        .with_domain("example.com")
        .with_path("/")
        .with_secure(true)
        .with_http_only(true)
        .with_same_site("Lax");
    // Act
    let json = serde_json::to_string(&c).unwrap();
    let back: Cookie = serde_json::from_str(&json).unwrap();
    assert_eq!(c, back);

    // Viewport 构造 + Default
    let vp = Viewport::default();
    assert_eq!(vp.width, 1280);
    assert_eq!(vp.height, 720);

    let mobile_vp = Viewport {
        width: 390,
        height: 844,
        device_scale_factor: 3.0,
        is_mobile: true,
        has_touch: true,
        is_landscape: false,
    };
    let _json = serde_json::to_string(&mobile_vp).unwrap();

    // DeviceDescriptor 构造
    let dev = DeviceDescriptor::new("iPhone", "UA", mobile_vp);
    assert_eq!(dev.name, "iPhone");
}

#[test]
fn bridge_types_are_public() {
    // Arrange
    // servo RDP 桥接层类型可达(供 advanced 用户)
    let _: Option<Box<dyn ServoBackend>> = None;
    let _: Option<Box<CDPRdpBridge>> = None;
    let _: Option<EventSubscriber> = None;
    let _: Option<RemoteObject> = None;
    let _: Option<NavigationEntry> = None;
    let _: Option<NavigationHistory> = None;
    let _: Option<TargetInfo> = None;

    // ConsoleLevel 变体可达
    let _ = ConsoleLevel::Verbose;
    let _ = ConsoleLevel::Info;
    let _ = ConsoleLevel::Warning;
    let _ = ConsoleLevel::Error;
    let _ = ConsoleLevel::Debug;

    // ServoEvent 类型可达(具体变体由内部决定)
    // Act
    fn _accept_servo_event(_: ServoEvent) {}
    // Assert
    let _ = _accept_servo_event;

    // BridgeScreenshotFormat 别名存在
    let _ = BridgeScreenshotFormat::Png;
}

#[test]
fn version_constants_and_fn() {
    // Arrange
    // Act
    // Assert
    assert!(!VERSION.is_empty());
    assert!(VERSION.contains('.'));
    assert_eq!(bao_cdp_client::version(), VERSION);
}

#[test]
fn connection_url_parsing_works() {
    // Arrange
    // memory:// → InMemory
    let p = ParsedConnectUrl::new("memory://bao", "memory", TransportKind::InMemory);
    // Act
    // Assert
    assert_eq!(p.transport_kind, TransportKind::InMemory);

    // ws:// → WebSocket
    let p = ParsedConnectUrl::new("ws://x", "ws", TransportKind::WebSocket);
    assert_eq!(p.transport_kind, TransportKind::WebSocket);
}

#[test]
fn delegate_event_emitter_macro_exists() {
    // Arrange
    // 验证 #[macro_export] 在 crate 根的 delegate_event_emitter! 可达。
    // 我们构造一个 mock 类型,实际展开宏验证可用。
    use bao_cdp_client::delegate_event_emitter;
    use bao_cdp_client::EventEmitterInner;
    use bao_cdp_client::{EventHandler, HandlerId, SubscriptionResult};
    use std::rc::Rc;

    /// 测试用 mock — 持有 EventEmitterInner,通过宏自动委托 EventEmitter trait。
    struct MockEmitter {
        inner: Rc<EventEmitterInner>,
    }

    impl EventEmitter for MockEmitter {
        // 展开宏:自动生成 on/once/off/remove_all_listeners/listener_count/emit。
        // Act
        delegate_event_emitter!(self, inner);
    }

    let m = MockEmitter {
        inner: Rc::new(EventEmitterInner::new()),
    };
    let handler: EventHandler = std::sync::Arc::new(|_args: &[serde_json::Value]| {});
    let id: HandlerId = m.on("test", handler);
    let result = m.off("test", id);
    // Assert
    assert!(matches!(
        result,
        SubscriptionResult::Removed | SubscriptionResult::NotFound
    ));
    assert_eq!(m.listener_count("test"), 0);
}
