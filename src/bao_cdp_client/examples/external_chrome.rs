//! # 示例:连接外部 Chrome(WebSocket transport)
//!
//! 演示如何通过 `ws://` 连接到一个正在运行的 Chrome / Chromium 实例
//! (用 `--remote-debugging-port=9222` 启动)。
//!
//! ## 启动 Chrome
//!
//! ```sh
//! google-chrome \
//!   --headless \
//!   --remote-debugging-port=9222 \
//!   --disable-gpu \
//!   --no-sandbox
//! ```
//!
//! ## 运行
//!
//! ```sh
//! cargo run --example external_chrome -p bao_cdp_client
//! ```
//!
//! ## 输出
//!
//! ```text
//! WebSocket URL: ws://127.0.0.1:9222
//! Scheme:        ws
//! Kind:          WebSocket
//! Ready to dispatch CDP commands...
//! ```
//!
//! ## 实际场景
//!
//! 连接成功后,典型工作流:
//!
//! ```no_run
//! use bao_cdp_client::Browser;
//! use bao_cdp_client::types::ScreenshotFormat;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let browser = Browser::connect("ws://127.0.0.1:9222")?;
//!     let mut transport = browser.build_websocket_transport()?;
//!
//!     // 发送 CDP 命令(Target.createTarget → 新建 tab)
//!     let resp = transport.send_command(
//!         "Target.createTarget",
//!         serde_json::json!({ "url": "https://example.com" }),
//!         None,
//!     )?;
//!     let target_id = resp["targetId"].as_str().unwrap();
//!     println!("Created target: {}", target_id);
//!
//!     Ok(())
//! }
//! ```
//!
//! @trace REQ-BAO-API-001 [level:library]
//! @trace REQ-BAO-API-002 [interface:Transport]
//! @trace REQ-BAO-API-008 [level:library]

use bao_cdp_client::Browser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ws:// 直连 — Browser::connect 仅解析 URL,实际握手由 build_websocket_transport 触发。
    let browser = Browser::connect("ws://127.0.0.1:9222")?;

    println!("WebSocket URL: {}", browser.url());
    println!("Scheme:        {}", browser.scheme());
    println!("Kind:          {:?}", browser.transport_kind());

    assert!(browser.is_websocket());
    assert!(!browser.is_in_memory());

    // 如果 Chrome 实际在 9222 端口监听,可以取消注释下面的代码做真实连接:
    //
    // let mut transport = browser.build_websocket_transport()?;
    // let resp = transport.send_command("Browser.getVersion", serde_json::json!({}), None)?;
    // println!("Browser version: {}", resp["product"]);

    println!("Ready to dispatch CDP commands...");
    println!("\nExample completed successfully.");
    Ok(())
}
