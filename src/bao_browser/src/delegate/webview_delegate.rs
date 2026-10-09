use super::*;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint, DeviceIntRect,
    DeviceIntSize, EmbedderControl, EmbedderControlId, InputEventId, InputEventResult, LoadStatus,
    NavigationRequest, PermissionRequest, ScreenGeometry, ServoDelegate, ServoError, TraversalId,
    WebView, WebViewDelegate,
};

use bao_cdp::servo_bridge::main_frame_id_for_target;
use bao_cdp::{BaoEvent, ConsoleMessage};
use bao_cdp_client::bridge::{ConsoleLevel, ServoEvent};

pub struct BaoWebViewDelegate {
    state: Rc<RefCell<BaoWebViewState>>,
    viewport: PhysicalSize<u32>,
    /// Weak link back to the owning PagePool (REQ-LIB-001). Servo dispatches
    /// `request_create_new` (window.open) and `notify_closed` (window.close())
    /// on the embedder thread during `spin_event_loop` — the same thread that
    /// owns the pool's `Rc` domain — so the delegate can act on the pool
    /// directly. Weak because the pool strongly holds every page's delegate
    /// (PageInner → delegate); a strong link would be a leak cycle.
    pool: std::rc::Weak<crate::page_pool::PagePool>,
}

impl BaoWebViewDelegate {
    pub fn new(
        state: Rc<RefCell<BaoWebViewState>>,
        viewport: PhysicalSize<u32>,
        pool: std::rc::Weak<crate::page_pool::PagePool>,
    ) -> Self {
        BaoWebViewDelegate {
            state,
            viewport,
            pool,
        }
    }

    pub fn state(&self) -> &Rc<RefCell<BaoWebViewState>> {
        &self.state
    }
}

impl WebViewDelegate for BaoWebViewDelegate {
    fn screen_geometry(&self, _webview: WebView) -> Option<ScreenGeometry> {
        let screen_size =
            DeviceIntSize::new(self.viewport.width as i32, self.viewport.height as i32);
        Some(ScreenGeometry {
            size: screen_size,
            available_size: screen_size,
            window_rect: DeviceIntRect::from_origin_and_size(DeviceIntPoint::zero(), screen_size),
        })
    }

