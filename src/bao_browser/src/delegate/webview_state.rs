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
    WebResourceLoad, WebResourceRequest, WebResourceResponse, WebView, WebViewDelegate,
};

use bao_cdp::servo_bridge::main_frame_id_for_target;
use bao_cdp::{BaoEvent, ConsoleMessage};
use bao_cdp_client::bridge::{ConsoleLevel, ServoEvent};

// ─── Fetch 请求拦截设施(REQ-CDP-006,设计 /tmp/cdp-006-interception-design.md)───
//
// 拦截面 = servo 正式 embedder seam:`WebViewDelegate::load_web_resource`
// 覆写(webview_delegate.rs)+ 本文件的拦截槽/pending 注册表/匹配纯函数/
// resolve 入口。接入点选型与能力边界见设计 §2/§7;同线程不变量见
// `pending_intercepted_loads` 字段注释(设计 §9 风险 1)。
//
// pattern/决议类型单源 = `bao_cdp::servo_bridge`(E2 落库的
// `FetchPattern` / `FetchRequestStage` / `FetchResolveAction`;E2 注释明文
// "browser side consumes these types directly")—— 禁平行双源。

use bao_cdp::servo_bridge::{FetchPattern, FetchRequestStage, FetchResolveAction};

/// 进程内单调 interceptionId 序列源(`"interception-{n}"`,设计 §3.2)。
/// 进程级(非 per-page)使多页 id 不碰撞。
static INTERCEPTION_ID_SERIAL: AtomicU64 = AtomicU64::new(0);

/// 进程内单调 paused requestId 序列源(`"cdpreq-{n:x}"`)。
///
/// 边界登记(设计 §4 偏差):vendor `WebResourceRequest` 不携带 net 侧
/// `Request.id`(`request_interceptor.rs:89-97` 构造面只有
/// method/url/headers/destination/referrer/main_frame/redirect),embedder
/// 拿不到 C19-② tap 的同一身份 —— 与 `Network.requestWillBeSent` 的
/// requestId 关联在 P1 不可能,只能 pause 点新鲜 mint。客户端按
/// url+method 关联;真正关联需要 vendor 给 `WebResourceRequest` 加 id
/// 载体(phase-2 vendor seam)。
static PAUSED_REQUEST_ID_SERIAL: AtomicU64 = AtomicU64::new(0);

/// `Fetch.fulfillRequest` 解码后响应体上限(设计 §7/§9:超限显式错误,
/// 不静默截断)。作用于 base64 解码后的字节数。
pub(crate) const FETCH_FULFILL_BODY_LIMIT: usize = 8 * 1024 * 1024;

/// 拦截槽的有效配置(`Fetch.enable` / `Network.setRequestInterception`
/// 落下来的形态)。
/// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
#[derive(Clone, Debug)]
pub struct FetchInterceptionSlot {
    /// 空 = 不拦截任何请求(E2 协议层把「未带 patterns」归一化为一条
    /// catch-all Request 阶段 pattern,显式空列表即字面义)。
    pub patterns: Vec<FetchPattern>,
    /// 命中事件选择旗标(E2 `BridgeCommand::FetchEnable` 的
    /// `legacy_network_events`):true → 命中发 `Network.requestIntercepted`
    /// (criterion 字面命名面);false → `Fetch.requestPaused`。每 hit 只发
    /// 所属启用面的事件,未启用的域不收伪事件 —— 对设计 §5「双事件同点
    /// 发射」的裁定改形(E2 落库形态,本侧跟随)。
    pub legacy_network_events: bool,
}

/// `fetch_resolve` 的结果(调用方映射 CDP 错误码:UnknownId/Rejected → -32602)。
/// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchResolveOutcome {
    /// 命中并已应答(pending 移除)。
    Resolved,
    /// 未命中任何 pending(未知或已决议 id)。
    UnknownId,
    /// 参数越界(fulfill status 越界 / body 解码失败或超限 / 头非法);请求
    /// 保持 paused,客户端可携合法载荷重试(Chromium 同语义:报错不消费
    /// pause)。
    Rejected(String),
}

