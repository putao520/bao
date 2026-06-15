//! # 示例:多页面管理 — Page pool 与 BrowserContext
//!
//! 演示 bao_cdp_client 高层 API 的多 tab 管理:
//! - 一个 [`Browser`] → 多个 [`BrowserContext`](incognito-like 隔离)
//! - 一个 [`BrowserContext`] → 多个 [`Page`](独立 tab/iframe)
//! - 通过 [`types::Viewport`] 控制每个 Page 的视口
//!
//! ## 用例
//!
//! 典型场景:
//! 1. 爬虫:一个 BrowserContext 持久化 cookie,另一个 incognito 模式隔离测试
//! 2. UI 测试:多个 Page 同时打开不同 viewport(桌面 / 移动)
//! 3. 服务端渲染:Page pool 复用,降低冷启动开销
//!
//! ## 完整代码(伪)
//!
//! ```no_run
//! use bao_cdp_client::Browser;
//! use bao_cdp_client::types::Viewport;
//! use std::rc::Rc;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let browser = Browser::connect("memory://bao")?;
//!     // 高层 Browser 句柄(持有 transport + 状态):
//!     // let high_level = HighLevelBrowser::from_transport(transport);
//!     // let ctx_default = high_level.default_context();
//!     // let ctx_incog = high_level.new_context(Default::default())?;
//!
//!     // let mobile_viewport = Viewport {
//!     //     width: 390, height: 844,
//!     //     device_scale_factor: 3.0,
//!     //     is_mobile: true,
//!     //     has_touch: true,
//!     //     is_landscape: false,
//!     // };
//!     // let desktop_viewport = Viewport::default();
//!     //
//!     // let page1 = ctx_incog.new_page("https://m.example.com", mobile_viewport)?;
//!     // let page2 = ctx_default.new_page("https://example.com", desktop_viewport)?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## 运行
//!
//! ```sh
//! cargo run --example multi_page -p bao_cdp_client
//! ```
//!
//! @trace REQ-BAO-API-006 [class:Page]
//! @trace REQ-BAO-API-008 [level:library]

use bao_cdp_client::types::Viewport;
use bao_cdp_client::Browser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 演示 Viewport 配置(不同设备)
    let mobile = Viewport {
        width: 390,
        height: 844,
        device_scale_factor: 3.0,
        is_mobile: true,
        has_touch: true,
        is_landscape: false,
    };
    let desktop = Viewport::default();

    println!("Mobile viewport:  {}x{} (scale={})",
        mobile.width, mobile.height, mobile.device_scale_factor);
    println!("Desktop viewport: {}x{} (scale={})",
        desktop.width, desktop.height, desktop.device_scale_factor);

    // 路由校验
    let browser = Browser::connect("memory://bao")?;
    println!("\nBrowser transport: {:?}", browser.transport_kind());
    assert!(browser.is_in_memory());

    // 真实场景下,这里会:
    //   let ctx_default = browser.default_context();
    //   let ctx_incog = browser.new_context(Default::default())?;
    //   let pages: Vec<Rc<Page>> = (0..4).map(|_| ctx_default.new_page(...)).collect();

    println!("\nExample completed successfully.");
    Ok(())
}
