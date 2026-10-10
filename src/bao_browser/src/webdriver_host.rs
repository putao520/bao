// WPT official-toolchain wiring (REQ-BRW-002): the bao binary as the
// WebDriver endpoint upstream wptrunner's `servo` product drives.
//
// Upstream shape (servoshell ports/servoshell/webdriver.rs +
// running_app_state.rs, 2026-09-27 baseline): the `webdriver_server`
// component owns the whole WebDriver HTTP protocol; the embedder only pumps
// `WebDriverCommandMsg` off a channel and answers the embedder-side commands
// (window/page lifecycle, load-status plumbing, dialogs, screenshots) against
// its webviews. This module is that embedder face, mapped onto bao's
// PagePool/PageHandle model — zero protocol code lives here.
//
// The delegate callbacks (load complete / traversal complete / input handled /
// embedder controls) fire on script threads with no back-reference to the
// pump, so the shared bookkeeping lives in a process-global registry (the
// same shape as bao's WEBVIEW_ID_BY_PAGE and servo's R53-A per-WebViewId
// registries). The mutex is held only across map moves and channel sends —
// never across a servo call.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use crossbeam_channel::{Receiver, Sender};
use servo::{
    EmbedderControl, EmbedderControlId, EventLoopWaker, GenericSender, InputEvent, InputEventId,
    NewWindowTypeHint, Preferences, SimpleDialog, TraversalId, WebDriverCommandMsg,
    WebDriverJSResult, WebDriverLoadStatus, WebDriverScriptCommand, WebDriverUserPrompt,
    WebDriverUserPromptAction, WebViewId,
};

/// Poll-loop no-op waker: bao's `run_browser` event loop re-polls
/// `servo.spin_event_loop()` on a bounded 1ms cadence, so a wake signal has
/// no scheduler to interrupt — the next poll drains the webdriver channel on
/// its own (≤1ms latency). Fills `start_server`'s required waker argument
/// without a platform event loop bao does not have.
#[derive(Clone, Copy, Default)]
pub struct SpinLoopWaker;

impl EventLoopWaker for SpinLoopWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(SpinLoopWaker)
    }

    fn wake(&self) {
        // No-op: the owning loop re-polls on its fixed cadence (see type doc).
    }
}

/// Cross-thread bookkeeping shared between the command pump (this module, on
/// the event-loop thread) and the servo delegate callbacks (script threads).
#[derive(Default)]
struct WebdriverBridge {
    /// Per-webview load-status channels handed over by `webdriver_server`
    /// (LoadUrl / Refresh / NewWindow / AddLoadStatusSender). Fulfilled by
    /// the delegate's `notify_load_status_changed(Complete)` hook.
    load_status_senders: HashMap<WebViewId, GenericSender<WebDriverLoadStatus>>,
    /// Loads whose `Complete` edge fired with no waiter registered — the
    /// edge is dropped otherwise (e26 livelock root, 2026-10-03): a
    /// `NewWindow` webview's initial about:blank load completes while the
    /// pump thread is still inside `create_webdriver_page` (the pipeline
    /// pumps through `wait_for_pipeline_ready`'s internal spin), so the
    /// Complete reaches [`notify_load_complete`] BEFORE the drain arm
    /// inserts the session's load-status sender. Without this latch the
    /// edge is lost, `webdriver_server`'s `wait_document_ready` then waits
    /// out the full page-load timeout (300s default) for a second Complete
    /// that never comes, and the serial dispatcher wedges every subsequent
    /// WebDriver command in the process (wptrunner: browser killed →
    /// CRASH; probability rises with concurrency because contention
    /// randomizes whether the load or the create wins the race).
    completed_loads: std::collections::HashSet<WebViewId>,
    /// go_back/go_forward traversal → completion channel, keyed by the
    /// `TraversalId` the delegate's `notify_traversal_complete` reports.
    pending_traversals: HashMap<TraversalId, GenericSender<WebDriverLoadStatus>>,
    /// Input events whose DOM handling the delegate must acknowledge
    /// (`notify_input_event_handled`).
    pending_input_events: HashMap<InputEventId, Sender<()>>,
    /// Input events whose handled edge fired with no waiter registered —
    /// replayed by the dispatch arm's insert (see
    /// [`notify_input_event_handled`]).
    handled_input_events: std::collections::HashSet<InputEventId>,
    /// The single in-flight WebDriver script evaluation's response channel —
    /// servoshell's interrupt face for "user prompt during script
    /// evaluation" (a dialog resolves the evaluation with null).
    script_interrupt: Option<GenericSender<WebDriverJSResult>>,
    /// Embedder controls (dialogs, pickers) servo asked the embedder to show
    /// while WebDriver owns the session. webdriver_server's prompt endpoints
    /// read this as "the current user prompt".
    embedder_controls: HashMap<WebViewId, Vec<EmbedderControl>>,
}

