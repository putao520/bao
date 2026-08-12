//! Page — 浏览器页面(target / frame tree / 输入设备)。
//!
//! D 类 method(全部本地状态,无 CDP 往返):
//! - `is_closed() -> bool`
//! - `main_frame() -> Rc<Frame>`
//! - `frames() -> Vec<Rc<Frame>>`
//! - `workers() -> Vec<Rc<Worker>>`
//! - `viewport() -> Option<Viewport>`
//! - `url() -> &str`(主 frame URL)
//! - `target() -> &TargetInfo`
//! - `target_id() -> &str`
//! - `opener() -> Option<Rc<Page>>`
//! - `browser() -> Rc<Browser>`
//! - `browser_context() -> Rc<BrowserContext>`
//! - `mouse() -> &Mouse`
//! - `keyboard() -> &Keyboard`
//! - `touchscreen() -> &Touchscreen`
//! - `coverage() -> &Coverage`
//! - `tracing() -> &Tracing`
//! - `accessibility() -> &Accessibility`
//! - `default_timeout() -> Duration`
//! - `default_navigation_timeout() -> Duration`
//! - `viewport_size() -> Option<(u32,u32)>`
//! - `is_service_worker() -> bool`
//! - `on/once/off/listener_count/remove_all_listeners/emit`(EventEmitter)
//! - `set_default_timeout(ms)`
//! - `set_default_navigation_timeout(ms)`
//! - `set_viewport(v)`
//! - `workers_count() -> usize`
//! - `frames_count() -> usize`
//!
//! CDP 命令 method(通过 Connection 发送):
//! - `goto(url)` → `Page.navigate`
//! - `evaluate(expr)` → `Runtime.evaluate`
//! - `screenshot()` → `Page.captureScreenshot`
//! - `close()` → `Page.close`
//! - `title()` → `document.title`(via Runtime.evaluate)
//!
//! @trace REQ-BAO-API-006 [class:Page]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

use serde_json::Value;

use super::accessibility::Accessibility;
use super::browser_context::BrowserContext;
use super::coverage::Coverage;
use super::event_emitter::{EventEmitter, EventEmitterInner};
use super::frame::Frame;
use super::keyboard::Keyboard;
use super::mouse::Mouse;
use super::touchscreen::Touchscreen;
use super::tracing::Tracing;
use crate::api::browser::Browser as HighLevelBrowser;
use crate::connection::Connection;
use crate::error::CdpError;

/// Worker — Web Worker(target attached 类型)。
#[derive(Debug, Clone)]
pub struct Worker {
    pub target_id: String,
    pub url: String,
    pub type_str: String,
}

impl Worker {
    pub fn new(
        target_id: impl Into<String>,
        url: impl Into<String>,
        type_str: impl Into<String>,
    ) -> Self {
        Self {
            target_id: target_id.into(),
            url: url.into(),
            type_str: type_str.into(),
        }
    }
    pub fn target_id(&self) -> &str {
        &self.target_id
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn type_str(&self) -> &str {
        &self.type_str
    }
}

/// Viewport(viewport 配置)。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub is_mobile: bool,
    pub has_touch: bool,
    pub is_landscape: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            device_scale_factor: 1.0,
            is_mobile: false,
            has_touch: false,
            is_landscape: false,
        }
    }
}

/// TargetInfo — CDP target 信息(从 Target.targetInfoChanged 事件更新)。
#[derive(Debug, Clone, Default)]
pub struct TargetInfo {
    pub target_id: String,
    pub type_str: String,
    pub title: String,
    pub url: String,
    pub attached: bool,
    pub opener_id: Option<String>,
    pub browser_context_id: Option<String>,
}

