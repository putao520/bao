# bao-core

**一个可以直接嵌入 Rust 应用的可编程 JS/TS 系统 Runtime。**

`bao-core` 是 Bao 面向消费者的统一库入口。它把 SpiderMonkey、Rust-native 的 Node.js/Bun 风格系统 API、Servo Web Runtime、CDP 兼容能力与 Stealth 组合进同一套 Runtime。

Bao **不是**想做另一个独立运行的 Node/Bun 代餐，也不是以浏览器自动化产品为主要定位。它想解决的是：给 Rust 应用增加一层可编程执行能力，用于动态逻辑、自动化、Workflow、插件，以及 Agent 生成的程序，同时资源和生命周期仍然由 Rust Host 掌握。

**浏览器只是 Runtime 的一种能力，不是项目本身。** 不需要网页时，任务可以只使用文件、HTTP、crypto、SQLite、模块与普通 JS 控制流；只有真正需要页面时才进入 Web/DOM Runtime。

**[English](https://github.com/putao520/bao/blob/master/src/bao/README.md)** · 完整项目说明：[github.com/putao520/bao](https://github.com/putao520/bao)

> **包名与导入名：** package 名是 `bao-core`，library 名是 `bao`。
> `Cargo.toml` 写 `bao-core`，代码写 `use bao::…`。

```toml
[dependencies]
bao-core = "0.1.5"
```

## Bao 适合什么，不适合什么

- **独立 JS/TS 服务或 CLI：** 优先 Node.js / Bun。
- **纯 Chromium 浏览器自动化：** 优先 Playwright + Chromium。
- **很小的规则/表达式脚本：** Rhai 或其他轻量脚本引擎更轻。
- **紧凑、强隔离的插件 ABI：** WASM 往往更合适。
- **只需要嵌入 JavaScript 引擎：** QuickJS 等项目直接解决这一层问题。
- **Rust 产品需要丰富可编程系统层、按需 Web/DOM，或者需要一层贴近应用状态的 Agent 程序执行基础：** 这才是 Bao 想解决的问题。

Bao 有意从“裸 JS 引擎”更上一层开始。真实产品里的脚本很快就会需要模块、异步语义、系统 API、生命周期、兼容性、Web 能力，以及宿主仍然掌握控制权的执行模型，而不只是 `eval()`。

## 当前入口

### 1. 不创建页面，只使用系统 Runtime

Node/Bun 风格 host surface 重导出到：

```rust
bao::runtime::*
```

任务需要系统/runtime API、但不需要 Web/DOM 时走这条路径。

> **当前 alpha 阶段存在两个同名 `BaoRuntime`：**
> `bao::runtime::BaoRuntime` 是 Node/Bun API host；顶层 `bao::BaoRuntime`
> 是统一浏览器协调器。

### 2. 任务需要网页时再加入 Web/DOM

```rust,no_run
use std::time::Duration;
use bao::{BaoConfig, BaoRuntime, BrowserError, PageConfig, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig::default())?;
    page.navigate("https://example.com")?;
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;
    let title = page.evaluate_js_web("document.title")?; // Page Realm：仅 Web API
    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let _ = (title, png);
    Ok(())
}
```

### 3. 同一任务里组合可信系统脚本与 DOM

`page.evaluate_js` 运行在 Bao 由宿主控制的 **Node Realm**。可信宿主脚本可以在同一段代码里使用 DOM 和 Node/Bun 风格系统 API：

```js
const h1  = document.querySelector('h1')?.textContent;
const txt = require('fs').readFileSync('demo.txt', 'utf8');
const res = await fetch('https://example.com/robots.txt');
```

网站自己的 JavaScript 仍然留在普通 Page Realm，不会因此获得 `require` / filesystem 权限。

### 4. CDP 兼容

Bao 提供内置 CDP surface 和 Playwright 风格 Rust client。同一 client 可以进程内连接，也可以走 WebSocket：

```rust,no_run
use bao::{Browser, ConnectError};

fn connect() -> Result<(), ConnectError> {
    let mut browser = Browser::connect("memory://bao")?;
    let _version = browser.version()?;
    let _targets = browser.pages()?;
    Ok(())
}
```

CDP 是兼容边界，不代表 Bao 在宣称自己就是 Chrome。coverage 与 lifecycle compatibility 仍在持续完善。

## 当前能力栈

| 层 | Bao 当前提供什么 |
|---|---|
| JS Runtime | 基于 SpiderMonkey 的 JavaScript Runtime |
| 系统 API | Node/Bun 风格模块、filesystem、HTTP/fetch、crypto、`bun:sqlite`、process/runtime primitive 等 Rust-native 基础层 |
| Web Runtime | Servo DOM/CSS/layout/render，page 与截图 |
| Realm 边界 | 网站代码在 Page Realm；可信宿主系统脚本在 Node Realm |
| 自动化 | 内置 CDP + `memory://bao` 进程内 client transport |
| Stealth | 运行时配置的 TLS/HTTP/浏览器可见指纹控制 |

## 当前重要限制

Bao 仍是 **0.x alpha**：

- Linux x86_64 是当前唯一完整验证的平台。
- Node/Bun、Web、CDP 的兼容面已经很大，但尚未完成。
- Realm 隔离不等于任意不可信代码沙箱已经完成；细粒度 capability/quota/audit 仍在建设。
- `JSContext` 是线程局部的。`JSObject` / GC pointer 不能跨线程；跨线程只传 id、handle、owned message 或序列化数据，JS 操作必须回 owner thread。
- 当前完整栈恒链，browser/CDP/Stealth/Node 层不是通过 Cargo product feature 删除，而主要由运行时行为选择。
- 第一次构建要从源码编译 SpiderMonkey，因此天然比轻量脚本 crate 更重。
- 当前要求 Rust nightly。
- macOS 已有编译面工作，但还没有完成 Apple 真机验证。

## 包族

`bao-core` 是统一门面。已发布包族还包括 `bao-browser`、`bao-engine`、
`bao-stealth`、`bao-cdp`、`bao-cdp-client`、`bun-runtime`、`bun-*`
Rust-native 基础层，以及 `bao-mozjs(-sys)`、`bao-mozjs-src-*`、
`bao-servo-*`、`bao-stylo`、`bao-ipc-channel` 等 Runtime/Browser 依赖包。

## 许可证

MPL-2.0（SpiderMonkey + Servo）· MIT（Bun-derived crates）。
