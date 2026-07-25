# bao_native_stubs inventory

> SSOT for product-path stub eradication.  
> Policy: **no new noop stubs to silence dual-def**. Full-real preferred.  
> Residual NoopBlocker = P0 until real owner lands.  
> Updated: 2026-07-25 (next wave: `linux_trace` + `FastMalloc`)

## Product policy (iron · always-on)

| Rule | Meaning | Forbidden |
|------|---------|-----------|
| **No env switch for capability** | Do **not** add `BAO_*` / `FROG_*` / new `BUN_*` env to select stub vs real, or to hide missing symbols | “set env → full product”; env-gated noop |
| **No Cargo feature to hide stubs** | Do **not** reintroduce `native-stubs` / capability features that strip half the host surface | `cfg(feature=…)` product capability split; “turn off feature = green” |
| **Product never links stubs** | Public `bao` has **no** dep / optional dep / `force_link` on `bao_native_stubs` | product hard-dep, `__force_link_entry` in product binary, dual-def with stub noops |
| **Unique real `#[no_mangle]`** | Each exported symbol has **one** real body (owner crate); residual noops only until code E lands, then delete from stubs | dual-def; silent empty body as “done” |

Platform differences use **`cfg(target_os)` compile-time backends** (Linux / macOS / Windows), not runtime env/feature capability gates.

## Force-link map (product)

| Crate | Hard dep on `bao_native_stubs`? | `force_link` / `__force_link_entry` anchor |
|-------|----------------------------------|-------------------------------------------|
| **`bao` (public package)** | **No (never: no feature, no optional dep)** | **None** |
| `bun_runtime` (`bao_runtime`) | **dev-only** (tests may force_link) | product: `product_process_exit` + `product_buffered_reader` + `product_native_symbols` (no PE/BR link_noop residual) |
| `bao_browser` | **No** (dropped hard dep) | **None** |
| `bao_engine` | **dev-only** | test anchor only |
| `bun_core` | **dev-only** | test-only `force_link()` |
| `bao_uloop` | **dev-only** | — |
| `bao_workflow_host` | **No** (by design — dual-def free) | — |

**Default product (`bao`):** does **not** depend on, feature-gate, or force-link `bao_native_stubs` (no optional dep).  

**PE/BR closed-set residual:** **0** — real owners in `bun_runtime::product_process_exit` + `product_buffered_reader` (P1/P2); `product_dispatch_residual` deleted (P3).  

**Next-wave residual:** `FastMalloc` ✅ + `linux_trace` ✅ both residual=0 (Win/macOS/Linux).

---

## Classification legend

| Class | Meaning |
|-------|---------|
| **RealImpl** | Functional body (not silent noop). Temporary home OK only until moved to named owner. |
| **NoopBlocker** | Silent/trivial noop; product or closed-set dispatch needs a definition. **P0 residual.** |
| **Dead** | Removed / never linked / comment-only / superseded by C lib or other crate. Do not reintroduce. |

---

## `#[no_mangle]` / exported symbols

### RealImpl (keep; prefer migrate to owner)

| Symbol | Noop? | Real owner (target) | Product force_link? | Notes |
|--------|-------|---------------------|---------------------|-------|
| `posix_spawn_bun` | No | `bun_core::spawn_ffi` / `spawn_sys` | via runtime | Real `posix_spawnp` path; only def is here |
| `ares_inet_pton` | No | `bun_cares_sys` / `bun_core` | via runtime | Pure Rust IPv4/6 parse |
| `bun_cpu_features` | No | `bun_crash_handler::CPUFeatures` (caller) | via runtime | Returns u64 flags |
| `is_executable_file` | No | `bun_sys` | via runtime | `stat` + `S_IXUSR` |
| `bun_restore_stdio` | Partial | `bun_core::output` | via runtime | Flush stdout/stderr only |
| `WTF__DumpStackTrace` | Partial | `bun_crash_handler` | via runtime | Local `Backtrace::capture` |
| `Bun__StackCheck__initialize` | Approx | `bun_core::util::StackCheck` | via runtime | Returns 8MiB constant |
| `Bun__StackCheck__getMaxStack` | Approx | `bun_core::util::StackCheck` | via runtime | `pthread_getattr_np` stack end |
| `Bun__registerSignalsForForwarding` | Partial | `spawn_sys` | via runtime | Stores PID only |
| `Bun__unregisterSignalsForForwarding` | Partial | `spawn_sys` | via runtime | Clears PID |
| `Bun__sendPendingSignalIfNecessary` | Partial | `spawn_sys` | via runtime | `kill(SIGTERM)` if PID set |
| `Bun__currentSyncPID` | Data | `spawn_sys` | via runtime | `AtomicI64` static |
| `on_before_reload_process_linux` | Partial | `bun_core::util` | via runtime | `sync()` before exec |
| `BunString__fromBytes` | Simplified | `bun_core::string` | via runtime | UTF-8 lossy Box |
| `Bun__WTFStringImpl__destroy` | Simplified | `bun_core::string` | via runtime | `Box::from_raw` free |
| `Bun__Node__UseSystemCA` | Data | TLS / root_certs | via force_c_lib | `static mut bool = true` |
| `BUN__warn__extra_ca_load_failed` | Functional | TLS / root_certs | via force_c_lib | eprintln warning |
| `bun_ssl_ctx_cache_on_free` | Safe empty | BoringSSL EX_free | via force_c_lib | Empty free until SSLContextCache lands |
| `__force_link_entry` | Meta | this crate | product residual | Entry for `#[used]` anchors |

