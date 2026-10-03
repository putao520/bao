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
    EmbedderControl, EmbedderControlId, EventLoopWaker, GenericSender, InputEventId,
    NewWindowTypeHint, Preferences, SimpleDialog, TraversalId, WebDriverCommandMsg,
    WebDriverJSResult, WebDriverLoadStatus, WebDriverScriptCommand, WebDriverUserPrompt,
    WebDriverUserPromptAction, WebViewId,
};

/// Spin-loop no-op waker: bao's `run_browser` event loop spins
/// `servo.spin_event_loop()` continuously (yield-based), so a wake signal has
/// no scheduler to interrupt — the next spin drains the webdriver channel on
/// its own. Fills `start_server`'s required waker argument without a platform
/// event loop bao does not have.
#[derive(Clone, Copy, Default)]
pub struct SpinLoopWaker;

impl EventLoopWaker for SpinLoopWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(SpinLoopWaker)
    }

    fn wake(&self) {
        // No-op: the owning loop spins unconditionally (see type doc).
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
    /// go_back/go_forward traversal → completion channel, keyed by the
    /// `TraversalId` the delegate's `notify_traversal_complete` reports.
    pending_traversals: HashMap<TraversalId, GenericSender<WebDriverLoadStatus>>,
    /// Input events whose DOM handling the delegate must acknowledge
    /// (`notify_input_event_handled`).
    pending_input_events: HashMap<InputEventId, Sender<()>>,
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
pub fn notify_load_complete(webview_id: WebViewId) {
    with_bridge(|bridge| {
        if let Some(sender) = bridge.load_status_senders.remove(&webview_id) {
            let _ = sender.send(WebDriverLoadStatus::Complete);
        }
    });
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
pub fn notify_input_event_handled(event_id: InputEventId) {
    with_bridge(|bridge| {
        if let Some(sender) = bridge.pending_input_events.remove(&event_id) {
            let _ = sender.send(());
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
        if let Some(response_sender) = &bridge.script_interrupt {
            let _ = response_sender.send(Ok(servo::JSValue::Null));
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
                WebDriverCommandMsg::NewWindow(
                    type_hint,
                    response_sender,
                    load_status_sender,
                ) => {
                    let _ = type_hint; // bao is headless: tab/window hints coincide
                    let new_page = url::Url::parse("about:blank")
                        .ok()
                        .and_then(|url| runtime.create_webdriver_page(url));
                    match new_page {
                        Some((webview_id, page)) => {
                            let _ = response_sender.send(webview_id);
                            if let Some(load_status_sender) = load_status_sender {
                                with_bridge(|bridge| {
                                    bridge
                                        .load_status_senders
                                        .insert(webview_id, load_status_sender);
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
                WebDriverCommandMsg::CloseWebView(webview_id, response_sender) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        let _ = runtime.page_pool().close_page(page.id());
                    }
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
                        with_bridge(|bridge| {
                            bridge
                                .load_status_senders
                                .insert(webview_id, load_status_sender);
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
                            bridge
                                .load_status_senders
                                .insert(webview_id, load_status_sender);
                        });
                        if let Err(error) = page.reload() {
                            log::error!("[webdriver] Refresh failed: {error}");
                        }
                    }
                }
                WebDriverCommandMsg::GoBack(webview_id, load_status_sender) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        if let Some(traversal_id) = page.webdriver_go_back() {
                            with_bridge(|bridge| {
                                bridge
                                    .pending_traversals
                                    .insert(traversal_id, load_status_sender);
                            });
                        }
                    }
                }
                WebDriverCommandMsg::GoForward(webview_id, load_status_sender) => {
                    if let Some((_, page)) = runtime.page_for_webview(webview_id) {
                        if let Some(traversal_id) = page.webdriver_go_forward() {
                            with_bridge(|bridge| {
                                bridge
                                    .pending_traversals
                                    .insert(traversal_id, load_status_sender);
                            });
                        }
                    }
                }
                WebDriverCommandMsg::InputEvent(webview_id, input_event, response_sender) => {
                    match runtime.page_for_webview(webview_id) {
                        Some((_, page)) => {
                            let event_id = page.webdriver_dispatch_input_event(input_event);
                            if let (Some(event_id), Some(response_sender)) =
                                (event_id, response_sender)
                            {
                                with_bridge(|bridge| {
                                    bridge
                                        .pending_input_events
                                        .insert(event_id, response_sender);
                                });
                            }
                        }
                        None => {
                            log::error!(
                                "[webdriver] InputEvent for unknown webview {webview_id:?}"
                            );
                        }
                    }
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
                            with_bridge(|bridge| {
                                bridge
                                    .load_status_senders
                                    .insert(*webview_id, load_status_sender.clone());
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
            }
        }
    }
}

/// Apply `--pref=K=V` / `--prefs-file` overrides onto servo's process-global
/// preference store. Must run BEFORE `BrowserRuntime::new` (the store is read
/// at Servo construction). Mirrors executorservo.py's `parse_pref_value`
/// coercion: `true`/`false` → bool, numeric → f64, everything else stays a
/// string. Fail-closed: a bad override aborts the launch instead of silently
/// running with missing test prefs.
pub fn apply_pref_overrides(overrides: &[(String, String)]) -> Result<(), String> {
    if overrides.is_empty() {
        return Ok(());
    }
    let mut value = serde_json::to_value(servo::prefs::get().clone())
        .map_err(|e| format!("serializing current prefs failed: {e}"))?;
    let serde_json::Value::Object(ref mut map) = value else {
        return Err("prefs did not serialize to a JSON object".into());
    };
    for (key, raw) in overrides {
        let coerced = match raw.as_str() {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            other => other
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .unwrap_or_else(|| serde_json::Value::String(other.to_string())),
        };
        map.insert(key.clone(), coerced);
    }
    let merged: Preferences = serde_json::from_value(value)
        .map_err(|e| format!("pref overrides do not match the Preferences schema: {e}"))?;
    servo::prefs::set(merged);
    Ok(())
}

/// Load a wpt `--prefs-file` (JSON object of pref → raw value) into the same
/// override list `apply_pref_overrides` consumes. Fail-closed on unreadable
/// or non-object files.
pub fn load_prefs_file(path: &str) -> Result<Vec<(String, String)>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("reading prefs file {path} failed: {e}"))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parsing prefs file {path} failed: {e}"))?;
    let serde_json::Value::Object(map) = parsed else {
        return Err(format!("prefs file {path} is not a JSON object"));
    };
    Ok(map
        .into_iter()
        .map(|(key, value)| {
            let raw = match value {
                serde_json::Value::Bool(true) => "true".to_string(),
                serde_json::Value::Bool(false) => "false".to_string(),
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::String(string) => string,
                other => other.to_string(),
            };
            (key, raw)
        })
        .collect())
}
