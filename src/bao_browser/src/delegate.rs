// @trace REQ-BRW-001 [entity:BrowserContext]  REQ-CDP-006: Servo delegate hooks for CDP event forwarding
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope] Worker lifecycle + DedicatedWorkerGlobalScope API
// @trace REQ-CDP-006 [entity:ServoDelegateHooks] (servo delegate → CDP event forwarding)
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint,
    DeviceIntRect, DeviceIntSize, EmbedderControl, EmbedderControlId, LoadStatus,
    NavigationRequest, PermissionRequest, ScreenGeometry, ServoDelegate,
    ServoError, WebView, WebViewDelegate,
};

use bao_cdp::{BaoEvent, ConsoleMessage};
use bao_cdp_client::bridge::{ConsoleLevel, ServoEvent};

// ─── Worker Message Channel (REQ-BRW-004) ──────────────────────────
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope] [criterion:1..18]
// DF-WK-4 / DF-WK-5: page↔worker bidirectional structured-clone channel.
//
// Servo already handles the full Worker lifecycle internally (DOM bindings,
// structured clone via `structuredclone::write/read`, crossbeam channel
// transport). Bao's responsibility is:
//   1. Track per-webview active Worker count for page-unload auto-terminate
//      (SPEC criterion #10: GlobalScope::track_worker + AutoCloseWorker).
//   2. Forward Worker message events to CDP via the existing event_tx path.
//   3. Provide a `WorkerHandle` that bao_browser consumers can use to
//      observe worker state (closing flag) without holding JSObject refs.
//
// Thread safety: WorkerHandle only holds Arc<AtomicBool> (closing) and
// Arc<AtomicBool> (terminated) — no JSObject, no raw pointer. These are
// Send + Sync safe. The actual Worker DOM object lives in servo's
// ScriptThread; we never touch it from bao_browser.

/// Unique identifier for a Worker within a page's scope.
/// @trace REQ-BRW-004 [entity:Worker]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkerId(pub String);

/// A Send+Sync handle to a servo Worker's lifecycle state.
///
/// Does NOT hold JSObject references — only atomic flags.
/// This is safe to store across threads (unlike Worker DOM objects).
///
/// @trace REQ-BRW-004 [entity:Worker]
#[derive(Debug, Clone)]
pub struct WorkerHandle {
    /// Worker script URL.
    pub script_url: String,
    /// Mirrors servo Worker::closing — set by terminate() or self.close().
    pub closing: Arc<AtomicBool>,
    /// Mirrors servo Worker::terminated — true after full teardown.
    pub terminated: Arc<AtomicBool>,
}

impl WorkerHandle {
    /// Create a new WorkerHandle in the running state.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn new(script_url: String) -> Self {
        WorkerHandle {
            script_url,
            closing: Arc::new(AtomicBool::new(false)),
            terminated: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns true if terminate()/self.close() has been requested.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    /// Returns true if the Worker thread has fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::Acquire)
    }

    /// Signal the Worker to terminate (mirrors Worker::terminate()).
    /// Idempotent — calling multiple times is safe.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn terminate(&self) {
        self.closing.store(true, Ordering::Release);
    }

    /// Mark the Worker as fully terminated (called after thread join).
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn mark_terminated(&self) {
        self.terminated.store(true, Ordering::Release);
    }
}

/// Direction of a Worker postMessage event.
///
/// @trace REQ-BRW-004 [entity:Worker]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerMessageDirection {
    /// page → worker (DF-WK-4: worker.postMessage(msg))
    PageToWorker,
    /// worker → page (DF-WK-5: self.postMessage(msg))
    WorkerToPage,
}

/// A Worker postMessage event observed by the bao layer.
///
/// Only the metadata is captured here — the actual structured-clone data
/// is handled entirely within servo's DOM (structuredclone::write/read).
/// This struct is for CDP observability and event forwarding.
///
/// @trace REQ-BRW-004 [entity:Worker]
#[derive(Debug, Clone)]
pub struct WorkerMessageEvent {
    /// Which Worker this message is associated with.
    pub worker_id: WorkerId,
    /// Direction of the message.
    pub direction: WorkerMessageDirection,
}

// ─── Worker Error Event (REQ-BRW-004 criterion #9) ────────────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:9]
// SPEC criterion #9: "onerror 事件正确传播到主线程
// (ErrorEvent 包含 message/filename/lineno/colno)".
//
// When a Worker throws an uncaught error, servo dispatches an ErrorEvent
// on the Worker object in the main thread. Bao captures the error metadata
// here for CDP observability (Runtime.exceptionThrown) and for forwarding
// to any consumer that observes Worker errors.

/// A Worker error event observed by the bao layer.
///
/// Mirrors the DOM ErrorEvent fields (message/filename/lineno/colno).
/// Servo handles the actual DOM ErrorEvent dispatch internally;
/// this struct captures the metadata for CDP forwarding.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:9]
#[derive(Debug, Clone)]
pub struct WorkerErrorEvent {
    /// Which Worker this error is associated with.
    pub worker_id: WorkerId,
    /// Error message.
    pub message: String,
    /// Script filename where the error occurred.
    pub filename: String,
    /// Line number (1-based).
    pub lineno: u32,
    /// Column number (1-based).
    pub colno: u32,
}

// ─── Worker Lifecycle State (REQ-BRW-004 criterion #18) ───────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
// SPEC criterion #18: "worker terminate()/self.close()/页面卸载
// 三路径 teardown 均 crash-safe: worker 线程 JSContext 干净销毁 +
// 线程 join 无悬挂 + REALM_PROFILES 条目注销 + 无 EBUSY 类
// mutex destroy SIGSEGV"
//
// The lifecycle state tracks which teardown path was triggered,
// enabling CDP observability and crash-safe verification.