static WEBDRIVER_BRIDGE: LazyLock<Mutex<WebdriverBridge>> =
    LazyLock::new(|| Mutex::new(WebdriverBridge::default()));

/// Run `f` over the bridge; `None` = the mutex was poisoned (a delegate
/// callback panicked — the session is dead anyway, dropping the command is
/// the honest degradation).
fn with_bridge<R>(f: impl FnOnce(&mut WebdriverBridge) -> R) -> Option<R> {
    WEBDRIVER_BRIDGE.lock().ok().map(|mut bridge| f(&mut bridge))
}

// ─── Delegate hook faces (called from delegate.rs on script threads) ───────

/// `WebViewDelegate::notify_load_status_changed` — resolve the webview's
/// pending WebDriver load-status waiters on `Complete`.
///
/// A Complete with no registered waiter is LATCHED, not dropped (see
/// [`WebdriverBridge::completed_loads`] for the livelock this closes): the
/// waiter's registration site replays it.
pub fn notify_load_complete(webview_id: WebViewId) {
    with_bridge(|bridge| {
        match bridge.load_status_senders.remove(&webview_id) {
            Some(sender) => {
                let _ = sender.send(WebDriverLoadStatus::Complete);
                bridge.completed_loads.remove(&webview_id);
            },
            None => {
                bridge.completed_loads.insert(webview_id);
            },
        }
    });
}

/// Register a load-status waiter for `webview_id`, resolving immediately if
/// that webview's Complete already fired (latched) — and otherwise clearing
/// any stale latch so only the NEW navigation's edge can satisfy the wait
/// (LoadUrl inserts BEFORE `navigate()`, so its own Complete cannot have
/// fired yet; anything latched predates this registration).
fn register_load_status_sender(
    bridge: &mut WebdriverBridge,
    webview_id: WebViewId,
    sender: GenericSender<WebDriverLoadStatus>,
    replay_latched: bool,
) {
    if replay_latched && bridge.completed_loads.remove(&webview_id) {
        let _ = sender.send(WebDriverLoadStatus::Complete);
        return;
    }
    bridge.completed_loads.remove(&webview_id);
    bridge.load_status_senders.insert(webview_id, sender);
}

/// `WebViewDelegate::notify_traversal_complete` — resolve the pending
/// go_back/go_forward waiter.
pub fn notify_traversal_complete(traversal_id: TraversalId) {
    with_bridge(|bridge| {
        if let Some(sender) = bridge.pending_traversals.remove(&traversal_id) {
            let _ = sender.send(WebDriverLoadStatus::Complete);
        }
    });
}

