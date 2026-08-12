# Changelog

All notable changes to Bao are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(with pre-release tags while the public API is unstable).

## [Unreleased]

### Pending
- Compatibility test suite (Web Platform / Node / Bun / CDP) — public pass rates.
- Benchmark harness — public, reproducible baselines.
- Playwright CDP integration smoke test in CI.

## [0.1.0-alpha.1] — 2026-08-12

Bao's first tagged release. **Alpha**: Linux x86_64 is the primary path; the
public API may change between alpha releases; production use is not advised.

### Added — Runtime
- Unified public library package `bao` (single entry point; the full stack —
  engine + browser + runtime + CDP + stealth — is always linked).
- `BaoRuntime` — manages a Servo instance + `PagePool` (multi-page, per-page
  idle recycling).
- `PageHandle` high-level API: `navigate` / `evaluate_js` / `evaluate_js_web` /
  `take_screenshot` / state machine (`Created → Navigating → Interactive → Idle → Closed`).
- Dual-realm security model: **Node Realm** (Node.js + Bun APIs + DOM) vs
  **Page Realm** (Web APIs + DOM only). DOM and Node APIs coexist in one
  programmable runtime yet remain isolated.
- JSContext model: one global `JSEngine` + one thread-local `JSContext` per
  `ScriptThread` (mirrors servo upstream). Cross-thread `JSObject` raw-pointer
  passing is forbidden.

### Added — Node.js / Bun API compatibility
- `require` / `fs` / `path` / `crypto` / `http` / `process` / `stream` /
  `bun:sqlite` / `bun:ffi` / `vm` / `url` / `os` / `zlib` / `dns` / `net` /
  `tls` / `events` / `util` / `querystring` — always-on, same JSContext as Web APIs.
- Stealth HTTP client (`stealth_http`) for fingerprint-aware requests.

### Added — Browser engine
- Servo integration: DOM + CSS + Layout + WebRender, real rendering engine
  (not a headless mock).
- Web Workers via servo's native `Worker::Constructor` (DedicatedWorkerGlobalScope).
- Per-instance IPC routing and idempotent multi-`BaoRuntime` support.

### Added — CDP & automation
- Built-in CDP Server: 12 domains (Page, Runtime, DOM, Network, Debugger,
  Input, Emulation, CSS, Overlay, Log, Fetch, Target).
- Playwright/Puppeteer connectivity over `ws://127.0.0.1:9222`.
- `bao_cdp_client`: Rust high-level `Browser` API (`memory://bao` in-process /
  `ws://` external), Playwright-style.

### Added — Browser identity & privacy
- Configurable identity profiles: TLS (JA3/JA4), HTTP/2 (AKAMAI), Navigator,
  Screen, WebGL, Canvas privacy, Audio privacy, input behavior simulation.
- `StealthProfile::firefox_default()` / `chrome_default()`, `Clone` + custom profile support.

### Added — CLI
- `bao run [-e <CODE> | <FILE>] [--module]`
- `bao build <ENTRYPOINT>` (bundler)
- `bao test [FILES...]`
- `bao install [ARGS...]`
- `bao browser [--url URL] [--cdp-port PORT] [--headless|--no-headless] [--stealth]`

### Added — Project / OSS
- License files: `LICENSE` (dual MIT/MPL-2.0), `LICENSE-MIT`, `LICENSE-MPL-2.0`.
- `NOTICE`, `THIRD_PARTY_LICENSES.md` (MPL-2.0 §3.3(b) modified-file disclosure).
- `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `SUPPORT.md`.
- `docs/architecture.md`, `docs/OSS-PRODUCTIZATION-PLAN.md`.
- BCE (Bug-Class Eradication) regression gate in CI.

### Known limitations (alpha)
- **Platform**: Linux x86_64 only (macOS/Windows event-loop not yet proven).
- **API stability**: public API may change; not SemVer-stable until `v1.0`.
- **First build is slow**: mozjs compiles SpiderMonkey from source.
- **Compatibility**: Web Platform / Node / Bun / CDP coverage is partial —
  see `compat/` (in progress) for measured pass rates. Not a drop-in
  Chrome/Node replacement.
- `Bun.serve` + `fetch(self)` end-to-end has residual issues in the HTTP
  write path (see `src/BUG-KNOWLEDGE.md` BCE-20260618-007-R2).

[Unreleased]: https://github.com/putao520/bao/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/putao520/bao/releases/tag/v0.1.0-alpha.1
