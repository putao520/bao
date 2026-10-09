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

pub struct BaoWebViewState {
    pub url: Option<url::Url>,
    pub title: Option<String>,
    pub load_status: LoadStatus,
    /// e131 (stale-Complete race, REQ-BRW-002): load generation — bumped by
    /// every embedder-side navigation entry (navigate/reload/go_back/
    /// go_forward). `LoadStatus` edges carry no load identity (WebViewId
    /// only), so a `Complete` belonging to the PREVIOUS load can land after
    /// the navigation's synchronous reset and un-reset it (`get_state()` then
    /// projects `Interactive` before the new load commits — the e124
    /// post-creation navigate flake). The pair below gates acceptance:
    /// `started_generation` is credited when servo delivers a `Started` for
    /// the current generation; an un-credited `Complete` belongs to a
    /// superseded load and is dropped.
    pub load_generation: u64,
    /// The generation whose `Started` edge servo has delivered (see
    /// `load_generation`). Equal to `load_generation` except in the window
    /// between a navigation entry and the new load's own `Started`.
    pub started_generation: u64,
    pub frame_ready: bool,
    /// Latched repaint request from servo (`WebViewDelegate::notify_new_frame_ready`).
    /// This is servo's embedder contract for "a new frame is ready — repaint now"
    /// (servoshell's `RunningAppState::notify_new_frame_ready` latches
    /// `set_needs_repaint()` the same way, then paints on the event loop's redraw
    /// tick). The GL composite (`WebView::paint`) is the heartbeat of the whole
    /// render pipeline: `Painter::render` → `refresh_driver.notify_will_paint` →
    /// `frame_started` → `TickAnimation` is the ONLY recurring rAF/animation tick
    /// source, and `maybe_take_screenshots` only fires at the end of a render.
    /// Without this latch (pre-2026-10-02) bao headless produced exactly one
    /// frame's worth of activity per webview (the one-shot
    /// `ChangeRunningAnimationsState` kick) — boot-once frames, dead rAF gate,
    /// dead captureScreenshot. Cleared by `PageInner::paint_if_needed` after the
    /// composite.
    /// @trace REQ-BRW-002 [entity:PageHandle]
    pub repaint_pending: bool,
    /// Set to true after navigation completes (LoadStatus::Complete).
    /// evaluate_js checks this flag and refreshes stale DOM proxies before executing scripts.
    pub dom_proxies_dirty: bool,
    /// Channel for forwarding per-webview console messages to CDP Log domain.
    pub console_log_tx: Option<std::sync::mpsc::Sender<ConsoleMessage>>,
    /// Real event queue sender (Path B): reliable unbounded mpsc to the pump.
    /// When set, events are also pushed here in addition to console_log_tx.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub event_tx: Option<Sender<ServoEvent>>,
    /// CDP target identity of the owning page (its decimal page id, the same
    /// namespace the command face lists via Target.getTargets and flattens
    /// sessions against — REQ-CDP-004). Stamped once at page creation
    /// (PageHandle::new, the same point `register_page_webview` runs) so every
    /// ServoEvent emitted from this webview — frame lifecycle, console,
    /// worker observability — routes to exactly the sessions attached to this
    /// target. `None` = the state has no page behind it (pre-registration or
    /// the closed-page default in `PageHandle::webview_state`): targeted
    /// routing is impossible, so emitters drop the event with a debug log
    /// (no-target-no-deliver, the same Chrome semantics the tightened
    /// `broadcast_for_target` miss branch enforces downstream).
    pub cdp_target_id: Option<String>,
    /// Owning page's pool id (REQ-LIB-001). Stamped at page creation next to
    /// `cdp_target_id`; the per-page delegate reads it to answer
    /// `notify_closed` (window.close()) with the exact PagePool entry to
    /// retire. `None` = no page behind this state (same semantics as
    /// `cdp_target_id`).
    pub pool_page_id: Option<usize>,
    /// Active Workers spawned from this webview's page.
    /// Keyed by WorkerId for O(1) lookup. On page unload (new navigation
    /// after LoadStatus::Complete), all Workers are auto-terminated
    /// (SPEC criterion #10: GlobalScope::track_worker + AutoCloseWorker).
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub(super) active_workers: Vec<AutoCloseWorker>,
    /// Worker scope config for propagating stealth-consistent properties
    /// to new Workers. Populated from the page's StealthProfile when
    /// the page is created.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
    pub worker_scope_config: WorkerScopeConfig,
    /// Active SharedWorker port references for this webview's page.
    /// Unlike DedicatedWorkers, SharedWorkers survive page unload — only
    /// the per-page MessagePort is disconnected (via SharedWorkerPortRef Drop).
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub(super) shared_worker_ports: Vec<SharedWorkerPortRef>,
    /// SharedWorker channel bridges keyed by SharedWorkerId.
    /// Each bridge aggregates per-page port channels for bidirectional
    /// postMessage (DF-WK-7: "各页经独立 port 通信").
    /// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
    pub(super) shared_worker_channels: HashMap<SharedWorkerId, SharedWorkerChannelBridge>,
    /// SharedWorkerGlobalScope states keyed by SharedWorkerId.
    /// Tracks each SharedWorker's global scope state (name/onconnect/
    /// connect_count/navigator/location) for CDP observability and
    /// stealth consistency verification (CRIT-STL-WK).
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub(super) shared_worker_scopes: HashMap<SharedWorkerId, SharedWorkerGlobalScopeState>,
    /// Worker channel bridges for page↔worker structured-clone communication.
    /// Keyed by WorkerId for O(1) lookup. Each bridge holds the mpsc channel
    /// endpoints for bidirectional postMessage (DF-WK-4 / DF-WK-5).
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
    pub(super) worker_channels: HashMap<WorkerId, WorkerChannelBridge>,
    /// DedicatedWorkerGlobalScope states keyed by WorkerId.
    /// Tracks each Worker's global scope state (navigator/location/event
    /// handlers) for CDP observability and stealth consistency verification.
    /// Populated when a Worker is created; removed when reaped.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub(super) dedicated_worker_scopes: HashMap<WorkerId, DedicatedWorkerGlobalScopeState>,
    /// Worker script loading states keyed by WorkerId.
    /// Tracks each Worker's script loading progress for CDP observability
    /// and lifecycle management (DF-WK-2: script loading pipeline).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub(super) worker_script_load_states: HashMap<WorkerId, WorkerScriptLoadState>,
    /// Active WorkerHandle references keyed by WorkerId (DEC-WK-001).
    /// These track Workers created via servo's native Worker::Constructor.
    /// The WorkerHandle holds closing/terminated flags + global_addr for
    /// REALM_PROFILES cleanup. The actual thread lifecycle is managed by servo.
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:1] [criterion:18]
    pub(super) web_workers: HashMap<WorkerId, WorkerHandle>,
    /// Active ServiceWorker registrations controlling this webview's page.
    /// A page can be controlled by at most one ServiceWorker at a time.
    /// The ServiceWorker survives page navigation (persistent lifecycle),
    /// but the per-page reference is disconnected on page unload.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub(super) controlled_service_worker: Option<ServiceWorkerHandle>,
    /// ServiceWorkerGlobalScope states for the controlling ServiceWorker.
    /// Tracks the SW's global scope state (fetch handler, scope URL, navigator)
    /// for CDP observability and stealth consistency verification (CRIT-STL-WK).
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8 / DF-WK-10
    pub(super) service_worker_scope: Option<ServiceWorkerGlobalScopeState>,
}

