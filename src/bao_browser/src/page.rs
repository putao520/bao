// @trace REQ-BRW-001 [entity:PageHandle]  REQ-BRW-002: Page lifecycle management (navigate, evaluate, screenshot)
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use servo::{
    Servo, SoftwareRenderingContext, WebView,
    WebViewBuilder, RenderingContext,
};

use crate::config::PageConfig;
use crate::delegate::{BaoWebViewDelegate, BaoWebViewState};
use crate::error::BrowserError;
use crate::permission::PermissionGuard;
use crate::screenshot::{encode_image, ScreenshotFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    Created,
    Navigating,
    Interactive,
    Idle,
    Closed,
}

pub struct PageInner {
    pub id: usize,
    pub webview: WebView,
    pub rendering_context: Rc<SoftwareRenderingContext>,
    pub delegate: Rc<BaoWebViewDelegate>,
    pub state: Rc<RefCell<PageState>>,
    pub webview_state: Rc<RefCell<BaoWebViewState>>,
    pub viewport: PhysicalSize<u32>,
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
    pub permission: PermissionGuard,
    pub last_active_at: RefCell<Instant>,
    pub created_at: Instant,
}

impl PageInner {
    pub fn touch(&self) {
        *self.last_active_at.borrow_mut() = Instant::now();
    }

    pub fn navigate(&self, url: &str) -> Result<(), BrowserError> {
        let parsed = url::Url::parse(url)
            .map_err(|e| BrowserError::Navigation(format!("invalid URL: {e}")))?;
        self.webview.load(parsed);
        self.touch();
        *self.state.borrow_mut() = PageState::Navigating;
        Ok(())
    }

    pub fn evaluate_js(&self, script: &str) -> Result<String, BrowserError> {
        let (tx, rx) = mpsc::channel();
        self.webview.evaluate_javascript(script.to_string(), move |result| {
            let _ = tx.send(result);
        });

        self.spin_until_timeout(Duration::from_secs(10), || rx.try_recv().ok())?;

        let result = rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| BrowserError::JavaScript("evaluation timed out".into()))?
            .map_err(|e| BrowserError::JavaScript(format!("{e:?}")))?;

