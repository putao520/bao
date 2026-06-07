# Bao (包子) — Node.js + 浏览器 + 反指纹，一个运行时全搞定

Bun 的 SpiderMonkey 引擎分支，融合 Servo 浏览器引擎。Node.js API 始终在线，浏览器始终可用，反指纹内置。

## 核心差异化

| 特性 | 说明 |
|------|------|
| **SpiderMonkey 引擎** | 替代 JSC，与 Servo 共享同一 JSContext。DOM 对象和 Node.js 对象原生互操作 |
| **全功能浏览器** | DOM + CSS + 布局 + 渲染 + 截图，Servo 真实渲染引擎，非 headless 模拟 |
| **Node.js/Bun API 始终在线** | require/fs/crypto/http/process 与 Web API 同一上下文共存，无需切换运行时 |
| **反指纹内置** | TLS JA3/JA4 + HTTP/2 Akamai + Canvas 噪声 + Navigator/Screen/WebGL/Audio + 行为模拟，开箱即用 |

## CLI 快速开始

```bash
# 运行 JS 脚本（Node.js API 可用）
bao run index.js
bao run -e "console.log(require('fs').readFileSync('/etc/hostname', 'utf8'))"

# 启动浏览器（带 CDP + 反指纹）
bao browser --url https://example.com --cdp-port 9222 --stealth

# 打包
bao build src/index.ts --target bundle --minify

# 测试
bao test

# 安装依赖
bao install lodash
```

## Rust 库使用方案

### 场景 A：纯 Node.js 运行时（无浏览器）

```rust
use bao_engine::BaoRuntime;

let mut rt = BaoRuntime::new()?;
rt.eval("const fs = require('fs'); console.log(fs.readdirSync('.').length)", "<eval>")?;
```

### 场景 B：浏览器 + Node.js 双层 JS 模型（核心场景）

```rust
use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool, ScreenshotFormat};
use bao_stealth::StealthProfile;

let runtime = BaoRuntime::new(BaoConfig::default())?;
let pool = runtime.page_pool();

// 创建页面（可选 stealth profile）
let page = pool.create_page(&PageConfig {
    url: Some("https://example.com".into()),
    stealth_profile: Some(StealthProfile::firefox_default()),
    ..Default::default()
})?;

// 可信脚本 — Node.js + DOM 全权限
let title = page.evaluate_js("document.querySelector('h1').textContent")?;
let files = page.evaluate_js("require('fs').readdirSync('.').join(',')")?;

// 页面脚本 — 仅 Web API，Node.js 不可见
let ua = page.evaluate_js_web("navigator.userAgent")?;

// 截图
let png: Vec<u8> = page.take_screenshot(ScreenshotFormat::Png)?;

page.close()?;
```

**双层 JS 安全模型**：

| 方法 | 可用 API | 机制 |
|------|---------|------|
| `evaluate_js()` | Node.js + Web API + DOM | CommonJS 参数注入（IIFE），scope 执行后自动清理 |
| `evaluate_js_web()` | 仅 Web API + DOM | 标准 web 沙箱，`typeof require === 'undefined'` |
| 页面 JS | 仅 Web API + DOM | Node.js API 不写入 Window global |

`evaluate_js()` 将脚本包装为：

```js
(function() {
  var __scope = globalThis.__bao_privileged_apis;
  delete globalThis.__bao_privileged_apis;
  delete globalThis.__bao_setEnv;
  delete globalThis.__bao_delEnv;
  delete globalThis.Buffer;
  if (!__scope) throw new Error('Bao: privileged API scope not available');
  (function(require, module, exports, Bun, process, Buffer, __filename, __dirname) {
    <your_script>
  })(__scope.require, __scope.module, __scope.module.exports, __scope.Bun, __scope.process, __scope.Buffer, __scope.__filename, __scope.__dirname);
})();
```

Node API 作为函数参数传入，执行后不残留 globalThis。servo script thread 单线程，无时序攻击窗口。

### 场景 C：浏览器 + CDP 调试

```rust
use bao_browser::{BaoConfig, BaoRuntime, BrowserConfig, run_browser};
use bao_stealth::StealthProfile;

// 方式 1：BaoConfig 设置 cdp_port
let runtime = BaoRuntime::new(BaoConfig {
    cdp_port: Some(9222),
    ..Default::default()
})?;

// 方式 2：run_browser 便捷函数（阻塞运行）
run_browser(BrowserConfig {
    url: Some("https://example.com".into()),
    cdp_port: 9222,
    stealth_profile: Some(StealthProfile::firefox_default()),
    ..Default::default()
})?;
```