### RealImpl — **moved out this wave** (deleted from stubs)

| Symbol | New owner | Notes |
|--------|-----------|-------|
| `sys_preadv2` | `bun_sys` | Real `libc::preadv2` (was return `-1` NoopBlocker) |
| `sys_pwritev2` | `bun_sys` | Real `libc::pwritev2` |
| `__bun_get_vm_ctx` | `bun_runtime::dispatch` | Real Mini/Js ctx (was Mini+null) |
| `__bun_dns_prefetch` | `bun_runtime::dispatch` | Async resolve warm path |
| `WTF__numberOfProcessorCores` | `bun_core::util` | Real `sysconf(_SC_NPROCESSORS_ONLN)` |
| `URL__*` (getters + fromString/deinit/file/originLength/getHref*) | `bun_url::whatwg` pure + `product_native_symbols` | Pure WHATWG parse; noops deleted |
| `WTF__parseES5Date` | `bun_core::wtf` + product no_mangle | Pure ES5 ISO-8601 → ms |
| `WTF__parseDouble` | `bun_core::fmt::parse_double_raw` + product no_mangle | Pure partial JS double |
| `WTF__dtoa` | `bun_core::fmt::dtoa_into` + product no_mangle | Pure f64→ASCII |
| `__bun_regex_compile/matches/drop` | `product_native_symbols` (`regex` crate) | Real compile/match; Rust ABI |
| `WTF__releaseFastMallocFreeMemoryForThisThread` | `bun_alloc` | Real `mi_collect(false)` via bun_mimalloc_sys; empty stubs deleted |

### NoopBlocker — **P0 residual** (need real owner tasks)

| Symbol / group | Owner task | Product force? | Status | Why P0 |
|----------------|------------|----------------|--------|--------|

### Dead (must not reintroduce)

| Symbol / pattern | Why dead |
|------------------|----------|
| `uws_*` / `us_socket_*` Rust stubs | Real C++ in `libuwsockets.a` |
| `us_poll_*` / `us_create_poll` Rust stubs | `libusockets.a` |
| `us_loop_*` / `uws_get_loop` stubs | Real in `bao_uloop` |
| BoringSSL `SSL_*` Rust stubs | `bun_boringssl_sys` |
| `mi_*` / `highway_*` / `ZSTD_*` / Brotli Rust stubs | Sys crates / pure Rust |
| `__bun_crash_handler_out_of_memory` stub | Real `!` in `bun_crash_handler` |
| `Bun__lock__size` / epoll_pwait2 kernel check stubs | `bun_threading` / `bun_analytics` |
| link_noop LifecycleScript / SecurityScan | Real `link_impl` in `bun_install` |
| **ProcessExit** product residual `link_noop_*` (Subprocess, Shell, Cron*, ChromeProcess, HostProcess, FilterRun*, MultiRun*, TestParallelWorker) | Real `link_impl` in `bun_runtime::product_process_exit` (P1); residual module deleted (P3). LifecycleScript/SecurityScan/SyncWindows already owned. **residual=0** |
| **BufferedReaderParentLink** product residual `link_noop_*` (SubprocessPipeReader, Shell*, FileReader, FileResponseStream, Terminal, Cron*, FilterRun*, MultiRun*, TestParallelWorkerPipe) | Real `impl_buffered_reader_parent!` in `bun_runtime::product_buffered_reader` (P2); residual module deleted (P3). LifecycleScript/SecurityScan already owned. **residual=0** |
| `product_dispatch_residual` module | Deleted P3 after PE/BR true owners landed |
| `sys_preadv2` return -1 stub | Real in `bun_sys` |
| `__bun_get_vm_ctx` Mini+null stub | Real in `bun_runtime::dispatch` |
| `__bun_dns_prefetch` empty stub | Real in `bun_runtime::dispatch` |
| `WTF__numberOfProcessorCores` return 1 stub | Real in `bun_core::util` |
| `URL__*` dead/identity/None stubs | Pure `bun_url::whatwg` + product RealImpl |
| `WTF__parseES5Date` / `parseDouble` / `dtoa` NaN/0 stubs | Pure `bun_core` + product RealImpl |
| `__bun_regex_*` always-fail stubs | `regex` crate RealImpl in product |
| `WTF__releaseFastMallocFreeMemoryForThisThread` empty stub | Real in `bun_alloc` (`mi_collect(false)`) |

