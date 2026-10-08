// @trace REQ-BRW-001 [entity:BrowserContext] [entity:PageHandle]
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope]
// @trace REQ-BRW-4 [entity:Worker] [entity:SharedWorker] [entity:ServiceWorker]
// @trace REQ-CLI-002
#![allow(dead_code, unused_imports)]
// REQ-BRW-001: Browser engine integration with servo
// REQ-BRW-004: Worker constructor bridging to Page Realm (DF-WK-11)
// REQ-BRW-4: Worker/SharedWorker/ServiceWorker constructors on JS global object
// REQ-CLI-002: bao browser 子命令 → servo 初始化 + CDP 端口输出
// REQ-LIB-004: BrowserRuntime top-level coordinator
mod cdp_handler;
pub mod cdp_memory;
mod config;
mod delegate;
mod error;
mod page;
mod page_pool;
mod phase_watch;
mod permission;
mod runtime_bridge;
mod screenshot;
pub mod screencast;
#[cfg(feature = "webdriver")]
pub mod webdriver_host;
mod ws_registry;

pub use config::{BaoConfig, BrowserConfig, PageConfig};
// Bridge-command handler — the servo-side executor that drains
// BridgeCommand during the event loop (run_with_bridge's per-command entry).
// Public for e2e tests that drive the same loop shape as run_browser.
pub use cdp_handler::handle_bridge_command;
pub use delegate::{
    crash_safe_teardown_worker, is_javascript_mime_type, servo_event_emitted_total,
    AutoCloseWorker, BaoServoDelegate, BaoWebViewDelegate, BaoWebViewState,
    DedicatedWorkerGlobalScopeState, ServiceWorkerFetchInterceptMode,
    ServiceWorkerGlobalScopeState, ServiceWorkerHandle, ServiceWorkerRegistrationId,
    ServiceWorkerRegistrationState, ServiceWorkerRegistrationTracking, ServiceWorkerScopeConfig,
    SharedWorkerChannelBridge, SharedWorkerConnectEvent, SharedWorkerGlobalScopeState,
    SharedWorkerHandle, SharedWorkerId, SharedWorkerPortChannel, SharedWorkerPortEndpoints,
    SharedWorkerPortRef, SharedWorkerScopeConfig, StructuredClonePayload, WorkerChannelBridge,
    WorkerChannelEndpoints, WorkerErrorEvent, WorkerGlobalScopeState, WorkerHandle, WorkerId,
    WorkerLifecycleState, WorkerLocation, WorkerMessageDirection, WorkerMessageEvent,
    WorkerNavigator, WorkerNetworkInformation, WorkerScopeConfig, WorkerScriptLoadError,
    WorkerScriptLoadResult, WorkerScriptLoadState, WorkerScriptLoader, WorkerScriptSource,
    WorkerScriptType, WorkerStructuredMessage, WorkerTeardownPath, WorkerTeardownResult,
};
pub use error::BrowserError;
pub use page::{PageHandle, PageState};
pub use page_pool::PagePool;
pub use permission::{Permission, PermissionDenied, PermissionGuard};
pub use runtime_bridge::{
    register_worker_scope_callback_native, BridgeChannel, BridgeCommand, BridgeReceiver,
    BridgeResponse, EvaluateResult, RuntimeBridge, WorkerScopeInitFn,
};
pub use screenshot::{encode_image, ScreenshotFormat};
pub use ws_registry::BaoWsRegistry;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use servo::{Opts, Preferences, Servo, ServoBuilder};

use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::bridge_channel;
use bao_cdp_client::bridge::{translate, ServoEvent};
use cdp_server::{CdpServer, EventBroadcaster, EventSender, ServerConfig};

// BAO PATCH (BCE-20260627-009): Process-global servo opts initialization.
// servo's `opts::initialize_options` uses an `OnceLock<Opts>` that panics on re-init,
// and `opts::get()` lazily fills it with `Default`. If ANY servo code calls `get()`
// before our explicit `initialize_options`, the OnceLock locks to Default and bao's
// config (force_isolate_event_loops=true) can never win.
//
// `BAO_SERVO_OPTS_INIT` is a `LazyLock` that runs `initialize_options` with bao's
// config on first access. `BrowserRuntime::new` forces it (`.clone()` triggers init)
// BEFORE constructing `Servo`, winning the OnceLock race process-wide. Multi-instance
// safety: subsequent `BrowserRuntime::new` calls hit the idempotent path in the patched
// `initialize_options` (same bao config → no-op).
static BAO_SERVO_OPTS_INIT: std::sync::LazyLock<()> = std::sync::LazyLock::new(|| {
    servo::opts::initialize_options(Opts {
        force_isolate_event_loops: true,
        disable_script_debugger: true,
        ..Opts::default()
    });
});

// ─── ServoEvent real 路径投递探针(REQ-CDP-004) ─────────────────────
//
// consumer 腿计数:泵在 `run_with_bridge` drain 循环里每消费一个 ServoEvent
// 计一。producer 腿是 delegate.rs 的 `servo_event_emitted_total()`;可靠队列
// 语义下 drain 通过后两腿相等。suite 级 delivery 断言消费(nextest 每 test
// 独立进程,进程内取 delta 即可)。

/// 泵消费腿累计数。
static SERVO_EVENT_PUMPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// real 路径投递探针(consumer 腿)累计数。suite 级 delivery 断言消费。
pub fn servo_event_pumped_total() -> u64 {
    use std::sync::atomic::Ordering;
    SERVO_EVENT_PUMPED.load(Ordering::Relaxed)
}

/// Drain every currently-queued ServoEvent from the real event queue,
/// invoking `handle` per event. This is THE pump-consumption face — the
/// delivery probe (`servo_event_pumped_total`) counts here, so every drain
/// site built on this helper participates in the emitted==pumped assertion
/// (`run_with_bridge` and the suite pump-shape harnesses alike).
///
/// @trace REQ-CDP-004 [req:REQ-CDP-004] [level:library]
pub fn drain_servo_events(
    servo_event_rx: &std::sync::mpsc::Receiver<ServoEvent>,
    mut handle: impl FnMut(ServoEvent),
) {
    while let Ok(servo_event) = servo_event_rx.try_recv() {
        SERVO_EVENT_PUMPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        handle(servo_event);
    }
}

/// Deprecated alias for the browser coordinator runtime (0.x transition;
/// removed in 1.0).
#[deprecated(since = "0.4.0", note = "renamed to `BrowserRuntime`; will be removed in 1.0")]
pub type BaoRuntime = BrowserRuntime;

pub struct BrowserRuntime {
    servo: Rc<Servo>,
    delegate: Rc<BaoServoDelegate>,
    page_pool: Rc<PagePool>,
    cdp_port: Option<u16>,
    /// Receiver side of the memory:// CDP bridge installed into
    /// `bao_cdp_client`'s process registry — `run()` drains it so
    /// servo-touching in-process CDP commands get real execution.
    cdp_bridge: Option<std::sync::Arc<cdp_memory::MemoryCdpBridge>>,
    cdp_bridge_rx: Option<bao_cdp::servo_bridge::BridgeReceiver>,
    /// Generation token for registry teardown (Drop clears only if this
    /// runtime's bridge is still the installed one).
    cdp_bridge_token: Option<usize>,
    /// WPT official-toolchain face (REQ-BRW-002): when set, the run loops
    /// drain the WebDriver embedder channel in addition to the servo loop.
    /// Installed by `run_browser` before the loop starts.
    #[cfg(feature = "webdriver")]
    webdriver_host: std::cell::RefCell<Option<std::sync::Arc<webdriver_host::WebDriverHost>>>,
    /// Cooperative exit flag — WebDriver `Shutdown` (and only that) sets it;
    /// every run loop checks it each iteration.
    exit_scheduled: std::cell::Cell<bool>,
}