/// pause 事件载荷(`register_paused_load` 返回,delegate 据此组装
/// 槽旗标选定的那一枚事件)。
pub(crate) struct FetchPausedRequestInfo {
    pub interception_id: String,
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub resource_type: &'static str,
    pub is_redirect: bool,
    /// 命中事件选择(取自启用时的 [`FetchInterceptionSlot::
    /// legacy_network_events`])。
    pub legacy_network_events: bool,
}

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
    /// The single Overlay highlight slot (REQ-CDP-007-C6, design
    /// /tmp/cdp-007-overlay-design.md §3.1): armed by `Overlay.highlightNode`
    /// (each call replaces — one highlight at a time, Chromium semantics),
    /// cleared by `Overlay.hideHighlight` and by every CDP navigation entry
    /// (positional node ids do not survive a document change). Its only
    /// visual carrier is the embedder-side frame annotation
    /// (`cdp_handler::draw_page_highlight`) — nothing here touches the page
    /// DOM or the servo display list (the stealth invariant, design §2.2).
    /// A page close drops the state — and the slot — wholesale.
    /// @trace REQ-CDP-007 [entity:PageHandle] [criterion:6]
    pub highlight: Option<crate::overlay::HighlightState>,
    /// Fetch 请求拦截槽(REQ-CDP-006,设计 /tmp/cdp-006-interception-design.md
    /// §3.1):`None` = InterceptionOff(默认)。`Fetch.enable` /
    /// `Network.setRequestInterception` 写入(后写者胜),`Fetch.disable` 清除。
    /// 槽是 per-target(非 per-session)——两个 CDP session 对同一 target 各自
    /// enable 时后写者胜,事件广播到当前事件通道所有订阅者(设计 §5 文档化偏差)。
    pub(super) fetch_interception: Option<FetchInterceptionSlot>,
    /// 已 pause、未决议的 `WebResourceLoad` 注册表,键 = interceptionId。
    ///
    /// 同线程不变量(设计 §9 风险 1 的文档验证):本字段只被两条泵线程路径
    /// 触及 —— ① `WebViewDelegate::load_web_resource` 回调(servo 以
    /// `Rc<dyn WebViewDelegate>` 持有委托,结构性 `!Send`,回调只发生在持有
    /// WebView 的 embedder/泵线程,`servo_delegate.rs:35-58` 注释明文);
    /// ② BridgeCommand 消费(cdp_handler 的 drain,同在泵线程)。整个
    /// `BaoWebViewState` 经 `Rc<RefCell<_>>` 被这两条路径共享,`Rc` 结构性
    /// 禁止跨线程 —— 平字段即安全,无需内层 `RefCell`/`Mutex`。若未来状态
    /// 被改为 `Arc`/Send 形态,本注册表必须重设计(设计 §9 风险 1),禁静默
    /// 换 `Mutex`(`WebResourceLoad` 的 Send 界未核实)。
    ///
    /// 任何 pending 在 `Fetch.disable` / 页面关闭(整个 state drop)时按
    /// drop=DoNotIntercept 自然续行 —— 设计上不存在页面级楔死通道。
    pub(super) pending_intercepted_loads: HashMap<String, WebResourceLoad>,
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
            highlight: None,
            fetch_interception: None,
            pending_intercepted_loads: HashMap::new(),
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

    // ─── Fetch 请求拦截设施(REQ-CDP-006,设计 §3/§5)────────────────

    /// `Fetch.enable{patterns}` / `Network.setRequestInterception{patterns}`:
    /// 写拦截槽(替换语义,后写者胜 —— 设计 §5 多 session 偏差的登记形态)。
    /// `legacy_network_events` 选定命中事件面(E2 BridgeCommand 落库形态,
    /// 见 [`FetchInterceptionSlot::legacy_network_events`])。
    ///
    /// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
    pub fn fetch_enable(&mut self, patterns: Vec<FetchPattern>, legacy_network_events: bool) {
        log::debug!(
            "[fetch-intercept] enable: {} pattern(s), legacy={} (replaces previous config)",
            patterns.len(),
            legacy_network_events
        );
        self.fetch_interception = Some(FetchInterceptionSlot {
            patterns,
            legacy_network_events,
        });
    }

    /// `Fetch.disable`:清拦截槽 + 排空 pending(每个 drop = DoNotIntercept =
    /// 网络续行)。这是「任何时刻都能解困」的出口(设计 §3.2 不变量)。
    ///
    /// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
    pub fn fetch_disable(&mut self) {
        self.fetch_interception = None;
        self.drain_pending_intercepted_loads("Fetch.disable");
    }

    /// 排空 pending 注册表(每个挂起 load drop → `AutomaticResponder` 发
    /// `DoNotIntercept` → 原始请求照常进网络;fetch 任务不被楔死)。
    pub(crate) fn drain_pending_intercepted_loads(&mut self, reason: &str) {
        if !self.pending_intercepted_loads.is_empty() {
            let n = self.pending_intercepted_loads.len();
            log::debug!(
                "[fetch-intercept] draining {n} paused load(s): {reason} (each drop = DoNotIntercept)"
            );
            self.pending_intercepted_loads.clear();
        }
    }

    /// pause 决策点:该请求在该阶段是否命中拦截(委托回调唯一入口;
    /// pause 点恒为 Request 阶段 —— servo 拦截面在网络请求发出前)。
    ///
    /// 规则:槽 off → false;patterns 空 → false(E2 协议层已把「未带
    /// patterns」归一化为 catch-all pattern,空列表即字面义「不拦截」);
    /// 否则任一 pattern 全维命中。
    ///
    /// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
    pub(crate) fn should_intercept(&self, request: &WebResourceRequest) -> bool {
        self.should_intercept_view(&InterceptRequestView {
            url: &request.url,
            resource_type: resource_type_for_request(request),
        })
    }

    /// [`Self::should_intercept`] 的视图核心(无引擎测试构造面:
    /// `WebResourceRequest` 的 destination 字段类型无法从 bao_browser 命名,
    /// 测试无法直接构造 —— 经投影视图解耦;pause 阶段恒 Request)。
    pub(crate) fn should_intercept_view(&self, view: &InterceptRequestView<'_>) -> bool {
        let Some(slot) = &self.fetch_interception else {
            return false;
        };
        slot.patterns
            .iter()
            .any(|pattern| pattern_matches_view(pattern, view, FetchRequestStage::Request))
    }

    /// 登记一个已命中的 pause:mint 双 id(interceptionId/requestId)、把
    /// load 暂存进 pending 注册表(load 未 intercept,drop 默认 = continue),
    /// 返回事件载荷供 delegate 发射槽旗标选定的那一枚事件。
    ///
    /// 必须先经 [`Self::should_intercept`] 命中后再调用(本方法不做二次匹配;
    /// 无槽调用 = 编序失误,按 legacy=false 兜底取旗标是不可达面)。
    ///
    /// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
    pub(crate) fn register_paused_load(&mut self, load: WebResourceLoad) -> FetchPausedRequestInfo {
        let interception_n = INTERCEPTION_ID_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
        let request_n = PAUSED_REQUEST_ID_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
        let interception_id = format!("interception-{interception_n}");
        let request_id = format!("cdpreq-{request_n:x}");
        let legacy_network_events = self
            .fetch_interception
            .as_ref()
            .map(|slot| slot.legacy_network_events)
            .unwrap_or(false);
        let info = FetchPausedRequestInfo {
            url: load.request.url.to_string(),
            method: load.request.method.as_str().to_string(),
            headers: request_header_pairs(&load.request),
            resource_type: resource_type_for_request(&load.request),
            is_redirect: load.request.is_redirect,
            legacy_network_events,
            interception_id: interception_id.clone(),
            request_id,
        };
        self.pending_intercepted_loads.insert(interception_id, load);
        info
    }

    /// 拦截决议入口(E4 经 `BridgeCommand::FetchResolve` 到达,同泵线程)。
    /// 三向应答按 vendor 真实 API(`webview_delegate.rs:246-337`):
    /// continue = drop(load) → `DoNotIntercept`;fail = intercept+cancel →
    /// `LoadCancelled`;fulfill = intercept+send_body_data+finish。
    ///
    /// 浏览器侧义务(E2 落库契约):fulfill 的 `body` 是客户端原样的
    /// base64 文本 —— 本侧解码(bao `base64` 0.22)并按
    /// [`FETCH_FULFILL_BODY_LIMIT`] 对解码后字节数设上限;`response_headers`
    /// 是 `{name: value}` JSON 对象,本侧展开为 wire 头。
    ///
    /// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
    pub fn fetch_resolve(
        &mut self,
        interception_id: &str,
        action: FetchResolveAction,
    ) -> FetchResolveOutcome {
        if !self.pending_intercepted_loads.contains_key(interception_id) {
            return FetchResolveOutcome::UnknownId;
        }
        // fulfill 载荷先解码/校验后消费:越界报 Rejected 且请求保持 paused
        // (Chromium 同语义:报错不消费 pause;越界路径零副作用)。
        let fulfillment = match &action {
            FetchResolveAction::Continue | FetchResolveAction::Fail { .. } => None,
            FetchResolveAction::Fulfill {
                response_code,
                status_message,
                response_headers,
                body,
            } => match prepare_fulfillment(*response_code, status_message, response_headers, body) {
                Ok(fulfillment) => Some(fulfillment),
                Err(reason) => return FetchResolveOutcome::Rejected(reason),
            },
        };
        let load = self
            .pending_intercepted_loads
            .remove(interception_id)
            .expect("contains_key checked above");
        match action {
            // Continue 路径:drop(load) → AutomaticResponder 发 DoNotIntercept
            // → 原始请求照常进网络(vendor webview_delegate.rs:254-257)。
            FetchResolveAction::Continue => {
                drop(load);
            }
            FetchResolveAction::Fail { error_reason } => {
                log::debug!(
                    "[fetch-intercept] failRequest {interception_id}: errorReason={} \
                     (servo 面无错误码通道,page 侧统一表现为网络错误)",
                    error_reason.as_deref().unwrap_or("<none>")
                );
                // intercept(Start)+ cancel(CancelLoad)→ net 侧
                // `*response = Some(Response::network_error(LoadCancelled))`
                // (vendor request_interceptor.rs:149-152)。
                let url = load.request.url.clone();
                let intercepted = load.intercept(WebResourceResponse::new(url));
                intercepted.cancel();
            }
            FetchResolveAction::Fulfill { .. } => {
                let Fulfillment {
                    status_code,
                    status_message,
                    headers,
                    body,
                } = fulfillment.expect("Fulfill arm ⇒ prepared above");
                let url = load.request.url.clone();
                let mut response = WebResourceResponse::new(url);
                response.status_code = status_code;
                response.status_message = status_message;
                response.headers = headers;
                let mut intercepted = load.intercept(response);
                if !body.is_empty() {
                    intercepted.send_body_data(body);
                }
                intercepted.finish();
            }
        }
        FetchResolveOutcome::Resolved
    }
}

