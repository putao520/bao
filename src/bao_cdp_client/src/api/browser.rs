//! Browser — 高层 Browser 类(Playwright 风格)。
//!
//! D 类 method(全部本地状态,无 CDP 往返):
//! - `is_connected() -> bool`
//! - `process() -> Option<Pid>`
//! - `ws_endpoint() -> &str`
//! - `version() -> Option<String>`(本地缓存)
//! - `user_agent() -> Option<String>`(本地缓存)
//! - `disconnect()`(本地标记)
//! - `close()`(关闭所有 context + 标记断开)
//! - `contexts() -> Vec<Rc<BrowserContext>>`
//! - `context_count() -> usize`
//! - `on/once/off/remove_all_listeners/listener_count/emit`(EventEmitter)
//!
//! 非 D 类 method(new_context / new_page)需要自引用(`Rc<Browser>`),
//! 通过 [`new_context_on_rc`] / [`new_page_on_rc`] 辅助函数实现(避免
//! self-referential struct 复杂性)。
//!
//! @trace REQ-BAO-API-006 [class:Browser]

use std::cell::RefCell;
use std::rc::Rc;

use crate::connection::Connection;

use super::browser_context::{BrowserContext, ContextOptions};
use super::event_emitter::{EventEmitter, EventEmitterInner};
use super::page::Page;

/// Process ID 类型。
pub type Pid = u32;

/// BrowserOptions — Browser 构造配置。
#[derive(Debug, Clone, Default)]
pub struct BrowserOptions {
    pub ws_endpoint: String,
    pub initial_version: Option<String>,
    pub initial_user_agent: Option<String>,
    pub initial_pid: Option<Pid>,
}

/// Browser 本地状态(高层)。
///
/// @trace REQ-BAO-API-006 [class:Browser]
pub struct Browser {
    /// WebSocket endpoint URL(或 memory URL)。
    ws_endpoint: String,
    /// 缓存的 Browser.getVersion 响应(本地缓存;由调用方在 transport 完成后填入)。
    version: RefCell<Option<String>>,
    /// 缓存的 User-Agent。
    user_agent: RefCell<Option<String>>,
    /// 进程 PID(若为外部 Chrome)。
    pid: RefCell<Option<Pid>>,
    /// 是否已断开连接(transport close 或 disconnected 事件后置 true)。
    disconnected: RefCell<bool>,
    /// Contexts 列表(本地)。
    contexts: RefCell<Vec<Rc<BrowserContext>>>,
    /// Connection 引用(共享 — 所有 Page 共用同一 CDP 连接)。
    /// `None` 表示未连接(测试用 / 占位 Browser)。
    connection: RefCell<Option<Rc<RefCell<Connection>>>>,
    /// EventEmitter inner。
    events: Rc<EventEmitterInner>,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser")
            .field("ws_endpoint", &self.ws_endpoint)
            .field("version", &self.version.borrow())
            .field("user_agent", &self.user_agent.borrow())
            .field("disconnected", &self.disconnected.borrow())
            .field("context_count", &self.contexts.borrow().len())
            .field("has_connection", &self.connection.borrow().is_some())
            .finish()
    }
}

