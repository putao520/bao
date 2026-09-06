# Bun Evolution — Bao Library Architecture Transposition Plan

> Status: ACTIVE
> Parent: #31
> First execution issue: #32
> SSOT: this file

## 0. Purpose

Bao does not use JSC and is not a Bun-style standalone executable runtime. Bao is an embeddable Rust library. Bun is therefore not only a code upstream; it is a runtime-engineering knowledge upstream whose design must be transposed into Bao's same-process, multi-threaded, explicit-ownership architecture.

Core rule:

`Bun executable/process assumptions -> Bao runtime/thread/task/channel ownership`

Public APIs whose semantics require OS processes (`Bun.spawn*`, Node `child_process`) remain real process APIs.

## 1. Upstream adoption taxonomy

Every relevant Bun upstream change must be classified as one or more of:

- DIRECT-ABSORB
- SEMANTIC-PORT
- ARCHITECTURE-ADOPT
- THREAD-TRANSPOSE
- CHANNEL-TRANSPOSE
- BCE-TRANSPOSE
- SIMPLIFICATION
- ALREADY
- N/A

A JSC/C++/process-oriented file is not automatically N/A.

## 2. Library Architecture Transposition rules

| Bun assumption | Bao target |
|---|---|
| internal helper process | worker thread / task / runtime component |
| internal IPC | typed in-process channel / owned message |
| process-local mutable state | BaoRuntime-local / ScriptThread-local / TLS |
| process lifecycle | explicit start/cancel/drain/join/drop state machine |
| process VM isolation | thread-local JSContext + Realm/Compartment/Zone |
| process-global cache | immutable/shared or runtime-owned cache with explicit owner |
| process exit | return error/terminal state to host, never kill host |
| process crash containment | KEEP-OS-ISOLATION only after explicit adjudication |

## 3. Hard invariants

1. Bao internal architecture must not spawn hidden helper/daemon/worker processes merely because Bun does.
2. JSObject / GC cell / raw SpiderMonkey pointers never cross threads or channels.
3. Threading conversion is not `process -> std::thread::spawn`; ownership, cancellation, backpressure, shutdown and error propagation must be redesigned.
4. Shared mutable global state is not an acceptable substitute for process isolation without explicit ownership proof.
5. Public `Bun.spawn*` / Node `child_process` remain true OS-process semantics.
6. Library failures return control to host; internal Bao paths must not terminate the embedding process.

## 4. Phase plan

### B0 — Process/IPC architecture census [ACTIVE]
Issue: #32

Inventory Bao and mirrored Bun architecture for:
- fork/spawn/exec used for internal orchestration
- helper/daemon/service processes
- worker processes
- IPC/pipe/socketpair/process message transport
- PID/signal/process-exit lifecycle assumptions
- process-local mutable globals/caches/state
- crash-isolation assumptions

Classification per item:
- PUBLIC-PROCESS-SEMANTICS
- KEEP-OS-ISOLATION
- THREAD-TRANSPOSE
- TASK-TRANSPOSE
- CHANNEL-TRANSPOSE
- RUNTIME-LOCALIZE
- TLS-LOCALIZE
- ALREADY-TRANSPOSED
- N/A

**This phase may not end with inventory only.** After census, choose one highest-risk unblocked item and implement a complete transposition slice.

### B1 — Runtime/thread ownership
Convert process-local assumptions into BaoRuntime/ScriptThread/worker ownership. Align with #15 Runtime and #23 SM Realm topology.

### B2 — IPC to channels
Replace internal process transport with typed owned in-process messages where applicable. Define close/backpressure/cancel/timeout behavior.

### B3 — Lifecycle and failure semantics
Replace PID/exit/signal-driven internal lifecycle with BaoRuntime/worker state machines. Verify host process survives runtime errors/close.

### B4 — Reliability transposition
For Bun bugfixes involving process, event-loop, GC, keepalive, teardown, race, UAF or resource ownership, derive the abstract bug class and run BCE across Bao.

### B5 — Simplification
Use newer Bun design and SM/Servo native primitives to remove Bao glue/workarounds. Track removed LOC/duplicate schedulers/caches/bridges.

