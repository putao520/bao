# bao_native_stubs inventory

> SSOT for product-path stub eradication.  
> Policy: **no new noop stubs to silence dual-def**. Full-real preferred.  
> Residual NoopBlocker = P0 until real owner lands.  
> Updated: 2026-07-25

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

**PE/BR closed-set residual:** **0** — real owners in `bun_runtime::product_process_exit` + `product_buffered_reader` (P1/P2); `product_dispatch_residual` deleted (P3). Other NoopBlocker `#[no_mangle]` may still live in this crate until owner tasks finish.

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

### NoopBlocker — **P0 residual** (need real owner tasks)

| Symbol / group | Owner task | Product force? | Why P0 |
|----------------|------------|----------------|--------|
| `URL__*` (all getters + fromString/deinit/file) | `bun_url` pure-Rust / drop FFI | via runtime | Dead/identity/None — bun_url still declares extern |
| `WTF__parseES5Date` / `parseDouble` / `dtoa` | `bun_core::wtf` / `fmt` | via runtime | Always NaN / length 0 |
| `__bun_regex_compile/matches/drop` | regex crate + install_types | via runtime | Always fail → exact-string fallback |
| `Bun__linux_trace_*` | `bun_perf` or drop call sites | via runtime | Always false / empty |
| `WTF__releaseFastMallocFreeMemoryForThisThread` | allocator tier | via runtime | Empty |

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

---

## Eradication backlog (ordered)

1. ~~**P0** — Real `ProcessExit` + `BufferedReaderParentLink` for Subprocess/Shell (product spawn).~~ **DONE** (P1/P2 real owners + P3 residual delete; residual=0).
2. **P0** — `bun_url`: drop dead FFI stubs; finish pure-Rust WHATWG surface.
3. **P0** — Move remaining RealImpl out of this crate → drop hard dep from `bun_runtime`.
4. **P1** — Real WTF numeric parsers or pure-Rust call-site replacements.
5. **P1** — Regex for `PnpmMatcher` via `regex` crate.
6. **P1** — CLI crates (`src/cli`) must consume product PE/BR owners (single `link_impl` site) if co-linked — do not reintroduce product residual noops.

---

## Dual-def rule (iron)

On full product path (`bao` → runtime → `bun_install` + this crate):

- **Never** `link_noop` a variant that already has `link_impl` in a co-linked crate.
- **Never** stub a `#[no_mangle]` already exported by C lib or owner crate.
- Prefer **delete stub** over weak-link / rename hacks.
- **Do not delete** product-required closed-set noops until real `link_impl` exists (frog-tools link fails).