impl Browser {
    /// 构造 Browser(传入 ws_endpoint / 初始 options)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn new(opts: BrowserOptions) -> Self {
        Self {
            ws_endpoint: opts.ws_endpoint,
            version: RefCell::new(opts.initial_version),
            user_agent: RefCell::new(opts.initial_user_agent),
            pid: RefCell::new(opts.initial_pid),
            disconnected: RefCell::new(false),
            contexts: RefCell::new(Vec::new()),
            connection: RefCell::new(None),
            events: Rc::new(EventEmitterInner::new()),
        }
    }

    /// 测试构造(简化)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn new_for_test(ws_endpoint: &str) -> Self {
        Self::new(BrowserOptions {
            ws_endpoint: ws_endpoint.to_string(),
            initial_version: None,
            initial_user_agent: None,
            initial_pid: None,
        })
    }

    /// 是否已连接(transport 未关闭)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn is_connected(&self) -> bool {
        !*self.disconnected.borrow()
    }

    /// 标记已断开(transport close / disconnected 事件触发)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn set_disconnected(&self) {
        *self.disconnected.borrow_mut() = true;
    }

    /// 进程 PID(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn process(&self) -> Option<Pid> {
        *self.pid.borrow()
    }

    /// 设置 PID。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn set_pid(&self, pid: Pid) {
        *self.pid.borrow_mut() = Some(pid);
    }

    /// WebSocket endpoint。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn ws_endpoint(&self) -> &str {
        &self.ws_endpoint
    }

    /// Version(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn version(&self) -> Option<String> {
        self.version.borrow().clone()
    }

    /// 设置 version(Browser.getVersion 响应填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn set_version(&self, v: impl Into<String>) {
        *self.version.borrow_mut() = Some(v.into());
    }

    /// User-Agent(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn user_agent(&self) -> Option<String> {
        self.user_agent.borrow().clone()
    }

    /// 设置 user agent。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn set_user_agent(&self, ua: impl Into<String>) {
        *self.user_agent.borrow_mut() = Some(ua.into());
    }

    /// 所有 contexts(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn contexts(&self) -> Vec<Rc<BrowserContext>> {
        self.contexts.borrow().clone()
    }

    /// Context 数量。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn context_count(&self) -> usize {
        self.contexts.borrow().len()
    }

    /// Disconnect — 标记本地断开。
    ///
    /// 注:本 TASK 仅本地状态。真正调用 transport.close 由调用方在外部封装。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn disconnect(&self) {
        *self.disconnected.borrow_mut() = true;
    }

    /// Close — 标记断开 + 关闭所有 context + 关闭所有 page。
    ///
    /// 注:本 TASK 仅本地状态。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn close(&self) {
        *self.disconnected.borrow_mut() = true;
        let contexts = self.contexts.borrow().clone();
        for ctx in contexts.iter() {
            ctx.close();
        }
        self.contexts.borrow_mut().clear();
    }

    /// EventEmitter inner 引用。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn events_inner(&self) -> &Rc<EventEmitterInner> {
        &self.events
    }

    /// 是否绑定 Connection(可发送 CDP 命令)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn has_connection(&self) -> bool {
        self.connection.borrow().is_some()
    }

    /// 设置 Connection(共享引用)。
    ///
    /// Browser 持有 Connection,所有通过此 Browser 创建的 Page 共享同一连接。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn set_connection(&self, conn: Rc<RefCell<Connection>>) {
        *self.connection.borrow_mut() = Some(conn);
    }

    /// 获取 Connection 的共享引用(若已绑定)。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    pub fn connection(&self) -> Option<Rc<RefCell<Connection>>> {
        self.connection.borrow().clone()
    }

    /// 测试辅助:创建 BrowserContext(默认 options)。
    ///
    /// 注:由于 Rust 的所有权模型,`new_context` 需要 `Rc<Browser>` 才能
    /// 创建持有 Browser Weak 引用的 BrowserContext。本方法把 self 包装
    /// 检查 + 调用 [`new_context_on_rc`],便于测试使用。
    ///
    /// 调用方约定:self 必须由 `Rc` 持有(否则 panic)。实际生产代码应直接用
    /// `Rc<Browser>` 调用 [`new_context_on_rc`]。
    ///
    /// @trace REQ-BAO-API-006 [class:Browser]
    #[doc(hidden)]
    pub fn new_context_for_test(self_rc: &Rc<Browser>) -> Rc<BrowserContext> {
        new_context_on_rc(self_rc, ContextOptions::default())
    }
}

impl EventEmitter for Browser {
    delegate_event_emitter!(self, events);
}

// ============================================================================
// 自引用辅助函数
// ============================================================================

/// 创建 BrowserContext(需要 `Rc<Browser>` 才能持有 Weak self 引用)。
///
/// @trace REQ-BAO-API-006 [class:Browser]
pub fn new_context_on_rc(this: &Rc<Browser>, opts: ContextOptions) -> Rc<BrowserContext> {
    let ctx = Rc::new(BrowserContext::new(Rc::downgrade(this), opts));
    this.contexts.borrow_mut().push(ctx.clone());
    ctx
}

/// 创建 Page(若 contexts 为空,自动创建 default context)。
///
/// @trace REQ-BAO-API-006 [class:Browser]
pub fn new_page_on_rc(this: &Rc<Browser>) -> Rc<Page> {
    let ctx = if this.contexts.borrow().is_empty() {
        new_context_on_rc(this, ContextOptions::default())
    } else {
        this.contexts.borrow()[0].clone()
    };
    new_page_on_context(&ctx)
}

