// @trace REQ-ENG-006 REQ-IMPL-01 REQ-IMPL-02 REQ-IMPL-03 REQ-IMPL-04 REQ-IMPL-05
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_imports)]
// @trace REQ-IMPL-01: Phase 1 SpiderMonkey engine replacement (completed)
// @trace REQ-IMPL-02: Phase 2 servo engine integration + rendering (completed)
// @trace REQ-IMPL-03: Phase 3 CDP Server implementation (completed)
// @trace REQ-IMPL-04: Phase 4 Stealth anti-fingerprinting (completed)
// @trace REQ-IMPL-05: Phase 5 Integration testing and release (completed)

// Force-link all C library symbol replacements from bao_native_stubs
// (pure Rust implementations of mimalloc, BoringSSL, uSockets, etc.)
// Without this, the linker may GC unreferenced symbols from the rlib.
fn _force_native_stubs_link() {
    bao_native_stubs::force_link();
    bao_uloop::force_link();
}
pub mod bun_api;
pub mod bun_test;
pub mod dispatch;
pub mod fetch_api;
pub mod gc_store;
pub mod globals;
pub mod web_api;
pub mod node_buffer;
pub mod node_child_process;
pub mod node_crypto;
pub mod node_dns;
pub mod node_events;
pub mod node_fs;
pub mod node_http;
pub mod node_https;
pub mod node_module;
pub mod node_net;
pub mod node_os;
pub mod node_path;
pub mod node_perf_hooks;
pub mod node_querystring;
pub mod node_readline;
pub mod permission_bridge;
pub mod node_stream;
pub mod node_string_decoder;
pub mod node_timers_module;
pub mod node_tls;
pub mod node_tty;
pub mod node_url;
pub mod node_util;
pub mod node_vm;
pub mod node_zlib;
pub mod require;
pub mod runtime;
pub mod timers;
pub mod http_client;
// resolver_bridge: P1-D 阶段实现，依赖 bun_ast + bao_engine::set_resolver 尚未就绪
// pub mod resolver_bridge;
pub mod stealth_http;

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

/// Register atexit handler to prevent SpiderMonkey GC crashes on process exit.
/// Must be called before any JsContext creation in test binaries.
pub fn install_exit_handler() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        extern "C" fn noop() {}
        unsafe {
            libc::atexit(noop);
        }
    });
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