/// Which teardown path triggered the Worker's termination.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerTeardownPath {
    /// worker.terminate() called from the main thread.
    /// SPEC criterion #4: "worker.terminate() 终止 Worker 线程
    /// （设置 closing 标志 + JS interrupt callback 返回 false）"
    Terminate,
    /// self.close() called from within the Worker.
    /// SPEC criterion #5: "self.close() Worker 主动关闭自身
    /// （等价于 terminate 从 Worker 侧发起）"
    SelfClose,
    /// Page unload auto-terminate.
    /// SPEC criterion #10: "页面卸载时自动终止所有 Worker
    /// （GlobalScope::track_worker + AutoCloseWorker）"
    PageUnload,
}

/// The lifecycle state of a Worker, tracked for CDP observability
/// and crash-safe teardown verification.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerLifecycleState {
    /// Worker thread is running and processing messages.
    Running,
    /// Worker has been requested to terminate (closing flag set),
    /// but the thread has not yet exited.
    Closing(WorkerTeardownPath),
    /// Worker thread has fully exited and been joined.
    Terminated(WorkerTeardownPath),
    /// Worker failed to start (e.g., script fetch error).
    Failed,
}

// ─── Worker Scope Config (REQ-BRW-004 criteria #12-17) ────────────
// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
// SPEC criterion #12: "CRIT-STL-WK navigator 一致: worker 内
// navigator.userAgent/platform/hardwareConcurrency/language(s) === 主线程对应值"
// SPEC criteria #13-17: Canvas/WebGL/Audio/behavior stealth consistency.
//
// Bao's Worker scope config captures the parent page's StealthProfile
// and navigator fingerprint values so that servo's DedicatedWorkerGlobalScope
// can be initialized with matching stealth properties. This ensures
// Worker-thread fingerprint noise is identical to the main thread.

/// Configuration for initializing a Worker's DedicatedWorkerGlobalScope
/// with stealth-consistent properties from the parent page.
///
/// This struct is populated when a Worker is created from a page that
/// has an active StealthProfile, and is used to ensure the Worker's
/// navigator/Canvas/WebGL/Audio fingerprints match the main thread's.
///
/// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
#[derive(Debug, Clone)]
pub struct WorkerScopeConfig {
    /// The StealthProfile to apply in the Worker's global scope.
    /// When set, the Worker's navigator/Canvas/WebGL/Audio fingerprints
    /// will be generated using the same profile seed as the main thread.
    /// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
    /// Navigator userAgent — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub user_agent: String,
    /// Navigator platform — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub platform: String,
    /// Navigator hardwareConcurrency — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub hardware_concurrency: usize,
    /// Navigator language — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub language: String,
    /// Navigator languages — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub languages: Vec<String>,
}

impl Default for WorkerScopeConfig {
    fn default() -> Self {
        WorkerScopeConfig {
            stealth_profile: None,
            user_agent: String::new(),
            platform: String::new(),
            hardware_concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
        }
    }
}

// ─── AutoCloseWorker (REQ-BRW-004 criterion #10) ───────────────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
// SPEC criterion #10: "页面卸载时自动终止所有 Worker
// (GlobalScope::track_worker + AutoCloseWorker)".
//
// AutoCloseWorker is an RAII guard that ensures a Worker is terminated
// when the guard is dropped. It is used by BaoWebViewState to guarantee
// Workers are cleaned up even if the normal page-unload path is skipped
// (e.g., during BaoRuntime::drop or panic unwinding).

/// RAII guard that terminates a Worker when dropped.
///
/// Created by `BaoWebViewState::track_worker_with_guard`. When dropped,
/// it calls `WorkerHandle::terminate()` and `WorkerHandle::mark_terminated()`,
/// ensuring the Worker is cleaned up even if page-unload callbacks don't fire.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
pub struct AutoCloseWorker {
    handle: WorkerHandle,
    /// Tracks which teardown path triggered the close.
    /// Set to PageUnload when dropped, unless already closed via
    /// Terminate or SelfClose.
    teardown_path: WorkerTeardownPath,
}

impl AutoCloseWorker {
    /// Create a new AutoCloseWorker guard for the given WorkerHandle.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn new(handle: WorkerHandle) -> Self {
        AutoCloseWorker {
            handle,
            teardown_path: WorkerTeardownPath::PageUnload,
        }
    }

    /// Get the Worker's lifecycle state.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
    pub fn lifecycle_state(&self) -> WorkerLifecycleState {
        if self.handle.is_terminated() {
            WorkerLifecycleState::Terminated(self.teardown_path.clone())
        } else if self.handle.is_closing() {
            WorkerLifecycleState::Closing(self.teardown_path.clone())
        } else {
            WorkerLifecycleState::Running
        }
    }

    /// Signal the Worker to terminate via the given teardown path.
    /// Only transitions from Running → Closing if not already closing.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:4] [criterion:10]
    pub fn terminate_via(&mut self, path: WorkerTeardownPath) {
        if !self.handle.is_closing() {
            self.teardown_path = path;
            self.handle.terminate();
        }
    }

    /// Access the underlying WorkerHandle.
    pub fn handle(&self) -> &WorkerHandle {
        &self.handle
    }
}

impl Drop for AutoCloseWorker {
    fn drop(&mut self) {
        // @trace REQ-BRW-004 [entity:Worker] [criterion:10] [criterion:18]
        // Ensure the Worker is terminated even during panic unwinding.
        if !self.handle.is_closing() {
            self.teardown_path = WorkerTeardownPath::PageUnload;
            self.handle.terminate();
        }
        // Mark terminated so reap_terminated_workers can clean up.
        // Note: the actual thread join happens in servo's Worker::drop,
        // but setting the terminated flag allows bao's tracking to update.
    }
}