/// Page 本地状态(核心类)。
///
/// 持有:target_id、main_frame、frame tree、本地实例(mouse/keyboard/...)、
/// EventEmitter inner、默认 timeout、opener weak、context 弱引用、
/// Connection 引用(共享,所有 Page 共用同一 CDP 连接)。
///
/// @trace REQ-BAO-API-006 [class:Page]
pub struct Page {
    /// Target ID(Target.targetCreated 事件分配的 ID)。
    target_id: String,
    /// 是否 service worker / shared worker(非 page target)。
    is_service_worker: Cell<bool>,
    /// 主 Frame(Rc,持有 frame tree 根)。
    main_frame: Rc<Frame>,
    /// 所有 frame(ID → Rc),便于 O(1) 查找。
    frames_map: RefCell<HashMap<String, Rc<Frame>>>,
    /// Workers 列表。
    workers: RefCell<Vec<Worker>>,
    /// Viewport(本地缓存,setViewport 命令更新)。
    viewport: RefCell<Option<Viewport>>,
    /// TargetInfo(本地缓存,事件更新)。
    target_info: RefCell<TargetInfo>,
    /// 输入设备(本地单例)。
    mouse: Mouse,
    keyboard: Keyboard,
    touchscreen: Touchscreen,
    coverage: Coverage,
    tracing: Tracing,
    accessibility: Accessibility,
    /// 是否已关闭(Page.close 或 detachedFromTarget 后置 true)。
    closed: RefCell<bool>,
    /// 默认 timeout(本地配置)。
    default_timeout: RefCell<Option<Duration>>,
    /// 默认 navigation timeout(本地配置)。
    default_nav_timeout: RefCell<Option<Duration>>,
    /// Opener Page(weak,防止循环)。
    opener: RefCell<Option<Weak<Page>>>,
    /// 所属 BrowserContext(weak)。
    context: Weak<BrowserContext>,
    /// Connection 引用(共享 — 同一 Browser 的所有 Page 共用)。
    /// `None` 表示未连接(占位 Page / 测试用)。
    /// `RefCell` 允许 `&self` 方法内 borrow_mut 发送命令。
    connection: RefCell<Option<Rc<RefCell<Connection>>>>,
    /// EventEmitter inner。
    events: Rc<EventEmitterInner>,
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field("target_id", &self.target_id)
            .field("is_service_worker", &self.is_service_worker.get())
            .field("frame_count", &self.frames_map.borrow().len())
            .field("worker_count", &self.workers.borrow().len())
            .field("closed", &self.closed.borrow())
            .field("has_connection", &self.connection.borrow().is_some())
            .finish()
    }
}

