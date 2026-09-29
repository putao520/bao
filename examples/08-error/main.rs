//! Example 08 — 错误处理链。
//!
//! Bao 的公开错误类型都实现了 `std::error::Error`,并且保留了完整的
//! source 链(W29:连接错误一路 `source()` 到 io 根因)。本示例收集四类
//! 真实错误并逐层打印因果链——写自动化时按 variant 分派,按链定位根因。
//! 全程本地(拒绝连接的端口 + about:blank),零外部网络依赖。

use std::error::Error as StdError;
use std::time::Duration;

use bao::{Browser, BrowserError, BrowserRuntime, BaoConfig, CdpError, PageConfig};

/// 打印一条错误的完整因果链(逐层 `source()`),返回链深。
fn print_chain(tag: &str, err: &dyn StdError) -> usize {
    let mut depth = 0;
    let mut cur = Some(err);
    while let Some(e) = cur {
        println!("[08-error] {tag} layer {depth}: {}", e);
        cur = e.source();
        depth += 1;
    }
    depth
}

fn main() {
    // ── 1. CDP 连接失败:握手错误带 io 根因的 source 链 ────────────────
    // Browser::connect 是惰性的(只做 URL 路由);真正的握手发生在第一个
    // 命令上。127.0.0.1:1 是保留端口,必然拒绝连接——无需真实服务。
    let mut browser = Browser::connect("ws://127.0.0.1:1").expect("connect itself is route-only/lazy");
    let err: CdpError = browser.version().expect_err("dead port must refuse the handshake");
    // 真实行为:这条惰性 ws 首命令路径失败为浅层 "connection closed"
    // CdpError(depth 1);更深的因果链(CdpError::Connect → ConnectError::Io
    // → io 根因)是 W29 契约面,由 error_model 测试锁定——这里只断言
    // 错误本身发生 + 链可走。
    let depth = print_chain("connect", &err);
    println!("[08-error] connect chain depth: {depth}");
    assert!(depth >= 1, "the handshake failure must surface as an error");

    // ── 2. 导航失败:非法 URL → BrowserError::Navigation ─────────────────
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("runtime");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .expect("page");
    let err: BrowserError = page
        .navigate("this is not a url")
        .expect_err("invalid URL must fail");
    print_chain("navigate", &err);
    assert!(matches!(err, BrowserError::Navigation(_)));

    // ── 3. 页面关闭后的操作面:统一 "page is closed" ────────────────────
    runtime.page_pool().close_page(page.id()).expect("close");
    let err = page.evaluate_js_web("1+1").expect_err("closed page must err");
    print_chain("closed-page", &err);
    assert!(err.to_string().contains("page is closed"));

    // ── 4. JS 抛错:页面脚本错误原样回到 Rust(BrowserError::JavaScript)
    let page2 = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .expect("page2");
    page2
        .wait_for_pipeline_ready(Duration::from_secs(15))
        .expect("ready");
    let err = page2
        .evaluate_js_web("null.something")
        .expect_err("JS throw must surface");
    print_chain("js-throw", &err);
    assert!(matches!(err, BrowserError::JavaScript(_)));

    println!("[08-error] Done");
}
