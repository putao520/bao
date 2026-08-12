# Example 04 — 服务端网页自动化 / 爬虫

展示用 Bao 做服务端爬虫:导航 → 用 Node Realm 的 DOM 抽取 + Node.js HTTP 客户端下载资源 → 保存到本地。
**整个流程在同一段 JS 内完成,不需要在 Rust 和浏览器之间来回序列化数据**——这是 Bao 相对于
Puppeteer/Playwright 的关键优势。

## 典型应用场景

- 价格监控 / 竞品分析(需要真实浏览器渲染,避开 SPA/反爬)
- 服务端截图生成(headless,无需 X server)
- 端到端测试(配合 CDP)
- 数据抓取 → 直接存文件/DB(因为 Node.js fs/sqlite 在同一 runtime 在线)

## 运行

```bash
cargo run
```

## 预期输出

```
[04-crawler] BaoRuntime ready
[04-crawler] Navigating to https://example.com ...
[04-crawler] Extracting page structure ...
[04-crawler]   ↳ page title      = "Example Domain"
[04-crawler]   ↳ h1 count         = 1
[04-crawler]   ↳ paragraph count  = 2
[04-crawler]   ↳ outbound links   = 0
[04-crawler] Fetching robots.txt via Node http ...
[04-crawler]   ↳ http GET status  = 200
[04-crawler]   ↳ body length      = 437 bytes
[04-crawler] Saving report to crawl-report.txt ...
[04-crawler] Done — see crawl-report.txt
```

## 核心 API 调用

```rust
// 一次 evaluate_js 把「DOM 抽取 + HTTP 下载 + 文件保存」全部做完
let report = page.evaluate_js(r#"
    const title    = document.title;
    const links    = [...document.querySelectorAll('a')].map(a => a.href);
    const fs       = require('fs');

    // Node.js fetch(Bao 内置 Node.js API)
    const resp     = await fetch('https://example.com/robots.txt');
    const robots   = await resp.text();

    fs.writeFileSync('crawl-report.txt', JSON.stringify({ title, links, robots }, null, 2));
    'done';
"#)?;
```

## 关键点

- **零数据序列化开销**:DOM 抽取 + HTTP + fs 都在 SpiderMonkey 同一 runtime,Rust 只接收最终结果字符串
- **真实浏览器渲染**:servo 完整执行 HTML→CSS→Layout→Paint,SPA 也能渲染(JavaScript via SpiderMonkey)
- **可扩展**:搭配 `StealthProfile` 启用反指纹,搭配 `PagePool` 并发多页面
- **替代 Puppeteer + Node** 的组合:Bao 本身就是「Rust 浏览器 + Node.js」,省去跨进程通信
