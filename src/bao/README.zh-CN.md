# bao-core

**单 Rust 栈的高性能反指纹浏览器运行时**——SpiderMonkey + Servo + 始终在线的 Node.js/Bun API + 内置 Stealth 与 CDP。

**[English](https://github.com/putao520/bao/blob/master/src/bao/README.md)**

> **包名与导入名:** 本包名是 `bao-core`,库名钉定为 `bao`。
> `Cargo.toml` 写 `bao-core`,代码写 `use bao::…`。

```toml
[dependencies]
bao-core = "0.0.1"
```

## 用法 —— 三个入口

### 1. 浏览器嵌入(主路径)

```rust,no_run
use std::time::Duration;
use bao::{BaoConfig, BrowserError, PageConfig, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    let runtime = BaoRuntime::new(BaoConfig::default())?;   // 顶层协调器
    let page = runtime.create_page(&PageConfig::default())?;
    page.navigate("https://example.com")?;
    // servo 硬约束——求值前必须等 pipeline 就绪:
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;
    let title = page.evaluate_js_web("document.title")?;    // Page Realm(仅 Web API)
    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let _ = (title, png);
    Ok(())
}
```

### 2. CDP 自动化(Playwright 风格)

库配置(`BaoConfig::cdp_port`)启动 CDP server,再同进程或走 WebSocket 连接:

```rust,no_run
use bao::{Browser, BaoConfig, BrowserError, ConnectError};

fn start_runtime_with_cdp() -> Result<(), BrowserError> {
    // cdp_port 在 ws://127.0.0.1:<port> 启动内置 CDP server。
    let _runtime = BaoRuntime::new(BaoConfig {
        cdp_port: Some(9222),
        ..BaoConfig::default()
    })?;
    Ok(())
}

fn connect() -> Result<(), ConnectError> {
    let mut browser = Browser::connect("memory://bao")?;    // 进程内;或 "ws://host:port"
    let _version = browser.version()?;                      // CDP Browser.version
    let _targets = browser.pages()?;                        // 等价 GET /json/list
    Ok(())
}
```

### 3. 页面内 Node/Bun API(双 Realm)

`page.evaluate_js` 在 **Node Realm** 执行:DOM 与 `require` / `fs` /
`fetch` / `Bun` 同一作用域。

```js
const h1  = document.querySelector('h1')?.textContent;      // DOM(servo)
const txt = require('fs').readFileSync('demo.txt', 'utf8'); // Node API(Bao)
const res = await fetch('https://example.com/robots.txt');  // Node fetch
```

不带页面的 Node/Bun host 搭建用 `bao::runtime::`(`bun_runtime` 面)。

> ⚠ **同名陷阱:** `bao::runtime::BaoRuntime`(Node/Bun host)≠ 顶层
> `bao::BaoRuntime`(浏览器协调器)。浏览器嵌入用顶层名;Node/Bun host
> 搭建用 `bao::runtime::`。

## 硬约束与前置

- **JSContext 线程局部**——DOM ↔ Node 互操作留在创建线程;跨线程传
  `JSObject` 指针 = SIGSEGV。跨线程只传 page id / 句柄 / 序列化数据。
- **全栈恒链**——没有 Cargo feature 可关浏览器/CDP/stealth/Node;行为
  是运行时选择(`StealthProfile`、`Permission`)。
- **首次构建从源码编译 SpiderMonkey**——需 clang、python3、make;首次
  20–40 分钟(之后缓存)。
- **Linux 媒体播放需系统 GStreamer 运行库**——servo 媒体栈(Linux 下经
  `bao-servo-media-auto`)运行时加载:`apt install libgstreamer1.0-0
  gstreamer1.0-plugins-base gstreamer1.0-plugins-bad`(或发行版等价包;
  `-dev` 包仅编译期需要)。
- **需要 Rust nightly 工具链**——本仓钉 `nightly-2026-07-20`(见
  `rust-toolchain.toml`);rustup 使用者执行 `rustup override set
  nightly-2026-07-20` 或等价操作。stable 编译器会报 E0554(非 nightly
  使用 `#![feature]`)。
- `BAO_<SUFFIX>` 环境变量别名到 `BUN_<SUFFIX>`。

## 包族

`bao-core` 是门面;家族含 `bao-browser`、`bao-engine`、`bao-stealth`、
`bao-cdp`、`bao-cdp-client`、`bun-runtime` 与 `bun_*` 基座层,另有维护
fork:`bao-mozjs(-sys)`(+ `bao-mozjs-src-*` 源码卫星)、`bao-servo-*`、
`bao-stylo`、`bao-ipc-channel`。

## 许可证

MPL-2.0(SpiderMonkey + Servo)· MIT(Bun 系 crate)。