/// `WebViewDelegate::notify_input_event_handled` — acknowledge the pending
/// WebDriver input event.
///
/// The handled edge can arrive before the dispatch arm registers the
/// event's sender: the ScriptThread finishes the key handling and reports
/// `InputEventsHandled` (routed to the delegate independently of the pump
/// thread) while the pump is still between `webdriver_dispatch_input_event`
/// and the `pending_input_events` insert. Without the latch the edge is
/// dropped and `wait_for_input_event_handled`'s unbounded `recv` wedges the
/// serial dispatcher forever — the input-event twin of the e26 load-status
/// livelock (same lost-edge race, same latch-replay fix; WPT editing red
/// face 2026-10-09: keydown-hides-editor tests TIMEOUT under load).
pub fn notify_input_event_handled(event_id: InputEventId) {
    with_bridge(|bridge| {
        if let Some(sender) = bridge.pending_input_events.remove(&event_id) {
            let _ = sender.send(());
        } else {
            // Edge before waiter — latch it for replay at insert time.
            // Events dispatched without a response sender (the non-blocking
            // touch path) may latch without ever being drained: one leaked
            // `usize` per such event, the same bounded-growth trade-off as
            // `completed_loads`.
            bridge.handled_input_events.insert(event_id);
        }
    });
}

/// `WebViewDelegate::show_embedder_control` under an active WebDriver
/// session: stash the control for the prompt endpoints, interrupt any
/// in-flight script evaluation (WebDriver spec: a prompt during script
/// evaluation resolves it with null) and report the load as Blocked (a
/// dialog blocks page load).
pub fn show_embedder_control(webview_id: WebViewId, control: EmbedderControl) {
    with_bridge(|bridge| {
        // Only a user prompt (a script-initiated dialog) interrupts an
        // in-flight script evaluation (servoshell: `show_embedder_control`'s
        // webdriver branch gates `interrupt_webdriver_script_evaluation` on
        // `SimpleDialog` — the WebDriver spec's "user prompt during script
        // evaluation resolves it with null"). Interrupting on other controls
        // breaks the wptrunner testdriver protocol: `InputMethod` fires every
        // time an editable element is focused, and its phantom null resolves
        // the pending async-script poll out-of-band — the page's next
        // testdriver message then lands in the dead evaluation's channel and
        // `performActions` never runs (keydown-hides-editor test family,
        // 2026-10-09 e154 forensics).
        if matches!(control, EmbedderControl::SimpleDialog(..)) {
            if let Some(response_sender) = &bridge.script_interrupt {
                let _ = response_sender.send(Ok(servo::JSValue::Null));
            }
        }

        let is_blocking = matches!(
            &control,
            EmbedderControl::SimpleDialog(..)
                | EmbedderControl::FilePicker { .. }
                | EmbedderControl::SelectElement { .. }
        );
        bridge
            .embedder_controls
            .entry(webview_id)
            .or_default()
            .push(control);

        if is_blocking {
            if let Some(sender) = bridge.load_status_senders.get(&webview_id) {
                let _ = sender.send(WebDriverLoadStatus::Blocked);
            }
        }
    });
}

/// `WebViewDelegate::hide_embedder_control` under an active WebDriver session.
pub fn hide_embedder_control(webview_id: WebViewId, control_id: EmbedderControlId) {
    with_bridge(|bridge| {
        if let Some(controls) = bridge.embedder_controls.get_mut(&webview_id) {
            controls.retain(|control| control.id() != control_id);
        }
        bridge
            .embedder_controls
            .retain(|_, controls| !controls.is_empty());
    });
}

/// Current prompt type for a webview, from the newest stashed control
/// (<https://w3c.github.io/webdriver/#dfn-handle-any-user-prompts> Step 3).
fn current_active_dialog_webdriver_type(
    controls: &[EmbedderControl],
) -> Option<WebDriverUserPrompt> {
    match controls.last()? {
        EmbedderControl::SimpleDialog(SimpleDialog::Alert(..)) => Some(WebDriverUserPrompt::Alert),
        EmbedderControl::SimpleDialog(SimpleDialog::Confirm(..)) => {
            Some(WebDriverUserPrompt::Confirm)
        }
        EmbedderControl::SimpleDialog(SimpleDialog::Prompt(..)) => {
            Some(WebDriverUserPrompt::Prompt)
        }
        EmbedderControl::FilePicker { .. } => Some(WebDriverUserPrompt::File),
        EmbedderControl::SelectElement { .. } => Some(WebDriverUserPrompt::Default),
        _ => None,
    }
}