/// 已就绪的合成响应载荷(`prepare_fulfillment` 产出,resolve 的 Fulfill 臂
/// 经字段赋值灌入 `WebResourceResponse` 消费)。`status_code`/`headers`
/// 的字段类型来自 `http` crate —— 无法从 bao_browser 命名(servo 只
/// glob re-export `embedder_traits` 的项,`http` 是其私有依赖),依赖一行
/// `http = "1.4"` 已请示 Commander(主 lock 已有 http 1.4.1 节点,
/// embedder_traits 自己 pin 同版本,零新增编译节点)。
pub(crate) struct Fulfillment {
    pub status_code: http::StatusCode,
    pub status_message: Vec<u8>,
    pub headers: http::HeaderMap,
    pub body: Vec<u8>,
}

/// fulfill 载荷的解码与校验(纯函数):`response_code` 合法域(100..=999,
/// `http::StatusCode` 的可表示域)+ body base64 解码 + 解码后上限
/// ([`FETCH_FULFILL_BODY_LIMIT`])+ 响应头展开(`{name: value}` JSON 对象,
/// 值必须全为字符串)。任一步失败 → Err(显式错误,请求保持 paused)。
///
/// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
pub(crate) fn prepare_fulfillment(
    response_code: i64,
    status_message: &Option<String>,
    response_headers: &Option<serde_json::Value>,
    body_base64: &Option<String>,
) -> Result<Fulfillment, String> {
    if !(100..=999).contains(&response_code) {
        return Err(format!(
            "invalid fulfill status code: {response_code} (expected 100..=999)"
        ));
    }
    let status_code = match http::StatusCode::from_u16(response_code as u16) {
        Ok(code) => code,
        Err(e) => return Err(format!("invalid fulfill status code: {response_code} ({e})")),
    };
    let body = match body_base64 {
        Some(text) => {
            use base64::Engine as _;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(text.as_bytes())
                .map_err(|e| format!("invalid fulfill body (base64): {e}"))?;
            if decoded.len() > FETCH_FULFILL_BODY_LIMIT {
                return Err(format!(
                    "fulfill body {} bytes exceeds the {} MiB limit",
                    decoded.len(),
                    FETCH_FULFILL_BODY_LIMIT / (1024 * 1024)
                ));
            }
            decoded
        }
        None => Vec::new(),
    };
    let mut headers = http::HeaderMap::new();
    if let Some(value) = response_headers {
        let Some(map) = value.as_object() else {
            return Err("fulfill responseHeaders must be a {name: value} object".to_string());
        };
        for (name, header_value) in map {
            let Some(header_value) = header_value.as_str() else {
                return Err(format!("fulfill header {name:?} must be a string value"));
            };
            match (
                http::HeaderName::try_from(name.as_str()),
                http::HeaderValue::from_str(header_value),
            ) {
                (Ok(name), Ok(value)) => {
                    headers.insert(name, value);
                }
                (Err(e), _) => {
                    return Err(format!("invalid fulfill header {name:?}: {e}"));
                }
                (_, Err(e)) => {
                    return Err(format!("invalid fulfill header {name:?}: {e}"));
                }
            }
        }
    }
    Ok(Fulfillment {
        status_code,
        status_message: status_message
            .as_deref()
            .unwrap_or("OK")
            .as_bytes()
            .to_vec(),
        headers,
        body,
    })
}

