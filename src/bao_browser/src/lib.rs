#![allow(dead_code, unused_imports)]
// REQ-BRW-001: Browser engine integration with servo
// REQ-LIB-004: BaoRuntime top-level coordinator
mod config;
mod delegate;
mod error;
mod page;
mod page_pool;
mod permission;
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


pub struct BaoRuntime {
    servo: Rc<Servo>,
    delegate: Rc<BaoServoDelegate>,
    page_pool: Rc<PagePool>,
    cdp_port: Option<u16>,
}

impl BaoRuntime {
    pub fn new(config: BaoConfig) -> Result<Self, BrowserError> {
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
        self.page_pool.create_page(config)
    }

    pub fn spin_event_loop(&self) {
        self.servo.spin_event_loop();
    }

    pub fn run(&self) -> Result<(), BrowserError> {
        let max_wait = Duration::from_secs(30);
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
}

impl Drop for BaoRuntime {
    fn drop(&mut self) {
        self.page_pool.close_all();
    }
}

pub fn run_browser(config: BrowserConfig) -> Result<(), BrowserError> {
    let bao_config: BaoConfig = config.into();
    let cdp_port = bao_config.cdp_port;

    let runtime = BaoRuntime::new(bao_config)?;

    if let Some(port) = cdp_port {
        let handle = std::thread::spawn(move || {
            let mut cdp = bao_cdp::CDPServer::new(port);
            let _ = cdp.run();
        });
        let result = runtime.run();
        handle.thread().unpark();
        return result;
    }

    runtime.run()
}
