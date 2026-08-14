// @trace REQ-BRW-001 [entity:BrowserContext] [entity:PageHandle]
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope]
// @trace REQ-BRW-4 [entity:Worker] [entity:SharedWorker] [entity:ServiceWorker]
// @trace REQ-CLI-002
#![allow(dead_code, unused_imports)]
// REQ-BRW-001: Browser engine integration with servo
// REQ-BRW-004: Worker constructor bridging to Page Realm (DF-WK-11)
// REQ-BRW-4: Worker/SharedWorker/ServiceWorker constructors on JS global object
// REQ-CLI-002: bao browser 子命令 → servo 初始化 + CDP 端口输出
// REQ-LIB-004: BaoRuntime top-level coordinator
mod cdp_handler;
mod config;
mod delegate;
mod error;
mod page;
mod page_pool;
mod permission;
mod runtime_bridge;
mod screenshot;

pub use config::{BaoConfig, BrowserConfig, PageConfig};
pub use delegate::{
    crash_safe_teardown_worker, is_javascript_mime_type, AutoCloseWorker, BaoServoDelegate,
    BaoWebViewDelegate, BaoWebViewState, DedicatedWorkerGlobalScopeState, ServiceWorkerFetchEvent,
    ServiceWorkerFetchInterceptMode, ServiceWorkerGlobalScopeState, ServiceWorkerHandle,
    ServiceWorkerRegistrationId, ServiceWorkerRegistrationState, ServiceWorkerRegistrationTracking,
    ServiceWorkerScopeConfig, SharedWorkerChannelBridge, SharedWorkerConnectEvent,
    SharedWorkerGlobalScopeState, SharedWorkerHandle, SharedWorkerId, SharedWorkerPortChannel,
    SharedWorkerPortEndpoints, SharedWorkerPortRef, SharedWorkerScopeConfig,
    StructuredClonePayload, WorkerChannelBridge, WorkerChannelEndpoints, WorkerErrorEvent,
    WorkerGlobalScopeState, WorkerHandle, WorkerId, WorkerLifecycleState, WorkerLocation,
    WorkerMessageDirection, WorkerMessageEvent, WorkerNavigator, WorkerNetworkInformation,
    WorkerScopeConfig, WorkerScriptLoadError, WorkerScriptLoadResult, WorkerScriptLoadState,
    WorkerScriptLoader, WorkerScriptSource, WorkerScriptType, WorkerStructuredMessage,
    WorkerTeardownPath, WorkerTeardownResult,
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

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use servo::{Opts, Servo, ServoBuilder};

use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::bridge_channel;
use bao_cdp_client::bridge::{translate, ServoEvent};
use cdp_server::{
    CdpServer, DomainRegistry, EmptyHandler, EventBroadcaster, EventSender, ServerConfig,
};

// BAO PATCH (BCE-20260627-009): Process-global servo opts initialization.
// servo's `opts::initialize_options` uses an `OnceLock<Opts>` that panics on re-init,
// and `opts::get()` lazily fills it with `Default`. If ANY servo code calls `get()`
// before our explicit `initialize_options`, the OnceLock locks to Default and bao's
// config (force_isolate_event_loops=true) can never win.
//
// `BAO_SERVO_OPTS_INIT` is a `LazyLock` that runs `initialize_options` with bao's
// config on first access. `BaoRuntime::new` forces it (`.clone()` triggers init)
// BEFORE constructing `Servo`, winning the OnceLock race process-wide. Multi-instance
// safety: subsequent `BaoRuntime::new` calls hit the idempotent path in the patched
// `initialize_options` (same bao config → no-op).
static BAO_SERVO_OPTS_INIT: std::sync::LazyLock<()> = std::sync::LazyLock::new(|| {
    servo::opts::initialize_options(Opts {
        force_isolate_event_loops: true,
        disable_script_debugger: true,
        ..Opts::default()
    });
});

pub struct BaoRuntime {
    servo: Rc<Servo>,
    delegate: Rc<BaoServoDelegate>,
    page_pool: Rc<PagePool>,
    cdp_port: Option<u16>,
}

impl BaoRuntime {
    pub fn new(config: BaoConfig) -> Result<Self, BrowserError> {
        config.validate().map_err(BrowserError::Init)?;

        // Force-init servo's process-global OnceLock<Opts> BEFORE any servo code
        // calls get() (which would lazily lock in Default). This wins the race
        // against servo's get_or_init(Default::default).
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
        // re-initializing panics. Each `BaoRuntime::new` → `Servo::new` →
        // `initialize_options`. Multiple BaoRuntime instances (production
        // multi-tenant + concurrent integration tests) therefore collide.
        // Strategy:
        //   (1) Detect whether servo config is already initialized by reading
        //       `servo::opts::get()` (returns `&'static Opts`, never panics —
        //       falls back to `Default` via `get_or_init`).
        //   (2) `force_isolate_event_loops` is `false` in `Opts::default` but
        //       `true` in our desired config, so it is a reliable sentinel for
        //       "already initialized by a prior BaoRuntime".
        //   (3) On the already-initialized path we skip `.opts(...)` — but
        //       `Servo::new` still calls `initialize_options` internally, so the
        //       vendor-side patch (idempotent `initialize_options`) is the real
        //       guarantee. This bao-layer check just avoids passing conflicting
        //       opts when we know a prior instance already configured servo.
        let desired_opts = Opts {
            force_isolate_event_loops: true,
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
        // so observing `true` here means a prior BaoRuntime already won.
        let servo_already_initialized = servo::opts::is_initialized();
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
            // across multiple BaoRuntime instances.
            //
            // NOTE: we use `is_initialized()` (pure read, no side effect),
            // NOT `opts::get()`. `opts::get()` uses `get_or_init(Default)`,
            // which would itself populate the `OnceLock` with defaults on the
            // very first call — racing against `Servo::new`'s real
            // `initialize_options((true, true))` and causing a spurious
            // "conflicting config" panic.
            ServoBuilder::default()
                .opts(servo::opts::get().clone())
                .build()
        } else {
            ServoBuilder::default().opts(desired_opts).build()
        });

        let delegate = Rc::new(BaoServoDelegate::new());
        servo.set_delegate(Rc::clone(&delegate) as Rc<dyn servo::ServoDelegate>);

        let page_pool = Rc::new(PagePool::new(
            Rc::clone(&servo),
            Rc::clone(&delegate),
            &config,
        ));

        Ok(BaoRuntime {
            servo,
            delegate,
            page_pool,
            cdp_port: config.cdp_port,
        })
    }

    pub fn page_pool(&self) -> &Rc<PagePool> {
        &self.page_pool
    }

    pub fn create_page(&self, config: &PageConfig) -> Result<PageHandle, BrowserError> {
        let page = self.page_pool.create_page(config)?;

        // Drive servo's event loop until the WebView pipeline is ready.
        // Without this, inject_all_with_profile() → drain_callbacks() → evaluate_js_web()
        // will SIGSEGV because servo's script thread hasn't finished setting up
        // the pipeline for this WebView.
        page.wait_for_pipeline_ready(Duration::from_secs(5))?;

        runtime_bridge::inject_all_with_profile(&page, &config.stealth_profile)?;
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
    ///      `register_worker_scope_callback_native` (invoked at page-init time,
    ///      see `inject_all_with_profile`). The callback runs on the Worker
    ///      thread via the servo vendor patch `drain_worker_scope_callbacks`.
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
        // The scope callback (registered at page-init via
        // register_worker_scope_callback_native) inherits this profile onto
        // the Worker's DedicatedWorkerGlobalScope.
        // @trace REQ-BRW-004 [criterion:12..17] CRIT-STL-WK
        let scope_config = webview_state.borrow().worker_scope_config.clone();

        // Generate WorkerId
        let worker_id = crate::delegate::WorkerId(url.to_string());

        // Create WorkerHandle — tracks closing/terminated state via
        // Arc<AtomicBool> and the worker_global_addr for REALM_PROFILES
        // cleanup (criterion #18). The servo-native scope callback (registered
        // globally) writes the Worker's global address here on creation.
        // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
        let handle = WorkerHandle::new(url.to_string());

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
        self.delegate.set_console_log_tx(tx);
    }

    /// Set the structured event forwarding channel on the servo delegate.
    /// When set, servo callbacks push ServoEvent (Path B) as the primary event path.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn set_event_channel(&self, tx: std::sync::mpsc::Sender<ServoEvent>) {
        self.delegate.set_event_tx(tx);
    }

    pub fn run(&self) -> Result<(), BrowserError> {
        let max_wait = Duration::from_secs(300);
        let start = std::time::Instant::now();

        while start.elapsed() < max_wait {
            self.servo.spin_event_loop();
            self.page_pool.check_idle_pages();
            // Yield instead of sleep — servo spin_event_loop is non-blocking.
            std::thread::yield_now();
        }

        let _stats = self.page_pool.stats();

        Ok(())
    }

    /// Run with a CDP bridge that processes commands during the event loop.
    /// Also drains ServoEvent from the EventSubscriber path (Path B) and
    /// broadcasts translated CdpEvents via the shared EventBroadcaster.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn run_with_bridge(
        &self,
        bridge_rx: bao_cdp::servo_bridge::BridgeReceiver,
        servo_event_rx: std::sync::mpsc::Receiver<ServoEvent>,
        broadcaster: Arc<EventBroadcaster>,
    ) -> Result<(), BrowserError> {
        let max_wait = Duration::from_secs(3600);
        let start = std::time::Instant::now();

        while start.elapsed() < max_wait {
            self.servo.spin_event_loop();
            self.page_pool.check_idle_pages();

            // Process pending CDP bridge commands
            bridge_rx.drain(|cmd| cdp_handler::handle_bridge_command(cmd, &self.page_pool));

            // Drain ServoEvent from EventSubscriber (Path B) and broadcast
            // as CDP events via the shared EventBroadcaster.
            // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
            while let Ok(servo_event) = servo_event_rx.try_recv() {
                let cdp_events = translate(servo_event);
                for cdp_event in cdp_events {
                    broadcaster.send_event(&cdp_event.method, cdp_event.params);
                }
            }

            // Yield instead of sleep — check bridge commands more frequently.
            std::thread::yield_now();
        }

        Ok(())
    }
}