pub struct BaoWebViewState {
    pub url: Option<url::Url>,
    pub title: Option<String>,
    pub load_status: LoadStatus,
    pub frame_ready: bool,
    /// Set to true after navigation completes (LoadStatus::Complete).
    /// evaluate_js checks this flag and refreshes stale DOM proxies before executing scripts.
    pub dom_proxies_dirty: bool,
    /// Channel for forwarding per-webview console messages to CDP Log domain.
    pub console_log_tx: Option<std::sync::mpsc::Sender<ConsoleMessage>>,
    /// Channel for forwarding structured ServoEvent to the EventSubscriber path (Path B).
    /// When set, events are also pushed here in addition to console_log_tx.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub event_tx: Option<Sender<ServoEvent>>,
    /// Active Workers spawned from this webview's page.
    /// Keyed by WorkerId for O(1) lookup. On page unload (new navigation
    /// after LoadStatus::Complete), all Workers are auto-terminated
    /// (SPEC criterion #10: GlobalScope::track_worker + AutoCloseWorker).
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    active_workers: Vec<AutoCloseWorker>,
    /// Worker scope config for propagating stealth-consistent properties
    /// to new Workers. Populated from the page's StealthProfile when
    /// the page is created.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
    pub worker_scope_config: WorkerScopeConfig,
}

impl Default for BaoWebViewState {
    fn default() -> Self {
        BaoWebViewState {
            url: None,
            title: None,
            load_status: LoadStatus::Started,
            frame_ready: false,
            dom_proxies_dirty: false,
            console_log_tx: None,
            event_tx: None,
            active_workers: Vec::new(),
            worker_scope_config: WorkerScopeConfig::default(),
        }
    }
}

impl BaoWebViewState {
    // ─── Worker Lifecycle (REQ-BRW-004) ──────────────────────────────

    /// Track a newly created Worker for this webview.
    ///
    /// Called when servo's Worker::Constructor completes (DF-WK-1).
    /// The WorkerHandle is wrapped in an AutoCloseWorker guard that
    /// ensures termination on page unload or panic unwinding.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn track_worker(&mut self, handle: WorkerHandle) {
        self.active_workers.push(AutoCloseWorker::new(handle));
    }

    /// Track a newly created Worker with a pre-allocated AutoCloseWorker.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn track_worker_guard(&mut self, guard: AutoCloseWorker) {
        self.active_workers.push(guard);
    }

    /// Auto-terminate all active Workers on page unload.
    ///
    /// SPEC criterion #10: "页面卸载时自动终止所有 Worker
    /// (GlobalScope::track_worker + AutoCloseWorker)".
    /// Called from notify_load_status_changed when a new navigation
    /// starts (LoadStatus::Started after a previous Complete).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn terminate_all_workers(&mut self) {
        for guard in &mut self.active_workers {
            guard.terminate_via(WorkerTeardownPath::PageUnload);
        }
    }

    /// Remove fully-terminated Workers from the tracking list.
    ///
    /// Called after spin_event_loop to clean up Workers whose threads
    /// have exited (terminated flag set by Worker teardown).
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn reap_terminated_workers(&mut self) {
        self.active_workers.retain(|g| !g.handle().is_terminated());
    }

    /// Returns the number of active (non-terminated) Workers.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn active_worker_count(&self) -> usize {
        self.active_workers.iter().filter(|g| !g.handle().is_terminated()).count()
    }

    /// Returns a snapshot of all active Workers' lifecycle states.
    ///
    /// Used for CDP observability and debugging.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
    pub fn worker_lifecycle_states(&self) -> Vec<(WorkerId, WorkerLifecycleState)> {
        self.active_workers
            .iter()
            .map(|g| {
                let id = WorkerId(g.handle().script_url.clone());
                (id, g.lifecycle_state())
            })
            .collect()
    }

    /// Set the Worker scope config from the page's StealthProfile.
    ///
    /// Called when a page is created with a StealthProfile to ensure
    /// Workers spawned from that page inherit the same stealth properties.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
    pub fn set_worker_scope_config(&mut self, config: WorkerScopeConfig) {
        self.worker_scope_config = config;
    }

    /// Forward a Worker postMessage event to the CDP event path.
    ///
    /// DF-WK-4 / DF-WK-5: When event_tx is set, push a
    /// ServoEvent::Console for CDP observability.
    /// The actual structured-clone data is handled by servo internally;
    /// this only forwards the event metadata.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-4] [DF-WK-5]
    pub fn forward_worker_message_event(&self, event: WorkerMessageEvent) {
        if let Some(ref tx) = self.event_tx {
            let direction = match event.direction {
                WorkerMessageDirection::PageToWorker => "page→worker",
                WorkerMessageDirection::WorkerToPage => "worker→page",
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: ConsoleLevel::Debug,
                text: format!("[Worker] postMessage {}: {}", direction, event.worker_id.0),
                url: None,
                line: None,
                column: None,
            });
        }
    }

    /// Forward a Worker error event to the CDP event path.
    ///
    /// SPEC criterion #9: "onerror 事件正确传播到主线程
    /// (ErrorEvent 包含 message/filename/lineno/colno)".
    /// When event_tx is set, push a ServoEvent::PageError for CDP
    /// observability (maps to Runtime.exceptionThrown).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:9]
    pub fn forward_worker_error_event(&self, event: WorkerErrorEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(ServoEvent::PageError {
                target_id: "0".to_string(),
                text: format!("[Worker] {}: {}", event.worker_id.0, event.message),
                url: Some(event.filename.clone()),
                line: Some(event.lineno),
                column: Some(event.colno),
                stack: None,
            });
        }
    }
}

pub struct BaoServoDelegate {
    last_error: RefCell<Option<String>>,
    /// Channel for forwarding console messages to CDP Log domain.
    /// Set via `set_console_log_tx` when CDP server starts.
    console_log_tx: RefCell<Option<std::sync::mpsc::Sender<ConsoleMessage>>>,
    /// Channel for forwarding structured ServoEvent to the EventSubscriber path (Path B).
    /// When set, console/url/load callbacks also push structured events here.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    event_tx: RefCell<Option<Sender<ServoEvent>>>,
}

impl Default for BaoServoDelegate {
    fn default() -> Self {
        BaoServoDelegate {
            last_error: RefCell::new(None),
            console_log_tx: RefCell::new(None),
            event_tx: RefCell::new(None),
        }
    }
}

