// @trace REQ-ENG-001 [entity:BaoRuntime] REQ-ENG-006 REQ-IMPL-01 REQ-IMPL-02 REQ-IMPL-03 REQ-IMPL-04 REQ-IMPL-05 REQ-PURE-010 [level:library] [entity:BaoRuntime]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_imports)]
// @trace REQ-PURE-010: bao_runtime (Rust) replaces deleted bun_runtime (Zig) — zero Zig deps, zero JSC refs
// @trace REQ-IMPL-01: Phase 1 SpiderMonkey engine replacement (completed)
// @trace REQ-IMPL-02: Phase 2 servo engine integration + rendering (completed)
// @trace REQ-IMPL-03: Phase 3 CDP Server implementation (completed)
// @trace REQ-IMPL-04: Phase 4 Stealth anti-fingerprinting (completed)
// @trace REQ-IMPL-05: Phase 5 Integration testing and release (completed)

pub mod bao_browser_global;
pub mod bun_api;
pub mod bun_build;
pub mod bun_builtins;
// @trace REQ-ENG-006 [api:Bun.* Bun-face completion wave] — per-domain Bun
// API modules mounted by populate_bun_object (bun_api.rs).
pub mod bun_glob_api;
pub mod bun_hash_api;
pub mod bun_inspect_api;
pub mod bun_mime_api;
pub mod bun_serde_api;
pub mod bun_spawn_sync;
pub mod bun_util_api;
pub mod bun_ffi;
pub mod bun_sqlite;
pub mod bun_test;
pub mod dispatch;
pub mod fetch_api;
// @trace REQ-ENG-006 [entity:IpcChannel] — child_process IPC + cluster fd passing (SCM_RIGHTS)
pub mod ipc_channel;
pub mod workflow_host_global;
// @trace REQ-ENG-010 [entity:FetchTasklet] — async HTTP integration helper
// (BCE-20260618-007). Shared by node_http/node_https/node_tls JS-native entries.
pub mod bun_listen;
pub mod bun_password;
pub mod bun_shell;
pub mod bun_udp;
pub mod fetch_async;
pub mod gc_store;
pub mod globals;
pub mod h3_fetch;
pub mod http_client;
pub mod install;
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
// win-cross, #18 (W7): real bodies behind `uws_sys`'s link-time-dispatch
// handles (`WindowsNamedPipe__*` externs).
#[cfg(windows)]
pub mod socket;
pub mod node_wasi;
pub mod node_worker_threads;
pub mod node_zlib;
pub mod permission_bridge;
pub mod require;
pub mod resolver_bridge;
pub mod runtime;
// @trace REQ-CLI-001 [entity:BaoRuntime] — SIGINT→ExecutionControl::cancel
// bridge (SM-EVOLUTION #24 S1 CLI wiring; ledger S1 legislation proposal
// consumed by user ruling 2026-09-10). Internal experimental surface.
pub mod interrupt_bridge;
pub mod s3_api;
pub mod stealth_http;
pub mod timers;
// @trace REQ-ENG-006 [api:process.on uncaughtException/unhandledRejection] —
// unified uncaught-exception / unhandled-rejection router (Node semantics).
pub mod uncaught;
pub mod web_api;
// @trace REQ-ENG-005 [api:Web Streams] — full WHATWG Streams implementation
// (web_streams.js, ported from Bun). Installed on the global by
// node_stream::install for realms that lack servo's native streams (CLI +
// browser privileged evaluate realm). Previously dead code: the module was
// never declared, so CLI fell back to a broken inline polyfill whose
// TransformStream hung the event loop (BCE-20260816-STREAM-WEB).
pub mod web_streams;
// @trace REQ-ENG-001 [entity:BaoRuntime] [api:fetch] — full WHATWG
// Headers/Request/Response classes (installed on the global by
// globals::install_web_apis; consumed by fetch_api::fetch_fn).
pub mod web_fetch_classes;
// @trace STUB-INVENTORY: product real ProcessExit owners (no link_noop) — residual=0 for PE
pub mod product_process_exit;
// @trace STUB-INVENTORY: product real BufferedReaderParentLink owners (no link_noop) — residual=0 for BR
pub mod product_buffered_reader;
// @trace STUB-INVENTORY: product residual RealImpl rehomed from bao_native_stubs
pub mod product_native_symbols;