/// Respond to the newest stashed `SimpleDialog` (accept/dismiss/ignore) and
/// return its message text — the HandleUserPrompt embedder contract.
fn respond_to_active_simple_dialog(
    controls: &mut Vec<EmbedderControl>,
    action: WebDriverUserPromptAction,
) -> Result<String, ()> {
    let Some(EmbedderControl::SimpleDialog(simple_dialog)) = controls.last() else {
        return Err(());
    };
    let result_text = simple_dialog.message().to_owned();
    if action == WebDriverUserPromptAction::Ignore {
        return Ok(result_text);
    }
    let Some(EmbedderControl::SimpleDialog(simple_dialog)) = controls.pop() else {
        return Err(());
    };
    match action {
        WebDriverUserPromptAction::Accept => simple_dialog.confirm(),
        WebDriverUserPromptAction::Dismiss => simple_dialog.dismiss(),
        WebDriverUserPromptAction::Ignore => unreachable!("returned early above"),
    }
    Ok(result_text)
}

/// Host state owned by the pump loop: the receiving end of the embedder
/// channel `webdriver_server::start_server` was handed.
pub struct WebDriverHost {
    receiver: Receiver<WebDriverCommandMsg>,
}

/// NewWindow arm: headless tab/window hints coincide, so both create a
/// fresh about:blank page, answer the sender, register the optional
/// load-status waiter (replay_latched = true: the new webview is
/// brand-new, so a latched Complete can only be its initial about:blank
/// load — exactly the edge this waiter exists to observe; dropping it
/// here is the e26 livelock, see `WebdriverBridge::completed_loads`),
/// and wait for the initial pipeline.
fn webdriver_handle_new_window(runtime: &crate::BrowserRuntime, msg: WebDriverCommandMsg) {
    let WebDriverCommandMsg::NewWindow(_type_hint, response_sender, load_status_sender) = msg
    else {
        return;
    };
    let new_page = url::Url::parse("about:blank")
        .ok()
        .and_then(|url| runtime.create_webdriver_page(url));
    match new_page {
        Some((webview_id, page)) => {
            let _ = response_sender.send(webview_id);
            if let Some(load_status_sender) = load_status_sender {
                with_bridge(|bridge| {
                    register_load_status_sender(bridge, webview_id, load_status_sender, true);
                });
            }
            page.wait_for_pipeline_ready(std::time::Duration::from_secs(15))
                .ok();
        }
        None => {
            log::error!("[webdriver] NewWindow: page creation failed");
            // Sender drop = the HTTP surface reports failure.
        }
    }
}

/// GoBack/GoForward arm tail: register the traversal's load-status sender
/// with the bridge (resolved by `notify_traversal_complete`).
fn webdriver_register_traversal(
    traversal_id: TraversalId,
    load_status_sender: GenericSender<WebDriverLoadStatus>,
) {
    with_bridge(|bridge| {
        bridge.pending_traversals.insert(traversal_id, load_status_sender);
    });
}

/// InputEvent arm: dispatch into the page and wire the response channel
/// to the handled/pending race — the handled edge can race ahead of this
/// insert, in which case it is replayed so the dispatcher's `recv`
/// resolves instead of waiting on a notification that already fired.
fn webdriver_handle_input_event(
    runtime: &crate::BrowserRuntime,
    webview_id: WebViewId,
    input_event: InputEvent,
    response_sender: Option<Sender<()>>,
) {
    match runtime.page_for_webview(webview_id) {
        Some((_, page)) => {
            let event_id = page.webdriver_dispatch_input_event(input_event);
            if let (Some(event_id), Some(response_sender)) = (event_id, response_sender) {
                with_bridge(|bridge| {
                    if bridge.handled_input_events.remove(&event_id) {
                        let _ = response_sender.send(());
                    } else {
                        bridge.pending_input_events.insert(event_id, response_sender);
                    }
                });
            }
        }
        None => {
            log::error!("[webdriver] InputEvent for unknown webview {webview_id:?}");
        }
    }
}

