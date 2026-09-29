# Bao Examples

8 个独立可运行的示例,展示 Bao(Rust-native programmable browser runtime)的核心能力。

## 运行方式

每个示例都是独立的 Cargo crate,不在主 workspace 内(根 `Cargo.toml` 已 `exclude = ["examples"]`)。
进入对应子目录后用 `cargo run` 即可。

```bash
cd examples/01-browser && cargo run
cd examples/02-playwright && cargo run
cd examples/03-node-dom && cargo run
cd examples/04-crawler && cargo run
cd examples/05-multi-page && cargo run   # 零网络
cd examples/06-shutdown && cargo run     # 零网络
cd examples/07-timeout && cargo run      # 零网络
cd examples/08-error && cargo run        # 零网络
```

> 首次构建会从源码编译 SpiderMonkey + servo,耗时较长(几十分钟),后续增量很快。

## 示例矩阵

| # | 目录 | 展示能力 | 核心 API |
|---|------|---------|----------|
| 01 | [`01-browser/`](01-browser/) | 最基本的 servo 浏览器嵌入 | `BaoRuntime` → `create_page` → `navigate` → `evaluate_js_web` → `take_screenshot` |
| 02 | [`02-playwright/`](02-playwright/) | CDP 自动化(Playwright/Puppeteer 连接 Bao) | `Browser::connect("ws://127.0.0.1:9222")` + Node.js `playwright` `connectOverCDP` |
| 03 | [`03-node-dom/`](03-node-dom/) | Node.js API × DOM 同一 runtime 共存(双 Realm) | `evaluate_js`(Node Realm: `document.querySelector` + `require('fs')`) |
| 04 | [`04-crawler/`](04-crawler/) | 服务端网页自动化 / 爬虫(导航 + 抽取 + 下载) | `navigate` + `evaluate_js` 抽链接 + `require('http').get` 下载 |
| 05 | [`05-multi-page/`](05-multi-page/) | 多页面管理(PagePool 并发页 + idle 归还) | `create_page` ×2 + `page_pool().stats()/release_page/get_page` |
| 06 | [`06-shutdown/`](06-shutdown/) | 优雅关闭(pending work drain + 有界 drop) | `wait_for_function` + `close_page` + `PageState::Closed` + W27 有界 drop |
| 07 | [`07-timeout/`](07-timeout/) | 执行超时(引擎级中断,死循环可掐断) | `evaluate_js[_web]_with_timeout` |
| 08 | [`08-error/`](08-error/) | 错误处理链(source() 因果链逐层打印) | `BrowserError`/`ConnectError` + `std::error::Error::source` |

## 关于 Bao 的核心卖点

- **Rust-native**: Servo(不是 Chrome)+ SpiderMonkey(不是 V8),全程 Rust 编译
- **Node.js + DOM 共存**: 同一 SpiderMonkey JSContext 下的两个 Realm(Node Realm / Page Realm),`evaluate_js` 在 Node Realm 同时拥有 `document.querySelector` 和 `require('fs')`(见示例 03)
- **CDP 协议**: Playwright/Puppeteer 无需改造直连 `ws://127.0.0.1:9222`,但背后跑的是 servo(见示例 02)
- **反指纹**: 默认内置 TLS/HTTP2/Canvas/WebGL 隐藏能力,运行时配置即可启用

详细的 API 说明见根目录 [`README.md`](../README.md) 的 "Rust API 用法" 和 "PageHandle API 速查" 章节。
