# Bao (包子)

**An embeddable programmable JS/TS system runtime for Rust applications.**

Bao combines SpiderMonkey, Rust-native Node.js/Bun-compatible system APIs, Servo Web Runtime, CDP compatibility, and Stealth in one library. The goal is **not** to build another Node/Bun replacement or another browser automation product. The goal is to give a Rust application a programmable execution layer for dynamic logic, automation, workflows, plugins, and Agent-generated programs — while the Rust host keeps ownership of resources, lifecycle, and product boundaries.

**The browser is a capability of the runtime, not the product.** A task can stay in normal JS/TS for files, HTTP, crypto, SQLite, modules, and application logic, then enter a real Web/DOM runtime only when the job actually needs a page.

**[中文文档](./README.zh-CN.md)** · Status: **0.x alpha** — Linux x86_64 · APIs may change · [CHANGELOG](./CHANGELOG.md)

---

## Why Bao exists

A Rust product often wants two things at the same time:

1. a stable, strongly owned native core; and
2. a fast-changing programmable layer for rules, automation, plugins, workflows, and AI-generated task logic.

A tiny embedded expression engine is often too small once scripts need files, networking, modules, crypto, databases, async I/O, or Web APIs. Starting a separate Node service works, but it moves state, lifecycle, packaging, permissions, and failures across a process boundary.

Bao explores a different shape:

```text
Rust application
│
├── stable native core
│   ├── resources / state
│   ├── scheduling
│   ├── permissions
│   └── performance-sensitive code
│
└── Bao programmable layer
    ├── JS / TS control flow
    ├── Node/Bun-style system APIs
    ├── HTTP / filesystem / crypto / SQLite / ...
    ├── Web / DOM runtime when needed
    └── CDP compatibility boundary
```

For Agent workloads, this means the model does not have to turn every deterministic step into a separate tool call. It can generate a short program, let normal language constructs (`for`, `try/catch`, `Promise.all`, modules, streams, etc.) carry local control flow, and return to the model when a real decision is needed.

Bao is **runtime infrastructure**, not an Agent framework: it does not try to own planning, memory, tool registries, or long-running workflow products.

## What Bao is — and is not

| If your main problem is... | Usually start with... | Where Bao fits |
|---|---|---|
| A standalone JavaScript/TypeScript server or CLI | Node.js / Bun | Bao is not trying to replace them as executable runtimes. |
| Pure browser automation with maximum Chromium compatibility | Playwright + Chromium | Bao is interesting when Web is one capability inside a larger Rust-hosted execution environment. |
| A small embedded rule/expression language | Rhai or another small scripting engine | Bao is intentionally much heavier because it aims to provide a full system runtime, not just expressions. |
| A compact isolated plugin ABI | WASM may be a better fit | Bao focuses on a familiar JS/TS programming environment with rich system/Web capabilities; a complete untrusted-code sandbox is not finished. |
| A Rust product that needs rich dynamic scripting, automation, or Agent execution close to application state | **Bao** | This is the problem Bao is designed to explore. |

QuickJS and other embeddable JS engines solve an important lower-level problem: embedding JavaScript. Bao starts one layer higher — the difficult part is not only evaluating JS, but providing the system APIs, async runtime semantics, Web runtime, lifecycle, compatibility, and host-controlled execution model that real application scripts quickly need.

## The library advantage

Bao is intended to be **linked into the Rust product**, rather than treated as a separate service that the product has to orchestrate.

That distinction matters when you want the host to keep control of:

- task and runtime lifecycle;
- permissions and resource ownership;
- application state and native objects;
- threads and scheduling;
- files, sockets, database connections, and page handles;
- cancellation, shutdown, and error propagation.

The long-term direction is to make application capabilities programmable without forcing every low-level operation to become a remote tool or RPC. Node/Bun compatibility, Servo, CDP, and Stealth are building blocks toward that goal, not the product definition by themselves.

## Using the library

**Package name vs import name:** the crate is published as **`bao-core`**, while the library name is **`bao`**. Your `Cargo.toml` says `bao-core`; your Rust code says `use bao::…`.

```toml
[dependencies]
bao-core = "0.1.5"
```

```rust
use bao::{BaoConfig, BaoRuntime};
```

### Entry 1 — Node/Bun-style system runtime without a page

If you need the system-runtime surface without creating a browser page, Bao re-exports the `bun_runtime` host under:

```rust
bao::runtime::*
```

This is the path for Node/Bun-style modules and system APIs when Web/DOM is not part of the task.

> **Two `BaoRuntime` types currently exist:** `bao::runtime::BaoRuntime` is the Node/Bun API host, while top-level `bao::BaoRuntime` is the unified browser coordinator. This naming is an alpha-era API constraint and may evolve.

### Entry 2 — add Web/DOM when the task needs it

Top-level `BaoRuntime` gives the Rust host access to Servo pages. The important point is that Web capability stays inside the same Bao stack rather than requiring a Node → Playwright → Chromium sidecar chain.

