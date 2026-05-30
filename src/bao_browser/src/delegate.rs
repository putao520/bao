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
}

impl Default for BaoWebViewState {
    fn default() -> Self {
        BaoWebViewState {
            url: None,
            title: None,
            load_status: LoadStatus::Started,
            frame_ready: false,
        }
    }
}

pub struct BaoServoDelegate {
    last_error: RefCell<Option<String>>,
}

impl BaoServoDelegate {
    pub fn new() -> Self {
        BaoServoDelegate {
            last_error: RefCell::new(None),
        }
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