/// 在 BrowserContext 上创建 Page。
///
/// 如果 Browser 绑定了 Connection,自动将共享连接传递给新 Page,
/// 使其可发送 CDP 命令。
///
/// @trace REQ-BAO-API-006 [class:BrowserContext]
pub fn new_page_on_context(ctx: &Rc<BrowserContext>) -> Rc<Page> {
    let target_id = format!("TARGET-{}", ctx.pages_count() + 1);
    let conn = ctx.browser().connection();
    let p = Rc::new(Page::new_with_connection(target_id, Rc::downgrade(ctx), conn));
    ctx.add_page(p.clone());
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::event_emitter::EventHandler;

    #[test]
    fn new_initial_state() {
        let b = Browser::new_for_test("ws://127.0.0.1:9222");
        assert!(b.is_connected());
        assert_eq!(b.ws_endpoint(), "ws://127.0.0.1:9222");
        assert!(b.version().is_none());
        assert!(b.user_agent().is_none());
        assert!(b.process().is_none());
        assert_eq!(b.context_count(), 0);
    }

    #[test]
    fn is_connected_default_true() {
        let b = Browser::new_for_test("ws://x");
        assert!(b.is_connected());
    }

    #[test]
    fn disconnect_marks_disconnected() {
        let b = Browser::new_for_test("ws://x");
        b.disconnect();
        assert!(!b.is_connected());
    }

    #[test]
    fn set_disconnected_idempotent() {
        let b = Browser::new_for_test("ws://x");
        b.set_disconnected();
        b.set_disconnected();
        assert!(!b.is_connected());
    }

    #[test]
    fn version_round_trip() {
        let b = Browser::new_for_test("ws://x");
        b.set_version("HeadlessChrome/120.0");
        assert_eq!(b.version(), Some("HeadlessChrome/120.0".into()));
    }

    #[test]
    fn user_agent_round_trip() {
        let b = Browser::new_for_test("ws://x");
        b.set_user_agent("Mozilla/5.0 (X11; Linux x86_64)");
        assert!(b.user_agent().unwrap().contains("Linux"));
    }

    #[test]
    fn pid_round_trip() {
        let b = Browser::new_for_test("ws://x");
        b.set_pid(12345);
        assert_eq!(b.process(), Some(12345));
    }

    #[test]
    fn close_clears_contexts_and_marks_disconnected() {
        let b = Rc::new(Browser::new_for_test("ws://x"));
        let _ctx = new_context_on_rc(&b, ContextOptions::default());
        assert_eq!(b.context_count(), 1);
        b.close();
        assert!(!b.is_connected());
        assert_eq!(b.context_count(), 0);
    }

    #[test]
    fn new_context_on_rc_incognito() {
        let b = Rc::new(Browser::new_for_test("ws://x"));
        let ctx = new_context_on_rc(&b, ContextOptions { incognito: true, ..Default::default() });
        assert_eq!(b.context_count(), 1);
        assert!(ctx.is_incognito());
    }

    #[test]
    fn new_page_on_rc_first_context() {
        let b = Rc::new(Browser::new_for_test("ws://x"));
        let p = new_page_on_rc(&b);
        assert_eq!(b.context_count(), 1);
        assert_eq!(p.target_id(), "TARGET-1");
    }

    #[test]
    fn new_page_on_rc_increments_target_id() {
        let b = Rc::new(Browser::new_for_test("ws://x"));
        let p1 = new_page_on_rc(&b);
        let p2 = new_page_on_rc(&b);
        assert_eq!(p1.target_id(), "TARGET-1");
        assert_eq!(p2.target_id(), "TARGET-2");
    }

    #[test]
    fn event_emitter_via_trait() {
        let b = Browser::new_for_test("ws://x");
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = counter.clone();
        let h: EventHandler = std::sync::Arc::new(move |_| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        b.on("disconnected", h);
        b.emit("disconnected", &[]);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(b.listener_count("disconnected"), 1);
    }

    #[test]
    fn event_emitter_remove_all() {
        let b = Browser::new_for_test("ws://x");
        let h: EventHandler = std::sync::Arc::new(|_| {});
        b.on("a", h.clone());
        b.on("b", h);
        assert_eq!(b.listener_count("a"), 1);
        b.remove_all_listeners(Some("a"));
        assert_eq!(b.listener_count("a"), 0);
        assert_eq!(b.listener_count("b"), 1);
        b.remove_all_listeners(None);
        assert_eq!(b.listener_count("b"), 0);
    }

    #[test]
    fn contexts_returns_cloned_list() {
        let b = Rc::new(Browser::new_for_test("ws://x"));
        new_context_on_rc(&b, ContextOptions::default());
        new_context_on_rc(&b, ContextOptions { incognito: true, ..Default::default() });
        let list = b.contexts();
        assert_eq!(list.len(), 2);
        assert!(!list[0].is_incognito());
        assert!(list[1].is_incognito());
    }

    #[test]
    fn new_page_on_context_propagates_connection() {
        use crate::connection::Connection;
        use crate::transport::{InMemoryTransport, InMemoryBridge, InMemoryBridgeResponse};
        use std::sync::Arc;

        struct MockBridge;
        impl InMemoryBridge for MockBridge {
            fn dispatch_command(&self, _m: &str, _p: serde_json::Value, _s: Option<&str>) -> InMemoryBridgeResponse {
                InMemoryBridgeResponse::Ok(serde_json::Value::Null)
            }
        }

        let b = Rc::new(Browser::new_for_test("ws://x"));
        assert!(!b.has_connection());

        // Attach connection to Browser.
        let bridge: Arc<dyn InMemoryBridge> = Arc::new(MockBridge);
        let transport = InMemoryTransport::new(bridge);
        let conn = Rc::new(RefCell::new(Connection::from_transport(Box::new(transport))));
        b.set_connection(conn);
        assert!(b.has_connection());

        // Create page via new_page_on_rc — should inherit connection.
        let p = new_page_on_rc(&b);
        assert!(p.has_connection());
    }

    #[test]
    fn new_page_on_context_without_connection_has_no_connection() {
        let b = Rc::new(Browser::new_for_test("ws://x"));
        assert!(!b.has_connection());
        let p = new_page_on_rc(&b);
        assert!(!p.has_connection());
    }
}