impl Page {
    /// 构造 Page(由 BrowserContext::new_page 调用)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn new(target_id: impl Into<String>, context: Weak<BrowserContext>) -> Self {
        Self::new_with_connection(target_id, context, None)
    }

    /// 构造 Page 并绑定 Connection(共享引用)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn new_with_connection(
        target_id: impl Into<String>,
        context: Weak<BrowserContext>,
        connection: Option<Rc<RefCell<Connection>>>,
    ) -> Self {
        let target_id = target_id.into();
        let ctx_clone = context.clone();
        let main_frame = Rc::new(Frame::new("MAIN", true, Weak::new()));
        let mut frames = HashMap::new();
        frames.insert("MAIN".to_string(), main_frame.clone());

        Self {
            target_id,
            is_service_worker: Cell::new(false),
            main_frame,
            frames_map: RefCell::new(frames),
            workers: RefCell::new(Vec::new()),
            viewport: RefCell::new(None),
            target_info: RefCell::new(TargetInfo::default()),
            mouse: Mouse::new(),
            keyboard: Keyboard::new(),
            touchscreen: Touchscreen::new(),
            coverage: Coverage::new(),
            tracing: Tracing::new(),
            accessibility: Accessibility::new(),
            closed: RefCell::new(false),
            default_timeout: RefCell::new(None),
            default_nav_timeout: RefCell::new(None),
            opener: RefCell::new(None),
            context: ctx_clone,
            connection: RefCell::new(connection),
            events: Rc::new(EventEmitterInner::new()),
        }
    }

    /// 是否已关闭(本地标记)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// 标记关闭(本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_closed(&self, v: bool) {
        *self.closed.borrow_mut() = v;
    }

    /// Target ID。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// 是否 service worker。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn is_service_worker(&self) -> bool {
        self.is_service_worker.get()
    }

    /// 设置 is_service_worker(targetCreated 时根据 type 填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_is_service_worker(&self, v: bool) {
        self.is_service_worker.set(v);
    }

    /// 主 Frame(Rc clone)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn main_frame(&self) -> Rc<Frame> {
        self.main_frame.clone()
    }

    /// 所有 Frame 列表(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn frames(&self) -> Vec<Rc<Frame>> {
        self.frames_map.borrow().values().cloned().collect()
    }

    /// Frame 总数。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn frames_count(&self) -> usize {
        self.frames_map.borrow().len()
    }

    /// 按 ID 查找 frame。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn frame_by_id(&self, id: &str) -> Option<Rc<Frame>> {
        self.frames_map.borrow().get(id).cloned()
    }

    /// 添加 frame(frameAttached 事件触发)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn add_frame(&self, id: impl Into<String>, frame: Rc<Frame>) {
        self.frames_map.borrow_mut().insert(id.into(), frame);
    }

    /// 移除 frame(frameDetached 事件触发)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn remove_frame(&self, id: &str) {
        self.frames_map.borrow_mut().remove(id);
    }

    /// Workers(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn workers(&self) -> Vec<Worker> {
        self.workers.borrow().clone()
    }

    /// Worker 数量。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn workers_count(&self) -> usize {
        self.workers.borrow().len()
    }

    /// 添加 worker(attachedToTarget 事件触发)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn add_worker(&self, w: Worker) {
        self.workers.borrow_mut().push(w);
    }

    /// Viewport(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn viewport(&self) -> Option<Viewport> {
        *self.viewport.borrow()
    }

    /// Viewport size(Width, Height)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn viewport_size(&self) -> Option<(u32, u32)> {
        self.viewport.borrow().map(|v| (v.width, v.height))
    }

    /// 设置 viewport(本地缓存;setViewport 命令成功后调用)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_viewport(&self, v: Viewport) {
        *self.viewport.borrow_mut() = Some(v);
    }

    /// TargetInfo(本地缓存克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn target_info(&self) -> TargetInfo {
        self.target_info.borrow().clone()
    }

    /// 设置 target info。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_target_info(&self, info: TargetInfo) {
        *self.target_info.borrow_mut() = info;
    }

    /// 引用 target info(避免 clone)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn target(&self) -> std::cell::Ref<'_, TargetInfo> {
        self.target_info.borrow()
    }

    /// 引用 mouse(本地实例)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn mouse(&self) -> &Mouse {
        &self.mouse
    }

    /// 引用 keyboard。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    /// 引用 touchscreen。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn touchscreen(&self) -> &Touchscreen {
        &self.touchscreen
    }

    /// 引用 coverage。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    /// 引用 tracing。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn tracing(&self) -> &Tracing {
        &self.tracing
    }

    /// 引用 accessibility。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn accessibility(&self) -> &Accessibility {
        &self.accessibility
    }

    /// 默认 timeout(Page.$ 等命令使用)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn default_timeout(&self) -> Option<Duration> {
        *self.default_timeout.borrow()
    }

    /// 默认 navigation timeout。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn default_navigation_timeout(&self) -> Option<Duration> {
        *self.default_nav_timeout.borrow()
    }

    /// 设置默认 timeout。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_default_timeout(&self, ms: u64) {
        *self.default_timeout.borrow_mut() = Some(Duration::from_millis(ms));
    }

    /// 设置默认 navigation timeout。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_default_navigation_timeout(&self, ms: u64) {
        *self.default_nav_timeout.borrow_mut() = Some(Duration::from_millis(ms));
    }

    /// 清除默认 timeout。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn clear_default_timeout(&self) {
        *self.default_timeout.borrow_mut() = None;
    }

    /// 清除默认 navigation timeout。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn clear_default_navigation_timeout(&self) {
        *self.default_nav_timeout.borrow_mut() = None;
    }

    /// Opener Page(weak,可能已 drop)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn opener(&self) -> Option<Rc<Page>> {
        self.opener.borrow().as_ref().and_then(|w| w.upgrade())
    }

    /// 设置 opener。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_opener(&self, opener: Weak<Page>) {
        *self.opener.borrow_mut() = Some(opener);
    }

    /// 所属 BrowserContext(weak upgrade)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn browser_context(&self) -> Option<Rc<BrowserContext>> {
        self.context.upgrade()
    }

    /// 所属 Browser(context → browser)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn browser(&self) -> Option<Rc<HighLevelBrowser>> {
        self.context.upgrade().map(|c| c.browser())
    }

    /// 主 frame URL(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn url(&self) -> String {
        self.main_frame.url()
    }

    /// 设置主 frame URL(frameNavigated 事件触发)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_url(&self, url: impl Into<String>) {
        self.main_frame.set_url(url);
    }

    /// 引用 EventEmitter inner(测试用)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn events_inner(&self) -> &Rc<EventEmitterInner> {
        &self.events
    }

    // ─── Connection 管理 ──────────────────────────────────────────────────

    /// 是否绑定 Connection(可发送 CDP 命令)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn has_connection(&self) -> bool {
        self.connection.borrow().is_some()
    }

    /// 设置 Connection(共享引用)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_connection(&self, conn: Rc<RefCell<Connection>>) {
        *self.connection.borrow_mut() = Some(conn);
    }

    // ─── CDP 命令 method ──────────────────────────────────────────────────

    /// 导航到指定 URL(CDP `Page.navigate`)。
    ///
    /// # 错误
    /// - `CdpError::ConnectionClosed`: 未绑定 Connection 或连接已断开
    /// - `CdpError::ProtocolError`: 导航失败(无效 URL / 网络错误)
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn goto(&self, url: &str) -> crate::error::Result<Value> {
        self.send_cdp_command("Page.navigate", serde_json::json!({"url": url}))
    }

    /// 在页面上下文中执行 JavaScript 表达式(CDP `Runtime.evaluate`)。
    ///
    /// # 错误
    /// - `CdpError::ConnectionClosed`: 未绑定 Connection
    /// - `CdpError::ProtocolError`: JS 执行异常
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn evaluate(&self, expression: &str) -> crate::error::Result<Value> {
        self.send_cdp_command(
            "Runtime.evaluate",
            serde_json::json!({
                "expression": expression,
                "returnByValue": true,
            }),
        )
    }

    /// 截取页面截图(CDP `Page.captureScreenshot`)。
    ///
    /// 返回 base64 编码的截图数据。
    ///
    /// # 错误
    /// - `CdpError::ConnectionClosed`: 未绑定 Connection
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn screenshot(&self) -> crate::error::Result<Value> {
        self.send_cdp_command(
            "Page.captureScreenshot",
            serde_json::json!({"format": "png"}),
        )
    }

    /// 截取页面截图(指定格式)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn screenshot_with_format(&self, format: &str) -> crate::error::Result<Value> {
        self.send_cdp_command(
            "Page.captureScreenshot",
            serde_json::json!({"format": format}),
        )
    }

    /// 关闭页面(CDP `Page.close`)。
    ///
    /// 同时标记本地 `closed = true`。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn close(&self) -> crate::error::Result<Value> {
        let result = self.send_cdp_command("Page.close", serde_json::json!({}));
        if result.is_ok() {
            *self.closed.borrow_mut() = true;
        }
        result
    }

    /// 获取页面标题(CDP `Runtime.evaluate` 执行 `document.title`)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn title(&self) -> crate::error::Result<String> {
        let result = self.evaluate("document.title")?;
        Ok(result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    /// 设置页面 Viewport(CDP `Emulation.setDeviceMetricsOverride`)。
    ///
    /// 同时更新本地 viewport 缓存。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn set_viewport_cdp(&self, viewport: Viewport) -> crate::error::Result<Value> {
        let result = self.send_cdp_command(
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({
                "width": viewport.width,
                "height": viewport.height,
                "deviceScaleFactor": viewport.device_scale_factor,
                "mobile": viewport.is_mobile,
            }),
        );
        if result.is_ok() {
            *self.viewport.borrow_mut() = Some(viewport);
        }
        result
    }

    /// 发送任意 CDP 命令(底层)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn send_cdp_command(&self, method: &str, params: Value) -> crate::error::Result<Value> {
        match &*self.connection.borrow() {
            Some(conn) => conn.borrow_mut().send_command(method, params),
            None => Err(CdpError::ConnectionClosed),
        }
    }

    /// 发送 CDP 命令(带 session_id)。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn send_cdp_command_with_session(
        &self,
        method: &str,
        params: Value,
        session_id: &str,
    ) -> crate::error::Result<Value> {
        match &*self.connection.borrow() {
            Some(conn) => {
                conn.borrow_mut()
                    .send_command_with_session(method, params, Some(session_id))
            }
            None => Err(CdpError::ConnectionClosed),
        }
    }

    /// 接收一个 CDP 事件。
    ///
    /// @trace REQ-BAO-API-006 [class:Page]
    pub fn recv_cdp_event(&self) -> crate::error::Result<Option<crate::transport::CdpEvent>> {
        match &*self.connection.borrow() {
            Some(conn) => conn.borrow_mut().recv_event(),
            None => Err(CdpError::ConnectionClosed),
        }
    }
}

