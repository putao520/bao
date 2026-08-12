# Bao Roadmap

> **Living roadmap.** This document tracks high-level milestones. Detailed
> progress is managed via GitHub Milestones & Issues. Items move from `v0.x`
> to `v1.0` as the surface stabilizes.
>
> Status labels follow the same honesty-first rule as the Compatibility Matrix:
> `[x]` = shipped and tested; `[ ]` = not yet; `~` = partial / in-progress.

## v0.1 — Runtime foundation (current: alpha)

Core runtime stack in place. APIs exist but not yet stable.

- [x] SpiderMonkey engine integration (mozjs, per-thread JSContext following servo upstream)
- [x] Servo browser integration (DOM / CSS / Layout / WebRender)
- [x] Node.js / Bun API surface (`fs` / `path` / `crypto` / `http` / `process` / `bun:sqlite` / `bun:ffi` / `fetch` / `vm` / `timers` / ...)
- [x] CDP Server (12 domains: Page / Runtime / DOM / Network / Debugger / Input / Emulation / CSS / Overlay / Log / Fetch / Target)
- [x] Multi-page runtime (`PagePool` + `PageHandle`)
- [x] Dual-realm isolation (Node Realm + Page Realm)
- [x] Configurable browser identity & privacy (TLS JA3/JA4 / HTTP/2 AKAMAI / Canvas / WebGL / Audio / Navigator / Behavior)
- [x] CLI binary (`bao run` / `bao browser` / `bao build` / `bao test` / `bao install`)
- [x] Unified public library surface (`bao` package — full stack always linked, no Cargo feature split)
- [x] EBUSY patch on mozjs (default multi-threaded `cargo test` no SIGSEGV)
- [ ] Linux x86_64 production-quality (currently alpha — APIs may change)
- [ ] API stability pass (public `bao` lib API audit, SemVer contract)
- [ ] `bao compat` aggregation command (compat pass-rate reporting)
- [ ] `bench` harness + first public REPORT.md

## v0.2 — CDP compatibility

Make Bao a reliable CDP target for Playwright / Puppeteer.

- [ ] Playwright full connect→navigate→evaluate→screenshot→close lifecycle (currently Experimental)
- [ ] Puppeteer lifecycle parity with Playwright
- [ ] CDP Debugger domain completion (currently Partial)
- [ ] Network domain headers / cookies completeness
- [ ] CDP method coverage matrix published (`compat/cdp/`)
- [ ] `bao_cdp_client::Browser` hardened for production Rust embedding

## v0.3 — Web compatibility

Publish Web Platform pass rates against servo WPT.

- [ ] WPT subset (DOM / HTML / CSS / Fetch) public pass rates on Bao
- [ ] Track divergence between Bao integration and servo upstream WPT
- [ ] WebSocket completion (currently Partial)
- [ ] Navigation lifecycle state machine (Created / Navigating / Interactive / Idle / Closed) hardened
- [ ] Full-page render regression suite (pixel-level, not just DOM)

## v0.4 — Node / Bun compatibility

Publish Node / Bun API pass rates.

- [ ] `node:crypto` full coverage measurement
- [ ] `node:http` / `node:https` stability
- [ ] `bun:sqlite` / `bun:ffi` conformance suite
- [ ] Bun API coverage measurement (`Bun.file` / `Bun.write` / `Bun.serve` / `Bun.spawn` / ...)
- [ ] Aggregate `node_conformance/` into public pass-rate report

## v0.5 — Multi-page stability

Production-grade multi-page operation.

- [ ] Concurrent page lifecycle stress (100+ pages, churn)
- [ ] Memory leak elimination across page churn (BCE-004 residual = 0)
- [ ] Cross-thread `JSObject` pointer discipline enforced (no SIGSEGV under PagePool chaos)
- [ ] Browser identity & privacy profile hardening (per-page StealthProfile independence)
- [ ] Idle page reclamation tuning (`idle_ttl`, RSS ceiling)

## v0.6 — Performance baseline

First public, reproducible performance report.

- [ ] `bench/` harness implemented (runtime / browser / automation / node_api dimensions)
- [ ] Comparison baselines against Bun / Node / Chromium + Playwright
- [ ] Performance regression CI (`bench.yml`, manual trigger)
- [ ] `bench/REPORT.md` published (version-bound)

## v1.0 — Stable embedding API

Lock the surface, harden for production.

- [ ] Locked public API (`bao` lib, SemVer)
- [ ] Cross-platform: macOS / Windows (event loop not yet proven)
- [ ] Production hardening (fuzzing, long-running soak tests)
- [ ] Documentation complete (API reference + embedding guide)
- [ ] Public compat matrix + benchmark report covering v0.1–v0.6 milestones
- [ ] Release process defined (CHANGELOG, semver bumps, artifact publishing)

---

## How to read this roadmap

- **Status reflects README.md Compatibility Matrix + CLAUDE.md test status**, not aspirational claims
- Items marked `[x]` are landed in `master` with test coverage
- Items marked `[ ]` are planned, not started or in-progress (see GitHub Issues for detail)
- **No estimated dates** — Bao is maintained transparently; milestones ship when the corresponding compatibility / stability bars are met, not on a fixed calendar
- **Scope discipline** — `v1.0` is reached only when `compat/` and `bench/` show real (not aspirational) numbers backing each `v0.x` milestone

## Out of scope (explicit non-goals)

- **Replacing Chromium** — Bao is a Rust-native programmable browser runtime, not a Chrome replacement
- **Mobile platforms** — no iOS / Android target planned
- **Embedded devices** — SpiderMonkey + servo footprint is too large for microcontrollers
- **Backwards compatibility for pre-v1.0 APIs** — until `v1.0` ships, public API may break between minor versions
