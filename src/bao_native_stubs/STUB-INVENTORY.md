# bao_native_stubs inventory

> SSOT for product-path stub eradication.  
> Policy: **no new noop stubs to silence dual-def / undefined**. Full-real preferred.  
> Residual undefined after noop deletion = owner tasks (not re-stub here).  
> Updated: 2026-07-25 (noop purge wave)

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
**Residual:** product still **transitively** links stubs via `bun_runtime` / `bao_browser` until remaining RealImpl move to owners (P0 table below).

---

## Classification legend

| Class | Meaning |
|-------|---------|
| **RealImpl** | Functional body (not silent noop). Temporary home OK only until moved to named owner. |
| **Undefined (was NoopBlocker)** | Pure noop **deleted** this wave; symbol must be defined by owner or call site removed. |
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

### RealImpl — **moved out earlier** (deleted from stubs)

| Symbol | New owner | Notes |
|--------|-----------|-------|
| `sys_preadv2` | `bun_sys` | Real `libc::preadv2` (was return `-1` NoopBlocker) |
| `sys_pwritev2` | `bun_sys` | Real `libc::pwritev2` (was undeclared/undefined) |
| `__bun_get_vm_ctx` | `bun_runtime::dispatch` | Real Mini/Js ctx (was Mini+null owner) |
| `__bun_dns_prefetch` | `bun_runtime::dispatch` | Owner hook; non-blocking warm path |
| `WTF__numberOfProcessorCores` | `bun_core` | Real `available_parallelism` (was hard-coded `1`) |

### Pure noop `#[no_mangle]` — **DELETED this wave** (was NoopBlocker)

Do **not** reintroduce. Residual = undefined until owner implements or drops FFI.

| Symbol | Owner task | Why deleted |
|--------|------------|-------------|
| `URL__getHref` / `URL__getHrefJoin` / `URL__fromString` | `bun_url` pure-Rust / drop FFI | Identity / dead-tag / always `None` |
| `URL__pathname/protocol/hostname/hash/host/password/username/search/fragmentIdentifier` | `bun_url` | Dead strings |
| `URL__getFileURLString` / `URL__pathFromFileURL` / `URL__port` / `URL__deinit` | `bun_url` | Dead / 0 / empty |
| `WTF__parseES5Date` / `WTF__parseDouble` / `WTF__dtoa` | `bun_core::wtf` / `fmt` | Always NaN / length 0 |
| `__bun_regex_compile` / `__bun_regex_matches` / `__bun_regex_drop` | `bun_install_types` + regex crate | Always fail → exact-string only |
| `Bun__linux_trace_init` / `Bun__linux_trace_emit` | `bun_perf` or delete call sites | Always false / empty |
| `WTF__releaseFastMallocFreeMemoryForThisThread` | allocator tier / `bun_alloc` | Empty |

### Dispatch `link_noop_*` — **DELETED this wave** (entire block)

Removed:

```text
bun_io::link_noop_BufferedReaderParentLink!(SubprocessPipeReader, ShellPipeReader, …)
bun_spawn::link_noop_ProcessExit!(Subprocess, Shell, FilterRunHandle, …)
```

Deps `bun_io` / `bun_spawn` dropped from this crate's `Cargo.toml` (only used for those macros).

| Variant group | Status | Real owner if any | Notes |
|---------------|--------|-------------------|-------|
| LifecycleScript / SecurityScan | **Real** | `bun_install` `link_impl_*` / `impl_buffered_reader_parent!` | Dual-def if re-listed as noop |
| FilterRunHandle / MultiRun* / TestParallelWorker* | **Real when CLI linked** | `bun_cli` | Not on default product graph → residual undefined without CLI |
| Subprocess / Shell / ChromeProcess / HostProcess / Cron* (ProcessExit) | **Undefined (P0)** | `bao_runtime` / shell / browser process | Need real `link_impl_ProcessExit!` |
| SubprocessPipeReader / Shell* / FileReader / FileResponseStream / Terminal / Cron* (ParentLink) | **Undefined (P0)** | runtime / shell / http | Need real `link_impl_BufferedReaderParentLink!` |

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
| Any `link_noop_*` in this crate | Policy: delete, do not re-stub |
| Pure noop URL / regex / WTF parse / linux_trace / releaseFastMalloc | Policy: delete, do not re-stub |

---

## Eradication backlog (ordered)

1. **P0** — Real `ProcessExit` + `BufferedReaderParentLink` for Subprocess/Shell (product spawn) in true owners (`bun_io`/`bun_spawn`/`bao_runtime`/shell/browser).
2. **P0** — `bun_url`: drop dead FFI; finish pure-Rust WHATWG surface (href/file URL).
3. **P0** — Move remaining RealImpl out of this crate (`posix_spawn_bun`, strings, signals, stack check) → owner crates; then **drop hard dep** from `bun_runtime` / `bao_browser`.
4. **P1** — Real WTF numeric parsers or pure-Rust call-site replacements.
5. **P1** — Regex for `PnpmMatcher` via `regex` crate, not always-None.
6. **P1** — Ensure CLI product builds always link `bun_cli` impls for FilterRun*/MultiRun*/TestParallelWorker*.

---

## Dual-def rule (iron)

On full product path (`bao` → runtime → `bun_install` + this crate):

- **Never** `link_noop` a variant that already has `link_impl` in a co-linked crate.
- **Never** stub a `#[no_mangle]` already exported by C lib or owner crate.
- Prefer **delete stub** over weak-link / rename hacks.
- Prefer **undefined + owner task** over silent noop (this wave).
