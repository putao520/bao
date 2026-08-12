//! Example 04 — 服务端网页自动化 / 爬虫。
//!
//! 展示用 Bao 做完整的 server-side automation 流程:
//!   1. 导航到目标页
//!   2. 用 Node Realm 的 evaluate_js 一次抽取 DOM 结构 + Node fetch 下载资源
//!   3. 用 Node fs 把报告写盘
//!
//! 全程「浏览器渲染 + Node.js API」在同一个 SpiderMonkey runtime 内,
//! Rust 只接收最终结果字符串,无跨进程通信开销。

use std::time::Duration;

use bao::{BaoConfig, BrowserError, PageConfig};

fn main() -> Result<(), BrowserError> {
    // 1. Runtime + page
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    println!("[04-crawler] BaoRuntime ready");
    let page = runtime.create_page(&PageConfig::default())?;

    // 2. 导航
    println!("[04-crawler] Navigating to https://example.com ...");
    page.navigate("https://example.com")?;
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;

    // 3. ★ 核心:Node Realm 一次做三件事 — DOM 抽取 + HTTP 下载 + 文件保存
    println!("[04-crawler] Extracting page structure + fetching robots.txt ...");
    let crawl_script = r#"
        // ---- DOM 抽取(servo 提供 Web API)----
        const title       = document.title;
        const h1Count     = document.querySelectorAll('h1').length;
        const pCount      = document.querySelectorAll('p').length;
        const linkCount   = document.querySelectorAll('a').length;
        const links       = [...document.querySelectorAll('a')]
                              .map(a => ({ text: a.textContent.trim(), href: a.href }));

        // ---- Node.js fetch(Bao 兼容层内置)----
        let robotsStatus  = 0;
        let robotsLen     = 0;
        try {
            const resp = await fetch('https://example.com/robots.txt');
            robotsStatus = resp.status;
            const text = await resp.text();
            robotsLen = text.length;
        } catch (e) {
            robotsStatus = -1;
            robotsLen = 0;
        }

        // ---- Node.js fs(Bao 兼容层内置)----
        const fs = require('fs');
        const report = {
            url:       location.href,
            title,
            h1Count,
            pCount,
            linkCount,
            links,
            robotsStatus,
            robotsLen,
            fetchedAt: new Date().toISOString(),
        };
        fs.writeFileSync('crawl-report.json', JSON.stringify(report, null, 2));

        // ---- console.log 让 Rust 侧 stdout 直接看到关键数字 ----
        console.log('[04-crawler]   ↳ page title      =', JSON.stringify(title));
        console.log('[04-crawler]   ↳ h1 count         =', h1Count);
        console.log('[04-crawler]   ↳ paragraph count  =', pCount);
        console.log('[04-crawler]   ↳ link count       =', linkCount);
        console.log('[04-crawler]   ↳ http GET status  =', robotsStatus);
        console.log('[04-crawler]   ↳ body length      =', robotsLen, 'bytes');

        'done';
    "#;
    let out = page.evaluate_js(crawl_script)?;
    println!("[04-crawler] evaluate_js returned: {}", out);

    // 4. Rust 侧确认文件落盘(Node fs.writeFileSync 写的)
    let exists = std::path::Path::new("crawl-report.json").exists();
    println!(
        "[04-crawler] Report saved: crawl-report.json exists = {} (open it to see full JSON)",
        exists
    );
    println!("[04-crawler] Done");
    Ok(())
}