```rust,no_run
use std::time::Duration;
use bao::{BaoConfig, BaoRuntime, BrowserError, PageConfig, PageState, ScreenshotFormat};

fn main() -> Result<(), BrowserError> {
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    let page = runtime.create_page(&PageConfig::default())?;

    page.navigate("https://example.com")?;
    // Current Servo lifecycle constraint: wait for the pipeline before evaluating.
    page.wait_for_pipeline_ready(Duration::from_secs(30))?;

    // Page Realm: Web API only — no require/fs.
    let title = page.evaluate_js_web("document.title")?;

    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let state: PageState = page.get_state();
    let _ = (title, png, state);
    Ok(())
}
```

### Entry 3 — trusted system script + DOM in one task

`page.evaluate_js` runs in Bao's **Node Realm**. A host-triggered trusted script can use DOM together with Node/Bun-style system APIs:

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

The page's own JavaScript does **not** get those system capabilities. Bao separates the normal Page Realm from the host-controlled Node Realm; putting `fs` on arbitrary page `window` objects would defeat the security boundary.

### Entry 4 — CDP automation compatibility

Bao also exposes a CDP server and a Playwright-style Rust client. The same client abstraction can connect in-process through `memory://bao` or over WebSocket.

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

**Pump contract:** Servo-domain CDP commands (`Runtime.evaluate`, `Page.navigate`, …) execute on the runtime thread and currently need the host to drive that thread with `runtime.pump_cdp(Duration)` or the `run()` loop. Protocol-only commands such as `Browser.version` and `pages()` do not. An unpumped Servo-domain command times out instead of returning fake success.

CDP support is a compatibility boundary, not a claim that Bao is Chrome. Method coverage, event ordering, object lifecycle, and Playwright assumptions are still being tested and expanded.

## Current capability stack

| Layer | What Bao provides |
|---|---|
| Programmable language | SpiderMonkey-based JavaScript runtime; TS/tooling support is part of the broader runtime direction |
| System runtime | Node/Bun-style modules and APIs: `require`, filesystem, HTTP/fetch, crypto, `bun:sqlite`, process/runtime primitives, and related Rust-native building blocks |
| Web runtime | Servo DOM/CSS/layout/rendering, multi-page `PagePool` / `PageHandle`, screenshots |
| Realm boundary | Page Realm for site code; host-controlled Node Realm for trusted system scripts |
| Automation compatibility | Built-in CDP surface + Playwright-style Rust client, including `memory://bao` in-process transport |
| Stealth | TLS/HTTP and browser-visible fingerprint controls through runtime `StealthProfile` configuration |

## Important current limitations

Bao is still **0.x alpha**. In particular:

- Linux x86_64 is the only fully verified platform today.
- Node/Bun, Web, and CDP compatibility are substantial but not complete; handler/API existence is not treated as proof of semantic compatibility.
- Realm separation is **not** a finished arbitrary-untrusted-code sandbox. Fine-grained capability, quota, audit, and stronger isolation work is still ongoing.
- `JSContext` is thread-local. Never pass `JSObject` / GC pointers across threads; cross-thread paths must use ids, handles, owned messages, or serialized data and execute JS back on the owning thread.
- The full stack is currently always linked; there are no Cargo product features that remove browser/CDP/Stealth/Node layers. Runtime configuration selects behavior.
- The first build compiles SpiderMonkey from source and is intentionally heavier than a small scripting crate.
- Rust nightly is currently required; the repository pins the supported toolchain.
- macOS has compile-surface work but has not yet completed real-hardware build/link/test validation.

These trade-offs are deliberate for the current phase: Bao is first trying to make one integrated runtime behaviorally reliable before turning every subsystem into an optional matrix.

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
| `bao-browser` | Servo embedding: `BaoRuntime`, `PagePool`, `PageHandle` |
| `bao-cdp` / `bao-cdp-client` | CDP server surface / Playwright-style Rust client |
| `bao-stealth` | Stealth engine + `StealthProfile` |
| `bao-mozjs`, `bao-mozjs-sys`, `bao-mozjs-src-*`, `bao-servo-*`, `bao-stylo`, `bao-ipc-channel` | Maintained runtime/browser dependency family |

## Project direction

Bao is being developed around a simple question:

> **Can a Rust application expose a rich, familiar, programmable execution environment without giving up native ownership — and can that same layer become useful for Agent-generated programs as well as human-written scripts?**

That is why the project cares as much about lifecycle, error semantics, event-loop fairness, Realm boundaries, cancellation, compatibility, GC ownership, and host safety as it does about adding API names.

## License

MPL-2.0 (SpiderMonkey + Servo) · MIT (Bun-derived crates). See `LICENSE-MPL-2.0`, `LICENSE-MIT`, and `THIRD_PARTY_LICENSES.md`.

---

*Developing the repository itself: clone [putao520/bao](https://github.com/putao520/bao). The `examples/` directory contains runnable integration examples.*