/// 请求头序列 → `HashMap<String, String>`(wire 形态,lossy-UTF8;与 C19-②
/// tap 的 `bao_header_pairs` 同款语义)。经 `&WebResourceRequest` 间接携带
/// `http::HeaderMap` 字段类型(该类型无法从 bao_browser 命名,见回报偏差 3)。
fn request_header_pairs(request: &WebResourceRequest) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value) in request.headers.iter() {
        map.insert(
            name.as_str().to_string(),
            String::from_utf8_lossy(value.as_bytes()).into_owned(),
        );
    }
    map
}

/// 单请求的资源类型映射(`WebResourceRequest.destination` → CDP ResourceType)。
fn resource_type_for_request(request: &WebResourceRequest) -> &'static str {
    destination_debug_name_to_resource_type(&format!("{:?}", request.destination))
}

/// csp `Destination` 变体名 → CDP ResourceType 字符串(bao 侧独立实现;映射
/// 表语义对齐 vendor `http_loader.rs` 的 `bao_destination_to_resource_type`,
/// 使 `Fetch.requestPaused.resourceType` 与 `Network.requestWillBeSent.type`
/// 输出一致)。
///
/// 以 `derive(Debug)` 变体名为键:fieldless enum 的 Debug 输出恒等于变体
/// 标识符本身(csp 0.9.0 实测),而 `content_security_policy::Destination`
/// 无法从 bao_browser 命名(非本 crate 依赖、全树无 re-export;两文件
/// scope 约束)。未知名 → "Other"(与 vendor 表兜底臂同形)。
fn destination_debug_name_to_resource_type(debug_name: &str) -> &'static str {
    match debug_name {
        "Document" | "Frame" | "IFrame" => "Document",
        "Script" | "ServiceWorker" | "SharedWorker" | "Worker" | "Xslt" => "Script",
        "Style" => "Stylesheet",
        "Image" => "Image",
        "Font" => "Font",
        "Audio" | "Video" | "Track" => "Media",
        "Manifest" => "Manifest",
        "Json" | "Text" => "Fetch",
        _ => "Other",
    }
}

/// 匹配核心的输入视图(从 `WebResourceRequest` 投影;独立类型使纯函数可在
/// 无引擎测试里构造)。
pub(crate) struct InterceptRequestView<'a> {
    pub url: &'a url::Url,
    pub resource_type: &'static str,
}

/// 单条 pattern 对单请求的全维匹配(urlPattern × resourceType × stage)。
/// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
fn pattern_matches_view(
    pattern: &FetchPattern,
    view: &InterceptRequestView<'_>,
    stage: FetchRequestStage,
) -> bool {
    if pattern.request_stage != stage {
        return false;
    }
    if let Some(resource_type) = &pattern.resource_type {
        if resource_type != view.resource_type {
            return false;
        }
    }
    match &pattern.url_pattern {
        Some(url_pattern) => url_pattern_matches(url_pattern, view.url),
        None => true,
    }
}