### B6 — Permanent upstream loop
Every daily Bun wave runs the expanded taxonomy and updates this ledger. No regression back to ABSORB/ALREADY/N/A-only triage.

## 5. JSC/Bun -> SM/Bao semantic mapping

| Bun/JSC concept | Bao mapping |
|---|---|
| JSC VM | process-wide JSEngine + thread-local JSContext ownership |
| JSGlobalObject | SM Realm/global + Bao runtime/page identity |
| JSC microtasks | SM JobQueue + Bao scheduler (#25) |
| JSC termination | SM Interrupt + Bao ExecutionControl (#24) |
| bytecode cache | SM Stencil/XDR (#26) |
| process-local isolation | BaoRuntime/TLS + SM Realm/Zone |
| internal IPC | typed in-process channels |
| process shutdown | cancel -> drain -> join -> release -> return to host |

## 6. First scheduled-agent execution contract

On the next daily-ops run that consumes #31/#32:

1. Read #31, #32, this file, CLAUDE.md and `.claude/upstream-baseline.json`.
2. Rebase facts against current Bao master and current Bun baseline.
3. Build the B0 census with concrete code paths and Bun source references.
4. Rank findings by:
   `correctness/lifecycle > resource ownership > host-process safety > simplification > performance`.
5. Pick exactly one highest-priority unblocked slice.
6. Write the slice design into this file before code.
7. Modify real code in the same run.
8. Add positive + negative + lifecycle tests.
9. Run scoped nextest according to repository test discipline.
10. If the slice touches crash/race/leak/UAF/starvation/teardown, run BCE and record residual=0 or exact blocker.
11. Commit the implementation.
12. Update this file with changed paths, tests, evidence, commit and the single next action.

Forbidden end states when an executable slice exists:
- analysis complete
- plan complete
- inventory complete
- recommendation only

## 7. Initial code areas to inspect

Do not assume these are bugs; they are audit anchors:

- `src/bao_runtime/src/bun_spawn_sync.rs` — distinguish public process semantics from internal orchestration.
- `src/spawn/` — public/process primitives and any internal reuse.
- runtime/event-loop/worker code — process-local assumptions that may have survived Bun transposition.
- IPC/message/router abstractions — determine whether any are still process-shaped when used purely in-process.
- global statics/singletons inherited from Bun executable assumptions.
- shutdown/error paths that call or model process exit rather than runtime termination.

## 8. Evidence ledger

### Current baseline
- Bao baseline at plan creation: commit after `57e2fe4435d3a126ca1cf9a0005d24e223af5700`.
- Bun upstream baseline must be read from `.claude/upstream-baseline.json` at execution time; do not freeze the value here.

### Completed slices

- 2026-09-02 (daily-ops live): taxonomy applied to the standing 2026-08-30 triage backlog — the 39 items judged "absorb" are re-labeled DIRECT-ABSORB (no re-triage). Batch 1 landed in commit `c0a09301`: bd630c1d7e (errno: out-of-table kernel errno → EUNKNOWN, in-crate transmute count now 0), 79936e42ab (JSON string formatters escape lone surrogates / malformed UTF-8), d578a8c70d (json5 escape errors point at the offending character), 77d916c56e (scoped debug log single write(2)), 1beee7ae72 (CSS tokens printed as CSS in parse errors). Verification: scoped nextest over the 6 touched crates — 127 run / 127 passed / 1 skipped / 0 failed. Backlog 58 → 53; 85 further bun commits in-window remain untriaged (nine-way taxonomy mandatory for them). Adjacent pre-existing divergence registered, out of batch scope: `src/highway` `index_of_needs_escape_for_javascript_string` fast path returns the first `\` before an earlier quote char (upstream SIMD returns first-overall; reachable via `quote_for_json` today).

### B0 — Process/IPC architecture census (2026-09-05, issue #32)

Full census table (50 rows + 8-scope-row coverage statement): `.plans/b0-census-2026-09-05.md` (companion file, this entry is the summary + pointer).

Coverage result per §4 B0 scope: internal spawn/fork/exec helpers = **zero internal-orchestration hits** (all spawns are public API semantics or CLI entries); internal IPC = **zero process-shaped transport** (typed mpsc channels already; the one socketpair `IpcChannel` exists only as public child_process/cluster IPC support); daemon/background helper processes = **zero hit**; internal worker processes/compile/installer/runtime helpers = **zero hit** (workers are threads; lifecycle scripts are npm public contract; TestParallelWorker/MultiRun/FilterRun/Cron/Chrome exit-kinds registered but runner-less, dormant); process-global mutable state = 15 RUNTIME-LOCALIZE rows; PID/exit/signal lifecycle = orderly-exit already TLS+drop-chain, signal-forwarding quartet is windowed public spawnSync semantics, `Global::exit`/ParentDeathWatchdog machinery dormant from all live bao-layer paths; crash isolation = zero internal OS-process isolation (KEEP-OS-ISOLATION adjudication: none required); shutdown/error paths = bao layer returns control to host (only `bao_bin` main exits the process).

Label distribution (closed nine-class set, unknown=0): PUBLIC-PROCESS-SEMANTICS 12 · KEEP-OS-ISOLATION 0 · THREAD-TRANSPOSE 0 remaining · TASK-TRANSPOSE 1 · CHANNEL-TRANSPOSE 0 remaining · RUNTIME-LOCALIZE 15 · TLS-LOCALIZE 0 remaining · ALREADY-TRANSPOSED 12 · N/A 10.

Top-3 transposition slice candidates (ranked correctness/lifecycle > resource ownership > host-process safety > simplification > performance):
1. **UNBLOCKED** — `src/bao_runtime/src/runtime.rs:43-55` `init_env_aliases()` mutates host process env via `std::env::set_var` inside the library constructor (`BaoRuntime::new`). RUNTIME-LOCALIZE. UB-adjacent under threads, irreversible, cross-runtime interference, hottest library entry point.
2. **UNBLOCKED** — `src/bao_stealth/src/http2.rs:186` `GLOBAL_HTTP2_FINGERPRINT` process-global: per-runtime stealth profiles fight over one H2 fingerprint while TLS/JS fingerprints are per-realm → detectable inconsistency. RUNTIME-LOCALIZE; per-realm keying pattern already exists (`engine_props.rs:333`).
3. **UNBLOCKED (largest)** — detached-thread cluster (row 43 of census: CDP server thread `bao_browser/src/lib.rs:574` never joined; WS-connect/fs/crypto/build per-op threads; per-child `pipe_poll_thread`): mechanism already threads, remaining gap is cancel/drain/join ownership on `BaoRuntime::drop`. TASK-TRANSPOSE; v1 = CDP server thread stop+join.

Blocked rows (documented): servo per-runtime Opts (servo upstream process-global `OnceLock<Opts>`); `NODE_REALM_BY_WEBVIEW`/`PAGE_GLOBAL_BY_WEBVIEW` multi-instance keying (#23 Realm topology); console timers/counter scoping (product-semantics ruling needed).

Permanent invariants carried into B1-B3: no bao-layer path may reach `bun_core::Global::exit`; `ParentDeathWatchdog` and crash auto-reload stay CLI-host-only (never armed from library paths).

Bun references verified read-only at local clone HEAD `e85606d484` (2026-09-05): `src/jsc/ipc.zig`, `src/runtime/api/bun/process.zig:318-406` (WaiterThread), `src/runtime/api/bun/subprocess.zig`/`spawn.zig`, `src/install/PackageManager.rs:84` (exact GLOBAL_CTX mirror), `src/bun_core/Global.zig:103-230` (is_exiting/Bun__onExit), `src/bun.zig:1574/1686/2012`, `src/jsc/VirtualMachine.zig:327` (threadlocal vm), `src/jsc/web_worker.zig`, `src/io/ParentDeathWatchdog.zig`, `src/runtime/cli/test/parallel/Worker.zig`.

### BCE residual
B0 census (2026-09-05): zero live BUG-class internal-process assumptions — every process-exit/process-global residue found is either already transposed (TLS/channel/thread + orderly-exit), public process semantics, or dormant executable-tier machinery unreachable from bao-layer paths (invariants recorded above). Transposition targets are architecture debt (B1-B3), not open BCE cases.

### Simplification ledger
None yet.

### Slice #1 completion (recorded 2026-09-07; landed in `dad8135d`, batch-3 wave)

`init_env_aliases` localization — **DONE, evidence**: `BaoRuntime::new()` no longer mutates host env (`src/bao_runtime/src/runtime.rs:26-28` retirement comment); alias resolved at the env read layer (`src/bun_core/util.rs:353-382` — `BUN_<SUFFIX>` miss falls back to `BAO_<SUFFIX>`, host-env-only variant preserved); zero direct `env::var("BUN_*")` reads remain in the bao layer; tests: `src/bao_runtime/tests/suite/env_alias_tests.rs` + `src/bao_cli/tests/cli_dispatch.rs:319`. (Ledger §9 had gone stale pointing at this slice; corrected 2026-09-07.)

### 2026-09-07 (daily-ops live)

- Nine-way taxonomy applied to the 21-commit bun window `f42e980255..d316760e8c`: ABSORB 1 (`e8541037c4` PathBuffer uninit UB → next-batch queue head; 177 call-site sweep) / ALREADY 3 / N-A-HOST 5 / N-A-UNREACHABLE 2 / N/A 10. Evidence: `.claude/daily-ops/triage-bun-2026-09-07.md`.
- SEMANTIC-PORT registered: bao `dns.lookup` returns empty result on EAI failure (`node_dns.rs:1823` `unwrap_or_default` + JS shim `callback(null, "", 4)`) where Node reports `getaddrinfo EAI_*` — found while judging `07f629a4c6` (whose init_eai fold bug is unreachable in bao: zero callers, live mapping `gai_error_to_dns_code` already correct).
- Batch-1 code: bun correctness ×4 queued from 09-06 (`f42e980255` zstd drain, `bdbe669b15` brotli drain, `86b2e060cf` install lockfile pool-by-bytes, `c01965ff72` bundler `[hash]` widen) + **#32 candidate #3 v1: CDP server thread stop+join** (`cdp-server/src/server.rs:93` run loop has no stop condition; `bao_browser/src/lib.rs:574` spawns without join; the existing `unpark()` is a no-op against a sleeping thread).
- Candidate #2 premise revision (blocking re-rank, not execution): `GLOBAL_HTTP2_FINGERPRINT` (`bao_stealth/src/http2.rs:186`) is process-global, but so is the TLS wire config it mirrors — both set at the same lifecycle point (`bao_browser/src/runtime_bridge.rs:1258` `servo::set_stealth_tls_config` + `:1274` h2 snapshot). The census's "TLS per-realm vs H2 global" contrast only holds for the JS-visible face (`engine_props.rs:333` REALM_PROFILES). Per-page wire fingerprinting (TLS+H2 together) requires page-identity plumbing into the servo net connector — one architecture decision covering both surfaces, not an H2-only fix. Re-ranked below #3 v1.

## 9. Next single action

**#32 candidate #3 v1 — CDP server thread stop+join on drop — is in flight (2026-09-07 batch-1). On completion, the next single action is the re-scoped candidate #2: wire page-identity through to the servo net connector so BOTH `StealthTlsWireConfig` and the H2 fingerprint can be keyed per-page (single decision, both surfaces; `runtime_bridge.rs:1258/1274` is the seam).** Then the e8541037c4 PathBuffer pool sweep (177 call sites).

## 10. Definition of Done

- internal process/IPC/process-local architecture inventory = 100% classified
- unnecessary internal helper/worker OS processes = 0
- retained OS-process internals all have explicit isolation adjudication
- public process APIs preserve process semantics
- process-local mutable state has explicit Runtime/TLS/thread ownership
- internal IPC is typed in-process messaging where OS IPC is unnecessary
- Bao runtime close/failure never terminates the host process
- multi-runtime and multi-thread isolation tests pass
- daily Bun upstream waves permanently use the expanded evolution taxonomy
- simplification/BCE evidence is continuously accumulated here
