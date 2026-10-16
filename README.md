# Bao (包子)

**Bao is a Rust library that gives you both a Bun/Node-compatible system runtime and a real browser (Servo DOM + CDP) in one crate, so a Rust project can embed it directly with one dependency — no Node process, no Chromium sidecar.**

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

`evaluate_js` runs this in the host-controlled Node Realm: `document`, `require('fs')`, and `fetch` share one global ([`examples/03-node-dom`](examples/03-node-dom)). Page scripts use `evaluate_js_web` and do not get `require` or `fs`. `wait_for_pipeline_ready` follows `navigate` because the Servo pipeline is created asynchronously. `demo.txt` is a file in the working directory.

**[中文文档](./README.zh-CN.md)** · Status: **0.x alpha** — Linux x86_64 · Windows x86_64 · APIs may change · [CHANGELOG](./CHANGELOG.md)

## Why not X

| Alternative | What stays outside the crate |
|---|---|
| **Playwright + Chromium** | Chromium is a separate browser process, driven over CDP. Page JavaScript and a Node/Bun script are different processes you package and supervise beside your binary. |
| **Node / Bun sidecar** | The system runtime is another process. Script lifecycle, permissions, files, and results cross a boundary the Rust host does not own. |
| **Deno / QuickJS / Rhai** | Deno is its own runtime process. QuickJS embeds JavaScript without this Node/Bun API set or Servo. Rhai is an expression language, not a JS system runtime with a browser. |

## Who it's for

- **Rust crawlers and anti-detection scraping.** Navigate a Servo page, read the DOM, and use `fs` / `fetch` in the same trusted script. `StealthProfile` is runtime configuration for TLS/HTTP fingerprints and browser-visible surfaces (Canvas, Navigator, WebGL, Audio, behavior).
- **Agent execution environments.** A host can run a short program that uses ordinary JS control flow (`for`, `try/catch`, `Promise.all`, modules) together with system and DOM APIs. Bao does not implement a planner, memory, or tool registry.
- **Automation and plugin layers inside Rust products.** Link the crate into the product. The host keeps task lifecycle, permissions, and native resources; scripts hold the part that changes.

## Install

The published package is **`bao-core`**. The library name is **`bao`**: `Cargo.toml` says `bao-core`, Rust code says `use bao::…`.

```toml
[dependencies]
bao-core = "0.3.12"
```

```rust
use bao::{BaoConfig, BrowserRuntime};
```

`0.3.12` is the current crates.io release. `bao::runtime::NodeRuntime` is the Node/Bun host when you do not need a page. Top-level `bao::BrowserRuntime` is the browser coordinator. The old `BaoRuntime` name remains a deprecated alias on both faces through 0.x.

## Important current limitations

Bao is still **0.x alpha**. In particular:

- Fully verified platforms today: Linux x86_64 and Windows x86_64 (cross-built from Linux via `scripts/win-cross-env.sh`, real-machine run + battery 2026-09-26). The full three-tier target matrix — each platform with an explicit tier, evidence, and upgrade condition — lives in [docs/platform-support.md](docs/platform-support.md).
- Node/Bun, Web, and CDP compatibility are substantial but not complete; handler/API existence is not treated as proof of semantic compatibility.
- Realm separation is **not** a finished arbitrary-untrusted-code sandbox. Fine-grained capability, quota, audit, and stronger isolation work is still ongoing.
- `JSContext` is thread-local. Never pass `JSObject` / GC pointers across threads; cross-thread paths must use ids, handles, owned messages, or serialized data and execute JS back on the owning thread.
- The full stack is currently always linked; there are no Cargo product features that remove browser/CDP/Stealth/Node layers. Runtime configuration selects behavior.
- The first build compiles SpiderMonkey from source and is intentionally heavier than a small scripting crate.
- Rust nightly is currently required; the repository pins the supported toolchain.
- macOS has compile-surface work but has not yet completed real-hardware build/link/test validation.

## Build prerequisites

- clang, python3, make (SpiderMonkey build)
- the repository-pinned Rust nightly toolchain
- Linux media playback requires the appropriate system GStreamer runtime libraries

The first SpiderMonkey build is cached afterwards. See repository build documentation for the current toolchain, platform, and media details.

