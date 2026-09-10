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
- **Canvas face** (census R53 member 3): per-canvas noise config, deletes `canvas_noise.rs` globals — `CanvasMsg` path carries no webview identity today; heaviest plumbing, deliberately staged out.
- **Fallback bucket last-write-wins** (identity-less fetches only, post-SW-stamp = SW script updates + misc infra): process-global still written per install; pre-R53 behavior preserved by design; documented residual, not a page-visible surface.
- `fetch()` thread-local profile face (proposal member 5, `TL_STEALTH_PROFILE` last-install-wins on shared ScriptThread) — NOT in this wave's scope; same-seam follow-up candidate.
- Webviewless resource handler (member 4) — untouched, constant verdict, per proposal.

## 9. Next single action

**R53-A net face landed (⑥ above). Next single action from §8: R53-A phase 2 canvas face — per-canvas `CanvasNoiseConfig` stamped at canvas creation, read at the `GetImageData` choke point, deleting the `canvas_noise.rs` globals** (identity must be threaded onto the `CanvasMsg` command path first). Then the e8541037c4 PathBuffer pool sweep (177 call sites).

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
