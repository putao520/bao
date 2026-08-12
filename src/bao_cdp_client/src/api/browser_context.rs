//! BrowserContext — 浏览器上下文(incognito / 隔离)。
//!
//! D 类 method 全部本地状态:
//! - `browser() -> Rc<Browser>`
//! - `pages() -> Vec<Rc<Page>>`
//! - `is_incognito() -> bool`
//! - `override_permissions(origin, perms)`(本地状态)
//! - `clear_permission_overrides()`(本地状态)
//! - `permission_overrides() -> HashMap<origin, Vec<PermissionOverride>>`
//! - `on/once/off/remove_all_listeners/listener_count/emit`(EventEmitter)
//! - `close()`(关闭所有 page)
//!
//! @trace REQ-BAO-API-006 [class:BrowserContext]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use super::event_emitter::{EventEmitter, EventEmitterInner};
use super::page::Page;
use crate::api::browser::Browser as HighLevelBrowser;

/// Permission override 类型(CDP Permission name)。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PermissionOverride {
    Geolocation,
    Notifications,
    Camera,
    Microphone,
    ClipboardReadWrite,
    ClipboardSanitizedWrite,
    /// 其他任意 permission name。
    Other(String),
}

impl PermissionOverride {
    pub fn as_str(&self) -> &str {
        match self {
            PermissionOverride::Geolocation => "geolocation",
            PermissionOverride::Notifications => "notifications",
            PermissionOverride::Camera => "camera",
            PermissionOverride::Microphone => "microphone",
            PermissionOverride::ClipboardReadWrite => "clipboard-read",
            PermissionOverride::ClipboardSanitizedWrite => "clipboard-write",
            PermissionOverride::Other(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "geolocation" => PermissionOverride::Geolocation,
            "notifications" => PermissionOverride::Notifications,
            "camera" => PermissionOverride::Camera,
            "microphone" => PermissionOverride::Microphone,
            "clipboard-read" => PermissionOverride::ClipboardReadWrite,
            "clipboard-write" => PermissionOverride::ClipboardSanitizedWrite,
            other => PermissionOverride::Other(other.to_string()),
        }
    }
}

/// ContextOptions — BrowserContext 配置。
#[derive(Debug, Clone, Default)]
pub struct ContextOptions {
    /// 是否 incognito。
    pub incognito: bool,
    /// viewport(传递给新 page)。
    pub viewport: Option<super::page::Viewport>,
    /// locale。
    pub locale: Option<String>,
    /// timezone_id。
    pub timezone_id: Option<String>,
    /// user_agent override。
    pub user_agent: Option<String>,
    /// http credentials。
    pub http_credentials: Option<HashMap<String, String>>,
}

/// BrowserContext 本地状态。
///
/// @trace REQ-BAO-API-006 [class:BrowserContext]
pub struct BrowserContext {
    /// 所属 Browser(Rc,持有强引用;close 时 drop Page)。
    browser: Weak<HighLevelBrowser>,
    /// Pages 列表(本地持有)。
    pages: RefCell<Vec<Rc<Page>>>,
    /// Options。
    opts: RefCell<ContextOptions>,
    /// Permission overrides: origin → permissions。
    permission_overrides: RefCell<HashMap<String, Vec<PermissionOverride>>>,
    /// 是否已关闭。
    closed: RefCell<bool>,
    /// EventEmitter inner。
    events: Rc<EventEmitterInner>,
    /// Context ID(CDP Browser.createBrowserContext 返回的 id)。
    context_id: RefCell<Option<String>>,
}

impl std::fmt::Debug for BrowserContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserContext")
            .field("page_count", &self.pages.borrow().len())
            .field("closed", &self.closed.borrow())
            .field("context_id", &self.context_id.borrow())
            .finish()
    }
}

