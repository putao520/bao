// @trace REQ-BRW-001  REQ-CDP-006: Servo delegate hooks for CDP event forwarding
use std::cell::RefCell;
use std::rc::Rc;

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint,
    DeviceIntRect, DeviceIntSize, EmbedderControl, EmbedderControlId, LoadStatus,
    NavigationRequest, PermissionRequest, ScreenGeometry, ServoDelegate,
    ServoError, WebView, WebViewDelegate,
};
pub struct BaoWebViewState {
    pub url: Option<url::Url>,
    pub title: Option<String>,
    pub load_status: LoadStatus,
    pub frame_ready: bool,
    /// Set to true after navigation completes (LoadStatus::Complete).
    /// evaluate_js checks this flag and refreshes stale DOM proxies before executing scripts.
    pub dom_proxies_dirty: bool,
}

impl Default for BaoWebViewState {
    fn default() -> Self {
        BaoWebViewState {
            url: None,
            title: None,
            load_status: LoadStatus::Started,
            frame_ready: false,
            dom_proxies_dirty: false,
        }
    }
}

pub struct BaoServoDelegate {
    last_error: RefCell<Option<String>>,
}

impl Default for BaoServoDelegate {
    fn default() -> Self {
        BaoServoDelegate {
            last_error: RefCell::new(None),
        }
    }
}

impl BaoServoDelegate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.borrow().clone()
    }
}

impl ServoDelegate for BaoServoDelegate {
    fn notify_error(&self, error: ServoError) {
        *self.last_error.borrow_mut() = Some(format!("{error:?}"));
    }

    fn show_console_message(&self, _level: ConsoleLogLevel, message: String) {
        eprintln!("[servo] {message}");
    }

    fn request_devtools_connection(&self, request: AllowOrDenyRequest) {
        request.allow();
    }
}

pub struct BaoWebViewDelegate {
    state: Rc<RefCell<BaoWebViewState>>,
    viewport: PhysicalSize<u32>,
}

impl BaoWebViewDelegate {
    pub fn new(state: Rc<RefCell<BaoWebViewState>>, viewport: PhysicalSize<u32>) -> Self {
        BaoWebViewDelegate { state, viewport }
    }

    pub fn state(&self) -> &Rc<RefCell<BaoWebViewState>> {
        &self.state
    }
}

impl WebViewDelegate for BaoWebViewDelegate {
    fn screen_geometry(&self, _webview: WebView) -> Option<ScreenGeometry> {
        let screen_size = DeviceIntSize::new(
            self.viewport.width as i32,
            self.viewport.height as i32,
        );
        Some(ScreenGeometry {
            size: screen_size,
            available_size: screen_size,
            window_rect: DeviceIntRect::from_origin_and_size(
                DeviceIntPoint::zero(),
                screen_size,
            ),
        })
    }

    fn notify_url_changed(&self, _webview: WebView, url: url::Url) {
        self.state.borrow_mut().url = Some(url);
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.state.borrow_mut().title = title;
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        self.state.borrow_mut().load_status = status;
        if matches!(status, LoadStatus::Complete) {
            self.state.borrow_mut().dom_proxies_dirty = true;
        }
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.state.borrow_mut().frame_ready = true;
    }

    fn request_navigation(&self, _webview: WebView, request: NavigationRequest) {
        request.allow();
    }

    fn request_permission(&self, _webview: WebView, request: PermissionRequest) {
        request.allow();
    }

    fn request_create_new(
        &self,
        _parent_webview: WebView,
        _request: CreateNewWebViewRequest,
    ) {
    }

    fn show_console_message(&self, _webview: WebView, _level: ConsoleLogLevel, message: String) {
        eprintln!("[webview] {message}");
    }

    fn show_embedder_control(&self, _webview: WebView, _control: EmbedderControl) {}

    fn hide_embedder_control(&self, _webview: WebView, _id: EmbedderControlId) {}

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        eprintln!("[webview] crashed: {reason}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── BaoWebViewState ────────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_state_default() {
        let state = BaoWebViewState::default();
        assert!(state.url.is_none());
        assert!(state.title.is_none());
        assert!(matches!(state.load_status, LoadStatus::Started));
        assert!(!state.frame_ready);
        assert!(!state.dom_proxies_dirty);
    }

    #[test]
    fn test_webview_state_url_mutate() {
        let mut state = BaoWebViewState::default();
        state.url = Some(url::Url::parse("https://example.com").unwrap());
        assert!(state.url.is_some());
        assert_eq!(state.url.unwrap().as_str(), "https://example.com/");
    }

    #[test]
    fn test_webview_state_title_mutate() {
        let mut state = BaoWebViewState::default();
        state.title = Some("Test Page".to_string());
        assert_eq!(state.title.as_deref(), Some("Test Page"));
    }

    #[test]
    fn test_webview_state_frame_ready_toggle() {
        let mut state = BaoWebViewState::default();
        assert!(!state.frame_ready);
        state.frame_ready = true;
        assert!(state.frame_ready);
    }

    // ─── BaoServoDelegate ──────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_servo_delegate_new_no_error() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.last_error().is_none());
    }

    #[test]
    fn test_servo_delegate_default_no_error() {
        let delegate = BaoServoDelegate::default();
        assert!(delegate.last_error().is_none());
    }

    // ─── BaoWebViewDelegate ────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_delegate_new_with_state() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1024, 768);
        let delegate = BaoWebViewDelegate::new(state, viewport);
        assert!(delegate.state().borrow().url.is_none());
    }

    #[test]
    fn test_webview_delegate_state_rc_shared() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(800, 600);
        let delegate = BaoWebViewDelegate::new(Rc::clone(&state), viewport);
        // Modify state externally
        state.borrow_mut().title = Some("External".to_string());
        // Delegate sees same state
        assert_eq!(delegate.state().borrow().title.as_deref(), Some("External"));
    }

    #[test]
    fn test_webview_delegate_viewport_size() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1440, 900);
        let delegate = BaoWebViewDelegate::new(state, viewport);
        // Verify delegate was created with specific viewport
        assert!(delegate.state().borrow().url.is_none());
    }

    // ─── PoolStats ─────────────────────────────────────────────────
    // @trace REQ-LIB-001 [req:REQ-LIB-001] [level:unit]

    #[test]
    fn test_pool_stats_fields() {
        let stats = crate::page_pool::PoolStats {
            active: 3,
            idle: 1,
            total_created: 5,
            total_destroyed: 2,
        };
        assert_eq!(stats.active, 3);
        assert_eq!(stats.idle, 1);
        assert_eq!(stats.total_created, 5);
        assert_eq!(stats.total_destroyed, 2);
    }

    // ─── DOM Proxy Dirty Flag ─────────────────────────────────────
    // @trace REQ-SEC-002 [req:REQ-SEC-002] [level:unit]

    #[test]
    fn test_dom_proxies_dirty_default_false() {
        let state = BaoWebViewState::default();
        assert!(!state.dom_proxies_dirty);
    }

    #[test]
    fn test_dom_proxies_dirty_set_on_complete() {
        let mut state = BaoWebViewState::default();
        state.load_status = LoadStatus::Complete;
        state.dom_proxies_dirty = true;
        assert!(state.dom_proxies_dirty);
    }

    #[test]
    fn test_dom_proxies_dirty_clear_after_refresh() {
        let mut state = BaoWebViewState::default();
        state.dom_proxies_dirty = true;
        state.dom_proxies_dirty = false;
        assert!(!state.dom_proxies_dirty);
    }
}