impl BrowserRuntime {
    pub fn new(config: BaoConfig) -> Result<Self, BrowserError> {
        config.validate().map_err(BrowserError::Init)?;

        // Force-init servo's process-global OnceLock<Opts> BEFORE any servo code
        // calls get() (which would lazily lock in Default). This wins the race
        // against servo's get_or_init(Default::default).
        //
        // Config-derived opts go in FIRST (initialize_options is
        // first-writer-wins): the static LazyLock below only exists to win the
        // same race for code paths that never construct a BrowserRuntime, so once
        // this call runs it is a no-op. `ignore_certificate_errors` is the one
        // field tests set non-default (WPT posture against local self-signed
        // TLS fixtures, e.g. the U2 h2 e2e matrix).
        servo::opts::initialize_options(Opts {
            force_isolate_event_loops: true,
            disable_script_debugger: true,
            ignore_certificate_errors: config.ignore_certificate_errors,
            certificate_path: config.certificate_path.clone(),
            ..Opts::default()
        });
        std::sync::LazyLock::force(&BAO_SERVO_OPTS_INIT);

        // BUG-ENG-366: `force_isolate_event_loops` only governs servo's event-loop
        // multiplexing (per-pipeline ScriptThread vs shared). It does NOT control
        // SpiderMonkey Compartment isolation — every page always gets its own
        // Window global in a distinct Compartment (servo DOM invariant), and the
        // Node Realm is created via NewCompartmentAndZone unconditionally
        // (runtime_bridge::create_node_realm_native). Stealth noise is keyed
        // per-Realm via bao_stealth::engine_props::set_profile_for_global, so
        // even with `force_isolate_event_loops: false` each page's Canvas /
        // Navigator / WebGL / Audio fingerprints remain isolated. The flag is
        // kept `true` here purely to bound servo's resource use (one ScriptThread
        // per page) — disabling it does not regress isolation.
        // @trace REQ-SEC-002 [req:REQ-SEC-002] [req:BUG-ENG-366]
        //
        // BAO PATCH (BCE-20260627-009): Idempotent servo config init.
        // servo's `opts::initialize_options` uses a process-global `OnceLock<Opts>`;
        // re-initializing panics. Each `BrowserRuntime::new` → `Servo::new` →
        // `initialize_options`. Multiple BrowserRuntime instances (production
        // multi-tenant + concurrent integration tests) therefore collide.
        // Strategy:
        //   (1) Detect whether servo config is already initialized by reading
        //       `servo::opts::get()` (returns `&'static Opts`, never panics —
        //       falls back to `Default` via `get_or_init`).
        //   (2) `force_isolate_event_loops` is `false` in `Opts::default` but
        //       `true` in our desired config, so it is a reliable sentinel for
        //       "already initialized by a prior BrowserRuntime".
        //   (3) On the already-initialized path we skip `.opts(...)` — but
        //       `Servo::new` still calls `initialize_options` internally, so the
        //       vendor-side patch (idempotent `initialize_options`) is the real
        //       guarantee. This bao-layer check just avoids passing conflicting
        //       opts when we know a prior instance already configured servo.
        let desired_opts = Opts {
            force_isolate_event_loops: true,
            ignore_certificate_errors: config.ignore_certificate_errors,
            // BAO PATCH (BCE-20260621-002): Skip servo's
            // `JS::Debugger::addDebuggee` path entirely. Bao embeds
            // servo but uses `bao_cdp` (its own CDP) and never connects
            // to servo's devtools server, so the servo Debugger is pure
            // overhead and a SIGSEGV source: `fire_add_debuggee` marks
            // every page's Realm as a debuggee
            // (`Realm::setIsDebuggee`), which toggles
            // BaselineInterpreter debugger instrumentation. Under bao's
            // multi-page + navigate + later-`evaluate` workload, a
            // subsequent JIT OSR dereferences
            // `cx->activation_->prev()->asInterpreter()` as NULL and
            // SIGSEGVs deterministically. Setting this flag bypasses
            // `fire_add_debuggee` (gated upstream in
            // `script_thread.rs`), so `setIsDebuggee` is never called
            // and the JIT toggle never happens. Servo's default `false`
            // keeps devtools working for normal servo embedders.
            disable_script_debugger: true,
            ..Opts::default()
        };
        // `opts::get()` is `get_or_init(Default::default)`: returns the
        // process-wide config if already set, otherwise `Default` (where
        // `force_isolate_event_loops == false`). Our config sets it `true`,
        // so observing `true` here means a prior BrowserRuntime already won.
        let servo_already_initialized = servo::opts::is_initialized();

        // Pref override surface (bao is the embedder; vendor defaults stay
        // untouched). `Servo::new` ends with
        // `prefs::set(preferences.unwrap_or_default())` — passing NO builder
        // preferences resets every pref to `Preferences::default()`. So the
        // only durable injection point is `ServoBuilder::preferences`, on
        // BOTH branches (the already-initialized branch would otherwise wipe
        // the flip below on the next BrowserRuntime). Both branches start from
        // `Preferences::default()` — exactly what `Servo::new` would install
        // without a builder override — so the only delta is the flip.
        //
        // `dom_indexeddb_enabled` defaults to false upstream (experimental),
        // but bao is a full browser runtime and servo ships a real IDB
        // implementation; `GlobalScope::obtain_storage_key` reads this pref
        // at runtime per IDB open, so every page (and worker) scope gets it.
        let mut preferences = Preferences::default();
        preferences.dom_indexeddb_enabled = true;
        // `dom_offscreen_canvas_enabled` defaults to false upstream
        // (experimental); the vendor OffscreenCanvas implementation
        // (`script/dom/canvas/offscreencanvas.rs`) is complete
        // (Constructor/getContext/transferToImageBitmap/convertToBlob,
        // `Exposed=(Window,Worker)`), so the pref is the only gate.
        // REQ-BRW-004 C13 + user ruling 2026-09-09: stealth pages need a
        // worker-realm canvas surface (CreepJS/fp-collect probe OffscreenCanvas
        // inside Workers; `undefined` there is itself a fingerprint signal).
        preferences.dom_offscreen_canvas_enabled = true;
        // `dom_serviceworker_enabled` defaults to false upstream; the vendor SW
        // implementation is real (`ServiceWorkerContainer.register` → job →
        // `ServiceWorkerGlobalScope::run_serviceworker_scope`). The pref gates
        // `navigator.serviceWorker`, the container interface and the
        // ServiceWorkerGlobalScope global (webidl `Pref=`), so without the flip
        // pages see no SW surface at all and the SW scope's embedder drain
        // (REQ-BRW-004 S1, DF-WK-10 stealth inheritance) can never fire.
        // REQ-BRW-004 C19 prerequisite + user ruling 2026-09-09.
        preferences.dom_serviceworker_enabled = true;
        // `dom_webgl2_enabled` defaults to false upstream; the vendor WebGL2
        // implementation (`script/dom/webgl/webgl2renderingcontext.rs`) is real
        // and the pref is the first gate in
        // `OffscreenCanvas::get_or_init_webgl2_context` (via
        // `WebGL2RenderingContext::is_webgl2_enabled`), before any channel
        // dispatch — without the flip `getContext('webgl2')` returns null in
        // BOTH realms. Real browsers ship WebGL2 on, so a missing webgl2
        // surface is itself a fingerprint signal. REQ-BRW-004 C14 + user
        // ruling 2026-09-09.
        preferences.dom_webgl2_enabled = true;
        // `dom_sharedworker_enabled` defaults to false in the bao vendor
        // snapshot (config/prefs.rs — the upstream default at the vendor
        // baseline is true; this false is a bao-local single-line change), so
        // the generated `SharedWorkerBinding::ConstructorEnabled` gate keeps
        // the `SharedWorker` interface object off the Window global and every
        // `new SharedWorker()` dies with a ReferenceError (e81 profile: L2
        // 127-cell variant tax). The vendor implementation is real
        // (`SharedWorker::Constructor` → registry →
        // `SharedWorkerGlobalScope::run_shared_worker_scope`, upstream
        // 9fc8f7389), so the pref is the only gate. Flip here (bao is the
        // embedder; vendor defaults stay untouched) — same four-flip precedent
        // as IDB / OffscreenCanvas / SW / WebGL2 above.
        // REQ-BRW-004 third worker scope + user ruling: the SharedWorker
        // scope's embedder drain (REQ-BRW-004 per-Worker injector tier, third
        // scope) lands in the same wave — constitution A: an enabled-but-bare
        // realm is itself a detection vector, so enablement and drain ship
        // together.
        preferences.dom_sharedworker_enabled = true;
        // `dom_intersection_observer_enabled` defaults to false upstream
        // (config/prefs.rs:454); the vendor IntersectionObserver
        // implementation is real (`script/dom/intersectionobserver/` —
        // observe/unobserve/disconnect + spec-annotated notification
        // algorithm, driven from the update-the-rendering steps in
        // `script_thread.rs`), and the webidl `[Pref=]` gate is the only
        // thing keeping the interface object off the Window global. Two
        // consumers need the Chrome-parity flip (real Chrome ships it on):
        // (1) puppeteer ≥13's `ElementHandle.scrollIntoViewIfNeeded` →
        // `isIntersectingViewport` constructs an IntersectionObserver IN
        // PAGE CONTEXT on every `page.click()` — without the flip each
        // click dies with `ReferenceError: IntersectionObserver is not
        // defined` (e124 puppeteer_real_lifecycle_e2e red); (2) the missing
        // global is itself a fingerprint signal (detection pages probe for
        // its presence). Same six-flip precedent as the five above.
        preferences.dom_intersection_observer_enabled = true;
        // `dom_permissions_enabled` defaults to false upstream; the vendor
        // Permissions implementation is REAL (`script/dom/permission/` —
        // Permissions.query/request/revoke resolving spec-shaped
        // PermissionStatus DOM objects with EventTarget + onchange, exposed
        // on Navigator AND WorkerNavigator via the webidl `[Pref=]` gate),
        // and the pref is the only thing keeping `navigator.permissions`
        // off the globals. Real Chrome ships the Permissions API on every
        // page; its absence is itself a fingerprint signal —
        // bot.sannysoft.com's "Permissions" row
        // (`navigator.permissions.query({name:'notifications'})`) fails on
        // the missing API alone (e124 residual: permissions-result:F).
        // Query semantics after the flip follow the spec defaults and
        // Chrome's posture: "prompt" on secure contexts (nothing granted),
        // "denied" on non-secure contexts. Same flip precedent as the six
        // above. REQ-BRW-002 rendering-face API gap closure (e130).
        preferences.dom_permissions_enabled = true;
        // `dom_notification_enabled` defaults to false upstream; the vendor
        // Notification implementation is REAL
        // (`script/dom/serviceworker/notification.rs` — constructor with
        // spec validation, static `permission` getter, requestPermission
        // with the deprecated-callback promise shape, full attribute
        // surface), pref-gated in Notification.webidl only. The
        // sannysoft Permissions row is a TWO-API probe: it reads
        // `Notification.permission` right beside
        // `navigator.permissions.query(...)` — with `Notification`
        // undefined the whole async probe throws and the row never colors
        // green. After the flip `Notification.permission` reports "default"
        // (the notifications permission state is "prompt" on secure
        // contexts — desktop Chrome's default posture), which does NOT
        // match the headless-Chrome signature the row actually hunts
        // (`Notification.permission === 'denied' && query.state ===
        // 'prompt'`). Same flip precedent as the seven above.
        // REQ-BRW-002 rendering-face API gap closure (e130).
        preferences.dom_notification_enabled = true;
        // `dom_exec_command_enabled` defaults to false upstream
        // (servoshell EXPERIMENTAL_PREFS); the vendor editing engine is
        // REAL and upstream-terminal (EditingContext + execCommand 25
        // commands + selection, 2026-10-06 snapshot swap, e149 recon
        // §1.3 byte-verified). The pref gates BOTH the
        // `document.execCommand`/`queryCommand*` webidl face AND
        // `Document::perform_editing_action` — every Document-context
        // editing action (contenteditable/designMode typing via
        // `EditingAction::InsertText`, Enter/paragraph, delete, arrows).
        // Chromium AND Firefox both ship execCommand; a missing surface is
        // itself a probe vector (anti-fingerprint constitution A), and the
        // servo WPT meta expectations for the editing domain are generated
        // with the pref ON (wptrunner passes
        // `--enable-experimental-web-platform-features` unconditionally).
        // User ruling 2026-10-08 ruling A (e150 campaign root finding).
        // Same flip precedent as the eight above.
        preferences.dom_exec_command_enabled = true;

        // CLI pref overrides (user ruling 2026-10-08 ruling A): applied on
        // top of the curated flips. `Servo::new` ends with
        // `prefs::set(builder.preferences.unwrap_or_default())`, so THIS is
        // the only durable injection point — the pre-launch global
        // application (`webdriver_host::apply_pref_overrides` from
        // `run_browser_entry`) would otherwise be wiped by the reset.
        // Fail-closed: a bad override aborts BrowserRuntime::new.
        preferences = crate::webdriver_host::apply_pref_overrides_to(
            preferences,
            &config.pref_overrides,
        )
        .map_err(BrowserError::Init)?;

        let servo: Rc<Servo> = Rc::new(if servo_already_initialized {
            // Already initialized. `Servo::new` (servo.rs:877) ALWAYS calls
            // `initialize_options(opts.unwrap_or_default())` — if we pass no
            // `.opts(...)`, it would invoke `initialize_options(Default)`
            // with (force_isolate_event_loops=false, disable_script_debugger=false),
            // which DIFFERS from the already-set (true, true) and would trip
            // the "conflicting bao config" panic in the patched
            // `initialize_options`. To stay idempotent, we clone the
            // already-stored opts and re-pass them: `Servo::new`'s internal
            // `initialize_options(existing.clone())` then sees identical
            // bao fields and becomes a no-op. This is the only way to keep
            // `Servo::new`'s unconditional `initialize_options` call safe
            // across multiple BrowserRuntime instances.
            //
            // NOTE: we use `is_initialized()` (pure read, no side effect),
            // NOT `opts::get()`. `opts::get()` uses `get_or_init(Default)`,
            // which would itself populate the `OnceLock` with defaults on the
            // very first call — racing against `Servo::new`'s real
            // `initialize_options((true, true))` and causing a spurious
            // "conflicting config" panic.
            ServoBuilder::default()
                .opts(servo::opts::get().clone())
                .preferences(preferences)
                .build()
        } else {
            ServoBuilder::default()
                .opts(desired_opts)
                .preferences(preferences)
                .build()
        });

        let delegate = Rc::new(BaoServoDelegate::new());
        servo.set_delegate(Rc::clone(&delegate) as Rc<dyn servo::ServoDelegate>);

        // BCE (page-realm async fetch black hole): wire the embedder
        // event-loop pump bridge — BOTH directions, process-globally (first
        // registration wins, so a second BrowserRuntime re-registers no-ops).
        //
        // Page realms install the Node-stack `fetch` override (same stack,
        // same fingerprint — the page-net unification posture), whose resolve
        // ConcurrentTask lands on the ScriptThread's bao MiniEventLoop — a
        // loop servo never ticks: `handle_msgs` blocks on servo's own
        // receivers. Without the bridge the request egressed but the page's
        // `fetch()` Promise never settled (fetch_axis_probe_tests B axis).
        //   pump side: servo calls it on each ScriptThread right after its
        //     blocking recv wakes, with the thread's JSContext.
        //   wake side: the fetch machinery captures the creating thread's
        //     wake closure (a servo `WakeUp` self-send) and fires it from the
        //     HTTPThread on resolve — that is exactly what unblocks the recv
        //     above. Node-realm threads have no servo wake entry (`None`) and
        //     keep their node-loop pumping unchanged.
        servo::register_bao_event_loop_pump(Box::new(|cx_ptr| {
            bun_runtime::timers::pump_embedder_thread(cx_ptr as *mut mozjs::jsapi::JSContext);
        }));
        // RED-1 P-A (user ruling 2026-09-10): same-registered-domain
        // navigation discards the old page realm on the REUSED
        // ScriptThread — servo cancels its own task sources in
        // `Window::clear_js_runtime`, but bao timers registered against the
        // old realm's global survived it: deadlines fired zombie callbacks
        // into the WindowState::Zombie realm, re-arming setImmediate chains
        // ran forever, and the raw-rooted `global_root` pinned the realm
        // against GC (per-navigation accumulation). Bridge the discard
        // (vendor patch, same registration face as the pump above) to bao's
        // per-thread timer registry purge — a document's timers die with
        // the document (browser navigation semantics).
        servo::register_bao_realm_discard_cancel(Box::new(|cx_ptr, global_ptr| {
            bun_runtime::timers::cancel_timers_for_global(
                cx_ptr as *mut mozjs::jsapi::JSContext,
                global_ptr as *mut mozjs::jsapi::JSObject,
            );
        }));
        // ISSUE #25 generalization (2026-09-29): liveness-probe half of the
        // discard story — servo-side external-thread resolve sites (audio
        // render/resume, media play, image decode, gamepad haptics, XR,
        // cookie store) query this before re-entering a realm's JS and drop
        // the settle when the realm was discarded. Each hit is counted
        // (post_discard_resolve_suppressed_total) for the guard's e2e
        // matrix.
        servo::register_bao_realm_liveness_probe(Box::new(|global_ptr| {
            let discarded = bun_runtime::timers::is_global_discarded(
                global_ptr as *mut mozjs::jsapi::JSObject,
            );
            if discarded {
                bun_runtime::timers::post_discard_resolve_suppressed_fetch_add();
            }
            discarded
        }));
        // ISSUE #24 servo wiring (2026-09-29): install the engine-native
        // execution-control armer — the servo evaluation paths
        // (ScriptThread embedder eval, worker `on_complete` / `importScripts`)
        // route armed (timeout) evaluations through this bridge into
        // bao_engine's ExecutionControl (JS_AddInterruptCallback + owner-thread
        // armed stack + deadline watcher). A named fn (not a closure literal)
        // so the higher-ranked closure-parameter coercion is explicit.
        servo::register_bao_execution_control_armer(Box::new(bao_execution_control_armer));
        // BCE-20260910-004 (settings-stack push — the missing half of the
        // pump bridge): the pump fires page-realm bao timers outside any
        // servo script settings-stack entry, so a page callback touching
        // `location.*` / `document.open()` / canvas origin-clean hit
        // `entry_global().unwrap()` on an empty stack and panicked
        // (settings_stack.rs:36, Script#3 meituan). Lend servo's own
        // "prepare to run script" wrapper (`run_a_script`) to bao's timer
        // dispatch — the same contract every servo JS entry honors.
        // Zero-capture forwarder is load-bearing: `BaoSettingsRunner` is a
        // bare fn pointer, so every BrowserRuntime's registration compares equal
        // (first-writer-wins contract, see timers::register_bao_settings_runner).
        //
        // W26 (BCE 2026-09-29, node-realm timer fire kills ScriptThread):
        // `fire_js` dispatches EVERY bao timer through this runner —
        // including NODE-realm timers armed via `evaluate_js`. But
        // `servo::bao_run_in_script_settings` is DOM-only
        // (`GlobalScope::from_object` unwraps and panics on a plain JS
        // global), and that panic KILLS the whole ScriptThread: every page
        // it hosts loses its realms — W21a's "churn clears the registry"
        // evidence was exactly this (the registry entry survives the dead
        // thread with its pointer zeroed by the dying runtime's GC tracer,
        // and the survivor's evaluate then reports "Node Realm not
        // initialized"). Discriminate by the registry: the settings-stack
        // push applies only to registered PAGE-realm globals; node-realm
        // (and unknown) globals keep the bare dispatch — `fire_js`'s own
        // documented contract ("unregistered node realms keep the bare
        // dispatch") finally holds.
        bun_runtime::timers::register_bao_settings_runner(|cx, global, f| {
            bao_timer_settings_runner(
                cx as *mut std::ffi::c_void,
                global as *mut std::ffi::c_void,
                f,
            );
        });
        bun_runtime::fetch_async::set_thread_wakeup_bridge(|| {
            servo::bao_current_thread_wake_fn().map(|wake| {
                wake as bun_runtime::fetch_async::ThreadWakeup
            })
        });

        // BCE-20260910-002 (embedder pump gap — webview-less fetch): SW/worker
        // realms carry no webview, so their fetches round-trip
        // `WebResourceRequested(target_webview_id=None)` through the
        // net→embedder channel, which ONLY `Servo::spin_event_loop` drains —
        // and bao pumps that solely from PageHandle interaction APIs. With the
        // owning page idle (no evaluate/screenshot in flight), the round-trip
        // was never answered and the SW's fetch parked forever (the
        // fetchevent 25s stall). A resident pump thread is structurally
        // impossible: `Servo(Rc<ServoInner>)` is !Send/!Sync (RefCell/Rc
        // state), so only the creating thread may spin — and that thread can
        // be asleep in user code no bao hook can reach. Instead the net
        // interceptor consults this process-global handler for webview-less
        // requests; `PassThrough` is byte-equivalent to what the embedder
        // path produces for bao today (BaoServoDelegate inherits the no-op
        // `ServoDelegate::load_web_resource` → `WebResourceLoad` drop →
        // default DoNotIntercept). Webview-owned requests keep the full
        // embedder round-trip (CDP/stealth mediation unchanged). If bao ever
        // overrides `load_web_resource` for webview-less loads, that logic
        // belongs in this handler.
        servo::set_webviewless_resource_handler(Some(Arc::new(
            |_request| servo::BaoWebviewlessResourceVerdict::PassThrough,
        )));

        let page_pool = Rc::new(PagePool::new(
            Rc::clone(&servo),
            Rc::clone(&delegate),
            &config,
        ));
        // REQ-LIB-001: arm the pool's own weak handle — every page delegate
        // carries a derived Weak so servo's embedder dispatch
        // (request_create_new / notify_closed) reaches the pool from the
        // pump thread.
        page_pool.arm_self_weak(Rc::downgrade(&page_pool));

        // #40 page-pipeline stall watchdog: every page op runs on this
        // thread; when one wedges inside a never-returning primitive the
        // process hangs silently (soak MTBF≈46min). The watchdog thread is
        // the only in-process witness that can still speak — it dumps the
        // stalled phase + thread wchan snapshot to the log (idempotent,
        // process-wide, pure observer).
        phase_watch::spawn_watchdog();

        // memory:// CDP transport (published consumer contract:
        // `Browser::connect("memory://bao")` → `version()`/`pages()`).
        // Install the host-side bridge into bao_cdp_client's process
        // registry (last-writer-wins across runtimes); `run()` drains the
        // receiver so servo-routed commands execute for real.
        let (cdp_bridge, cdp_bridge_rx) = cdp_memory::MemoryCdpBridge::new("");
        let cdp_bridge_token =
            bao_cdp_client::browser::set_process_memory_bridge(
                cdp_bridge.clone() as std::sync::Arc<dyn bao_cdp_client::transport::InMemoryBridge>,
            );

        Ok(BrowserRuntime {
            servo,
            delegate,
            page_pool,
            cdp_port: config.cdp_port,
            cdp_bridge: Some(cdp_bridge),
            cdp_bridge_rx: Some(cdp_bridge_rx),
            cdp_bridge_token: Some(cdp_bridge_token),
            #[cfg(feature = "webdriver")]
            webdriver_host: std::cell::RefCell::new(None),
            exit_scheduled: std::cell::Cell::new(false),
        })
    }