impl BrowserContext {
    /// 构造 BrowserContext。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn new(browser: Weak<HighLevelBrowser>, opts: ContextOptions) -> Self {
        Self {
            browser,
            pages: RefCell::new(Vec::new()),
            opts: RefCell::new(opts),
            permission_overrides: RefCell::new(HashMap::new()),
            closed: RefCell::new(false),
            events: Rc::new(EventEmitterInner::new()),
            context_id: RefCell::new(None),
        }
    }

    /// 所属 Browser(weak upgrade)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn browser(&self) -> Rc<HighLevelBrowser> {
        // 实践中 Browser 持有 BrowserContext 的强引用,所以 upgrade 总成功。
        // 若 Browser 已 drop,context 也已 drop,所以这是死代码路径(返回临时强引用
        // 不可行)。这里 unwrap_or_else panic 是合理的契约保证。
        self.browser
            .upgrade()
            .expect("BrowserContext::browser: parent Browser dropped")
    }

    /// Pages 列表(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn pages(&self) -> Vec<Rc<Page>> {
        self.pages.borrow().clone()
    }

    /// Page 数量。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn pages_count(&self) -> usize {
        self.pages.borrow().len()
    }

    /// 是否 incognito(从 opts 读)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn is_incognito(&self) -> bool {
        self.opts.borrow().incognito
    }

    /// Context options(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn options(&self) -> ContextOptions {
        self.opts.borrow().clone()
    }

    /// 设置 context id(CDP Browser.createBrowserContext 响应填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn set_context_id(&self, id: impl Into<String>) {
        *self.context_id.borrow_mut() = Some(id.into());
    }

    /// Context ID。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn context_id(&self) -> Option<String> {
        self.context_id.borrow().clone()
    }

    /// 是否已关闭。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// 覆盖指定 origin 的 permissions(本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn override_permissions(&self, origin: impl Into<String>, perms: Vec<PermissionOverride>) {
        self.permission_overrides
            .borrow_mut()
            .insert(origin.into(), perms);
    }

    /// 清除所有 permission overrides(本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn clear_permission_overrides(&self) {
        self.permission_overrides.borrow_mut().clear();
    }

    /// 返回所有 permission overrides(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn permission_overrides(&self) -> HashMap<String, Vec<PermissionOverride>> {
        self.permission_overrides.borrow().clone()
    }

    /// 查找 origin 的 permission overrides(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn permissions_for(&self, origin: &str) -> Vec<PermissionOverride> {
        self.permission_overrides
            .borrow()
            .get(origin)
            .cloned()
            .unwrap_or_default()
    }

    /// 添加 Page(由 BrowserContext::new_page 调用)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn add_page(&self, p: Rc<Page>) {
        self.pages.borrow_mut().push(p);
    }

    /// 移除 Page(targetDestroyed / Page.close 后调用)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn remove_page(&self, target_id: &str) {
        self.pages
            .borrow_mut()
            .retain(|p| p.target_id() != target_id);
    }

    /// 按 target_id 查找 Page。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn page_by_target(&self, target_id: &str) -> Option<Rc<Page>> {
        self.pages
            .borrow()
            .iter()
            .find(|p| p.target_id() == target_id)
            .cloned()
    }

    /// EventEmitter inner 引用。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn events_inner(&self) -> &Rc<EventEmitterInner> {
        &self.events
    }

    /// Close — 标记本地 + 关闭所有 page。
    ///
    /// 注:本 TASK 仅本地状态;真正调用 Browser.disposeBrowserContext 走
    /// transport 由调用方在 Browser 上完成。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    pub fn close(&self) {
        *self.closed.borrow_mut() = true;
        for p in self.pages.borrow().iter() {
            p.set_closed(true);
        }
        self.pages.borrow_mut().clear();
    }
}

impl EventEmitter for BrowserContext {
    delegate_event_emitter!(self, events);
}

// ============================================================================
// 测试辅助方法
// ============================================================================

impl BrowserContext {
    /// 测试辅助:在 `Rc<BrowserContext>` 上创建 Page(自引用 weak)。
    ///
    /// @trace REQ-BAO-API-006 [class:BrowserContext]
    #[doc(hidden)]
    pub fn new_page_for_test(self_rc: &Rc<BrowserContext>) -> Rc<Page> {
        super::browser::new_page_on_context(self_rc)
    }
}

#[cfg(test)]
mod tests {
    use super::super::event_emitter::EventHandler;
    use super::*;

    fn make_browser() -> Rc<HighLevelBrowser> {
        Rc::new(HighLevelBrowser::new_for_test("ws://x"))
    }

