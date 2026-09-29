# Bao Public API — Stability Declaration (W22a / #17 Beta freeze)

The **declaration carrier is the code itself**: `src/bao/src/lib.rs` (the
umbrella) *is* the Stable declaration — this document states the freeze
semantics; `docs/api-surface.json` is the machine archive; the gate is
`scripts/api-surface.sh`.

## Tiers

### Stable — the umbrella top-level re-exports (41 items) + 1 deprecated alias

`BaoConfig` `BrowserConfig` `BrowserError` `BrowserRuntime` `PageConfig`
`PageHandle` `PagePool` `PageState` `Permission` `PermissionDenied`
`PermissionGuard` `ScreenshotFormat` `encode_image` `run_browser`
`AudioProfile` `BehaviorConfig` `BehaviorSimulator` `CanvasNoise` `FontConfig`
`Http2Fingerprint` `NavigatorProfile` `ScreenProfile` `StealthEngine`
`StealthHooks` `StealthProfile` `StealthTlsWireConfig` `TlsFingerprint`
`WebGLProfile` `Browser` `CdpError` `ConnectError` `Connection`
`ConnectionConfig` `Cookie` `DeviceDescriptor` `Viewport` `WaitUntilState`
`BackendKind` `CdpRouter` `CdpServer` `CdpSession`

**Commitment (Beta freeze):** removal or signature change of any item above is
a breaking change — gated by `scripts/semver-gate.sh` (major-version
requirement blocks). A NEW top-level item must be added to
`docs/api-surface.json` deliberately (`scripts/api-surface.sh --update` +
reviewed diff) — the gate rejects unarchived top-level pub items.

**Deprecated transition alias:** `BaoRuntime` (= `BrowserRuntime`,
`#[deprecated(since = "0.4.0")]`) stays available until **1.0**, then is
removed. It is single-listed in the archive; it must never gain a `#[doc(hidden)]`.

### Experimental — the 7 namespaced modules + the 7 transit crates

`browser` `engine` `runtime` `cdp` `cdp_client` `stealth` `uloop` namespaces
(`pub use <crate>::*` full pass-through) and the crates behind them — **1180
public items** measured at freeze time (bao-browser 73 / bao_engine 61 /
bun_runtime 448 / bao_cdp 43 / bao_cdp_client 434 / bao_stealth 54 /
bao_uloop 67).

**Commitment:** items may break within a minor version; the change must be
recorded in the crate changelog. The gate reports surface-count drift as
advisory (not blocking).

### Internal — `#[doc(hidden)]` pub items (25 registered)

Grep-census based (rustdoc JSON does not serialize the hidden marker). Every
entry carries a deliberate-hidden rationale at its definition site; the gate
rejects any NEW unregistered hidden item. Stable-tier items must never be
doc(hidden).

## Gate

```
scripts/api-surface.sh --check     # gate mode (CI / wave-end): red on drift
scripts/api-surface.sh --update    # rewrite the archive after a REVIEWED change
```

Interplay with `scripts/semver-gate.sh`: semver-gate detects **breaks of
existing items** (major blocks); api-surface detects **unarchived new pub
items** and **unregistered hidden items**. Both run as script gates (not in
nextest — rustdoc generation is minutes-scale).