        self.touch();
        Ok(format_js_value(&result))
    }

    pub fn take_screenshot(&self, format: ScreenshotFormat) -> Result<Vec<u8>, BrowserError> {
        self.webview.paint();

        let (tx, rx) = mpsc::channel();
        self.webview.take_screenshot(None, move |result| {
            let _ = tx.send(result);
        });

        self.spin_until_timeout(Duration::from_secs(10), || rx.try_recv().ok())?;

        let image = rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| BrowserError::Rendering("screenshot timed out".into()))?
            .map_err(|e| BrowserError::Rendering(format!("{e:?}")))?;

        self.touch();
        encode_image(&image, format)
    }

    pub fn page_title(&self) -> Option<String> {
        self.webview_state.borrow().title.clone()
    }

    pub fn current_url(&self) -> Option<String> {
        self.webview_state.borrow().url.as_ref().map(|u| u.to_string())
    }

    pub fn get_state(&self) -> PageState {
        *self.state.borrow()
    }

    fn spin_until_timeout<T>(
        &self,
        timeout: Duration,
        f: impl Fn() -> Option<T>,
    ) -> Result<(), BrowserError> {
        let start = Instant::now();
        loop {
            self.webview.paint();
            if f().is_some() {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(BrowserError::Init("operation timed out".into()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[derive(Clone)]
pub struct PageHandle {
    inner: Rc<RefCell<Option<PageInner>>>,
    id: usize,
    servo: Rc<Servo>,
    delegate: Rc<crate::delegate::BaoServoDelegate>,
}

impl PageHandle {
    pub(crate) fn new(
        servo: Rc<Servo>,
        servo_delegate: Rc<crate::delegate::BaoServoDelegate>,
        config: &PageConfig,
        default_viewport: PhysicalSize<u32>,
        id: usize,
    ) -> Result<Self, BrowserError> {
        let viewport = PhysicalSize::new(
            config.viewport_width.unwrap_or(default_viewport.width),
            config.viewport_height.unwrap_or(default_viewport.height),
        );

        let rendering_context = Rc::new(
            SoftwareRenderingContext::new(viewport)
                .map_err(|e| BrowserError::Init(format!("rendering context failed: {e:?}")))?,
        );

        let webview_state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let webview_delegate = Rc::new(BaoWebViewDelegate::new(Rc::clone(&webview_state), viewport));
        let state = Rc::new(RefCell::new(PageState::Created));

        let mut builder = WebViewBuilder::new(
            &servo,
            rendering_context.clone() as Rc<dyn RenderingContext>,
        )
        .delegate(Rc::clone(&webview_delegate) as Rc<dyn servo::WebViewDelegate>);

        if let Some(ref url_str) = config.url {
            let url = url::Url::parse(url_str)
                .map_err(|e| BrowserError::Init(format!("invalid URL: {e}")))?;
            builder = builder.url(url);
        }

        let webview = builder.build();

        let inner = PageInner {
            id,
            webview,
            rendering_context,
            delegate: webview_delegate,
            state,
            webview_state,
            viewport,
            stealth_profile: config.stealth_profile.clone(),
            permission: match &config.permission {
                Some(perm) => PermissionGuard::new(perm.clone()),
                None => PermissionGuard::none(),
            },
            last_active_at: RefCell::new(Instant::now()),
            created_at: Instant::now(),
        };

        Ok(PageHandle {
            inner: Rc::new(RefCell::new(Some(inner))),
            id,
            servo,
            delegate: servo_delegate,
        })
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn navigate(&self, url: &str) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.navigate(url))
    }

    pub fn evaluate_js(&self, script: &str) -> Result<String, BrowserError> {
        self.with_inner(|inner| inner.evaluate_js(script))
    }

    pub fn take_screenshot(&self, format: ScreenshotFormat) -> Result<Vec<u8>, BrowserError> {
        self.with_inner(|inner| inner.take_screenshot(format))
    }

    pub fn page_title(&self) -> Option<String> {
        self.with_inner_opt(|inner| inner.page_title())
    }

    pub fn current_url(&self) -> Option<String> {
        self.with_inner_opt(|inner| inner.current_url())
    }

    pub fn get_state(&self) -> PageState {
        self.inner
            .borrow()
            .as_ref()
            .map_or(PageState::Closed, |inner| inner.get_state())
    }

    pub fn is_alive(&self) -> bool {
        self.inner.borrow().is_some()
    }

    pub fn permission(&self) -> PermissionGuard {
        let borrow = self.inner.borrow();
        match borrow.as_ref() {
            Some(inner) => inner.permission.clone(),
            None => PermissionGuard::none(),
        }
    }

    pub fn close(&self) -> Result<(), BrowserError> {
        let mut borrow = self.inner.borrow_mut();
        if let Some(inner) = borrow.take() {
            *inner.state.borrow_mut() = PageState::Closed;
            drop(inner);
        }
        Ok(())
    }

    fn with_inner<F, R>(&self, f: F) -> Result<R, BrowserError>
    where
        F: FnOnce(&PageInner) -> Result<R, BrowserError>,
    {
        let borrow = self.inner.borrow();
        match borrow.as_ref() {
            Some(inner) => f(inner),
            None => Err(BrowserError::Init("page is closed".into())),
        }
    }

    fn with_inner_opt<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&PageInner) -> Option<R>,
    {
        let borrow = self.inner.borrow();
        borrow.as_ref().and_then(f)
    }
}

fn format_js_value(v: &servo::JSValue) -> String {
    match v {
        servo::JSValue::String(s) => s.clone(),
        servo::JSValue::Number(n) => n.to_string(),
        servo::JSValue::Boolean(b) => b.to_string(),
        servo::JSValue::Null => "null".into(),
        servo::JSValue::Undefined => "undefined".into(),
        servo::JSValue::Element(id) => format!("[Element: {id}]"),
        servo::JSValue::ShadowRoot(id) => format!("[ShadowRoot: {id}]"),
        servo::JSValue::Frame(id) => format!("[Frame: {id}]"),
        servo::JSValue::Window(id) => format!("[Window: {id}]"),
        servo::JSValue::Array(items) => {
            let formatted: Vec<String> = items.iter().map(format_js_value).collect();
            format!("[{}]", formatted.join(", "))
        }
        servo::JSValue::Object(map) => {
            let formatted: Vec<String> = map
                .iter()
                .map(|(k, val)| format!("{}: {}", k, format_js_value(val)))
                .collect();
            format!("{{{}}}", formatted.join(", "))
        }
    }
}

// @trace REQ-BRW-001 REQ-BRW-002 [req:REQ-BRW-001,REQ-BRW-002] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_state_variants_equal_to_themselves() {
        assert_eq!(PageState::Created, PageState::Created);
        assert_eq!(PageState::Navigating, PageState::Navigating);
        assert_eq!(PageState::Interactive, PageState::Interactive);
        assert_eq!(PageState::Idle, PageState::Idle);
        assert_eq!(PageState::Closed, PageState::Closed);
    }

    #[test]
    fn page_state_clone_works() {
        let state = PageState::Navigating;
        let cloned = state.clone();
        assert_eq!(state, cloned);
    }

    #[test]
    fn page_state_copy_works() {
        let state = PageState::Interactive;
        let copied: PageState = state;
        assert_eq!(state, copied);
    }

    #[test]
    fn page_state_debug_format_includes_variant_name() {
        assert!(format!("{:?}", PageState::Created).contains("Created"));
        assert!(format!("{:?}", PageState::Navigating).contains("Navigating"));
        assert!(format!("{:?}", PageState::Interactive).contains("Interactive"));
        assert!(format!("{:?}", PageState::Idle).contains("Idle"));
        assert!(format!("{:?}", PageState::Closed).contains("Closed"));
    }

    #[test]
    fn page_state_created_not_equal_closed() {
        assert_ne!(PageState::Created, PageState::Closed);
    }

    #[test]
    fn format_js_value_string() {
        let value = servo::JSValue::String("hello".into());
        assert_eq!(format_js_value(&value), "hello");
    }

    #[test]
    fn format_js_value_number() {
        let value = servo::JSValue::Number(42.5);
        assert_eq!(format_js_value(&value), "42.5");
    }

    #[test]
    fn format_js_value_boolean_true() {
        let value = servo::JSValue::Boolean(true);
        assert_eq!(format_js_value(&value), "true");
    }

    #[test]
    fn format_js_value_null() {
        let value = servo::JSValue::Null;
        assert_eq!(format_js_value(&value), "null");
    }

    #[test]
    fn format_js_value_undefined() {
        let value = servo::JSValue::Undefined;
        assert_eq!(format_js_value(&value), "undefined");
    }

    #[test]
    fn format_js_value_array() {
        let value = servo::JSValue::Array(vec![
            servo::JSValue::Number(1.0),
            servo::JSValue::Number(2.0),
            servo::JSValue::Number(3.0),
        ]);
        assert_eq!(format_js_value(&value), "[1, 2, 3]");
    }

    #[test]
    fn format_js_value_object() {
        let mut map = HashMap::new();
        map.insert("name".into(), servo::JSValue::String("test".into()));
        map.insert("count".into(), servo::JSValue::Number(5.0));
        let value = servo::JSValue::Object(map);
        let result = format_js_value(&value);
        assert!(result.starts_with('{') && result.ends_with('}'));
        assert!(result.contains("name: test"));
        assert!(result.contains("count: 5"));
    }

    #[test]
    fn format_js_value_element() {
        let value = servo::JSValue::Element("div#main".into());
        assert_eq!(format_js_value(&value), "[Element: div#main]");
    }

    #[test]
    fn format_js_value_shadow_root() {
        let value = servo::JSValue::ShadowRoot("host-element".into());
        assert_eq!(format_js_value(&value), "[ShadowRoot: host-element]");
    }

    #[test]
    fn format_js_value_frame() {
        let value = servo::JSValue::Frame("iframe-123".into());
        assert_eq!(format_js_value(&value), "[Frame: iframe-123]");
    }

    #[test]
    fn format_js_value_window() {
        let value = servo::JSValue::Window("window-456".into());
        assert_eq!(format_js_value(&value), "[Window: window-456]");
    }
}
