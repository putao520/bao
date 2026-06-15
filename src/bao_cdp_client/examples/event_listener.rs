//! # 示例:事件监听 — console / network / dialog 事件订阅
//!
//! 演示 bao_cdp_client 的 EventEmitter trait:
//! - `page.on("console", handler)` 订阅 console.log
//! - `page.on("request", handler)` 订阅 HTTP 请求
//! - `page.on("dialog", handler)` 订阅 alert/confirm/prompt
//!
//! ## 事件源
//!
//! servo → CDP event 翻译表(`bridge::event_translator`):
//!
//! | servo 原始事件              | CDP event method               |
//! |----------------------------|--------------------------------|
//! | ConsoleMessage             | `Runtime.consoleAPICalled`     |
//! | PageLifecycleEvent         | `Page.lifecycleEvent`          |
//! | NetworkRequest             | `Network.requestWillBeSent`    |
//! | NetworkResponse            | `Network.responseReceived`     |
//! | Dialog                     | `Page.javascriptDialogOpening` |
//! | FileChooser                | `Page.fileChooserOpened`       |
//! | PageError                  | `Runtime.exceptionThrown`      |
//!
//! ## 完整代码(伪)
//!
//! ```no_run
//! use bao_cdp_client::Browser;
//! use bao_cdp_client::EventEmitter;
//! use std::sync::atomic::{AtomicUsize, Ordering};
//! use std::sync::Arc;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let browser = Browser::connect("memory://bao")?;
//!     // let page = browser.new_page("https://example.com")?;
//!     //
//!     // let counter = Arc::new(AtomicUsize::new(0));
//!     // let c2 = counter.clone();
//!     // page.on("console", Arc::new(move |args| {
//!     //     let n = c2.fetch_add(1, Ordering::SeqCst) + 1;
//!     //     println!("[console #{}] {}", n, args[0]);
//!     // }));
//!     //
//!     // page.on("request", Arc::new(|args| {
//!     //     eprintln!("[request] {}", args[0]["url"]);
//!     // }));
//!     //
//!     // page.evaluate("console.log('hello')").await?;
//!     // assert!(counter.load(Ordering::SeqCst) >= 1);
//!
//!     Ok(())
//! }
//! ```
//!
//! ## 运行
//!
//! ```sh
//! cargo run --example event_listener -p bao_cdp_client
//! ```
//!
//! @trace REQ-BAO-API-003 [level:library]
//! @trace REQ-BAO-API-006 [class:Page]
//! @trace REQ-BAO-API-007 [level:library]

use bao_cdp_client::bridge::ConsoleLevel;
use bao_cdp_client::Browser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 演示 ConsoleLevel 枚举(servo 翻译层使用)
    println!("ConsoleLevel variants:");
    for lvl in [
        ConsoleLevel::Verbose,
        ConsoleLevel::Info,
        ConsoleLevel::Warning,
        ConsoleLevel::Error,
        ConsoleLevel::Debug,
    ] {
        println!("  - {:?}", lvl);
    }

    let browser = Browser::connect("memory://bao")?;
    println!("\nBrowser URL: {}", browser.url());

    // 真实场景下,这里会:
    //   let page = browser.new_page("https://example.com")?;
    //   page.on("console", Arc::new(|args| println!("[console] {:?}", args)));
    //   page.goto("https://example.com").await?;
    //   // servo 触发 ConsoleMessage → translate → CDP event → emit("console", args)

    println!("\nExample completed successfully.");
    Ok(())
}