    fn make_ctx(browser: Rc<HighLevelBrowser>) -> Rc<BrowserContext> {
        HighLevelBrowser::new_context_for_test(&browser)
    }

    #[test]
    fn new_initial_state() {
        let b = make_browser();
        let ctx = make_ctx(b);
        assert!(!ctx.is_closed());
        assert!(!ctx.is_incognito());
        assert_eq!(ctx.pages_count(), 0);
        assert!(ctx.permission_overrides().is_empty());
        assert!(ctx.context_id().is_none());
    }

    #[test]
    fn browser_lookup() {
        let b = make_browser();
        let ctx = make_ctx(b.clone());
        let got = ctx.browser();
        assert!(Rc::ptr_eq(&got, &b));
    }

    #[test]
    fn context_id_round_trip() {
        let b = make_browser();
        let ctx = make_ctx(b);
        ctx.set_context_id("CTX-1");
        assert_eq!(ctx.context_id(), Some("CTX-1".into()));
    }

    #[test]
    fn override_permissions_local() {
        let b = make_browser();
        let ctx = make_ctx(b);
        ctx.override_permissions("https://example.com", vec![PermissionOverride::Geolocation]);
        assert_eq!(ctx.permission_overrides().len(), 1);
        let perms = ctx.permissions_for("https://example.com");
        assert_eq!(perms.len(), 1);
        assert_eq!(perms[0], PermissionOverride::Geolocation);
    }

    #[test]
    fn clear_permission_overrides_local() {
        let b = make_browser();
        let ctx = make_ctx(b);
        ctx.override_permissions("https://a", vec![PermissionOverride::Camera]);
        ctx.override_permissions("https://b", vec![PermissionOverride::Microphone]);
        assert_eq!(ctx.permission_overrides().len(), 2);
        ctx.clear_permission_overrides();
        assert!(ctx.permission_overrides().is_empty());
    }

    #[test]
    fn incognito_option() {
        let b = make_browser();
        let ctx = Rc::new(BrowserContext::new(
            Rc::downgrade(&b),
            ContextOptions {
                incognito: true,
                ..Default::default()
            },
        ));
        assert!(ctx.is_incognito());
    }

    #[test]
    fn close_marks_pages_and_self() {
        let b = make_browser();
        let ctx = make_ctx(b.clone());
        let p = BrowserContext::new_page_for_test(&ctx);
        assert_eq!(ctx.pages_count(), 1);
        ctx.close();
        assert!(ctx.is_closed());
        assert_eq!(ctx.pages_count(), 0);
        assert!(p.is_closed());
    }

    #[test]
    fn add_remove_page() {
        let b = make_browser();
        let ctx = make_ctx(b.clone());
        let p = BrowserContext::new_page_for_test(&ctx);
        assert_eq!(ctx.pages_count(), 1);
        ctx.remove_page(p.target_id());
        assert_eq!(ctx.pages_count(), 0);
    }

    #[test]
    fn page_by_target_lookup() {
        let b = make_browser();
        let ctx = make_ctx(b.clone());
        let p = BrowserContext::new_page_for_test(&ctx);
        let got = ctx.page_by_target(p.target_id());
        assert!(got.is_some());
        assert!(Rc::ptr_eq(&got.unwrap(), &p));
    }

    #[test]
    fn event_emitter_via_trait() {
        let b = make_browser();
        let ctx = make_ctx(b);
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = counter.clone();
        let h: EventHandler = std::sync::Arc::new(move |_| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        ctx.on("page", h);
        ctx.emit("page", &[]);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(ctx.listener_count("page"), 1);
    }

    #[test]
    fn permission_override_from_str() {
        assert_eq!(
            PermissionOverride::from_str("geolocation"),
            PermissionOverride::Geolocation
        );
        assert_eq!(
            PermissionOverride::from_str("notifications"),
            PermissionOverride::Notifications
        );
        assert_eq!(
            PermissionOverride::from_str("custom"),
            PermissionOverride::Other("custom".into())
        );
    }

    #[test]
    fn permission_override_as_str() {
        assert_eq!(PermissionOverride::Camera.as_str(), "camera");
        assert_eq!(PermissionOverride::Other("foo".into()).as_str(), "foo");
    }
}