---

## Eradication backlog (ordered)

1. ~~**P0** — Real `ProcessExit` + `BufferedReaderParentLink` for Subprocess/Shell (product spawn).~~ **DONE** (P1/P2 real owners + P3 residual delete; residual=0).
2. ~~**P0** — `bun_url`: drop dead FFI stubs; finish pure-Rust WHATWG surface.~~ **DONE** (`whatwg` pure + product `URL__*` RealImpl; stub noops deleted).
3. **P0** — Move remaining RealImpl out of this crate → drop hard dep from `bun_runtime` (dev-only force_link only).
4. ~~**P1** — Real WTF numeric parsers or pure-Rust call-site replacements.~~ **DONE** (`bun_core::{fmt,wtf}` pure + product no_mangle).
5. ~~**P1** — Regex for `PnpmMatcher` via `regex` crate.~~ **DONE** (`product_native_symbols` RealImpl).
6. **P1** — CLI crates (`src/cli`) must consume product PE/BR owners (single `link_impl` site) if co-linked — do not reintroduce product residual noops.
7. ~~**P1 / next wave** — `Bun__linux_trace_*`.~~ **DONE** (`bao_runtime::linux_trace` a74be633).
8. ~~**P1 / next wave** — `WTF__releaseFastMallocFreeMemoryForThisThread`.~~ **DONE** (`bun_alloc` `mi_collect(false)`; empty stubs deleted).

---

## Next wave — `linux_trace` + `FastMalloc` (Win / macOS / Linux)

> Scope: residual NoopBlocker → unique real owners on product path.  
> **Policy (iron):** no env switch for capability · no Cargo feature to hide stubs · **product never links `bao_native_stubs`** · platform via `cfg(target_os)` only.

### Status snapshot

| Item | Residual | Notes |
|------|----------|-------|
| **FastMalloc** `WTF__releaseFastMallocFreeMemoryForThisThread` | ✅ **0** | Real owner `bun_alloc` → `mi_collect(false)` (portable mimalloc); stubs + product empty deleted |
| **linux_trace** `Bun__linux_trace_*` | ✅ **0** | `bao_runtime::linux_trace` RealImpl Win/macOS/Linux |
| **Product never links stubs** | ✅ | `bao` no dep / no force_link |
| **No env / no capability feature** | ✅ policy | Permanent; code review gate |

### Goals

| Symbol group | Real behavior (target) | Owner | After code E |
|--------------|------------------------|-------|--------------|
| `Bun__linux_trace_init` / `emit` / `close` | Linux ftrace marker when tracefs usable; honest disable on other OS | product single site (`product_native_symbols` or `bun_perf` / `bun_core::perf`) | delete stub defs; residual=0 |
| `WTF__releaseFastMallocFreeMemoryForThisThread` | Thread allocator free-list / cache release | ✅ `bun_alloc` | done |

### ABI note (`linux_trace` — must unify in code E)

Call sites disagree today — code E **must pick one ABI** and fix all declarers + defs:

| Surface | Current decl shape (observed) | Notes |
|---------|-------------------------------|-------|
| `bun_core::perf::sys` / `bun_perf` | `init() -> c_int`; `emit(name: *const c_char, duration_ns: i64) -> c_int`; `close()` | Canonical Bun ftrace (linux_perf_tracing.cpp era) |
| `product_native_symbols` / `bao_native_stubs` residual | `init() -> bool`; `emit(id, name, cat, phase, ts, pid, tid, extra)` void | **Mismatch** — residual shape; **must not** ship dual ABIs |

