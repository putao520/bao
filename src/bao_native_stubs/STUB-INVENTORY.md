# bao_native_stubs inventory

> SSOT for product-path stub eradication.  
> Policy: **no new noop stubs to silence dual-def**. Full-real preferred.  
> Residual NoopBlocker = P0 until real owner lands.  
> Updated: 2026-07-25

## Force-link map (product)

| Crate | Hard dep on `bao_native_stubs`? | `force_link` / `__force_link_entry` anchor |
|-------|----------------------------------|-------------------------------------------|
| **`bao` (public package)** | **No (feature `native-stubs`, non-default)** | **None (default)** |
| `bun_runtime` (`bao_runtime`) | Yes (residual — RealImpl still live here) | `BAO_NATIVE_STUBS_ANCHOR` → `__force_link_entry` |
| `bao_browser` | Yes (residual transitive product path) | `BAO_NATIVE_STUBS_ANCHOR` |
| `bao_engine` | **dev-only** | test anchor only |
| `bun_core` | **dev-only** | test-only `force_link()` |
| `bao_uloop` | **dev-only** | — |
| `bao_workflow_host` | **No** (by design — dual-def free) | — |

**Default product (`bao` without features):** does **not** directly depend on or force-link `bao_native_stubs`.  
**Residual:** product still **transitively** links stubs via `bun_runtime` / `bao_browser` until remaining RealImpl + dispatch noops move to owners (P0 table below).

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
| `bun_cpu_features` | No | `bun_crash_handler::CPUFeatures` (caller) | via runtime | Returns u64 flags; crash_handler declares `extern` u8 — ABI drift residual |
| `is_executable_file` | No | `bun_sys` | via runtime | `stat` + `S_IXUSR` |
| `bun_restore_stdio` | Partial | `bun_core::output` | via runtime | Flush stdout/stderr only |
| `WTF__DumpStackTrace` | Partial | `bun_crash_handler` | via runtime | Local `Backtrace::capture`; crash_handler also declares different ABI `(ptr, count)` |
| `Bun__StackCheck__initialize` | Approx | `bun_core::util::StackCheck` | via runtime | Returns 8MiB constant |
| `Bun__StackCheck__getMaxStack` | Approx | `bun_core::util::StackCheck` | via runtime | `pthread_getattr_np` stack end |
| `Bun__registerSignalsForForwarding` | Partial | `spawn_sys` | via runtime | Stores PID only |
| `Bun__unregisterSignalsForForwarding` | Partial | `spawn_sys` | via runtime | Clears PID |
| `Bun__sendPendingSignalIfNecessary` | Partial | `spawn_sys` | via runtime | `kill(SIGTERM)` if PID set |
| `Bun__currentSyncPID` | Data | `spawn_sys` | via runtime | `AtomicI64` static |
| `on_before_reload_process_linux` | Partial | `bun_core::util` | via runtime | `sync()` before exec |
| `BunString__fromBytes` | Simplified | `bun_core::string` | via runtime | UTF-8 lossy Box; not full WTFString |
| `Bun__WTFStringImpl__destroy` | Simplified | `bun_core::string` | via runtime | `Box::from_raw` free |
| `Bun__Node__UseSystemCA` | Data | TLS / root_certs | via force_c_lib | `static mut bool = true` |
| `BUN__warn__extra_ca_load_failed` | Functional | TLS / root_certs | via force_c_lib | eprintln warning |
| `bun_ssl_ctx_cache_on_free` | Safe empty | BoringSSL EX_free | via force_c_lib | Cache not wired; empty free is correct until SSLContextCache lands |
| `__force_link_entry` | Meta | this crate | product residual | Entry for `#[used]` anchors |

### RealImpl — **moved out this wave** (deleted from stubs)

| Symbol | New owner | Notes |
|--------|-----------|-------|
| `sys_preadv2` | `bun_sys` | Real `libc::preadv2` (was return `-1` NoopBlocker) |
| `sys_pwritev2` | `bun_sys` | Real `libc::pwritev2` (was undeclared/undefined) |
| `__bun_get_vm_ctx` | `bun_runtime::dispatch` | Real Mini/Js ctx (was Mini+null owner) |
| `__bun_dns_prefetch` | `bun_runtime::dispatch` | Owner hook; non-blocking warm path |
| `WTF__numberOfProcessorCores` | `bun_core` | Real `available_parallelism` (was hard-coded `1`) |

### NoopBlocker — **P0 residual** (need real owner tasks)

