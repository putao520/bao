//! Example 05 — 多页面管理(PagePool)。
//!
//! 一个 BrowserRuntime 里并发持有多个 page:交错导航、独立求值、
//! 池级统计(active/idle/created/destroyed)与 idle 归还。
//! 全程 data: URL,零外部网络依赖。

use std::time::Duration;

use bao::{BrowserRuntime, BaoConfig, BrowserError, PageConfig};

fn main() -> Result<(), BrowserError> {
    // 1. Runtime(每个 page 一个 ScriptThread/JSContext,页间完全隔离)
    let runtime = BrowserRuntime::new(BaoConfig::default())?;

    // 2. 两个 page,不同初始内容
    let p1 = runtime.create_page(&PageConfig {
        url: Some("data:text/html,<title>page-one</title><h1 id=a>ONE</h1>".into()),
        ..Default::default()
    })?;
    let p2 = runtime.create_page(&PageConfig {
        url: Some("data:text/html,<title>page-two</title><h1 id=a>TWO</h1>".into()),
        ..Default::default()
    })?;
    p1.wait_for_pipeline_ready(Duration::from_secs(15))?;
    p2.wait_for_pipeline_ready(Duration::from_secs(15))?;

    // 3. 交错求值:同样的选择器,两页各自返回自己的内容(隔离证明)
    let t1 = p1.evaluate_js_web("document.title")?;
    let t2 = p2.evaluate_js_web("document.title")?;
    let h1 = p1.evaluate_js_web("document.getElementById('a').textContent")?;
    let h2 = p2.evaluate_js_web("document.getElementById('a').textContent")?;
    println!("[05-multi-page] p1: title={t1} h1={h1}");
    println!("[05-multi-page] p2: title={t2} h1={h2}");
    assert!(t1.contains("page-one") && t2.contains("page-two"));
    assert!(h1.contains("ONE") && h2.contains("TWO"));

    // 4. 池级统计:PagePool 的权威账本
    let stats = runtime.page_pool().stats();
    println!(
        "[05-multi-page] pool: active={} idle={} created={} destroyed={}",
        stats.active, stats.idle, stats.total_created, stats.total_destroyed
    );
    assert_eq!((stats.active, stats.idle, stats.total_created), (2, 0, 2));

    // 5. 归还一页到 idle 池(不销毁,可再取回)
    runtime.page_pool().release_page(p1.id());
    let stats = runtime.page_pool().stats();
    println!(
        "[05-multi-page] after release(p1): active={} idle={}",
        stats.active, stats.idle
    );
    assert_eq!((stats.active, stats.idle), (1, 1));

    // 6. 再取回:idle → active,页面状态原样
    let p1_back = runtime
        .page_pool()
        .get_page(p1.id())
        .ok_or_else(|| BrowserError::Init("get_page from idle failed".into()))?;
    let again = p1_back.evaluate_js_web("document.title")?;
    println!("[05-multi-page] p1 reacquired: title={again} (content survived idle)");
    assert!(again.contains("page-one"));

    // 7. 收尾:逐页关闭(池计数随之归零),runtime drop 时兜底 close_all
    runtime.page_pool().close_page(p1.id())?;
    runtime.page_pool().close_page(p2.id())?;
    let stats = runtime.page_pool().stats();
    println!(
        "[05-multi-page] after close: active={} idle={} destroyed={}",
        stats.active, stats.idle, stats.total_destroyed
    );
    assert_eq!((stats.active, stats.idle, stats.total_destroyed), (0, 0, 2));

    println!("[05-multi-page] Done");
    Ok(())
}
