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
    pub servo: Rc<Servo>,
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

    /// Evaluate JavaScript in privileged mode (REQ-SEC-002).
    ///
    /// Scripts run via this method have full Node.js/Bun runtime access:
    /// require, fs, crypto, Bun, process, Buffer, etc. These APIs are
    /// passed as function parameters to the IIFE wrapper, NOT installed
    /// on the Window global.
    ///
    /// The flow is:
    /// 1. Register callback via `inject_node_apis_for_evaluate` — this
    ///    creates `__bao_privileged_apis` on global (non-enumerable, configurable)
    /// 2. Drain the callback by evaluating an empty script — this ensures
    ///    the scope object exists on global before the user script runs
    /// 3. Execute the wrapped user script — the IIFE extracts the scope,
    ///    deletes it from globalThis, and passes values as parameters
    ///
    /// Web page JS running in the same realm cannot access Node APIs because:
    /// - The scope object is deleted before any page JS can run
    /// - servo's script thread is single-threaded, no interleaving is possible
    /// - Node APIs are never installed on the Window global (REQ-SEC-003)
    pub fn evaluate_js(&self, script: &str) -> Result<String, BrowserError> {
        // Phase 1: Register callback to create scope object on global
        let webview_id = self.webview.id();
        crate::runtime_bridge::inject_node_apis_for_evaluate(webview_id);

        // Phase 2: Drain the callback so the scope object is created.
        // We evaluate an empty script — servo drains pending callbacks
        // before executing JS, so the scope object will exist on global
        // after this call returns.
        self.evaluate_js_web("")?;

        // Phase 3: Execute the user script with CommonJS parameter injection.
        // The IIFE wrapper extracts the scope, deletes it, and passes
        // Node API values as function parameters.
        let wrapped = Self::wrap_privileged_script(script);
        let saved = Rc::new(RefCell::new(None));
        let cb_saved = saved.clone();
        self.webview.evaluate_javascript(wrapped, move |result| {
            *cb_saved.borrow_mut() = Some(result);
        });

        self.spin_servo(Duration::from_secs(15), || saved.borrow().is_none())?;

        let result = saved.borrow().clone()
            .ok_or_else(|| BrowserError::JavaScript("no evaluation result".into()))?
            .map_err(|e| BrowserError::JavaScript(format!("{e:?}")))?;

        self.touch();
        Ok(format_js_value(&result))
    }

    /// Evaluate JavaScript without Node API injection — web-only mode.
    ///
    /// Used internally for page operations that should not trigger Node API
    /// injection (e.g., empty script to drain callbacks, stealth checks).
    pub fn evaluate_js_web(&self, script: &str) -> Result<String, BrowserError> {
        let saved = Rc::new(RefCell::new(None));
        let cb_saved = saved.clone();
        self.webview.evaluate_javascript(script.to_string(), move |result| {
            *cb_saved.borrow_mut() = Some(result);
        });

        self.spin_servo(Duration::from_secs(15), || saved.borrow().is_none())?;

        let result = saved.borrow().clone()
            .ok_or_else(|| BrowserError::JavaScript("no evaluation result".into()))?
            .map_err(|e| BrowserError::JavaScript(format!("{e:?}")))?;

        self.touch();
        Ok(format_js_value(&result))
    }

    /// Wrap user script in a CommonJS IIFE that receives Node APIs as parameters.
    ///
    /// The wrapper:
    /// 1. Extracts the scope object from globalThis.__bao_privileged_apis
    /// 2. Deletes the scope object from globalThis (prevents page JS access)
    /// 3. Deletes global helper functions used by process.env Proxy
    /// 4. Deletes global Buffer (installed temporarily for prototype JS eval)
    /// 5. Passes all Node API values as function parameters to the user script
    ///
    /// This ensures Node APIs are never on the Window global when page JS runs
    /// (REQ-SEC-002/REQ-SEC-003). The scope object is non-enumerable, so casual
    /// inspection cannot find it. Even Reflect.ownKeys cannot exploit it because
    /// servo's script thread is single-threaded — no interleaving is possible.
    fn wrap_privileged_script(script: &str) -> String {
        format!(
            "(function() {{\
             \n  var __scope = globalThis.__bao_privileged_apis;\
             \n  delete globalThis.__bao_privileged_apis;\
             \n  delete globalThis.__bao_setEnv;\
             \n  delete globalThis.__bao_delEnv;\
             \n  delete globalThis.Buffer;\
             \n  if (!__scope) throw new Error('Bao: privileged API scope not available');\
             \n  (function(require, module, exports, Bun, process, Buffer, __filename, __dirname) {{\
             \n    {script}\
             \n  }})(__scope.require, __scope.module, __scope.module.exports, __scope.Bun, __scope.process, __scope.Buffer, __scope.__filename, __scope.__dirname);\
             \n}})();"
        )
    }

    pub fn take_screenshot(&self, format: ScreenshotFormat) -> Result<Vec<u8>, BrowserError> {
        self.webview.paint();

        let saved = Rc::new(RefCell::new(None));
        let cb_saved = saved.clone();
        self.webview.take_screenshot(None, move |result| {
            *cb_saved.borrow_mut() = Some(result);
        });

        self.spin_servo(Duration::from_secs(15), || saved.borrow().is_none())?;

        let image = saved.borrow().clone()
            .ok_or_else(|| BrowserError::Rendering("no screenshot result".into()))?
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

    /// Spin servo's event loop until the callback returns false or timeout.
    /// Mirrors servo's official test pattern: single spin_event_loop() per
    /// iteration with 1ms sleep, checking a shared RefCell for results.
    fn spin_servo(
        &self,
        timeout: Duration,
        callback: impl Fn() -> bool,
    ) -> Result<(), BrowserError> {
        let start = Instant::now();
        while callback() {
            self.servo.spin_event_loop();
            self.webview.paint();
            if start.elapsed() > timeout {
                return Err(BrowserError::Init("operation timed out".into()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
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
            servo: Rc::clone(&servo),
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

    /// Evaluate JS without Node API injection — web-only mode.
    ///
    /// Public for security verification: tests need to confirm that
    /// page-level JS cannot access Node APIs (REQ-SEC-002/003).
    pub fn evaluate_js_web(&self, script: &str) -> Result<String, BrowserError> {
        self.with_inner(|inner| inner.evaluate_js_web(script))
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

    pub(crate) fn webview_id(&self) -> Option<servo::WebViewId> {
        self.inner.borrow().as_ref().map(|inner| inner.webview.id())
    }

    pub fn permission(&self) -> PermissionGuard {
        let borrow = self.inner.borrow();
        match borrow.as_ref() {
            Some(inner) => inner.permission.clone(),
            None => PermissionGuard::none(),
        }
    }

    pub fn stealth_profile(&self) -> Option<bao_stealth::StealthProfile> {
        self.inner.borrow().as_ref().and_then(|inner| inner.stealth_profile.clone())
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

    // ── REQ-SEC-002: Dual-layer JS model structural verification ──────────
    // @trace TEST-SEC-002 [req:REQ-SEC-001,REQ-SEC-002,REQ-SEC-003] [level:unit]

    /// Verify evaluate_js wraps scripts in CommonJS parameter injection IIFE.
    /// REQ-SEC-002: evaluate_js scripts must be wrapped so Node APIs are passed
    /// as function parameters, NOT installed on the Window global.
    #[test]
    fn evaluate_js_wraps_script_in_commonjs_iife() {
        let wrapped = super::PageInner::wrap_privileged_script("return 42");
        // Outer IIFE that extracts and deletes scope
        assert!(
            wrapped.starts_with("(function() {"),
            "REQ-SEC-002 REGRESSION: wrap_privileged_script must produce outer IIFE, got: {}",
            wrapped
        );
        assert!(
            wrapped.contains("var __scope = globalThis.__bao_privileged_apis"),
            "REQ-SEC-002 REGRESSION: wrapper must extract scope from globalThis, got: {}",
            wrapped
        );
        assert!(
            wrapped.contains("delete globalThis.__bao_privileged_apis"),
            "REQ-SEC-002 REGRESSION: wrapper must delete scope from globalThis, got: {}",
            wrapped
        );
        // Inner function with CommonJS parameters
        assert!(
            wrapped.contains("function(require, module, exports, Bun, process, Buffer, __filename, __dirname)"),
            "REQ-SEC-002 REGRESSION: inner function must receive Node API parameters, got: {}",
            wrapped
        );
        // Scope values passed as arguments
        assert!(
            wrapped.contains("__scope.require"),
            "REQ-SEC-002 REGRESSION: must pass require from scope, got: {}",
            wrapped
        );
        assert!(
            wrapped.contains("__scope.module.exports"),
            "REQ-SEC-002 REGRESSION: must pass module.exports from scope, got: {}",
            wrapped
        );
        assert!(
            wrapped.contains("return 42"),
            "wrapped script must contain original script"
        );
    }

    /// Verify wrap_privileged_script deletes global helper functions.
    /// REQ-SEC-003: __bao_setEnv/__bao_delEnv must be deleted after scope
    /// extraction so page JS cannot use them to manipulate process.env.
    #[test]
    fn wrap_privileged_script_deletes_env_helpers() {
        let wrapped = super::PageInner::wrap_privileged_script("1");
        assert!(
            wrapped.contains("delete globalThis.__bao_setEnv"),
            "REQ-SEC-003 REGRESSION: wrapper must delete __bao_setEnv, got: {}",
            wrapped
        );
        assert!(
            wrapped.contains("delete globalThis.__bao_delEnv"),
            "REQ-SEC-003 REGRESSION: wrapper must delete __bao_delEnv, got: {}",
            wrapped
        );
    }

    /// Verify wrap_privileged_script deletes global Buffer.
    /// REQ-SEC-003: Buffer must be deleted from globalThis after scope
    /// extraction so page JS cannot access it.
    #[test]
    fn wrap_privileged_script_deletes_global_buffer() {
        let wrapped = super::PageInner::wrap_privileged_script("1");
        assert!(
            wrapped.contains("delete globalThis.Buffer"),
            "REQ-SEC-003 REGRESSION: wrapper must delete globalThis.Buffer, got: {}",
            wrapped
        );
    }

    /// Verify wrap_privileged_script throws if scope is missing.
    /// REQ-SEC-002: The wrapper must detect if __bao_privileged_apis was
    /// not created (e.g., callback drain failed) and throw an error.
    #[test]
    fn wrap_privileged_script_throws_on_missing_scope() {
        let wrapped = super::PageInner::wrap_privileged_script("1");
        assert!(
            wrapped.contains("if (!__scope) throw new Error"),
            "REQ-SEC-002 REGRESSION: wrapper must throw if scope is null, got: {}",
            wrapped
        );
    }

    /// Verify wrap_privileged_script preserves script content faithfully.
    #[test]
    fn wrap_privileged_script_preserves_content() {
        let script = "const x = require('fs'); x.readFileSync('/etc/passwd')";
        let wrapped = super::PageInner::wrap_privileged_script(script);
        assert!(wrapped.contains(script), "script content must be preserved exactly");
    }

    /// Verify wrap_privileged_script handles empty script.
    #[test]
    fn wrap_privileged_script_empty() {
        let wrapped = super::PageInner::wrap_privileged_script("");
        assert!(wrapped.contains("(function() {"), "empty script still wrapped in outer IIFE");
        assert!(wrapped.contains("function(require, module, exports, Bun, process, Buffer, __filename, __dirname)"),
            "empty script still receives CommonJS parameters");
    }

    /// Verify wrap_privileged_script handles multi-line script.
    #[test]
    fn wrap_privileged_script_multiline() {
        let script = "const a = 1;\nconst b = 2;\nreturn a + b;";
        let wrapped = super::PageInner::wrap_privileged_script(script);
        assert!(wrapped.contains("const a = 1;"), "multiline script preserved");
        assert!(wrapped.contains("return a + b;"), "multiline script preserved");
    }
}