| Symbol | Owner task | Product force? | Why P0 |
|--------|------------|----------------|--------|
| `URL__getHref` | `bun_url` pure-Rust / drop FFI | via runtime | Identity / dead-tag; bun_url already has pure parse for some paths |
| `URL__getHrefJoin` | `bun_url` | via runtime | Returns dead |
| `URL__fromString` | `bun_url` | via runtime | Always `None` |
| `URL__pathname/protocol/hostname/hash/host/password/username/search/fragmentIdentifier` | `bun_url` | via runtime | Dead strings |
| `URL__getFileURLString` / `URL__pathFromFileURL` | `bun_url` | via runtime | Dead |
| `URL__port` | `bun_url` | via runtime | Always 0 |
| `URL__deinit` | `bun_url` | via runtime | Empty |
| `WTF__parseES5Date` | `bun_core::wtf` + real parser | via runtime | Always NaN |
| `WTF__parseDouble` | `bun_core::fmt` | via runtime | Always NaN |
| `WTF__dtoa` | `bun_core::fmt` | via runtime | Length 0 |
| `__bun_regex_compile` / `__bun_regex_matches` / `__bun_regex_drop` | `bun_install_types` + regex crate | via runtime | Always fail → exact-string fallback only |
| `Bun__linux_trace_init` / `Bun__linux_trace_emit` | `bun_perf` or delete call sites | via runtime | Always false / empty |
| `WTF__releaseFastMallocFreeMemoryForThisThread` | allocator tier | via runtime | Empty |

### Dispatch `link_noop_*` (closed-set; product needs defs)

Generated by `bun_io::link_noop_BufferedReaderParentLink!` / `bun_spawn::link_noop_ProcessExit!`.  
Symbols: `__bun_dispatch__BufferedReaderParentLink__<Variant>__*` / `__bun_dispatch__ProcessExit__<Variant>__*`.

| Variant group | Class | Real owner if any | Notes |
|---------------|-------|-------------------|-------|
| LifecycleScript / SecurityScan | **Dead (removed)** | `bun_install` `link_impl_*` | Dual-def if re-listed |
| FilterRunHandle / MultiRun* / TestParallelWorker* | NoopBlocker | `bun_cli` only | CLI not on product graph → product keeps noop |
| Subprocess / Shell / ChromeProcess / HostProcess / Cron* | **P0 NoopBlocker** | `bao_runtime` / shell / browser process | Product uses spawn paths; noops swallow exits |
| SubprocessPipeReader / Shell* / FileReader / FileResponseStream / Terminal / Cron* | **P0 NoopBlocker** | runtime / shell / http | Pipe parent callbacks silent |

### Dead (must not reintroduce)

| Symbol / pattern | Why dead |
|------------------|----------|
| `uws_*` / `us_socket_*` Rust stubs | Real C++ in `libuwsockets.a` (`bun_uws_sys`) |
| `us_poll_*` / `us_create_poll` Rust stubs | `libusockets.a` |
| `us_loop_*` / `uws_get_loop` stubs | Real in `bao_uloop` |
| BoringSSL `SSL_*` Rust stubs | `bun_boringssl_sys` |
| `mi_*` / `highway_*` / `ZSTD_*` / Brotli Rust stubs | Respective sys crates / pure Rust |
| `__bun_crash_handler_out_of_memory` stub | Real `!` in `bun_crash_handler` |
| `Bun__lock__size` / `Bun__isEpollPwait2SupportedOnLinuxKernel` stubs | `bun_threading` / `bun_analytics` |
| `Bun__JSC_onBeforeWait` / `Bun__panic` / `sys_epoll_pwait2` stubs | `bun_uws_sys` / `bun_platform` |
| link_noop LifecycleScript / SecurityScan | Real `link_impl` in `bun_install` |

---

## Eradication backlog (ordered)

1. **P0** — Real `ProcessExit` + `BufferedReaderParentLink` for Subprocess/Shell (product spawn).
2. **P0** — `bun_url`: drop dead FFI stubs; finish pure-Rust WHATWG surface (href/file URL).
3. **P0** — Move remaining RealImpl out of this crate (`posix_spawn_bun`, strings, signals, stack check) → owner crates; then **drop hard dep** from `bun_runtime` / `bao_browser`.
4. **P1** — Real WTF numeric parsers or pure-Rust call-site replacements.
5. **P1** — Regex for `PnpmMatcher` via `regex` crate, not always-None.
6. **P1** — Remove CLI-only noops from product link once closed-set can split or CLI always provides impls.

---

## Dual-def rule (iron)

On full product path (`bao` → runtime → `bun_install` + this crate):

- **Never** `link_noop` a variant that already has `link_impl` in a co-linked crate.
- **Never** stub a `#[no_mangle]` already exported by C lib or owner crate.
- Prefer **delete stub** over weak-link / rename hacks.
