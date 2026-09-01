# Bao（包子）

**一个可以直接嵌入 Rust 应用的可编程 JS/TS 系统 Runtime。**

Bao 把 SpiderMonkey、Rust-native 的 Node.js/Bun 风格系统 API、Servo Web Runtime、CDP 兼容边界与 Stealth 组合进同一个库。它的目标**不是再做一个 Node/Bun 代餐，也不是再做一个浏览器自动化产品**，而是给 Rust 产品增加一层可编程执行能力：动态业务逻辑、自动化、Workflow、插件，以及未来由 Agent 生成的短程序，都可以靠近宿主的资源和生命周期执行，同时真正的资源、权限和产品边界仍由 Rust Host 掌握。

**浏览器是 Runtime 的一种能力，不是 Bao 这个项目本身。** 一段任务可以一直用普通 JS/TS 处理文件、HTTP、crypto、SQLite、模块和业务逻辑，只有真的需要网页时才进入 Web/DOM Runtime。

**[English](./README.md)** · 状态：**0.x alpha** — Linux x86_64 · API 可能变化 · [CHANGELOG](./CHANGELOG.md)

---

## 为什么会有 Bao

一个 Rust 产品经常同时需要两件看起来矛盾的事：

1. 底层核心稳定、强类型、资源归属清楚；
2. 上层规则、自动化、插件、Workflow 和 AI 任务逻辑可以快速变化。

只嵌一个很小的表达式引擎，脚本一旦开始需要文件、网络、模块、crypto、数据库、异步 I/O 或 Web API，很快就不够了。另起一个 Node 服务当然可以，但状态、生命周期、打包、权限和错误也随之跨进程。

Bao 想探索的是另一种结构：

```text
Rust Application
│
├── 稳定的 Native Core
│   ├── 资源 / 状态
│   ├── 调度
│   ├── 权限
│   └── 性能敏感代码
│
└── Bao 可编程层
    ├── JS / TS 控制流
    ├── Node/Bun 风格系统 API
    ├── HTTP / filesystem / crypto / SQLite / ...
    ├── 需要时进入 Web / DOM Runtime
    └── CDP 兼容边界
```

放到 Agent 场景里，这意味着模型不一定要把每一个确定性步骤都拆成一次 Tool Call。它可以生成一段短程序，让 `for`、`try/catch`、`Promise.all`、模块、stream 等普通语言结构承担局部控制流，真正遇到需要判断的问题时再回到模型。

Bao 是 **Runtime 基础设施**，不是 Agent Framework。它不试图自己包办 planner、memory、tool registry 或长时间 Workflow 产品。

## Bao 是什么，也不是什么

| 你的主要问题 | 通常先用什么 | Bao 的位置 |
|---|---|---|
| 做独立 JavaScript/TypeScript 服务或 CLI | Node.js / Bun | Bao 不打算取代它们作为 executable runtime。 |
| 纯浏览器自动化，并且最看重 Chromium 兼容性 | Playwright + Chromium | 当 Web 只是 Rust 产品内部执行环境的一项能力时，Bao 才更有意义。 |
| 只需要很小的规则/表达式脚本 | Rhai 或其他轻量脚本引擎 | Bao 故意更重，因为它目标是一套完整系统 Runtime，而不是表达式执行器。 |
| 需要紧凑、强隔离的插件 ABI | WASM 往往更合适 | Bao 更关注熟悉的 JS/TS 编程环境和丰富的系统/Web 能力；任意不可信代码沙箱目前还没做完。 |
| Rust 产品需要丰富动态脚本、自动化或贴近应用状态的 Agent 执行层 | **Bao** | 这才是 Bao 真正想解决的问题。 |

QuickJS 以及其他嵌入式 JS 引擎解决的是一个很重要的底层问题：**怎样把 JavaScript 嵌进去**。Bao 从更上一层开始：真实产品最难的往往不只是 `eval()`，而是系统 API、异步 Runtime 语义、Web Runtime、生命周期、兼容性，以及宿主仍然掌握控制权的执行模型。

## Bao 最核心的库优势

Bao 想成为的是**链接进 Rust 产品里的库**，而不是产品还需要额外编排的一个服务。

这件事的价值在于宿主可以继续自己掌握：

- task / runtime 生命周期；
- 权限与资源所有权；
- 应用状态和 native 对象；
- 线程与调度；
- 文件、socket、数据库连接和 page handle；
- cancellation、shutdown 与错误传播。