/// SendAlertText arm: update the active prompt dialog's current value.
fn webdriver_handle_send_alert_text(webview_id: WebViewId, text: String) {
    with_bridge(|bridge| {
        if let Some(controls) = bridge.embedder_controls.get_mut(&webview_id) {
            if let Some(EmbedderControl::SimpleDialog(SimpleDialog::Prompt(
                ref mut prompt_dialog,
            ))) = controls.last_mut()
            {
                prompt_dialog.set_current_value(&text);
            }
        }
    });
}

impl WebDriverHost {
    /// Start the upstream WebDriver HTTP server for this process and return
    /// the host that must be drained from the event loop.
    pub fn start(port: u16) -> Self {
        let preferences = servo::prefs::get().clone();
        let (embedder_sender, receiver) = crossbeam_channel::unbounded();
        webdriver_server::start_server(
            port,
            embedder_sender,
            Box::new(SpinLoopWaker),
            preferences,
        );
        Self { receiver }
    }

    /// Drain every queued command, answering embedder-side ones against the
    /// runtime's pages and forwarding engine-side ones into servo
    /// (`Servo::execute_webdriver_command` — the constellation/script-thread
    /// face servo owns). Port of servoshell's `handle_webdriver_messages`.
    pub fn drain(&self, runtime: &crate::BrowserRuntime) {
        while let Ok(msg) = self.receiver.try_recv() {
            match msg {
                WebDriverCommandMsg::ResetAllCookies(sender) => {
                    runtime.servo().site_data_manager().clear_cookies(None);
                    let _ = sender.send(());
                }
                WebDriverCommandMsg::Shutdown => {
                    runtime.schedule_exit();
                }
                WebDriverCommandMsg::IsWebViewOpen(webview_id, sender) => {
                    let _ = sender.send(runtime.page_for_webview(webview_id).is_some());
                }
                WebDriverCommandMsg::IsBrowsingContextOpen(..) => {
                    // Engine-side: the constellation owns browsing contexts.
                    runtime.servo().execute_webdriver_command(msg);
                }
                WebDriverCommandMsg::NewWindow(..) => {
                    // bao is headless: tab/window hints coincide.
                    webdriver_handle_new_window(runtime, msg);
                }
                WebDriverCommandMsg::CloseWebView(webview_id, response_sender) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        let _ = runtime.page_pool().close_page(page.id());
                    }
                    with_bridge(|bridge| {
                        bridge.load_status_senders.remove(&webview_id);
                        bridge.completed_loads.remove(&webview_id);
                    });
                    let _ = response_sender.send(());
                }
                WebDriverCommandMsg::FocusWebView(webview_id) => {
                    // Headless: focusing is bookkeeping only; bump the idle
                    // clock so idle reaping never closes a driven page.
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        page.webdriver_touch();
                    }
                }
                WebDriverCommandMsg::FocusBrowsingContext(..) => {
                    runtime.servo().execute_webdriver_command(msg);
                }
                WebDriverCommandMsg::GetAllWebViews(response_sender) => {
                    let _ = response_sender.send(runtime.webdriver_webview_ids());
                }
                WebDriverCommandMsg::GetWindowRect(webview_id, response_sender) => {
                    let rect = runtime
                        .page_for_webview(webview_id)
                        .map(|(_, page)| page.webdriver_window_rect());
                    let _ = response_sender.send(rect.unwrap_or_default());
                }
                WebDriverCommandMsg::MaximizeWebView(webview_id, response_sender) => {
                    let rect = runtime
                        .page_for_webview(webview_id)
                        .map(|(_, page)| {
                            page.webdriver_touch();
                            page.webdriver_window_rect()
                        })
                        .unwrap_or_default();
                    let _ = response_sender.send(rect);
                }
                WebDriverCommandMsg::SetWindowRect(
                    webview_id,
                    requested_rect,
                    response_sender,
                ) => {
                    let rect = runtime
                        .page_for_webview(webview_id)
                        .map(|(_, page)| page.webdriver_set_window_rect(requested_rect))
                        .unwrap_or_default();
                    let _ = response_sender.send(rect);
                }
                WebDriverCommandMsg::GetViewportSize(webview_id, response_sender) => {
                    let size = runtime
                        .page_for_webview(webview_id)
                        .map(|(_, page)| page.webdriver_viewport_size())
                        .unwrap_or_default();
                    let _ = response_sender.send(size);
                }
                // Only received at session start: the focused webview is the
                // initial page's webview.
                WebDriverCommandMsg::GetFocusedWebView(response_sender) => {
                    let _ = response_sender.send(runtime.webdriver_webview_ids().into_iter().next());
                }
                WebDriverCommandMsg::LoadUrl(webview_id, url, load_status_sender) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        // replay_latched = false: the sender is registered
                        // BEFORE `navigate()` fires, so its own Complete
                        // cannot have latched yet — a latch here predates this
                        // registration (boot load) and must not satisfy it.
                        with_bridge(|bridge| {
                            register_load_status_sender(
                                bridge,
                                webview_id,
                                load_status_sender,
                                false,
                            );
                        });
                        if let Err(error) = page.navigate(url.as_str()) {
                            log::error!("[webdriver] LoadUrl failed: {error}");
                        }
                    } else {
                        log::error!("[webdriver] LoadUrl for unknown webview {webview_id:?}");
                    }
                }
                WebDriverCommandMsg::Refresh(webview_id, load_status_sender) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        with_bridge(|bridge| {
                            register_load_status_sender(
                                bridge,
                                webview_id,
                                load_status_sender,
                                false,
                            );
                        });
                        if let Err(error) = page.reload() {
                            log::error!("[webdriver] Refresh failed: {error}");
                        }
                    }
                }
                WebDriverCommandMsg::GoBack(webview_id, load_status_sender) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        if let Some(traversal_id) = page.webdriver_go_back() {
                            webdriver_register_traversal(traversal_id, load_status_sender);
                        }
                    }
                }
                WebDriverCommandMsg::GoForward(webview_id, load_status_sender) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        if let Some(traversal_id) = page.webdriver_go_forward() {
                            webdriver_register_traversal(traversal_id, load_status_sender);
                        }
                    }
                }
                WebDriverCommandMsg::InputEvent(webview_id, input_event, response_sender) => {
                    webdriver_handle_input_event(runtime, webview_id, input_event, response_sender);
                }
                WebDriverCommandMsg::ScriptCommand(_, ref script_command) => {
                    // Embedder-side bookkeeping BEFORE handing the command to
                    // servo (mirrors servoshell's handle_webdriver_script_command):
                    // the in-flight evaluation's response channel is the
                    // interrupt face; Add/RemoveLoadStatusSender move the
                    // load-status plumbing.
                    match script_command {
                        WebDriverScriptCommand::ExecuteScriptWithCallback(
                            _,
                            response_sender,
                        ) => {
                            with_bridge(|bridge| {
                                bridge.script_interrupt = Some(response_sender.clone());
                            });
                        }
                        WebDriverScriptCommand::AddLoadStatusSender(
                            webview_id,
                            load_status_sender,
                        ) => {
                            // replay_latched = false: registered BEFORE the
                            // click/navigation it tracks (element-click face),
                            // so a latch here is stale, not this navigation.
                            with_bridge(|bridge| {
                                register_load_status_sender(
                                    bridge,
                                    *webview_id,
                                    load_status_sender.clone(),
                                    false,
                                );
                            });
                        }
                        WebDriverScriptCommand::RemoveLoadStatusSender(webview_id) => {
                            with_bridge(|bridge| {
                                bridge.load_status_senders.remove(webview_id);
                            });
                        }
                        _ => {
                            with_bridge(|bridge| bridge.script_interrupt = None);
                        }
                    }
                    runtime.servo().execute_webdriver_command(msg);
                }
                WebDriverCommandMsg::CurrentUserPrompt(webview_id, response_sender) => {
                    let current = with_bridge(|bridge| {
                        bridge.embedder_controls.get(&webview_id).and_then(|controls| {
                            current_active_dialog_webdriver_type(controls)
                        })
                    });
                    let _ = response_sender.send(current.unwrap_or(None));
                }
                WebDriverCommandMsg::HandleUserPrompt(webview_id, action, response_sender) => {
                    let result = with_bridge(|bridge| {
                        let Some(controls) = bridge.embedder_controls.get_mut(&webview_id) else {
                            return Err(());
                        };
                        respond_to_active_simple_dialog(controls, action)
                    });
                    let _ = response_sender.send(result.unwrap_or(Err(())));
                }
                WebDriverCommandMsg::GetAlertText(webview_id, response_sender) => {
                    let text = with_bridge(|bridge| {
                        bridge
                            .embedder_controls
                            .get(&webview_id)
                            .and_then(|controls| controls.last())
                            .and_then(|control| match control {
                                EmbedderControl::SimpleDialog(simple_dialog) => {
                                    Some(simple_dialog.message().to_owned())
                                }
                                _ => None,
                            })
                    });
                    let _ = response_sender.send(text.unwrap_or(None).ok_or(()));
                }
                WebDriverCommandMsg::SendAlertText(webview_id, text) => {
                    webdriver_handle_send_alert_text(webview_id, text);
                }
                WebDriverCommandMsg::TakeScreenshot(webview_id, rect, result_sender) => {
                    match runtime.page_for_webview(webview_id) {
                        Some((_, page)) => page.webdriver_take_screenshot(rect, result_sender),
                        None => {
                            log::error!(
                                "[webdriver] TakeScreenshot for unknown webview {webview_id:?}"
                            );
                            let _ = result_sender
                                .send(Err(servo::ScreenshotCaptureError::WebViewDoesNotExist));
                        }
                    }
                }
                // BAO PATCH (REQ-BRW-002 e106, main-session cross-boundary
                // authorization 2026-10-06): the vendor enum's
                // interaction-selection face (e99 snapshot swap a7272f16
                // replay follow-through — this exhaustive match was
                // non-exhaustive against it, masked until the variant
                // E0599s above were fixed). Headless: selection is
                // bookkeeping only — same shape as the FocusWebView arm
                // (bump the idle clock so idle reaping never closes a
                // driven page).
                WebDriverCommandMsg::SelectWebViewForInteraction(webview_id) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        page.webdriver_touch();
                    }
                }
                // Only received at session start: the webview selected for
                // interaction is the initial page's webview (same answer as
                // GetFocusedWebView above).
                WebDriverCommandMsg::GetWebViewSelectedForInteraction(response_sender) => {
                    let _ = response_sender
                        .send(runtime.webdriver_webview_ids().into_iter().next());
                }
            }
        }
    }
}

// Pref surface lives in `crate::prefs` (always compiled — the durable
// application point is core `BrowserRuntime::new`, outside the webdriver
// feature gate). Re-exported here to keep this module's public surface
// stable for the CLI entry (`bao_browser::webdriver_host::*`).
pub use crate::prefs::{apply_pref_overrides, apply_pref_overrides_to, load_prefs_file};
