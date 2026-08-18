# bao-core

**A high-performance anti-fingerprint browser runtime in a single Rust stack** — SpiderMonkey + Servo + always-on Node.js/Bun APIs + built-in Stealth and CDP.

**[中文文档](https://github.com/putao520/bao/blob/master/src/bao/README.zh-CN.md)**

> **Package name vs import name:** this package is `bao-core`; the library
> name is pinned to `bao`. `Cargo.toml` says `bao-core`, code says
> `use bao::…`.

```toml
[dependencies]
bao-core = "0.0.1"
```

## Usage — three entry points

### 1. Embed the browser (primary)

```rust,no_run
use std::time::Duration;
use bao::{BaoConfig, BrowserError, PageConfig, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    let runtime = BaoRuntime::new(BaoConfig::default())?;   // top-level coordinator
    let page = runtime.create_page(&PageConfig::default())?;
    page.navigate("https://example.com")?;
    // servo hard constraint — always wait for the pipeline before evaluating:
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;
    let title = page.evaluate_js_web("document.title")?;    // Page Realm (Web API only)
    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let _ = (title, png);
    Ok(())
}
```

### 2. CDP automation (Playwright-style)

Start the CDP server from library config (`BaoConfig::cdp_port`), then
connect in-process or over WebSocket:

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
    let mut browser = Browser::connect("memory://bao")?;    // in-process; or "ws://host:port"
    let _version = browser.version()?;                      // CDP Browser.version
    let _targets = browser.pages()?;                        // like GET /json/list
    Ok(())
}
```

### 3. Node/Bun APIs inside the page (dual realm)

`page.evaluate_js` runs in the **Node Realm**: DOM and `require` / `fs` /
`fetch` / `Bun` in one scope.

```js
const h1  = document.querySelector('h1')?.textContent;      // DOM (servo)
const txt = require('fs').readFileSync('demo.txt', 'utf8'); // Node API (Bao)
const res = await fetch('https://example.com/robots.txt');  // Node fetch
```

Node/Bun host setup without a page: `bao::runtime::` (the `bun_runtime`
surface).

> ⚠ **Same-name trap:** `bao::runtime::BaoRuntime` (Node/Bun host) ≠
> top-level `bao::BaoRuntime` (browser coordinator). Browser embedding uses
> the top-level name; Node/Bun host setup uses `bao::runtime::`.

## Hard constraints & prerequisites

- **JSContext is thread-local** — DOM ↔ Node interop must stay on the
  creating thread; never pass `JSObject` pointers across threads (SIGSEGV).
  Cross-thread, pass page ids / handles / serialized data.
- **Full stack always linked** — no Cargo features disable browser/CDP/
  stealth/Node; behaviour is a runtime choice (`StealthProfile`,
  `Permission`).
- **First build compiles SpiderMonkey from source** — clang, python3, make
  required; expect 20–40 min once (cached afterwards).
- **Linux media playback needs system GStreamer runtime libraries** — the
  servo media stack (`bao-servo-media-auto` on Linux) loads them at
  runtime: `apt install libgstreamer1.0-0 gstreamer1.0-plugins-base
  gstreamer1.0-plugins-bad` (or your distro's equivalent; `-dev` packages
  are a compile-time concern only).
- `BAO_<SUFFIX>` environment variables are aliased onto `BUN_<SUFFIX>`.

## Package family

`bao-core` is the facade; the family includes `bao-browser`, `bao-engine`,
`bao-stealth`, `bao-cdp`, `bao-cdp-client`, `bun-runtime` and the `bun_*`
base layer, plus maintained forks `bao-mozjs(-sys)` (+ `bao-mozjs-src-*`
source satellites), `bao-servo-*`, `bao-stylo`, `bao-ipc-channel`.

## License

MPL-2.0 (SpiderMonkey + Servo) · MIT (Bun-derived crates).