/// Link-time faces for the MiniEventLoop upward extern "Rust" contracts
/// (stdio Blob.Store + the thread VM getter) — issue #18 W7 second layer.
pub mod webcore_faces;
// @trace STUB-INVENTORY: Bun__linux_trace_* RealImpl (cross-platform; residual=0)
pub mod linux_trace;

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

/// Set only the exit code without requesting an exit.
/// Backs the `process.exitCode` property setter: assigning the property alone
/// steers the final exit code (honoured on natural exit and by 'exit'
/// listeners), while the process keeps running until the event loop drains
/// or process.exit() is called — Node semantics.
pub fn set_exit_code(code: i32) {
    EXIT_CODE.with(|c| c.set(code));
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
pub unsafe fn js_to_rust_string(
    cx: *mut mozjs::jsapi::JSContext,
    val: mozjs::jsval::JSVal,
) -> String {
    let ptr = val.to_string();
    match ::std::ptr::NonNull::new(ptr) {
        Some(nn) => mozjs::conversions::jsstr_to_string(&mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx)), nn),
        None => String::new(),
    }
}

/// Safe JSString pointer conversion: returns "" if pointer is null.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext pointer and `s` is either null or a valid JSString pointer.
pub unsafe fn jsstr_to_rust_string(
    cx: *mut mozjs::jsapi::JSContext,
    s: *mut mozjs::jsapi::JSString,
) -> String {
    match ::std::ptr::NonNull::new(s) {
        Some(nn) => mozjs::conversions::jsstr_to_string(&mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx)), nn),
        None => String::new(),
    }
}

/// Force-link `bun_install` compilation unit so the linker resolves
/// `__bun_resolver_init_package_manager` from `bun_install::auto_installer`.
#[inline(never)]
pub fn force_link_bun_install() {
    let _ = bun_install::Subcommand::Install;
}

// @trace STUB-INVENTORY: default product path must not force-link bao_native_stubs.
// Closed-set PE/BR → product_process_exit + product_buffered_reader (real link_impl).
// Real C libs → force_link_native_c_libs. Do not reintroduce BAO_NATIVE_STUBS_ANCHOR / link_noop residual.

// Real C-lib force-link (former bao_native_stubs::force_c_lib_stubs chain):
// quic.c (bun_uws_sys) needs liblsquic; TLS needs boringssl; loop from bao_uloop.
#[inline(never)]
fn force_link_native_c_libs() {
    let _ = bun_lsquic_sys::force_link as *const () as usize;
    let _ = bun_lsquic_sys::force_link_lshpack as *const () as usize;
    let _ = bun_boringssl_sys::force_link as *const () as usize;
    let _ = bao_uloop::force_link as *const () as usize;
    // Keep loop entry symbols live without calling with null (null-deref risk).
    let _ = bao_uloop::bao_loop_tick as *const () as usize;
    let _ = bao_uloop::us_wakeup_loop as *const () as usize;
    let _ = bao_uloop::uws_get_loop as *const () as usize;
    // Retain product real ProcessExit + BufferedReader + RealImpl compilation units.
    // product_dispatch_residual deleted (P3): PE/BR noops residual=0.
    product_process_exit::force_link_product_process_exit();
    product_buffered_reader::force_link_product_buffered_reader();
    product_native_symbols::force_link_product_native_symbols();
}

#[used]
static FORCE_NATIVE_C_LIBS: fn() = force_link_native_c_libs;

// The webcore faces (`__bun_stdio_blob_store_new` / `__bun_js_vm_get`) are
// provided by this crate itself, so they are already in the lib-test
// compilation unit — the anchor below only keeps them referenced.
//
// DO NOT reference `bao_bundler::force_link_test_seams()` here (5d3c1f8a
// regression): `bao_bundler` is a dev-dep whose normal closure contains
// `bun_runtime` itself, so using it puts a *second* copy of this crate's rlib
// on the lib-test link line next to the test compilation unit → lld-link/COFF
// duplicate-symbol on every `#[no_mangle]` body (`WindowsNamedPipe__*`,
// `Bun__linux_trace_*`, `__bun_stdio_blob_store_new`, `__bun_js_vm_get`,
// `bao_sysv_call`). The 18 `JSBundlerPlugin__*` / `DevServerHandle__Bake__*` /
// `VmLoaderCtx__Runtime__*` faces are not referenced in this closure (their
// referrer `bun_bundler` is not a dependency of this crate), so this link line
// needs no supplier for them; the link itself is the resolver proof.
#[cfg(test)]
#[allow(non_snake_case)]
mod test_link_seams {
    use core::ffi::c_void;

