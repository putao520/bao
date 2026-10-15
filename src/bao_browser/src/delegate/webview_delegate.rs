use super::*;

use bao_ipc::{Receiver, Sender};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint, DeviceIntRect,
    DeviceIntSize, EmbedderControl, EmbedderControlId, InputEventId, InputEventResult, LoadStatus,
    NavigationRequest, PermissionRequest, ScreenGeometry, ServoDelegate, ServoError, TraversalId,
    WebResourceLoad, WebView, WebViewDelegate,
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
            self.state
                .borrow()
                .log_unroutable_event("Page.frameNavigated");
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
        // REQ-CDS-009: the popup inherits its opener's browser context
        // alongside the fingerprint/permission config — an auxiliary of a
        // context-2 page must not silently land in the default context.
        let browser_context_id = opener.as_ref().map(|page| pool.page_context_id(page.id()));
        if pool
            .create_popup_page(
                request,
                self.viewport,
                stealth_profile,
                permission,
                browser_context_id,
            )
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

    // @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
    // Fetch 请求拦截面的 embedder 端(REQ-CDP-006,设计
    // /tmp/cdp-006-interception-design.md §2.1 候选 A / §3.2):servo 为
    // webview 关联的每个 HTTP(S) 请求发起的正式 embedder 往返 —— 不覆写时
    // 继承上游 no-op,drop(load) 即默认 DoNotIntercept,与今天的行为字节
    // 等价(零风险底座)。覆写后:槽 off / 不匹配 / 无事件路由 → 同一
    // drop=放行;命中 → 暂存 pending + 发射启用旗标选定的 pause 事件
    // (`Fetch.requestPaused` 或 legacy 面 `Network.requestIntercepted`),
    // fetch 任务停在 responder 上等待 CDP 决议(continue/fail/fulfill 经
    // `BaoWebViewState::fetch_resolve`)。
    //
    // 线程模型:本回调发生在泵线程(servo 以 `Rc<dyn WebViewDelegate>` 持有
    // 委托,结构性 !Send —— servo_delegate.rs:35-58),与 `fetch_resolve`
    // (BridgeCommand 泵线程消费)同线程 —— pending 注册表平字段即安全
    // (webview_state.rs `pending_intercepted_loads` 字段注释,设计 §9 风险 1)。
    //
    // 事件序(设计 §4):Network.requestWillBeSent 由 C19-② net tap 在
    // main_fetch step 1 发出(methods.rs:429),先于 intercept_request 往返
    // (methods.rs:563)—— 两者先后进入同一无界 bao_ipc 事件通道,FIFO 保序,
    // pause 事件恒在 requestWillBeSent 之后,无需重排。
    fn load_web_resource(&self, _webview: WebView, load: WebResourceLoad) {
        let matched = {
            let state = self.state.borrow();
            state.should_intercept(&load.request)
        };
        if !matched {
            // 槽 off / 不匹配:drop = AutomaticResponder DoNotIntercept
            // (= 不覆写时今天的行为,字节等价;零能力删除的默认臂)。
            drop(load);
            return;
        }
        // pause 许可门:无 CDP 身份或无事件通道的页,客户端永远收不到
        // pause 事件 —— pause 等于无解楔死;fail-open 到 continue。
        let (target_id, event_tx) = {
            let state = self.state.borrow();
            match pause_event_route(&state) {
                Some(route) => route,
                None => {
                    log::debug!(
                        "[fetch-intercept] pattern hit but no CDP event route (target/event_tx \
                         absent) — passing through instead of wedging the request"
                    );
                    drop(load);
                    return;
                }
            }
        };
        let info = {
            let mut state = self.state.borrow_mut();
            state.register_paused_load(load)
        };
        log::debug!(
            "[fetch-intercept] paused {} {} ({} as {})",
            info.method,
            info.url,
            info.interception_id,
            info.resource_type
        );
        let event = paused_servo_event(target_id, info);
        send_servo_event(&event_tx, event);
    }
}

/// pause 许可门:页有 CDP 身份且有事件通道时返回 `(target_id, event_tx)`
/// 路由,否则 None(fail-open 到 continue,防无解楔死)。
fn pause_event_route(state: &BaoWebViewState) -> Option<(String, Sender<ServoEvent>)> {
    let target_id = state.cdp_target()?;
    let event_tx = state.event_tx.clone()?;
    Some((target_id, event_tx))
}