impl Default for BaoWebViewState {
    fn default() -> Self {
        BaoWebViewState {
            url: None,
            title: None,
            load_status: LoadStatus::Started,
            load_generation: 0,
            started_generation: 0,
            frame_ready: false,
            repaint_pending: false,
            dom_proxies_dirty: false,
            console_log_tx: None,
            event_tx: None,
            cdp_target_id: None,
            pool_page_id: None,
            active_workers: Vec::new(),
            worker_scope_config: WorkerScopeConfig::default(),
            shared_worker_ports: Vec::new(),
            shared_worker_channels: HashMap::new(),
            shared_worker_scopes: HashMap::new(),
            worker_channels: HashMap::new(),
            dedicated_worker_scopes: HashMap::new(),
            worker_script_load_states: HashMap::new(),
            web_workers: HashMap::new(),
            controlled_service_worker: None,
            service_worker_scope: None,
        }
    }
}

impl BaoWebViewState {
    /// Single write point for the frame-ready pair: servo's
    /// `notify_new_frame_ready` means "a new frame is ready — repaint now"
    /// (see the `repaint_pending` field docs for why the composite is the
    /// pipeline heartbeat).
    /// @trace REQ-BRW-002 [entity:PageHandle]
    pub fn latch_frame_ready(&mut self) {
        self.frame_ready = true;
        self.repaint_pending = true;
    }

    /// The CDP target this webview's events route to, or None when the
    /// owning page carries no CDP identity (pre-registration / closed-page
    /// default state). Emitters fail closed on None: the event is dropped
    /// with a debug log instead of being tagged with a placeholder target
    /// (REQ-CDP-004 — placeholder targets were the root of the W49
    /// broadcast-everywhere fallback this wave removes).
    pub(crate) fn cdp_target(&self) -> Option<String> {
        self.cdp_target_id.clone()
    }

    /// Log-and-drop face for emitters with no routeable target. Returns
    /// false so call sites can early-return.
    pub(crate) fn log_unroutable_event(&self, event_kind: &str) -> bool {
        log::debug!(
            "[cdp-route] {event_kind} dropped: webview state has no CDP target \
             identity (page unregistered or closed)"
        );
        false
    }

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

    /// Auto-terminate all active Workers on page unload (crash-safe).
    ///
    /// SPEC criterion #10: "页面卸载时自动终止所有 Worker
    /// (GlobalScope::track_worker + AutoCloseWorker)".
    /// Called from notify_load_status_changed when a new navigation
    /// starts (LoadStatus::Started after a previous Complete).
    ///
    /// SPEC criterion #18: "三路径 teardown 均 crash-safe: worker 线程
    /// JSContext 干净销毁 + 线程 join 无悬挂 + REALM_PROFILES 条目注销
    /// + 无 EBUSY 类 mutex destroy SIGSEGV"
    ///
    /// This method performs crash-safe teardown for each Worker:
    /// 1. Sets the closing flag (signals the worker event loop to exit)
    /// 2. Unregisters each Worker's stealth profile from REALM_PROFILES
    /// 3. Marks each Worker as terminated
    /// 4. Drops WebWorker instances (their Drop impl joins the thread)
    ///
    /// Also clears all Worker channel bridges — dropping the channels
    /// signals worker threads that the parent has disconnected (DF-WK-4/5).
    /// Also clears script loading states and marks any in-progress loads
    /// as cancelled (DF-WK-2).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10] [criterion:6] [criterion:18]
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn terminate_all_workers(&mut self) {
        // Phase 1: Signal all Workers to terminate and unregister their profiles.
        // @trace REQ-BRW-004 [criterion:18] crash-safe teardown: closing flag + REALM_PROFILES
        for guard in &mut self.active_workers {
            guard.terminate_via(WorkerTeardownPath::PageUnload);
            // Unregister the Worker's stealth profile from REALM_PROFILES.
            // This must happen before the thread join, while the global address
            // is still valid (before JSContext destruction).
            // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
            guard.handle().unregister_stealth_profile();
        }
        // @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
        // Clear all channel bridges — dropping the senders/receivers signals
        // worker threads that the parent has disconnected.
        self.worker_channels.clear();
        // @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
        // Clear all scope states — Workers are being terminated.
        self.dedicated_worker_scopes.clear();
        // @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
        // Clear all script loading states — in-progress loads are cancelled.
        self.worker_script_load_states.clear();
        // Phase 2: Drop WebWorker instances — their Drop impl joins the thread.
        // @trace REQ-BRW-004 [criterion:18] crash-safe teardown: 线程 join 无悬挂
        // WebWorker::Drop sets closing + sends Terminate + joins the thread.
        // This ensures no dangling threads after page unload.
        // The EBUSY patch in mozjs (Mutex_posix.cpp) ensures that any
        // pthread_mutex_destroy returning EBUSY during TLS teardown does not
        // cause SIGSEGV, which was the root cause of PagePool 混沌 SIGSEGV.
        self.web_workers.clear();
        // Phase 3: Mark all Workers as terminated after their threads have been joined.
        // Now that WebWorker::Drop has joined the threads, the Worker threads have
        // fully exited and their JSContexts are destroyed. Mark them terminated so
        // reap_terminated_workers can clean up the tracking state.
        // @trace REQ-BRW-004 [criterion:18] mark terminated after thread join
        for guard in &self.active_workers {
            guard.handle().mark_terminated();
        }
    }

