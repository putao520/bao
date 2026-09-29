# Release Notes — vNext (draft material for the next tag)

> Working material for the upcoming tag closure. External-readable summary of
> what changed since `0.1.0-alpha.1`/current published line, merged by theme
> (internal program labels omitted). Facts only; each claim is test- or
> evidence-backed in the repository.

## Runtime stability — three crash classes eradicated at the root

- **Node-realm timers no longer kill the hosting thread.** A timer armed in
  the privileged Node Realm previously dispatched through a DOM-only helper
  and panicked the whole ScriptThread, silently taking every page on that
  thread with it (symptom: long-lived pages losing their Node Realm). The
  dispatch now discriminates realms; node-realm timers are usable for the
  first time in browser-runtime processes.
- **GC compaction move-staleness class closed (two variants).** The first
  compacting collection exposed that native wrappers holding raw GC pointer
  slots never participated in relocation updates: the font-ready promise
  face (unreachable-wrapper variant) and the DOM reflector back-pointer
  (reachable-wrapper variant) each read pre-move addresses off freed
  chunks. Both fixed at the storage/trace layer; the standing discipline —
  native structs holding GC pointer slots must never have empty traces —
  is now enforced by regression tests.
- **Teardown is bounded.** `ServoInner::drop`'s join spin had no timeout:
  any dead or wedged servo thread hung runtime teardown forever (a >13 min
  live repro was forensically attributed to the panic classes above). The
  drop now abandons the join after 15 s, leaks the wedged thread with a
  loud log record, and lets teardown complete — liveness over perfect
  reclamation.

## Page lifecycle — an explicit, guarded state machine

`PageState` is no longer an enum with scattered bare writes: a
SPEC-mirrored transition table (6 states, 10 legal edges) with one
centralized guarded write point; illegal transitions are refused and
logged. Initial-URL pages now leave `Created` correctly, and pool idle
reclamation materializes the spec's `Idle` state on the page itself.
Three test tiers pin it: a 48-cell exhaustive matrix, real-runtime edge
tests, and shutdown-order tests.

## Engine — opt-in persistent stencil cache

Cross-process bytecode caching for heavy scripts, layered under the
in-memory stencil cache: `BAO_XDR_CACHE_DIR` opts in (unset = the layer is
fully disabled, zero disk IO). Content-addressed entries with full key
verification, build-id invalidation, torn-write/corruption fail-closed as
misses with self-healing re-stores, and atomic temp+rename writes. A
decode-fingerprint gate proves decoded stencils instantiate identically to
fresh compilation across realms; two-process cold/warm measurement shows
**3.34×** on a 2000-function blob (cold compile+store vs warm disk decode).

## Multi-page stability & soak

- Page-stress gate met: **N=100 pages / 300 s with crash/hang/guard = 0**,
  zero stranding, fd recovery; an N=500 capacity probe also ran crash-free.
- A mixed-workload soak scenario (7 op classes interleaved across long-lived
  pages with page churn) runs the full fail-closed contract: class
  transitions and async overlap are mechanically counted, every class
  verifies its own results, and the 2-minute run completes with zero
  failures.
- Long-run memory: steady-state soak growth **−83%** after the realm-discard
  shrink hook. The residual per-realm chunk retention is documented as a
  known limitation on the engine's realm-discard face (upstream issue
  candidate).

## API surface, errors, and release engineering

- **API surface declaration gate**: zero undeclared public items
  (`docs/api.md` three-tier declaration: Stable / Experimental / Internal).
- **SemVer gate** and **consumer gates** (registry-path 5-crate install,
  drift check) plus a nightly toolchain bump probe; one 49-crate release
  closure already executed as precedent. `bao-core` is published (0.3.1).
- **Error source chains closed**: public error types preserve full
  `std::error::Error::source()` chains (connect errors reach their io root);
  the string-face ledger is documented.
- **Env contract inventory**: 154 `BUN_*`/`BAO_*` keys documented with an
  accessor↔doc lock test.
- **CLI contract snapshots**: 18 golden faces + exit-code matrix.

## Compatibility & platform

- The four compatibility inventories (Node / Bun / CDP / Web) are published
  under `compat/` with the `bao compat` report command — inventories only;
  pass rates stay TBD until actually measured.
- **Windows x86_64: Supported** (cross-build from Linux + real-machine
  battery); the three-tier platform matrix (Supported / Experimental /
  plan) is legislated in `docs/platform-support.md`.

## Examples & docs

- Runnable examples grew **4 → 8** (multi-page pool, graceful shutdown,
  engine-level timeouts, error-chain handling added; existing four
  repaired for API drift) — all locked against API drift by a compile
  carrier test.
- SIGTERM-respawn semantics pinned: children spawned under a host with
  ignored/blocked signals still terminate on `child.kill('SIGTERM')`
  (sub-millisecond convergence, regression-tested under a hostile
  spawner-state precondition).
