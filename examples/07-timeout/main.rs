//! Example 07 — 执行超时(ExecutionControl 面)。
//!
//! `evaluate_js[_web]_with_timeout` 给任意脚本一个引擎级硬超时:
//! 超时经 SpiderMonkey 中断回调实现,死循环也会被掐断并返回稳定的
//! 超时语义(不是外层轮询杀线程)。
//! 全程 about:blank,零外部网络依赖。

use std::time::Duration;

use bao::{BrowserRuntime, BaoConfig, BrowserError, PageConfig};

fn main() -> Result<(), BrowserError> {
    let runtime = BrowserRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    })?;
    page.wait_for_pipeline_ready(Duration::from_secs(15))?;

    // 1. 正常脚本 + 宽裕超时:不受影响
    let v = page.evaluate_js_with_timeout("6 * 7", Some(Duration::from_secs(5)))?;
    println!("[07-timeout] bounded eval ok: {v}");
    assert_eq!(v.trim(), "42");

    // 2. 死循环 + 500ms 超时:被引擎中断,返回稳定的超时错误
    let t = std::time::Instant::now();
    let err = page
        .evaluate_js_with_timeout("while (true) {}", Some(Duration::from_millis(500)))
        .expect_err("runaway script must be terminated");
    println!(
        "[07-timeout] runaway terminated in {:?} → {err}",
        t.elapsed()
    );
    // 错误文本携带超时语义(ISSUE #24 的稳定 message 契约)
    assert!(err.to_string().to_lowercase().contains("timeout"));

    // 3. 引擎在超时后仍然健康:下一次求值照常工作
    let v = page.evaluate_js_with_timeout("'alive'", Some(Duration::from_secs(5)))?;
    println!("[07-timeout] engine healthy after termination: {v}");
    assert_eq!(v.trim_matches('"'), "alive");

    // 4. 同一面也存在于 Web Realm(page 不可信 JS 的对应入口)
    let v = page.evaluate_js_web_with_timeout("2 + 3", Some(Duration::from_secs(5)))?;
    println!("[07-timeout] web-realm bounded eval: {v}");
    assert_eq!(v.trim(), "5");

    // 5. Node Realm 面(evaluate_js 的默认入口)同样受控——CLI 的
    //    `--timeout` 只是这一层的前缀包装
    let err = page
        .evaluate_js_with_timeout(
            "(function () { var s = 0; while (true) { s++; } })()",
            Some(Duration::from_millis(500)),
        )
        .expect_err("node-realm runaway must be terminated");
    println!("[07-timeout] node-realm runaway terminated → {err}");

    println!("[07-timeout] Done");
    Ok(())
}