/// `Fetch.urlPattern` 全串通配匹配(Chromium 真值形态,X.8 裁定③;设计 §9
/// 风险 3 的「协议/主机/路径分段」断言系误传,以 oracle 实测为准勘误):
/// Chromium `fetch_handler.cc` → `DevToolsURLLoaderInterceptor::Pattern` →
/// `base::MatchPattern(url.spec(), pattern)` —— 对 URL 的规范序列化形态
/// (GURL::spec;`url::Url::as_str()` 同值)做**全串** glob,无协议/主机/路径
/// 分段概念。
///
/// 通配语义 = `base::MatchPattern`(base/strings/pattern.cc 实测):
/// - `*` = 零或多个任意字符;
/// - `?` = 恰一个任意字符(CDP 文档只声明 `*`,Chromium 实现把 pattern 中
///   的字面 `?` 也当单字符通配 —— 含 query 的 pattern 在 Chromium 同样有此
///   现象,忠实对齐并在此登记);
/// - `\` = 转义下一字符(`\*`/`\?`/`\\` 为字面);
/// - 连续通配段合读:含 `*` 的段 = 任意长缺口;纯 `?` 段 = 恰好 n 字符的
///   有界缺口。
/// - 大小写敏感(URL 规范形态 scheme/host 已小写化,用户 pattern 按字面写);
/// - 空 pattern = 不匹配任何;整段 `"*"` = 匹配一切。
///
/// @trace REQ-CDP-006 [entity:FetchInterception] [criterion:6]
pub(crate) fn url_pattern_matches(pattern: &str, url: &url::Url) -> bool {
    glob_matches(url.as_str(), pattern)
}

/// `base::MatchPattern` 的 Rust 实现(全串 glob;语义见
/// [`url_pattern_matches`] 注释)。字面段 × 通配段(缺口预算)扫描;无界
/// 段回溯尝试所有断点 —— 拦截 pattern 通常 1-2 个 `*`,最坏指数在实际输入
/// 上不可达(Chromium 自身同为朴素实现,源码留有 TODO)。
fn glob_matches(text: &str, pattern: &str) -> bool {
    match_pattern_tokens(text.as_bytes(), &parse_glob_tokens(pattern), 0)
}

/// 已解析的 pattern 记号:字面段 / 通配段。
#[derive(Debug, PartialEq, Eq)]
enum GlobToken {
    /// 字面字节段(转义已解:背斜杠不进段)。
    Literal(Vec<u8>),
    /// 连续通配段:`min` = 段内 `?` 个数(至少吃 min 字符);`unbounded` =
    /// 段内含 `*`(缺口任意长)。
    WildcardRun { min: usize, unbounded: bool },
}

/// 解析 pattern 为记号序列(`?`/`*` 分段,`\` 转义;孤立尾 `\` 按字面)。
fn parse_glob_tokens(pattern: &str) -> Vec<GlobToken> {
    let bytes = pattern.as_bytes();
    let mut tokens: Vec<GlobToken> = Vec::new();
    let mut literal: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                // 转义:下一字符按字面入段,背斜杠本身不进段。
                literal.push(bytes[i + 1]);
                i += 2;
            }
            b'*' | b'?' => {
                if !literal.is_empty() {
                    tokens.push(GlobToken::Literal(std::mem::take(&mut literal)));
                }
                let (mut min, mut unbounded) = (0usize, false);
                while i < bytes.len() && matches!(bytes[i], b'*' | b'?') {
                    if bytes[i] == b'*' {
                        unbounded = true;
                    } else {
                        min += 1;
                    }
                    i += 1;
                }
                tokens.push(GlobToken::WildcardRun { min, unbounded });
            }
            c => {
                literal.push(c);
                i += 1;
            }
        }
    }
    if !literal.is_empty() {
        tokens.push(GlobToken::Literal(literal));
    }
    tokens
}