    /// The process servo handle (WebDriver host face: command forwarding +
    /// site-data manager).
    pub fn servo(&self) -> &Rc<Servo> {
        &self.servo
    }

    /// Schedule a cooperative exit — the run loops return Ok(()) at their
    /// next iteration boundary. Only the WebDriver `Shutdown` command sets
    /// this (the CDP path keeps its own stop handle).
    pub fn schedule_exit(&self) {
        self.exit_scheduled.set(true);
    }

    pub fn exit_scheduled(&self) -> bool {
        self.exit_scheduled.get()
    }

    /// Install the WebDriver host (must happen before the run loop starts;
    /// `webdriver_server::start_server` binds the HTTP port immediately).
    #[cfg(feature = "webdriver")]
    pub fn install_webdriver_host(&self, port: u16) {
        let host = std::sync::Arc::new(webdriver_host::WebDriverHost::start(port));
        self.webdriver_host.borrow_mut().replace(host);
    }

    #[cfg(feature = "webdriver")]
    fn webdriver_host(&self) -> Option<std::sync::Arc<webdriver_host::WebDriverHost>> {
        self.webdriver_host.borrow().clone()
    }

    /// The webview ids of every live page (GetAllWebViews face).
    #[cfg(feature = "webdriver")]
    pub fn webdriver_webview_ids(&self) -> Vec<servo::WebViewId> {
        self.page_pool
            .live_page_ids()
            .into_iter()
            .filter_map(|id| self.page_pool.get_page(id))
            .filter_map(|page| page.webdriver_webview_id())
            .collect()
    }

