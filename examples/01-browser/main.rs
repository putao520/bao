//! Example 01 — 最基本的 servo 浏览器嵌入。
//!
//! 展示 Bao 的最小编码路径:
//!   BaoRuntime::new → create_page → navigate → evaluate_js_web → take_screenshot
//!
//! 这个示例不调用 Node.js API(那是示例 03 的事),只用 Page Realm 的 Web API。
//! Page Realm 上的 `document` / `navigator` 等同于 servo 原生浏览器环境。

use std::time::Duration;

use bao::{BaoConfig, BrowserError, PageConfig, PageState, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    // 1. 创建 Bao 浏览器 runtime(单进程 servo + SpiderMonkey + 内置 CDP/Node/Stealth)
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    println!("[01-browser] BaoRuntime ready");

    // 2. 在 runtime 中创建一个 page(PagePool 会持有它的生命周期)
    let page = runtime.create_page(&PageConfig::default())?;
    println!("[01-browser] Page created (id={})", page.id());

    // 3. 导航到一个真实 URL
    println!("[01-browser] Navigating to https://example.com ...");
    page.navigate("https://example.com")?;

    // 4. 等待 servo 的 pipeline 就绪(frame_ready + drain_callbacks)。
    //    跳过这步直接 evaluate_js_web 会触发 SIGSEGV,这是 servo 的硬约束。
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;
    println!(
        "[01-browser] URL after navigate: {}",
        page.current_url().unwrap_or_default()
    );

    // 5. 在 Page Realm 执行 JS(只有 Web API,没有 Node 的 require/fs)
    let title = page.evaluate_js_web("document.title")?;
    println!("[01-browser] <title> = {}", title.trim_matches('"'));
    let ua = page.evaluate_js_web("navigator.userAgent")?;
    println!("[01-browser] navigator.userAgent (Page Realm) = {}", ua);

    // 6. 截图(内存渲染 → RgbaImage → PNG 编码)
    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let out_path = "bao-01-browser.png";
    std::fs::write(out_path, &png)
        .map_err(|e| BrowserError::Rendering(format!("write screenshot failed: {e}")))?;
    println!(
        "[01-browser] Screenshot saved: {out_path} ({} bytes)",
        png.len()
    );

    // 7. 查看页面状态
    let state: PageState = page.get_state();
    println!("[01-browser] Page state: {:?}", state);
    println!(
        "[01-browser] Page title (via page_title()): {:?}",
        page.page_title()
    );

    println!("[01-browser] Done");
    Ok(())
}
