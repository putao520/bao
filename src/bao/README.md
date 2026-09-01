# bao-core

**An embeddable programmable JS/TS system runtime for Rust applications.**

`bao-core` is the consumer-facing Bao library. It combines SpiderMonkey, Rust-native Node.js/Bun-style system APIs, Servo Web Runtime, CDP compatibility, and Stealth in one stack.

Bao is **not** trying to be another standalone Node/Bun replacement, and it is not primarily a browser automation product. Its purpose is to give a Rust application a programmable execution layer for dynamic logic, automation, workflows, plugins, and Agent-generated programs while the Rust host keeps control of resources and lifecycle.

**The browser is a runtime capability, not the product.** Use files, HTTP, crypto, SQLite, modules, and normal JS control flow without a page; enter Web/DOM only when a task actually needs it.

**[中文文档](https://github.com/putao520/bao/blob/master/src/bao/README.zh-CN.md)** · Full project overview: [github.com/putao520/bao](https://github.com/putao520/bao)

> **Package name vs import name:** the package is `bao-core`; the library
> name is `bao`. `Cargo.toml` says `bao-core`, code says `use bao::…`.

```toml
[dependencies]
bao-core = "0.1.5"
```

## Where Bao fits

- **Standalone JS/TS server or CLI:** start with Node.js or Bun.
- **Pure Chromium automation:** start with Playwright + Chromium.
- **Small embedded rules/expressions:** Rhai or another small scripting engine is lighter.
- **Compact isolated plugin ABI:** WASM may be a better fit.
- **Embeddable JavaScript engine only:** QuickJS and similar engines solve that lower-level problem directly.
- **Rust product that needs a rich programmable system layer, optional Web/DOM, or an application-native execution foundation for Agent-generated programs:** this is the problem Bao is designed to explore.

Bao intentionally starts above the raw-JS-engine layer. Real application scripting quickly needs modules, async semantics, system APIs, lifecycle, compatibility, Web capability, and host-controlled ownership — not only `eval()`.

## Current entry points

### 1. System runtime without a page

The Node/Bun-style host surface is re-exported under:

```rust
bao::runtime::*
```

Use this path when a task needs system/runtime APIs but no Web/DOM page.

> **Current alpha naming:** `bao::runtime::BaoRuntime` is the Node/Bun API
> host; top-level `bao::BaoRuntime` is the unified browser coordinator.

### 2. Add Web/DOM when the task needs it

```rust,no_run
use std::time::Duration;
use bao::{BaoConfig, BaoRuntime, BrowserError, PageConfig, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig::default())?;
    page.navigate("https://example.com")?;
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;
    let title = page.evaluate_js_web("document.title")?; // Page Realm: Web API only
    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let _ = (title, png);
    Ok(())
}
```

### 3. Trusted system script + DOM in one task

`page.evaluate_js` runs in Bao's host-controlled **Node Realm**, where a trusted host script can combine DOM with Node/Bun-style system APIs:

```js
const h1  = document.querySelector('h1')?.textContent;
const txt = require('fs').readFileSync('demo.txt', 'utf8');
const res = await fetch('https://example.com/robots.txt');
```

The site's own JavaScript stays in the normal Page Realm and does not receive `require` / filesystem capabilities.

### 4. CDP compatibility

Bao provides a built-in CDP surface plus a Playwright-style Rust client. The same client can connect in-process or over WebSocket:

```rust,no_run
use bao::{Browser, ConnectError};

fn connect() -> Result<(), ConnectError> {
    let mut browser = Browser::connect("memory://bao")?;
    let _version = browser.version()?;
    let _targets = browser.pages()?;
    Ok(())
}
```

CDP is a compatibility boundary, not a claim that Bao is Chrome. Coverage and lifecycle compatibility are still being expanded.

## Current capability stack

| Layer | What Bao provides |
|---|---|
| JS runtime | SpiderMonkey-based runtime |
| System APIs | Node/Bun-style modules, filesystem, HTTP/fetch, crypto, `bun:sqlite`, process/runtime primitives, and related Rust-native layers |
| Web runtime | Servo DOM/CSS/layout/rendering, pages, screenshots |
| Realm boundary | Page Realm for site code; host-controlled Node Realm for trusted system scripts |
| Automation | Built-in CDP + `memory://bao` in-process client transport |
| Stealth | Runtime-configured TLS/HTTP/browser-visible fingerprint controls |

## Important current limitations

Bao is still **0.x alpha**:

- Linux x86_64 is the only fully verified platform today.
- Node/Bun, Web, and CDP compatibility are substantial but incomplete.
- Realm separation is not a finished arbitrary-untrusted-code sandbox; fine-grained capability/quota/audit work is still ongoing.
- `JSContext` is thread-local. Never pass `JSObject` / GC pointers across threads; use ids, handles, owned messages, or serialized data and execute JS on the owning thread.
- The full stack is currently always linked; browser/CDP/Stealth/Node layers are selected by runtime behavior rather than Cargo product-feature removal.
- First build compiles SpiderMonkey from source and is intentionally heavier than a small scripting crate.
- Rust nightly is currently required.
- macOS has compile-surface work but no completed real-hardware validation yet.

## Package family

`bao-core` is the facade. The published family includes `bao-browser`,
`bao-engine`, `bao-stealth`, `bao-cdp`, `bao-cdp-client`, `bun-runtime`,
the `bun-*` Rust-native base layer, and maintained runtime/browser dependency
packages such as `bao-mozjs(-sys)`, `bao-mozjs-src-*`, `bao-servo-*`,
`bao-stylo`, and `bao-ipc-channel`.

## License

MPL-2.0 (SpiderMonkey + Servo) · MIT (Bun-derived crates).
