//! Example 06 — 优雅关闭。
//!
//! 关闭序三件事:
//!   1. 让页面的 pending work 真实落定(web 定时器 + promise)
//!   2. 逐页 close(Worker 终止 → 注册表清理 → PageState Closed)
//!   3. runtime drop(close_all 兜底 + W27 有界 join,不悬挂)
//! 全程 data: URL,零外部网络依赖。

use std::time::Duration;

use bao::{BrowserRuntime, BaoConfig, BrowserError, PageConfig, PageState};

fn main() -> Result<(), BrowserError> {
    let runtime = BrowserRuntime::new(BaoConfig::default())?;

    // 1. 一个带 pending work 的页面:web 定时器链 + 异步 fetch 标志
    let page = runtime.create_page(&PageConfig {
        url: Some("data:text/html,<title>shutdown-demo</title>".into()),
        ..Default::default()
    })?;
    page.wait_for_pipeline_ready(Duration::from_secs(15))?;

    // 2. arm 一条 web 定时器链(servo TimerScheduler 驱动)
    page.evaluate_js_web(
        "globalThis.__done = 0; \
         setTimeout(function() { setTimeout(function() { globalThis.__done = 1; }, 20); }, 20); \
         'armed'",
    )?;

    // 3. drain:驱动事件循环直到 pending work 落定(优雅关闭的前提是
    //    先让页面自己的异步面走完)
    let settled = page.wait_for_function("globalThis.__done === 1", Duration::from_secs(10));
    println!("[06-shutdown] pending work drained: {settled:?}");
    settled?;

    // 4. 逐页关闭:状态机走 Closing → Closed,Worker/注册表随之清理
    let id = page.id();
    runtime.page_pool().close_page(id)?;
    let state = page.get_state();
    println!("[06-shutdown] page #{id} state after close: {state:?}");
    assert_eq!(state, PageState::Closed);

    // 5. 池账本归零
    let stats = runtime.page_pool().stats();
    println!(
        "[06-shutdown] pool after close: active={} idle={} destroyed={}",
        stats.active, stats.idle, stats.total_destroyed
    );
    assert_eq!((stats.active, stats.idle, stats.total_destroyed), (0, 0, 1));

    // 6. runtime drop:close_all 兜底 + servo join。ServoInner::drop 的
    //    join 自旋有 15s 上限(W27):即使某线程卡死,teardown 也有界、
    //    泄漏会被如实记录,不会无限悬挂。
    let t = std::time::Instant::now();
    drop(runtime);
    println!("[06-shutdown] runtime dropped in {:?}", t.elapsed());

    println!("[06-shutdown] Done");
    Ok(())
}