CDP 端点：

| 端点 | 用途 |
|------|------|
| `http://127.0.0.1:9222/json/version` | 版本信息 |
| `http://127.0.0.1:9222/json/list` | 目标列表 |
| `ws://127.0.0.1:9222/devtools/page/{targetId}` | WebSocket 调试协议 |

支持 12 个 CDP 域：Page, Runtime, DOM, Network, Debugger, Input, Emulation, CSS, Overlay, Log, Fetch, Target。

## 反指纹 API

```rust
use bao_stealth::{StealthEngine, StealthProfile};

// Firefox 指纹（默认）
let profile = StealthProfile::firefox_default();
let engine = StealthEngine::new(profile);

// 各维度访问
engine.navigator();     // UA string, vendor, platform, hardwareConcurrency
engine.screen();        // width, height, colorDepth
engine.tls_config();    // JA3/JA4 hash, cipher suites, extensions
engine.http2_config();  // Akamai HTTP/2 fingerprint, SETTINGS frame
engine.canvas_noise();  // per-pixel noise injection
engine.webgl();         // vendor/renderer override
engine.audio();         // AudioContext fingerprint
engine.behavior();      // mouse path, typing delays, scroll patterns
```

预置两种 profile：

- `StealthProfile::firefox_default()` — Firefox ESR 指纹
- `StealthProfile::chrome_default()` — Chrome 指纹

## PagePool 多页面管理

```rust
let pool = runtime.page_pool();

let page1 = pool.create_page(&PageConfig {
    url: Some("https://a.com".into()),
    ..Default::default()
})?;
let page2 = pool.create_page(&PageConfig {
    url: Some("https://b.com".into()),
    stealth_profile: Some(StealthProfile::firefox_default()),
    ..Default::default()
})?;

// 池统计
let stats = pool.stats();
println!("active: {}, idle: {}, total created: {}",
    stats.active, stats.idle, stats.total_created);

// 空闲页面回收（idle_ttl 默认 60s）
pool.check_idle_pages();

// 全部关闭
pool.close_all();
```

## PageHandle API 速查

| 方法 | 返回 | 说明 |
|------|------|------|
| `navigate(url)` | `Result<(), BrowserError>` | 导航到新 URL |
| `evaluate_js(script)` | `Result<String, BrowserError>` | 可信脚本执行（Node.js + DOM） |
| `evaluate_js_web(script)` | `Result<String, BrowserError>` | 页面脚本执行（仅 Web API） |
| `take_screenshot(format)` | `Result<Vec<u8>, BrowserError>` | 截图（PNG/JPEG） |
| `page_title()` | `Option<String>` | 页面标题 |
| `current_url()` | `Option<String>` | 当前 URL |
| `get_state()` | `PageState` | 页面状态（Created/Navigating/Interactive/Idle/Closed） |
| `close()` | `Result<(), BrowserError>` | 关闭页面 |

## 架构

```
┌─────────────────────────────────────────────┐
│                  bao (CLI)                   │
├──────────┬──────────┬──────────┬────────────┤
│ bao_engine│bao_browser│ bao_cdp │ bao_stealth│
│ SpiderMonkey│  Servo   │ CDP WS  │ 反指纹    │
│ Node.js API│ DOM/CSS  │ 12域    │ TLS/H2/   │
│ require/fs │ 渲染/截图│ Router  │ Canvas/   │
│ crypto/http│ PagePool │ Session │ Navigator │
├──────────┴──────────┴──────────┴────────────┤
│           Bun ~85 Rust crates (复用)          │
└─────────────────────────────────────────────┘
```

**单 JSContext 融合**：servo 创建 JSContext，bao_engine 寄生同一指针。所有模式（CLI/browser/CDP）共享唯一 JSContext，DOM 对象和 Node.js 对象原生互操作，零序列化开销。

## Cargo 依赖

```toml
[dependencies]
bao_browser = { path = "src/bao_browser" }
bao_stealth = { path = "src/bao_stealth" }
bao_cdp     = { path = "src/bao_cdp" }
bao_engine  = { path = "src/bao_engine" }
```

## 许可证

MPL-2.0 (SpiderMonkey + Servo) + MIT (Bun crates)
