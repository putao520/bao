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
    /// Node Realm global object pointer for privileged evaluate_js (REQ-SEC-002).
    /// Created via JS_NewGlobalObject in its own Compartment — physically
    /// isolated from Page Realm (Window). Page JS cannot discover this.
    pub node_realm_global: RefCell<*mut mozjs::jsapi::JSObject>,
    /// Page Realm global pointer (servo's Window object) — used as key
    /// to look up this page's Node Realm from the per-page HashMap.
    pub page_global: RefCell<*mut mozjs::jsapi::JSObject>,
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

    /// Drain pending servo script thread callbacks by evaluating a minimal script.
    ///
    /// When `register_script_thread_callback` is called, the callback is queued
    /// but only executes during `handle_evaluate_javascript` on servo's script
    /// thread. This method triggers that drain by evaluating `";"` (minimal valid JS).
    ///
    /// If the pipeline isn't ready yet (WebView just created, constellation hasn't
    /// finished setup), servo returns InternalError. This method spins the event
    /// loop and retries until the pipeline is ready or the timeout expires.
    ///
    /// Returns the result of the drain evaluation (typically "undefined").
    pub fn drain_callbacks(&self) -> Result<String, BrowserError> {
        let max_attempts = 50;
        let attempt_interval = Duration::from_millis(20);

        for _ in 0..max_attempts {
            match self.evaluate_js_web(";") {
                Ok(result) => return Ok(result),
                Err(BrowserError::JavaScript(msg)) if msg.contains("InternalError") => {
                    // Pipeline not ready — spin servo event loop and retry.
                    self.servo.spin_event_loop();
                    self.webview.paint();
                    std::thread::sleep(attempt_interval);
                    continue;
                }
                Err(other) => return Err(other),
            }
        }

        Err(BrowserError::Init("callback drain failed: pipeline not ready after timeout".into()))
    }

    /// Evaluate JavaScript in privileged mode (REQ-SEC-002).
    ///
    /// Scripts run via this method have full Node.js/Bun runtime access:
    /// require, fs, crypto, Bun, process, Buffer, etc. These APIs are
    /// injected by `runtime_bridge::inject_node_apis_with_stealth` as
    /// engine-layer host functions on the page global, plus NODE_POLYFILLS
    /// JS polyfill for require/Buffer/process.
    ///
    /// Security model (REQ-SEC-002):
    /// - Node APIs are scoped via IIFE — injected as function parameters,
    ///   not written to Window globalThis.
    /// - After evaluate_js returns, page JS (via evaluate_js_web) cannot
    ///   see Node APIs because they were IIFE parameters, not global vars.
    /// - evaluate_js_web sees only Web APIs — typeof require === 'undefined'.
    /// Evaluate JS with full Node.js/Bun API access via Node Realm (REQ-SEC-002).
    ///
    /// The script executes in the Node Realm — an independent SpiderMonkey
    /// Compartment that has require/process/Buffer/Bun/fs/crypto installed
    /// on its global. The Page Realm physically cannot see the Node Realm.
    ///
    /// Flow: register callback → drain_callbacks → read EvaluateResult
    pub fn evaluate_js(&self, script: &str) -> Result<String, BrowserError> {
        let webview_id = self.webview.id();

        // Refresh stale DOM proxies after navigation (REQ-SEC-002 safety).
        // servo replaces Window/Document/Navigator on navigation; Node Realm's
        // cross-Compartment proxies must be refreshed to avoid use-after-free.
        if self.webview_state.borrow().dom_proxies_dirty {
            let old_pg = *self.page_global.borrow();
            if !old_pg.is_null() {
                crate::runtime_bridge::register_refresh_dom_proxies(webview_id, old_pg);
                self.drain_callbacks()?;
                // After drain, LAST_PAGE_GLOBAL holds the new page_global
                let new_pg = crate::runtime_bridge::get_last_page_global();
                if !new_pg.is_null() {
                    let new_node = crate::runtime_bridge::get_node_realm_global(new_pg);
                    *self.page_global.borrow_mut() = new_pg;
                    *self.node_realm_global.borrow_mut() = new_node;
                }
            }
            self.webview_state.borrow_mut().dom_proxies_dirty = false;
        }

        // Look up Node Realm for THIS page (per-page HashMap, REQ-SEC-002)
        let pg = *self.page_global.borrow();
        let node_global = if pg.is_null() {
            *self.node_realm_global.borrow()
        } else {
            crate::runtime_bridge::get_node_realm_global(pg)
        };

        if node_global.is_null() {
            // Node Realm not initialized — fallback to IIFE injection
            let wrapped = format!(
                "(function(require, process, Buffer, Bun, __dirname, __filename) {{ \
                   'use strict'; \
                   return ({script}); \
                 }})( \
                   typeof require !== 'undefined' ? require : undefined, \
                   typeof process !== 'undefined' ? process : undefined, \
                   typeof Buffer !== 'undefined' ? Buffer : undefined, \
                   typeof Bun !== 'undefined' ? Bun : undefined, \
                   typeof __dirname !== 'undefined' ? __dirname : '/', \
                   typeof __filename !== 'undefined' ? __filename : '/index.js' \
                 )"
            );
            return self.evaluate_js_web(&wrapped);
        }

        // Execute via Node Realm
        let result = crate::runtime_bridge::evaluate_js_via_node_realm(webview_id, script);
        self.drain_callbacks()?;

        let eval_result = result.lock().unwrap();
        match (&eval_result.value, &eval_result.error) {
            (Some(val), _) => Ok(val.clone()),
            (_, Some(err)) => Err(BrowserError::JavaScript(err.clone())),
            (None, None) => Ok(String::new()),
        }
    }

    /// Evaluate JavaScript without Node API injection — web-only mode.
    ///
    /// Executes directly in the Page Realm (Window global).
    /// Page JS has only Web API access — typeof require === 'undefined'.
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
            node_realm_global: RefCell::new(std::ptr::null_mut()),
            page_global: RefCell::new(std::ptr::null_mut()),
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

    /// Drain pending servo script thread callbacks.
    ///
    /// See [`PageInner::drain_callbacks`] for details.
    /// Wait for servo's WebView pipeline to be ready for script evaluation.
    ///
    /// After `pool.create_page()`, servo's constellation hasn't finished setting
    /// up the script thread pipeline. Calling `evaluate_js_web` too early causes
    /// SIGSEGV. This method spins the event loop (without paint) until the
    /// pipeline accepts script evaluation (drain_callbacks succeeds) or timeout.
    pub fn wait_for_pipeline_ready(&self, timeout: Duration) -> Result<(), BrowserError> {
        let start = Instant::now();

        // Phase 1: Spin servo event loop to let constellation create the pipeline.
        // Do NOT call paint() here — paint on an uninitialized pipeline segfaults.
        while start.elapsed() < timeout {
            self.with_inner(|inner| {
                inner.servo.spin_event_loop();
                Ok(())
            })?;
            std::thread::sleep(Duration::from_millis(20));

            // Try drain — if it succeeds, pipeline is ready.
            match self.drain_callbacks() {
                Ok(_) => return Ok(()),
                Err(BrowserError::JavaScript(msg)) if msg.contains("InternalError") => continue,
                Err(other) => return Err(other),
            }
        }
        Err(BrowserError::Init("pipeline not ready after timeout".into()))
    }

    pub fn drain_callbacks(&self) -> Result<String, BrowserError> {
        self.with_inner(|inner| inner.drain_callbacks())
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
            let pg = *inner.page_global.borrow();
            if !pg.is_null() {
                crate::runtime_bridge::remove_node_realm(pg);
            }
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

    /// Store page_global and node_realm_global pointers in PageInner (REQ-SEC-002).
    /// Called by runtime_bridge after drain_callbacks populates the per-page HashMap.
    pub fn set_page_global(&self, page_global: *mut mozjs::jsapi::JSObject, node_global: *mut mozjs::jsapi::JSObject) {
        let borrow = self.inner.borrow();
        if let Some(inner) = borrow.as_ref() {
            *inner.page_global.borrow_mut() = page_global;
            *inner.node_realm_global.borrow_mut() = node_global;
        }
    }

    /// Check whether the Node Realm was successfully created for this page.
    /// Returns (page_global_set, node_realm_set) — both should be true after
    /// `inject_node_apis_with_stealth` completes successfully.
    pub fn has_node_realm(&self) -> (bool, bool) {
        let borrow = self.inner.borrow();
        if let Some(inner) = borrow.as_ref() {
            let pg = *inner.page_global.borrow();
            let ng = *inner.node_realm_global.borrow();
            return (!pg.is_null(), !ng.is_null());
        }
        (false, false)
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

    // ── REQ-SEC-002/003: IIFE-scoped Node API isolation verification ──
    // @trace TEST-SEC-002 [req:REQ-SEC-002,REQ-SEC-003] [level:unit]
    // Security model: evaluate_js wraps scripts in IIFE with Node API parameters.
    // Node APIs (require, process, Buffer, etc.) are IIFE parameters, not global vars.
    // After IIFE returns, the parameters are gone — page JS cannot see them.

    /// Verify evaluate_js uses Node Realm execution when available (REQ-SEC-002).
    /// Falls back to IIFE injection when Node Realm is not initialized.
    #[test]
    fn evaluate_js_uses_node_realm_or_iife_fallback() {
        let source = include_str!("page.rs");
        let func_start = source.find("pub fn evaluate_js(&self, script: &str)")
            .expect("evaluate_js function not found");
        let func_body = &source[func_start..func_start + 2800.min(source.len() - func_start)];
        // Must check Node Realm availability
        assert!(
            func_body.contains("get_node_realm_global"),
            "REQ-SEC-002 REGRESSION: evaluate_js must check Node Realm global"
        );
        // Must use Node Realm execution path
        assert!(
            func_body.contains("evaluate_js_via_node_realm"),
            "REQ-SEC-002 REGRESSION: evaluate_js must use Node Realm execution"
        );
        // Must have IIFE fallback when Node Realm not initialized
        assert!(
            func_body.contains("(function(require, process, Buffer, Bun, __dirname, __filename)"),
            "REQ-SEC-002 REGRESSION: evaluate_js must have IIFE fallback"
        );
    }

    /// Verify evaluate_js drain callbacks after Node Realm execution.
    /// REQ-SEC-002: Results must be read after servo script thread callback.
    #[test]
    fn evaluate_js_drains_callbacks_for_result() {
        let source = include_str!("page.rs");
        let func_start = source.find("pub fn evaluate_js(&self, script: &str)")
            .expect("evaluate_js function not found");
        let func_body = &source[func_start..func_start + 2800.min(source.len() - func_start)];
        assert!(
            func_body.contains("drain_callbacks"),
            "REQ-SEC-002 REGRESSION: evaluate_js must drain callbacks after Node Realm execution"
        );
    }

    /// Verify evaluate_js reads result from shared EvaluateResult.
    /// REQ-SEC-002: Result must come from Arc<Mutex<EvaluateResult>>.
    #[test]
    fn evaluate_js_reads_evaluate_result() {
        let source = include_str!("page.rs");
        let func_start = source.find("pub fn evaluate_js(&self, script: &str)")
            .expect("evaluate_js function not found");
        let func_body = &source[func_start..func_start + 2800.min(source.len() - func_start)];
        assert!(
            func_body.contains("eval_result"),
            "REQ-SEC-002 REGRESSION: evaluate_js must read EvaluateResult"
        );
    }

    /// Verify Node APIs are NOT installed on page global by install_all_native.
    /// REQ-SEC-003: install_all_native must NOT call install_node_apis or install_all.
    #[test]
    fn page_global_has_no_node_apis() {
        let source = include_str!("runtime_bridge.rs");
        let func_start = source.find("unsafe fn install_all_native")
            .expect("install_all_native function not found");
        let func_body = &source[func_start..func_start + 2000.min(source.len() - func_start)];

        assert!(
            func_body.contains("bao_runtime::fetch_api::install_fetch_global"),
            "REQ-SEC-003 REGRESSION: install_all_native must install Web APIs (fetch)"
        );
        assert!(
            func_body.contains("bao_runtime::timers::install_timer_globals"),
            "REQ-SEC-003 REGRESSION: install_all_native must install Web APIs (timers)"
        );
        assert!(
            !func_body.contains("globals::install_all("),
            "REQ-SEC-003 REGRESSION: install_all_native must NOT call install_all()"
        );
        assert!(
            !func_body.contains("globals::install_node_apis("),
            "REQ-SEC-003 REGRESSION: install_all_native must NOT call install_node_apis() on page global"
        );
    }

    /// Verify Node APIs are installed on Node Realm global (not page global).
    /// REQ-SEC-002: Node Realm has both Node + Web APIs for privileged scripts.
    #[test]
    fn node_realm_has_node_apis() {
        let source = include_str!("runtime_bridge.rs");
        let func_start = source.find("unsafe fn create_node_realm_native")
            .expect("create_node_realm_native function not found");
        let func_end = source[func_start..].find("pub fn inject_node_apis")
            .or_else(|| source[func_start..].find("/// Inject Node.js APIs as native"))
            .expect("end boundary not found");
        let func_body = &source[func_start..func_start + func_end];

        assert!(
            func_body.contains("bao_runtime::globals::install_node_apis"),
            "REQ-SEC-002 REGRESSION: create_node_realm_native must install Node APIs on Node Realm global"
        );
        assert!(
            func_body.contains("bao_runtime::globals::install_web_apis"),
            "REQ-SEC-002: Node Realm must also have Web APIs for trusted scripts"
        );
    }

    /// Verify Node Realm is in its own Compartment (NewCompartmentAndZone).
    /// REQ-SEC-002: Physical isolation via SpiderMonkey Compartment boundary.
    #[test]
    fn node_realm_uses_new_compartment() {
        let source = include_str!("runtime_bridge.rs");
        let func_start = source.find("unsafe fn create_node_realm_native")
            .expect("create_node_realm_native function not found");
        let func_body = &source[func_start..func_start + 2000.min(source.len() - func_start)];
        assert!(
            func_body.contains("NewCompartmentAndZone"),
            "REQ-SEC-002 REGRESSION: Node Realm must use NewCompartmentAndZone"
        );
        assert!(
            func_body.contains("SIMPLE_GLOBAL_CLASS"),
            "REQ-SEC-002 REGRESSION: Node Realm must use SIMPLE_GLOBAL_CLASS"
        );
    }

    /// Verify evaluate_in_node_realm uses AutoRealm for Compartment isolation.
    #[test]
    fn evaluate_in_node_realm_uses_auto_realm() {
        let source = include_str!("runtime_bridge.rs");
        assert!(
            source.contains("AutoRealm::new_from_handle"),
            "REQ-SEC-002 REGRESSION: evaluate_in_node_realm must use AutoRealm"
        );
    }

    /// Verify per-page Node Realm storage exists (REQ-SEC-002).
    /// Node Realm globals are stored in thread_local HashMap keyed by page_global.
    #[test]
    fn node_realm_global_stored_per_page() {
        let source = include_str!("runtime_bridge.rs");
        assert!(
            source.contains("NODE_REALMS"),
            "REQ-SEC-002 REGRESSION: must have NODE_REALMS per-page storage"
        );
        assert!(
            source.contains("store_node_realm"),
            "REQ-SEC-002 REGRESSION: must have store_node_realm accessor"
        );
        assert!(
            source.contains("get_node_realm"),
            "REQ-SEC-002 REGRESSION: must have get_node_realm accessor"
        );
        assert!(
            source.contains("get_node_realm_global"),
            "REQ-SEC-002 REGRESSION: must have get_node_realm_global accessor"
        );
    }

    /// Verify PageInner stores node_realm_global pointer for Node Realm lifecycle.
    #[test]
    fn page_inner_has_node_realm_global_field() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("node_realm_global: RefCell<*mut mozjs::jsapi::JSObject>"),
            "REQ-SEC-002 REGRESSION: PageInner must have node_realm_global field"
        );
    }

    /// Verify drain_callbacks method exists on PageInner.
    /// REQ-SEC-002: Callback drain must handle InternalError from pending pipeline.
    #[test]
    fn page_inner_has_drain_callbacks_method() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("fn drain_callbacks(&self)"),
            "REQ-SEC-002 REGRESSION: PageInner must have drain_callbacks method"
        );
        assert!(
            source.contains("InternalError"),
            "REQ-SEC-002 REGRESSION: drain_callbacks must handle InternalError retry"
        );
    }
}
