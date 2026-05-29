// REQ-BRW-002: Page lifecycle management (navigate, evaluate, screenshot)
use std::cell::RefCell;
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
    pub stealth: bool,
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
            stealth: config.stealth,
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