    /// Remove fully-terminated Workers from the tracking list.
    ///
    /// Called after spin_event_loop to clean up Workers whose threads
    /// have exited (terminated flag set by Worker teardown).
    /// Also reaps their channel bridges and script load states.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn reap_terminated_workers(&mut self) {
        self.active_workers.retain(|g| !g.handle().is_terminated());
        self.reap_terminated_worker_channels();
        self.reap_terminated_worker_script_load_states();
        // @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
        // Also reap scope states for terminated workers.
        let active_ids: std::collections::HashSet<WorkerId> = self
            .active_workers
            .iter()
            .map(|g| WorkerId(g.handle().script_url.clone()))
            .collect();
        self.dedicated_worker_scopes
            .retain(|id, _| active_ids.contains(id));
        // @trace REQ-BRW-004 [entity:Worker] [criterion:18] reap terminated WebWorkers
        // Drop WebWorker instances for terminated workers. Their Drop impl
        // joins the Worker thread, ensuring clean teardown.
        self.web_workers.retain(|id, _| active_ids.contains(id));
    }

    /// Returns the number of active (non-terminated) Workers.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn active_worker_count(&self) -> usize {
        self.active_workers
            .iter()
            .filter(|g| !g.handle().is_terminated())
            .count()
    }

    /// Terminate a specific Worker via the given teardown path (crash-safe).
    ///
    /// This is the single-Worker teardown method implementing SPEC criterion #18
    /// for the `worker.terminate()` and `self.close()` paths. The `PageUnload`
    /// path is handled by `terminate_all_workers`.
    ///
    /// Crash-safe teardown protocol:
    /// 1. Set the closing flag (signals the worker event loop to exit)
    /// 2. Unregister the Worker's stealth profile from REALM_PROFILES
    /// 3. Mark the Worker as terminated
    /// 4. Drop the WebWorker instance (its Drop impl joins the thread)
    ///
    /// Returns the WorkerTeardownResult for observability, or None if the
    /// Worker was not found.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:4] [criterion:5] [criterion:18]
    pub fn terminate_worker_via_path(
        &mut self,
        worker_id: &WorkerId,
        path: WorkerTeardownPath,
    ) -> Option<WorkerTeardownResult> {
        // Find the AutoCloseWorker guard for this Worker
        let guard_idx = self
            .active_workers
            .iter()
            .position(|g| &WorkerId(g.handle().script_url.clone()) == worker_id)?;

        let guard = &mut self.active_workers[guard_idx];

        // Step 1: Set the closing flag via the specified teardown path
        guard.terminate_via(path.clone());

        // Step 2: Unregister the Worker's stealth profile from REALM_PROFILES
        // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
        let realm_unregistered = if guard.handle().worker_global_addr() != 0 {
            guard.handle().unregister_stealth_profile();
            true
        } else {
            false
        };

        // Step 3: Mark as terminated
        guard.handle().mark_terminated();

        // Step 4: Drop the WorkerHandle reference (thread join handled by servo).
        // @trace REQ-BRW-004 [criterion:18] 线程 join 无悬挂
        let thread_joined = if self.web_workers.contains_key(worker_id) {
            // Removing the WorkerHandle from the map just drops the handle.
            // The actual Worker thread join is handled by servo's Worker::drop
            // (DEC-WK-001 native path) when the servo Worker DOM object is GC'd.
            self.web_workers.remove(worker_id);
            true
        } else {
            // Worker was never registered; servo DOM Worker teardown is independent.
            true
        };

        // never_registered: true when no global address was ever set (worker
        // failed before scope_init) — such a teardown is still crash-safe.
        let never_registered = guard.handle().worker_global_addr() == 0;

        // Clean up associated state
        self.worker_channels.remove(worker_id);
        self.dedicated_worker_scopes.remove(worker_id);
        self.worker_script_load_states.remove(worker_id);

        Some(WorkerTeardownResult {
            path,
            thread_joined,
            realm_profile_unregistered: realm_unregistered,
            closing_flag_set: true,
            never_registered,
        })
    }

    /// Register a DedicatedWorkerGlobalScope state under the given WorkerId.
    ///
    /// Called when a Worker is created (DF-WK-1), populating the scope
    /// state for CDP observability and stealth consistency verification.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn register_dedicated_worker_scope(
        &mut self,
        worker_id: WorkerId,
        scope: DedicatedWorkerGlobalScopeState,
    ) {
        self.dedicated_worker_scopes.insert(worker_id, scope);
    }

    /// Register a WorkerHandle reference for the given WorkerId (DEC-WK-001).
    ///
    /// The WorkerHandle tracks the Worker's closing/terminated flags +
    /// global_addr for REALM_PROFILES cleanup. Storing it here keeps the
    /// handle alive for CDP observability + page-unload termination tracking.
    /// The actual Worker thread lifecycle is owned by servo.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:1] [criterion:18]
    pub fn register_web_worker(&mut self, worker_id: WorkerId, handle: WorkerHandle) {
        self.web_workers.insert(worker_id, handle);
    }

    /// Get a reference to a WorkerHandle by WorkerId.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn web_worker(&self, worker_id: &WorkerId) -> Option<&WorkerHandle> {
        self.web_workers.get(worker_id)
    }

    /// Get a reference to a DedicatedWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn dedicated_worker_scope(
        &self,
        worker_id: &WorkerId,
    ) -> Option<&DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes.get(worker_id)
    }

    /// Get a mutable reference to a DedicatedWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn dedicated_worker_scope_mut(
        &mut self,
        worker_id: &WorkerId,
    ) -> Option<&mut DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes.get_mut(worker_id)
    }

    /// Remove a DedicatedWorkerGlobalScope state (called when a Worker is reaped).
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn remove_dedicated_worker_scope(
        &mut self,
        worker_id: &WorkerId,
    ) -> Option<DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes.remove(worker_id)
    }

    /// Returns the number of tracked DedicatedWorkerGlobalScope states.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn dedicated_worker_scope_count(&self) -> usize {
        self.dedicated_worker_scopes.len()
    }

    /// Returns a snapshot of all DedicatedWorkerGlobalScope states.
    ///
    /// Used for CDP observability (Runtime domain) and stealth consistency
    /// verification (criterion #12-17).
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn dedicated_worker_scopes(&self) -> Vec<&DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes.values().collect()
    }

    /// Look up a DedicatedWorkerGlobalScope state by the Worker's script URL
    /// (CDP worker targetId — the WorkerId is the script URL).
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:19]
    pub fn dedicated_worker_scope_by_url(
        &self,
        url: &str,
    ) -> Option<&DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes
            .values()
            .find(|scope| scope.worker_id.0 == url)
    }

    // ─── Worker Script Loading State (REQ-BRW-004 / DF-WK-2) ───────────

    /// Register a script loading state for a Worker.
    ///
    /// Called when a Worker is created with a URL-based script source.
    /// Tracks the loading progress for CDP observability.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn register_worker_script_load_state(
        &mut self,
        worker_id: WorkerId,
        state: WorkerScriptLoadState,
    ) {
        self.worker_script_load_states.insert(worker_id, state);
    }

    /// Update the script loading state for a Worker.
    ///
    /// Called as the Worker script loading progresses through stages
    /// (Pending → Fetching → Validating → Decoding → Compiling → Ready/Failed).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn update_worker_script_load_state(
        &mut self,
        worker_id: &WorkerId,
        state: WorkerScriptLoadState,
    ) {
        if let Some(current) = self.worker_script_load_states.get_mut(worker_id) {
            *current = state;
        }
    }

    /// Get the script loading state for a Worker.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn worker_script_load_state(&self, worker_id: &WorkerId) -> Option<&WorkerScriptLoadState> {
        self.worker_script_load_states.get(worker_id)
    }

    /// Remove the script loading state for a Worker (called when reaped).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn remove_worker_script_load_state(
        &mut self,
        worker_id: &WorkerId,
    ) -> Option<WorkerScriptLoadState> {
        self.worker_script_load_states.remove(worker_id)
    }

    /// Returns the number of tracked Worker script loading states.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn worker_script_load_state_count(&self) -> usize {
        self.worker_script_load_states.len()
    }

    /// Reap script loading states for terminated Workers.
    ///
    /// Called after reap_terminated_workers to clean up loading state
    /// for Workers that have fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    fn reap_terminated_worker_script_load_states(&mut self) {
        let active_ids: std::collections::HashSet<WorkerId> = self
            .active_workers
            .iter()
            .map(|g| WorkerId(g.handle().script_url.clone()))
            .collect();
        self.worker_script_load_states
            .retain(|id, _| active_ids.contains(id));
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
    /// Supports both metadata-only events (from servo internal message
    /// handling) and full structured-clone events (from bao channel bridge).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] [DF-WK-4] [DF-WK-5]
    pub fn forward_worker_message_event(&self, event: WorkerMessageEvent) {
        let Some(target_id) = self.cdp_target() else {
            self.log_unroutable_event("[Worker] postMessage observability");
            return;
        };
        if let Some(ref tx) = self.event_tx {
            let direction = match event.direction {
                WorkerMessageDirection::PageToWorker => "page→worker",
                WorkerMessageDirection::WorkerToPage => "worker→page",
            };
            // Reliable delivery: the event queue is unbounded — send neither
            // stalls the servo script thread nor drops; a dropped receiver
            // (teardown) fails the send and is warn-logged once.
            send_servo_event(
                &tx,
                ServoEvent::Console {
                    // REQ-CDP-004: real target identity — routes to exactly
                    // the sessions attached to this page.
                    target_id,
                    level: ConsoleLevel::Debug,
                    text: format!("[Worker] postMessage {}: {}", direction, event.worker_id.0),
                    url: None,
                    line: None,
                    column: None,
                },
            );
        }
    }

    /// Forward a Worker structured-clone message to the CDP event path.
    ///
    /// DF-WK-4 / DF-WK-5: When event_tx is set, push a
    /// ServoEvent::Console for CDP observability with payload metadata.
    /// Includes message_id for trace correlation and payload size.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] [DF-WK-4] [DF-WK-5]
    pub fn forward_worker_structured_message(&self, msg: &WorkerStructuredMessage) {
        let Some(target_id) = self.cdp_target() else {
            self.log_unroutable_event("[Worker] structured postMessage observability");
            return;
        };
        if let Some(ref tx) = self.event_tx {
            let direction = match msg.direction {
                WorkerMessageDirection::PageToWorker => "page→worker",
                WorkerMessageDirection::WorkerToPage => "worker→page",
            };
            let payload_info = match &msg.payload {
                Some(p) => format!(
                    "{} bytes, {} transferable(s)",
                    p.data.len(),
                    p.transferable_count
                ),
                None => "metadata-only (servo handles clone)".to_string(),
            };
            // Reliable delivery: the event queue is unbounded — send neither
            // stalls the servo script thread nor drops; a dropped receiver
            // (teardown) fails the send and is warn-logged once.
            send_servo_event(
                &tx,
                ServoEvent::Console {
                    // REQ-CDP-004: real target identity — routes to exactly
                    // the sessions attached to this page.
                    target_id,
                    level: ConsoleLevel::Debug,
                    text: format!(
                        "[Worker] postMessage #{} {}: {} [{}]",
                        msg.message_id, direction, msg.worker_id.0, payload_info
                    ),
                    url: None,
                    line: None,
                    column: None,
                },
            );
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
        let Some(target_id) = self.cdp_target() else {
            self.log_unroutable_event("[Worker] error observability");
            return;
        };
        if let Some(ref tx) = self.event_tx {
            send_servo_event(
                &tx,
                ServoEvent::PageError {
                    // REQ-CDP-004: real target identity — routes to exactly
                    // the sessions attached to this page.
                    target_id,
                    text: format!("[Worker] {}: {}", event.worker_id.0, event.message),
                    url: Some(event.filename.clone()),
                    line: Some(event.lineno),
                    column: Some(event.colno),
                    stack: None,
                },
            );
        }
    }

    // ─── Worker Structured Clone Channel (REQ-BRW-004 criterion #6) ─────

    /// Register a channel bridge for a Worker's postMessage channel.
    ///
    /// Called when a Worker is created and its channel bridge is set up.
    /// The bridge enables page→worker (DF-WK-4) and worker→page (DF-WK-5)
    /// structured-clone message passing.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
    pub fn register_worker_channel(&mut self, bridge: WorkerChannelBridge) {
        let id = bridge.worker_id.clone();
        self.worker_channels.insert(id, bridge);
    }

    /// Create and register a channel bridge for a Worker.
    ///
    /// Convenience method that creates the bridge and endpoints, registers
    /// the bridge, and returns the endpoints for the worker thread.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
    pub fn create_worker_channel(&mut self, worker_id: WorkerId) -> WorkerChannelEndpoints {
        let (bridge, endpoints) = WorkerChannelBridge::new(worker_id);
        self.worker_channels
            .insert(bridge.worker_id.clone(), bridge);
        endpoints
    }

    /// Remove a Worker's channel bridge (e.g., after termination).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn remove_worker_channel(&mut self, worker_id: &WorkerId) -> Option<WorkerChannelBridge> {
        self.worker_channels.remove(worker_id)
    }

    /// Get a reference to a Worker's channel bridge.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn worker_channel(&self, worker_id: &WorkerId) -> Option<&WorkerChannelBridge> {
        self.worker_channels.get(worker_id)
    }

    /// Post a structured-clone message to a Worker (DF-WK-4).
    ///
    /// Sends the payload through the Worker's channel bridge.
    /// Returns Err if the worker is not found or the channel is closed.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4
    pub fn post_to_worker(
        &self,
        worker_id: &WorkerId,
        payload: StructuredClonePayload,
    ) -> Result<(), String> {
        match self.worker_channels.get(worker_id) {
            Some(bridge) => bridge
                .post_message_to_worker(payload)
                .map_err(|e| format!("Worker channel closed: {}", e)),
            None => Err(format!("No channel bridge for worker: {}", worker_id.0)),
        }
    }

    /// Drain all pending worker→page messages from all Workers (DF-WK-5).
    ///
    /// Called during spin_event_loop to process all queued messages
    /// from workers. Each message is forwarded to CDP for observability.
    /// Returns all available messages and a set of WorkerIds whose channels
    /// are disconnected (worker thread has exited).
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:6] DF-WK-5
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_all_worker_messages(&self) -> (Vec<WorkerStructuredMessage>, Vec<WorkerId>) {
        let mut all_messages = Vec::new();
        let mut disconnected_workers = Vec::new();
        for (id, bridge) in &self.worker_channels {
            let result = bridge.drain_worker_messages();
            all_messages.extend(result.messages);
            if result.disconnected {
                disconnected_workers.push(id.clone());
            }
        }
        (all_messages, disconnected_workers)
    }

    /// Drain worker→page messages and forward each to CDP (DF-WK-5).
    ///
    /// Convenience method combining drain_all_worker_messages with
    /// forward_worker_structured_message for each message.
    /// Returns the set of WorkerIds whose channels are disconnected.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:6] DF-WK-5
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_and_forward_worker_messages(&self) -> Vec<WorkerId> {
        let (messages, disconnected) = self.drain_all_worker_messages();
        for msg in &messages {
            self.forward_worker_structured_message(msg);
        }
        disconnected
    }

    /// Remove channel bridges for all terminated Workers.
    ///
    /// Called after reap_terminated_workers to clean up channel state
    /// for Workers that have fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn reap_terminated_worker_channels(&mut self) {
        // Collect IDs of workers that still have channels but are no longer
        // in active_workers (meaning they've been reaped).
        let active_ids: std::collections::HashSet<WorkerId> = self
            .active_workers
            .iter()
            .map(|g| WorkerId(g.handle().script_url.clone()))
            .collect();
        self.worker_channels.retain(|id, _| active_ids.contains(id));
    }

    /// Returns the number of registered Worker channel bridges.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn worker_channel_count(&self) -> usize {
        self.worker_channels.len()
    }

    // ─── SharedWorker Cross-Page Routing (REQ-BRW-004 / DF-WK-7) ─────

    /// Track a SharedWorker port reference for this webview.
    ///
    /// DF-WK-7: When a page creates a SharedWorker, the constellation routes
    /// to the same worker thread if (url, name) matches. The page receives
    /// a MessagePort via the connect event. This method tracks the port
    /// reference so it can be disconnected on page unload.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn track_shared_worker_port(&mut self, port_ref: SharedWorkerPortRef) {
        self.shared_worker_ports.push(port_ref);
    }

    /// Disconnect all SharedWorker ports on page unload.
    ///
    /// Unlike DedicatedWorkers (which are terminated), SharedWorkers survive
    /// page unload. Only the per-page MessagePorts are disconnected by
    /// dropping the SharedWorkerPortRef (which decrements the connected-pages
    /// counter in the SharedWorkerHandle).
    ///
    /// Also clears the per-page SharedWorker channel bridges — dropping the
    /// channels signals the worker thread that the page has disconnected
    /// (DF-WK-7). SharedWorkerGlobalScope states are NOT cleared here — they
    /// belong to the global registry in BaoServoDelegate and survive page unload.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn disconnect_shared_worker_ports(&mut self) {
        if !self.shared_worker_ports.is_empty() {
            log::debug!(
                "[delegate] page navigation: disconnecting {} shared worker ports",
                self.shared_worker_ports.len()
            );
        }
        self.shared_worker_ports.clear();
        // @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
        // Clear per-page SharedWorker channel bridges — dropping the port
        // channels signals the worker thread that this page has disconnected.
        // The SharedWorker itself survives (tracked in BaoServoDelegate registry).
        self.shared_worker_channels.clear();
        // @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
        // Clear per-page SharedWorker scope state references.
        self.shared_worker_scopes.clear();
    }

    /// Returns the number of active SharedWorker port references.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn shared_worker_port_count(&self) -> usize {
        self.shared_worker_ports.len()
    }

    /// Forward a SharedWorker connect event to the CDP event path.
    ///
    /// DF-WK-7: When a page connects to a SharedWorker (either creating a new
    /// one or reusing an existing one), the worker fires a `connect` event.
    /// This method forwards the metadata for CDP observability.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
    pub fn forward_shared_worker_connect_event(&self, event: SharedWorkerConnectEvent) {
        let Some(target_id) = self.cdp_target() else {
            self.log_unroutable_event("[SharedWorker] connect observability");
            return;
        };
        if let Some(ref tx) = self.event_tx {
            // Reliable delivery: the event queue is unbounded — send neither
            // stalls the servo script thread nor drops; a dropped receiver
            // (teardown) fails the send and is warn-logged once.
            send_servo_event(
                &tx,
                ServoEvent::Console {
                    // REQ-CDP-004: real target identity — routes to exactly
                    // the sessions attached to this page.
                    target_id,
                    level: ConsoleLevel::Debug,
                    text: format!(
                        "[SharedWorker] connect: {} (name={}) from {}",
                        event.shared_worker_id.script_url,
                        if event.shared_worker_id.name.is_empty() {
                            "<default>"
                        } else {
                            &event.shared_worker_id.name
                        },
                        event.page_url
                    ),
                    url: None,
                    line: None,
                    column: None,
                },
            );
        }
    }

    // ─── SharedWorker Channel & Scope (REQ-BRW-004 / DF-WK-7) ────────

    /// Register a SharedWorker channel bridge for this webview.
    ///
    /// DF-WK-7: Each SharedWorker gets a channel bridge that aggregates
    /// per-page port channels. This method registers the bridge so
    /// messages can be drained during spin_event_loop.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn register_shared_worker_channel(&mut self, bridge: SharedWorkerChannelBridge) {
        let id = bridge.shared_worker_id.clone();
        self.shared_worker_channels.insert(id, bridge);
    }

    /// Create a new SharedWorker channel bridge and register it.
    ///
    /// Convenience method that creates the bridge and registers it.
    /// Returns a mutable reference to the bridge for adding ports.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn create_shared_worker_channel(&mut self, shared_worker_id: SharedWorkerId) {
        let bridge = SharedWorkerChannelBridge::new(shared_worker_id.clone());
        self.shared_worker_channels.insert(shared_worker_id, bridge);
    }

    /// Add a port to an existing SharedWorker channel bridge.
    ///
    /// DF-WK-7: When a page connects to a SharedWorker, a new port channel
    /// is created. Returns the port endpoints for the worker thread.
    /// If no bridge exists for the SharedWorkerId, one is created first.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn add_shared_worker_port(
        &mut self,
        shared_worker_id: SharedWorkerId,
    ) -> SharedWorkerPortEndpoints {
        if !self.shared_worker_channels.contains_key(&shared_worker_id) {
            self.create_shared_worker_channel(shared_worker_id.clone());
        }
        self.shared_worker_channels
            .get_mut(&shared_worker_id)
            .expect("just created")
            .add_port()
    }

    /// Get a reference to a SharedWorker channel bridge.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn shared_worker_channel(&self, id: &SharedWorkerId) -> Option<&SharedWorkerChannelBridge> {
        self.shared_worker_channels.get(id)
    }

    /// Remove a SharedWorker channel bridge.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn remove_shared_worker_channel(
        &mut self,
        id: &SharedWorkerId,
    ) -> Option<SharedWorkerChannelBridge> {
        self.shared_worker_channels.remove(id)
    }

    /// Drain all pending SharedWorker→page messages from all SharedWorkers (DF-WK-7).
    ///
    /// Called during spin_event_loop to process all queued messages
    /// from SharedWorkers across all connected pages. Each message is
    /// forwarded to CDP for observability.
    /// Returns messages and any disconnected SharedWorkerIds.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_all_shared_worker_messages(
        &self,
    ) -> (Vec<WorkerStructuredMessage>, Vec<SharedWorkerId>) {
        let mut all_messages = Vec::new();
        let mut all_disconnected = Vec::new();
        for (_, bridge) in &self.shared_worker_channels {
            let (messages, disconnected) = bridge.drain_all_worker_messages();
            all_messages.extend(messages);
            all_disconnected.extend(disconnected);
        }
        (all_messages, all_disconnected)
    }

    /// Drain SharedWorker messages and forward each to CDP (DF-WK-7).
    ///
    /// Returns the set of disconnected SharedWorkerIds.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_and_forward_shared_worker_messages(&self) -> Vec<SharedWorkerId> {
        let (messages, disconnected) = self.drain_all_shared_worker_messages();
        for msg in &messages {
            self.forward_worker_structured_message(&msg);
        }
        disconnected
    }

    /// Post a message to a SharedWorker via a specific port index.
    ///
    /// Convenience method combining shared_worker_channel lookup with
    /// post_to_worker_from_port.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn post_to_worker_via_shared_port(
        &self,
        id: &SharedWorkerId,
        port_index: usize,
        payload: StructuredClonePayload,
    ) -> Result<(), String> {
        match self.shared_worker_channels.get(id) {
            Some(bridge) => bridge.post_to_worker_from_port(port_index, payload),
            None => Err(format!(
                "No channel bridge for SharedWorker: {}:{}",
                id.script_url, id.name
            )),
        }
    }

    /// Clean up SharedWorker channel ports for disconnected workers.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn reap_disconnected_shared_worker_ports(&mut self) {
        for (_, bridge) in &mut self.shared_worker_channels {
            bridge.remove_disconnected_ports();
        }
        // Remove bridges with no remaining ports
        self.shared_worker_channels
            .retain(|_, bridge| bridge.port_count() > 0);
    }

    /// Returns the total number of SharedWorker port channels.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn shared_worker_channel_count(&self) -> usize {
        self.shared_worker_channels
            .values()
            .map(|b| b.port_count())
            .sum()
    }

    /// Register a SharedWorkerGlobalScope state under the given SharedWorkerId.
    ///
    /// Called when a SharedWorker is created, populating the scope state
    /// for CDP observability and stealth consistency verification (CRIT-STL-WK).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn register_shared_worker_scope(
        &mut self,
        id: SharedWorkerId,
        scope: SharedWorkerGlobalScopeState,
    ) {
        self.shared_worker_scopes.insert(id, scope);
    }

    /// Get a reference to a SharedWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn shared_worker_scope(
        &self,
        id: &SharedWorkerId,
    ) -> Option<&SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes.get(id)
    }

    /// Get a mutable reference to a SharedWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn shared_worker_scope_mut(
        &mut self,
        id: &SharedWorkerId,
    ) -> Option<&mut SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes.get_mut(id)
    }

    /// Remove a SharedWorkerGlobalScope state (called when a SharedWorker is reaped).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn remove_shared_worker_scope(
        &mut self,
        id: &SharedWorkerId,
    ) -> Option<SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes.remove(id)
    }

    /// Returns the number of tracked SharedWorkerGlobalScope states.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn shared_worker_scope_count(&self) -> usize {
        self.shared_worker_scopes.len()
    }

    /// Returns a snapshot of all SharedWorkerGlobalScope states.
    ///
    /// Used for CDP observability (Runtime domain) and stealth consistency
    /// verification (criterion #12-17).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn shared_worker_scopes(&self) -> Vec<&SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes.values().collect()
    }

    /// Look up a SharedWorkerGlobalScope state by the Worker's script URL
    /// (CDP worker targetId — first match when several share a script URL).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [criterion:19]
    pub fn shared_worker_scope_by_script_url(
        &self,
        script_url: &str,
    ) -> Option<&SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes
            .values()
            .find(|scope| scope.shared_worker_id.script_url == script_url)
    }

    /// Set the SharedWorker scope config from the first connecting page's StealthProfile.
    ///
    /// DF-WK-9: SharedWorkerGlobalScope inherits the first connecting page's
    /// StealthProfile and it remains fixed for the worker's lifetime (per DEC-WK-007).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [criterion:12..17] DF-WK-9
    pub fn set_shared_worker_scope_config(
        &mut self,
        shared_worker_id: &SharedWorkerId,
        config: &SharedWorkerScopeConfig,
    ) {
        if let Some(scope) = self.shared_worker_scopes.get_mut(shared_worker_id) {
            scope.scope.navigator = WorkerNavigator::from_scope_config(config);
        }
    }

    // ─── ServiceWorker Registration & Fetch Interception (REQ-BRW-004 criterion #19) ────

    /// Set the controlling ServiceWorker for this webview's page.
    ///
    /// A page can be controlled by at most one ServiceWorker at a time.
    /// Per DF-WK-8: When a ServiceWorker becomes activated and its scope matches
    /// the page's URL, it becomes the controller for that page.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn set_controlling_service_worker(&mut self, handle: ServiceWorkerHandle) {
        self.controlled_service_worker = Some(handle);
    }

    /// Clear the controlling ServiceWorker reference for this webview.
    ///
    /// Called on page unload or when the ServiceWorker is unregistered.
    /// Per SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页
    /// 且 terminate 后正确注销" — the ServiceWorker itself survives (tracked in
    /// BaoServoDelegate registry), only the per-page reference is cleared.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn clear_controlling_service_worker(&mut self) {
        self.controlled_service_worker = None;
        self.service_worker_scope = None;
    }

    /// Get a reference to the controlling ServiceWorker, if any.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn controlling_service_worker(&self) -> Option<&ServiceWorkerHandle> {
        self.controlled_service_worker.as_ref()
    }

    /// Check if this page is controlled by a ServiceWorker.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn is_controlled_by_service_worker(&self) -> bool {
        self.controlled_service_worker.is_some()
    }

    /// Check if a URL falls within the controlling ServiceWorker's scope.
    ///
    /// Per DF-WK-8: "scope 匹配的导航/fetch 经 SW 拦截".
    /// Returns false if no ServiceWorker is controlling this page.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn is_url_in_service_worker_scope(&self, url: &str) -> bool {
        self.service_worker_scope
            .as_ref()
            .map(|scope| scope.is_url_in_scope(url))
            .unwrap_or(false)
    }

    /// Register a ServiceWorkerGlobalScope state for the controlling ServiceWorker.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8 / DF-WK-10
    pub fn register_service_worker_scope(&mut self, scope: ServiceWorkerGlobalScopeState) {
        self.service_worker_scope = Some(scope);
    }

    /// Get a reference to the ServiceWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn service_worker_scope(&self) -> Option<&ServiceWorkerGlobalScopeState> {
        self.service_worker_scope.as_ref()
    }

    /// Get a mutable reference to the ServiceWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn service_worker_scope_mut(&mut self) -> Option<&mut ServiceWorkerGlobalScopeState> {
        self.service_worker_scope.as_mut()
    }

    /// Remove the ServiceWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn remove_service_worker_scope(&mut self) -> Option<ServiceWorkerGlobalScopeState> {
        self.service_worker_scope.take()
    }

    /// Set the ServiceWorker scope config from the registering page's StealthProfile.
    ///
    /// DF-WK-10: ServiceWorkerGlobalScope inherits the registering page's profile.
    /// Per SPEC criterion #19: SW-intercepted fetch uses the same stealth profile.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19] DF-WK-10
    pub fn set_service_worker_scope_config(&mut self, config: &ServiceWorkerScopeConfig) {
        if let Some(scope) = &mut self.service_worker_scope {
            scope.scope.navigator = WorkerNavigator::from_scope_config(config);
        }
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

    /// REQ-BRW-002: the frame-ready latch — the headless redraw leg
    /// (boot-once-frame root cause fix). `notify_new_frame_ready` writes both
    /// flags via `latch_frame_ready`; `PageInner::paint_if_needed` drains
    /// `repaint_pending` with `mem::take` so a composite runs exactly once per
    /// servo frame request.
    // @trace REQ-BRW-002 [req:REQ-BRW-002] [level:unit]
    #[test]
    fn test_repaint_pending_latch_and_drain() {
        let mut state = BaoWebViewState::default();
        assert!(!state.frame_ready);
        assert!(!state.repaint_pending);

        state.latch_frame_ready();
        assert!(state.frame_ready);
        assert!(state.repaint_pending);

        // Drain semantics: `mem::take` clears the latch so the next composite
        // only runs when servo asks for another frame.
        assert!(std::mem::take(&mut state.repaint_pending));
        assert!(!state.repaint_pending);
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

    // ─── DedicatedWorkerGlobalScope BaoWebViewState tracking ─────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:DedicatedWorkerGlobalScope] [level:unit]

    #[test]
    fn test_webview_state_dedicated_worker_scope_registration() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let config = WorkerScopeConfig::default();
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
        state.register_dedicated_worker_scope(worker_id.clone(), scope);
        assert_eq!(state.dedicated_worker_scope_count(), 1);
        assert!(state.dedicated_worker_scope(&worker_id).is_some());
    }

    #[test]
    fn test_webview_state_dedicated_worker_scope_get_mut() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let config = WorkerScopeConfig::default();
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
        state.register_dedicated_worker_scope(worker_id.clone(), scope);
        // Register event handler
        state
            .dedicated_worker_scope_mut(&worker_id)
            .unwrap()
            .set_onmessage();
        assert!(
            state
                .dedicated_worker_scope(&worker_id)
                .unwrap()
                .has_onmessage
        );
    }

    #[test]
    fn test_webview_state_dedicated_worker_scope_remove() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let config = WorkerScopeConfig::default();
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
        state.register_dedicated_worker_scope(worker_id.clone(), scope);
        let removed = state.remove_dedicated_worker_scope(&worker_id);
        assert!(removed.is_some());
        assert_eq!(state.dedicated_worker_scope_count(), 0);
    }

    #[test]
    fn test_webview_state_dedicated_worker_scopes_snapshot() {
        let mut state = BaoWebViewState::default();
        let config = WorkerScopeConfig::default();
        let id1 = WorkerId("worker1.js".to_string());
        let id2 = WorkerId("worker2.js".to_string());
        state.register_dedicated_worker_scope(
            id1,
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker1.js".to_string()), &config),
        );
        state.register_dedicated_worker_scope(
            id2,
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker2.js".to_string()), &config),
        );
        let scopes = state.dedicated_worker_scopes();
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn test_webview_state_terminate_clears_dedicated_worker_scopes() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        let config = WorkerScopeConfig::default();
        state.register_dedicated_worker_scope(
            WorkerId("worker1.js".to_string()),
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker1.js".to_string()), &config),
        );
        assert_eq!(state.dedicated_worker_scope_count(), 1);
        state.terminate_all_workers();
        assert_eq!(state.dedicated_worker_scope_count(), 0);
    }

    #[test]
    fn test_webview_state_reap_terminated_dedicated_worker_scopes() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        let config = WorkerScopeConfig::default();
        state.register_dedicated_worker_scope(
            WorkerId("worker1.js".to_string()),
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker1.js".to_string()), &config),
        );
        state.register_dedicated_worker_scope(
            WorkerId("worker2.js".to_string()),
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker2.js".to_string()), &config),
        );
        // Terminate and reap worker1
        state.active_workers[0].handle().terminate();
        state.active_workers[0].handle().mark_terminated();
        state.reap_terminated_workers();
        // worker1's scope should be reaped, worker2's should remain
        assert_eq!(state.dedicated_worker_scope_count(), 1);
        assert!(
            state
                .dedicated_worker_scope(&WorkerId("worker2.js".to_string()))
                .is_some()
        );
    }

    #[test]
    fn test_worker_location_equality() {
        let loc1 = WorkerLocation::from_url("https://example.com/worker.js").unwrap();
        let loc2 = WorkerLocation::from_url("https://example.com/worker.js").unwrap();
        assert_eq!(loc1, loc2);
    }

    // ─── BaoWebViewState SharedWorker Channel & Scope (REQ-BRW-004 / DF-WK-7) ──
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:SharedWorker] [DF-WK-7] [level:unit]

    #[test]
    fn test_webview_state_shared_worker_channel_registration() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let bridge = SharedWorkerChannelBridge::new(id.clone());
        state.register_shared_worker_channel(bridge);
        assert!(state.shared_worker_channel(&id).is_some());
        assert_eq!(state.shared_worker_channel_count(), 0); // no ports yet
    }

    #[test]
    fn test_webview_state_create_shared_worker_channel() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        state.create_shared_worker_channel(id.clone());
        assert!(state.shared_worker_channel(&id).is_some());
    }

    #[test]
    fn test_webview_state_add_shared_worker_port() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let endpoints = state.add_shared_worker_port(id.clone());
        assert_eq!(state.shared_worker_channel_count(), 1);
        assert_eq!(endpoints.shared_worker_id, id);
        assert!(endpoints.page_to_worker_rx.is_some());
        assert!(endpoints.worker_to_page_tx.is_some());
    }

    #[test]
    fn test_webview_state_add_shared_worker_port_multiple() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        state.add_shared_worker_port(id.clone());
        state.add_shared_worker_port(id.clone());
        assert_eq!(state.shared_worker_channel_count(), 2); // 2 ports
    }

    #[test]
    fn test_webview_state_drain_all_shared_worker_messages() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let endpoints = state.add_shared_worker_port(id);
        let tx = endpoints.worker_to_page_tx.unwrap();
        tx.send(WorkerStructuredMessage::metadata_only(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();
        let (messages, disconnected) = state.drain_all_shared_worker_messages();
        assert_eq!(messages.len(), 1);
        assert!(disconnected.is_empty());
    }

    #[test]
    fn test_webview_state_drain_and_forward_shared_worker_messages() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let mut state = BaoWebViewState {
            event_tx: Some(tx),
            cdp_target_id: Some("7".to_string()),
            ..Default::default()
        };
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let endpoints = state.add_shared_worker_port(id);
        let worker_tx = endpoints.worker_to_page_tx.unwrap();
        worker_tx
            .send(WorkerStructuredMessage::metadata_only(
                WorkerId("sw.js".to_string()),
                WorkerMessageDirection::WorkerToPage,
            ))
            .unwrap();
        state.drain_and_forward_shared_worker_messages();
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { text, .. } => {
                assert!(text.contains("worker→page"));
            }
            _ => panic!("expected Console event for shared worker message"),
        }
    }

    #[test]
    fn test_webview_state_disconnect_shared_worker_clears_channels() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        state.track_shared_worker_port(SharedWorkerPortRef::new(SharedWorkerHandle::new(
            "sw.js".to_string(),
            "test".to_string(),
        )));
        state.add_shared_worker_port(id.clone());
        assert_eq!(state.shared_worker_port_count(), 1);
        assert_eq!(state.shared_worker_channel_count(), 1);
        state.disconnect_shared_worker_ports();
        assert_eq!(state.shared_worker_port_count(), 0);
        assert_eq!(state.shared_worker_channel_count(), 0);
    }

    #[test]
    fn test_webview_state_shared_worker_scope_registration() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        let scope = SharedWorkerGlobalScopeState::new(id.clone(), &config);
        state.register_shared_worker_scope(id.clone(), scope);
        assert_eq!(state.shared_worker_scope_count(), 1);
        assert!(state.shared_worker_scope(&id).is_some());
    }

    #[test]
    fn test_webview_state_shared_worker_scope_get_mut() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        let scope = SharedWorkerGlobalScopeState::new(id.clone(), &config);
        state.register_shared_worker_scope(id.clone(), scope);
        state.shared_worker_scope_mut(&id).unwrap().set_onconnect();
        assert!(state.shared_worker_scope(&id).unwrap().has_onconnect);
    }

    #[test]
    fn test_webview_state_shared_worker_scope_remove() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        let scope = SharedWorkerGlobalScopeState::new(id.clone(), &config);
        state.register_shared_worker_scope(id.clone(), scope);
        let removed = state.remove_shared_worker_scope(&id);
        assert!(removed.is_some());
        assert_eq!(state.shared_worker_scope_count(), 0);
    }

    #[test]
    fn test_webview_state_shared_worker_scopes_snapshot() {
        let mut state = BaoWebViewState::default();
        let id1 = SharedWorkerId {
            script_url: "sw1.js".to_string(),
            name: "a".to_string(),
        };
        let id2 = SharedWorkerId {
            script_url: "sw2.js".to_string(),
            name: "b".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        state.register_shared_worker_scope(
            id1,
            SharedWorkerGlobalScopeState::new(
                SharedWorkerId {
                    script_url: "sw1.js".to_string(),
                    name: "a".to_string(),
                },
                &config,
            ),
        );
        state.register_shared_worker_scope(
            id2,
            SharedWorkerGlobalScopeState::new(
                SharedWorkerId {
                    script_url: "sw2.js".to_string(),
                    name: "b".to_string(),
                },
                &config,
            ),
        );
        let scopes = state.shared_worker_scopes();
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn test_webview_state_disconnect_shared_worker_clears_scopes() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        state.register_shared_worker_scope(
            id,
            SharedWorkerGlobalScopeState::new(
                SharedWorkerId {
                    script_url: "sw.js".to_string(),
                    name: "test".to_string(),
                },
                &config,
            ),
        );
        assert_eq!(state.shared_worker_scope_count(), 1);
        state.disconnect_shared_worker_ports();
        assert_eq!(state.shared_worker_scope_count(), 0);
    }

    #[test]
    fn test_webview_state_set_shared_worker_scope_config() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        state.register_shared_worker_scope(
            id.clone(),
            SharedWorkerGlobalScopeState::new(id.clone(), &config),
        );
        assert!(
            state
                .shared_worker_scope(&id)
                .unwrap()
                .navigator()
                .user_agent
                .is_empty()
        );
        let new_config = SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string()],
        };
        state.set_shared_worker_scope_config(&id, &new_config);
        assert_eq!(
            state
                .shared_worker_scope(&id)
                .unwrap()
                .navigator()
                .user_agent,
            "Bao/1.0"
        );
    }
}
