# Bao (包子)

**A high-performance anti-fingerprint browser runtime in a single Rust stack** — SpiderMonkey (JS engine) + Servo (full browser engine) + always-on Node.js/Bun API compatibility + built-in Stealth, in one Rust runtime. No Chromium, no Node child process, no Playwright-to-Chrome bridge: the browser engine *is* the runtime.

> Status: **0.x alpha** — Linux x86_64 · APIs may change. See [CHANGELOG](./CHANGELOG.md).

---

## Using the library (`bao-core`)

**Package name vs import name (read this first):** the crate is published as
**`bao-core`**, but the library name is pinned to **`bao`** — your
`Cargo.toml` says `bao-core`, your code says `use bao::…`:

```toml
[dependencies]
bao-core = "0.0.1"
```

```rust
use bao::{BaoConfig, BaoRuntime};   // ← `bao`, not `bao_core`
```

### Integration path 1 — embed the browser (primary)

`BaoRuntime` (top-level) → `create_page` → `navigate` → JS → screenshot.
From [`examples/01-browser/main.rs`](./examples/01-browser/main.rs):

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

    // Page Realm: Web API only (no require/fs).
    let title = page.evaluate_js_web("document.title")?;

    let png = page.take_screenshot(ScreenshotFormat::Png)?;
    let state: PageState = page.get_state();
    let _ = (title, png, state);
    Ok(())
}
```

### Integration path 2 — CDP automation (Playwright-style)

The `Browser` client connects in-process (zero-copy, no port) or over a
WebSocket served by the built-in CDP server (`bao browser --cdp-port 9222`).
From [`examples/02-playwright/main.rs`](./examples/02-playwright/main.rs) and
the `lib.rs` doctest:

```rust,no_run
use bao::{Browser, ConnectError};

fn main() -> Result<(), ConnectError> {
    // In-process transport — or "ws://127.0.0.1:9222" for an external Bao.
    let mut browser = Browser::connect("memory://bao")?;
    let version = browser.version()?;          // CDP Browser.version
    let targets = browser.pages()?;            // equivalent of GET /json/list
    let _ = (version, targets);
    Ok(())
}
```

### Integration path 3 — Node/Bun APIs in the page (dual realm)

`evaluate_js` runs in the **Node Realm**, where the same global scope has the
DOM *and* `require` / `fs` / `fetch` / `Bun` / `process` (an alias `Bao`
points at the same `Bun` object). From
[`examples/03-node-dom/main.rs`](./examples/03-node-dom/main.rs) and
[`examples/04-crawler/main.rs`](./examples/04-crawler/main.rs):

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

For Node/Bun host setup without a page, the `bun_runtime` crate's surface is
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
| CDP | Built-in server (`ws://…` for Playwright/Puppeteer) + Playwright-style Rust client (`Browser::connect("memory://bao" \| ws URL)`) |
| Stealth | TLS JA3/JA4, HTTP/2, Canvas/WebGL/Audio/Navigator/behavior fingerprints; runtime `StealthProfile` |

## crates.io package family

`bao-core` is the only package you normally depend on. The rest of the family
is published for internal layering and direct use if you need a slice:

- `bao-core` — unified facade (this README's usage)
- `bao-browser`, `bao-engine`, `bao-stealth`, `bao-cdp`, `bao-cdp-client`,
  `bun-runtime` and the `bun_*` layer (base64/zlib/http/dns/…)
- Forks: `bao-mozjs` / `bao-mozjs-sys` (+ `bao-mozjs-src-*` source
  satellites), `bao-servo-*` (Servo components), `bao-stylo`,
  `bao-ipc-channel`

The `bao` CLI binary is **not** published to crates.io; build it from this
repository (`cargo build -p bao_bin`).

## License

MPL-2.0 (SpiderMonkey + Servo) · MIT (Bun-derived crates). See
`LICENSE-MPL-2.0` / `LICENSE-MIT` and `THIRD_PARTY_LICENSES.md`.