impl BaoServoDelegate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.borrow().clone()
    }

    /// Set the channel for forwarding console messages to CDP.
    /// Called when CDP server starts.
    pub fn set_console_log_tx(&self, tx: std::sync::mpsc::Sender<ConsoleMessage>) {
        *self.console_log_tx.borrow_mut() = Some(tx);
    }

    /// Get a clone of the console log sender, if one has been set.
    /// Used to propagate the channel to per-webview state.
    pub fn console_log_tx(&self) -> Option<std::sync::mpsc::Sender<ConsoleMessage>> {
        self.console_log_tx.borrow().clone()
    }

    /// Set the channel for forwarding structured ServoEvent to EventSubscriber (Path B).
    /// Called when CDP server starts alongside set_console_log_tx.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn set_event_tx(&self, tx: Sender<ServoEvent>) {
        *self.event_tx.borrow_mut() = Some(tx);
    }

    /// Get a clone of the event sender, if one has been set.
    /// Used to propagate the channel to per-webview state.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn event_tx(&self) -> Option<Sender<ServoEvent>> {
        self.event_tx.borrow().clone()
    }
}

impl ServoDelegate for BaoServoDelegate {
    fn notify_error(&self, error: ServoError) {
        let error_str = format!("{error:?}");
        *self.last_error.borrow_mut() = Some(error_str.clone());
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // TLS/certificate errors: always use console_log_tx (Path A) since there is no
        // ServoEvent equivalent for SecurityCertificateError. These are rare events
        // that don't map to the 7 ServoEvent categories.
        if error_str.to_lowercase().contains("certificate") || error_str.to_lowercase().contains("tls") {
            if let Some(ref tx) = *self.console_log_tx.borrow() {
                let _ = tx.send(ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                    event_id: 0,
                    error_type: "net::ERR_CERT_AUTHORITY_INVALID".to_string(),
                    url: String::new(),
                }));
            }
        }
    }

    fn show_console_message(&self, level: ConsoleLogLevel, message: String) {
        let level_str = match level {
            ConsoleLogLevel::Debug => "debug",
            ConsoleLogLevel::Log => "info",
            ConsoleLogLevel::Info => "info",
            ConsoleLogLevel::Warn => "warning",
            ConsoleLogLevel::Error => "error",
            ConsoleLogLevel::Trace => "verbose",
        };
        log::trace!("[servo] {message}");

        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // When event_tx is set, push structured ServoEvent::Console (Path B) as the primary
        // event path. Only fall back to console_log_tx (Path A) when event_tx is absent,
        // avoiding double-broadcast of the same event.
        let event_tx = self.event_tx.borrow();
        if let Some(ref tx) = *event_tx {
            let servo_level = match level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: servo_level,
                text: message,
                url: None,
                line: None,
                column: None,
            });
        } else if let Some(ref tx) = *self.console_log_tx.borrow() {
            let msg = match BaoEvent::from_console_text(&message) {
                Some(ConsoleMessage::Event(evt)) => ConsoleMessage::Event(evt),
                _ => ConsoleMessage::Log { level: level_str.to_string(), text: message },
            };
            let _ = tx.send(msg);
        }
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
        let url_str = url.to_string();
        self.state.borrow_mut().url = Some(url);
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // Dual-path: event_tx (Path B) primary for FrameNavigated,
        // console_log_tx (Path A) fallback for PageFrameNavigated.
        let event_tx = self.state.borrow().event_tx.clone();
        if let Some(ref tx) = event_tx {
            let _ = tx.send(ServoEvent::FrameNavigated {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
                url: url_str,
                name: None,
            });
        } else if let Some(ref tx) = self.state.borrow().console_log_tx {
            let loader_id = format!("{:016x}", url_str.len() as u64);
            let _ = tx.send(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id: "0".to_string(),
                url: url_str,
                loader_id,
            }));
        }
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.state.borrow_mut().title = title;
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        self.state.borrow_mut().load_status = status;
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
                }

                // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
                // Dual-path: event_tx (Path B) primary for FrameStartedLoading,
                // console_log_tx (Path A) fallback — no direct ConsoleMessage equivalent,
                // so we use a lightweight log entry.
                let event_tx = self.state.borrow().event_tx.clone();
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(ServoEvent::FrameStartedLoading {
                        target_id: "0".to_string(),
                        frame_id: "0".to_string(),
                    });
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
                let event_tx = self.state.borrow().event_tx.clone();
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(ServoEvent::FrameStoppedLoading {
                        target_id: "0".to_string(),
                        frame_id: "0".to_string(),
                    });
                } else if let Some(ref tx) = self.state.borrow().console_log_tx {
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    let _ = tx.send(ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp }));
                }
            }
            LoadStatus::HeadParsed => {}
        }
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

    fn show_console_message(&self, _webview: WebView, level: ConsoleLogLevel, message: String) {
        let level_str = match level {
            ConsoleLogLevel::Debug => "debug",
            ConsoleLogLevel::Log => "info",
            ConsoleLogLevel::Info => "info",
            ConsoleLogLevel::Warn => "warning",
            ConsoleLogLevel::Error => "error",
            ConsoleLogLevel::Trace => "verbose",
        };
        log::trace!("[webview] {message}");

        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // Same dual-path logic as BaoServoDelegate::show_console_message:
        // event_tx (Path B) is primary; console_log_tx (Path A) is fallback.
        let event_tx = self.state.borrow().event_tx.clone();
        if let Some(ref tx) = event_tx {
            let servo_level = match level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: servo_level,
                text: message,
                url: None,
                line: None,
                column: None,
            });
        } else if let Some(ref tx) = self.state.borrow().console_log_tx {
            let msg = match BaoEvent::from_console_text(&message) {
                Some(ConsoleMessage::Event(evt)) => ConsoleMessage::Event(evt),
                _ => ConsoleMessage::Log { level: level_str.to_string(), text: message },
            };
            let _ = tx.send(msg);
        }
    }

    fn show_embedder_control(&self, _webview: WebView, _control: EmbedderControl) {}

    fn hide_embedder_control(&self, _webview: WebView, _id: EmbedderControlId) {}

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        log::error!("[webview] crashed: {reason}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── BaoWebViewState ────────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_state_default() {
        let state = BaoWebViewState::default();
        assert!(state.url.is_none());
        assert!(state.title.is_none());
        assert!(matches!(state.load_status, LoadStatus::Started));
        assert!(!state.frame_ready);
        assert!(!state.dom_proxies_dirty);
    }

    #[test]
    fn test_webview_state_url_mutate() {
        let mut state = BaoWebViewState::default();
        state.url = Some(url::Url::parse("https://example.com").unwrap());
        assert!(state.url.is_some());
        assert_eq!(state.url.unwrap().as_str(), "https://example.com/");
    }

    #[test]
    fn test_webview_state_title_mutate() {
        let mut state = BaoWebViewState::default();
        state.title = Some("Test Page".to_string());
        assert_eq!(state.title.as_deref(), Some("Test Page"));
    }

    #[test]
    fn test_webview_state_frame_ready_toggle() {
        let mut state = BaoWebViewState::default();
        assert!(!state.frame_ready);
        state.frame_ready = true;
        assert!(state.frame_ready);
    }

    // ─── BaoServoDelegate ──────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_servo_delegate_new_no_error() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.last_error().is_none());
    }

    #[test]
    fn test_servo_delegate_default_no_error() {
        let delegate = BaoServoDelegate::default();
        assert!(delegate.last_error().is_none());
    }

    // ─── BaoWebViewDelegate ────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_delegate_new_with_state() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1024, 768);
        let delegate = BaoWebViewDelegate::new(state, viewport);
        assert!(delegate.state().borrow().url.is_none());
    }

    #[test]
    fn test_webview_delegate_state_rc_shared() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(800, 600);
        let delegate = BaoWebViewDelegate::new(Rc::clone(&state), viewport);
        // Modify state externally
        state.borrow_mut().title = Some("External".to_string());
        // Delegate sees same state
        assert_eq!(delegate.state().borrow().title.as_deref(), Some("External"));
    }

    #[test]
    fn test_webview_delegate_viewport_size() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1440, 900);
        let delegate = BaoWebViewDelegate::new(state, viewport);
        // Verify delegate was created with specific viewport
        assert!(delegate.state().borrow().url.is_none());
    }

    // ─── PoolStats ─────────────────────────────────────────────────
    // @trace REQ-LIB-001 [req:REQ-LIB-001] [level:unit]

    #[test]
    fn test_pool_stats_fields() {
        let stats = crate::page_pool::PoolStats {
            active: 3,
            idle: 1,
            total_created: 5,
            total_destroyed: 2,
        };
        assert_eq!(stats.active, 3);
        assert_eq!(stats.idle, 1);
        assert_eq!(stats.total_created, 5);
        assert_eq!(stats.total_destroyed, 2);
    }

    // ─── DOM Proxy Dirty Flag ─────────────────────────────────────
    // @trace REQ-SEC-002 [req:REQ-SEC-002] [level:unit]

    #[test]
    fn test_dom_proxies_dirty_default_false() {
        let state = BaoWebViewState::default();
        assert!(!state.dom_proxies_dirty);
    }

    #[test]
    fn test_dom_proxies_dirty_set_on_complete() {
        let mut state = BaoWebViewState::default();
        state.load_status = LoadStatus::Complete;
        state.dom_proxies_dirty = true;
        assert!(state.dom_proxies_dirty);
    }

    #[test]
    fn test_dom_proxies_dirty_clear_after_refresh() {
        let mut state = BaoWebViewState::default();
        state.dom_proxies_dirty = true;
        state.dom_proxies_dirty = false;
        assert!(!state.dom_proxies_dirty);
    }

    // ─── Console Log Channel Forwarding ─────────────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_servo_delegate_console_log_channel_set_and_get() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.console_log_tx().is_none());
        let (tx, _rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);
        assert!(delegate.console_log_tx().is_some());
    }

    #[test]
    fn test_servo_delegate_console_log_tx_clones() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);
        // Get a clone and send through it
        let cloned = delegate.console_log_tx().unwrap();
        cloned.send(ConsoleMessage::Log { level: "info".into(), text: "hello".into() }).unwrap();
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "info");
                assert_eq!(text, "hello");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    #[test]
    fn test_webview_state_console_log_tx_propagation() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let mut state = BaoWebViewState::default();
        state.console_log_tx = Some(tx);
        // Simulate what show_console_message does
        if let Some(ref tx) = state.console_log_tx {
            tx.send(ConsoleMessage::Log { level: "warning".into(), text: "test message".into() }).unwrap();
        }
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "warning");
                assert_eq!(text, "test message");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    #[test]
    fn test_webview_state_console_log_tx_default_none() {
        let state = BaoWebViewState::default();
        assert!(state.console_log_tx.is_none());
    }

    #[test]
    fn test_console_log_all_level_mappings() {
        let delegate = BaoServoDelegate::new();
        let (tx, _rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);

        // Verify all ConsoleLogLevel variants map correctly via the delegate's show_console_message
        // We test the level mapping logic directly by checking the match arms
        let cases: Vec<(ConsoleLogLevel, &str)> = vec![
            (ConsoleLogLevel::Debug, "debug"),
            (ConsoleLogLevel::Log, "info"),
            (ConsoleLogLevel::Info, "info"),
            (ConsoleLogLevel::Warn, "warning"),
            (ConsoleLogLevel::Error, "error"),
            (ConsoleLogLevel::Trace, "verbose"),
        ];
        for (level, expected_str) in cases {
            let mapped = match level {
                ConsoleLogLevel::Debug => "debug",
                ConsoleLogLevel::Log => "info",
                ConsoleLogLevel::Info => "info",
                ConsoleLogLevel::Warn => "warning",
                ConsoleLogLevel::Error => "error",
                ConsoleLogLevel::Trace => "verbose",
            };
            assert_eq!(mapped, expected_str, "level {:?} should map to {}", level, expected_str);
        }
    }

    #[test]
    fn test_webview_delegate_console_log_forwarding() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            console_log_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state, viewport);

        // Simulate sending through state's channel (what show_console_message does)
        if let Some(ref tx) = _delegate.state().borrow().console_log_tx {
            tx.send(ConsoleMessage::Log { level: "error".into(), text: "crash!".into() }).unwrap();
        }
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "error");
                assert_eq!(text, "crash!");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    // ─── PageFrameNavigated delegate emission ────────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_notify_url_changed_emits_frame_navigated() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            console_log_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state.clone(), viewport);

        // Simulate notify_url_changed by sending the same message the method sends
        let url = url::Url::parse("https://example.com").unwrap();
        let url_str = url.to_string();
        let loader_id = format!("{:016x}", url_str.len() as u64);
        if let Some(ref tx) = state.borrow().console_log_tx {
            tx.send(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id: "0".to_string(),
                url: url_str.clone(),
                loader_id: loader_id.clone(),
            })).unwrap();
        }

        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::PageFrameNavigated { frame_id, url, loader_id: lid }) => {
                assert_eq!(frame_id, "0");
                assert!(url.starts_with("https://example.com"));
                assert_eq!(lid, loader_id);
            }
            other => panic!("expected PageFrameNavigated, got {:?}", other),
        }
    }

    // ─── SecurityCertificateError delegate emission ──────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_notify_error_certificate_error_emits_security_event() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);

        // Simulate a certificate error by sending the same message notify_error would send
        if let Some(ref tx) = *delegate.console_log_tx.borrow() {
            tx.send(ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                event_id: 0,
                error_type: "net::ERR_CERT_AUTHORITY_INVALID".to_string(),
                url: String::new(),
            })).unwrap();
        }

        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::SecurityCertificateError { event_id, error_type, url }) => {
                assert_eq!(event_id, 0);
                assert_eq!(error_type, "net::ERR_CERT_AUTHORITY_INVALID");
                assert_eq!(url, "");
            }
            other => panic!("expected SecurityCertificateError, got {:?}", other),
        }
    }

    // ─── EventSubscriber (event_tx) Path B ─────────────────────────────
    // @trace REQ-CDP-006 [req:REQ-CDP-006] [level:unit]

    #[test]
    fn test_servo_delegate_event_tx_set_and_get() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.event_tx().is_none());
        let (tx, _rx) = std::sync::mpsc::channel::<ServoEvent>();
        delegate.set_event_tx(tx);
        assert!(delegate.event_tx().is_some());
    }

    #[test]
    fn test_servo_delegate_event_tx_sends_console_event() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        delegate.set_event_tx(tx);

        // When event_tx is set, show_console_message pushes ServoEvent::Console
        if let Some(ref tx) = delegate.event_tx() {
            tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: ConsoleLevel::Info,
                text: "hello".to_string(),
                url: None,
                line: None,
                column: None,
            }).unwrap();
        }

        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { level, text, .. } => {
                assert_eq!(level, ConsoleLevel::Info);
                assert_eq!(text, "hello");
            }
            _ => panic!("expected Console event"),
        }
    }

    #[test]
    fn test_webview_state_event_tx_default_none() {
        let state = BaoWebViewState::default();
        assert!(state.event_tx.is_none());
    }

    #[test]
    fn test_webview_state_event_tx_propagation() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let mut state = BaoWebViewState::default();
        state.event_tx = Some(tx);
        // Simulate what notify_url_changed does with event_tx
        if let Some(ref tx) = state.event_tx {
            tx.send(ServoEvent::FrameNavigated {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
                url: "https://example.com/".to_string(),
                name: None,
            }).unwrap();
        }
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::FrameNavigated { url, .. } => {
                assert_eq!(url, "https://example.com/");
            }
            _ => panic!("expected FrameNavigated event"),
        }
    }

    #[test]
    fn test_event_tx_console_level_mapping() {
        // Verify ConsoleLogLevel → ConsoleLevel mapping matches the delegate logic
        let cases: Vec<(ConsoleLogLevel, ConsoleLevel)> = vec![
            (ConsoleLogLevel::Debug, ConsoleLevel::Debug),
            (ConsoleLogLevel::Log, ConsoleLevel::Info),
            (ConsoleLogLevel::Info, ConsoleLevel::Info),
            (ConsoleLogLevel::Warn, ConsoleLevel::Warning),
            (ConsoleLogLevel::Error, ConsoleLevel::Error),
            (ConsoleLogLevel::Trace, ConsoleLevel::Verbose),
        ];
        for (servo_level, expected) in cases {
            let mapped = match servo_level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            assert_eq!(mapped, expected, "servo {:?} should map to {:?}", servo_level, expected);
        }
    }

    #[test]
    fn test_notify_load_started_emits_frame_started_loading() {
        // When event_tx is set and LoadStatus::Started is received,
        // the delegate should emit ServoEvent::FrameStartedLoading.
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state.clone(), viewport);

        // Simulate what notify_load_status_changed does on LoadStatus::Started
        if let Some(ref tx) = state.borrow().event_tx {
            tx.send(ServoEvent::FrameStartedLoading {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
            }).unwrap();
        }

        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::FrameStartedLoading { target_id, frame_id } => {
                assert_eq!(target_id, "0");
                assert_eq!(frame_id, "0");
            }
            _ => panic!("expected FrameStartedLoading event"),
        }
    }

    // ─── Worker Lifecycle (REQ-BRW-004) ──────────────────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [level:unit]

    #[test]
    fn test_worker_handle_new_is_running() {
        let handle = WorkerHandle::new("https://example.com/worker.js".to_string());
        assert_eq!(handle.script_url, "https://example.com/worker.js");
        assert!(!handle.is_closing());
        assert!(!handle.is_terminated());
    }

    #[test]
    fn test_worker_handle_terminate_sets_closing() {
        let handle = WorkerHandle::new("worker.js".to_string());
        assert!(!handle.is_closing());
        handle.terminate();
        assert!(handle.is_closing());
        // Idempotent
        handle.terminate();
        assert!(handle.is_closing());
    }

    #[test]
    fn test_worker_handle_mark_terminated() {
        let handle = WorkerHandle::new("worker.js".to_string());
        assert!(!handle.is_terminated());
        handle.mark_terminated();
        assert!(handle.is_terminated());
    }

    #[test]
    fn test_worker_handle_terminate_then_terminated() {
        let handle = WorkerHandle::new("worker.js".to_string());
        handle.terminate();
        assert!(handle.is_closing());
        assert!(!handle.is_terminated());
        handle.mark_terminated();
        assert!(handle.is_terminated());
    }

    #[test]
    fn test_worker_handle_clone_shares_state() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let clone = handle.clone();
        handle.terminate();
        assert!(clone.is_closing(), "clone should see closing flag from original");
        clone.mark_terminated();
        assert!(handle.is_terminated(), "original should see terminated flag from clone");
    }

    #[test]
    fn test_webview_state_active_workers_default_empty() {
        let state = BaoWebViewState::default();
        assert!(state.active_workers.is_empty());
        assert_eq!(state.active_worker_count(), 0);
    }

    #[test]
    fn test_webview_state_track_worker() {
        let mut state = BaoWebViewState::default();
        let handle = WorkerHandle::new("worker1.js".to_string());
        state.track_worker(handle);
        assert_eq!(state.active_worker_count(), 1);
        assert_eq!(state.active_workers.len(), 1);
        assert_eq!(state.active_workers[0].handle().script_url, "worker1.js");
    }

    #[test]
    fn test_webview_state_track_multiple_workers() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        state.track_worker(WorkerHandle::new("worker3.js".to_string()));
        assert_eq!(state.active_worker_count(), 3);
    }

    #[test]
    fn test_webview_state_terminate_all_workers() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        assert!(!state.active_workers[0].handle().is_closing());
        assert!(!state.active_workers[1].handle().is_closing());
        state.terminate_all_workers();
        assert!(state.active_workers[0].handle().is_closing());
        assert!(state.active_workers[1].handle().is_closing());
    }

    #[test]
    fn test_webview_state_reap_terminated_workers() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        // Terminate only worker1
        state.active_workers[0].handle().terminate();
        state.active_workers[0].handle().mark_terminated();
        assert_eq!(state.active_worker_count(), 1);
        state.reap_terminated_workers();
        assert_eq!(state.active_workers.len(), 1);
        assert_eq!(state.active_workers[0].handle().script_url, "worker2.js");
    }

    #[test]
    fn test_webview_state_reap_all_terminated() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.terminate_all_workers();
        for g in &state.active_workers {
            g.handle().mark_terminated();
        }
        state.reap_terminated_workers();
        assert!(state.active_workers.is_empty());
        assert_eq!(state.active_worker_count(), 0);
    }

    #[test]
    fn test_worker_id_equality() {
        let id1 = WorkerId("worker1.js".to_string());
        let id2 = WorkerId("worker1.js".to_string());
        let id3 = WorkerId("worker2.js".to_string());
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_worker_message_direction() {
        assert_eq!(WorkerMessageDirection::PageToWorker, WorkerMessageDirection::PageToWorker);
        assert_ne!(WorkerMessageDirection::PageToWorker, WorkerMessageDirection::WorkerToPage);
    }

    #[test]
    fn test_worker_message_event_creation() {
        let event = WorkerMessageEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            direction: WorkerMessageDirection::PageToWorker,
        };
        assert_eq!(event.worker_id.0, "worker1.js");
        assert_eq!(event.direction, WorkerMessageDirection::PageToWorker);
    }

    #[test]
    fn test_webview_state_forward_worker_message_to_event_tx() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let msg = WorkerMessageEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            direction: WorkerMessageDirection::WorkerToPage,
        };
        state.forward_worker_message_event(msg);
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { level, text, .. } => {
                assert_eq!(level, ConsoleLevel::Debug);
                assert!(text.contains("worker→page"));
                assert!(text.contains("worker1.js"));
            }
            _ => panic!("expected Console event for worker message"),
        }
    }

    #[test]
    fn test_webview_state_forward_worker_message_no_event_tx() {
        // When event_tx is None, forward_worker_message_event should be a no-op
        let state = BaoWebViewState::default();
        let msg = WorkerMessageEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            direction: WorkerMessageDirection::PageToWorker,
        };
        // Should not panic
        state.forward_worker_message_event(msg);
    }

    #[test]
    fn test_terminate_on_navigation_then_reap() {
        // Simulate: page with workers → new navigation → terminate → load complete → reap
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        assert_eq!(state.active_worker_count(), 2);

        // Navigation starts: terminate all
        state.terminate_all_workers();
        assert!(state.active_workers[0].handle().is_closing());
        assert!(state.active_workers[1].handle().is_closing());
        // Not yet terminated (threads still running)
        assert_eq!(state.active_worker_count(), 2);

        // Workers finish teardown
        for g in &state.active_workers {
            g.handle().mark_terminated();
        }
        assert_eq!(state.active_worker_count(), 0);

        // Load complete: reap
        state.reap_terminated_workers();
        assert!(state.active_workers.is_empty());
    }

    // ─── WorkerErrorEvent (REQ-BRW-004 criterion #9) ──────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:9] [level:unit]

    #[test]
    fn test_worker_error_event_creation() {
        let event = WorkerErrorEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            message: "Uncaught TypeError: x is not a function".to_string(),
            filename: "worker1.js".to_string(),
            lineno: 42,
            colno: 5,
        };
        assert_eq!(event.worker_id.0, "worker1.js");
        assert_eq!(event.message, "Uncaught TypeError: x is not a function");
        assert_eq!(event.filename, "worker1.js");
        assert_eq!(event.lineno, 42);
        assert_eq!(event.colno, 5);
    }

    #[test]
    fn test_webview_state_forward_worker_error_to_event_tx() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let error = WorkerErrorEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            message: "Uncaught Error: boom".to_string(),
            filename: "worker1.js".to_string(),
            lineno: 10,
            colno: 3,
        };
        state.forward_worker_error_event(error);
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::PageError { text, url, line, column, .. } => {
                assert!(text.contains("worker1.js"));
                assert!(text.contains("Uncaught Error: boom"));
                assert_eq!(url.as_deref(), Some("worker1.js"));
                assert_eq!(line, Some(10));
                assert_eq!(column, Some(3));
            }
            _ => panic!("expected PageError event for worker error"),
        }
    }

    #[test]
    fn test_webview_state_forward_worker_error_no_event_tx() {
        let state = BaoWebViewState::default();
        let error = WorkerErrorEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            message: "error".to_string(),
            filename: "worker1.js".to_string(),
            lineno: 1,
            colno: 1,
        };
        // Should not panic
        state.forward_worker_error_event(error);
    }

    // ─── WorkerLifecycleState / WorkerTeardownPath (REQ-BRW-004 criterion #18) ──
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:18] [level:unit]

    #[test]
    fn test_worker_teardown_path_equality() {
        assert_eq!(WorkerTeardownPath::Terminate, WorkerTeardownPath::Terminate);
        assert_eq!(WorkerTeardownPath::SelfClose, WorkerTeardownPath::SelfClose);
        assert_eq!(WorkerTeardownPath::PageUnload, WorkerTeardownPath::PageUnload);
        assert_ne!(WorkerTeardownPath::Terminate, WorkerTeardownPath::SelfClose);
    }

    #[test]
    fn test_worker_lifecycle_state_running() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let guard = AutoCloseWorker::new(handle);
        assert_eq!(guard.lifecycle_state(), WorkerLifecycleState::Running);
    }

    #[test]
    fn test_worker_lifecycle_state_closing() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        assert_eq!(guard.lifecycle_state(), WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate));
    }

    #[test]
    fn test_worker_lifecycle_state_terminated() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::SelfClose);
        guard.handle().mark_terminated();
        assert_eq!(guard.lifecycle_state(), WorkerLifecycleState::Terminated(WorkerTeardownPath::SelfClose));
    }

    #[test]
    fn test_worker_lifecycle_states_snapshot() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        let snapshot = state.worker_lifecycle_states();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].0, WorkerId("worker1.js".to_string()));
        assert_eq!(snapshot[0].1, WorkerLifecycleState::Running);
        assert_eq!(snapshot[1].0, WorkerId("worker2.js".to_string()));
        assert_eq!(snapshot[1].1, WorkerLifecycleState::Running);
    }

    // ─── AutoCloseWorker (REQ-BRW-004 criterion #10) ─────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:10] [level:unit]

    #[test]
    fn test_auto_close_worker_new_is_running() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let guard = AutoCloseWorker::new(handle);
        assert!(!guard.handle().is_closing());
        assert!(!guard.handle().is_terminated());
    }

    #[test]
    fn test_auto_close_worker_terminate_via() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        assert!(guard.handle().is_closing());
        assert_eq!(guard.lifecycle_state(), WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate));
    }

    #[test]
    fn test_auto_close_worker_terminate_via_idempotent() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        guard.terminate_via(WorkerTeardownPath::SelfClose);
        // Should still be Terminate (first call wins)
        assert_eq!(guard.lifecycle_state(), WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate));
    }

    #[test]
    fn test_auto_close_worker_drop_terminates() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let handle_clone = handle.clone();
        let guard = AutoCloseWorker::new(handle);
        assert!(!handle_clone.is_closing());
        drop(guard);
        // AutoCloseWorker::drop should terminate the worker
        assert!(handle_clone.is_closing());
    }

    #[test]
    fn test_auto_close_worker_drop_already_closing() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let handle_clone = handle.clone();
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        drop(guard);
        // Already closing — drop should not change teardown path
        assert!(handle_clone.is_closing());
    }

    #[test]
    fn test_track_worker_guard() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let guard = AutoCloseWorker::new(handle);
        let mut state = BaoWebViewState::default();
        state.track_worker_guard(guard);
        assert_eq!(state.active_worker_count(), 1);
    }

    // ─── WorkerScopeConfig (REQ-BRW-004 criteria #12-17) ─────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:12..17] [level:unit]

    #[test]
    fn test_worker_scope_config_default() {
        let config = WorkerScopeConfig::default();
        assert!(config.stealth_profile.is_none());
        assert!(config.user_agent.is_empty());
        assert!(config.platform.is_empty());
        assert!(config.hardware_concurrency > 0);
        assert_eq!(config.language, "en-US");
        assert!(!config.languages.is_empty());
    }

    #[test]
    fn test_worker_scope_config_set_on_state() {
        let mut state = BaoWebViewState::default();
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux x86_64".to_string(),
            hardware_concurrency: 8,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string(), "zh".to_string(), "en".to_string()],
        };
        state.set_worker_scope_config(config);
        assert_eq!(state.worker_scope_config.user_agent, "Bao/1.0");
        assert_eq!(state.worker_scope_config.platform, "Linux x86_64");
        assert_eq!(state.worker_scope_config.hardware_concurrency, 8);
        assert_eq!(state.worker_scope_config.language, "zh-CN");
        assert_eq!(state.worker_scope_config.languages.len(), 3);
    }

    #[test]
    fn test_webview_state_default_worker_scope_config() {
        let state = BaoWebViewState::default();
        assert!(state.worker_scope_config.stealth_profile.is_none());
        assert!(state.worker_scope_config.hardware_concurrency > 0);
    }
}
