# Bao(包子)

**单 Rust 栈的高性能反指纹浏览器运行时**——SpiderMonkey(JS 引擎)+ Servo(完整浏览器引擎)+ 始终在线的 Node.js/Bun API 兼容层 + 内置 Stealth。无 Chromium、无 Node 子进程、无自动化桥接:浏览器引擎本身就是运行时。

**[English](./README.md)** · 状态:**0.x alpha**——Linux x86_64 · API 可能变化 · [CHANGELOG](./CHANGELOG.md)

---

## 使用本库

**包名与导入名(先读这段):** crate 发布名为 **`bao-core`**,库名钉定为 **`bao`**——`Cargo.toml` 写 `bao-core`,代码写 `use bao::…`:

```toml
[dependencies]
bao-core = "0.0.1"
```

```rust
use bao::{BaoConfig, BaoRuntime};   // ← 是 `bao`,不是 `bao_core`
```

### 接入路径一 —— 浏览器嵌入(主路径)

顶层 `BaoRuntime` → `create_page` → `navigate` → 执行 JS → 截图
(片段取自仓库 `examples/01-browser`):

```rust,no_run
use std::time::Duration;
use bao::{BaoConfig, BrowserError, PageConfig, PageState, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    // 一个 runtime = servo + SpiderMonkey + 内置 CDP/Node/Stealth。
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig::default())?;

    page.navigate("https://example.com")?;
    // servo 硬约束:求值前必须等 pipeline 就绪,否则可能 SIGSEGV。
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;

    // Page Realm:只有 Web API(没有 require/fs)。
    let title = page.evaluate_js_web("document.title")?;

    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let state: PageState = page.get_state();
    let _ = (title, png, state);
    Ok(())
}
```

### 接入路径二 —— CDP 自动化(Playwright 风格)

用库配置 `BaoConfig::cdp_port` 启动内置 CDP server,客户端可同进程零拷贝连接,或走 WebSocket(Playwright/Puppeteer 可直连同一 URL):

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
    // 进程内传输——或 "ws://127.0.0.1:9222"(Playwright/Puppeteer 同址可连)。
    let mut browser = Browser::connect("memory://bao")?;
    let _version = browser.version()?;   // CDP Browser.version
    let _targets = browser.pages()?;     // 等价 GET /json/list
    Ok(())
}
```

### 接入路径三 —— 页面内使用 Node/Bun API(双 Realm)

`page.evaluate_js` 在 **Node Realm** 执行:同一全局作用域同时拥有 DOM
*和* `require` / `fs` / `fetch` / `Bun` / `process`(`Bao` 是同一 `Bun`
对象的别名)。片段取自 `examples/03-node-dom` / `examples/04-crawler`:

```rust,no_run
# use std::time::Duration;
# use bao::{BaoConfig, BrowserError, PageConfig};
# fn main() -> Result<(), BrowserError> {
#     let runtime = BaoRuntime::new(BaoConfig::default())?;
#     let page = runtime.create_page(&PageConfig::default())?;
#     page.navigate("https://example.com")?;
#     page.wait_for_pipeline_ready(Duration::from_secs(30))?;
let script = r#"
    const h1 = document.querySelector('h1')?.textContent ?? "(none)"; // DOM(servo)
    const fs  = require('fs');                                         // Node API(Bao)
    const txt = fs.readFileSync('demo.txt', 'utf8');
    const res = await fetch('https://example.com/robots.txt');        // Node fetch
    JSON.stringify({ h1, txt, status: res.status })
"#;
let json = page.evaluate_js(script)?;   // Node Realm——Web 与 Node/Bun 同一作用域
let _ = json;
#     Ok(())
# }
```

不带页面的 Node/Bun host 搭建用 `bao::runtime::`(重导出的 `bun_runtime`
面,见下方同名陷阱)。

### ⚠ 同名陷阱:两个 `BaoRuntime`

`bao::runtime::BaoRuntime`(bun_runtime 的 Node/Bun API host)**不是**顶层
`bao::BaoRuntime`(浏览器协调器)。规则直引 `src/bao/src/lib.rs`:

> *浏览器嵌入 → 用顶层 `bao::BaoRuntime`;
> Node/Bun host 搭建 → 用 `bao::runtime::`*。

### 硬约束与前置(开始前必读)

- **JSContext 线程局部。** DOM ↔ Node.js 互操作必须发生在创建线程上。
  跨线程传 `JSObject` 指针会破坏 activation 栈 → SIGSEGV。跨线程只传
  `PageId` / 句柄 / 序列化数据。跳过 `wait_for_pipeline_ready()` 直接
  求值同理。
- **全栈恒链——没有 Cargo feature 开关**可以关掉浏览器/CDP/stealth/Node。
  行为在*运行时*选择(`StealthProfile`、`Permission` 守卫)。
- **首次构建从源码编译 SpiderMonkey**(需 clang、python3、make;首次
  20–40 分钟,之后走缓存)。
- **环境变量**:`BUN_*` 生效,`BAO_<SUFFIX>` 启动时别名到
  `BUN_<SUFFIX>`(如 `BUN_BUNFIG` ≡ `BAO_BUNFIG`)。

## 能力矩阵

| 领域 | 你得到什么 |
|---|---|
| 浏览器引擎 | Servo DOM/CSS/布局/渲染,多页面 `PagePool` / `PageHandle`,截图 |
| JS 引擎 | SpiderMonkey,线程局部 JSContext,双 Realm(Web / Node+Bun) |
| Node/Bun API | `require`、`fs`、`http`、`crypto`、`bun:sqlite`、`fetch`、`Bun.*`(= `Bao.*`),始终在线 |
| CDP | `BaoConfig::cdp_port` 启动 server(`ws://…`,Playwright/Puppeteer 兼容)+ Playwright 风格 Rust 客户端(`Browser::connect("memory://bao" \| ws URL)`) |
| Stealth | TLS JA3/JA4、HTTP/2、Canvas/WebGL/Audio/Navigator/行为指纹;运行时 `StealthProfile` |

## crates.io 包族

`bao-core` 是你唯一需要依赖的包;以下为需要细粒度切片时的直用面:

| 包 | 角色 |
|---|---|
| `bao-core` | 统一门面——本 README 的接入面 |
| `bao-engine` / `bun-sm` | SpiderMonkey 引擎层 |
| `bao-browser` | Servo 嵌入:`BaoRuntime`、`PagePool`、`PageHandle` |
| `bao-stealth` | 反指纹引擎 + `StealthProfile` |
| `bao-cdp` / `bao-cdp-client` | CDP server 面 / Playwright 风格客户端 |
| `bun-runtime` | Node.js/Bun API 兼容 host |
| `bun-*` | 基座层(base64、zlib、http、dns、resolver、transpiler……) |
| `bao-mozjs`、`bao-mozjs-sys`(+ `bao-mozjs-src-*`)、`bao-servo-*`、`bao-stylo`、`bao-ipc-channel` | 以一等包形态维护的 fork |

## 许可证

MPL-2.0(SpiderMonkey + Servo)· MIT(Bun 系 crate)。见
`LICENSE-MPL-2.0` / `LICENSE-MIT` 与 `THIRD_PARTY_LICENSES.md`。

---

*仓库开发:clone [putao520/bao](https://github.com/putao520/bao);`examples/` 内含本文片段取材的四个可运行示例。*
