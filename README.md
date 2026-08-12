# Bao

**A Rust-native programmable browser runtime built on Servo and SpiderMonkey.**

Run Web APIs, Node/Bun-compatible APIs, and browser automation — all inside one
Rust runtime. No Chromium, no Node child process, no Playwright-to-Chrome bridge:
the browser engine *is* the runtime.

> Status: **v0.1.0-alpha** — Linux x86_64 only · APIs may change · not production-ready.
> See [CHANGELOG](./CHANGELOG.md) · [Compatibility Matrix](#compatibility-matrix)

```
Servo · SpiderMonkey · Rust · CDP · Playwright · Node/Bun APIs
```

---

## Why Bao?

Most browser automation stacks are *layers on top of* a browser process:

```
App → Playwright → CDP/WebSocket → Chrome process → V8
```

Bao collapses the stack. The browser engine and the JS runtime are one process,
and that same runtime also speaks Node/Bun APIs:

```
        Bao
 ┌──────────────┐
 │   Servo      │   ← DOM / CSS / Layout / WebRender (real engine)
 │ SpiderMonkey │   ← per-thread JSContext (the JS runtime)
 │   Bun APIs   │   ← require / fs / http / crypto / bun:sqlite ...
 └──────────────┘
        ↑
   CDP (ws) ──── Playwright / Puppeteer / Rust `Browser` API
```

The key difference: **DOM and Node/Bun APIs live in one programmable browser
runtime.** A page script can call `document.querySelector(...)` while a trusted
script in the same runtime calls `require('fs')` — coexisting yet isolated via
the [dual-realm model](#dual-realm-security-model).

That makes Bao useful as:

- an **embeddable browser engine** (drive Servo from Rust, no Chromium),
- a **server-side render/automation runtime** (Web + Node APIs together),
- a **CDP target** (point Playwright/Puppeteer at it like Chrome),
- a **configurable-identity browser** (TLS/HTTP2/Canvas/Navigator profiles).

> [Browser identity & privacy](#browser-identity--privacy) profiles are a
> capability of Bao, not its whole purpose. The headline is *programmable
> browser runtime*, not "anti-fingerprint browser".

---

## Quick Start

```bash
git clone https://github.com/putao520/bao.git && cd bao
./scripts/bootstrap.sh        # one-shot: toolchain + build + `bao doctor`
                             # (first build compiles SpiderMonkey — slow)
```

Or manually:

```bash
cargo build -p bao_bin        # target/debug/bao
bao doctor                    # check your environment
bao run -e 'console.log(1 + 1)'          # Node/Bun APIs always available
```

### Local CI with `just`

CI runs **locally** — mozjs compiles SpiderMonkey from source, which is too slow
to be practical on hosted CI runners. The [`justfile`](./justfile) is the
authoritative "does it pass" entry point (`just` is required).

Two ways to run CI locally — **same result, pick one**:

```bash
# (A) Direct cargo recipes — fast feedback, all force --jobs 1 (mozjs EBUSY patch)
just ci          # fmt + lint + check + test + bce
just ci-fast     # skip the slow test suite
just bce         # BCE regression gates (Bug-Class Eradication)
just smoke       # `bao browser` under Xvfb + verify CDP (needs xvfb-run)

# (B) Reuse the GitHub Actions workflows via `act` (needs docker daemon)
#     GHA and local share ONE workflow source-of-truth, no duplication.
just gha-list    # show all workflow jobs act recognizes
just gha-ci      # run .github/workflows/ci.yml in a local container
just gha-smoke   # run browser-smoke.yml locally
just             # list all recipes
```

### 30-second demo

```js
// The same runtime runs DOM *and* Node code.
console.log(document.title);                    // → Web API (DOM)
console.log(require('fs').readdirSync('.').length);  // → Node API
```

```js
// Point Playwright at Bao like it's Chrome.
const { chromium } = require('playwright');
const browser = await chromium.connectOverCDP('http://127.0.0.1:9222');
const page = await browser.newPage();
await page.goto('https://example.com');
console.log(await page.title());               // backed by Servo, not Chromium
```

---

## Compatibility Matrix

What works today. **Not everything is green** — partial/experimental is listed
honestly; that's the point. Measured pass rates live in `compat/` (in progress).

| Capability | Bao | Status |
|---|:---:|---|
| HTML DOM | ✓ | Servo |
| CSS Layout | ✓ | Servo |
| WebRender | ✓ | Servo |
| JavaScript (SpiderMonkey) | ✓ | mozjs |
| CommonJS / ESM | ✓ | Bao/Bun |
| `fs` / `path` / `crypto` / `http` | ✓ | Bao/Bun |
| `bun:sqlite` / `bun:ffi` | ✓ | Bao/Bun |
| WebSocket | Partial | — |
| CDP Page / Runtime / DOM / Network | ✓ | 12 domains |
| CDP Debugger | Partial | — |
| Playwright over CDP | Experimental | connect/eval/screenshot |
| Puppeteer over CDP | Experimental | — |
| Platform: Linux x86_64 | ✓ | primary |
| Platform: macOS / Windows | — | event loop not yet proven |

---

## 核心特性

| 特性 | 说明 |
|------|------|
| **SpiderMonkey 引擎** | 替代 JSC(mozjs crate,MPL-2.0)。全局唯一 JSEngine + 每个 ScriptThread 线程局部 JSContext(servo 上游模型) |
| **servo 全功能浏览器** | DOM + CSS + Layout + webrender 渲染 + 截图,真实渲染引擎而非 headless 模拟 |
| **Node.js/Bun API 始终在线** | `require` / `fs` / `path` / `crypto` / `http` / `process` / `bun:sqlite` / `bun:ffi` 与 Web API 同一 JSContext 共存,无需切换运行时 |
| **浏览器身份与隐私**(Browser Identity & Privacy) | 可配置的 TLS JA3/JA4(匹配 Firefox/Chrome) · HTTP/2 AKAMAI fingerprint · Canvas/WebGL/Audio 隐私 · Navigator/Screen profile · 输入行为模拟(贝塞尔鼠标路径 + 拟人点击/打字) |
| **Headless 多页面库** | `PagePool` 多页面管理 + idle 回收;`PageHandle` 高层 API(navigate / evaluate / screenshot) |
| **CDP 自动化** | 内置 CDP Server,12 个域(Page/Runtime/DOM/Network/Debugger/Input/Emulation/CSS/Overlay/Log/Fetch/Target),Playwright/Puppeteer 兼容 |
| **Bun crate 100% 复用** | ~76 个 Bun 纯 Rust crate 零修改复用(HTTP/Resolver/Bundler/DNS/Base64/...) |

## 构建安装

```bash
# 克隆
git clone https://github.com/putao520/bao.git && cd bao

# 构建(首次构建 mozjs 从源码编译 SpiderMonkey,耗时较长；需 nightly + clang/C++)
cargo build

# 构建 CLI 二进制
cargo build -p bao_bin
# 产物: target/debug/bao

# 校验统一库 package
cargo check -p bao
cargo test -p bao

# 全仓测试(更重)
cargo test
```

> **mozjs 首次编译**:从源码编译 SpiderMonkey,需要 C++ 编译器 + 较长时间。已内置 EBUSY patch,`cargo test` 默认多线程零 SIGSEGV。  
> **平台**:当前主路径以 **Linux x86_64** 为主；macOS/Windows 事件循环尚未全平台证明可编。

## 快速开始

### 例 1:运行 JS 脚本(Bun 兼容)

```bash
# 运行文件(Node.js API 可用)
bao run index.js
bao run index.mjs           # ESM
bao run --module index.mjs  # 强制 ESM

# 内联代码(-e / --eval)
bao run -e "console.log(require('fs').readFileSync('/etc/hostname', 'utf8'))"
bao -e "console.log(Bun.version)"   # 顶层 -e 等价于 bao run -e
```

`index.js` 示例:

```js
// Node.js API 始终可用
const fs = require('fs');
const zlib = require('zlib');
console.log('cwd files:', fs.readdirSync('.').length);
console.log('platform:', process.platform);
console.log('node version:', process.version);

// Bun.* / Bao.* 是同一对象别名(Bun.version 已桥接)
console.log('Bun version:', Bun.version);

// node:zlib 在线
const bytes = zlib.deflateSync(Buffer.from('hello bao'));
console.log('deflated bytes:', bytes.length);
```

### 例 2:启动浏览器(交互 / 反指纹访问)

```bash
# 启动浏览器(默认 headless + CDP on 9222)
bao browser

# 指定 URL + 反指纹(Firefox profile)
bao browser --url https://bot.sannysoft.com --stealth

# 自定义 CDP 端口
bao browser --url https://example.com --cdp-port 9333 --stealth
```

`bao browser` flags:

| flag | 默认 | 说明 |
|------|------|------|
| `--url <URL>` | — | 启动后导航到的 URL |
| `--cdp-port <PORT>` | `9222` | CDP Server 监听端口(Playwright/Puppeteer 连此) |
| `--headless` / `--no-headless` | `true` | 是否无头 |
| `--stealth` | `false` | 启用 Firefox StealthProfile(TLS/HTTP2/Canvas/Navigator/WebGL/Audio/行为模拟) |

> `--stealth` 当前在 CLI 上使用 `StealthProfile::firefox_default()`。要在 Rust 侧用 Chrome profile,见例 4。

### 例 3:反指纹访问指纹检测网站

```bash
# 用 Firefox 反指纹 profile 访问经典指纹检测站
bao browser --url https://bot.sannysoft.com --stealth

# 另一个窗口用 Playwright 连上 CDP 抓截图
npx playwright-cli screenshot --cdp-endpoint http://127.0.0.1:9222 \
  --output sannysoft.png --full-page
```

Stealth 各维度(都由 `StealthProfile::firefox_default()` 自动启用):

| 维度 | 实测目标 |
|------|---------|
| TLS | JA3 / JA4 匹配 Firefox |
| HTTP/2 | AKAMAI fingerprint(SETTINGS + PRIORITY frame 模式) |
| Canvas | per-pixel 噪声注入(每 profile seed 不同) |
| WebGL | vendor / renderer override(VENDOR/RENDERING 时 `UNMASKED_`) |
| Audio | AudioContext fingerprint 噪声 |
| Navigator | userAgent / vendor / platform / hardwareConcurrency / deviceMemory |
| Screen | width / height / colorDepth |
| Behavior | 贝塞尔曲线鼠标路径 + 拟人点击/打字延迟 |

### 例 4:Headless 多页面库(Rust API)

```rust
// 统一库入口：只依赖 package `bao`（整栈始终链接，无产品 feature 拆分）
use bao::{BaoConfig, BaoRuntime, PageConfig, ScreenshotFormat, StealthProfile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 浏览器运行时(全局唯一 servo + 每页一线程)
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    let pool = runtime.page_pool();

    // 创建 Firefox 反指纹页面
    let page = pool.create_page(&PageConfig {
        url: Some("https://example.com".into()),
        stealth_profile: Some(StealthProfile::firefox_default()),
        ..Default::default()
    })?;

    // 可信脚本 — Node.js + Web API + DOM 全权限(Node Realm 注入)
    let title = page.evaluate_js("document.querySelector('h1').textContent")?;
    let files = page.evaluate_js(
        "require('fs').readdirSync('.').slice(0,5).join(',')"
    )?;
    println!("title={title}, files={files}");

    // 页面脚本 — 仅 Web API + DOM(Page Realm,typeof require === 'undefined')
    let ua = page.evaluate_js_web("navigator.userAgent")?;
    println!("ua={ua}");

    // 截图
    let png: Vec<u8> = page.take_screenshot(ScreenshotFormat::Png)?;
    std::fs::write("example.png", png)?;

    page.close()?;
    Ok(())
}
```

### Dual-Realm Security Model(双层 JS 隔离)

Node Realm + Page Realm 隔离:

| 方法 | 可用 API | Realm |
|------|---------|-------|
| `evaluate_js()` | Node.js + Web API + DOM | Node Realm(独立 Compartment,脚本通过 Node global 执行) |
| `evaluate_js_web()` | 仅 Web API + DOM | Page Realm(Window global,`typeof require === 'undefined'`) |
| 页面 JS | 仅 Web API + DOM | Page Realm(Node.js API 不写入 Window global) |

**多页面管理**:

```rust
let pool = runtime.page_pool();

let page_a = pool.create_page(&PageConfig {
    url: Some("https://a.com".into()),
    ..Default::default()
})?;
let page_b = pool.create_page(&PageConfig {
    url: Some("https://b.com".into()),
    stealth_profile: Some(StealthProfile::chrome_default()),
    ..Default::default()
})?;

let stats = pool.stats();
println!("active={}, idle={}, total_created={}",
    stats.active, stats.idle, stats.total_created);

pool.check_idle_pages();   // 回收 idle_ttl(默认 60s)超时的空闲页
pool.close_all();
```

### 例 5:CDP 自动化(Playwright / Puppeteer 连接)

```bash
# 启动带 CDP 的浏览器
bao browser --url https://example.com --cdp-port 9222 --stealth
```

CDP 端点:

| 端点 | 用途 |
|------|------|
| `http://127.0.0.1:9222/json/version` | 版本信息 |
| `http://127.0.0.1:9222/json/list` | 目标列表 |
| `ws://127.0.0.1:9222/devtools/page/{targetId}` | WebSocket 调试协议 |

Playwright 连接(Node.js):

```js
const { chromium } = require('playwright');

(async () => {
  // 连接 Bao 内置 servo 的 CDP
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9222');
  const context = browser.contexts()[0];
  const page = await context.newPage();
  await page.goto('https://example.com');
  console.log(await page.title());
  await page.screenshot({ path: 'cdp.png', fullPage: true });
  await browser.close();
})();
```

**Rust 侧用统一库 `bao`**(Playwright 风格高层 API,按 URL scheme 路由):

```rust
use bao::Browser;

// 同进程 servo(memory:// scheme → InMemoryTransport,零网络往返)
let browser = Browser::connect("memory://bao")?;
assert!(browser.is_in_memory());

// 或连外部 Chrome / Chromium(ws:// / http://)
let browser = Browser::connect("ws://127.0.0.1:9222")?;
assert!(browser.is_websocket());
```

支持 12 个 CDP 域:Page · Runtime · DOM · Network · Debugger · Input · Emulation · CSS · Overlay · Log · Fetch · Target。

## Browser Identity & Privacy

> 原 "Stealth 反指纹" 能力。重新定位为**可配置的浏览器身份与隐私 profile**——它是 Bao 的一项能力,而非全部目的。实现完整(`tls.rs` / `http2.rs` / `behavior.rs` / `canvas.rs` / `webgl_audio.rs` 均为独立大模块),通过 `StealthProfile` 配置驱动。

Stealth 反指纹 API:

```rust
use bao::{StealthEngine, StealthProfile};

// 预置 profile（运行时配置；整栈始终链接，不是 Cargo feature）
let ff = StealthProfile::firefox_default();   // Firefox ESR 指纹
let ch = StealthProfile::chrome_default();    // Chrome 指纹

// 各维度访问
let engine = StealthEngine::new(ff);
engine.tls_config();    // JA3/JA4 hash + cipher suites + extensions
engine.http2_config();  // AKAMAI HTTP/2 fingerprint + SETTINGS frame
engine.canvas_noise();  // per-pixel noise injection
engine.navigator();     // UA + vendor + platform + hardwareConcurrency
engine.screen();        // width + height + colorDepth
engine.webgl();         // vendor/renderer override
engine.audio();         // AudioContext fingerprint
engine.behavior();      // 鼠标路径 + 打字延迟 + 滚动模式
```

`StealthProfile` 是 `Clone + Debug` 的纯数据结构,可直接构造自定义 profile。每个 profile 的 Canvas/Audio/Behavior 用不同 seed 生成不同噪声。

## PageHandle API 速查

| 方法 | 返回 | 说明 |
|------|------|------|
| `navigate(url)` | `Result<(), BrowserError>` | 导航到新 URL |
| `evaluate_js(script)` | `Result<String, BrowserError>` | 可信脚本(Node.js + DOM,Node Realm) |
| `evaluate_js_web(script)` | `Result<String, BrowserError>` | 页面脚本(仅 Web API,Page Realm) |
| `take_screenshot(format)` | `Result<Vec<u8>, BrowserError>` | 截图(PNG / JPEG) |
| `page_title()` | `Option<String>` | 页面标题 |
| `current_url()` | `Option<String>` | 当前 URL |
| `get_state()` | `PageState` | 状态(Created / Navigating / Interactive / Idle / Closed) |
| `permission()` | `PermissionGuard` | 权限守卫 |
| `stealth_profile()` | `Option<StealthProfile>` | 当前 profile |
| `close()` | `Result<(), BrowserError>` | 关闭页面 |

## CLI 命令总览

```
bao                              # 必须带子命令(--help 查看)
bao -e, --eval <CODE>            # 顶层 eval(等价 bao run -e)
bao run [-e <CODE> | <FILE>]     # 运行 JS(--module 强制 ESM)
bao build <ENTRYPOINT>           # 打包(--target bun|node|browser|macro --format esm|cjs|iife --minify --sourcemap --outdir <DIR>)
bao test [FILES...]              # 测试运行器(默认扫 test/ tests/ __tests__/)
bao install [ARGS...]            # 安装依赖(转发给 bun_install)
bao browser [--url URL]          # 启动浏览器
        [--cdp-port PORT]        #   默认 9222
        [--headless | --no-headless]  # 默认 headless
        [--stealth]              #   启用 Firefox StealthProfile
```

## 架构概览

对外只暴露 **一个** Cargo package：`bao`（整栈始终链接）。下图中的 `bao_*` 是 monorepo **内部**分层，**不要**在宿主项目里分别 path 依赖。

```
┌──────────────────────────────────────────────────────────┐
│  公共 package: bao  (唯一对外 lib 入口)                    │
│  re-export: BaoRuntime / Browser / StealthProfile / …    │
├──────────────────────────────────────────────────────────┤
│                     bao CLI binary                        │
│            bao_bin → bao_cli (clap subcommands)           │
├────────────┬────────────┬──────────┬─────────────────────┤
│ (internal) │ (internal) │(internal)│ (internal)          │
│ bao_engine │ bao_browser│ bao_cdp  │ bao_stealth         │
│ SpiderMonkey│  Servo 桥  │ CDP WS   │ 反指纹              │
├────────────┴────────────┴──────────┴─────────────────────┤
│ (internal) bao_cdp_client / bao_runtime / bao_uloop       │
├──────────────────────────────────────────────────────────┤
│          Bun ~85 个纯 Rust crate(零修改复用)             │
├──────────────────────────────────────────────────────────┤
│ mozjs · libservo · boringssl · cdp-protocol              │
└──────────────────────────────────────────────────────────┘
```

| crate | 对外？ | 职责 |
|-------|--------|------|
| **`bao`** | **是（唯一公共 lib）** | 整栈 re-export；宿主只依赖此 package |
| `bao_engine` 等 | 否（内部） | monorepo 实现分层；见 [CLAUDE.md](./CLAUDE.md) |
| `bao_cli` / `bao_bin` | CLI 二进制 | `bao` 命令行入口 |

## 文档

- [CLAUDE.md](./CLAUDE.md) — 项目指令(架构原则、铁律、复用映射、SPEC 索引)
- [.spec/](./.spec/) — SPEC(单一真相来源):
  - `10-REQUIREMENTS.html` — 31 REQ(ENG/CLI/BRW/CDP/STL/LIB 六域)
  - `02-SYSTEM.html` — 系统架构(Bun Crate DAG + Servo 组件 + 融合映射)
  - `03-PROCESS.html` — 核心流程(JS 执行管线 · 渲染管线 · CDP 路由)
  - `05-IMPLEMENTATION.html` — 实施路线图(5 阶段)

## Cargo 依赖（嵌入到自己项目）

**只依赖一个公共 package：`bao`。** 整栈（引擎 + 浏览器 + Node/Bun API + CDP + Stealth）始终链接；**禁止**再写多个 `path = "src/bao_*"` 依赖；**不使用** Cargo product features 拆分能力。

```toml
[dependencies]
# 本仓库内嵌（package 路径是 src/bao，crate 名是 bao）
bao = { path = "../bao/src/bao" }

# 或 git
# bao = { git = "https://github.com/putao520/bao", package = "bao" }
```

```rust
use bao::{BaoConfig, BaoRuntime, Browser, PageConfig, StealthProfile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _runtime = BaoRuntime::new(BaoConfig::default())?;
    let _ = StealthProfile::firefox_default();
    let _browser = Browser::connect("ws://127.0.0.1:9222")?;
    Ok(())
}
```

| 错误（不要这样写） | 正确 |
|--------------------|------|
| `bao_browser = { path = "src/bao_browser" }` 等一串 | 仅 `bao = { path = "…/src/bao" }` 或 git `package = "bao"` |
| `use bao_browser::…` / `use bao_stealth::…` | `use bao::…` |

完整方案见 [docs/unified-library-integration.md](./docs/unified-library-integration.md)。  
构建前提（fail-closed）：nightly + clang/C++ + `vendor/`（mozjs/servo/boringssl 等）。

## 许可证

MPL-2.0(SpiderMonkey + Servo) + MIT(Bun crates)