/// 组装 pause 事件(独立纯函数使字段映射可单测 —— 真 `WebResourceLoad` 值
/// 无法在无引擎测试中构造)。事件面由启用旗标选定:legacy_network_events
/// → `Network.requestIntercepted`(criterion 字面命名面),否则
/// `Fetch.requestPaused`(E2 BridgeCommand 落库形态;对设计 §5「双事件
/// 同点发射」的裁定改形 —— 每 hit 只发所属启用面的事件,未启用的域不收
/// 伪事件)。
fn paused_servo_event(target_id: String, info: FetchPausedRequestInfo) -> ServoEvent {
    let FetchPausedRequestInfo {
        interception_id,
        request_id,
        url,
        method,
        headers,
        resource_type,
        is_redirect,
        legacy_network_events,
    } = info;
    let resource_type = resource_type.to_string();
    if legacy_network_events {
        ServoEvent::NetworkRequestIntercepted {
            target_id,
            request_id,
            interception_id,
            url,
            method,
            headers,
            resource_type,
            is_redirect,
        }
    } else {
        ServoEvent::FetchRequestPaused {
            target_id,
            request_id,
            interception_id,
            url,
            method,
            headers,
            resource_type,
            is_redirect,
        }
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
        let (tx, rx) = bao_ipc::unbounded::<ConsoleMessage>();
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
        let frame_id =
            main_frame_id_for_target(&state.borrow().cdp_target().expect("stamped target"));
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

    // ─── Fetch 请求拦截面(REQ-CDP-006,设计 §3.2/§5)────────────────
    // @trace REQ-CDP-006 [req:REQ-CDP-006] [entity:FetchInterception]
    // [criterion:6] [level:unit]

    #[test]
    fn test_pause_event_route_gate() {
        // 无 CDP 身份 / 无事件通道的页不 pause(fail-open 到 continue,
        // 防无解楔死);双齐才放行。
        let state = BaoWebViewState::default();
        assert!(pause_event_route(&state).is_none());

        let state = BaoWebViewState {
            cdp_target_id: Some("7".to_string()),
            ..Default::default()
        };
        assert!(pause_event_route(&state).is_none());

        let (tx, _rx) = bao_ipc::unbounded::<ServoEvent>();
        let state = BaoWebViewState {
            cdp_target_id: Some("7".to_string()),
            event_tx: Some(tx),
            ..Default::default()
        };
        let (target_id, _event_tx) = pause_event_route(&state).expect("admissible");
        assert_eq!(target_id, "7");
    }

    #[test]
    fn test_paused_servo_event_carries_full_payload() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/html".to_string());
        let info = FetchPausedRequestInfo {
            interception_id: "interception-9".to_string(),
            request_id: "cdpreq-9".to_string(),
            url: "https://example.com/a?b=1".to_string(),
            method: "GET".to_string(),
            headers,
            resource_type: "Document",
            is_redirect: false,
            legacy_network_events: false,
        };
        match paused_servo_event("7".to_string(), info) {
            ServoEvent::FetchRequestPaused {
                target_id,
                request_id,
                interception_id,
                url,
                method,
                headers,
                resource_type,
                is_redirect,
            } => {
                assert_eq!(target_id, "7");
                assert_eq!(request_id, "cdpreq-9");
                assert_eq!(interception_id, "interception-9");
                assert_eq!(url, "https://example.com/a?b=1");
                assert_eq!(method, "GET");
                assert_eq!(
                    headers.get("content-type").map(String::as_str),
                    Some("text/html")
                );
                assert_eq!(resource_type, "Document");
                assert!(!is_redirect);
            }
            other => panic!("expected FetchRequestPaused, got {other:?}"),
        }
    }

    #[test]
    fn test_paused_servo_event_legacy_flag_selects_network_face() {
        // legacy_network_events=true → 命中发 Network.requestIntercepted
        // (criterion 字面命名面),不发 Fetch.requestPaused。
        let info = FetchPausedRequestInfo {
            interception_id: "interception-1".to_string(),
            request_id: "cdpreq-1".to_string(),
            url: "https://example.com/".to_string(),
            method: "GET".to_string(),
            headers: HashMap::new(),
            resource_type: "Document",
            is_redirect: false,
            legacy_network_events: true,
        };
        match paused_servo_event("7".to_string(), info) {
            ServoEvent::NetworkRequestIntercepted {
                target_id,
                request_id,
                interception_id,
                resource_type,
                ..
            } => {
                assert_eq!(target_id, "7");
                assert_eq!(request_id, "cdpreq-1");
                assert_eq!(interception_id, "interception-1");
                assert_eq!(resource_type, "Document");
            }
            other => panic!("expected NetworkRequestIntercepted, got {other:?}"),
        }
    }
}