**DoD:** one ABI SSOT (prefer `bun_core::perf::sys`), one `#[no_mangle]` def, zero stub def on product link.

### Compatibility matrix (target backends)

Compile-time `cfg(target_os)` only — **not** env/feature capability gates.

#### A. `Bun__linux_trace_*` (ftrace-class host tracing) — 🔶 residual open

| OS | Backend | `init` | `emit` | `close` | Product always-on? |
|----|---------|--------|--------|---------|-------------------|
| **Linux** | **tracefs / ftrace** — open `trace_marker` (debugfs fallback); write duration events matching Bun `C\|…` format | Probe marker path; success only if writable | Write event line; no-op if init failed | Close FD | **Yes** — symbol always linked; probe may report unsupported |
| **Android** | Same as Linux when tracefs present | Same | Same | Same | **Yes** |
| **macOS** | **Not ftrace** — host spans use **os_signpost / os_log** via `bun_perf::Darwin` (separate path) | `linux_trace_init` → honest **unsupported** (0/false); do not pretend ftrace | no-op | no-op | **Yes** — symbols present; Darwin is real macOS tracer |
| **Windows** | No ftrace; `bun_perf` **Disabled** backend | honest unsupported | no-op | no-op | **Yes** — symbols present; no ETW fake unless dedicated later owner |

| OS | Forbidden |
|----|-----------|
| All | Env/feature to swap stub↔real; product force_link of stub `linux_trace_*`; dual-def stub+product |
| macOS/Win | Deleting symbols while call sites remain; silent dual empty bodies as “done” |

#### B. `WTF__releaseFastMallocFreeMemoryForThisThread` — ✅ residual=0

| OS | Backend (shipped) | Behavior | Product always-on? |
|----|-------------------|----------|-------------------|
| **Linux** | **mimalloc** `mi_collect(false)` via `bun_mimalloc_sys` | Release this thread’s free memory toward OS/arena | **Yes** |
| **macOS** | Same (`mi_collect` portable) | Same | **Yes** |
| **Windows** | Same (`mi_collect` portable) | Same | **Yes** |

| OS | Forbidden |
|----|-----------|
| All | Reintroduce empty `{}` in stubs or `product_native_symbols` (dual-def / fake complete) |

### Residual table (this wave)

| ID | Item | Residual | Code E | Notes |
|----|------|----------|--------|-------|
| **NW-LT** | **NW-FM** | `WTF__releaseFastMallocFreeMemoryForThisThread` | ✅ **0** | closed | `bun_alloc` `mi_collect(false)` |
| **NW-STUB-DEL-LT** | Delete `linux_trace` stub defs after unique product owner | ✅ **0** | closed | stubs deleted with RealImpl |
| **NW-PRODUCT-LINK** | Product never links `bao_native_stubs` | ✅ | closed | `bao` no dep / no force_link |
| **NW-POLICY** | No env switch · no feature hide · always-on | ✅ policy | permanent | Enforce on code review |

> When NW-LT + NW-STUB-DEL-LT close: next-wave residual=0 for these symbols. **Until then do not** claim full stub eradication for `linux_trace`.

### Acceptance checklist (remaining = linux_trace)

1. Single ABI + single `#[no_mangle]` for `linux_trace_*` matching `bun_core::perf` callers.
2. Linux: real tracefs probe + write; macOS/Win: honest unsupported (Darwin/Disabled for actual spans).
3. `rg` product link graph: **0** `bao_native_stubs` in default `bao` deps; **0** dual-def.
4. **No** new env; **no** capability feature; platform via `cfg(target_os)`.
5. Flip NW-LT residual → 0 in this inventory + audit-26.

---

## Dual-def rule (iron)

On full product path (`bao` → runtime → `bun_install` + this crate):

- **Never** `link_noop` a variant that already has `link_impl` in a co-linked crate.
- **Never** stub a `#[no_mangle]` already exported by C lib or owner crate.
- Prefer **delete stub** over weak-link / rename hacks.
- **Do not delete** product-required closed-set noops until real `link_impl` exists (frog-tools link fails).
- **Product does not link this crate** — residual noops here are for **dev/test force_link only** until deleted; product owner must land first.
