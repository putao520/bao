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
None yet.

### BCE residual
Unknown until B0 census.

### Simplification ledger
None yet.

## 9. Next single action

**Execute #32 Phase B0 census and, in the same run, complete one real process/process-local/IPC -> thread/runtime/channel transposition slice with tests.**

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