    /// Find a live page by servo WebViewId (the per-webview command face).
    #[cfg(feature = "webdriver")]
    pub fn page_for_webview(
        &self,
        webview_id: servo::WebViewId,
    ) -> Option<(usize, PageHandle)> {
        self.page_pool.live_page_ids().into_iter().find_map(|id| {
            self.page_pool.get_page(id).and_then(|page| {
                if page.webdriver_webview_id() == Some(webview_id) {
                    Some((id, page))
                } else {
                    None
                }
            })
        })
    }

    /// Create an about:blank page for a WebDriver NewWindow and return its
    /// (WebViewId, page) pair.
    #[cfg(feature = "webdriver")]
    pub fn create_webdriver_page(
        &self,
        url: url::Url,
    ) -> Option<(servo::WebViewId, PageHandle)> {
        let config = PageConfig {
            url: Some(url.to_string()),
            ..Default::default()
        };
        let page = self.create_page(&config).ok()?;
        let webview_id = page.webdriver_webview_id()?;
        Some((webview_id, page))
    }

    pub fn page_pool(&self) -> &Rc<PagePool> {
        &self.page_pool
    }

    pub fn create_page(&self, config: &PageConfig) -> Result<PageHandle, BrowserError> {
        // ALL page injection (engine/Web APIs + stealth props + Worker-scope
        // callback) happens exactly ONCE inside PagePool::create_page — the
        // single true source. This method used to re-run
        // inject_all_with_profile on the already-injected page; the second
        // install_webgl_override re-saved the first pass's JS hook as
        // "__originalGetParameter__", dead-looping every un-intercepted
        // getParameter into literal `undefined` in the Window realm (e36).
        // Pipeline readiness is established inside PagePool::create_page
        // (wait_for_pipeline_ready + drain_callbacks) before that injection.
        let page = self.page_pool.create_page(config)?;

        // The memory:// flat client face follows the newest page.
        if let Some(bridge) = &self.cdp_bridge {
            bridge.set_default_target(page.id().to_string());
        }
        Ok(page)
    }

