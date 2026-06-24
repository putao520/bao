// @trace REQ-ENG-001 [entity:BaoRuntime] REQ-ENG-006 REQ-IMPL-01 REQ-IMPL-02 REQ-IMPL-03 REQ-IMPL-04 REQ-IMPL-05 REQ-PURE-010 [level:library] [entity:BaoRuntime]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_imports)]
// @trace REQ-PURE-010: bao_runtime (Rust) replaces deleted bun_runtime (Zig) — zero Zig deps, zero JSC refs
// @trace REQ-IMPL-01: Phase 1 SpiderMonkey engine replacement (completed)
// @trace REQ-IMPL-02: Phase 2 servo engine integration + rendering (completed)
// @trace REQ-IMPL-03: Phase 3 CDP Server implementation (completed)
// @trace REQ-IMPL-04: Phase 4 Stealth anti-fingerprinting (completed)
// @trace REQ-IMPL-05: Phase 5 Integration testing and release (completed)

pub mod bun_api;
pub mod bun_builtins;
pub mod bun_sqlite;
pub mod bun_ffi;
pub mod bun_test;
pub mod bao_browser_global;
pub mod dispatch;
pub mod fetch_api;
// @trace REQ-ENG-010 [entity:FetchTasklet] — async HTTP integration helper
// (BCE-20260618-007). Shared by node_http/node_https/node_tls JS-native entries.
pub mod fetch_async;
pub mod h3_fetch;
pub mod gc_store;
pub mod globals;
pub mod web_api;
pub mod node_async_hooks;
pub mod node_buffer;
pub mod node_child_process;
pub mod node_cluster;
pub mod node_console;
pub mod node_constants;
pub mod node_crypto;
pub mod node_dgram;
pub mod node_diagnostics_channel;
pub mod node_dns;
pub mod node_domain;
pub mod node_events;
pub mod node_fs;
pub mod node_http;
pub mod node_http2;
pub mod node_http2_upgrade;
pub mod node_https;
pub mod node_inspector;
pub mod node_inspector_promises;
pub mod node_internal_http;
pub mod node_internal_streams;
pub mod node_module;
pub mod node_net;
pub mod node_os;
pub mod node_path;
pub mod node_perf_hooks;
pub mod node_punycode;
pub mod node_querystring;
pub mod node_readline;
pub mod node_repl;
pub mod permission_bridge;
pub mod node_stream;
pub mod node_stream_consumers;
pub mod node_stream_web;
pub mod node_string_decoder;
pub mod node_stubs;
pub mod node_subpath_aliases;
pub mod node_sys;
pub mod node_test;
pub mod node_timers_module;
pub mod node_tls;
pub mod node_tls_common;
pub mod node_tls_wrap;
pub mod node_trace_events;
pub mod node_tty;
pub mod node_url;
pub mod node_util;
pub mod node_util_types;
pub mod node_vm;
pub mod node_wasi;
pub mod node_worker_threads;
pub mod node_zlib;
pub mod require;
pub mod runtime;
pub mod timers;
pub mod http_client;
pub mod resolver_bridge;
pub mod s3_api;
pub mod stealth_http;
pub mod bun_listen;
pub mod bun_udp;
pub mod bun_shell;
pub mod bun_password;
pub mod install;

pub use runtime::BaoRuntime;

// ── Orderly exit infrastructure ──
// process.exit() / Bun.exit() set a flag instead of calling std::process::exit(),
// so the CLI main loop can return naturally → BaoRuntime drops → SmRuntimeGuard
// drops (Runtime then Engine) → JS_ShutDown. No segfault from bypassed drop chain.

thread_local! {
    static EXIT_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static EXIT_CODE: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Request process exit with the given code. Called by process.exit() and Bun.exit().
pub fn request_exit(code: i32) {
    EXIT_CODE.with(|c| c.set(code));
    EXIT_REQUESTED.with(|r| r.set(true));
}

/// Check whether process.exit() or Bun.exit() was called.
pub fn should_exit() -> bool {
    EXIT_REQUESTED.with(|r| r.get())
}

/// Return the exit code set by process.exit() / Bun.exit().
pub fn exit_code() -> i32 {
    EXIT_CODE.with(|c| c.get())
}

/// Clear the exit flag. Used by test runner between test files
/// so one file's process.exit() doesn't affect subsequent files.
pub fn clear_exit() {
    EXIT_REQUESTED.with(|r| r.set(false));
    EXIT_CODE.with(|c| c.set(0));
}

/// Install orderly shutdown hooks for SpiderMonkey.
///
/// No-op at init time. Tests should call `shutdown_thread_sm()` at the end
/// of each test function (before returning) to properly clean up the
/// SpiderMonkey Runtime and Engine. Without this, C++ TLS destructors
/// (`MutexImpl::~MutexImpl`) will SIGSEGV on thread exit.
///
/// This function is kept for API compatibility with existing test files.
pub fn install_exit_handler() {
    // No-op at init time. Cleanup happens via shutdown_thread_sm() at test end.
}

/// Shut down SpiderMonkey Runtime on the current thread.
///
/// Drops Runtime (→ JS_DestroyContext) stored in TLS. Safe to call multiple
/// times per thread (e.g., between tests). The JSEngine remains alive so
/// subsequent `for_test()` calls can create a new Runtime.
///
/// For process exit cleanup (JS_ShutDown), use `shutdown_engine()` instead.
pub fn shutdown_thread_sm() {
    bao_engine::context::JsContext::shutdown_thread_sm();
}

/// Shut down the SpiderMonkey engine entirely (process exit only).
///
/// Calls `JS_ShutDown` to clean up SpiderMonkey's process-wide C++ state.
/// After this, no new Runtime/JSContext can be created on any thread.
/// Should only be called at process exit.
pub fn shutdown_engine() {
    bao_engine::context::JsContext::shutdown_engine();
}

/// Safe JS string conversion: returns "" if JS string allocation fails.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext pointer and `val` is rooted or otherwise protected from GC.
pub unsafe fn js_to_rust_string(cx: *mut mozjs::jsapi::JSContext, val: mozjs::jsval::JSVal) -> String {
    let ptr = val.to_string();
    match ::std::ptr::NonNull::new(ptr) {
        Some(nn) => mozjs::conversions::jsstr_to_string(cx, nn),
        None => String::new(),
    }
}

/// Safe JSString pointer conversion: returns "" if pointer is null.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext pointer and `s` is either null or a valid JSString pointer.
pub unsafe fn jsstr_to_rust_string(cx: *mut mozjs::jsapi::JSContext, s: *mut mozjs::jsapi::JSString) -> String {
    match ::std::ptr::NonNull::new(s) {
        Some(nn) => mozjs::conversions::jsstr_to_string(cx, nn),
        None => String::new(),
    }
}

/// Force-link `bun_install` compilation unit so the linker resolves
/// `__bun_resolver_init_package_manager` from `bun_install::auto_installer`.
#[inline(never)]
pub fn force_link_bun_install() {
    let _ = bun_install::Subcommand::Install;
}

// Force-link bao_native_stubs (dispatch no-op stubs + C library bridges).
// Without this anchor, the linker GCs the entire bao_native_stubs compilation
// unit, causing undefined __bun_dispatch__* and C symbol errors in test binaries.
#[used]
static BAO_NATIVE_STUBS_ANCHOR: unsafe extern "C" fn() = bao_native_stubs::__force_link_entry;