/// 记号序列匹配:`ti` 起从 text 匹配 `tokens`,全消费且 text 恰好耗尽为真。
fn match_pattern_tokens(text: &[u8], tokens: &[GlobToken], ti: usize) -> bool {
    match tokens.split_first() {
        None => ti == text.len(),
        Some((GlobToken::WildcardRun { min, unbounded }, rest)) => {
            let after_min = ti + min;
            if after_min > text.len() {
                return false;
            }
            if !*unbounded {
                return match_pattern_tokens(text, rest, after_min);
            }
            // 无界缺口:尝试每个断点(从最少消费起)。
            (after_min..=text.len()).any(|split| match_pattern_tokens(text, rest, split))
        }
        Some((GlobToken::Literal(lit), rest)) => {
            text.len() >= ti + lit.len()
                && &text[ti..ti + lit.len()] == *lit
                && match_pattern_tokens(text, rest, ti + lit.len())
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

    // ─── Overlay highlight slot (REQ-CDP-007-C6) ─────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [criterion:6] [level:unit]

    #[test]
    fn test_webview_state_highlight_slot_roundtrip() {
        // Design §3.1: no highlight by default; highlightNode arms (replace),
        // hideHighlight clears.
        let mut state = BaoWebViewState::default();
        assert!(state.highlight.is_none(), "no highlight by default");
        state.highlight = Some(crate::overlay::HighlightState {
            node: crate::overlay::OverlayNodeRef::NodeId(42),
            spec: crate::overlay::HighlightSpec::default(),
        });
        assert_eq!(
            state.highlight.as_ref().unwrap().node,
            crate::overlay::OverlayNodeRef::NodeId(42)
        );
        // Replacement semantics: the single slot holds the newest highlight.
        state.highlight = Some(crate::overlay::HighlightState {
            node: crate::overlay::OverlayNodeRef::ObjectId("obj-7".to_string()),
            spec: crate::overlay::HighlightSpec::default(),
        });
        assert_eq!(
            state.highlight.as_ref().unwrap().node,
            crate::overlay::OverlayNodeRef::ObjectId("obj-7".to_string())
        );
        state.highlight = None;
        assert!(state.highlight.is_none());
    }

    // ─── Fetch 请求拦截设施:pattern 矩阵(REQ-CDP-006,设计 §3.1)────
    // @trace REQ-CDP-006 [req:REQ-CDP-006] [entity:FetchInterception]
    // [criterion:6] [level:unit]
    //
    // 纯函数矩阵(无引擎):urlPattern 通配 × resourceType × stage。
    // `WebResourceRequest` 的 destination 字段类型无法从 bao_browser 命名,
    // 测试经 `InterceptRequestView` 视图核心构造。pattern/阶段类型单源 =
    // bao_cdp::servo_bridge(E2 落库形态)。

    use bao_cdp::servo_bridge::{FetchRequestStage, FetchResolveAction};

    fn parse_url(s: &str) -> url::Url {
        url::Url::parse(s).expect("test url")
    }

    fn make_pattern(
        url_pattern: Option<&str>,
        resource_type: Option<&str>,
        stage: FetchRequestStage,
    ) -> FetchPattern {
        FetchPattern {
            url_pattern: url_pattern.map(str::to_string),
            resource_type: resource_type.map(str::to_string),
            request_stage: stage,
        }
    }

    #[test]
    fn test_url_pattern_wildcard_matrix_whole_string() {
        // X.8 裁定③:Chromium 真值 = base::MatchPattern 全串 glob over
        // URL 规范形态(无分段);矩阵按全串语义编码。
        let u = |s: &str| url::Url::parse(s).expect("test url");

        // 整段 "*" = 匹配一切;空 pattern = 不匹配任何。
        assert!(url_pattern_matches("*", &u("https://example.com/a?b=1")));
        assert!(!url_pattern_matches("", &u("https://example.com/")));

        // 字面全串:规范形态尾随 `/` 参与逐字对照。
        assert!(url_pattern_matches(
            "https://example.com/",
            &u("https://example.com/")
        ));
        // 无尾斜杠 pattern 不命中规范形态(spec 恒带 `/`)。
        assert!(!url_pattern_matches(
            "https://example.com",
            &u("https://example.com/")
        ));

        // 尾部 `*`:前缀匹配(scheme/host/path 全字面)。
        assert!(url_pattern_matches(
            "https://example.com/*",
            &u("https://example.com/")
        ));
        assert!(url_pattern_matches(
            "https://example.com/*",
            &u("https://example.com/a/b")
        ));
        assert!(url_pattern_matches(
            "https://example.com/*",
            &u("https://example.com/a?x=1")
        ));
        assert!(!url_pattern_matches(
            "https://example.com/*",
            &u("http://example.com/")
        ));
        assert!(!url_pattern_matches(
            "https://example.com/*",
            &u("https://other.com/")
        ));
        // 带显式端口:前缀字面失配(`.com/` vs `.com:`)→ 不命中。
        assert!(!url_pattern_matches(
            "https://example.com/*",
            &u("https://example.com:8080/x")
        ));

        // 精确 path:query 使 spec 变长 → 不命中。
        assert!(url_pattern_matches(
            "https://example.com/api",
            &u("https://example.com/api")
        ));
        assert!(!url_pattern_matches(
            "https://example.com/api",
            &u("https://example.com/api?x=1")
        ));
        assert!(!url_pattern_matches(
            "https://example.com/api",
            &u("https://example.com/api/extra")
        ));

        // 前缀通配(`*` 跨 `/` 与 `?`)。
        assert!(url_pattern_matches(
            "https://example.com/api*",
            &u("https://example.com/api")
        ));
        assert!(url_pattern_matches(
            "https://example.com/api*",
            &u("https://example.com/api/x")
        ));
        assert!(url_pattern_matches(
            "https://example.com/api*",
            &u("https://example.com/api?x=1")
        ));
        assert!(url_pattern_matches(
            "https://example.com/api*",
            &u("https://example.com/apifoo")
        ));

        // scheme 通配(`*://`);无前导通配的 pattern 按字面前缀锚定 ——
        // `example.com/*` 不命中带 scheme 的 spec(Chromium 同此;跨 scheme
        // 匹配需写 `*example.com/*`)。
        assert!(url_pattern_matches(
            "*://example.com/*",
            &u("http://example.com/")
        ));
        assert!(url_pattern_matches(
            "*://example.com/*",
            &u("https://example.com/")
        ));
        assert!(!url_pattern_matches(
            "example.com/*",
            &u("http://example.com/x")
        ));
        assert!(url_pattern_matches(
            "*example.com/*",
            &u("http://example.com/x")
        ));
        // 边界登记(Chromium 同现象):全串 glob 无 dot 边界 ——
        // `*example.com/*` 同样命中 `notexample.com`(`example.com` 是其
        // 字面子串)。host 后缀级约束在 Chromium glob 语义下不可表达。
        assert!(url_pattern_matches(
            "*example.com/*",
            &u("https://notexample.com/")
        ));

        // 跨段自由 glob(全串语义的自然能力)。
        assert!(url_pattern_matches(
            "https://x.com/a*b",
            &u("https://x.com/a/x/b")
        ));
        assert!(url_pattern_matches(
            "*example.com/*",
            &u("https://a.example.com/")
        ));
        assert!(url_pattern_matches(
            "https://example.com/*token=abc",
            &u("https://example.com/api?token=abc")
        ));

        // 大小写敏感(spec 规范形态已小写;大写 pattern 字面失配)。
        assert!(!url_pattern_matches(
            "https://EXAMPLE.com/*",
            &u("https://example.com/")
        ));
        assert!(!url_pattern_matches(
            "https://example.com/API",
            &u("https://example.com/api")
        ));

        // `?` = 恰一字符通配(base::MatchPattern 语义;Chromium 同此,
        // pattern 含字面 query 同样有此现象,注释登记)。
        assert!(url_pattern_matches(
            "https://example.com/api?x=1",
            &u("https://example.com/apiXx=1")
        ));
    }

    #[test]
    fn test_glob_matches_semantics() {
        // base::MatchPattern 语义:`*` 零或多、`?` 恰一、`\` 转义、
        // 连续通配段合读(含 `*` 无界;纯 `?` 恰 n 字符)。
        assert!(glob_matches("", ""));
        assert!(glob_matches("", "*"));
        assert!(glob_matches("abc", "abc"));
        assert!(glob_matches("abc", "*"));
        assert!(glob_matches("abc", "a*"));
        assert!(glob_matches("abc", "*c"));
        assert!(glob_matches("abc", "*b*"));
        assert!(glob_matches("a/bc", "a*c"));
        assert!(glob_matches("a/b", "*/b"));
        assert!(glob_matches("abc", "a*b*c"));
        assert!(!glob_matches("abc", ""));
        assert!(!glob_matches("abc", "abd"));
        assert!(!glob_matches("abc", "*b"));
        // `?` = 恰一字符。
        assert!(glob_matches("abc", "a?c"));
        assert!(glob_matches("abc", "???"));
        assert!(!glob_matches("ab", "a?c"));
        assert!(!glob_matches("abcd", "a?c"));
        // 通配段合读:纯 `?` 段 = 有界(恰 n);含 `*` = 无界。
        assert!(glob_matches("ab", "??"));
        assert!(!glob_matches("a", "??"));
        assert!(!glob_matches("abc", "??"));
        assert!(glob_matches("anything", "?*"));
        // `\` 转义:`\*`/`\?`/`\\` 为字面。
        assert!(glob_matches("a*b", "a\\*b"));
        assert!(!glob_matches("axb", "a\\*b"));
        assert!(glob_matches("a?b", "a\\?b"));
        assert!(glob_matches("a\\b", "a\\\\b"));
        // 孤立尾背斜杠按字面。
        assert!(glob_matches("abc\\", "abc\\"));
        assert!(!glob_matches("abc", "abc\\"));
    }

    #[test]
    fn test_pattern_stage_filter_matrix() {
        let script_url = parse_url("https://example.com/app.js");
        let script_view = InterceptRequestView {
            url: &script_url,
            resource_type: "Script",
        };
        let image_url = parse_url("https://example.com/a.png");
        let image_view = InterceptRequestView {
            url: &image_url,
            resource_type: "Image",
        };

        // Request 阶段 pattern 只在 Request 命中;Response 反之。
        let request_pattern = make_pattern(
            Some("https://example.com/*"),
            None,
            FetchRequestStage::Request,
        );
        let response_pattern = make_pattern(
            Some("https://example.com/*"),
            None,
            FetchRequestStage::Response,
        );
        assert!(pattern_matches_view(
            &request_pattern,
            &script_view,
            FetchRequestStage::Request
        ));
        assert!(!pattern_matches_view(
            &request_pattern,
            &script_view,
            FetchRequestStage::Response
        ));
        assert!(pattern_matches_view(
            &response_pattern,
            &script_view,
            FetchRequestStage::Response
        ));
        assert!(!pattern_matches_view(
            &response_pattern,
            &script_view,
            FetchRequestStage::Request
        ));

        // resourceType 过滤。
        let script_pattern = make_pattern(None, Some("Script"), FetchRequestStage::Request);
        assert!(pattern_matches_view(
            &script_pattern,
            &script_view,
            FetchRequestStage::Request
        ));
        assert!(!pattern_matches_view(
            &script_pattern,
            &image_view,
            FetchRequestStage::Request
        ));

        // urlPattern 过滤。
        let path_pattern = make_pattern(
            Some("https://example.com/*.js"),
            None,
            FetchRequestStage::Request,
        );
        assert!(pattern_matches_view(
            &path_pattern,
            &script_view,
            FetchRequestStage::Request
        ));
        assert!(!pattern_matches_view(
            &path_pattern,
            &image_view,
            FetchRequestStage::Request
        ));

        // 三维 AND 组合:翻任何一维即不命中。
        let combined = make_pattern(
            Some("https://example.com/*"),
            Some("Script"),
            FetchRequestStage::Request,
        );
        assert!(pattern_matches_view(
            &combined,
            &script_view,
            FetchRequestStage::Request
        ));
        assert!(!pattern_matches_view(
            &combined,
            &image_view,
            FetchRequestStage::Request
        ));
        assert!(!pattern_matches_view(
            &combined,
            &script_view,
            FetchRequestStage::Response
        ));

        // urlPattern 缺省(None)= URL 维不过滤。
        let no_url = make_pattern(None, None, FetchRequestStage::Request);
        assert!(pattern_matches_view(
            &no_url,
            &image_view,
            FetchRequestStage::Request
        ));
    }

    #[test]
    fn test_interception_state_machine() {
        let mut state = BaoWebViewState::default();
        let script_url = parse_url("https://example.com/app.js");
        let script_view = InterceptRequestView {
            url: &script_url,
            resource_type: "Script",
        };

        // 默认 InterceptionOff:恒不命中(= 不覆写时今天的行为)。
        assert!(!state.should_intercept_view(&script_view));

        // enable + 空 patterns = 字面义「不拦截」(E2 协议层把「未带
        // patterns」归一化为 catch-all pattern;显式空列表即空)。
        state.fetch_enable(Vec::new(), false);
        assert!(!state.should_intercept_view(&script_view));

        // catch-all pattern(urlPattern/resourceType 全 None)= 全命中。
        state.fetch_enable(
            vec![make_pattern(None, None, FetchRequestStage::Request)],
            false,
        );
        assert!(state.should_intercept_view(&script_view));

        // Response 阶段 pattern 在 Request pause 点永不命中(P1 边界)。
        state.fetch_enable(
            vec![make_pattern(
                Some("*.example.com/*"),
                None,
                FetchRequestStage::Response,
            )],
            false,
        );
        assert!(!state.should_intercept_view(&script_view));

        // 多 pattern 任一命中(全串 glob 形态:跨 scheme 需前导 `*`,
        // `*.example.com/*` 不命中裸域 —— 见 url_pattern_matches 注释)。
        state.fetch_enable(
            vec![
                make_pattern(
                    Some("https://other.com/*"),
                    None,
                    FetchRequestStage::Request,
                ),
                make_pattern(Some("*example.com/*"), None, FetchRequestStage::Request),
            ],
            false,
        );
        assert!(state.should_intercept_view(&script_view));

        // disable:回到 off。
        state.fetch_disable();
        assert!(!state.should_intercept_view(&script_view));
    }

    #[test]
    fn test_fulfill_payload_validation_matrix() {
        // status 合法域 100..=999(E2 传 i64,越域显式错误)。
        assert!(prepare_fulfillment_errors(200, None, None, None).is_none());
        assert!(prepare_fulfillment_errors(100, None, None, None).is_none());
        assert!(prepare_fulfillment_errors(999, None, None, None).is_none());
        assert!(prepare_fulfillment_errors(99, None, None, None).is_some());
        assert!(prepare_fulfillment_errors(1000, None, None, None).is_some());
        assert!(prepare_fulfillment_errors(0, None, None, None).is_some());
        assert!(prepare_fulfillment_errors(60_000, None, None, None).is_some());

        // body:合法 base64 / 非法 base64 / 解码后超限。
        assert!(prepare_fulfillment_errors(200, None, None, Some("aGVsbG8=")).is_none());
        assert!(prepare_fulfillment_errors(200, None, None, Some("!not-base64!")).is_some());
        let big = "A".repeat(FETCH_FULFILL_BODY_LIMIT * 4 / 3 + 16);
        assert!(prepare_fulfillment_errors(200, None, None, Some(big.as_str())).is_some());

        // 响应头:对象形态全字符串值;非对象/非字符串值显式错误。
        let headers = serde_json::json!({"content-type": "text/html"});
        assert!(prepare_fulfillment_errors(200, None, Some(&headers), None).is_none());
        let bad_headers = serde_json::json!({"x-num": 7});
        assert!(prepare_fulfillment_errors(200, None, Some(&bad_headers), None).is_some());
        assert!(prepare_fulfillment_errors(200, None, Some(&serde_json::json!([])), None).is_some());
    }

    /// `prepare_fulfillment` 的 Err 投影(status_code 恒 200,聚焦载荷校验)。
    fn prepare_fulfillment_errors(
        response_code: i64,
        status_message: Option<&str>,
        response_headers: Option<&serde_json::Value>,
        body: Option<&str>,
    ) -> Option<String> {
        prepare_fulfillment(
            response_code,
            &status_message.map(str::to_string),
            &response_headers.cloned(),
            &body.map(str::to_string),
        )
        .err()
    }

    #[test]
    fn test_fetch_resolve_unknown_id() {
        let mut state = BaoWebViewState::default();
        assert_eq!(
            state.fetch_resolve("interception-1", FetchResolveAction::Continue),
            FetchResolveOutcome::UnknownId
        );
        // 未知 id 优先于载荷校验(存在性先查)。
        assert_eq!(
            state.fetch_resolve(
                "interception-1",
                FetchResolveAction::Fulfill {
                    response_code: 60_000,
                    status_message: None,
                    response_headers: None,
                    body: None,
                }
            ),
            FetchResolveOutcome::UnknownId
        );
    }
}