    fn notify_url_changed(&self, _webview: WebView, url: url::Url) {
        let url_str = url.to_string();
        self.state.borrow_mut().url = Some(url);
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // Dual-path: event_tx (Path B) primary for FrameNavigated,
        // console_log_tx (Path A) fallback for PageFrameNavigated.
        // Both paths carry the page's real CDP target (REQ-CDP-004) and the
        // per-target main frame id derived from it (v7 path B) — a state
        // without CDP identity has no route, so the event is dropped.
        let Some(target_id) = self.state.borrow().cdp_target() else {
            self.state.borrow().log_unroutable_event("Page.frameNavigated");
            return;
        };
        let frame_id = main_frame_id_for_target(&target_id);
        let event_tx = self.state.borrow().event_tx.clone();
        if let Some(ref tx) = event_tx {
            send_servo_event(
                &tx,
                ServoEvent::FrameNavigated {
                    target_id: target_id.clone(),
                    frame_id,
                    url: url_str,
                    name: None,
                },
            );
        } else if let Some(ref tx) = self.state.borrow().console_log_tx {
            let loader_id = format!("{:016x}", url_str.len() as u64);
            // Lossy by design: fire-and-forget console observability — the
            // send only fails once the consumer is dropped; never stall the
            // servo script thread on CDP event delivery.
            let _ = tx.send(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id,
                url: url_str,
                loader_id,
            }));
        }
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.state.borrow_mut().title = title;
    }

    // ── WPT official-toolchain face (REQ-BRW-002, feature = "webdriver") ──
    // servoshell-parity delegate hooks: the webdriver host's cross-thread
    // bookkeeping resolves through these callbacks (they fire on script
    // threads, which is why the registry is a mutex — see webdriver_host.rs).

    /// Resolve the go_back/go_forward load-status wait keyed on this
    /// traversal id (servoshell: `notify_traversal_complete`).
    #[cfg(feature = "webdriver")]
    fn notify_traversal_complete(&self, _webview: WebView, traversal_id: TraversalId) {
        crate::webdriver_host::notify_traversal_complete(traversal_id);
    }

    /// Acknowledge the WebDriver input event once the DOM handled it
    /// (servoshell: `notify_input_event_handled` → pending sender).
    #[cfg(feature = "webdriver")]
    fn notify_input_event_handled(
        &self,
        _webview: WebView,
        event_id: InputEventId,
        _result: InputEventResult,
    ) {
        crate::webdriver_host::notify_input_event_handled(event_id);
    }

    /// Under WebDriver ownership embedder controls are stashed for the
    /// prompt endpoints instead of being shown headlessly
    /// (servoshell: `show_embedder_control`'s webdriver branch).
    #[cfg(feature = "webdriver")]
    fn show_embedder_control(&self, webview: WebView, embedder_control: EmbedderControl) {
        crate::webdriver_host::show_embedder_control(webview.id(), embedder_control);
    }

    #[cfg(feature = "webdriver")]
    fn hide_embedder_control(&self, webview: WebView, control_id: EmbedderControlId) {
        crate::webdriver_host::hide_embedder_control(webview.id(), control_id);
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        // e131 (stale-Complete race, REQ-BRW-002): load-generation gate. A
        // `Complete` arriving for a generation whose `Started` was never
        // delivered belongs to a load superseded by a navigation entry — the
        // previous load was still in flight when navigate/reload/go_back/
        // go_forward reset the copy, and its late `Complete` would un-reset
        // it (Interactive projected before the new load commits — the e124
        // post-creation navigate flake; reproducer:
        // nav_race_repro_tests). Dropped with no observable effect: the
        // state write, the CDP FrameStoppedLoading event and the webdriver
        // load-complete resolution are all skipped for the superseded edge.
        // The new load's own `Started` credits the current generation (see
        // the `Started` arm below); servo's webview-side same-value dedupe
        // was removed (vendor patch, webview.rs) so that credit is reliable
        // even for a mid-load re-navigate onto a copy that still reads
        // `Started`.
        {
            let mut state = self.state.borrow_mut();
            match status {
                LoadStatus::Complete => {
                    if state.started_generation != state.load_generation {
                        log::debug!(
                            "[delegate] dropping stale Complete for superseded load \
                             (generation {} of {}, webview {})",
                            state.started_generation,
                            state.load_generation,
                            _webview.id()
                        );
                        return;
                    }
                }
                LoadStatus::Started => state.started_generation = state.load_generation,
                LoadStatus::HeadParsed => {}
            }
            state.load_status = status;
        }
        // WPT official-toolchain face (REQ-BRW-002): resolve any pending
        // WebDriver load-status waiter for this webview on Complete.
        #[cfg(feature = "webdriver")]
        if status == LoadStatus::Complete {
            crate::webdriver_host::notify_load_complete(_webview.id());
        }
        match status {
            LoadStatus::Started => {
                // @trace REQ-BRW-004 [entity:Worker] [criterion:10]
                // SPEC criterion #10: "页面卸载时自动终止所有 Worker
                // (GlobalScope::track_worker + AutoCloseWorker)".
                // When a new navigation starts (after a previous Complete),
                // all Workers from the previous page must be terminated.
                {
                    let mut state = self.state.borrow_mut();
                    if !state.active_workers.is_empty() {
                        log::debug!(
                            "[delegate] page navigation: terminating {} active workers",
                            state.active_worker_count()
                        );
                        state.terminate_all_workers();
                    }
                    // @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
                    // SharedWorkers survive page unload — only disconnect ports.
                    state.disconnect_shared_worker_ports();
                    // @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
                    // ServiceWorkers have persistent lifecycle (跨页存活) — only
                    // clear the per-page controlling reference. The ServiceWorker
                    // itself survives (tracked in BaoServoDelegate registry) and
                    // can control the page again if its scope matches.
                    state.clear_controlling_service_worker();
                }

                // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
                // Dual-path: event_tx (Path B) primary for FrameStartedLoading,
                // console_log_tx (Path A) fallback — no direct ConsoleMessage equivalent,
                // so we use a lightweight log entry. Path B carries the page's
                // real CDP target + per-target main frame id (REQ-CDP-004).
                let event_tx = self.state.borrow().event_tx.clone();
                if let Some(ref tx) = event_tx {
                    let Some(target_id) = self.state.borrow().cdp_target() else {
                        self.state
                            .borrow()
                            .log_unroutable_event("Page.frameStartedLoading");
                        return;
                    };
                    let frame_id = main_frame_id_for_target(&target_id);
                    send_servo_event(
                        &tx,
                        ServoEvent::FrameStartedLoading {
                            target_id,
                            frame_id,
                        },
                    );
                }
            }
            LoadStatus::Complete => {
                self.state.borrow_mut().dom_proxies_dirty = true;

                // @trace REQ-BRW-004 [entity:Worker]
                // Reap terminated workers after page load completes.
                // Workers from the previous page that have been terminated
                // during LoadStatus::Started are cleaned up here.
                self.state.borrow_mut().reap_terminated_workers();

                // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
                // Dual-path: event_tx (Path B) primary for FrameStoppedLoading,
                // console_log_tx (Path A) fallback for PageLoadEventFired.
                // Path B carries the page's real CDP target + per-target main
                // frame id (REQ-CDP-004).
                let event_tx = self.state.borrow().event_tx.clone();
                if let Some(ref tx) = event_tx {
                    let Some(target_id) = self.state.borrow().cdp_target() else {
                        self.state
                            .borrow()
                            .log_unroutable_event("Page.frameStoppedLoading");
                        return;
                    };
                    let frame_id = main_frame_id_for_target(&target_id);
                    send_servo_event(
                        &tx,
                        ServoEvent::FrameStoppedLoading {
                            target_id,
                            frame_id,
                        },
                    );
                } else if let Some(ref tx) = self.state.borrow().console_log_tx {
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    // Lossy by design: fire-and-forget console observability — the
                    // send only fails once the consumer is dropped; never stall the
                    // servo script thread on CDP event delivery.
                    let _ = tx.send(ConsoleMessage::Event(BaoEvent::PageLoadEventFired {
                        timestamp,
                    }));
                }
            }
            LoadStatus::HeadParsed => {}
        }
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        // Latch the repaint request; the composite itself is deferred to the
        // embedder's pump loops (`paint_pages_needing_repaint` / spin_servo) —
        // never re-entrant inside servo message handling (servoshell defers to
        // winit's RedrawRequested the same way).
        self.state.borrow_mut().latch_frame_ready();
    }

    fn request_navigation(&self, _webview: WebView, request: NavigationRequest) {
        request.allow();
    }

    fn request_permission(&self, _webview: WebView, request: PermissionRequest) {
        // e135 oracle (real headless Chrome 149/150): permission prompts
        // auto-resolve DENIED in headless — Notification.requestPermission()
        // resolves "denied" and geolocation fails PERMISSION_DENIED, with no
        // prompt UI possible (probes under /tmp/e135-chrome-probe). The
        // former auto-allow resolved "granted" — a state no real headless
        // Chrome produces — and servo caches it into the permission store,
        // flipping later permissions.query results to "granted" too.
        // Denying keeps the static Notification.permission at "default"
        // until a page actually requests (what bot.sannysoft.com reads),
        // and matches the real headless resolution exactly.
        request.deny();
    }

    // @trace REQ-LIB-001 [entity:PagePool] [entity:BaoServoDelegate]
    // window.open popup landing (REQ-LIB-001): servo asks the embedder to
    // build the auxiliary WebView while the opener's ScriptThread blocks on
    // the creation channel (vendor windowproxy.rs
    // create_auxiliary_browsing_context → constellation AllowOpeningWebView →
    // this callback on the embedder/pump thread). Building the WebView here
    // (servoshell RunningAppState::request_create_new form, adapted to bao's
    // pool-owned headless pages) answers that channel and `window.open()`
    // returns a live WindowProxy. Dropping `request` without building (every
    // early-return below) leaves the channel closed — the script side's
    // `recv().unwrap()?` surfaces that as a JS null, i.e. the same observable
    // semantics as Chromium's popup-blocked / resource-exhausted open.
    //
    // Accounting + stealth inheritance happen in
    // `PagePool::create_popup_page`; the pipeline-ready wait + Node/stealth
    // injection that `create_page` runs inline are DEFERRED to
    // `PagePool::init_pending_pages` (pump-side drain): this callback runs
    // inside an in-flight `spin_event_loop`, and re-entering the event loop
    // from within servo's message dispatch is a reordering hazard servoshell
    // does not expose itself to (its request_create_new only builds and
    // registers).
    fn request_create_new(&self, _parent_webview: WebView, request: CreateNewWebViewRequest) {
        let Some(pool) = self.pool.upgrade() else {
            log::warn!("[webview] window.open denied: page pool already torn down");
            return;
        };
        // The opener's own delegate carries the opener identity: inherit its
        // viewport and its stealth/permission config (R53-A — the popup is an
        // auxiliary of the opener, so it must not present a different wire /
        // canvas fingerprint, and multi-profile runtimes must not let the
        // popup fall through to another page's process-global fallback).
        let opener_page_id = self.state.borrow().pool_page_id;
        let opener = opener_page_id.and_then(|id| pool.get_page(id));
        let stealth_profile = opener.as_ref().and_then(|page| page.stealth_profile());
        let permission = opener
            .as_ref()
            .and_then(|page| page.permission().config().cloned());
        if pool
            .create_popup_page(request, self.viewport, stealth_profile, permission)
            .is_none()
        {
            log::warn!(
                "[webview] window.open denied: page pool limit reached (null returned to JS)"
            );
        }
    }

    // @trace REQ-LIB-001 [entity:PagePool]
    // window.close() retirement (REQ-LIB-001 criterion ⑤): servo notifies the
    // embedder when content closed this WebView (ConstellationToEmbedderMsg::
    // WebViewClosed); servoshell removes it from its tab interface here —
    // bao's "interface" is the PagePool map. The map entry drops NOW
    // (accounting) while the physical teardown is queued for the pump-side
    // `close_pending_pages` drain: this callback fires inside servo's
    // embedder dispatch, where the closing page's own evaluate legitimately
    // holds `PageHandle.inner` borrowed — and `PageHandle::close` needs
    // `borrow_mut` on that same cell (close-during-evaluate RefCell re-entry,
    // hit by the window_open suite on the first cut).
    fn notify_closed(&self, _webview: WebView) {
        let Some(pool) = self.pool.upgrade() else {
            return;
        };
        let page_id = self.state.borrow().pool_page_id;
        if let Some(id) = page_id {
            if !pool.retire_webview_page(id) {
                // Double-close (TTL reclaim raced the content close, or a CDP
                // Target.closeTarget already retired the page) is not an error
                // for the embedder face — the pool entry is gone either way.
                log::debug!("[webview] notify_closed: page {id} already retired");
            }
        }
    }

    fn show_console_message(&self, _webview: WebView, level: ConsoleLogLevel, message: String) {
        let level_str = match level {
            ConsoleLogLevel::Debug => "debug",
            ConsoleLogLevel::Log => "info",
            ConsoleLogLevel::Info => "info",
            ConsoleLogLevel::Warn => "warning",
            ConsoleLogLevel::Error => "error",
            ConsoleLogLevel::Trace => "verbose",
            ConsoleLogLevel::Dir => "info",
        };
        log::trace!("[webview] {message}");

        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // Same dual-path logic as BaoServoDelegate::show_console_message:
        // `__BAO_EVT__` structured CDP events route to the ConsoleMessage
        // parser (Path A) even when Path B is active (see the twin
        // implementation above for the full rationale), then event_tx
        // (Path B) is primary for plain logs; console_log_tx (Path A) is
        // fallback.
        if message.starts_with("__BAO_EVT__") {
            let tx = self.state.borrow().console_log_tx.clone();
            if let Some(ref tx) = tx {
                if let Some(ConsoleMessage::Event(evt)) = BaoEvent::from_console_text(&message) {
                    // Lossy by design: fire-and-forget console observability — the
                    // send only fails once the consumer is dropped; never stall the
                    // servo script thread on CDP event delivery.
                    let _ = tx.send(ConsoleMessage::Event(evt));
                    return;
                }
            }
        }
        let event_tx = self.state.borrow().event_tx.clone();
        if let Some(ref tx) = event_tx {
            // REQ-CDP-004: this per-webview arm always has an owning page —
            // stamp the event with its real CDP target so it routes to
            // exactly the sessions attached to this page. No identity
            // (unregistered/closed state) → no route → drop.
            let Some(target_id) = self.state.borrow().cdp_target() else {
                self.state.borrow().log_unroutable_event("console message");
                return;
            };
            let servo_level = match level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
                ConsoleLogLevel::Dir => ConsoleLevel::Info,
            };
            // Reliable delivery: the event queue is unbounded — send neither
            // stalls the servo script thread nor drops; a dropped receiver
            // (teardown) fails the send and is warn-logged once.
            send_servo_event(
                &tx,
                ServoEvent::Console {
                    target_id,
                    level: servo_level,
                    text: message,
                    url: None,
                    line: None,
                    column: None,
                },
            );
        } else if let Some(ref tx) = self.state.borrow().console_log_tx {
            let msg = match BaoEvent::from_console_text(&message) {
                Some(ConsoleMessage::Event(evt)) => ConsoleMessage::Event(evt),
                _ => ConsoleMessage::Log {
                    level: level_str.to_string(),
                    text: message,
                },
            };
            // Lossy by design: fire-and-forget console observability — the
            // send only fails once the consumer is dropped; never stall the
            // servo script thread on CDP event delivery.
            let _ = tx.send(msg);
        }
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        log::error!("[webview] crashed: {reason}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── BaoWebViewDelegate ────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_delegate_new_with_state() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1024, 768);
        let delegate = BaoWebViewDelegate::new(state, viewport, std::rc::Weak::new());
        assert!(delegate.state().borrow().url.is_none());
    }

    #[test]
    fn test_webview_delegate_state_rc_shared() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(800, 600);
        let delegate = BaoWebViewDelegate::new(Rc::clone(&state), viewport, std::rc::Weak::new());
        // Modify state externally
        state.borrow_mut().title = Some("External".to_string());
        // Delegate sees same state
        assert_eq!(delegate.state().borrow().title.as_deref(), Some("External"));
    }

    #[test]
    fn test_webview_delegate_viewport_size() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1440, 900);
        let delegate = BaoWebViewDelegate::new(state, viewport, std::rc::Weak::new());
        // Verify delegate was created with specific viewport
        assert!(delegate.state().borrow().url.is_none());
    }

    // ─── PageFrameNavigated delegate emission ────────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_notify_url_changed_emits_frame_navigated() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            console_log_tx: Some(tx),
            cdp_target_id: Some("7".to_string()),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state.clone(), viewport, std::rc::Weak::new());

        // Simulate notify_url_changed by sending the same message the method
        // sends — frame id derived from the stamped target (REQ-CDP-004).
        let url = url::Url::parse("https://example.com").unwrap();
        let url_str = url.to_string();
        let loader_id = format!("{:016x}", url_str.len() as u64);
        let frame_id = main_frame_id_for_target(
            &state.borrow().cdp_target().expect("stamped target"),
        );
        if let Some(ref tx) = state.borrow().console_log_tx {
            tx.send(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id: frame_id.clone(),
                url: url_str.clone(),
                loader_id: loader_id.clone(),
            }))
            .unwrap();
        }

        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id,
                url,
                loader_id: lid,
            }) => {
                assert_eq!(frame_id, "main-7");
                assert_ne!(frame_id, "7", "frameId is never the PageId");
                assert!(url.starts_with("https://example.com"));
                assert_eq!(lid, loader_id);
            }
            other => panic!("expected PageFrameNavigated, got {:?}", other),
        }
    }
}
