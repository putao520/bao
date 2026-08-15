// @trace REQ-ENG-001
//! C library link bridge for symbols originally provided by Zig-compiled
//! C/Zig code in upstream Bun. This crate NO LONGER defines any `#[no_mangle]`
//! C-seam symbol itself (STUB-INVENTORY dual-def iron rule) — it is purely a
//! **test-linking anchor**: `force_link()` drags in the C archives and the
//! crates that own the seam symbols.
//!
//! ## Symbol ownership (one `#[no_mangle]` definition per symbol, everywhere)
//! - `bun_core::native_seam` / `bun_core::util::spawn_ffi` — stdio / StackCheck /
//!   crash dump / cpu features / executable probe / ares_inet_pton /
//!   BunString__fromBytes / WTFStringImpl destroy / posix_spawn_bun /
//!   reload-process sync
//! - `bao_uloop` — TLS C→Rust hooks (Bun__Node__UseSystemCA,
//!   BUN__warn__extra_ca_load_failed, bun_ssl_ctx_cache_on_free) +
//!   Bun__addrinfo_* DNS seam + loop dispatch symbols
//! - `bun_runtime::product_native_symbols` — URL__* / WTF__parse* / __bun_regex_*
//!   / signal-forwarding quartet (Bun__{register,unregister,sendPending}Signals*
//!   + Bun__currentSyncPID) / linux_trace
//! - `bao_boringssl_bridge` — UpgradedDuplex__* (12 fns)
//! - `bun_uws_sys::c_hooks` — Bun__JSC_onBeforeWait / Bun__panic / sys_epoll_pwait2
//! - `bun_crash_handler` — __bun_crash_handler_out_of_memory
//! - `bun_alloc` — WTF__releaseFastMallocFreeMemoryForThisThread
//! - `bun_core::util` — WTF__numberOfProcessorCores
//! - compiled C libraries: mimalloc (bun_mimalloc_sys), highway (bun_highway),
//!   zstd (pure Rust via bun_zstd), brotli (pure Rust), lsquic/lshpack
//!   (bun_lsquic_sys), BoringSSL (bun_boringssl_sys), uSockets (bun_uws_sys)
//!
//! Linker GC prevention: a ctor in .init_array auto-calls force_link() at load
//! time, so integration tests don't need explicit force_link() calls.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod c_lib_stubs;

/// Force the linker to include the C libraries and seam-symbol owners.
/// Call this from test code: `bao_native_stubs::force_link();`
///
/// Note: Only call this after the process is fully initialized (e.g., at the
/// start of a test function, not in a global ctor). Some chained code calls
/// libc functions that require full process initialization.
#[inline(never)]
pub fn force_link() {
    // bao_uloop's `#[unsafe(no_mangle)] extern "C"` loop symbols
    // (uws_get_loop / us_wakeup_loop / us_loop_run_bun_tick / ...), DNS seam
    // and TLS C→Rust hooks. Without this, the linker strips them and any code
    // path that touches `bun_event_loop::MiniEventLoop` fails to link.
    bao_uloop::force_link();

    // Compiled mimalloc C library (libmimalloc.a).
    bun_mimalloc_sys::force_link();

    // Compiled highway SIMD library (libhighway.a + libhighway_strings.a).
    bun_highway::force_link();

    // zstd — now pure Rust (zstd-pure-rs), no C library to link.
    bun_zstd::force_link();

    // bun_core / bao_uloop / bun_runtime seam symbols — owned there; do NOT
    // define or re-anchor them here (STUB-INVENTORY dual-def iron rule).
    // Former def sites removed: bun_restore_stdio, ares_inet_pton,
    // Bun__StackCheck__{initialize,getMaxStack}, WTF__DumpStackTrace,
    // on_before_reload_process_linux, posix_spawn_bun, bun_cpu_features,
    // is_executable_file, BunString__fromBytes, Bun__WTFStringImpl__destroy,
    // Bun__currentSyncPID, Bun__{register,unregister,sendPending}Signals*,
    // Bun__Node__UseSystemCA, BUN__warn__extra_ca_load_failed,
    // bun_ssl_ctx_cache_on_free — now single-defined in
    // bun_core::native_seam / bun_core::util::spawn_ffi / bao_uloop /
    // bun_runtime::product_native_symbols.

    // Force-link all c_lib_stubs symbols
    c_lib_stubs::force_c_lib_stubs();
}

// Entry point for downstream crates to force-link bao_native_stubs.
// Downstream crates place `#[used] static F: unsafe extern "C" fn() =
// bao_native_stubs::__force_link_entry;` in their lib.rs so the linker
// pulls in the entire bao_native_stubs compilation unit (which contains
// all dispatch stubs and C library replacements).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __force_link_entry() {
    force_link();
}

// ──────────────────────────────────────────────────────────────
// §0  Closed-set dispatch no-op stubs via bun_dispatch::link_noop_*
// ──────────────────────────────────────────────────────────────
// The proc-macro `link_interface!` auto-generates `link_noop_<Iface>!` which
// produces `#[unsafe(no_mangle)] extern "Rust"` stubs for every listed variant.
// Symbols live in this compilation unit, so the linker pulls them in automatically.
//
// Dual-def rule (full product path always links `bun_install` via `bun_runtime`):
// NEVER list variants that already have `link_impl_*!` in a co-linked crate.
//   - LifecycleScript / SecurityScan → real impls in bun_install
//   - All other BufferedReaderParentLink arms → `bun_runtime::product_buffered_reader`
//   - All other ProcessExit arms → `bun_runtime::product_process_exit`
// Do NOT reintroduce `link_noop_*` for those — dual-def with product.
// Listing co-owned variants dual-defines `__bun_dispatch__*__…` and breaks
// consumer lib-test link (gsc-frog-tools / `cargo test -p bun_runtime`).

// BufferedReaderParentLink: no link_noop — product_buffered_reader owns the set.
// ProcessExit: no link_noop — product_process_exit owns the set.
