// @trace REQ-BRW-001 [entity:PageHandle]  REQ-BRW-002: Page lifecycle management (navigate, evaluate, screenshot)
// @trace REQ-LIB-001 REQ-LIB-004: PageHandle high-level API (waitForSelector, click, type, fill, etc.)
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use servo::{
    CSSPixel, Code, CookieSource, InputEvent, Key, KeyState, KeyboardEvent, Location, Modifiers,
    MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, NamedKey, RenderingContext,
    Servo, SoftwareRenderingContext, StorageType, WebView, WebViewBuilder, WebViewPoint,
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
    /// Intermediate cleanup state (SM PageLifecycle, SPEC 03-PROCESS).
    /// Entered on: close_during_load / idle_ttl_expired / close.
    /// Exited on: cleanup_complete → Closed.
    /// @trace REQ-BRW-001 [sm:PageLifecycle]
    Closing,
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

    /// WebViewId of this page's servo WebView. Stable across navigation
    /// (servo ties the WebViewId to the WebView, not the pipeline).
    /// Used for all WebViewId-keyed runtime_bridge lookups (BCE-20260621-001).
    pub fn webview_id_opt(&self) -> Option<servo::WebViewId> {
        Some(self.webview.id())
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
    // @trace REQ-BRW-001 [entity:PageHandle]
    pub fn drain_callbacks(&self) -> Result<String, BrowserError> {
        let max_attempts = 100;

        for attempt in 0..max_attempts {
            match self.evaluate_js_web(";") {
                Ok(result) => return Ok(result),
                Err(BrowserError::JavaScript(msg)) if msg.contains("InternalError") => {
                    // Pipeline not ready — spin servo event loop and retry.
                    // Yield after every few attempts to avoid CPU spinning.
                    if attempt % 5 == 4 {
                        self.servo.spin_event_loop();
                        self.webview.paint();
                    }
                    continue;
                }
                Err(other) => return Err(other),
            }
        }

        Err(BrowserError::Init(
            "callback drain failed: pipeline not ready after timeout".into(),
        ))
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
    //
    // @trace REQ-BRW-003 [req:REQ-BRW-003] [criterion:C2,C4,C10]
    // BCE-20260621-001: page_global/node_realm_global are no longer looked up
    // by raw *mut JSObject — they are looked up by WebViewId. PageInner's
    // stored fields remain as opaque addresses for close()/cleanup, but the
    // evaluate path uses WebViewId-keyed access exclusively.
    pub fn evaluate_js(&self, script: &str) -> Result<String, BrowserError> {
        let webview_id = self.webview.id();

        // Refresh stale DOM proxies after navigation (REQ-SEC-002 safety).
        // servo replaces Window/Document/Navigator on navigation; the per-WebViewId
        // page_global mapping must be updated so lazy getters find the new
        // Page Realm. The Node Realm itself survives navigation (same WebViewId).
        if self.webview_state.borrow().dom_proxies_dirty {
            let old_pg = *self.page_global.borrow();
            crate::runtime_bridge::register_refresh_dom_proxies(webview_id, old_pg);
            self.drain_callbacks()?;
            // After drain, read the refreshed pointers via WebViewId.
            let new_pg = crate::runtime_bridge::get_page_global(webview_id);
            let new_node = crate::runtime_bridge::get_node_realm_global(webview_id);
            *self.page_global.borrow_mut() = new_pg;
            *self.node_realm_global.borrow_mut() = new_node;
            self.webview_state.borrow_mut().dom_proxies_dirty = false;
        }

        // Verify Node Realm exists for THIS page (via WebViewId, REQ-SEC-002).
        let node_global = crate::runtime_bridge::get_node_realm_global(webview_id);
        if node_global.is_null() {
            // Node Realm must be initialized at page creation (PagePool::create_page).
            // If we reach here, it's a programming error, not a lazy-init scenario.
            return Err(BrowserError::JavaScript(
                "Node Realm not initialized — this is a bug, eager init failed".into(),
            ));
        }

        // Execute via Node Realm (servo routes the callback by WebViewId →
        // this page's ScriptThread, where node_global was created).
        let result = crate::runtime_bridge::evaluate_js_via_node_realm(webview_id, script);
        self.drain_callbacks()?;

        let eval_result = result.get().expect("evaluate result not set after drain");
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
        self.webview
            .evaluate_javascript(script.to_string(), move |result| {
                *cb_saved.borrow_mut() = Some(result);
            });

        self.spin_servo(Duration::from_secs(15), || saved.borrow().is_none())?;

        let result = saved
            .borrow()
            .clone()
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

        let image = saved
            .borrow()
            .clone()
            .ok_or_else(|| BrowserError::Rendering("no screenshot result".into()))?
            .map_err(|e| BrowserError::Rendering(format!("{e:?}")))?;

        self.touch();
        encode_image(&image, format)
    }

    /// Reload the page via servo's WebView::reload().
    pub fn reload(&self) -> Result<(), BrowserError> {
        self.webview.reload();
        self.touch();
        *self.state.borrow_mut() = PageState::Navigating;
        Ok(())
    }

    /// Navigate back in history via servo's WebView::go_back().
    pub fn go_back(&self) -> Result<(), BrowserError> {
        self.webview.go_back(1);
        self.touch();
        *self.state.borrow_mut() = PageState::Navigating;
        Ok(())
    }

    /// Navigate forward in history via servo's WebView::go_forward().
    pub fn go_forward(&self) -> Result<(), BrowserError> {
        self.webview.go_forward(1);
        self.touch();
        *self.state.borrow_mut() = PageState::Navigating;
        Ok(())
    }

    /// Check if back navigation is possible.
    pub fn can_go_back(&self) -> bool {
        self.webview.can_go_back()
    }

    /// Check if forward navigation is possible.
    pub fn can_go_forward(&self) -> bool {
        self.webview.can_go_forward()
    }

    /// Set viewport size via servo's WebView::resize().
    pub fn set_viewport(&self, width: u32, height: u32) {
        let new_size = PhysicalSize::new(width, height);
        self.webview.resize(new_size);
        self.touch();
    }

    /// Focus the WebView window.
    pub fn focus(&self) {
        self.webview.focus();
    }

    /// Dispatch a mouse button event at the given page coordinates.
    pub fn dispatch_mouse_event(
        &self,
        action: MouseButtonAction,
        button: MouseButton,
        x: f32,
        y: f32,
    ) {
        let point = WebViewPoint::Page(euclid::Point2D::<f32, CSSPixel>::new(x, y));
        let event = InputEvent::MouseButton(MouseButtonEvent::new(action, button, point));
        self.webview.notify_input_event(event);
        self.touch();
    }

    /// Dispatch a mouse move event at the given page coordinates.
    pub fn dispatch_mouse_move(&self, x: f32, y: f32) {
        let point = WebViewPoint::Page(euclid::Point2D::<f32, CSSPixel>::new(x, y));
        let event = InputEvent::MouseMove(MouseMoveEvent::new(point));
        self.webview.notify_input_event(event);
        self.touch();
    }

    /// Dispatch a keyboard event.
    pub fn dispatch_key_event(&self, state: KeyState, key: Key, code: Code) {
        let keyboard_event = KeyboardEvent::new_without_event(
            state,
            key,
            code,
            Location::Standard,
            Modifiers::empty(),
            false,
            false,
        );
        let event = InputEvent::Keyboard(keyboard_event);
        self.webview.notify_input_event(event);
        self.touch();
    }

    /// Dispatch a keyboard event with full parameters.
    pub fn dispatch_key_event_full(
        &self,
        state: KeyState,
        key: Key,
        code: Code,
        location: Location,
        modifiers: Modifiers,
        repeat: bool,
    ) {
        let keyboard_event =
            KeyboardEvent::new_without_event(state, key, code, location, modifiers, repeat, false);
        let event = InputEvent::Keyboard(keyboard_event);
        self.webview.notify_input_event(event);
        self.touch();
    }

    /// Get cookies for the given URLs (or current page URL if empty).
    pub fn cookies(&self, urls: &[String]) -> Result<Vec<cookie::Cookie<'static>>, BrowserError> {
        let sdm = self.servo.site_data_manager();
        if urls.is_empty() {
            let current_url = self.current_url().unwrap_or_default();
            if current_url.is_empty() || current_url == "about:blank" {
                return Ok(Vec::new());
            }
            match url::Url::parse(&current_url) {
                Ok(parsed) => Ok(sdm.cookies_for_url(parsed, CookieSource::HTTP)),
                Err(_) => Ok(Vec::new()),
            }
        } else {
            let mut seen = std::collections::HashSet::new();
            let mut result = Vec::new();
            for url_str in urls {
                if let Ok(parsed) = url::Url::parse(url_str) {
                    for c in sdm.cookies_for_url(parsed, CookieSource::HTTP) {
                        let key = (
                            c.name().to_string(),
                            c.domain().unwrap_or("").to_string(),
                            c.path().unwrap_or("").to_string(),
                        );
                        if seen.insert(key) {
                            result.push(c);
                        }
                    }
                }
            }
            Ok(result)
        }
    }

    /// Set a cookie for the given URL.
    pub fn set_cookie(
        &self,
        url: &str,
        cookie: cookie::Cookie<'static>,
    ) -> Result<(), BrowserError> {
        let sdm = self.servo.site_data_manager();
        let parsed = url::Url::parse(url)
            .map_err(|e| BrowserError::Navigation(format!("invalid URL for setCookie: {e}")))?;
        sdm.set_cookie_for_url(parsed, cookie, None);
        self.touch();
        Ok(())
    }

    /// Delete cookies matching the given name for the given URL.
    /// If url is None, deletes cookies matching the name across all sites.
    pub fn delete_cookie(&self, name: &str, url: Option<&str>) -> Result<(), BrowserError> {
        let sdm = self.servo.site_data_manager();
        if let Some(url_str) = url {
            let parsed = url::Url::parse(url_str).map_err(|e| {
                BrowserError::Navigation(format!("invalid URL for deleteCookie: {e}"))
            })?;
            let current = sdm.cookies_for_url(parsed.clone(), CookieSource::HTTP);
            let site = parsed.host_str().unwrap_or("");
            sdm.clear_site_data(&[site], StorageType::Cookies);
            for c in current {
                if c.name() != name {
                    sdm.set_cookie_for_url(parsed.clone(), c, None);
                }
            }
        } else {
            let site_data = sdm.site_data(StorageType::Cookies);
            for sd in site_data {
                let site_name = sd.name();
                let url_str =
                    if site_name.starts_with("http://") || site_name.starts_with("https://") {
                        site_name.clone()
                    } else {
                        format!("https://{site_name}")
                    };
                if let Ok(parsed) = url::Url::parse(&url_str) {
                    let current = sdm.cookies_for_url(parsed.clone(), CookieSource::HTTP);
                    let has_match = current.iter().any(|c| c.name() == name);
                    if has_match {
                        sdm.clear_site_data(&[&site_name], StorageType::Cookies);
                        for c in current {
                            if c.name() != name {
                                sdm.set_cookie_for_url(parsed.clone(), c, None);
                            }
                        }
                    }
                }
            }
        }
        self.touch();
        Ok(())
    }

    /// Wait for an element matching the selector to appear in the DOM.
    /// Polls via JS evaluate until the element is found or timeout expires.
    pub fn wait_for_selector(&self, selector: &str, timeout: Duration) -> Result<(), BrowserError> {
        let js = format!(
            "(function() {{ return document.querySelector({}) !== null; }})()",
            serde_json::to_string(selector).unwrap_or_default()
        );
        let start = Instant::now();
        while start.elapsed() < timeout {
            match self.evaluate_js_web(&js) {
                Ok(ref result) if result == "true" => {
                    self.touch();
                    return Ok(());
                }
                Ok(_) => {}
                Err(BrowserError::JavaScript(ref msg)) if msg.contains("InternalError") => {
                    // Pipeline not ready — spin and retry
                    self.servo.spin_event_loop();
                    self.webview.paint();
                    continue;
                }
                Err(e) => return Err(e),
            }
            self.servo.spin_event_loop();
            self.webview.paint();
            std::thread::yield_now();
        }
        Err(BrowserError::Init(format!(
            "waitForSelector timed out after {}ms for: {selector}",
            timeout.as_millis()
        )))
    }

    /// Wait for a JS function/condition to return a truthy value.
    /// Polls via JS evaluate until the condition is met or timeout expires.
    pub fn wait_for_function(
        &self,
        fn_expression: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        let js = format!("(function() {{ return !!({fn_expression}); }})()");
        let start = Instant::now();
        while start.elapsed() < timeout {
            match self.evaluate_js_web(&js) {
                Ok(ref result) if result == "true" => {
                    self.touch();
                    return Ok(());
                }
                Ok(_) => {}
                Err(BrowserError::JavaScript(ref msg)) if msg.contains("InternalError") => {
                    self.servo.spin_event_loop();
                    self.webview.paint();
                    continue;
                }
                Err(e) => return Err(e),
            }
            self.servo.spin_event_loop();
            self.webview.paint();
            std::thread::yield_now();
        }
        Err(BrowserError::Init(format!(
            "waitForFunction timed out after {}ms",
            timeout.as_millis()
        )))
    }

    /// Wait for page navigation to complete (load status transitions to Complete).
    ///
    /// Tracks navigation via `LoadStatus` transitions rather than URL changes,
    /// which correctly handles same-URL navigation (reload, pushState to current URL).
    /// Detects when `load_status` transitions from Started/HeadParsed to Complete.
    pub fn wait_for_navigation(&self, timeout: Duration) -> Result<(), BrowserError> {
        let start = Instant::now();
        // Record the initial load_status. Navigation begins with Started,
        // so if we're already at Complete, we wait for a new Started first.
        let initial_status = self.webview_state.borrow().load_status;
        let mut saw_new_navigation = initial_status != servo::LoadStatus::Started;

        while start.elapsed() < timeout {
            let current_status = self.webview_state.borrow().load_status;

            if current_status == servo::LoadStatus::Started {
                // A new navigation has begun — we now wait for it to complete.
                saw_new_navigation = true;
            }

            if saw_new_navigation && current_status == servo::LoadStatus::Complete {
                self.touch();
                return Ok(());
            }

            self.servo.spin_event_loop();
            self.webview.paint();
            std::thread::yield_now();
        }
        Err(BrowserError::Init(format!(
            "waitForNavigation timed out after {}ms",
            timeout.as_millis()
        )))
    }

    /// Click an element matching the selector.
    /// Uses JS evaluate to find the element and get its position,
    /// then dispatches mouse events (down + up) via servo InputEvent.
    pub fn click_element(&self, selector: &str) -> Result<(), BrowserError> {
        // Get element center position via JS
        let js = format!(
            "(function() {{ var e = document.querySelector({}); if (!e) return null; var r = e.getBoundingClientRect(); return JSON.stringify({{x: r.x + r.width/2, y: r.y + r.height/2}}); }})()",
            serde_json::to_string(selector).unwrap_or_default()
        );
        let pos_str = self.evaluate_js_web(&js)?;
        if pos_str == "null" || pos_str.is_empty() {
            return Err(BrowserError::JavaScript(format!(
                "element not found for click: {selector}"
            )));
        }
        let pos: serde_json::Value = serde_json::from_str(&pos_str)
            .map_err(|e| BrowserError::JavaScript(format!("invalid position JSON: {e}")))?;
        let x = pos["x"].as_f64().unwrap_or(0.0) as f32;
        let y = pos["y"].as_f64().unwrap_or(0.0) as f32;

        // Dispatch mouseDown then mouseUp
        self.dispatch_mouse_event(MouseButtonAction::Down, MouseButton::Left, x, y);
        self.servo.spin_event_loop();
        self.webview.paint();
        self.dispatch_mouse_event(MouseButtonAction::Up, MouseButton::Left, x, y);
        Ok(())
    }

    /// Type text into the currently focused element by dispatching key events.
    /// Each character generates a keyDown + keyUp pair.
    pub fn type_text(&self, text: &str) -> Result<(), BrowserError> {
        for ch in text.chars() {
            let key = match ch {
                '\n' => Key::Named(NamedKey::Enter),
                '\t' => Key::Named(NamedKey::Tab),
                '\u{8}' => Key::Named(NamedKey::Backspace),
                '\u{7f}' => Key::Named(NamedKey::Delete),
                ' ' => Key::Character(" ".into()),
                c => Key::Character(c.to_string()),
            };
            let code = key_code_for_char(ch);
            self.dispatch_key_event_full(
                KeyState::Down,
                key.clone(),
                code.clone(),
                Location::Standard,
                Modifiers::empty(),
                false,
            );
            self.servo.spin_event_loop();
            self.webview.paint();
            self.dispatch_key_event_full(
                KeyState::Up,
                key,
                code,
                Location::Standard,
                Modifiers::empty(),
                false,
            );
        }
        Ok(())
    }

    /// Fill a form field identified by selector with the given value.
    /// Sets the value property via JS and dispatches input/change events.
    pub fn fill(&self, selector: &str, value: &str) -> Result<(), BrowserError> {
        let js = format!(
            "(function() {{ var e = document.querySelector({}); if (!e) return false; e.value = {}; e.dispatchEvent(new Event('input', {{bubbles: true}})); e.dispatchEvent(new Event('change', {{bubbles: true}})); return true; }})()",
            serde_json::to_string(selector).unwrap_or_default(),
            serde_json::to_string(value).unwrap_or_default(),
        );
        let result = self.evaluate_js_web(&js)?;
        if result == "false" {
            return Err(BrowserError::JavaScript(format!(
                "element not found for fill: {selector}"
            )));
        }
        Ok(())
    }

    /// Set the page HTML content via document.open/write/close.
    pub fn set_content(&self, html: &str) -> Result<(), BrowserError> {
        let js = format!(
            "(function() {{ document.open(); document.write({}); document.close(); }})()",
            serde_json::to_string(html).unwrap_or_default(),
        );
        self.evaluate_js_web(&js)?;
        Ok(())
    }

    /// Get the page HTML content via document.documentElement.outerHTML.
    pub fn content(&self) -> Result<String, BrowserError> {
        self.evaluate_js_web("document.documentElement.outerHTML")
    }

    /// Select options in a <select> element identified by selector.
    /// Values are the option values to select.
    pub fn select(&self, selector: &str, values: &[&str]) -> Result<(), BrowserError> {
        let values_json = serde_json::to_string(&values).unwrap_or_default();
        let js = format!(
            "(function() {{ var e = document.querySelector({}); if (!e) return false; var vals = {values_json}; Array.from(e.options).forEach(function(o) {{ o.selected = vals.indexOf(o.value) !== -1; }}); e.dispatchEvent(new Event('change', {{bubbles: true}})); return true; }})()",
            serde_json::to_string(selector).unwrap_or_default(),
        );
        let result = self.evaluate_js_web(&js)?;
        if result == "false" {
            return Err(BrowserError::JavaScript(format!(
                "element not found for select: {selector}"
            )));
        }
        Ok(())
    }

    /// Press a key (e.g. Enter, Tab, ArrowDown) by dispatching keyboard events.
    pub fn press(&self, key: &str) -> Result<(), BrowserError> {
        let (key_val, code_val) = parse_key_name(key);
        self.dispatch_key_event_full(
            KeyState::Down,
            key_val.clone(),
            code_val.clone(),
            Location::Standard,
            Modifiers::empty(),
            false,
        );
        self.servo.spin_event_loop();
        self.webview.paint();
        self.dispatch_key_event_full(
            KeyState::Up,
            key_val,
            code_val,
            Location::Standard,
            Modifiers::empty(),
            false,
        );
        Ok(())
    }

    /// Hover over an element matching the selector.
    /// Gets element position via JS, then dispatches mouseMove.
    pub fn hover(&self, selector: &str) -> Result<(), BrowserError> {
        let js = format!(
            "(function() {{ var e = document.querySelector({}); if (!e) return null; var r = e.getBoundingClientRect(); return JSON.stringify({{x: r.x + r.width/2, y: r.y + r.height/2}}); }})()",
            serde_json::to_string(selector).unwrap_or_default()
        );
        let pos_str = self.evaluate_js_web(&js)?;
        if pos_str == "null" || pos_str.is_empty() {
            return Err(BrowserError::JavaScript(format!(
                "element not found for hover: {selector}"
            )));
        }
        let pos: serde_json::Value = serde_json::from_str(&pos_str)
            .map_err(|e| BrowserError::JavaScript(format!("invalid position JSON: {e}")))?;
        let x = pos["x"].as_f64().unwrap_or(0.0) as f32;
        let y = pos["y"].as_f64().unwrap_or(0.0) as f32;
        self.dispatch_mouse_move(x, y);
        Ok(())
    }

    /// Focus an element matching the selector via JS.
    pub fn focus_element(&self, selector: &str) -> Result<(), BrowserError> {
        let js = format!(
            "(function() {{ var e = document.querySelector({}); if (!e) return false; e.focus(); return true; }})()",
            serde_json::to_string(selector).unwrap_or_default()
        );
        let result = self.evaluate_js_web(&js)?;
        if result == "false" {
            return Err(BrowserError::JavaScript(format!(
                "element not found for focus: {selector}"
            )));
        }
        Ok(())
    }

    /// Take screenshot with optional clip region, selector, or fullPage mode.
    pub fn take_screenshot_advanced(
        &self,
        format: ScreenshotFormat,
        clip: Option<(f64, f64, f64, f64)>,
        full_page: bool,
    ) -> Result<Vec<u8>, BrowserError> {
        let original_viewport = self.viewport;

        if full_page {
            // Resize viewport to full page height to capture everything.
            let height_js = "document.documentElement.scrollHeight";
            let height_str = self.evaluate_js_web(height_js).unwrap_or_default();
            let full_height: u32 = height_str
                .trim()
                .parse()
                .unwrap_or(original_viewport.height);
            let capped_height = full_height.max(original_viewport.height);
            if capped_height != original_viewport.height {
                self.set_viewport(original_viewport.width, capped_height);
                // Allow servo to re-layout at the new viewport size.
                self.servo.spin_event_loop();
                self.webview.paint();
            }
        }

        let result = self.take_screenshot(format);

        // Restore original viewport after full_page capture.
        if full_page && original_viewport != self.viewport {
            self.set_viewport(original_viewport.width, original_viewport.height);
        }

        let image_bytes = result?;

        // Apply clip region by decoding, cropping, and re-encoding.
        if let Some((x, y, w, h)) = clip {
            let mut img = image::load_from_memory(&image_bytes).map_err(|e| {
                BrowserError::Rendering(format!("failed to decode screenshot for clip: {e}"))
            })?;
            let crop_x = x.max(0.0) as u32;
            let crop_y = y.max(0.0) as u32;
            let crop_w = (w as u32).min(img.width().saturating_sub(crop_x));
            let crop_h = (h as u32).min(img.height().saturating_sub(crop_y));
            if crop_w == 0 || crop_h == 0 {
                return Err(BrowserError::Rendering(
                    "clip region has zero dimensions".into(),
                ));
            }
            let cropped = img.crop(crop_x, crop_y, crop_w, crop_h);
            let rgba = cropped.to_rgba8();
            // Re-determine format from the original bytes (PNG by default).
            let fmt = if image_bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
                ScreenshotFormat::Png
            } else {
                ScreenshotFormat::Jpeg
            };
            return encode_image(&rgba, fmt);
        }

        Ok(image_bytes)
    }

    pub fn page_title(&self) -> Option<String> {
        self.webview_state.borrow().title.clone()
    }

    pub fn current_url(&self) -> Option<String> {
        self.webview_state
            .borrow()
            .url
            .as_ref()
            .map(|u| u.to_string())
    }

    pub fn get_state(&self) -> PageState {
        *self.state.borrow()
    }

    /// Spin servo's event loop until the callback returns false or timeout.
    /// Uses yield_now instead of sleep to avoid blocking the thread.
    // @trace REQ-BRW-001 [entity:PageHandle]
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
            // Yield instead of sleep — servo event loop is non-blocking,
            // and we want to check callback as soon as possible.
            std::thread::yield_now();
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
        // Propagate console log channel from servo delegate to per-webview state
        if let Some(tx) = servo_delegate.console_log_tx() {
            webview_state.borrow_mut().console_log_tx = Some(tx);
        }
        // @trace REQ-BRW-004 [criterion:12..17] CRIT-STL-WK stealth consistency
        // Auto-populate worker_scope_config from the page's StealthProfile so that
        // Workers spawned from this page inherit identical navigator/Canvas/WebGL/Audio
        // fingerprints. Without this, WorkerScopeConfig defaults to stealth_profile: None
        // and Workers would see servo's native fingerprint values instead.
        if let Some(ref profile) = config.stealth_profile {
            webview_state.borrow_mut().set_worker_scope_config(
                crate::delegate::WorkerScopeConfig::from(profile as &bao_stealth::StealthProfile),
            );
        }
        let webview_delegate =
            Rc::new(BaoWebViewDelegate::new(Rc::clone(&webview_state), viewport));
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

    /// Wait for servo's WebView pipeline to be ready for script evaluation.
    ///
    /// After `pool.create_page()`, servo's constellation hasn't finished setting
    /// up the script thread pipeline. Calling `evaluate_js_web` too early causes
    /// SIGSEGV. This method waits for `frame_ready` callback from servo, then
    /// verifies pipeline readiness via drain_callbacks.
    ///
    /// Uses event-driven notification via `notify_new_frame_ready` callback
    /// instead of sleep polling.
    // @trace REQ-BRW-001 [entity:PageHandle] [sm:PageLifecycle]
    pub fn wait_for_pipeline_ready(&self, timeout: Duration) -> Result<(), BrowserError> {
        let start = Instant::now();

        // Event-driven wait: spin event loop until frame_ready callback fires.
        // The callback sets frame_ready = true via notify_new_frame_ready.
        while start.elapsed() < timeout {
            // Check frame_ready flag first (fast path, no sleep needed)
            let ready = self
                .with_inner_opt(|inner| Some(inner.webview_state.borrow().frame_ready))
                .unwrap_or(false);
            if ready {
                // Frame ready — verify pipeline by draining callbacks.
                return self.drain_callbacks().map(|_| ());
            }

            // Not ready yet — spin event loop to process servo messages.
            self.with_inner(|inner| {
                inner.servo.spin_event_loop();
                Ok(())
            })?;

            // Yield briefly to avoid CPU spinning (servo event loop is non-blocking).
            std::thread::yield_now();
        }

        Err(BrowserError::Init(
            "pipeline not ready after timeout".into(),
        ))
    }

    pub fn drain_callbacks(&self) -> Result<String, BrowserError> {
        self.with_inner(|inner| inner.drain_callbacks())
    }

    /// Evaluate JS with Node API injection (trusted context).
    /// Node Realm is eagerly initialized at page creation time (REQ-SEC-002).
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

    /// Access the Servo instance for this page (e.g. SiteDataManager, NetworkManager).
    /// Used by CDP domain handlers to access cookie/cache/network APIs.
    pub fn servo(&self) -> &Rc<Servo> {
        &self.servo
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
        self.inner
            .borrow()
            .as_ref()
            .and_then(|inner| inner.stealth_profile.clone())
    }

    /// Access the page's BaoWebViewState for Worker lifecycle management.
    ///
    /// Used by BaoRuntime::create_worker to access Worker tracking,
    /// channel bridges, and scope states.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn webview_state(&self) -> Rc<RefCell<BaoWebViewState>> {
        self.inner
            .borrow()
            .as_ref()
            .map(|inner| inner.webview_state.clone())
            .unwrap_or_else(|| Rc::new(RefCell::new(BaoWebViewState::default())))
    }

    // ── High-level PageHandle API (REQ-LIB-001, REQ-LIB-004) ──────────────

    /// Wait for an element matching the selector to appear in the DOM.
    pub fn wait_for_selector(&self, selector: &str, timeout: Duration) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.wait_for_selector(selector, timeout))
    }

    /// Wait for a JS function/condition to return a truthy value.
    pub fn wait_for_function(
        &self,
        fn_expression: &str,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.wait_for_function(fn_expression, timeout))
    }

    /// Wait for page navigation to complete (URL change + load complete).
    pub fn wait_for_navigation(&self, timeout: Duration) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.wait_for_navigation(timeout))
    }

    /// Click an element matching the selector.
    pub fn click(&self, selector: &str) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.click_element(selector))
    }

    /// Type text into the currently focused element by dispatching key events.
    pub fn type_text(&self, text: &str) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.type_text(text))
    }

    /// Fill a form field identified by selector with the given value.
    pub fn fill(&self, selector: &str, value: &str) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.fill(selector, value))
    }

    /// Set the page HTML content via document.open/write/close.
    pub fn set_content(&self, html: &str) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.set_content(html))
    }

    /// Get the page HTML content via document.documentElement.outerHTML.
    pub fn content(&self) -> Result<String, BrowserError> {
        self.with_inner(|inner| inner.content())
    }

    /// Set viewport size.
    pub fn set_viewport(&self, width: u32, height: u32) -> Result<(), BrowserError> {
        self.with_inner(|inner| {
            inner.set_viewport(width, height);
            Ok(())
        })
    }

    /// Get cookies for the given URLs (or current page URL if empty).
    pub fn cookies(&self, urls: &[String]) -> Result<Vec<cookie::Cookie<'static>>, BrowserError> {
        self.with_inner(|inner| inner.cookies(urls))
    }

    /// Set a cookie for the given URL.
    pub fn set_cookie(
        &self,
        url: &str,
        cookie: cookie::Cookie<'static>,
    ) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.set_cookie(url, cookie))
    }

    /// Delete cookies matching the given name for the given URL.
    pub fn delete_cookie(&self, name: &str, url: Option<&str>) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.delete_cookie(name, url))
    }

    /// Select options in a <select> element identified by selector.
    pub fn select(&self, selector: &str, values: &[&str]) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.select(selector, values))
    }

    /// Press a key (e.g. "Enter", "Tab", "ArrowDown").
    pub fn press(&self, key: &str) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.press(key))
    }

    /// Hover over an element matching the selector.
    pub fn hover(&self, selector: &str) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.hover(selector))
    }

    /// Focus an element matching the selector.
    pub fn focus_element(&self, selector: &str) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.focus_element(selector))
    }

    /// Reload the page.
    pub fn reload(&self) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.reload())
    }

    /// Navigate back in history.
    pub fn go_back(&self) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.go_back())
    }

    /// Navigate forward in history.
    pub fn go_forward(&self) -> Result<(), BrowserError> {
        self.with_inner(|inner| inner.go_forward())
    }

    /// Check if back navigation is possible.
    pub fn can_go_back(&self) -> bool {
        self.with_inner_opt(|inner| Some(inner.can_go_back()))
            .unwrap_or(false)
    }

    /// Check if forward navigation is possible.
    pub fn can_go_forward(&self) -> bool {
        self.with_inner_opt(|inner| Some(inner.can_go_forward()))
            .unwrap_or(false)
    }

    /// Take screenshot with optional clip region and fullPage mode.
    pub fn take_screenshot_advanced(
        &self,
        format: ScreenshotFormat,
        clip: Option<(f64, f64, f64, f64)>,
        full_page: bool,
    ) -> Result<Vec<u8>, BrowserError> {
        self.with_inner(|inner| inner.take_screenshot_advanced(format, clip, full_page))
    }

    /// Dispatch a mouse button event at the given page coordinates.
    pub fn dispatch_mouse_event(
        &self,
        action: MouseButtonAction,
        button: MouseButton,
        x: f32,
        y: f32,
    ) -> Result<(), BrowserError> {
        self.with_inner(|inner| {
            inner.dispatch_mouse_event(action, button, x, y);
            Ok(())
        })
    }

    /// Dispatch a mouse move event at the given page coordinates.
    pub fn dispatch_mouse_move(&self, x: f32, y: f32) -> Result<(), BrowserError> {
        self.with_inner(|inner| {
            inner.dispatch_mouse_move(x, y);
            Ok(())
        })
    }

    /// Dispatch a keyboard event.
    pub fn dispatch_key_event(
        &self,
        state: KeyState,
        key: Key,
        code: Code,
    ) -> Result<(), BrowserError> {
        self.with_inner(|inner| {
            inner.dispatch_key_event(state, key, code);
            Ok(())
        })
    }

    pub fn close(&self) -> Result<(), BrowserError> {
        let mut borrow = self.inner.borrow_mut();
        if let Some(inner) = borrow.take() {
            // SM PageLifecycle (SPEC 03-PROCESS): transition to Closing FIRST,
            // before any cleanup. Covers: close_during_load / idle_ttl_expired / close.
            // @trace REQ-BRW-001 [sm:PageLifecycle] criterion: Closing state
            *inner.state.borrow_mut() = PageState::Closing;
            // @trace REQ-BRW-004 [entity:Worker] [criterion:10]
            // SPEC criterion #10: "页面卸载时自动终止所有 Worker
            // (GlobalScope::track_worker + AutoCloseWorker)".
            // Explicitly terminate all Workers BEFORE dropping PageInner,
            // ensuring correct teardown order:
            //   1. Set closing flags + unregister stealth profiles (while JSContext alive)
            //   2. Drop WebWorker instances (join threads)
            //   3. AutoCloseWorker::Drop runs as idempotent cleanup
            // Without this, BaoWebViewState field Drop order would drop web_workers
            // (which joins threads) BEFORE active_workers (AutoCloseWorker), causing
            // stealth profile unregistration to happen after thread exit.
            {
                let mut ws = inner.webview_state.borrow_mut();
                if ws.active_worker_count() > 0 {
                    log::debug!(
                        "[page] close: terminating {} active workers",
                        ws.active_worker_count()
                    );
                    ws.terminate_all_workers();
                }
            }
            let pg = *inner.page_global.borrow();
            let ng = *inner.node_realm_global.borrow();
            // BCE-20260621-001: remove per-page Node Realm entries via WebViewId
            // (NOT raw *mut JSObject). The raw pointers are kept locally only to
            // drop the stealth profile mappings.
            if let Some(wid) = inner.webview_id_opt() {
                crate::runtime_bridge::remove_node_realm_by_id(wid);
            }
            if !pg.is_null() {
                // BUG-ENG-366: drop the per-Realm stealth profiles so the next
                // page reusing the same global address does not inherit a stale
                // fingerprint. @trace REQ-SEC-002 [req:REQ-SEC-002] [req:BUG-ENG-366]
                bao_stealth::engine_props::remove_profile_for_global(pg as usize);
            }
            if !ng.is_null() {
                bao_stealth::engine_props::remove_profile_for_global(ng as usize);
            }
            // SM PageLifecycle: cleanup_complete → Closed
            // @trace REQ-BRW-001 [sm:PageLifecycle] criterion: cleanup_complete transition
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
    pub fn set_page_global(
        &self,
        page_global: *mut mozjs::jsapi::JSObject,
        node_global: *mut mozjs::jsapi::JSObject,
    ) {
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

/// Map a character to its keyboard Code value for type_text dispatch.
fn key_code_for_char(ch: char) -> Code {
    match ch {
        'a' => Code::KeyA,
        'b' => Code::KeyB,
        'c' => Code::KeyC,
        'd' => Code::KeyD,
        'e' => Code::KeyE,
        'f' => Code::KeyF,
        'g' => Code::KeyG,
        'h' => Code::KeyH,
        'i' => Code::KeyI,
        'j' => Code::KeyJ,
        'k' => Code::KeyK,
        'l' => Code::KeyL,
        'm' => Code::KeyM,
        'n' => Code::KeyN,
        'o' => Code::KeyO,
        'p' => Code::KeyP,
        'q' => Code::KeyQ,
        'r' => Code::KeyR,
        's' => Code::KeyS,
        't' => Code::KeyT,
        'u' => Code::KeyU,
        'v' => Code::KeyV,
        'w' => Code::KeyW,
        'x' => Code::KeyX,
        'y' => Code::KeyY,
        'z' => Code::KeyZ,
        'A' => Code::KeyA,
        'B' => Code::KeyB,
        'C' => Code::KeyC,
        'D' => Code::KeyD,
        'E' => Code::KeyE,
        'F' => Code::KeyF,
        'G' => Code::KeyG,
        'H' => Code::KeyH,
        'I' => Code::KeyI,
        'J' => Code::KeyJ,
        'K' => Code::KeyK,
        'L' => Code::KeyL,
        'M' => Code::KeyM,
        'N' => Code::KeyN,
        'O' => Code::KeyO,
        'P' => Code::KeyP,
        'Q' => Code::KeyQ,
        'R' => Code::KeyR,
        'S' => Code::KeyS,
        'T' => Code::KeyT,
        'U' => Code::KeyU,
        'V' => Code::KeyV,
        'W' => Code::KeyW,
        'X' => Code::KeyX,
        'Y' => Code::KeyY,
        'Z' => Code::KeyZ,
        '0' => Code::Digit0,
        '1' => Code::Digit1,
        '2' => Code::Digit2,
        '3' => Code::Digit3,
        '4' => Code::Digit4,
        '5' => Code::Digit5,
        '6' => Code::Digit6,
        '7' => Code::Digit7,
        '8' => Code::Digit8,
        '9' => Code::Digit9,
        '\n' => Code::Enter,
        '\t' => Code::Tab,
        '\u{8}' => Code::Backspace,
        '\u{7f}' => Code::Delete,
        ' ' => Code::Space,
        ';' => Code::Semicolon,
        '=' => Code::Equal,
        ',' => Code::Comma,
        '-' => Code::Minus,
        '.' => Code::Period,
        '/' => Code::Slash,
        '`' => Code::Backquote,
        '[' => Code::BracketLeft,
        '\\' => Code::Backslash,
        ']' => Code::BracketRight,
        '\'' => Code::Quote,
        _ => Code::Unidentified,
    }
}

/// Parse a key name string (e.g. "Enter", "ArrowDown", "a") into (Key, Code).
fn parse_key_name(name: &str) -> (Key, Code) {
    match name {
        "Enter" => (Key::Named(NamedKey::Enter), Code::Enter),
        "Tab" => (Key::Named(NamedKey::Tab), Code::Tab),
        "Escape" | "Esc" => (Key::Named(NamedKey::Escape), Code::Escape),
        "Backspace" => (Key::Named(NamedKey::Backspace), Code::Backspace),
        "Delete" => (Key::Named(NamedKey::Delete), Code::Delete),
        "Space" => (Key::Character(" ".into()), Code::Space),
        "ArrowUp" => (Key::Named(NamedKey::ArrowUp), Code::ArrowUp),
        "ArrowDown" => (Key::Named(NamedKey::ArrowDown), Code::ArrowDown),
        "ArrowLeft" => (Key::Named(NamedKey::ArrowLeft), Code::ArrowLeft),
        "ArrowRight" => (Key::Named(NamedKey::ArrowRight), Code::ArrowRight),
        "Home" => (Key::Named(NamedKey::Home), Code::Home),
        "End" => (Key::Named(NamedKey::End), Code::End),
        "PageUp" => (Key::Named(NamedKey::PageUp), Code::PageUp),
        "PageDown" => (Key::Named(NamedKey::PageDown), Code::PageDown),
        "Insert" => (Key::Named(NamedKey::Insert), Code::Insert),
        "F1" => (Key::Named(NamedKey::F1), Code::F1),
        "F2" => (Key::Named(NamedKey::F2), Code::F2),
        "F3" => (Key::Named(NamedKey::F3), Code::F3),
        "F4" => (Key::Named(NamedKey::F4), Code::F4),
        "F5" => (Key::Named(NamedKey::F5), Code::F5),
        "F6" => (Key::Named(NamedKey::F6), Code::F6),
        "F7" => (Key::Named(NamedKey::F7), Code::F7),
        "F8" => (Key::Named(NamedKey::F8), Code::F8),
        "F9" => (Key::Named(NamedKey::F9), Code::F9),
        "F10" => (Key::Named(NamedKey::F10), Code::F10),
        "F11" => (Key::Named(NamedKey::F11), Code::F11),
        "F12" => (Key::Named(NamedKey::F12), Code::F12),
        "ControlLeft" | "Control" => (Key::Named(NamedKey::Control), Code::ControlLeft),
        "ControlRight" => (Key::Named(NamedKey::Control), Code::ControlRight),
        "ShiftLeft" | "Shift" => (Key::Named(NamedKey::Shift), Code::ShiftLeft),
        "ShiftRight" => (Key::Named(NamedKey::Shift), Code::ShiftRight),
        "AltLeft" | "Alt" => (Key::Named(NamedKey::Alt), Code::AltLeft),
        "AltRight" => (Key::Named(NamedKey::Alt), Code::AltRight),
        "MetaLeft" | "Meta" => (Key::Named(NamedKey::Meta), Code::MetaLeft),
        "MetaRight" => (Key::Named(NamedKey::Meta), Code::MetaRight),
        "CapsLock" => (Key::Named(NamedKey::CapsLock), Code::CapsLock),
        "NumLock" => (Key::Named(NamedKey::NumLock), Code::NumLock),
        "ScrollLock" => (Key::Named(NamedKey::ScrollLock), Code::ScrollLock),
        // Single character
        s if s.chars().count() == 1 => {
            let ch = s.chars().next().unwrap();
            let key = Key::Character(ch.to_string());
            let code = key_code_for_char(ch);
            (key, code)
        }
        // Fallback: treat as character key
        s => (Key::Character(s.to_string()), Code::Unidentified),
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
        assert_eq!(PageState::Closing, PageState::Closing);
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
        assert!(format!("{:?}", PageState::Closing).contains("Closing"));
        assert!(format!("{:?}", PageState::Closed).contains("Closed"));
    }

    #[test]
    fn page_state_closing_distinct_from_neighbors() {
        // SM PageLifecycle: Closing is a distinct intermediate state, not equal
        // to its entry (Idle/Interactive/Navigating) or exit (Closed) neighbors.
        assert_ne!(PageState::Closing, PageState::Idle);
        assert_ne!(PageState::Closing, PageState::Interactive);
        assert_ne!(PageState::Closing, PageState::Closed);
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
        let func_start = source
            .find("pub fn evaluate_js(&self, script: &str)")
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
        // Must detect null Node Realm as programming error (eager init at create_page)
        assert!(
            func_body.contains("eager init failed"),
            "REQ-SEC-002 REGRESSION: evaluate_js must detect uninitialized Node Realm"
        );
    }

    /// Verify evaluate_js drain callbacks after Node Realm execution.
    /// REQ-SEC-002: Results must be read after servo script thread callback.
    #[test]
    fn evaluate_js_drains_callbacks_for_result() {
        let source = include_str!("page.rs");
        let func_start = source
            .find("pub fn evaluate_js(&self, script: &str)")
            .expect("evaluate_js function not found");
        let func_body = &source[func_start..func_start + 2800.min(source.len() - func_start)];
        assert!(
            func_body.contains("drain_callbacks"),
            "REQ-SEC-002 REGRESSION: evaluate_js must drain callbacks after Node Realm execution"
        );
    }

    /// Verify evaluate_js reads result from shared EvaluateResult.
    /// REQ-SEC-002: Result must come from Arc<OnceLock<EvaluateResult>>.
    #[test]
    fn evaluate_js_reads_evaluate_result() {
        let source = include_str!("page.rs");
        let func_start = source
            .find("pub fn evaluate_js(&self, script: &str)")
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
        let func_start = source
            .find("unsafe fn install_all_native")
            .expect("install_all_native function not found");
        let func_body = &source[func_start..func_start + 5000.min(source.len() - func_start)];

        assert!(
            func_body.contains("bun_runtime::fetch_api::install_fetch_global"),
            "REQ-SEC-003 REGRESSION: install_all_native must install Web APIs (fetch)"
        );
        assert!(
            func_body.contains("bun_runtime::timers::install_timer_globals"),
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
        let func_start = source
            .find("unsafe fn create_node_realm_native")
            .expect("create_node_realm_native function not found");
        let func_end = source[func_start..]
            .find("pub fn inject_node_apis")
            .or_else(|| source[func_start..].find("/// Inject Node.js APIs as native"))
            .expect("end boundary not found");
        let func_body = &source[func_start..func_start + func_end];

        assert!(
            func_body.contains("bun_runtime::globals::install_node_apis"),
            "REQ-SEC-002 REGRESSION: create_node_realm_native must install Node APIs on Node Realm global"
        );
        assert!(
            func_body.contains("bun_runtime::globals::install_web_apis"),
            "REQ-SEC-002: Node Realm must also have Web APIs for trusted scripts"
        );
    }

    /// Verify Node Realm is in its own Compartment (NewCompartmentAndZone).
    /// REQ-SEC-002: Physical isolation via SpiderMonkey Compartment boundary.
    #[test]
    fn node_realm_uses_new_compartment() {
        let source = include_str!("runtime_bridge.rs");
        let func_start = source
            .find("unsafe fn create_node_realm_native")
            .expect("create_node_realm_native function not found");
        let func_body = &source[func_start..func_start + 3000.min(source.len() - func_start)];
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

        // Locate the evaluate_in_node_realm function body specifically.
        let func_start = source
            .find("pub unsafe fn evaluate_in_node_realm")
            .expect("evaluate_in_node_realm function not found");
        let func_body_start = source[func_start..]
            .find("{")
            .expect("function body start not found");
        let search_limit = source[func_start + func_body_start..]
            .find("unsafe fn create_node_realm_native")
            .unwrap_or(3000)
            .min(3000);
        let func_body =
            &source[func_start + func_body_start..func_start + func_body_start + search_limit];

        assert!(
            func_body.contains("AutoRealm::new"),
            "REQ-SEC-002 REGRESSION: evaluate_in_node_realm must use AutoRealm"
        );
    }

    /// Verify per-page Node Realm storage exists (REQ-SEC-002).
    /// BCE-20260621-001: WebViewId-keyed storage (NOT *mut JSObject-keyed).
    #[test]
    fn node_realm_global_stored_per_page() {
        let source = include_str!("runtime_bridge.rs");
        assert!(
            source.contains("NODE_REALM_BY_WEBVIEW"),
            "REQ-SEC-002 REGRESSION: must have NODE_REALM_BY_WEBVIEW per-page storage"
        );
        assert!(
            source.contains("store_node_realm"),
            "REQ-SEC-002 REGRESSION: must have store_node_realm accessor"
        );
        assert!(
            source.contains("get_node_realm_by_id"),
            "REQ-SEC-002 REGRESSION: must have get_node_realm_by_id accessor"
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

    // ── Key mapping helper tests ──────────────────────────────────────────
    // @trace REQ-LIB-001 [req:REQ-LIB-001] [level:unit]

    #[test]
    fn key_code_for_char_letters() {
        assert_eq!(super::key_code_for_char('a'), Code::KeyA);
        assert_eq!(super::key_code_for_char('Z'), Code::KeyZ);
    }

    #[test]
    fn key_code_for_char_digits() {
        assert_eq!(super::key_code_for_char('0'), Code::Digit0);
        assert_eq!(super::key_code_for_char('9'), Code::Digit9);
    }

    #[test]
    fn key_code_for_char_special() {
        assert_eq!(super::key_code_for_char('\n'), Code::Enter);
        assert_eq!(super::key_code_for_char('\t'), Code::Tab);
        assert_eq!(super::key_code_for_char(' '), Code::Space);
    }

    #[test]
    fn parse_key_name_enter() {
        let (key, code) = super::parse_key_name("Enter");
        assert!(matches!(key, Key::Named(NamedKey::Enter)));
        assert_eq!(code, Code::Enter);
    }

    #[test]
    fn parse_key_name_arrow_keys() {
        let (key, code) = super::parse_key_name("ArrowDown");
        assert!(matches!(key, Key::Named(NamedKey::ArrowDown)));
        assert_eq!(code, Code::ArrowDown);

        let (key, code) = super::parse_key_name("ArrowUp");
        assert!(matches!(key, Key::Named(NamedKey::ArrowUp)));
        assert_eq!(code, Code::ArrowUp);
    }

    #[test]
    fn parse_key_name_single_char() {
        let (key, code) = super::parse_key_name("a");
        assert!(matches!(key, Key::Character(s) if s == "a"));
        assert_eq!(code, Code::KeyA);
    }

    #[test]
    fn parse_key_name_function_keys() {
        let (key, code) = super::parse_key_name("F1");
        assert!(matches!(key, Key::Named(NamedKey::F1)));
        assert_eq!(code, Code::F1);
    }

    #[test]
    fn parse_key_name_escape_aliases() {
        let (key, code) = super::parse_key_name("Escape");
        assert!(matches!(key, Key::Named(NamedKey::Escape)));
        assert_eq!(code, Code::Escape);

        let (key, code) = super::parse_key_name("Esc");
        assert!(matches!(key, Key::Named(NamedKey::Escape)));
        assert_eq!(code, Code::Escape);
    }

    // ── PageHandle high-level API existence tests ───────────────────────
    // @trace REQ-LIB-001 [req:REQ-LIB-001] [level:unit]

    #[test]
    fn page_inner_has_wait_for_selector() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn wait_for_selector("),
            "REQ-LIB-001: PageInner must have wait_for_selector method"
        );
    }

    #[test]
    fn page_inner_has_wait_for_navigation() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn wait_for_navigation("),
            "REQ-LIB-001: PageInner must have wait_for_navigation method"
        );
    }

    #[test]
    fn page_inner_has_wait_for_function() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn wait_for_function("),
            "REQ-LIB-001: PageInner must have wait_for_function method"
        );
    }

    #[test]
    fn page_inner_has_click_element() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn click_element("),
            "REQ-LIB-001: PageInner must have click_element method"
        );
    }

    #[test]
    fn page_inner_has_type_text() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn type_text("),
            "REQ-LIB-001: PageInner must have type_text method"
        );
    }

    #[test]
    fn page_inner_has_fill() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn fill("),
            "REQ-LIB-001: PageInner must have fill method"
        );
    }

    #[test]
    fn page_inner_has_set_content() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn set_content("),
            "REQ-LIB-001: PageInner must have set_content method"
        );
    }

    #[test]
    fn page_inner_has_content() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn content("),
            "REQ-LIB-001: PageInner must have content method"
        );
    }

    #[test]
    fn page_inner_has_reload_go_back_go_forward() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn reload(&self)"),
            "REQ-LIB-001: PageInner must have reload method"
        );
        assert!(
            source.contains("pub fn go_back(&self)"),
            "REQ-LIB-001: PageInner must have go_back method"
        );
        assert!(
            source.contains("pub fn go_forward(&self)"),
            "REQ-LIB-001: PageInner must have go_forward method"
        );
    }

    #[test]
    fn page_inner_has_dispatch_mouse_event() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn dispatch_mouse_event("),
            "REQ-LIB-001: PageInner must have dispatch_mouse_event method"
        );
    }

    #[test]
    fn page_inner_has_dispatch_key_event() {
        let source = include_str!("page.rs");
        assert!(
            source.contains("pub fn dispatch_key_event("),
            "REQ-LIB-001: PageInner must have dispatch_key_event method"
        );
    }

    #[test]
    fn page_handle_has_high_level_api() {
        let source = include_str!("page.rs");
        // PageHandle delegates
        assert!(
            source.contains("pub fn click(&self"),
            "PageHandle must have click"
        );
        assert!(
            source.contains("pub fn type_text(&self"),
            "PageHandle must have type_text"
        );
        assert!(
            source.contains("pub fn fill(&self"),
            "PageHandle must have fill"
        );
        assert!(
            source.contains("pub fn set_content(&self"),
            "PageHandle must have set_content"
        );
        assert!(
            source.contains("pub fn content(&self"),
            "PageHandle must have content"
        );
        assert!(
            source.contains("pub fn press(&self"),
            "PageHandle must have press"
        );
        assert!(
            source.contains("pub fn hover(&self"),
            "PageHandle must have hover"
        );
        assert!(
            source.contains("pub fn focus_element(&self"),
            "PageHandle must have focus_element"
        );
        assert!(
            source.contains("pub fn reload(&self"),
            "PageHandle must have reload"
        );
        assert!(
            source.contains("pub fn go_back(&self"),
            "PageHandle must have go_back"
        );
        assert!(
            source.contains("pub fn go_forward(&self"),
            "PageHandle must have go_forward"
        );
        assert!(
            source.contains("pub fn select(&self"),
            "PageHandle must have select"
        );
        assert!(
            source.contains("pub fn set_viewport(&self"),
            "PageHandle must have set_viewport"
        );
        assert!(
            source.contains("pub fn cookies(&self"),
            "PageHandle must have cookies"
        );
        assert!(
            source.contains("pub fn set_cookie(&self"),
            "PageHandle must have set_cookie"
        );
        assert!(
            source.contains("pub fn delete_cookie(&self"),
            "PageHandle must have delete_cookie"
        );
    }
}
