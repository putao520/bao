# Bao（包子）

**Bao 是一个 Rust 库：同一个 crate 里同时提供 Bun/Node 兼容的系统运行时和真正的浏览器（Servo DOM + CDP），Rust 项目用一个依赖直接嵌入——不需要 Node 进程，也不需要 Chromium 边车。**

```rust
use std::time::Duration;
use bao::{BaoConfig, BrowserError, BrowserRuntime, PageConfig};

fn main() -> Result<(), BrowserError> {
    let runtime = BrowserRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig::default())?;
    page.navigate("https://example.com")?;
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;
    let json = page.evaluate_js(r#"
        const h1 = document.querySelector('h1')?.textContent ?? "(none)";
        const txt = require('fs').readFileSync('demo.txt', 'utf8');
        const res = await fetch('https://example.com/robots.txt');
        JSON.stringify({ h1, txt, status: res.status })
    "#)?;
    println!("{json}");
    Ok(())
}
```

`evaluate_js` 在宿主控制的 Node Realm 里跑这段脚本：`document`、`require('fs')` 和 `fetch` 在同一个 global 上（[`examples/03-node-dom`](examples/03-node-dom)）。页面脚本走 `evaluate_js_web`，拿不到 `require` 和 `fs`。`wait_for_pipeline_ready` 紧跟 `navigate`，因为 Servo pipeline 是异步建立的。`demo.txt` 是工作目录里的文件。

**[English](./README.md)** · 状态：**0.x alpha** — Linux x86_64 · Windows x86_64 · API 可能变化 · [CHANGELOG](./CHANGELOG.md)

## 为什么不是 X

| 方案 | crate 外面还要跑什么 |
|---|---|
| **Playwright + Chromium** | Chromium 是单独的浏览器进程，经 CDP 驱动。页面 JavaScript 和 Node/Bun 脚本分属不同进程，要在二进制旁边打包并监管。 |
| **Node / Bun 边车** | 系统运行时是另一个进程。脚本生命周期、权限、文件和返回值都跨过 Rust 宿主不拥有的边界。 |
| **Deno / QuickJS / Rhai** | Deno 是独立运行时进程。QuickJS 能嵌入 JavaScript，但没有这套 Node/Bun API，也没有 Servo。Rhai 是表达式语言，不是带浏览器的 JS 系统运行时。 |

## 适合谁

- **Rust 爬虫与反检测抓取。** 导航一个 Servo 页面，读 DOM，并在同一段可信脚本里使用 `fs` / `fetch`。`StealthProfile` 是运行时配置，覆盖 TLS/HTTP 指纹和浏览器可见面（Canvas、Navigator、WebGL、Audio、behavior）。
- **Agent 执行环境。** 宿主可以跑一段短程序，用普通 JS 控制流（`for`、`try/catch`、`Promise.all`、模块）加上系统 API 和 DOM。Bao 不实现 planner、memory 或 tool registry。
- **Rust 产品内部的自动化与插件层。** 把这个 crate 链进产品。宿主掌握任务生命周期、权限和原生资源；脚本承担会变的那一层。

## 安装

发布包名是 **`bao-core`**，库名是 **`bao`**。`Cargo.toml` 写 `bao-core`，Rust 代码写 `use bao::…`。

```toml
[dependencies]
bao-core = "0.3.12"
```

```rust
use bao::{BaoConfig, BrowserRuntime};
```

`0.3.12` 是当前 crates.io 版本。不需要页面时，Node/Bun 宿主是 `bao::runtime::NodeRuntime`。顶层 `bao::BrowserRuntime` 是浏览器协调器。旧名 `BaoRuntime` 在两侧都还是 deprecated alias，保留到 0.x。

## 当前重要限制

Bao 仍然是 **0.x alpha**。目前至少需要明确这些边界：

- 已完成完整验证的平台：Linux x86_64 与 Windows x86_64（自 Linux 经 `scripts/win-cross-env.sh` 交叉构建，真机运行 + 电池验证，2026-09-26）。三档平台矩阵见 [docs/platform-support.md](docs/platform-support.md)。
- Node/Bun、Web、CDP 的兼容面已经很大，但远未完成；“有 API / 有 handler”不等于行为兼容已经证明。
- Realm 隔离不等于任意不可信代码沙箱已经完成。细粒度 capability、quota、audit 与更强隔离仍在建设。
- `JSContext` 是线程局部的。`JSObject` / GC pointer 不能跨线程；跨线程只传 id、handle、owned message 或序列化数据，JS 操作必须回到 owner 线程。
- 当前完整栈恒链接。没有 Cargo product feature 可以卸掉 browser/CDP/Stealth/Node 层。行为由运行时配置选择。
- 第一次构建要从源码编译 SpiderMonkey，比轻量脚本 crate 更重。
- 当前要求 Rust nightly，仓库钉定已验证的工具链。
- macOS 已有编译面工作，但尚未完成 Apple 真机 build/link/test 验证。

## 构建前置

- clang、python3、make（SpiderMonkey 构建）
- 仓库钉定的 Rust nightly
- Linux 媒体播放需要对应的系统 GStreamer runtime libraries

SpiderMonkey 首次构建完成后会使用缓存。当前工具链、平台和媒体依赖细节以仓库构建文档为准。

## crates.io 包族

`bao-core` 是面向使用者的统一门面。包族同时公开更低层切片，供需要某一层的人直接依赖：