    /// Create a Dedicated Worker bridged to a page's servo Realm.
    ///
    /// This is the primary entry point for the Worker constructor bridging
    /// Create a Dedicated Worker via servo's native Worker::Constructor.
    ///
    /// Per DEC-WK-001 (BCE-20260627-008), bao no longer spawns a
    /// `bao_engine::WebWorker` bypass thread. Instead, this method dispatches
    /// `new Worker(url)` into the page via servo's DOM binding, and servo
    /// constructs the Worker thread + DedicatedWorkerGlobalScope internally.
    /// bao's role is reduced to:
    ///   1. Steering stealth profile + DedicatedWorkerGlobalScope Web APIs by
    ///      registering the scope callback via
    ///      `register_worker_scope_callback_native` (per-worker, see
    ///      `create_worker_with_url`; page-init registration for page-script
    ///      created Workers lives in `inject_all_with_profile`). The callback
    ///      runs on the Worker thread via the servo vendor patch
    ///      `drain_worker_scope_callbacks`.
    ///   2. Tracking the WorkerHandle for CDP observability + page-unload
    ///      termination (criterion #10, AutoCloseWorker).
    ///   3. Providing a WorkerChannelBridge for page↔worker postMessage
    ///      (criterion #6, DF-WK-4/5) so the bao side can still observe
    ///      structured-clone traffic even though the thread is servo-owned.
    ///
    /// The `script` argument is treated as a Worker script URL. For inline
    /// scripts, callers should materialize a `data:`/`blob:` URL and pass it
    /// here (or call `create_worker_with_url`).
    ///
    /// @trace DEC-WK-001 servo-native Worker path (bypass removed)
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:1..10] [criterion:12..18]
    /// @trace REQ-BRW-4 [criterion:C1..C4]
    pub fn create_worker(
        &self,
        page: &PageHandle,
        script: &str,
    ) -> Result<WorkerHandle, BrowserError> {
        self.create_worker_with_url(page, script)
    }

    /// Create a Dedicated Worker with a script URL resolved by servo's native
    /// script loading pipeline.
    ///
    /// Per DEC-WK-001 (BCE-20260627-008) the bypass `bao_engine::WebWorker`
    /// path is removed. The Worker is constructed by servo's DOM
    /// `Worker::Constructor` when `new Worker(url)` is evaluated in the page.
    /// This method:
    ///   1. Builds the WorkerId + WorkerHandle (closing/terminated flags +
    ///      REALM_PROFILES global_addr_slot, criterion #18).
    ///   2. Wires up the WorkerChannelBridge (criterion #6, DF-WK-4/5).
    ///   3. Registers DedicatedWorkerGlobalScope state for CDP observability
    ///      (criteria #8, #12-17 stealth consistency).
    ///   4. Tracks the Worker with AutoCloseWorker (criterion #10).
    ///   5. Dispatches `new Worker(url)` into the page via servo's DOM binding.
    ///
    /// @trace DEC-WK-001 servo-native Worker path (bypass removed)
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    /// @trace REQ-BRW-4 [criterion:C1..C4]
    pub fn create_worker_with_url(
        &self,
        page: &PageHandle,
        url: &str,
    ) -> Result<WorkerHandle, BrowserError> {
        let webview_state = page.webview_state();

        // Get the page's WorkerScopeConfig for stealth consistency.
        // The per-worker scope callback registered below inherits this profile
        // onto the Worker's DedicatedWorkerGlobalScope.
        // @trace REQ-BRW-004 [criterion:12..17] CRIT-STL-WK
        let scope_config = webview_state.borrow().worker_scope_config.clone();

        // Generate WorkerId
        let worker_id = crate::delegate::WorkerId(url.to_string());

        // Create WorkerHandle — tracks closing/terminated state via
        // Arc<AtomicBool> and the worker_global_addr for REALM_PROFILES
        // cleanup (criterion #18). The per-worker scope callback registered
        // below (before the `new Worker(url)` dispatch) writes the Worker's
        // global address into this handle's slot on its first run on the
        // Worker thread, making crash-safe teardown's REALM_PROFILES
        // unregistration production-reachable (E22 audit defect C).
        // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
        let handle = WorkerHandle::new(url.to_string());

        // Register a per-worker scope callback carrying this handle's
        // global-addr slot, keyed to THIS page's WebViewId so only Workers
        // created by this page drain it (cross-page crosstalk fix: without
        // the key a global queue drain let one page's Worker consume another
        // page's queued callback). Registered strictly before the
        // `new Worker(url)` dispatch so the callback is queued when servo's
        // Worker thread drains EMBEDDER_WORKER_SCOPE_CALLBACKS at scope
        // construction (DEC-WK-001). On the callback's first run the Worker
        // global's address is backfilled into the handle and the page's
        // stealth profile is installed (criteria #12-17).
        // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
        let webview_id = page
            .webview_id()
            .ok_or_else(|| BrowserError::Init("page has no webview".into()))?;
        runtime_bridge::register_worker_scope_callback_native(
            webview_id,
            scope_config.stealth_profile.clone(),
            Some(handle.worker_global_addr_arc()),
        );
        // Second-phase per-worker registration RETIRED (REQ-BRW-004, e43
        // multi-worker gap fix): the page-init per-Worker interfaces-ready
        // INJECTOR (registered in PagePool::create_page, non-consuming) now
        // covers EVERY Worker of this page at the same post-interfaces drain
        // point — including this bao-created one. Keeping BOTH made the first
        // Worker run the W1a JS hooks blob twice at that point, and the audio
        // getChannelData wrapper (closure-based, no property-slot idempotency
        // guard) double-applied the deterministic noise, breaking cross-realm
        // digest equality. The scope callback above (slot backfill) stays
        // consume-once.
        // @trace REQ-BRW-004 [criterion:15] worker JS-hook per-Worker delivery

        // Create channel bridge (DF-WK-4/5). Even though servo owns the Worker
        // thread, bao still tracks the bidirectional structured-clone traffic
        // for CDP observability and message logging.
        // @trace REQ-BRW-004 [criterion:6] DF-WK-4 / DF-WK-5
        let _endpoints = webview_state
            .borrow_mut()
            .create_worker_channel(worker_id.clone());

        // Register DedicatedWorkerGlobalScope state for CDP observability
        // and stealth consistency verification.
        // @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
        let scope_state =
            crate::delegate::DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &scope_config);
        webview_state
            .borrow_mut()
            .register_dedicated_worker_scope(worker_id.clone(), scope_state);

        // Track the WorkerHandle with AutoCloseWorker — ensures termination on
        // page unload (SPEC criterion #10: GlobalScope::track_worker).
        // @trace REQ-BRW-004 [criterion:10] GlobalScope::track_worker + AutoCloseWorker
        webview_state.borrow_mut().track_worker(handle.clone());

        // Dispatch `new Worker(url)` into the page via servo's DOM binding.
        // servo's Worker::Constructor runs the full DF-WK-2 pipeline
        // (fetch → MIME check → decode → compile) and spawns the Worker thread
        // internally; bao's scope callback fires on the Worker thread to install
        // DedicatedWorkerGlobalScope APIs + stealth properties (criteria #8, #12-17).
        // @trace DEC-WK-001 servo-native Worker path
        // @trace REQ-BRW-004 [criterion:1] new Worker(url) creates worker thread
        // @trace REQ-BRW-004 [DF-WK-2] Worker script loading pipeline
        let new_worker_js = format!(
            "(function() {{ var w = new Worker({}); return ''; }})();",
            serde_json::Value::String(url.to_string())
        );
        page.evaluate_js_web(&new_worker_js).map_err(|e| {
            BrowserError::Init(format!(
                "Failed to dispatch new Worker({:?}) via servo DOM: {}",
                url, e
            ))
        })?;

        log::debug!(
            "[bao] dispatched new Worker({:?}) via servo DOM (tracked via AutoCloseWorker, DEC-WK-001 native path)",
            url
        );

        Ok(handle)
    }

    pub fn spin_event_loop(&self) {
        self.servo.spin_event_loop();
    }

    /// Set the console log forwarding channel on the servo delegate.
    /// Console messages from servo will be sent to this channel.
    pub fn set_console_log_channel(&self, tx: std::sync::mpsc::Sender<cdp_server::ConsoleMessage>) {
        self.delegate.set_console_log_tx(tx.clone());
        // Retro-propagate to every existing page's webview state, exactly
        // like set_event_channel below: servo routes console messages
        // per-webview (ShowConsoleApiMessage → the webview delegate reads
        // state.console_log_tx), so a channel set only on the runtime-level
        // delegate never reaches pages created before this call — in
        // run_browser that is precisely the initial page, whose
        // `__BAO_EVT__` CDP-event texts (Debugger.scriptParsed/.paused,
        // SM-EVOLUTION #27 裁决 2 transport) would die in the webview
        // delegate's unset state.
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        let stats = self.page_pool.stats();
        for id in 1..=(stats.active + stats.idle) {
            if let Some(page) = self.page_pool.get_page(id) {
                page.webview_state().borrow_mut().console_log_tx = Some(tx.clone());
            }
        }
    }

    /// Set the structured event forwarding channel on the servo delegate.
    /// When set, servo callbacks push ServoEvent (Path B) as the primary event path.
    /// Also propagates the sender to every existing page's webview state —
    /// servo routes per-webview callbacks (console/url/load) through the
    /// per-webview delegate, which reads `state.event_tx`, so the channel
    /// must live on each state, not only the runtime-level delegate.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn set_event_channel(&self, tx: std::sync::mpsc::Sender<ServoEvent>) {
        self.delegate.set_event_tx(tx.clone());
        let stats = self.page_pool.stats();
        for id in 1..=(stats.active + stats.idle) {
            if let Some(page) = self.page_pool.get_page(id) {
                page.webview_state().borrow_mut().event_tx = Some(tx.clone());
            }
        }
    }

    pub fn run(&self) -> Result<(), BrowserError> {
        let max_wait = Duration::from_secs(300);
        let start = std::time::Instant::now();

        // WebDriver sessions outlive the interactive 300s cap (a wptrunner
        // run keeps one browser for the whole suite chunk): under the
        // WebDriver host the loop runs until Shutdown, not until the clock.
        #[cfg(feature = "webdriver")]
        let webdriver_mode = self.webdriver_host.borrow().is_some();
        #[cfg(not(feature = "webdriver"))]
        let webdriver_mode = false;

        // Loop predicate: WebDriver mode runs until Shutdown (a wptrunner
        // session outlives any fixed wall clock); every other mode keeps the
        // interactive 300s cap.
        while if webdriver_mode {
            !self.exit_scheduled()
        } else {
            start.elapsed() < max_wait
        } {
            self.servo.spin_event_loop();
            // REQ-CDP-009 screencast tick BEFORE the repaint sweep: the
            // capture path composites on demand (consuming the latch), so
            // polling first keeps change detection exact (this pump has no
            // WS event path — memory clients only).
            screencast::drive(&self.page_pool, None);
            // Headless redraw leg: composite any webview servo requested a frame
            // for (refresh-driver heartbeat — see PagePool::paint_pages_needing_repaint).
            self.page_pool.paint_pages_needing_repaint();
            self.page_pool.check_idle_pages();
            // REQ-LIB-001: settle window.open popups — physical teardown for
            // content-closed pages first (window.close → notify_closed
            // retire), then pipeline-ready wait + injection for freshly
            // adopted popups. Both run pump-side, never re-entrant with the
            // event loop.
            self.page_pool.close_pending_pages();
            self.page_pool.init_pending_pages();
            // memory:// CDP commands (process-registry bridge) execute here:
            // the drain answers every in-process client command that routed
            // through the bridge channel (Runtime.evaluate, Target listing…).
            if let Some(rx) = &self.cdp_bridge_rx {
                rx.drain(|cmd| cdp_handler::handle_bridge_command(cmd, &self.page_pool));
            }
            // WPT official-toolchain face (REQ-BRW-002): answer the
            // webdriver_server embedder channel between spins.
            #[cfg(feature = "webdriver")]
            if let Some(host) = self.webdriver_host() {
                host.drain(self);
            }
            // Bounded poll (1ms): the loop drains non-blocking channels, so it
            // must re-poll on a fixed cadence rather than block — but a bare
            // `yield_now()` burns a FULL core per process forever (proven: an
            // idle single process sat at 99.4% CPU with zero voluntary context
            // switches, e26 evidence 2026-10-03). At N concurrent wptrunner
            // browsers that is N cores of pure spin amplifying scheduler
            // contention machine-wide. A 1ms poll keeps drain latency bounded
            // at ≤1ms (orders of magnitude under every WebDriver/WPT budget)
            // while dropping the idle burn to ~0.1% of a core.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let _stats = self.page_pool.stats();

        Ok(())
    }

    /// Bounded single-thread CDP pump: spin the servo loop and drain the
    /// memory:// CDP bridge for `duration`. This is the single-threaded
    /// consumer contract for in-process CDP — the `BrowserRuntime` is `!Send`
    /// (per-thread JSContext model), so the runtime thread pumps while a
    /// helper thread holds the `Browser` client whose dispatches arrive
    /// through the bridge channel this drain answers.
    ///
    /// `BrowserRuntime::run` is the unbounded version (it also drains).
    pub fn pump_cdp(&self, duration: std::time::Duration) {
        let start = std::time::Instant::now();
        while start.elapsed() < duration {
            self.servo.spin_event_loop();
            // REQ-CDP-009 screencast tick BEFORE the repaint sweep (same
            // ordering rationale as `run`; memory clients only here too).
            screencast::drive(&self.page_pool, None);
            // Headless redraw leg: composite any webview servo requested a frame
            // for (refresh-driver heartbeat — see PagePool::paint_pages_needing_repaint).
            self.page_pool.paint_pages_needing_repaint();
            self.page_pool.check_idle_pages();
            // REQ-LIB-001: settle window.open popups — physical teardown for
            // content-closed pages first (window.close → notify_closed
            // retire), then pipeline-ready wait + injection for freshly
            // adopted popups. Both run pump-side, never re-entrant with the
            // event loop.
            self.page_pool.close_pending_pages();
            self.page_pool.init_pending_pages();
            if let Some(rx) = &self.cdp_bridge_rx {
                rx.drain(|cmd| cdp_handler::handle_bridge_command(cmd, &self.page_pool));
            }
            // Bounded poll — same cadence rationale as `run` (yield_now burned
            // a full core for the whole pump duration).
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Run with a CDP bridge that processes commands during the event loop.
    /// Also drains ServoEvent from the real event queue (Path B — unbounded
    /// reliable mpsc) and broadcasts translated CdpEvents via the shared
    /// EventBroadcaster.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn run_with_bridge(
        &self,
        bridge_rx: bao_cdp::servo_bridge::BridgeReceiver,
        servo_event_rx: std::sync::mpsc::Receiver<ServoEvent>,
        broadcaster: Arc<EventBroadcaster>,
        event_demux: Option<Arc<BaoWsRegistry>>,
    ) -> Result<(), BrowserError> {
        let max_wait = Duration::from_secs(3600);
        let start = std::time::Instant::now();

        while start.elapsed() < max_wait {
            self.servo.spin_event_loop();
            // REQ-CDP-009 screencast tick BEFORE the repaint sweep (capture
            // consumes the latch; ordering rationale as `run`). WS-origin
            // frames ride the same target-scoped routing as servo events.
            let ws_sink = |target: &str, method: &str, params: serde_json::Value| {
                match event_demux.as_ref() {
                    Some(registry) => registry.broadcast_for_target(
                        broadcaster.as_ref(),
                        target,
                        method,
                        params,
                    ),
                    None => broadcaster.send_page_event(target, method, params),
                }
            };
            screencast::drive(&self.page_pool, Some(&ws_sink));
            // Headless redraw leg: composite any webview servo requested a frame
            // for (refresh-driver heartbeat — see PagePool::paint_pages_needing_repaint).
            // Without this the pipeline is boot-once: rAF ticks and screenshot
            // capture only advance inside `Painter::render`.
            self.page_pool.paint_pages_needing_repaint();
            self.page_pool.check_idle_pages();
            // REQ-LIB-001: settle window.open popups — physical teardown for
            // content-closed pages first (window.close → notify_closed
            // retire), then pipeline-ready wait + injection for freshly
            // adopted popups. Both run pump-side, never re-entrant with the
            // event loop.
            self.page_pool.close_pending_pages();
            self.page_pool.init_pending_pages();

            // Process pending CDP bridge commands
            bridge_rx.drain(|cmd| cdp_handler::handle_bridge_command(cmd, &self.page_pool));

            // Drain ServoEvent from the real event queue (Path B) and broadcast
            // as CDP events via the shared EventBroadcaster.
            // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
            drain_servo_events(&servo_event_rx, |servo_event| {
                // W43 flat-session demux: ServoEvents are target-scoped —
                // route to every CDP session attached to that target (tagged),
                // falling back to the untagged broadcast with no attachments.
                let target_id = servo_event.target_id().to_string();
                let cdp_events = translate(servo_event);
                match event_demux.as_ref() {
                    Some(registry) => {
                        for cdp_event in cdp_events {
                            registry.broadcast_for_target(
                                broadcaster.as_ref(),
                                &target_id,
                                &cdp_event.method,
                                cdp_event.params,
                            );
                        }
                    }
                    None => {
                        for cdp_event in cdp_events {
                            broadcaster.send_event(&cdp_event.method, cdp_event.params);
                        }
                    }
                }
            });

            // Bounded poll — same cadence rationale as `run` (yield_now burned
            // a full core for the whole pump duration; 1ms keeps bridge-command
            // latency negligible).
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        Ok(())
    }
}

impl Drop for BrowserRuntime {
    fn drop(&mut self) {
        self.page_pool.close_all();
        // Remove this runtime's memory bridge unless a newer runtime has
        // already replaced it in the process registry.
        if let Some(token) = self.cdp_bridge_token.take() {
            bao_cdp_client::browser::clear_process_memory_bridge(token);
        }
    }
}

/// RETIRED (REQ-BRW-004, e43 multi-worker gap fix): the consume-once
/// interfaces-ready callback registration that used to live here was replaced
/// by the PER-WORKER injector below (`register_worker_interfaces_ready_
/// injector_native`, registered once at page init and delivered to EVERY
/// Dedicated Worker of the page). Keeping both made the first Worker run the
/// W1a JS hooks blob twice at the second drain point — the audio
/// getChannelData wrapper is closure-based with no property-slot idempotency
/// guard (unlike getParameter's e36 __originalGetParameter__ gate), so the
/// double wrap double-applied the deterministic noise and broke cross-realm
/// digest equality (c15_worker_window_cross_realm_noise_consistency). The
/// vendor one-shot queue (`servo::register_worker_interfaces_ready_callback`)
/// remains for any future one-shot consumer; bao no longer registers one.
///
/// Register the second-phase (interfaces-ready) injector with PER-WORKER
/// delivery (REQ-BRW-004, user ruling 2026-09-09 vendor patch).
///
/// Same install as [`register_worker_interfaces_ready_callback_native`]
/// (re-run of the idempotent `set_profile_for_global` +
/// `install_stealth_props`), but delivered to EVERY Dedicated Worker of the
/// webview instead of only the first one that drains the consume-once queue
/// (e43 multi-worker gap: the 2nd+ page-JS `new Worker()` previously got
/// neither the engine getters nor any W1a JS hook — a bare fingerprintable
/// Worker). Never consumed; upserted per webview at page init.
///
/// @trace REQ-BRW-004 [criterion:15] worker JS-hook per-Worker delivery
fn register_worker_interfaces_ready_injector_native(
    webview_id: servo::WebViewId,
    profile: Option<bao_stealth::StealthProfile>,
) {
    let injector: servo::EmbedderWorkerInjector = std::sync::Arc::new(move |cx_ptr, global_ptr| {
        let raw_cx = cx_ptr as *mut mozjs::jsapi::JSContext;
        let raw_global = global_ptr as *mut mozjs::jsapi::JSObject;
        if raw_cx.is_null() || raw_global.is_null() {
            log::warn!(
                "[register_worker_interfaces_ready_injector_native] NULL cx/global — \
                 skipping interfaces-ready install (REQ-BRW-004 per-Worker delivery)"
            );
            return;
        }
        let Some(ref profile) = profile else {
            return;
        };
        unsafe {
            // Realm entry mirrors worker_scope_init_native (runtime_bridge):
            // the worker thread's cx starts in the NULL realm, and any JSAPI
            // that atomizes dereferences a NULL zone and SIGSEGVs without
            // this. AutoRealm roots the global and restores the NULL
            // starting realm on drop (leaveRealm is null-safe).
            use mozjs::context::JSContext;
            use mozjs::realm::AutoRealm;
            use std::ptr::NonNull;
            let cx_nn = NonNull::new_unchecked(raw_cx);
            let mut cx = JSContext::from_ptr(cx_nn);
            let _worker_realm = AutoRealm::new(&mut cx, NonNull::new_unchecked(raw_global));

            bao_stealth::engine_props::set_profile_for_global(raw_global as usize, profile);
            bao_stealth::engine_props::install_stealth_props(raw_cx, raw_global);
            // E22 audit defect A guard (same as the one-shot drain): do NOT
            // re-seed the servo rendering-layer canvas noise here — realm-
            // scoped noise is delivered via set_profile_for_global above.
        }
    });
    servo::register_worker_interfaces_ready_injector(webview_id, injector);
}

/// W26 (BCE 2026-09-29): the bao timer settings-runner entry — see the
/// registration site in [`BrowserRuntime::new`] for the full chain. Pushes
/// servo's script settings stack only for registered PAGE-realm globals;
/// NODE-realm globals (plain JS globals — timers armed via `evaluate_js`)
/// keep the bare dispatch, because `GlobalScope::from_object` is DOM-only
/// and panics otherwise, killing the hosting ScriptThread.
fn bao_timer_settings_runner(
    cx: *mut std::ffi::c_void,
    global: *mut std::ffi::c_void,
    f: &mut dyn FnMut(),
) {
    if crate::runtime_bridge::is_known_page_global(global) {
        servo::bao_run_in_script_settings(cx, global, f);
    } else {
        // Node realm (or unregistered global): no servo settings-stack
        // semantics exist for it — run the callback bare, the exact
        // pre-BCE-20260910-004 node-timer behavior.
        f();
    }
}

/// The engine-native execution-control armer installed into servo
/// (ISSUE #24 servo wiring; see the registration site in `BrowserRuntime::new`).
/// Contract: run the payload closure synchronously on the calling (owner JS)
/// thread under bao_engine's `ExecutionControl` armed with `timeout`, and
/// return its boxed output plus the stable termination message when a control
/// termination fired. HRTB over the payload lifetime: servo call sites close
/// over method-local borrows, while the payload VALUE stays `'static`.
fn bao_execution_control_armer<'a>(
    cx_ptr: *mut std::ffi::c_void,
    timeout: Duration,
    run: Box<dyn FnOnce() -> Box<dyn std::any::Any> + 'a>,
) -> (Box<dyn std::any::Any>, Option<String>) {
    let raw_cx = cx_ptr as *mut mozjs::jsapi::JSContext;
    // Owner-thread face: this bridge runs on servo's ScriptThread / worker
    // thread, which owns the context — the ExecutionControl ownership contract
    // holds, and run_raw_with_control asserts the binding fail-closed.
    let control = bao_engine::execution_control::ExecutionControl::for_context(raw_cx);
    let (boxed, state) =
        bao_engine::execution_control::run_raw_with_control(raw_cx, &control, Some(timeout), run);
    let termination = match state {
        bao_engine::execution_control::TerminalState::TimedOut
        | bao_engine::execution_control::TerminalState::Cancelled => {
            Some(bao_engine::execution_control::terminal_message(state))
        },
        _ => None,
    };
    (boxed, termination)
}

pub fn run_browser(config: BrowserConfig) -> Result<(), BrowserError> {
    let _stealth = config.stealth_profile.is_some();
    let url = config.url.clone();
    let webdriver_port = config.webdriver_port;
    let bao_config: BaoConfig = config.into();
    let cdp_port = bao_config.cdp_port;

    let runtime = BrowserRuntime::new(bao_config)?;

    // Create initial page
    let page_config = PageConfig {
        url: url.clone(),
        stealth_profile: None,
        ..Default::default()
    };
    let page = runtime.create_page(&page_config)?;
    if let Some(ref page_url) = url {
        log::debug!("[bao] navigating to {}", page_url);
    }

    // WPT official-toolchain face (REQ-BRW-002): wptrunner's `servo` product
    // launches the binary with `--webdriver=PORT` and speaks WebDriver to it.
    // The run loop is runtime.run() — under the host it runs until the
    // WebDriver Shutdown command instead of the interactive 300s cap.
    #[cfg(feature = "webdriver")]
    if let Some(port) = webdriver_port {
        runtime.install_webdriver_host(port);
        return runtime.run();
    }
    #[cfg(not(feature = "webdriver"))]
    let _ = webdriver_port;

    if let Some(port) = cdp_port {
        // Create bridge channel for CDP <-> servo communication
        let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));

        // Create console log forwarding channel: servo delegate → CDP Log domain
        let (console_tx, console_rx) = std::sync::mpsc::channel::<cdp_server::ConsoleMessage>();
        runtime.set_console_log_channel(console_tx);

        // Real event queue (Path B): unbounded reliable mpsc — the servo
        // delegate pushes ServoEvents that `run_with_bridge` drains and
        // translates. Delivery is lossless by construction (REQ-CDP-004);
        // the emitted/pumped probe pair (delegate.rs / the pump loop below)
        // asserts it.
        // @trace REQ-CDP-006
        let (event_tx, servo_event_rx) = std::sync::mpsc::channel::<ServoEvent>();
        runtime.set_event_channel(event_tx);

        // Build CdpServer and extract the shared broadcaster BEFORE moving the
        // server into its thread. The broadcaster is Arc<EventBroadcaster> which
        // shares the same SessionMap — events sent via this broadcaster reach all
        // connected WebSocket sessions.
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        //
        // REQ-CDP WS command face: the registry is the real command dispatcher
        // (BaoWsRegistry → bao_cdp::handle_command → servo bridge), not the
        // EmptyHandler placeholder — WS sessions get real Page.navigate /
        // Runtime.evaluate / Target.* round-trips (Playwright direct connect).
        let registry = Arc::new(BaoWsRegistry::new(bridge_tx.clone()));
        let config = ServerConfig::builder().host("127.0.0.1").port(port).build();
        // The default target is the initial page's real id (cdp_handler parses
        // decimal page ids — a timestamp hex would never resolve to a page).
        let target_id = page.id().to_string();
        let mut server = CdpServer::with_registry(config, registry.clone());
        let provider = Arc::new(ServoTargetProvider::new(
            bridge_tx,
            target_id,
            "127.0.0.1".into(),
            port,
        ));
        server.set_target_provider(provider);
        server.set_console_receiver(console_rx);
        // Clone the broadcaster before moving server into the thread.
        // Arc<EventBroadcaster> shares the same SessionMap with the server.
        let broadcaster = server.broadcaster();

        // Grab the stop handle BEFORE moving the server into its thread —
        // the spawner signals shutdown through this shared flag.
        let stop_flag = server.stop_handle();
        let server_thread = std::thread::spawn(move || {
            let _ = server.run();
        });

        let result = runtime.run_with_bridge(bridge_rx, servo_event_rx, broadcaster, Some(registry.clone()));
        // Deterministic CDP shutdown (B0 census #3): signal the cooperative
        // stop and join the server thread so the listener port, registry and
        // sessions are released before run_browser returns. Bounded: run()
        // checks the flag each iteration (10ms cadence), so the join only
        // waits on the already-signaled exit path.
        stop_flag.store(true, std::sync::atomic::Ordering::Release);
        let _ = server_thread.join();
        return result;
    }

    runtime.run()
}

// Higher-tier soft-link providers are dev-deps that nothing in this crate's
// production code path `use`s, so rustc never puts them on the test link line
// — GNU ld tolerates the undefined CYCLEBREAK faces in test executables
// (--gc-sections discards the dead referencing sections), lld-link/COFF with
// /OPT:NOREF does not (#28). This seam makes the lib-test unit load
// `bao_bundler`, whose rlib then resolves `JSBundlerPlugin__*` /
// `DevServerHandle__Bake__*` / `VmLoaderCtx__Runtime__*` / `__bun_macro_*` /
// HMR via archive lazy-pull. Twin: bao_engine / bao_stealth test_link_seams.
#[cfg(test)]
mod test_link_seams {
    #[test]
    fn link_higher_tier_seams() {
        bao_bundler::force_link_test_seams();
    }
}
