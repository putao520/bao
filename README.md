# Bao (包子)

**A high-performance anti-fingerprint browser runtime in a single Rust stack** — SpiderMonkey (JS engine) + Servo (full browser engine) + always-on Node.js/Bun API compatibility + built-in Stealth, in one Rust runtime. No Chromium, no Node child process, no automation bridge: the browser engine *is* the runtime.

**[中文文档](./README.zh-CN.md)** · Status: **0.x alpha** — Linux x86_64 · APIs may change · [CHANGELOG](./CHANGELOG.md)

---

## Using the library

**Package name vs import name (read this first):** the crate is published as
**`bao-core`**, the library name is pinned to **`bao`** — your `Cargo.toml`
says `bao-core`, your code says `use bao::…`:

```toml
[dependencies]
bao-core = "0.0.1"
```

```rust
use bao::{BaoConfig, BaoRuntime};   // ← `bao`, not `bao_core`
```

### Integration path 1 — embed the browser (primary)

Top-level `BaoRuntime` → `create_page` → `navigate` → JS → screenshot
(snippet adapted from `examples/01-browser` in the repository):

```rust,no_run
use std::time::Duration;
use bao::{BaoConfig, BrowserError, PageConfig, PageState, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    // One runtime = servo + SpiderMonkey + built-in CDP/Node/Stealth.
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig::default())?;

    page.navigate("https://example.com")?;
    // Hard servo constraint: wait for the pipeline before evaluating,
    // or you risk SIGSEGV.
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;

    // Page Realm: Web API only (no require/fs here).
    let title = page.evaluate_js_web("document.title")?;

    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let state: PageState = page.get_state();
    let _ = (title, png, state);
    Ok(())
}
```

### Integration path 2 — CDP automation (Playwright-style)

Start the CDP server from library config — `BaoConfig::cdp_port` — then
connect the Playwright-style client in-process (zero-copy, no port) or over
WebSocket:

```rust,no_run
use bao::{Browser, BaoConfig, BrowserError, ConnectError};

fn start_runtime_with_cdp() -> Result<(), BrowserError> {
    // cdp_port starts the built-in CDP server on ws://127.0.0.1:<port>.
    let _runtime = BaoRuntime::new(BaoConfig {
        cdp_port: Some(9222),
        ..BaoConfig::default()
    })?;
    Ok(())
}

fn connect() -> Result<(), ConnectError> {
    // In-process transport — or "ws://127.0.0.1:9222" (Playwright/Puppeteer
    // can point at the same URL).
    let mut browser = Browser::connect("memory://bao")?;
    let _version = browser.version()?;   // CDP Browser.version
    let _targets = browser.pages()?;     // equivalent of GET /json/list
    Ok(())
}
```

### Integration path 3 — Node/Bun APIs inside the page (dual realm)

`page.evaluate_js` runs in the **Node Realm**: the same global scope has the
DOM *and* `require` / `fs` / `fetch` / `Bun` / `process` (`Bao` is an alias
of the same `Bun` object). Snippet adapted from `examples/03-node-dom` /
`examples/04-crawler`:

```rust,no_run
# use std::time::Duration;
# use bao::{BaoConfig, BrowserError, PageConfig};
# fn main() -> Result<(), BrowserError> {
#     let runtime = BaoRuntime::new(BaoConfig::default())?;
#     let page = runtime.create_page(&PageConfig::default())?;
#     page.navigate("https://example.com")?;
#     page.wait_for_pipeline_ready(Duration::from_secs(30))?;
let script = r#"
    const h1 = document.querySelector('h1')?.textContent ?? "(none)"; // DOM (servo)
    const fs  = require('fs');                                         // Node API (Bao)
    const txt = fs.readFileSync('demo.txt', 'utf8');
    const res = await fetch('https://example.com/robots.txt');        // Node fetch
    JSON.stringify({ h1, txt, status: res.status })
"#;
let json = page.evaluate_js(script)?;   // Node Realm — Web + Node/Bun in one scope
let _ = json;
#     Ok(())
# }
```

For Node/Bun host setup without a page, the `bun_runtime` surface is
re-exported at `bao::runtime::` (see the trap note below).

### ⚠ Same-name trap: two `BaoRuntime` types

`bao::runtime::BaoRuntime` (the Node/Bun API host from `bun_runtime`) is
**not** the top-level `bao::BaoRuntime` (the browser coordinator). Rule of
thumb, straight from `src/bao/src/lib.rs`:

> *browser embedding → use the top-level `bao::BaoRuntime`;
> Node/Bun host setup → use `bao::runtime::`*.

### Hard constraints & prerequisites (be aware before you start)

- **JSContext is thread-local.** DOM ↔ Node.js interop must happen on the
  creating thread. Passing `JSObject` pointers across threads corrupts the
  activation stack → SIGSEGV. Cross-thread, pass `PageId` / handles /
  serialized data only. Same rule for skipping
  `wait_for_pipeline_ready()` before evaluating.
- **The full stack is always linked — there are no Cargo feature toggles**
  for browser/CDP/stealth/Node. Behaviour is selected at *runtime*
  (`StealthProfile`, `Permission` guards).
- **First build compiles SpiderMonkey from source** (clang, python3, make
  required; expect 20–40 min once — cached afterwards).
- **Environment variables**: `BUN_*` is honoured, and `BAO_<SUFFIX>` is
  aliased onto `BUN_<SUFFIX>` at startup (e.g. `BUN_BUNFIG` ≡ `BAO_BUNFIG`).

## Capability matrix

| Area | What you get |
|---|---|
| Browser engine | Servo DOM/CSS/layout/render, multi-page `PagePool` / `PageHandle`, screenshots |
| JS engine | SpiderMonkey, thread-local JSContext, dual realms (Web / Node+Bun) |
| Node/Bun API | `require`, `fs`, `http`, `crypto`, `bun:sqlite`, `fetch`, `Bun.*` (= `Bao.*`), always on |
| CDP | Server started via `BaoConfig::cdp_port` (`ws://…`, Playwright/Puppeteer compatible) + Playwright-style Rust client (`Browser::connect("memory://bao" \| ws URL)`) |
| Stealth | TLS JA3/JA4, HTTP/2, Canvas/WebGL/Audio/Navigator/behavior fingerprints; runtime `StealthProfile` |

## Package family on crates.io

`bao-core` is the package you depend on; everything below it is published
for direct use when you need a slice:

| Package | Role |
|---|---|
| `bao-core` | Unified facade — the integration surface in this README |
| `bao-engine` / `bun-sm` | SpiderMonkey engine layer |
| `bao-browser` | Servo embedding: `BaoRuntime`, `PagePool`, `PageHandle` |
| `bao-stealth` | Anti-fingerprint engine + `StealthProfile` |
| `bao-cdp` / `bao-cdp-client` | CDP server surface / Playwright-style client |
| `bun-runtime` | Node.js/Bun API compatibility host |
| `bun-*` | Base layer (base64, zlib, http, dns, resolver, transpiler, …) |
| `bao-mozjs`, `bao-mozjs-sys` (+ `bao-mozjs-src-*`), `bao-servo-*`, `bao-stylo`, `bao-ipc-channel` | Maintained forks vendored as first-class packages |

## License

MPL-2.0 (SpiderMonkey + Servo) · MIT (Bun-derived crates). See
`LICENSE-MPL-2.0` / `LICENSE-MIT` and `THIRD_PARTY_LICENSES.md`.

---

*Developing the repository itself: clone [putao520/bao](https://github.com/putao520/bao); the `examples/` directory contains the four runnable samples these snippets are adapted from.*
