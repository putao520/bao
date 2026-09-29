# Error Model Contract (docs/error-model.md)

W22d (#17-D) — observational ledger. Zero product-semantics changes were made
by this audit; findings are REPORTED, not fixed.

Locked face: `src/bao/tests/golden/errors.json` +
`src/bao/tests/error_model_tests.rs` (golden variant/Display lock + taxonomy +
source-chain state).

## 1. The 7 pub error types (measured)

| Type | Crate | Shape | Variants | Notes |
|---|---|---|---|---|
| `BrowserError` | bao_browser | enum | 5 (Init/Navigation/Rendering/JavaScript/CDP) | all-String payloads; Display prefixes locked |
| `ConnectError` | bao_cdp_client | enum | 6 (5 measured w22d + `Io` added W29) | `Io(io::Error)` carries the typed source |
| `CdpError` | bao_cdp_client | enum | 8 (7 measured w22d + `Connect` added W29) | IoError/JsonError/Connect carry typed sources |
| `ErrorCode` | bun_sm | C-style `repr(u8)` | 11 discriminants (NoError=0 … GenericError=10) | JS-kind classifier, NOT a std Error by design |
| `CryptoError` | bao_crypto | thiserror enum | 23 (measured; mixed struct/String payloads) | Display via `#[error]` |
| `TranspileError` | bun_transpiler | struct | `{message}` | prefix `transpile_ts error: ` |
| `JsError` | bun_sm | struct | `{message, filename, line, column, stack?}` | **runtime main-entry typed error**; `bun_core::JsError` is the internal 1-byte tag enum (Thrown/OutOfMemory/Terminated), a different face |

## 2. Source-chain findings — ALL FIXED (W29)

The four chain-loss findings from the w22d audit are fixed at the impl layer
(variant set grew ONLY where the From conversion needed a typed carrier —
that growth IS the fix, locked in the golden). The source-chain test now
asserts Some(source) + iterable root cause; a regression flips it loudly.

| # | Finding (w22d state) | Fix (W29) | Where |
|---|---|---|---|
| w22d-1 | `From<io::Error> for ConnectError` stringified into `ConnectionFailed(String)` | **`ConnectError::Io(std::io::Error)` variant added**; From keeps the typed source; `source()` returns it | bao_cdp_client/src/error.rs |
| w22d-2 | `CdpError` hand-written empty Error impl — `IoError(io::Error)` had `source() == None` | **`source()` match arm added**: IoError/JsonError/Connect → Some | bao_cdp_client/src/error.rs |
| w22d-3 | `From<serde_json::Error>` stringified into `JsonError(String)` | **`JsonError(serde_json::Error)` payload retyped** (zero external construction sites — census-verified); From keeps the typed source | bao_cdp_client/src/error.rs |
| w22d-4 | `From<ConnectError>` flattened to `ProtocolError(String)` — phases indistinguishable | **`CdpError::Connect(ConnectError)` variant added**; nested typed carrier, chain walks Connect → ConnectError → io root cause | bao_cdp_client/src/error.rs |

Still open (NOT error-chain work): the BridgeChannel `String` backlog below.

## 3. String-guess surface ledger (33 pub fns, 12 files)

Classification: **JS-semantics** (the error crosses into a JS `catch`; JS
Errors have no Rust taxonomy — keep-as-fact) / **infra** (pure Rust-to-Rust
boundary; typed error warranted — backlog) / **config/tooling** (human-facing
validation at startup or build time — keep-as-fact).

### bao_runtime — JS-semantics (16)

| Fn | Classification | Rationale |
|---|---|---|
| `permission_bridge::{check_fs_read,check_fs_write,check_net,check_env,check_run}` (5) | JS-semantics, keep | stable `"Permission denied"` denial thrown into JS `catch`; the STRING is the contract (pinned by security_sandbox) |
| `bun_ffi::{dlopen×2,close×2,symbol×2}` (6) | JS-semantics, keep | FFI errors surface as JS exceptions; OS error text is the payload. Backlog-negligible: typed io source would add nothing at the JS boundary |
| `bun_sqlite::{new,exec,run,close,serialize_bytes,backup_to_path}` (6) | JS-semantics + infra-lite, keep | rusqlite errors stringified into JS exceptions; typed source would be lost but no Rust consumer needs it today (backlog-negligible) |

### bao_browser (7)

| Fn | Classification | Rationale |
|---|---|---|
| `BaoConfig::validate` (1) | config/tooling, keep | startup human-facing config validation; message IS the product |
| `BridgeChannel::{ok,recv,send,fire_and_forget}` ×2 arms (6) | **infra, backlog** | pure Rust↔Rust CDP pipe: timeouts/disconnects as `String` cross the pump loop; typed `CdpError` (Timeout/ConnectionClosed) would let the CDP layer map them to `-32000`/closed without string matching. Lowest-cost real归一 candidate |

### misc (10)

| Fn | Classification | Rationale |
|---|---|---|
| `uws::ws_server::read_message` (1) | infra-internal, keep | framing sentinel (`Err(())`) on an internal server loop |
| `bao_bundler::build` (1) | tooling, keep | build-time human message |
| `css::values::css_string::parse` (1) | infra-internal, keep | parser-level `Result` consumed by the CSS parser's own recovery |
| `cdp_server::{server::run,registry::register}` (2) | infra, backlog-low | server lifecycle/registration — single consumer each, typed error low value today |
| `bun_sm::codegen::{parse_classes,validate_class_name,validate_member_name}` (3) | tooling, keep | build-time codegen validation (compile-fail face), not runtime |
| `bao_cdp_client::api::page::title` (1) | n/a | false positive of the grep — returns `crate::error::Result<String>` (CdpError), already typed |

### Summary

- **33 hits → 32 real String-error faces** (1 grep false positive, already typed).
- keep-as-fact: 30 (28 JS-semantics/config/tooling + 2 infra-internal single-consumer).
- **backlog (typed-error candidates): 6**, all in `BridgeChannel` (bao_browser/runtime_bridge) — the one place a Rust consumer branches on the error text today.
- runtime main entry (`NodeRuntime::eval`) is **typed** (`Result<_, JsError>` struct with location+stack) — the design's "预期残留面很小" holds: no host-capability path returns bare `String` errors.

## 4. Kind-discrimination ratio (taxonomy 2)

Enum-variant discriminators dominate: 5/7 types are enums with kind-bearing
variant names; the 2 structs (TranspileError, JsError) are single-kind
message carriers where the kind is the type itself. No type uses
`Error(String)` as a generic catch-all that consumers must string-parse —
the one historical risk area (permission denials) is a deliberate,
test-pinned string contract at the JS boundary (see §3).
