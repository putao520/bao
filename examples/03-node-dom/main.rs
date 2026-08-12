//! Example 03 — Node.js API × DOM 同一 runtime 共存(Bao 核心卖点)。
//!
//! 同一段 JS 脚本里:
//!   - `document.querySelector` 抽取 DOM(servo 提供)
//!   - `require('fs').readFileSync` 读本地文件(Bao 的 Node.js 兼容层提供)
//!
//! 这之所以能工作,是因为 `evaluate_js` 在 Node Realm 执行,该 Realm 的 global
//! 同时挂了 DOM 对象和 Node.js host functions。Page Realm(`evaluate_js_web`)
//! 则严格隔离,只有 Web API。

use std::time::Duration;

use bao::{BaoConfig, BrowserError, PageConfig};

fn main() -> Result<(), BrowserError> {
    // 1. 准备一个本地文件给 Node.js 的 fs 读
    std::fs::write("demo.txt", "hello from fs\n")
        .map_err(|e| BrowserError::Rendering(format!("write demo.txt: {e}")))?;
    println!("[03-node-dom] Writing a local file via Rust std::fs ...");

    // 2. 创建 runtime + page
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    println!("[03-node-dom] BaoRuntime ready");
    let page = runtime.create_page(&PageConfig::default())?;
    println!("[03-node-dom] Page created (id={})", page.id());

    // 3. 导航到一个有 DOM 的真实页面
    println!("[03-node-dom] Navigating to https://example.com ...");
    page.navigate("https://example.com")?;
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;

    // 4. ★ 核心:在 Node Realm 执行同时调 DOM + Node API 的脚本
    //    这段 JS 一次返回多个值,用 console.log 直接打到 stdout 便于查看。
    //    (不依赖 Rust 侧解析 JSON,保持示例零额外依赖)
    println!("[03-node-dom] Dual-realm JS executing ...");
    let dual_realm_script = r#"
        // DOM 抽取(servo 提供)
        const h1 = document.querySelector('h1')?.textContent ?? "(no h1)";

        // Node.js API(Bao 的 bun_runtime 兼容层提供)
        const fs   = require('fs');
        const file = fs.readFileSync('demo.txt', 'utf8');

        // Bun 全局对象(别名 Bao,同一对象)
        const hasBun = typeof Bun === 'object';

        // process 对象(Node.js 兼容)
        const hasNode = typeof process === 'object';

        console.log("[03-node-dom]   ↳ document.querySelector('h1') =", JSON.stringify(h1.trim()));
        console.log("[03-node-dom]   ↳ require('fs').readFileSync  =", JSON.stringify(file.trim()));
        console.log("[03-node-dom]   ↳ typeof Bun === 'object'     =", hasBun);
        console.log("[03-node-dom]   ↳ typeof process === 'object' =", hasNode);

        "ok";
    "#;
    let out = page.evaluate_js(dual_realm_script)?;
    println!("[03-node-dom] evaluate_js returned: {}", out);

    // 5. 对比:Page Realm 完全看不到 Node.js
    let page_realm_check = page.evaluate_js_web("typeof require")?;
    println!(
        "[03-node-dom] Page Realm check (evaluate_js_web): typeof require = {}",
        page_realm_check
    );
    println!("[03-node-dom] Done");
    Ok(())
}