impl Drop for BaoRuntime {
    fn drop(&mut self) {
        self.page_pool.close_all();
    }
}

pub fn run_browser(config: BrowserConfig) -> Result<(), BrowserError> {
    let _stealth = config.stealth_profile.is_some();
    let url = config.url.clone();
    let bao_config: BaoConfig = config.into();
    let cdp_port = bao_config.cdp_port;

    let runtime = BaoRuntime::new(bao_config)?;

    // Create initial page
    let page_config = PageConfig {
        url: url.clone(),
        stealth_profile: None,
        ..Default::default()
    };
    let _page = runtime.create_page(&page_config)?;
    if let Some(ref page_url) = url {
        log::debug!("[bao] navigating to {}", page_url);
    }

    if let Some(port) = cdp_port {
        // Create bridge channel for CDP <-> servo communication
        let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));

        // Create console log forwarding channel: servo delegate → CDP Log domain
        let (console_tx, console_rx) = std::sync::mpsc::channel::<cdp_server::ConsoleMessage>();
        runtime.set_console_log_channel(console_tx);

        // Create EventSubscriber pair for structured ServoEvent path (Path B).
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        let (event_subscriber, servo_event_rx) = bao_cdp_client::bridge::EventSubscriber::new();
        runtime.set_event_channel(event_subscriber.sender());

        // Build CdpServer and extract the shared broadcaster BEFORE moving the
        // server into its thread. The broadcaster is Arc<EventBroadcaster> which
        // shares the same SessionMap — events sent via this broadcaster reach all
        // connected WebSocket sessions.
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        let registry = Arc::new(DomainRegistry::<EmptyHandler>::new());
        let config = ServerConfig::builder().host("127.0.0.1").port(port).build();
        let target_id = format!(
            "{:016x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        );
        let mut server = CdpServer::with_registry(config, registry);
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

        let _handle = std::thread::spawn(move || {
            let _ = server.run();
        });

        let result = runtime.run_with_bridge(bridge_rx, servo_event_rx, broadcaster);
        _handle.thread().unpark();
        return result;
    }

    runtime.run()
}