## Package family on crates.io

`bao-core` is the consumer-facing facade. The published family also exposes lower-level pieces for users who need a specific slice:

| Package | Role |
|---|---|
| `bao-core` | Unified library facade |
| `bao-engine` / `bun-sm` | SpiderMonkey engine layer |
| `bun-runtime` | Node.js/Bun-style system runtime host |
| `bun-*` | Rust-native base layers: HTTP, resolver, install, crypto-related plumbing, bundler pieces, etc. |
| `bao-browser` | Servo embedding: `BrowserRuntime`, `PagePool`, `PageHandle` |
| `bao-cdp` / `bao-cdp-client` | CDP server surface / Playwright-style Rust client |
| `bao-stealth` | Stealth engine + `StealthProfile` |
| `bao-mozjs`, `bao-mozjs-sys`, `bao-mozjs-src-*`, `bao-servo-*`, `bao-stylo`, `bao-ipc-channel` | Maintained runtime/browser dependency family |

## Contributing

Read [CONTRIBUTING.md](./CONTRIBUTING.md). Bug reports go to [issues](https://github.com/putao520/bao/issues). Usage questions and design discussion go to [discussions](https://github.com/putao520/bao/discussions).

Clone [putao520/bao](https://github.com/putao520/bao). Runnable integration examples are in `examples/`.

## License

MPL-2.0 (SpiderMonkey + Servo) · MIT (Bun-derived crates). See `LICENSE-MPL-2.0`, `LICENSE-MIT`, and `THIRD_PARTY_LICENSES.md`.

---

## Other entry points

### System runtime without a page

If the task does not need a page, the Node/Bun host is `bao::runtime::NodeRuntime`, re-exported under `bao::runtime::*`.

### Page realm, screenshot, page state

`evaluate_js_web` stays in the Page Realm (Web APIs only). Screenshot and page state are on the page handle:

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

Bao exposes a CDP server and a Playwright-style Rust client. The client connects in-process through `memory://bao` or over WebSocket.

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

**Pump contract:** Servo-domain CDP commands (`Runtime.evaluate`, `Page.navigate`, …) execute on the runtime thread and currently need the host to drive that thread with `runtime.pump_cdp(Duration)` or the `run()` loop. Protocol-only commands such as `Browser.version` and `pages()` do not. An unpumped Servo-domain command times out instead of returning fake success.

CDP support is a compatibility boundary. Method coverage, event ordering, object lifecycle, and Playwright assumptions are still being tested. Bao is not Chrome.

## Current capability stack

| Layer | What Bao provides |
|---|---|
| Programmable language | SpiderMonkey-based JavaScript runtime |
| System runtime | Node/Bun-style modules and APIs: `require`, filesystem, HTTP/fetch, crypto, `bun:sqlite`, process/runtime primitives |
| Web runtime | Servo DOM/CSS/layout/rendering, multi-page `PagePool` / `PageHandle`, screenshots |
| Realm boundary | Page Realm for site code; host-controlled Node Realm for trusted system scripts |
| Automation compatibility | Built-in CDP surface + Playwright-style Rust client, including `memory://bao` in-process transport |
| Stealth | TLS/HTTP and browser-visible fingerprint controls through runtime `StealthProfile` configuration |

## Where Bao sits

A Rust product often wants a stable native core and a programmable layer for rules, automation, plugins, and short programs. A small expression engine runs out of room once scripts need files, networking, modules, or a page. A separate Node service moves lifecycle and failures across a process boundary.

Bao is that layer as a library: JS control flow, Node/Bun-style system APIs, and a Servo page when the task needs one, linked into the host. You can stay on the system runtime and open a page only for the step that needs DOM. A standalone JS/TS server or CLI is still Node's or Bun's job. A compact isolated plugin ABI may fit WASM better; realm separation is not a finished sandbox for arbitrary untrusted code.

The host keeps task lifecycle, permissions, application state, scheduling, files, sockets, database connections, page handles, cancellation, and shutdown.

```text
Rust application
│
├── native core
│   resources, scheduling, permissions, performance-sensitive code
│
└── Bao (one crate)
    JS control flow
    Node/Bun-style system APIs
    Servo page, when the task needs DOM
    CDP boundary
```