长期方向是：让应用自己的能力逐渐变成可编程能力，而不是把每个低层动作都包装成 RPC 或远程 Tool。Node/Bun compatibility、Servo、CDP、Stealth 都是在为这个目标提供积木，不是 Bao 的产品定义本身。

## 使用本库

**包名与导入名：** crate 发布名是 **`bao-core`**，库名是 **`bao`**。`Cargo.toml` 写 `bao-core`，Rust 代码写 `use bao::…`。

```toml
[dependencies]
bao-core = "0.1.5"
```

```rust
use bao::{BaoConfig, BaoRuntime};
```

### 入口一 —— 不创建页面，只使用 Node/Bun 风格系统 Runtime

如果任务不需要 Web/DOM，Bao 把 `bun_runtime` host 重导出到：

```rust
bao::runtime::*
```

这条路径面向 Node/Bun 风格模块与系统 API，不要求先创建浏览器页面。

> **目前有两个同名 `BaoRuntime`：** `bao::runtime::BaoRuntime` 是 Node/Bun API host，顶层 `bao::BaoRuntime` 是统一浏览器协调器。这是 alpha 阶段的 API 约束，后续仍可能调整。

### 入口二 —— 任务需要网页时再进入 Web/DOM Runtime

顶层 `BaoRuntime` 可以让 Rust Host 直接使用 Servo page。重点不是“Bao 自带浏览器”，而是 Web 能力仍然留在同一套 Bao Runtime 中，不必把任务切成 Node → Playwright → Chromium 的 sidecar 链路。

```rust,no_run
use std::time::Duration;
use bao::{BaoConfig, BaoRuntime, BrowserError, PageConfig, PageState, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig::default())?;

    page.navigate("https://example.com")?;
    // 当前 Servo 生命周期约束：求值前先等 pipeline ready。
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;

    // Page Realm：只有 Web API，没有 require/fs。
    let title = page.evaluate_js_web("document.title")?;

    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let state: PageState = page.get_state();
    let _ = (title, png, state);
    Ok(())
}
```

### 入口三 —— 同一任务里同时使用可信系统脚本和 DOM

`page.evaluate_js` 运行在 Bao 的 **Node Realm**。宿主主动执行的可信脚本可以同时使用 DOM 与 Node/Bun 风格系统 API：

```rust,no_run
# use std::time::Duration;
# use bao::{BaoConfig, BaoRuntime, BrowserError, PageConfig};
# fn main() -> Result<(), BrowserError> {
#     let runtime = BaoRuntime::new(BaoConfig::default())?;
#     let page = runtime.create_page(&PageConfig::default())?;
#     page.navigate("https://example.com")?;
#     page.wait_for_pipeline_ready(Duration::from_secs(30))?;
let script = r#"
    const h1 = document.querySelector('h1')?.textContent ?? "(none)";
    const fs = require('fs');
    const txt = fs.readFileSync('demo.txt', 'utf8');
    const res = await fetch('https://example.com/robots.txt');
    JSON.stringify({ h1, txt, status: res.status })
"#;
let json = page.evaluate_js(script)?;
let _ = json;
#     Ok(())
# }
```

网页自己的 JavaScript **拿不到**这些系统能力。Bao 把普通 Page Realm 与宿主控制的 Node Realm 分开；如果直接把 `fs` 挂到任意网页的 `window` 上，这个安全边界就失去了意义。

### 入口四 —— CDP 自动化生态兼容

Bao 同时提供 CDP server 和 Playwright 风格 Rust client。同一套 client 抽象既可以通过 `memory://bao` 在进程内连接，也可以走 WebSocket。