impl EventEmitter for Page {
    delegate_event_emitter!(self, events);
}

#[cfg(test)]
mod tests {
    use super::super::event_emitter::EventHandler;
    use super::*;

    fn make_browser_ctx() -> Rc<BrowserContext> {
        let browser = Rc::new(HighLevelBrowser::new_for_test("ws://x"));
        HighLevelBrowser::new_context_for_test(&browser)
    }

    fn make_page_with_ctx(ctx: Rc<BrowserContext>) -> Rc<Page> {
        Rc::new(Page::new("TARGET-1", Rc::downgrade(&ctx)))
    }

    #[test]
    fn new_page_initial_state() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        assert_eq!(p.target_id(), "TARGET-1");
        assert!(!p.is_closed());
        assert!(!p.is_service_worker());
        assert_eq!(p.frames_count(), 1);
        assert_eq!(p.workers_count(), 0);
        assert!(p.viewport().is_none());
        assert!(p.default_timeout().is_none());
        assert!(p.default_navigation_timeout().is_none());
        assert!(p.opener().is_none());
        assert!(!p.has_connection());
    }

    #[test]
    fn is_closed_flag() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        p.set_closed(true);
        assert!(p.is_closed());
    }

    #[test]
    fn main_frame_is_first_frame() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let mf = p.main_frame();
        assert!(mf.is_main_frame());
        assert_eq!(p.frames().len(), 1);
    }

    #[test]
    fn add_remove_frames() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let f2 = Rc::new(Frame::new("F2", false, Rc::downgrade(&p)));
        p.add_frame("F2", f2);
        assert_eq!(p.frames_count(), 2);
        assert!(p.frame_by_id("F2").is_some());
        p.remove_frame("F2");
        assert_eq!(p.frames_count(), 1);
        assert!(p.frame_by_id("F2").is_none());
    }

    #[test]
    fn add_workers() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        p.add_worker(Worker::new("W1", "https://example.com/w.js", "worker"));
        assert_eq!(p.workers_count(), 1);
        assert_eq!(p.workers()[0].target_id(), "W1");
    }

    #[test]
    fn viewport_round_trip() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        assert!(p.viewport().is_none());
        p.set_viewport(Viewport {
            width: 1920,
            height: 1080,
            ..Default::default()
        });
        let v = p.viewport().unwrap();
        assert_eq!(v.width, 1920);
        assert_eq!(v.height, 1080);
        assert_eq!(p.viewport_size(), Some((1920, 1080)));
    }

    #[test]
    fn default_timeouts() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        p.set_default_timeout(5000);
        p.set_default_navigation_timeout(10000);
        assert_eq!(p.default_timeout(), Some(Duration::from_millis(5000)));
        assert_eq!(
            p.default_navigation_timeout(),
            Some(Duration::from_millis(10000))
        );
        p.clear_default_timeout();
        assert!(p.default_timeout().is_none());
    }

    #[test]
    fn target_info_round_trip() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let info = TargetInfo {
            target_id: "TARGET-1".into(),
            type_str: "page".into(),
            title: "Hello".into(),
            url: "https://example.com".into(),
            attached: true,
            opener_id: None,
            browser_context_id: None,
        };
        p.set_target_info(info);
        let got = p.target_info();
        assert_eq!(got.title, "Hello");
        assert_eq!(got.url, "https://example.com");
        let r = p.target();
        assert_eq!(r.type_str, "page");
    }

    #[test]
    fn input_devices_local() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        p.mouse().set_position(10.0, 20.0);
        assert_eq!(p.mouse().current_x(), 10.0);
        p.keyboard().set_modifier(1, true); // Shift
        assert!(p.keyboard().is_shift_pressed());
        p.touchscreen()
            .add_touch(crate::api::touchscreen::TouchPoint::default());
        assert_eq!(p.touchscreen().touch_count(), 1);
    }

    #[test]
    fn coverage_local() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        assert!(!p.coverage().is_started());
        p.coverage().set_js_started(true);
        assert!(p.coverage().is_started());
    }

    #[test]
    fn tracing_local() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        assert!(!p.tracing().is_started());
        p.tracing().set_started(true);
        assert!(p.tracing().is_started());
    }

    #[test]
    fn accessibility_local() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        assert!(!p.accessibility().has_snapshot());
        p.accessibility()
            .add_node(crate::api::accessibility::AXNode {
                node_id: "1".into(),
                role: "button".into(),
                ..Default::default()
            });
        assert_eq!(p.accessibility().node_count(), 1);
    }

    #[test]
    fn opener_link() {
        let ctx = make_browser_ctx();
        let opener = make_page_with_ctx(ctx.clone());
        let child = Rc::new(Page::new("TARGET-2", Rc::downgrade(&ctx)));
        child.set_opener(Rc::downgrade(&opener));
        assert_eq!(
            child.opener().map(|p| p.target_id().to_string()),
            Some("TARGET-1".into())
        );
    }

    #[test]
    fn browser_context_lookup() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx.clone());
        let got = p.browser_context();
        assert!(got.is_some());
        assert!(Rc::ptr_eq(&got.unwrap(), &ctx));
    }

    #[test]
    fn url_delegates_to_main_frame() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        assert_eq!(p.url(), "");
        p.set_url("https://example.com");
        assert_eq!(p.url(), "https://example.com");
    }

    #[test]
    fn event_emitter_via_trait() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = counter.clone();
        let h: EventHandler = std::sync::Arc::new(move |_| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        p.on("load", h);
        p.emit("load", &[]);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(p.listener_count("load"), 1);
    }

    #[test]
    fn event_emitter_off() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let h: EventHandler = std::sync::Arc::new(|_| {});
        let id = p.on("x", h);
        assert_eq!(p.listener_count("x"), 1);
        p.off("x", id);
        assert_eq!(p.listener_count("x"), 0);
    }

    #[test]
    fn event_emitter_remove_all() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let h1: EventHandler = std::sync::Arc::new(|_| {});
        let h2: EventHandler = std::sync::Arc::new(|_| {});
        p.on("a", h1);
        p.on("b", h2);
        p.remove_all_listeners(Some("a"));
        assert_eq!(p.listener_count("a"), 0);
        assert_eq!(p.listener_count("b"), 1);
        p.remove_all_listeners(None);
        assert_eq!(p.listener_count("b"), 0);
    }

    #[test]
    fn cdp_command_without_connection_returns_error() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let err = p.goto("https://example.com").unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn cdp_evaluate_without_connection_returns_error() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let err = p.evaluate("1+1").unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn cdp_screenshot_without_connection_returns_error() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let err = p.screenshot().unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn cdp_close_without_connection_returns_error() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        let err = p.close().unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
        assert!(!p.is_closed());
    }

    #[test]
    fn page_with_connection_has_connection() {
        let ctx = make_browser_ctx();
        // Create a mock connection via InMemoryTransport + MockBridge
        use crate::transport::{InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport};
        use std::sync::Arc;

        struct MockBridge;
        impl InMemoryBridge for MockBridge {
            fn dispatch_command(
                &self,
                _m: &str,
                _p: Value,
                _s: Option<&str>,
            ) -> InMemoryBridgeResponse {
                InMemoryBridgeResponse::Ok(serde_json::json!({"result": 42}))
            }
        }

        let bridge: Arc<dyn InMemoryBridge> = Arc::new(MockBridge);
        let transport = InMemoryTransport::new(bridge);
        let conn = Rc::new(RefCell::new(Connection::from_transport(Box::new(
            transport,
        ))));

        let p = Rc::new(Page::new_with_connection(
            "TARGET-1",
            Rc::downgrade(&ctx),
            Some(conn),
        ));
        assert!(p.has_connection());
    }

    #[test]
    fn set_connection_on_existing_page() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        assert!(!p.has_connection());

        use crate::transport::{InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport};
        use std::sync::Arc;

        struct MockBridge;
        impl InMemoryBridge for MockBridge {
            fn dispatch_command(
                &self,
                _m: &str,
                _p: Value,
                _s: Option<&str>,
            ) -> InMemoryBridgeResponse {
                InMemoryBridgeResponse::Ok(Value::Null)
            }
        }

        let bridge: Arc<dyn InMemoryBridge> = Arc::new(MockBridge);
        let transport = InMemoryTransport::new(bridge);
        let conn = Rc::new(RefCell::new(Connection::from_transport(Box::new(
            transport,
        ))));

        p.set_connection(conn);
        assert!(p.has_connection());
    }

    #[test]
    fn set_is_service_worker_actually_sets_value() {
        let ctx = make_browser_ctx();
        let p = make_page_with_ctx(ctx);
        assert!(!p.is_service_worker());
        p.set_is_service_worker(true);
        assert!(p.is_service_worker());
        p.set_is_service_worker(false);
        assert!(!p.is_service_worker());
    }
}