    use bun_bundler::bundle_v2::JSBundlerPlugin;
    use bun_core::String as BunString;

    #[test]
    fn link_higher_tier_seams() {
        crate::webcore_faces::force_link_webcore_faces();
    }

    // ── CYCLEBREAK bundler faces (38) ────────────────────────────────────
    // Root cause of the removed `bao_bundler` reference above: this crate's
    // lib-test cannot link the higher-tier provider, so the faces its own
    // dependency closure references (via bun_bundler, a dep of bun_install /
    // bun_resolver) are supplied locally — every body mirrors its production
    // owner 1:1 (all owners are themselves Phase-1 honest-degraded bodies);
    // owner file named at each group. Same seam set as bun_sm's lib-test
    // (src/bun_sm/src/lib.rs test_link_seams).

    // ── JSBundlerPlugin faces (6) — owner: bao_bundler::js_bundler_plugin_seam
    // (@trace REQ-ENG-006): the honest **no-plugins** answers (Bao registers
    // no bundler plugins; every BundleV2 passes plugins: None).

    #[unsafe(no_mangle)]
    pub extern "C" fn JSBundlerPlugin__anyMatches(
        _this: &JSBundlerPlugin,
        _namespace: &mut BunString,
        _path: &mut BunString,
        _is_on_load: bool,
    ) -> bool {
        false
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn JSBundlerPlugin__matchOnLoad(
        _this: &mut JSBundlerPlugin,
        _namespace_string: &mut BunString,
        _path: &mut BunString,
        _context: *mut c_void,
        _default_loader: u8,
        _is_server_side: bool,
    ) {
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn JSBundlerPlugin__matchOnResolve(
        _this: &mut JSBundlerPlugin,
        _namespace_string: &mut BunString,
        _path: &mut BunString,
        _importer: &mut BunString,
        _context: *mut c_void,
        _kind: u8,
    ) {
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn JSBundlerPlugin__drainDeferred(_this: &mut JSBundlerPlugin, _rejected: bool) {
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn JSBundlerPlugin__hasOnBeforeParsePlugins(_this: &JSBundlerPlugin) -> i32 {
        0
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn JSBundlerPlugin__callOnBeforeParsePlugins(
        _this: &JSBundlerPlugin,
        _ctx: *mut c_void,
        _namespace: &BunString,
        _path: &BunString,
        _args: *mut c_void,
        _result: *mut c_void,
        _should_continue_running: *mut i32,
    ) -> i32 {
        0
    }

    // ── VmLoaderCtx[Runtime] faces (12) — owner: bao_bundler::vm_loader
    // `link_impl_VmLoaderCtx!` (BaoVmLoaderCtx, @trace REQ-ENG-005), Phase-1
    // bodies mirrored 1:1. `link_noop_VmLoaderCtx!` cannot supply this
    // interface (origin_host/… return `&'static [u8]`, no `Default`), so the
    // bodies are spelled against the interface's own ret/arg aliases (exact
    // ABI by construction). `blob_deinit` is not referenced by this closure.

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__origin_host(
        __owner: *mut (),
    ) -> bun_bundler::__VmLoaderCtx__origin_host__ret {
        let _ = __owner;
        b"localhost"
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__origin_path(
        __owner: *mut (),
    ) -> bun_bundler::__VmLoaderCtx__origin_path__ret {
        let _ = __owner;
        b"/"
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__main(
        __owner: *mut (),
    ) -> bun_bundler::__VmLoaderCtx__main__ret {
        let _ = __owner;
        b"<input>"
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__loaders(
        __owner: *mut (),
    ) -> bun_bundler::__VmLoaderCtx__loaders__ret {
        let _ = __owner;
        // Phase 1: null — the bundler uses its default loaders.
        core::ptr::null()
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__read_dir_info_package_json(
        __owner: *mut (),
        __read_dir_info_package_json_dir:
            bun_bundler::__VmLoaderCtx__read_dir_info_package_json__arg_dir<'_>,
    ) -> bun_bundler::__VmLoaderCtx__read_dir_info_package_json__ret {
        let _ = (__owner, __read_dir_info_package_json_dir);
        // Phase 1: no resolver integration.
        None
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__is_blob_url(
        __owner: *mut (),
        __is_blob_url_specifier: bun_bundler::__VmLoaderCtx__is_blob_url__arg_specifier<'_>,
    ) -> bun_bundler::__VmLoaderCtx__is_blob_url__ret {
        let _ = (__owner, __is_blob_url_specifier);
        false
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__resolve_blob(
        __owner: *mut (),
        __resolve_blob_specifier: bun_bundler::__VmLoaderCtx__resolve_blob__arg_specifier<'_>,
    ) -> bun_bundler::__VmLoaderCtx__resolve_blob__ret {
        let _ = (__owner, __resolve_blob_specifier);
        None
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__eval_source(
        __owner: *mut (),
    ) -> bun_bundler::__VmLoaderCtx__eval_source__ret {
        let _ = __owner;
        None
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__blob_loader(
        __owner: *mut (),
        __blob_loader_blob: bun_bundler::__VmLoaderCtx__blob_loader__arg_blob<'_>,
    ) -> bun_bundler::__VmLoaderCtx__blob_loader__ret {
        let _ = (__owner, __blob_loader_blob);
        None
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__blob_file_name(
        __owner: *mut (),
        __blob_file_name_blob: bun_bundler::__VmLoaderCtx__blob_file_name__arg_blob<'_>,
    ) -> bun_bundler::__VmLoaderCtx__blob_file_name__ret {
        let _ = (__owner, __blob_file_name_blob);
        None
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__blob_needs_read_file(
        __owner: *mut (),
        __blob_needs_read_file_blob: bun_bundler::__VmLoaderCtx__blob_needs_read_file__arg_blob<'_>,
    ) -> bun_bundler::__VmLoaderCtx__blob_needs_read_file__ret {
        let _ = (__owner, __blob_needs_read_file_blob);
        false
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__VmLoaderCtx__Runtime__blob_shared_view(
        __owner: *mut (),
        __blob_shared_view_blob: bun_bundler::__VmLoaderCtx__blob_shared_view__arg_blob<'_>,
    ) -> bun_bundler::__VmLoaderCtx__blob_shared_view__ret {
        let _ = (__owner, __blob_shared_view_blob);
        &[]
    }

    // ── DevServerHandle[Bake] faces (11) — owner: bao_bundler::dev_server
    // `link_impl_DevServerHandle!` (BaoDevServer, @trace REQ-CLI-001): no
    // bake dev server exists; no-op defaults + `Err(AllocError)` where the
    // interface demands `Result` (which `link_noop_` cannot generate).

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__barrel_needed_exports(
        __owner: *mut (),
    ) -> bun_bundler::__DevServerHandle__barrel_needed_exports__ret {
        let _ = __owner;
        core::ptr::null_mut()
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__log_for_resolution_failures(
        __owner: *mut (),
        __log_for_resolution_failures_abs_path:
            bun_bundler::__DevServerHandle__log_for_resolution_failures__arg_abs_path<'_>,
        __log_for_resolution_failures_graph:
            bun_bundler::__DevServerHandle__log_for_resolution_failures__arg_graph<'_>,
    ) -> bun_bundler::__DevServerHandle__log_for_resolution_failures__ret {
        let _ = (
            __owner,
            __log_for_resolution_failures_abs_path,
            __log_for_resolution_failures_graph,
        );
        core::ptr::null_mut()
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__finalize_bundle(
        __owner: *mut (),
        __finalize_bundle_bv2: bun_bundler::__DevServerHandle__finalize_bundle__arg_bv2<'_>,
        __finalize_bundle_result: bun_bundler::__DevServerHandle__finalize_bundle__arg_result<'_>,
    ) -> bun_bundler::__DevServerHandle__finalize_bundle__ret {
        let _ = (__owner, __finalize_bundle_bv2, __finalize_bundle_result);
        Err(bun_core::Error::from(bun_core::AllocError))
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__handle_parse_task_failure(
        __owner: *mut (),
        __handle_parse_task_failure_err:
            bun_bundler::__DevServerHandle__handle_parse_task_failure__arg_err<'_>,
        __handle_parse_task_failure_graph:
            bun_bundler::__DevServerHandle__handle_parse_task_failure__arg_graph<'_>,
        __handle_parse_task_failure_abs_path:
            bun_bundler::__DevServerHandle__handle_parse_task_failure__arg_abs_path<'_>,
        __handle_parse_task_failure_log:
            bun_bundler::__DevServerHandle__handle_parse_task_failure__arg_log<'_>,
        __handle_parse_task_failure_bv2:
            bun_bundler::__DevServerHandle__handle_parse_task_failure__arg_bv2<'_>,
    ) -> bun_bundler::__DevServerHandle__handle_parse_task_failure__ret {
        let _ = (
            __owner,
            __handle_parse_task_failure_err,
            __handle_parse_task_failure_graph,
            __handle_parse_task_failure_abs_path,
            __handle_parse_task_failure_log,
            __handle_parse_task_failure_bv2,
        );
        Err(bun_core::Error::from(bun_core::AllocError))
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__put_or_overwrite_asset(
        __owner: *mut (),
        __put_or_overwrite_asset_path:
            bun_bundler::__DevServerHandle__put_or_overwrite_asset__arg_path<'_>,
        __put_or_overwrite_asset_contents:
            bun_bundler::__DevServerHandle__put_or_overwrite_asset__arg_contents<'_>,
        __put_or_overwrite_asset_content_hash:
            bun_bundler::__DevServerHandle__put_or_overwrite_asset__arg_content_hash<'_>,
    ) -> bun_bundler::__DevServerHandle__put_or_overwrite_asset__ret {
        let _ = (
            __owner,
            __put_or_overwrite_asset_path,
            __put_or_overwrite_asset_contents,
            __put_or_overwrite_asset_content_hash,
        );
        Err(bun_core::Error::from(bun_core::AllocError))
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__track_resolution_failure(
        __owner: *mut (),
        __track_resolution_failure_import_source:
            bun_bundler::__DevServerHandle__track_resolution_failure__arg_import_source<'_>,
        __track_resolution_failure_specifier:
            bun_bundler::__DevServerHandle__track_resolution_failure__arg_specifier<'_>,
        __track_resolution_failure_renderer:
            bun_bundler::__DevServerHandle__track_resolution_failure__arg_renderer<'_>,
        __track_resolution_failure_loader:
            bun_bundler::__DevServerHandle__track_resolution_failure__arg_loader<'_>,
    ) -> bun_bundler::__DevServerHandle__track_resolution_failure__ret {
        let _ = (
            __owner,
            __track_resolution_failure_import_source,
            __track_resolution_failure_specifier,
            __track_resolution_failure_renderer,
            __track_resolution_failure_loader,
        );
        Err(bun_core::Error::from(bun_core::AllocError))
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__is_file_cached(
        __owner: *mut (),
        __is_file_cached_abs_path: bun_bundler::__DevServerHandle__is_file_cached__arg_abs_path<'_>,
        __is_file_cached_side: bun_bundler::__DevServerHandle__is_file_cached__arg_side<'_>,
    ) -> bun_bundler::__DevServerHandle__is_file_cached__ret {
        let _ = (__owner, __is_file_cached_abs_path, __is_file_cached_side);
        None
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__asset_hash(
        __owner: *mut (),
        __asset_hash_abs_path: bun_bundler::__DevServerHandle__asset_hash__arg_abs_path<'_>,
    ) -> bun_bundler::__DevServerHandle__asset_hash__ret {
        let _ = (__owner, __asset_hash_abs_path);
        None
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__current_bundle_start_data(
        __owner: *mut (),
    ) -> bun_bundler::__DevServerHandle__current_bundle_start_data__ret {
        let _ = __owner;
        core::ptr::null_mut()
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__register_barrel_with_deferrals(
        __owner: *mut (),
        __register_barrel_with_deferrals_path:
            bun_bundler::__DevServerHandle__register_barrel_with_deferrals__arg_path<'_>,
    ) -> bun_bundler::__DevServerHandle__register_barrel_with_deferrals__ret {
        let _ = (__owner, __register_barrel_with_deferrals_path);
        Err(bun_core::Error::from(bun_core::AllocError))
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__DevServerHandle__Bake__register_barrel_export(
        __owner: *mut (),
        __register_barrel_export_barrel_path:
            bun_bundler::__DevServerHandle__register_barrel_export__arg_barrel_path<'_>,
        __register_barrel_export_alias:
            bun_bundler::__DevServerHandle__register_barrel_export__arg_alias<'_>,
    ) -> bun_bundler::__DevServerHandle__register_barrel_export__ret {
        let _ = (
            __owner,
            __register_barrel_export_barrel_path,
            __register_barrel_export_alias,
        );
    }

    // ── TranspilerCacheImpl[Jsc] faces (2) — owner:
    // bao_bundler::transpiler_cache_seam (@trace REQ-ENG-006): the honest
    // disabled semantics — get never reports a hit, put stores nothing.

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__TranspilerCacheImpl__Jsc__get(
        __owner: *mut (),
        __get_source: bun_ast::__TranspilerCacheImpl__get__arg_source<'_>,
        __get_parser_options: bun_ast::__TranspilerCacheImpl__get__arg_parser_options<'_>,
        __get_used_jsx: bool,
    ) -> bool {
        let _ = (
            __owner,
            __get_source,
            __get_parser_options,
            __get_used_jsx,
        );
        // No cache backend: never report a hit.
        false
    }

    #[unsafe(no_mangle)]
    unsafe fn __bun_dispatch__TranspilerCacheImpl__Jsc__put(
        __owner: *mut (),
        __put_output_code: &[u8],
        __put_sourcemap: &[u8],
        __put_esm_record: &[u8],
    ) {
        // No cache backend: store nothing.
        let _ = (
            __owner,
            __put_output_code,
            __put_sourcemap,
            __put_esm_record,
        );
    }

    // ── macro-context faces (5) — owner: bao_bundler::macro_context: the
    // honest **no-macro-state** semantics (no macro VM in Bao).

    #[unsafe(no_mangle)]
    pub extern "Rust" fn __bun_macro_context_init(
        _transpiler: *mut c_void,
    ) -> bun_js_parser::Macro::MacroContext {
        bun_js_parser::Macro::MacroContext {
            javascript_object: bun_js_parser::Macro::MacroJSCtx::ZERO,
            data: core::ptr::null_mut(),
        }
    }

    #[unsafe(no_mangle)]
    pub extern "Rust" fn __bun_macro_context_deinit(data: *mut c_void) {
        if data.is_null() {
            return;
        }
        // A non-null `data` can only come from a different definer — never ours.
        unreachable!("Bao macro bridge: deinit on a foreign MacroContext");
    }

    #[unsafe(no_mangle)]
    pub extern "Rust" fn __bun_macro_context_call(
        _ctx: &mut bun_js_parser::Macro::MacroContext,
        _import_record_path: &[u8],
        _source_dir: &[u8],
        log: &mut bun_ast::Log,
        _source: &bun_ast::Source,
        _import_range: bun_ast::Range,
        _caller: bun_ast::Expr,
        _function_name: &[u8],
    ) -> Result<bun_ast::Expr, bun_core::Error> {
        log.add_error(
            None,
            bun_ast::Loc::EMPTY,
            b"Macros are not supported in this Bao build",
        );
        Err(bun_core::err!("MacroNotSupported"))
    }

    #[unsafe(no_mangle)]
    pub extern "Rust" fn __bun_macro_context_get_remap(
        data: *mut c_void,
        _path: &[u8],
    ) -> Option<&'static bun_js_parser::Macro::MacroRemapEntry> {
        if data.is_null() {
            return None;
        }
        unreachable!("Bao macro bridge: remap lookup on a foreign MacroContext");
    }

    #[unsafe(no_mangle)]
    pub extern "Rust" fn __bun_macro_collect_vm_garbage() {}

    // ── bundler bytecode / HMR faces — owners: bao_bundler::bytecode /
    // bao_bundler::hmr (@trace REQ-CLI-001), both Phase-1.

    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_jsc_generate_cached_bytecode(
        _format: bun_bundler::options_impl::Format,
        _source: &[u8],
        _source_provider_url: &bun_core::String,
    ) -> Option<Box<[u8]>> {
        // Phase 1: no SM bytecode cache. Return None so bundler emits source text.
        None
    }

    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_jsc_enable_hot_module_reloading_for_bundler(
        _bv2: core::ptr::NonNull<bun_bundler::BundleV2<'static>>,
    ) {
        // Phase 1: HMR not yet implemented for SM.
    }
}
