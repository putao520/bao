// @trace REQ-BRW-001 [entity:BrowserContext] [entity:PageHandle]
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope]
// @trace REQ-CLI-002
#![allow(dead_code, unused_imports)]
// REQ-BRW-001: Browser engine integration with servo
// REQ-BRW-004: Worker constructor bridging to Page Realm (DF-WK-11)
// REQ-CLI-002: bao browser 子命令 → servo 初始化 + CDP 端口输出
// REQ-LIB-004: BaoRuntime top-level coordinator
mod config;
mod cdp_handler;
mod delegate;
mod error;
mod page;
mod page_pool;
mod permission;
mod runtime_bridge;
mod screenshot;

pub use config::{BaoConfig, BrowserConfig, PageConfig};
pub use delegate::{
    BaoServoDelegate, BaoWebViewDelegate, WorkerHandle, WorkerId,
    WorkerMessageDirection, WorkerMessageEvent,
    WorkerErrorEvent, WorkerLifecycleState, WorkerTeardownPath,
    WorkerScopeConfig, AutoCloseWorker,
};
pub use error::BrowserError;
pub use page::{PageHandle, PageState};
pub use page_pool::PagePool;
pub use permission::{Permission, PermissionDenied, PermissionGuard};
pub use screenshot::{encode_image, ScreenshotFormat};
pub use runtime_bridge::{BridgeChannel, BridgeCommand, BridgeReceiver, BridgeResponse, EvaluateResult, RuntimeBridge};

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use servo::{
    Opts, Servo, ServoBuilder,
};

use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::bridge_channel;
use bao_cdp_client::bridge::{translate, ServoEvent};
use cdp_server::{CdpServer, DomainRegistry, EmptyHandler, EventBroadcaster, EventSender, ServerConfig};

// Force-link bao_native_stubs (dispatch no-op stubs + C library bridges).
// Without this anchor, the linker GCs the entire bao_native_stubs compilation
// unit, causing undefined __bun_dispatch__* and C symbol errors in test binaries.
#[used]
static BAO_NATIVE_STUBS_ANCHOR: unsafe extern "C" fn() = bao_native_stubs::__force_link_entry;

pub struct BaoRuntime {
    servo: Rc<Servo>,
    delegate: Rc<BaoServoDelegate>,
    page_pool: Rc<PagePool>,
    cdp_port: Option<u16>,
}

impl BaoRuntime {
    pub fn new(config: BaoConfig) -> Result<Self, BrowserError> {
        config.validate().map_err(BrowserError::Init)?;

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
        let servo: Rc<Servo> = Rc::new(
            ServoBuilder::default()
                .opts(Opts {
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
                })
                .build(),
        );

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
        let config = ServerConfig::builder()
            .host("127.0.0.1")
            .port(port)
            .build();
        let target_id = format!("{:016x}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64);
        let mut server = CdpServer::with_registry(config, registry);
        let provider = Arc::new(
            ServoTargetProvider::new(bridge_tx, target_id, "127.0.0.1".into(), port)
        );
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
