//! # 示例:基本用法 — 连接内嵌 servo + 导航 + 截图
//!
//! 演示 bao_cdp_client 最常见的最小工作流:
//! 1. `Browser::connect("memory://bao")` 路由到 InMemoryTransport
//! 2. 创建 [`Browser`] 句柄(实际握手由 build_transport 触发)
//! 3. 查询基本信息(URL / scheme / transport_kind)
//!
//! ## 运行
//!
//! ```sh
//! cargo run --example basic_usage -p bao_cdp_client
//! ```
//!
//! ## 输出
//!
//! ```text
//! Connected: Browser(memory://bao, kind=InMemory)
//! Scheme:    memory
//! Kind:      InMemory
//! ```
//!
//! @trace REQ-BAO-API-001 [level:library]
//! @trace REQ-BAO-API-008 [level:library]

use bao_cdp_client::Browser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // memory:// scheme 自动路由到 InMemoryTransport
    // (servo WebView 通过 CDPRdpBridge 桥接,同进程零网络往返)。
    let browser = Browser::connect("memory://bao")?;

    println!("Connected: {}", browser);
    println!("Scheme:    {}", browser.scheme());
    println!("Kind:      {:?}", browser.transport_kind());

    // 验证 transport 类型。
    assert!(browser.is_in_memory());
    assert!(!browser.is_websocket());

    // 真实场景下,这里会调用:
    //   let transport = browser.build_in_memory_transport(my_servo_bridge)?;
    //   let conn = Connection::new(ConnectionConfig::default());
    //   let page = conn.new_page("https://example.com").await?;
    //   let screenshot = page.screenshot().await?;
    //
    // 完整的 servo 桥接集成见 bao_browser::PagePool 实现。

    println!("\nExample completed successfully.");
    Ok(())
}