```rust,no_run
use bao::{BaoConfig, BaoRuntime, Browser, BrowserError, ConnectError};

fn start_runtime_with_cdp() -> Result<(), BrowserError> {
    let _runtime = BaoRuntime::new(BaoConfig {
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

**Pump 契约：** Servo 域 CDP 命令（`Runtime.evaluate`、`Page.navigate` 等）在 runtime 线程执行，目前需要宿主通过 `runtime.pump_cdp(Duration)` 或 `run()` loop 驱动。`Browser.version`、`pages()` 这类纯协议命令不需要。没有 pump 时，Servo 域命令会超时，而不是返回假成功。

CDP 是兼容边界，不代表 Bao 在宣称自己就是 Chrome。method coverage、event ordering、object lifecycle 和 Playwright 的隐含语义仍在持续测试和完善。

## 当前能力栈

| 层 | Bao 当前提供什么 |
|---|---|
| 可编程语言 | 基于 SpiderMonkey 的 JavaScript Runtime；TS/tooling 属于整体 Runtime 演进方向 |
| 系统 Runtime | Node/Bun 风格模块与 API：`require`、filesystem、HTTP/fetch、crypto、`bun:sqlite`、process/runtime primitive 等 Rust-native 基础能力 |
| Web Runtime | Servo DOM/CSS/layout/render，多页面 `PagePool` / `PageHandle`，截图 |
| Realm 边界 | 网站代码运行在 Page Realm；可信宿主系统脚本运行在 Node Realm |
| 自动化兼容 | 内置 CDP surface + Playwright 风格 Rust client，支持 `memory://bao` 进程内 transport |
| Stealth | 通过运行时 `StealthProfile` 控制 TLS/HTTP 与浏览器可见指纹 |

## 当前重要限制

Bao 仍然是 **0.x alpha**。目前至少需要明确这些边界：

- Linux x86_64 是唯一真正完成验证的平台。
- Node/Bun、Web、CDP 的兼容面已经很大，但远未完成；“有 API / 有 handler”不等于行为兼容已经证明。
- Realm 隔离**不等于**任意不可信代码沙箱已经完成；细粒度 capability、quota、audit 与更强隔离仍在建设。
- `JSContext` 是线程局部的。`JSObject` / GC pointer 不能跨线程；跨线程只传 id、handle、owned message 或序列化数据，真正 JS 操作必须回 owner thread。
- 当前完整栈恒链，没有 Cargo product feature 可以移除 browser/CDP/Stealth/Node 层，行为主要由运行时配置选择。
- 第一次构建需要从源码编译 SpiderMonkey，因此它天然比轻量脚本 crate 更重。
- 当前要求 Rust nightly，仓库钉定已验证工具链。
- macOS 已有编译面工作，但尚未完成 Apple 真机 build/link/test 验证。

这些 trade-off 是现阶段有意接受的：Bao 现在优先把一套完整 Runtime 的行为做可靠，再考虑把所有子系统拆成复杂的可选矩阵。

## 构建前置

- clang、python3、make（SpiderMonkey 构建）
- 仓库钉定的 Rust nightly
- Linux 媒体播放需要对应的系统 GStreamer runtime libraries

SpiderMonkey 首次构建完成后会使用缓存。当前工具链、平台和媒体依赖细节以仓库 build 文档为准。

## crates.io 包族

`bao-core` 是消费者主要使用的统一门面。包族同时公开更低层切片，供有明确需求的用户直接使用：

| 包 | 角色 |
|---|---|
| `bao-core` | 统一 library facade |
| `bao-engine` / `bun-sm` | SpiderMonkey 引擎层 |
| `bun-runtime` | Node.js/Bun 风格系统 Runtime host |
| `bun-*` | Rust-native 基础层：HTTP、resolver、install、crypto 相关 plumbing、bundler 组件等 |
| `bao-browser` | Servo 嵌入：`BaoRuntime`、`PagePool`、`PageHandle` |
| `bao-cdp` / `bao-cdp-client` | CDP server 面 / Playwright 风格 Rust client |
| `bao-stealth` | Stealth engine + `StealthProfile` |
| `bao-mozjs`、`bao-mozjs-sys`、`bao-mozjs-src-*`、`bao-servo-*`、`bao-stylo`、`bao-ipc-channel` | Runtime/Browser 依赖家族 |

## 项目方向

Bao 一直围绕一个问题在演进：

> **Rust 应用能不能在不丢掉 native ownership 的前提下，获得一套丰富、熟悉、真正可编程的执行环境；而这同一层基础设施，未来能不能既服务人写的脚本，也服务 Agent 生成的程序？**

这也是为什么 Bao 和“再加几个 API 名字”相比，更在意 lifecycle、错误语义、event-loop fairness、Realm 边界、cancellation、compatibility、GC ownership 与 host safety。

## 许可证

MPL-2.0（SpiderMonkey + Servo）· MIT（Bun-derived crates）。见 `LICENSE-MPL-2.0`、`LICENSE-MIT` 与 `THIRD_PARTY_LICENSES.md`。

---

*开发仓库：clone [putao520/bao](https://github.com/putao520/bao)。`examples/` 目录包含可运行的接入示例。*
