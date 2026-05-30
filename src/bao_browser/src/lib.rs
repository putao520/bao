// @trace REQ-BRW-001
#![allow(dead_code, unused_imports)]
// REQ-BRW-001: Browser engine integration with servo
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
pub use delegate::{BaoServoDelegate, BaoWebViewDelegate};
pub use error::BrowserError;
pub use page::{PageHandle, PageState};
pub use page_pool::PagePool;
pub use permission::{Permission, PermissionDenied, PermissionGuard};
pub use screenshot::{encode_image, ScreenshotFormat};

use std::rc::Rc;
use std::time::Duration;

use servo::{
    Opts, Servo, ServoBuilder,
};

use bao_cdp::servo_bridge::bridge_channel;
use bao_cdp::{CdpServer, ServerConfig};
use bao_cdp::domains::{register_all_domains_into, ServoTargetProvider};


pub struct BaoRuntime {
    servo: Rc<Servo>,
    delegate: Rc<BaoServoDelegate>,
    page_pool: Rc<PagePool>,
    cdp_port: Option<u16>,
}

impl BaoRuntime {
    pub fn new(config: BaoConfig) -> Result<Self, BrowserError> {
        config.validate().map_err(BrowserError::Init)?;

        let servo: Rc<Servo> = Rc::new(
            ServoBuilder::default()
                .opts(Opts::default())
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
        runtime_bridge::inject_all_with_profile(&page, &config.stealth_profile)?;
        Ok(page)
    }

    pub fn spin_event_loop(&self) {
        self.servo.spin_event_loop();
    }

    pub fn run(&self) -> Result<(), BrowserError> {
        let max_wait = Duration::from_secs(300);
        let start = std::time::Instant::now();

        while start.elapsed() < max_wait {
            self.servo.spin_event_loop();
            self.page_pool.check_idle_pages();
            std::thread::sleep(Duration::from_millis(10));
        }

        let stats = self.page_pool.stats();
        eprintln!("=== Bao Runtime ===");
        eprintln!("Pages: {}/{} active/idle", stats.active, stats.idle);
        eprintln!("Total created: {}", stats.total_created);
        eprintln!("Total destroyed: {}", stats.total_destroyed);

        Ok(())
    }

    /// Run with a CDP bridge that processes commands during the event loop.
    pub fn run_with_bridge(
        &self,
        bridge_rx: bao_cdp::servo_bridge::BridgeReceiver,
        active_page: &PageHandle,
    ) -> Result<(), BrowserError> {
        let max_wait = Duration::from_secs(3600);
        let start = std::time::Instant::now();

        while start.elapsed() < max_wait {
            self.servo.spin_event_loop();
            self.page_pool.check_idle_pages();

            // Process pending CDP bridge commands
            bridge_rx.drain(|cmd| cdp_handler::handle_bridge_command(cmd, active_page));

            std::thread::sleep(Duration::from_millis(5));
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
    let page = runtime.create_page(&page_config)?;
    if let Some(ref page_url) = url {
        eprintln!("[bao] navigating to {}", page_url);
    }

    if let Some(port) = cdp_port {
        // Create bridge channel for CDP <-> servo communication
        let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));

        let handle = std::thread::spawn(move || {
            let config = ServerConfig::builder()
                .host("127.0.0.1")
                .port(port)
                .build();
            let mut server = CdpServer::new(config);
            register_all_domains_into(bridge_tx.clone(), server.registry());
            let provider = std::sync::Arc::new(
                ServoTargetProvider::new(bridge_tx, "127.0.0.1".into(), port)
            );
            server.set_target_provider(provider);
            let _ = server.run();
        });

        let result = runtime.run_with_bridge(bridge_rx, &page);
        handle.thread().unpark();
        return result;
    }

    runtime.run()
}