| 包 | 角色 |
|---|---|
| `bao-core` | 统一 library facade |
| `bao-engine` / `bun-sm` | SpiderMonkey 引擎层 |
| `bun-runtime` | Node.js/Bun 风格系统运行时宿主 |
| `bun-*` | Rust-native 基础层：HTTP、resolver、install、crypto 相关 plumbing、bundler 组件等 |
| `bao-browser` | Servo 嵌入：`BrowserRuntime`、`PagePool`、`PageHandle` |
| `bao-cdp` / `bao-cdp-client` | CDP server 面 / Playwright 风格 Rust client |
| `bao-stealth` | Stealth engine + `StealthProfile` |
| `bao-mozjs`、`bao-mozjs-sys`、`bao-mozjs-src-*`、`bao-servo-*`、`bao-stylo`、`bao-ipc-channel` | 运行时 / 浏览器依赖家族 |

## 参与贡献

先读 [CONTRIBUTING.md](./CONTRIBUTING.md)。缺陷报告走 [issues](https://github.com/putao520/bao/issues)。使用问题和设计讨论走 [discussions](https://github.com/putao520/bao/discussions)。

开发仓库：clone [putao520/bao](https://github.com/putao520/bao)。可运行的接入示例在 `examples/`。

## 许可证

MPL-2.0（SpiderMonkey + Servo）· MIT（Bun 衍生 crate）。见 `LICENSE-MPL-2.0`、`LICENSE-MIT` 与 `THIRD_PARTY_LICENSES.md`。

---

## 其他入口

### 不创建页面，只用系统运行时

任务不需要页面时，Node/Bun 宿主是 `bao::runtime::NodeRuntime`，重导出在 `bao::runtime::*`。

### 仅页面 realm、截图、页面状态

`evaluate_js_web` 留在 Page Realm（只有 Web API）。截图和页面状态在 page handle 上：

```rust
use std::time::Duration;
use bao::{BaoConfig, BrowserError, BrowserRuntime, PageConfig, PageState, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    let runtime = BrowserRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig::default())?;
    page.navigate("https://example.com")?;
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;
    let title = page.evaluate_js_web("document.title")?;
    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let state: PageState = page.get_state();
    let _ = (title, png, state);
    Ok(())
}
```

### CDP

Bao 提供 CDP server 和 Playwright 风格的 Rust client。同一 client 可以通过 `memory://bao` 在进程内连接，也可以走 WebSocket。

```rust
use bao::{BaoConfig, Browser, BrowserError, BrowserRuntime, ConnectError};

fn start_runtime_with_cdp() -> Result<(), BrowserError> {
    let _runtime = BrowserRuntime::new(BaoConfig {
        cdp_port: Some(9222),
        ..BaoConfig::default()
    })?;
    Ok(())
}

fn connect() -> Result<(), ConnectError> {
    let mut browser = Browser::connect("memory://bao")?;
    let _version = browser.version()?;
    let _targets = browser.pages()?;
    Ok(())
}
```

**Pump 契约：** Servo 域 CDP 命令（`Runtime.evaluate`、`Page.navigate` 等）在 runtime 线程执行，目前需要宿主用 `runtime.pump_cdp(Duration)` 或 `run()` 循环驱动该线程。`Browser.version`、`pages()` 这类纯协议命令不需要。没有 pump 时，Servo 域命令会超时，而不是返回假成功。

CDP 是兼容边界。method coverage、事件顺序、对象生命周期和 Playwright 的隐含假设仍在测试。Bao 不是 Chrome。

## 当前能力栈

| 层 | Bao 提供什么 |
|---|---|
| 可编程语言 | 基于 SpiderMonkey 的 JavaScript 运行时 |
| 系统运行时 | Node/Bun 风格模块与 API：`require`、filesystem、HTTP/fetch、crypto、`bun:sqlite`、process/runtime 原语 |
| Web 运行时 | Servo DOM/CSS/layout/rendering，多页面 `PagePool` / `PageHandle`，截图 |
| Realm 边界 | 站点代码在 Page Realm；可信宿主脚本在 Node Realm |
| 自动化兼容 | 内置 CDP 面 + Playwright 风格 Rust client，含进程内 `memory://bao` |
| Stealth | 通过运行时 `StealthProfile` 配置 TLS/HTTP 与浏览器可见指纹 |

## Bao 放在哪一层

Rust 产品通常同时要一个稳定的原生核心，和一层给规则、自动化、插件、短程序用的可编程层。脚本一旦需要文件、网络、模块或页面，小表达式引擎就不够了。另起一个 Node 服务则把生命周期和失败推到进程边界之外。

Bao 把这一层做成库：JS 控制流、Node/Bun 风格系统 API，以及任务真的需要 DOM 时的 Servo 页面，链进宿主。可以一直停在系统运行时，只在需要 DOM 的那一步再开页面。独立的 JS/TS 服务或 CLI 仍然是 Node 或 Bun 的事。紧凑、强隔离的插件 ABI 也许更适合 WASM；Realm 分隔还不是任意不可信代码的成品沙箱。

宿主继续掌握任务生命周期、权限、应用状态、调度、文件、socket、数据库连接、page handle、取消和关闭。

```text
Rust 应用
│
├── 原生核心
│   资源、调度、权限、性能敏感代码
│
└── Bao（一个 crate）
    JS 控制流
    Node/Bun 风格系统 API
    需要 DOM 时的 Servo 页面
    CDP 边界
```
