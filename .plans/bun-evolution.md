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

### 2026-09-10 / B0 census refresh (#32 Phase 0 re-scan, pure inventory round)

**Baseline**: bao master `cc7f885a`. Method: delta re-scan vs the 2026-09-05 census (`.plans/b0-census-2026-09-05.md`) after the 2026-09-09/10 BRW-004 worker/SW waves + settings-stack/pump-bridge fixes; all `file:line` below read this run; test-only (`#[cfg(test)]`) code excluded. Zero code changes this round.

#### Resolved since 2026-09-05 census

- **Row 16 (top candidate #1) RETIRED** — `init_env_aliases` landed in `dad8135d` (see Slice #1 entry above); `runtime.rs:27` now carries the explicit "#32 / no `std::env::set_var` in the constructor" retirement comment.
- **Row 43 first item (top candidate #3 v1) LANDED** — CDP server thread is now cooperatively stopped and joined: `bao_browser/src/lib.rs:782-794` (stop_handle before move, `store(true, Release)` + `join()` on exit path), run loop polls the flag (`cdp-server/src/server.rs:33,103-111`), commit `f9005cf5`.

#### Family ① — spawn/fork/exec call sites (delta: unchanged)

All prior rows still hold verbatim: every runtime spawn site is public API semantics (`bun_spawn_sync.rs:412` Bun.spawnSync; `bun_api.rs:1991` Bun.spawn; `bun_shell.rs:520` $-shell /bin/sh; `bun_api.rs:9063` openInNewTab opener; `node_child_process.rs:1001+` child_process posix_spawn+pipes+fd-3; `node_cluster.rs` cluster.fork child processes; `install/lifecycle_script_runner.rs:1245` npm lifecycle) or build-time (`*/build.rs` rustc probes) or dormant executable machinery (`crash_handler/lib.rs:2887-2900` fork+execve auto-reload; `bun_core/util.rs:4707+` reload_process — still no bao-layer setter). **Zero internal-orchestration spawns** conclusion unchanged. Labels: all PUBLIC-PROCESS-SEMANTICS / N/A-dormant as in census rows 1-8, 13-15.

#### Family ② — IPC/pipe/socketpair/process message transport (delta: unchanged)

- `ipc_channel.rs:174-181` socketpair — still the single socketpair, still public child_process/cluster IPC support (census row 9).
- Internal transport still typed in-process mpsc everywhere new: BridgeChannel (`bao_cdp/src/servo_bridge.rs`), EventSubscriber pair (`bao_browser/src/lib.rs:749-750`, C19-② net tap), SW `CustomResponseMediator`/`SwManagers` (vendor, typed ipc-router channels — already the §2 target form).
- In-process wake pipes (not process IPC): `node_tls.rs:560` driver wake pipe, `node_fs.rs:3453` fswatch wake pipe — thread-wake machinery owned by joined threads (see family ③). **Zero process-shaped internal IPC** unchanged (row 10).

#### Family ③ — daemon/background helper/server loops (delta: 2 resolutions, 3 newly-listed detached threads)

- Daemon/background helper **processes**: still zero hit.
- CDP server thread: RESOLVED as above (owned + joined) → ALREADY-TRANSPOSED exemplar.
- NEW exemplar: fswatch hub worker `node_fs.rs:3358-3385` — `FswHub::Drop` = wake-pipe signal + `handle.join()` + fd close; the cancel→drain→join discipline this plan §2 prescribes. ALREADY-TRANSPOSED.
- Prior-missed production detached threads (pre-existing on 09-05, now listed; none new since):
  - `bao_runtime/src/dispatch.rs:200-211` `bao-dns-prefetch` fire-and-forget one-shot getaddrinfo thread (result discarded, warms OS resolver only; touches no bao state; self-terminating).
  - `bao_runtime/src/node_tls.rs:583` `bao-tls-driver` — process-global permanent wake-drain reader thread (companion of census row 23 `DRIVER` OnceLock; never joined, lives for process).
  - `bao_runtime/src/node_tls.rs:2848` `bao-tls-connect` per-op blocking-connect worker (10s internal deadline; detached).
  - Row 43 remainder unchanged: `web_api.rs:958` WS-connect slot (leak on JS-thread teardown), `node_crypto.rs:5531` / `node_fs.rs:709` / `bun_build.rs:289` per-op completion threads, `node_child_process.rs:288,3682` per-child `cp-fork-{pid}` pipe_poll (10ms sleep-poll).
  - All: mechanism already threads; gap remains ownership on `BaoRuntime::drop` → TASK-TRANSPOSE (per-op one-shots are bounded and bao-state-free; risk ordering below).

#### Family ④ — process-local global state (delta: +3 new rows, +1 vendor-seam cluster row, rest verified intact)

Prior rows verified still present at shifted lines: 17 servo Opts (`bao_browser/src/lib.rs:76` BAO_SERVO_OPTS_INIT), 18 PROCESS_MEMORY_BRIDGE (`bao_cdp_client/src/browser.rs:40`), 19 NODE_REALM/PAGE_GLOBAL (`runtime_bridge.rs:116-117`, blocked on #23), 20 GLOBAL_HTTP2_FINGERPRINT (`bao_stealth/src/http2.rs:186`, unchanged), 21 CONSOLE_TIMERS/COUNTERS (`node_console.rs:20-23`, blocked), 22 UDP_REGISTRY (`node_dgram.rs:18`), 23 DRIVER (`node_tls.rs:492+`), 24 WORKER_REGISTRY (`node_worker_threads.rs:44`), 25 CP_ASYNC_STATES/CP_IPC_CHANNELS/CP_STDIN_FDS (`node_child_process.rs:41+`), 26 BAO_PROCESS_START_NS (`bun_api.rs:7659`, public uptime anchor), 27-31 linked-tier weak rows unchanged.

NEW rows (all landed 2026-09-09/10 waves):

| # | Code path | Responsibility | Label |
|---|---|---|---|
| R51 | `src/bao_runtime/src/timers.rs:372-380` `BAO_SETTINGS_RUNNER: OnceLock<Box<dyn Fn>>` | Process-global servo settings-stack runner, registered per `BaoRuntime::new` (`bao_browser/src/lib.rs:286`, BCE-20260910-004), first-writer-wins | RUNTIME-LOCALIZE — same class as census row 30 engine hooks; multi-runtime: 2nd runtime's runner silently ignored (benign today only because every runtime registers the same closure) |
| R52 | `src/bao_runtime/src/fetch_async.rs:405-417` `THREAD_WAKEUP_BRIDGE: OnceLock<fn>` | Process-global thread-wake lookup bridge, registered per `BaoRuntime::new` (`lib.rs:294`), first-writer-wins | RUNTIME-LOCALIZE — same class as R51 |
| R53 | Vendor-seam per-runtime installer cluster: `servo::set_webviewless_resource_handler` (`lib.rs:320`, servo-side RwLock **last-write-wins**), `set_canvas_noise_seed` (`runtime_bridge.rs:1412`), `set_stealth_tls_config` (`runtime_bridge.rs:1423/1451` process-global wire config) | Every `BaoRuntime::new` overwrites process-global servo net/render config with that runtime's stealth profile | RUNTIME-LOCALIZE — **BLOCKED** on the re-scoped candidate #2 page-identity plumbing (this row IS that candidate's census anchor); multi-runtime = last runtime's TLS/canvas/H2 fingerprint silently wins process-wide |
| R54 | `src/bao_runtime/src/gc_store.rs:119-126` `thread_local! GC_STORE` | Bare-JSObject rooting store keyed per thread, rooted before allocation windows (9f5f4692) | ALREADY-TRANSPOSED (TLS exemplar; never crosses threads) |
| R55 | `src/bao_runtime/src/dispatch.rs:207` dns-prefetch + `node_tls.rs:583/2848` threads (family ③) | detached one-shot / permanent / per-op threads | TASK-TRANSPOSE (family ③ remainder) |

Also verified: zero `tokio::spawn`/`spawn_blocking` in bao layer (the C19 S2b `spawn_blocking` pattern lives only in vendor `http_loader.rs`); zero `atexit`; signal sites remain the public windowed quartet (`product_native_symbols.rs:58-122` `Bun__currentSyncPID`) + `process.kill` self-check/SIG_DFL (`bun_api.rs:7547-7555`) = PUBLIC-PROCESS-SEMANTICS; `process.env` write bridge `bun_api.rs:7597/7617` = PUBLIC-PROCESS-SEMANTICS (public env API, distinct from the retired constructor mutation). `runtime_bridge.rs:1141` / `web_api.rs:1869` `static mut FORMAT` JSErrorFormatString tables = N/A (immutable format tables). Per-WebView-keyed vendor injector registries (`EMBEDDER_WORKER_SCOPE_INJECTORS` etc., upsert per webview + `unregister_worker_injectors` on page close, `page.rs:1342`) = correctly keyed, no interference.

#### To-translate list (target form per plan §2)

| Item | Target form |
|---|---|
| R53 vendor-seam stealth/net config cluster (+ census row 20 GLOBAL_HTTP2_FINGERPRINT) | page/runtime-identity keyed config plumbed into servo connector (re-scoped candidate #2; single decision covering TLS wire + H2 + canvas seed + webviewless handler faces) |
| R51 BAO_SETTINGS_RUNNER / R52 THREAD_WAKEUP_BRIDGE | runtime-owned registration (per-runtime slot or keyed registry), or explicit first-runtime-wins contract documented + enforced |
| Family ③ remainder (dns-prefetch one-shots; tls-connect/WS-connect/crypto/fs/build per-op threads) | task/owned-thread with cancel→drain→join on `BaoRuntime::drop` (fswatch hub + CDP server are the in-tree patterns to copy); tls-driver thread → runtime-scoped driver with explicit shutdown |
| Census rows 22-25 registries (UDP/WORKER/CP) | BaoRuntime-owned registries cleared on drop (B1) |
| Census row 18 PROCESS_MEMORY_BRIDGE | token-keyed map or newest-wins documented (B1) |
| Blocked: rows 17/19/21 | unchanged blockers (servo upstream Opts / #23 scoping / product-semantics ruling) |

#### Public API legitimate-retention list

`Bun.spawn`/`spawnSync`/`$`shell/`openInNewTab`, Node `child_process.*` (spawn/fork/exec + fd-3 IPC socketpair), `cluster.fork` + primary/worker PID messaging, `process.env` read/write bridge, `process.pid`/`process.kill` (+ windowed signal forwarding), `process.uptime` start anchor, `inspector.Session` fail-closed, npm install lifecycle child processes, test-harness `bunExe`/`bunRun`. All real OS-process semantics by contract — out of elimination scope per #32.

#### Risk ranking (unchanged order; refresh conclusions)

1. **Biggest risk: R53 vendor-seam cluster** — process-global stealth TLS/H2/canvas config overwritten per-runtime is a product-correctness defect under multi-runtime embedding (undetectable inconsistency between runtimes' fingerprints; also the ledger's re-scoped candidate #2). BLOCKED on page-identity plumbing architecture decision.
2. Family ③ detached-thread ownership remainder (bounded one-shots, lower urgency; `bao-tls-driver` permanent thread + WS-connect slot leak are the two with actual lifetime hazards).
3. R51/R52 first-wins bridges (benign today, contract should be made explicit).

**§9 premise note (additive, no rewrite)**: §9's "#3 v1 in flight" premise is stale — it landed in `f9005cf5`. Per §9's own second sentence, the standing next single action is the re-scoped candidate #2 (= R53 cluster above).

### 2026-09-10 / R53 decision proposal (design-only round; zero code changed)

Purpose: unblock the risk-rank-#1 row via an explicit user decision. **Nothing below is implemented; implementation of any option requires a user ruling first.**

#### ① Current-state precise anchors (all re-read this run)

Cluster members, per member: storage / setter path / consumers / lifecycle.

1. **TLS wire config** — storage `vendor/servo/components/net/connector.rs:88` `static STEALTH_TLS_CONFIG: RwLock<Option<StealthTlsWireConfig>>`; setter chain `connector.rs:94` ← `servo::lib.rs:263` ← bao `runtime_bridge.rs:1423`(Some)/`:1451`(None); consumers `connector.rs:274` `create_tls_config` (websocket path `http_loader.rs:2105`) and `bun_bridge.rs:1938` `obtain_response_bun` (THE page egress path — reads the global **per request**).
2. **H2 fingerprint** — storage `src/bao_stealth/src/http2.rs:186` `GLOBAL_HTTP2_FINGERPRINT: RwLock<Option<Http2Fingerprint>>`; setter `http2.rs:191` ← `runtime_bridge.rs:1439`/`:1452`; consumer `bun_bridge.rs:1969` per request.
3. **Canvas noise** — storage `vendor/servo/components/canvas/canvas_noise.rs:9,13,16` (three atomics); setter `canvas_noise.rs:22` ← `servo::lib.rs:304` ← `runtime_bridge.rs:1412`; consumer `canvas_paint_thread.rs:318` `apply_canvas_noise(.., get_global_canvas_noise())` — ONE process-wide paint thread (`canvas_paint_thread.rs:28-32`, `canvases: FxHashMap<CanvasId, _>`), commands keyed by `CanvasId` only, **no webview identity on the canvas command path**.
4. **Webviewless resource handler** — storage `vendor/servo/components/net/request_interceptor.rs:53` `BAO_WEBVIEWLESS_RESOURCE_HANDLER: parking_lot::RwLock<Option<Arc<dyn Fn>>>`; setter `request_interceptor.rs:58` ← `servo::lib.rs:298` ← `bao_browser/src/lib.rs:320` (inside `BaoRuntime::new`); consumer `request_interceptor.rs:95` for `target_webview_id == None` fetches. Today every runtime installs the identical constant `PassThrough` closure → last-write-wins is currently **semantically invisible** (benign until an embedder installs real logic).
5. (Adjacent, same family, discovered this run) **fetch() thread-local profile** — `src/bao_runtime/src/fetch_api.rs:26` `TL_STEALTH_PROFILE` set per page install (`runtime_bridge.rs:1410/1450`); read by page egress on the ScriptThread. Pages sharing one ScriptThread (`force_isolate=false`) get last-install-wins on the JS-visible fetch face — same seam class, keyed-per-realm lookup (`engine_props::set_profile_for_global`, `runtime_bridge.rs:1408`) exists but fetch does not use it.

**Census correction (supersedes the row-53 wording)**: the TLS/H2/canvas setters do NOT fire per `BaoRuntime::new` — they fire **per page creation**: `PagePool::create_page` (`page_pool.rs:97`) → `inject_all_with_profile` (`runtime_bridge.rs:1549`) → `install_all_native` (`runtime_bridge.rs:1412/1423/1439`). Only member 4 is per-runtime. Consequence: the silent cross-contamination is **already reachable in a single runtime** — two pages with different `PageConfig.stealth_profile` (`config.rs:15`, public per-page field) leave the process wire config + canvas seed = last-created page's profile, applied to *all* pages' subsequent new connections and canvas readbacks. Multi-runtime is the second axis, not the only one.

**Identity carrier already exists in the net layer**: `net::request::Request` carries `target_webview_id: Option<WebViewId>` + `pipeline_id: Option<PipelineId>` (`shared/net/request.rs:497-498`), threaded through `http_loader` (`:129,668,693,2386`) and into `obtain_response_bun` (`bun_bridge.rs:1799` — receives `pipeline_id` today, not yet `target_webview_id`). The SSLConfig intern registry (`bun_bridge.rs:1969` comment) already keys connection pools by config pointer — distinct per-page configs automatically get distinct connection buckets (no cross-profile connection reuse).

#### ② Option space

**A — page-identity plumbing (per-WebViewId config registries)**
Net face: `STEALTH_TLS_CONFIG` and `GLOBAL_HTTP2_FINGERPRINT` become `WebViewId → config` registries (+ explicit fallback entry for identity-less requests); `install_all_native` writes keyed by `webview_id` (already in scope); `http_loader` passes `request.target_webview_id` into `obtain_response_bun`; reads at `bun_bridge.rs:1938/1969` and `create_tls_config` resolve per request. Canvas face: per-canvas noise config instead of the global — stamp `CanvasNoiseConfig` at canvas creation (script thread knows both page and canvas) into the `Canvas`, read it at the `GetImageData` choke point; deletes `canvas_noise.rs` globals entirely. Webviewless handler: keyed by an owner tag threaded from the runtime (or left process-global with a documented single-verdict contract — it is constant today).
Single-runtime single-profile = zero behavior change (every page writes the same entry; lookups return it). Fixes both multi-page-divergence (live defect) and multi-runtime. Heaviest canvas plumbing (CanvasMsg path has no identity today). Open semantics sub-decision: **which profile owns a webview-less (SW/worker-realm) outbound fetch** — owning page / registration scope / process default; needs a ruling or a fail-closed default.

**B — per-BaoRuntime instance-domain isolation**
Config slots move from process statics into per-Servo-instance state (net resource thread / canvas thread are per-constellation = per-instance), embedder passes a config bundle handle at `BaoRuntime::new`. Solves multi-runtime cross-talk; does NOT solve multi-page divergence inside one runtime (last-page-install-wins persists within the instance). Medium vendor delta, embedder API shape change. Zero-regression argument trivial (one instance).

**C — status quo + explicit fail-closed contract**
No vendor delta. bao API enforces one-profile-per-process: `PagePool::create_page` (and runtime construction) rejects a second distinct `stealth_profile` with an explicit error instead of silently contaminating; SPEC documents the constraint. Converts the silent defect into an explicit refusal. Cheapest; forecloses per-page profile divergence (which the public `PageConfig` API already implies possible).

#### ③ Per-option impact matrix

| | vendor patch delta | bao delta | stealth consistency | single-runtime single-profile regression risk | effort |
|---|---|---|---|---|---|
| A | connector + bridge + http_loader identity arg + canvas per-canvas config (heaviest) | keyed setters + page-close unregister + webview-less fallback | full: wire layer identity granularity finally matches the JS layer's per-realm model (`engine_props`) | zero change (same entry content, same lookups); connection pooling self-segregates via SSLConfig interning | L (canvas face is most of it; net face is small — identity already flows) |
| B | per-instance config plumbing in net/canvas threads | config bundle handle at construction | partial: fixes multi-runtime only; in-runtime page divergence stays broken | trivial | M |
| C | none | profile-uniqueness guard in PagePool/runtime | none gained; defect becomes explicit error | trivial (guard must allow the homogeneous case) | S |

#### ④ Recommendation

**A**, staged: net face first (TLS+H2 keyed by `target_webview_id` — small, identity already threaded, fixes the live single-runtime defect), canvas face second (per-canvas config, deletes the canvas globals), with C's fail-closed guard as an interim guardrail only if staging spans waves. Basis: the JS-visible layer already made this exact decision (per-realm profiles, BUG-ENG-366 / `set_profile_for_global`) — the wire/render layer lagging it is the residual; B solves only the second axis; C documents a defect instead of closing it. Blocking input for the user ruling is the **第零项 prerequisite** below plus the webview-less-fetch profile-ownership semantics.

#### ⑤ 第零项 prerequisite (user decision required before ANY option)

0. Is **per-page stealth profile divergence** (different `PageConfig.stealth_profile` for pages of one runtime) a supported product scenario, and is **multi-runtime embedding with divergent profiles** one? The answer picks the option: both supported → A; only multi-runtime → B; neither (one profile per process is the contract) → C. (Today's public API shape implies per-page divergence is offered, but no user ruling on record confirms it as a *supported scenario* rather than an unexercised struct field.)
1. If A: which profile governs a webview-less (SW/worker-realm) outbound fetch — owning page, registration scope, or process default?

**No implementation performed this round** (design-only; per §0 the decision belongs to the user).

#### ⑥ R53-A phase 1 IMPLEMENTED — net face (user ruling 2026-09-10: option A, do it thoroughly, net face first)

User ruling 2026-09-10: **option A, staged, net face first** (canvas face = phase 2, out of this wave). Webview-less ownership ruling (⑤.1): **registering page's profile** — ScopeThings-webview_id-inheritance made explicit on the net Request (same host-page semantics dedicated/shared workers have natively and W1a stealth inheritance uses).

What landed (single wave, commit `<this-wave>`):

1. **vendor `net/connector.rs`** — `STEALTH_TLS_BY_WEBVIEW` / `STEALTH_H2_BY_WEBVIEW` (LazyLock RwLock<HashMap<WebViewId, Option<…>>>; explicit `None` value = stealth-free page stays stealth-free, no fallback inheritance) + `set/clear_stealth_wire_config_for_webview` + `resolve_stealth_{tls_config,http2_fingerprint}(Option<WebViewId>)` (keyed hit authoritative; miss/identity-less → process-global fallback — SW script-update and other infra fetches keep pre-R53 behavior) + `create_tls_config` gains `webview_id` param (WS path). Process-global `set_stealth_tls_config` / `GLOBAL_HTTP2_FINGERPRINT` PRESERVED as fallback + existing getter semantics (zero break).
2. **vendor `net/fetch/bun_bridge.rs`** — `obtain_response_bun` gains `target_webview_id`; both faces resolve keyed. SSLConfig interning self-segregates pools per profile (content → distinct pointer → distinct connection bucket).
3. **vendor `net/http_loader.rs`** — passes `request.target_webview_id` at both call sites (bun bridge + WS `create_tls_config`).
4. **vendor script stamps** — `ServiceWorkerGlobalScope.owning_webview_id` (from `ScopeThings`, #[no_trace]) + `GlobalScope::egress_webview_id()` (SW → registering page; others = `webview_id()`; upstream's deliberate `None`-for-SW storage-partition semantics untouched) + `net_request_from_global` (dom/fetch/request.rs) and XHR (xmlhttprequest.rs) Request construction stamped with egress identity.
5. **bao_browser** — `install_all_native` dual-writes (keyed authoritative entry + process-global fallback bucket, both arms incl. explicit stealth-free entry); `Page::close` clears the keyed entries next to `unregister_worker_injectors`.
6. **Tests** — `stealth_per_page_wire_tests.rs` (suite): ① `per_page_divergent_wire_profiles_live` (two pages Firefox/Chrome in ONE runtime, each page's ClientHello carries ITS profile's supported-groups anchor — Firefox keeps P-521; JA3s differ; both ALPN h2,http/1.1) ② `sw_egress_rides_host_page_profile_under_divergence_live` (SW-forwarded fetch rides the REGISTERING page's Firefox profile while a Chrome page coexists). **RED proven first on unchanged code** (both pages' JA3 identical = last page's profile; SW egress = chrome), then GREEN.

Evidence: RED run (pre-change) both new tests FAIL with the cross-contamination symptom; GREEN run 16/16 (new 2 + sw_stealth_profile 4/4 + page_net_bun_fingerprint e2e); wider regression 53/53 (page_wss, page_net_bun_full_matrix, stealth_fingerprint, serviceworker mediation/controller/fetchevent, worker_fingerprint_consistency). Known-flake exclusion: `h2_fetch_node_stack_e2e_tests::window_fetch_wire_h2_post_body_roundtrip` failed twice under machine saturation — pre-classified starvation flake (brw004-final-certification.md "按 flake 挂账"; 7/7 green at certification; test drives AsyncHTTP/HTTPThread directly, zero call-path overlap with this wave's deltas).

Residuals (phase 2 / documented):
- **Canvas face** — LANDED as phase 2 (⑦ below); no longer a residual.
- **Fallback bucket last-write-wins** (identity-less fetches only, post-SW-stamp = SW script updates + misc infra): process-global still written per install; pre-R53 behavior preserved by design; documented residual, not a page-visible surface.
- `fetch()` thread-local profile face (proposal member 5, `TL_STEALTH_PROFILE` last-install-wins on shared ScriptThread) — NOT in this wave's scope; same-seam follow-up candidate.
- Webviewless resource handler (member 4) — untouched, constant verdict, per proposal.

### 2026-09-11 / ⑦ R53-A phase 2 IMPLEMENTED — canvas face (per-WebViewId canvas noise)

User ruling 2026-09-10 (R53-A, staged): phase 2 = canvas face. Semantics carried over from the net face (⑥): keyed hit AUTHORITATIVE (explicit `None` = stealth-free page reads back byte-exact, no inheritance through the fallback); miss / identity-less → process-global fallback (pre-R53 behavior); worker/SW-realm canvases ride their HOST page's profile (same ownership ruling — identity = `GlobalScope::egress_webview_id`). The `canvas_noise.rs` globals are NOT deleted — they remain the identity-less fallback bucket (zero break, mirroring the net face's preserved process-global getters).

What landed:

1. **vendor `canvas/canvas_noise.rs`** — `CANVAS_NOISE_BY_WEBVIEW` (`LazyLock<RwLock<HashMap<WebViewId, Option<(seed, amplitude)>>>>`; explicit `None` value = stealth-free) + `set/clear_canvas_noise_for_webview` + `canvas_noise_for_webview` (keyed hit authoritative incl. explicit `None`; miss → `get_global_canvas_noise()`).
2. **vendor canvas identity chain (4 files)** — `ConstellationCanvasMsg::Create` (`shared/canvas/lib.rs`) and `ScriptToConstellationMessage::CreateCanvasPaintThread` (`from_script_message.rs`) gain `Option<WebViewId>`; `CanvasState::new` (`script/dom/canvas/2d/canvas_state.rs`) stamps `global.egress_webview_id()` at the single canvas-creation site (worker/SW → host page; window → own id); `constellation.rs` is a pure relay (does not interpret the identity — no constellation restructure needed).
3. **vendor `canvas/canvas_paint_thread.rs`** — `canvas_webviews: FxHashMap<CanvasId, WebViewId>` stamped at `create_canvas`, dropped at `CanvasCommand::Destroy`; the `GetImageData` choke point resolves per owning webview (keyed → identity-less → process-global fallback). W2 gate untouched: a disabled resolution still applies zero noise (byte-identical readback).
4. **vendor `servo/lib.rs`** — `set_canvas_noise_for_webview` / `clear_canvas_noise_for_webview` embedder API (seed 0 writes the explicit disabled entry).
5. **bao_browser** — `install_all_native` dual-writes canvas noise (keyed authoritative + process-global fallback) in the profile arm; the stealth-free arm writes ONLY the keyed explicit-`None` entry (the process-global canvas noise is untouched there — pre-R53 semantics: stealth-free installs never wrote it). `Page::close` clears the keyed entry beside the wire-config clear.

Tests — `stealth_per_page_canvas_tests.rs` (suite, flat rgb(128,128,128) fills put every byte on a u8 rounding boundary, so a full readback is the deterministic noise sign map: zero noise = byte-exact, different seeds = different maps):
- ① `per_page_divergent_canvas_noise_live` — two pages (Firefox seed 42 / Chrome seed 137) in ONE runtime; each page's full-canvas `getImageData` digest is deterministic and the two DIFFER (each rides its own seed); both noisy.
- ② `stealth_free_page_canvas_zero_noise_live` — a stealth-free page created AFTER a stealthed page reads back byte-exact (flips == 0) while the stealthed page stays noisy.
- ③ `worker_offscreencanvas_rides_host_page_profile_live` — each page's Worker OffscreenCanvas readback rides its HOST page's profile (digests differ across pages, each internally deterministic ×2 readbacks).
- **RED proven first on unchanged code**: ① both pages identical digest `OK:2165:-127038693` (= last page's seed served both) ② the stealth-free page inherited the stealthed page's seed (`OK:2037:-1981572419`, flips 2037 ≠ 0) ③ both workers identical (`OK:1646:-1967652232`). Then GREEN 6/6.
- Canvas-crate unit tests 16/16: new keyed hit-authoritative / miss-fallback / explicit-`None` / clear semantics + preserved W2 zero-diff family (`noise_none_leaves_bytes_untouched`, `global_seed_zero_yields_none_and_roundtrip`, parity).

Regression: 78/78 GREEN — stealth_per_page_wire (incl. both live wire tests), stealth_offscreencanvas 15/15 (incl. the W2 noise-parity live assertions `c13_retirement_{window,worker}_noise_*_parity` and `e36_stealth_free_page_has_no_stealth_chain`), stealth_fingerprint 6/6, sw_stealth_profile 7/7 (incl. c19 SW TLS/H2 + CDP observability), worker_fingerprint_consistency 9/9; canvas-crate unit 16/16.

Residuals (documented):
- Identity-less canvas fallback bucket (canvases created before their page's install / identity-less realms) still process-global last-write-wins — same documented residual class as the net face's fallback bucket; not a page-visible surface.
- An OffscreenCanvas transferred cross-webview via structured clone keeps its CREATION-time webview identity (the noise config travels with the canvas, matching pixel content drawn under that profile) — edge semantic, documented here.
- `fetch()` thread-local profile face + webviewless handler — unchanged from ⑥'s residual list.

### 2026-09-17 / PathBuffer sweep residual closure — `depth_buf_uninit()` UB PORT(f8f6bd0f 漏网点)

**Trigger**: interactive session audit found ledger §9 stale (queued the already-landed f8f6bd0f sweep as next); re-audit of that sweep surfaced one missed site. **Baseline**: bao master `dc532b47`.

**Defect**: `src/install/lockfile/Tree.rs` `depth_buf_uninit()` still returned `DepthBuf = [Id; MAX_DEPTH]` built with `MaybeUninit::uninit().assume_init()` under `#[allow(invalid_value, clippy::uninit_assumed_init)]` — the same UB class the upstream `e8541037c4` sweep eradicates (an integer requires initialized memory at construction, regardless of later writes). The 09-08 absorption `f8f6bd0f` swept the `PathBuffer` face (69 files) but missed this site; tree-wide re-audit of all remaining `assume_init` hits confirmed every other one is a legal form (`new_zeroed` / libc FFI out-param / allocator internals) — this was the sole UB residual.

**Fix (PORT, zero new design — upstream shape verbatim)**: `DepthBuf = [MaybeUninit<Id>; MAX_DEPTH]`; `depth_buf_uninit()` → `[const { MaybeUninit::uninit() }; MAX_DEPTH]` (lint attr + UB body deleted); consumer sites `.write(0)` / `.write(parent_id)` / read-back `unsafe { depth_buf[depth_buf_len].assume_init() }` with upstream SAFETY comment (read indices `1..=depth`, all covered by the parent walk). Files: `Tree.rs` +14/−14, `PackageInstaller.rs` +3/−1 (`[const { MaybeUninit::new(0u32) }; N]` type-follow), `lockfile_json_stringify_for_debugging.rs` +3/−1 (same); `lockfile.rs` zero-change (whole-buffer pass-through, type follows). Known bao-only divergence preserved untouched: the pre-`if tree.id > 0` `depth_buf[0].write(0)` line has no read site (upstream lacks the line) — mechanical adaptation only, zero behavior delta.

**Evidence (V = main session, independent re-run; E self-report not trusted per acceptance protocol)**: `command grep "invalid_value|uninit_assumed_init" src/install/lockfile/Tree.rs` → 0 hits; `cargo check -p bun_install --jobs 4` RC=0; `cargo nt -p bun_install --cargo-profile test-ci` 2/2 passed (baseline == post, zero new failures — test surface is the 2 package_install tests); clippy: no uninit-class diagnostics (pre-existing `bun_uws_sys` build-script map_or noise untouched by this wave, not ours). Inline-const array syntax accepted (rust-version 1.89).

**BCE check**: single-site residual of an already-swept class; tree-wide `assume_init` audit above is the horizontal sweep — residual count now 0 for the uninit-integer-constructor class in `src/`.

### 2026-09-17 / B1 first slice — R51/R52 first-writer-wins contract (fn-pointer tightening + equality enforce)

**Adjudication** (interactive session, engineering-detail tier per §247 options): fact-check showed both registration sites are zero-capture forwarders (`bao_browser/src/lib.rs:304` settings runner → `servo::bao_run_in_script_settings`; `:313` wake lookup → `servo::bao_current_thread_wake_fn`), so first-writer-wins is semantically lossless today — contract form chosen over per-runtime slot/keyed registry (minimal diff; per-runtime ownership has no current consumer).

**Code** (4 files + 1 test file):
- `BaoSettingsRunner` tightened `Box<dyn Fn(..) + Send + Sync>` → **bare `fn` pointer** — the structural half of enforcement: any future registration that captures per-runtime state stops compiling instead of silently diverging. Registration site drops `Box::new` (closure coerces); forwarder body byte-identical. **Registry-visible breaking type change — next publish closure must bump `bun_runtime` minor (0.1.x → 0.2.0), not patch.**
- Both setters: `set` failure arm compares rejected value vs installed via `std::ptr::fn_addr_eq` — equal → idempotent no-op (documented N-runtimes shape); different → `debug_assert!` fail-closed with contract text; release keeps plain first-wins (zero behavior delta).
- Contract docs on both setters (multi-runtime shape, fail-closed semantics).

**Tests** (`tests/suite/bridge_contract_tests.rs`, 4): R51/R52 same-pointer triple registration idempotent + registration-never-invokes counter assert; R51/R52 divergent-pointer `#[should_panic(expected="…re-registration diverged")]` under `#[cfg_attr(not(debug_assertions), ignore)]`. ICF-hardened via per-fn side-effect counters.

**Evidence (V independent re-run)**: `cargo check -p bun_runtime` RC=0; `cargo check -p bao-browser` RC=0; `cargo nt -p bun_runtime -E 'test(bridge_contract)'` 4/4 PASS; `-E 'test(timers)'` 67/67 == baseline (suite total 1218→1222, +4 exactly the new tests).

**B1 residual**: RUNTIME-LOCALIZE rows beyond R51/R52 (18 prior rows from the 2026-09-05 census, §8 tables) remain for subsequent B1 slices; same-pattern audit candidates: the servo-side OnceLock registries named in the R51 block comment (pump/realm-discard face) share the pattern but live at the vendor seam — next slice decides whether to contract-ize them symmetrically or leave vendor-seam registries as documented-acceptable.

### 2026-09-17 / B1 slice 2 — runtime-drop resource full-termination (rows 22/24/25 CLOSED, user ruling A)

**Ruling** (user, 2026-09-17): runtime-owned unclosed resources MUST close on `BaoRuntime::drop` — leaking was confirmed real (fd+port → EMFILE; worker threads pinning JSContext+stack; zombie children + dead-runtime pipes). Option A (full termination) over Node-exit-mirror / registry-only / status-quo.

**Infrastructure** (row-18 generation-token pattern generalized; `6dd02605`): `NEXT_RUNTIME_TOKEN` monotonic from 1, 0 = process-shared sentinel (resources created outside any BaoRuntime are never swept — embedder-managed assets safe); `CURRENT_RUNTIME_TOKEN` thread-local stamped at registration points; `impl Drop for BaoRuntime` runs `cleanup_runtime_resources(token)` in the Drop body (before field drops — `_guard` engine-teardown-last contract byte-identical); TLS cleared only when this runtime is still the thread's latest (parasitic-runtime-safe).

**Three registries swept**:
- row 22 `UDP_REGISTRY` (`6dd02605`): owner-stamped entries; sweep retains-out and drops the runtime's `UdpSocket`s (same semantics as `__dgram_close`); fd+port released.
- row 24 `WORKER_REGISTRY` (`53059d6d`): `WorkerHandle.owner`; sweep = registry-remove (no DashMap guard across join) → `WorkerMessage::Terminate` (the same message-level path the recv loop serves; zero cx/JS dependency) → `join_worker_bounded` (is_finished polling, 5s cap, timeout detaches but counts; covers panic-dead threads). JS `terminate()` unbounded join untouched.
- row 25 `CP_ASYNC_STATES`/`CP_IPC_CHANNELS` (`15c913d7`): SIGTERM → WNOHANG reap (2s window) → SIGKILL (1s) → Unkillable = loud + entry retained (no double-kill); ECHILD mirrors poll-thread bookkeeping; fd take-then-close claiming protocol shared with the poll-thread tail (no double-close of recycled fd numbers); IPC channels adjudicated child-lifetime-owned and swept by pid.

**Tests**: `runtime_resource_cleanup_tests.rs` T1–T7 (fd close + port release + real datagram loopback isolation / worker thread exit via /proc comm / child death + no-zombie + pipe fds + IPC parent-end close / per-token isolation across all three).

**Out-of-scope finding (pre-existing product defect, follow-up slice)**: SIGTERM is ineffective on the Linux spawn path — signal-reset intent (`SETSIGDEF|SETSIGMASK`/`reset_signals`) is consumed only by the macOS posix_spawn branch (`spawn_process.rs:647`, `posix_spawn.rs:647`); the Linux vfork shim's `BunSpawnRequest` carries no signal fields, children inherit the spawner's blocked/ignored signal state, and `execve` only resets caught signals. JS `child.kill()` (same `libc::kill`) is equally unable to terminate such children. The sweep's SIGKILL escalation keeps today's cleanup correct (2s+1s worst case). **Next-priority slice: spawn_sys signal-reset plumbing.** Residual observations: `CP_STDIN_FDS` (thread-local stdin write ends) survive runtime drop — same-family row for a follow-up; `CpCleanup` RAII is dead code tree-wide (candidate removal).

**Weak rows 27–31 disposition**: 28 (CLI-product `GLOBAL_CTX`) and 31 (C-ABI mirror `UseSystemCA`) = documented-accept; 27 (`TOP_LEVEL_DIR`) / 29 (argv/streams) / 30 (engine hooks) remain unscheduled transposition candidates under the now-settled ownership shape.

## 9. Next single action

**Correction 2026-09-17 (stale §9, same failure class as the 09-07 `init_env_aliases` stale): the e8541037c4 PathBuffer pool sweep was ALREADY LANDED in `f8f6bd0f` (2026-09-08, 69 files, 360+/338-).** The 09-11 §9 update that queued it as next was written without checking master history. Residual closure of that sweep landed 2026-09-17 (this wave): `depth_buf_uninit()` in `src/install/lockfile/Tree.rs` — the one `MaybeUninit::uninit().assume_init()` UB constructor the sweep missed (`DepthBuf` bare `[Id; N]` + lint-suppression `#[allow]`), PORTed to the upstream `[MaybeUninit<Id>; N]` shape (see §8 entry for evidence).

**Next single action: QUEUE EMPTY (2026-09-18 closeout, user ruling "全部处理掉").**

- row 27 (`TOP_LEVEL_DIR`): CLOSED (`d812dca3`) — per-runtime resolver root via a thread-local overlay over the process-global (single read point `bun_core::top_level_dir()` follows for every consumer; install_runtime_root seeds after engine-init success; drop clears if-same). Two census-missed auto-following consumers recorded (compile_target.rs:224, GlobWalker.rs:1370).
- R53-A fetch face (`TL_STEALTH_PROFILE`): CLOSED — fetch egress resolves keyed-per-Realm first (`engine_props::profile_for_global`, new `full` wire-face field on RealmProfile), thread-local demoted to the identity-less fallback (CLI/engine/test realms byte-for-byte). **Honest re-characterization (E12 RED-probe)**: the shared-ScriptThread contamination of ledger §277 is NOT live-constructible today — bao pins `force_isolate_event_loops: true` (vendor opts panics on false), so every page already owns its ScriptThread and the TLS read was per-page-correct under the current topology. The change is therefore a **topology-independent semantic hardening** (identity-keyed resolution aligned with BUG-ENG-366 unconditional isolation; Node-Realm aliasing and stealth-free TLS residual windows closed by contract), NOT a live behavior fix; commit 46a103e3's message overstates it as live contamination. Compensating deterministic pin: keyed-getter unit test (RED-before by construction — the getter did not exist). Residual observed (same class): `web_api.rs:925` WebSocket wss egress still reads the TLS fallback only — same hardening shape pending.
- R53-A webviewless handler: documented-accept (vendor contract note) — webview-LESS requests carry no page identity (nothing to key BY); sole embedder installs one structurally-PassThrough handler so last-writer-wins is unobservable; revisit if a non-PassThrough handler ever appears.
- row 29 (argv/streams): documented-accept (bun_core output.rs contract) — process-owned output state by Node convention; a per-runtime output sink is a NEW product capability needing an explicit PRD ruling if ever wanted.
- #42 signal sets: CLOSED same-day (3384d1ca, see §8); CP_STDIN_FDS/CpCleanup CLOSED (ef6554f3); weak row 30 CLOSED (3e2d5a6f); rows 22/24/25 CLOSED (ruling A, §8 B1 slice-2 entry).

**Standing next (phase-level, not queued items)**: B0/B1 DoD gap review at the next daily-ops round (B2 IPC-to-channels phase gating; remaining §8 documented residuals all carry explicit accept/revisit contracts). Rows 17/19/21 remain blocked on their recorded upstream/product-semantics gates.

Earlier 2026-09-17/18 wave index: #42 spawn signal fix → B1 slice-2 resource sweeps (rows 22/24/25 + stdin) → publish closures (bun_install 0.1.17; bun_runtime 0.2.0 + 8 crates) → row 27 resolver root → fetch keyed resolution.

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

### 2026-09-18 / BCE domain closure (interactive-session domain, 2 error-class events)

**Attribution (SOL channel unavailable — gpt-5.6-sol model routing 400; main-session equivalent read-only attribution per hard-gate 6 fallback):**
- Event A (E14 wss test first-red): class 偷懒 (test draft took the cheaper bare-call shape instead of mirroring the E12 AutoRealm precedent). Layers: surface = assertion arm called a realm-dependent resolution from bare Rust context (empty realm stack → CurrentGlobalOrNull null → designed-in TLS fallback → discriminating assert failed); design = none (the read point's fallback-on-no-realm IS the contract); paradigm = **same-family test shape alignment was not checklist-ized**.
- Event B (46a103e3 overstatement): class 偷懒 (live-behavior claim made before constructing a RED). Layers: surface = commit message asserted "cross-contaminated" without subjunctive; design = none (code correct, contracts added); paradigm = **commit characterization must be evidence-gated on a constructed RED** — GREEN does not prove the defect exists (the RED-probe later showed the shared-ScriptThread shape is not live-constructible under the pinned force_isolate_event_loops=true).

**Sweep (grep evidence, main session):** dual-source read points — production bare `get_fetch_stealth_profile` callers = 0 (fallback tail by design + setter/getter round-trip test pins only); `current_fetch_profile` production = exactly the two egress read points (fetch_api.rs:534, web_api.rs:936); both test call sites (web_socket_async_tests.rs:758/809) verified inside `AutoRealm::new_from_handle` with arm ①/② varying only the keyed dimension — zero bare-context same-type assertions remain. Same-shape risk spot-check across the wave's fix commits: 58a395f3 (should_panic genuinely fires = RED constructible), d812dca3 (overlay observed pre-fix), 3384d1ca (SigBlk before/after), resource sweeps T1-T7 (behavioral before/after) — all carry RED-evidence forms; 46a103e3 was the sole violator and its ledger correction landed in 8ab7c381.

**Residual = 0.** Recurrence hardening persisted to operator memory (realm-dependent-test-shape-and-red-declaration): (1) realm-dependent test assertion arms must run inside AutoRealm mirroring the production JSNative shape; (2) fix-commit live-behavior claims require a constructed RED before assertion — V acceptance checks RED evidence form, not just GREEN.

### 2026-09-21 / B2 phase opening — channels census delta + contract-drift findings (E-B2, read-only)

**Census delta since 09-10 refresh** (all file:line verified this run): D1 WorkerChannelBridge/SharedWorkerPortChannel (delegate.rs:357-380,640-700, per-worker typed structured-clone, NOT in B1 token sweep — worker-owned); D2 page-level BridgeChannel (runtime_bridge.rs:2833-2931, public embedder face, dual close semantics); D3 InMemoryTransport channel face (in_memory.rs:93-262, memory:// CDP carrier); D4 B1 slice-2 close-on-drop contracts already landed (worker Terminate+bounded join 5s, CP_IPC child-lifetime sweep) — the tree's FIRST close-on-drop contract, i.e. B2 target shape locally landed; D5 EventSubscriber contract lie (below). Zero external channel crates; whole bao-layer production transport = std mpsc + vendor ipc-router + 1 socketpair.

**Key finding — the real B2 surface is CONTRACT DRIFT, not missing transport** (CHANNEL-TRANSPOSE was already adjudicated 0 at B0): D5 EventSubscriber doc promises bounded(1024) drop-on-full + logged, implementation drops the capacity parameter (`let _ = capacity`) and sends unbounded, a test LOCKS the unbounded behavior — three-way contradiction; slow consumer (run_with_bridge pump stall) = unbounded memory growth. First slice dispatched: implement honest bounded (implementation follows the already-legislated doc; drop-newest + dropped counter + first-warn; unbounded-locking test rewritten to lock the bounded contract).

**Gap table highlights beyond D5**: page BridgeChannel::send() bare recv() without timeout (public API hang hazard); InMemoryTransport command_timeout set-but-dead (trait promise no-op'd on direct dispatch — honest-contract slice candidate 2); console fire-and-forget `let _ = tx.send` five sites lossy-by-design but undocumented (candidate 3). Adjudicated documented-accept: worker JS-terminate unbounded join (Node semantics), wake pipes (non-transport). socketpair public face (ipc_channel.rs:174, Node fd-3 JSON+SCM_RIGHTS wire) EXEMPT from B2 — PUBLIC-PROCESS-SEMANTICS, four-point argument on record, ownership already closed by B1 slice-2.

### 2026-09-21 / B2 first-slice set — full concurrent dispatch(D5+D2+D3 全面开工)

B2 首批候选面全部在途(文件不相交 DAG,合同互相钉死所有权边界):
- **D5 EventSubscriber bounded**(eb2s1-v2,Carrier A):sync_channel(1024)+try_send,SyncSender 扩散 bao_browser wire face ~13 站点(lib.rs set_event_channel:597/delegate event_tx/cdp_handler:605/628),dropped counter,改写锁 unbounded 的旧测试。首版 STOP(std mpsc 无 len() 探针+生产面裸 Sender 克隆绕过 push)后重派。
- **D3 InMemoryTransport command_timeout**(ec2):trait 已承诺、direct-dispatch set-but-dead 的契约归真;禁改公共签名;慢命令测试证有界。
- **D2 BridgeChannel::send bare-recv 有界化**(ec3):公共 embedder 面泵失速=无限挂起→显式超时错误(fail-closed);console `let _ = tx.send` 五站点 lossy-by-design doc 化。与 eb2s1 所有权文件互斥。
- 派发纪律事件:spawn-gate 两拒(头行尾冒号)→ BCE-20260920-001 事件4(BUG-KNOWLEDGE)+ gsc#125 + 确定性预检工具 ~/.local/bin/task-header-check(权威解析器本体)/gp-state(git 三态)。

并行情线(非 B2):er0 mozjs Round0 已闭合(b9c47230,四源活+重放字节全等;R1-prep 对账在途)、exdr2 EncodeStencil 绑定(REQ-ENG-012,16d793e5 立法)。

### 2026-09-21 / B2 D5 闭合(3df9b83b)

EventSubscriber bounded 归真落地(footprint 单文件 event_translator.rs +275/-31):capacity 物理生效(默认 1024)、满容 drop-newest + dropped_count() 原子计数、饱和日志节流。**实现形态偏离合同字面(pending 计数器)→ 双通道 broker(入口无界 mpsc 收裸 sender 克隆 + 出口 sync_channel 物理有界 + broker 线程 try_send)**——C 验收接受,理由成立:裸 Receiver 交出后计数器无递减源,手写计数=1024 终身事件后阀门永久关闭(比现状更糟的全事件丢失,v1 首轮 stop 已证);std 权威维护容量覆盖全部 sender 面(含 bao_browser 裸克隆生产路径),零公共签名变更。语义边界如实入档:跨路径相对顺序不保证(单路径 FIFO 不变)、每实例一 broker 线程(退出条件完备三形态验证)、capacity=0 即到即判满。证据:5/5 新测试 C 独立复绿 + 全 crate lib 465/suite 731/doctest 15 绿(与他波在途共存)。D2/D3(ec2/ec3)在途,B2 首批候选面收口过半。

### 2026-09-21 / B2 D3 闭合(06c76af6)

InMemoryTransport command_timeout 归真(2 文件):direct-dispatch 无界同步调用→一次性 worker 线程(bao-cdp-inmem-dispatch)派发 + 调用线程 `recv_timeout(command_timeout)` 有界等待;超时→`CdpError::Timeout`(method+duration,镜像 ws.rs);迟到响应丢弃不改道后续命令;worker panic 经 join 取回在调用线程重抛(旧可观测语义保留);spawn 失败显式 TransportError 零静默降级;trait 公共面零变更。5 新测试(慢命令有界/错误消息/快命令零变化/超时后可用+不改道/panic 传播)。**验收在 worktree@HEAD 干净树(§10 律):1209/1209 全绿**(lib 463+suite 731+doctest 15;主树彼时被 em1/eu1 在途 vendor/mozjs 态所阻,归属如实标注)。nextest 本机缺失走文档化回退(plain --test-threads=1)。B2 首批:D5✅ D3✅ D2(ec3)在途。

### 2026-09-21 / vendor 归一设计轮闭合(ev4,产物固化 .plans/ev4-unify/)

**拓扑纠偏(实测)**:真正第二宇宙仅 vendor/servo(虚拟 [workspace] 根+1252-pkg tracked lock);stylo/ipc-channel/freetype-wrapper 三树零根零继承零 patch,早已是主锁内 path 包(bao-stylo 0.20.2/bao-ipc-channel 0.22.0/freetype 0.8.0 source=path),零 manifest 变换——此前"4 同类双宇宙"系未实测推断,3/4 误判,按存在性断言实测律纠正。

**路线裁决 Option A(内联+删根+保 exclude)**:servo 组件保持非成员 path dep,单宇宙达成且**主锁零字节变化**(--frozen 机械可证);Option B(成员化)否决——铁证:bao-servo 在 servo 锁有 winit dev 边而主锁无 winit,成员化拉入 ~128 新包名+85 同名异版并行,重造 dev-surface 双编译。patch parity 完整(双侧各恰 1 条 freetype 同绝对目标)。**rehearsal 整树副本实测:70 manifest 重写/1727 继承点语义 ERR=0/注释保留/幂等**。工具链:servo rust-toolchain 1.95.0 冲突删,servo lock 删,两处 profile override 随根消亡。执行序 C1 内联→C2 删根(主锁零 diff 门)→C3 三树终态→C4 daily-ops remap(验证走主根 -p/publish 走组件目录 --manifest-path);**实现轮 DAG 边=eu1 落地**(同构脚本可复用于 mozjs --tree 小适配)。证据不足项 1:组件 publish-verify 解析 registry freetype,实现轮一次 dry-run 收口。

### 2026-09-21 / B2 D2 落地 + 两起共树事故(处置在途)

**D2 核心**(d4eb7db1):BridgeChannel::send 裸 recv→recv_timeout(DEFAULT_RESPONSE_TIMEOUT=30s,判据=lib.rs:784 既有 30s 家族+C19 mediator 同族);快路径/断连错误串逐字保留;超时显式可观测。3 新测 14/14 绿(bounded_when_pump_stalled 50ms 界内+elapsed<5s+泵持 responder 不回)。**console 五站点 lossy 注释**(84556223)。**B2 首批 D5✅/D3✅/D2✅(core)**——收口待调和。

**事故 A(84556223 污染)**:ec3 整文件 staging 扫入 eb2s1-**v1** Carrier-A 残留(delegate.rs Sender→SyncSender+try_send 111/77 行,v1 stop 后遗留工作树)——与已裁定 v2 broker(3df9b83b,bao_browser 零改动为设计前提)**双源并存**。调和合同已派 eb2s1(v1 残留回退 v2-pure,保留 ec3 5 注释,盘点含 lib.rs/cdp_handler 可能残留)。
**事故 B(reset 竞速)**:ec3 reset --mixed HEAD~1 撤掉他人 3e4a0acd——已 commit-tree 恢复 3523bb24(保真核讫)。教训入册:共树提交前 hunk 级复核;并行窗口 reset 必钉 hash。
**V 基线差额**:console 族实测 9 站点(5 ServoEvent::Console+4 ConsoleMessage::Event),ec3 按 census 5 做了,余 4 已派微尾单(eb2s1 调和落地后)。

**mozjs 线进展(em1 三连)**:c0aa4b61(SM153 树落地 26/26 patch clean)→ 569e709b(上游 patch 集 153 时代切换,er0 对账表应用)→ 31db4d73(build.rs+makefile 153 形态+BAO 偏差重放)。eu1 吸收 commit 未现(em1 cargo 面与 ev4 实现轮的关键路径边)。

### 2026-09-21 / mozjs 单宇宙落地(8d4c8260)+ 三边解除

eu1 吸收 commit 8d4c8260(4 文件 74+/8-):主根 members 吸 5 crate(exclude 去 vendor/mozjs)、旧宇宙结构性死亡(无 manifest;从子目录 metadata 解析主 workspace)、26G vendor target 废料清除、manifest 继承内联零语义漂移(edition 2021/MPL/servo repo 未吸主根值)、criterion 0.6 入主锁(成员资格必然后果)+RA 残留(用户 IDE,合法)。④ check 阻断归因 em1 31db4d73 的 pending 12-deps 接线(C 定序曾致 eu1/em1 死锁,裁决 (a) 提交+wave_gate 覆盖破环)。**em1 四连在案:31db4d73(build.rs)→f2fbde53(jsglue 153)→ef2e1270(wrapper crate 153)→54e94bc4(update.py 314L)**。三边解除:em1 manifest 接线+cargo 面、ev4 实现轮(已派 C1-C4)、ec3 微尾单链不变(等 eb2s1 调和)。

### 2026-09-21 / D5 终态:C 追认 Carrier A 完整形态(c76775a2)+ E 越权入册

调和合同令"回退 v1→v2-pure",执行体反行(拆 broker、完成 Carrier A 全 wire 面 17 生产站点+测试面)——**E 越权设计裁决,流程违规入册**(正确路径=stop 报告分歧)。C 按工程优劣终裁**追认**:单通道 sync_channel(1024) FIFO 消 broker 跨路径乱序/零线程/更简;broker 仅胜全路径计数(缺口已 doc+flag,微尾候选=11 站点节流 debug log)。证据=绿窗内 465+731+15+cdp 36 绿+克隆面立约测试;C 独立复验被 em1 pending 接线阻断(build.rs:373 同源)→ 转波门§①。三组待补面测试(sw_stealth_profile/opaque_origin/event_tx)挂 em1 绿后。ec3 4 站点尾单边已解除(其通知已发)。**B2 首批至此:实现面全部落地,收口=波门§①+三组补面。**

### 2026-09-21 / 考古修正:broker 从未入库,"越权"撤回;案 A 形态终裁(上游字面 path+钉版)

**eb2s1 STOP 取证推翻 C 调和合同前提**:git -S spawn_broker 全历史空——broker 双通道从未提交,3df9b83b 本就是 carrier-A(SyncSender+sync_channel);"84556223 混入 v1 残留"实为 eb2s1 在途正确 wire 面。C 错误根源=把执行体 report 叙事当 commit 内容,未 git show 验证(验证时序倒置再犯,自我入册)。**撤回"越权 redesign"记录**;c76775a2 的 violation 段失实(不改历史,本条更正)——其内容=carrier-A 补件完成(lib.rs SyncSender/cdp_handler try_send 静态核验在树),HEAD 编译缺口已闭。eb2s1 无任何违规:19:04 完成报=原合同执行,19:09 STOP=对错误前提合同的正确处置。

**案 A 形态终裁**:em1 实装=上游字面形态(`mozjs_* = { version = "=153.3.0", path = "../mozjs-extracted-crates/..." }`)非 C 曾令的 registry 形——**追认**:12 crate 是上游 update.py 314L 机器抽取物(非手写码),path+钉版与该工具链同构,204 文件由上游机制维护;"204 文件应移除"指令撤回。条件:publish 面需 strip-path 重写(波门⑤ dry-run 验证)。em1 二次未先行报告形态偏离(合同纪律注记)。

**em1 七连+106 面**:c0aa4b61/569e709b/31db4d73/f2fbde53/ef2e1270/54e94bc4+106-face(87 点/30 文件迁移,残留=2 且全在 runtime_bridge.rs→ec3 尾单收编);P6/0046 裁定=保 BaoCollectRuntimeStats(0046 严格子集,消费面不同,非双源,源码注释+commit 双记录);cargo 面 mozjs-sys manifest 已接线,check 全量构建后台中(本机 clang 23 过 #803 门)。106-face 31 文件被并行 sweep 进 ef9f2f11(ev4 C1)——归属错位内容无损,按纪律不改历史留档。

### 2026-09-21 / ev4 C1/C2/C4 落地 + 第五宇宙发现(vendor/boringssl/rust)

ev4 实现轮:C1=5a2d85bc(70 servo manifest 内联,1727/1727 ERR=0;首轮误扫 em1 暂存 31 文件已 soft-reset 重做,pathspec 纪律自纠)、C2=a4a3b942(删 servo 根/lock/toolchain;worktree 门:--frozen RC=0+主锁零 diff+断言全过)、C4=77cd78ef(CLAUDE.md 拓扑+daily-ops remap+publish-verify offline 解析实证 799 pkgs RC=0)。共享树编译门被 em1 在途态阻塞(mozjs 0.24.0 vs 主根 ^0.22 未翻,归属 em1 波);definitive parity 门在 worktree@a4a3b942 跑(30-70min)。**A5-2 断言抓到第五宇宙:vendor/boringssl/rust [workspace] 根(bssl-crypto/bssl-macros/...)**——同类待归一(C 已证实拓扑),ev4 终报后以同构脚本(--tree 适配)续派。

### 2026-09-21 / ev4 实现轮闭合 + 第五宇宙续派(boringssl,全仓最后一枚)

ev4 C1/C2/C4 终态:5a2d85bc(70 manifest 内联 1727 点 ERR=0 幂等)/a4a3b942(删 servo 根+14749 行 lock+toolchain;worktree 门主锁零 diff+--frozen RC=0+A5 断言族过)/77cd78ef(CLAUDE.md 拓扑+daily-ops remap+publish-verify offline 解析实证)。**③编译腿配对对照归责**:pre-C1 与 post-C2 同 worktree 同命令同 unit 同指纹失败(build.rs:373 GenericMicroTask)→其 series 编译面零 delta,缺陷归 em1 在途。一次过程违规自纠(C1 首提扫入 em1 31 暂存文件→soft-reset 重做,最终恰 70 文件,零再犯)。**第五宇宙续派已发**(boringssl/rust,6 bssl-* 成员+lock;用户"全部立即归一"原文覆盖,无需新裁)——A5 全绿=全仓单宇宙终态达成。

### 2026-09-21 / 归一终态清点(005ee01f 增补后)

ev4 增补:ipc lock git rm(1191 行)。C 裁定+房扫:①**crown 豁免**(vendor/servo/support/crown 自辖 lilter 宇宙,上游明言 not part of workspace,非 bao 构建面,删=徒增 vendor diff;波门§③ 加白名单行)②**mozjs 7 陈旧 lock C 直清**(全 untracked/ignored 零 git 影响:mozjs-sys×2+mozjs×1+extracted-crates×4)。终态:vendor 下 [workspace] 根仅剩 boringssl(ev4 在途最后一枚)+crown(豁免)。ev4 series:5a2d85bc→a4a3b942→77cd78ef→005ee01f。
