//! SpiderMonkey-backed API compatibility layer replacing `bun_jsc`.
//!
//! `bun_sm` provides the same public API surface as the phantom `bun_jsc` crate,
//! but backed by SpiderMonkey via `bao_engine` + `mozjs`. When `bun_runtime`
//! changes `pub use bun_jsc::*` to `pub use bun_sm::*`, all 8,500+ JSC
//! references resolve to SM-backed implementations.
//!
//! # Architecture
//!
//! JSC types map to SM equivalents:
//! - `JSGlobalObject` → `*mut JSContext` (SM's JSContext owns the heap)
//! - `JSValue` → newtype over `bun_sm::value::JsValue`
//! - `CallFrame` → wrapper over SM `CallArgs`
//! - `VirtualMachine` → TLS singleton wrapping `BaoEventLoop`
//! - `JSPromise` → ZST wrapping `*mut JSObject`
//!
//! # ABI difference
//!
//! ```text
//! JSC: unsafe extern "C" fn(*mut JSGlobalObject, *mut CallFrame) -> JSValue
//! SM:  unsafe extern "C" fn(*mut JSContext, argc: u32, vp: *mut JSVal) -> bool
//! ```

// ─── Core SM types (moved from bao_engine) ───────────────────────────────
pub mod dispatch_sm;
pub mod error;
pub mod gc;
pub mod value;

// ─── Core value types ────────────────────────────────────────────────────
pub mod call_frame;
pub mod global_object;
pub mod js_type;
pub mod js_value;
pub mod virtual_machine;

// ─── Error system ────────────────────────────────────────────────────────
pub mod error_code;
pub mod js_error;
pub mod js_terminated;

// ─── Promise system ──────────────────────────────────────────────────────
pub mod js_promise;

// ─── Host function ABI ──────────────────────────────────────────────────
pub mod host_fn;
pub mod js_class;

// ─── GC rooting ──────────────────────────────────────────────────────────
pub mod arguments;
pub mod ensure_alive;
pub mod strong;

// ─── String types ────────────────────────────────────────────────────────

// ─── Cell / ArrayBuffer ──────────────────────────────────────────────────
pub mod array_buffer;
pub mod js_cell;

// ─── Event loop ──────────────────────────────────────────────────────────
pub mod event_loop;

// ─── Concurrent tasks ────────────────────────────────────────────────────
pub mod concurrent;

// ─── Codegen / generated ─────────────────────────────────────────────────
pub mod codegen;
pub mod generated;

// ─── Macros ──────────────────────────────────────────────────────────────
pub mod mark_binding;

// ─── Miscellaneous types ─────────────────────────────────────────────────
pub mod abort_signal;
pub mod async_module;
pub mod builtin_name;
pub mod bun_cpu_profiler;
pub mod c_api;
pub mod code_coverage;
pub mod common_strings;
pub mod console_object;
pub mod counters;
pub mod create_utf;
pub mod fetch_headers;
pub mod format_tag;
pub mod global_ref;
pub mod host_call;
pub mod hot_reloader;
pub mod initialize;
pub mod ipc;
pub mod js_object;
pub mod module_loader;
pub mod node_path;
pub mod rare_data;
pub mod regular_expression;
pub mod resolved_source;
pub mod runtime_transpiler_cache;
pub mod system_error;
pub mod thread_safe;
pub mod unprotect;
pub mod validation_scope;
pub mod weak;
pub mod webcore_types;
pub mod work_task;

// ─── Re-export proc-macro attributes ────────────────────────────────────
pub use bao_engine_macros::codegen_cached_accessors;
pub use bao_engine_macros::host_fn;

// ─── Top-level type aliases for bun_jsc compatibility ────────────────────
pub use arguments::{ArgumentsSlice, MarkedArgumentBuffer};
pub use array_buffer::ArrayBuffer;
pub use builtin_name::BuiltinName;
pub use call_frame::CallFrame;
pub use common_strings::CommonStrings;
pub use concurrent::{AnyTask, ConcurrentTask, WorkPoolTask};
pub use console_object::{ConsoleFormatter, ConsoleObject};
pub use ensure_alive::EnsureStillAlive;
pub use error::JsError;
pub use error_code::ErrorCode;
pub use event_loop::EventLoop as SmEventLoop;
pub use event_loop::EventLoopHandle;
pub use global_object::JSGlobalObject;
pub use global_object::RangeErrorOptions;
pub use global_object::node_realm_options;
pub use js_cell::JsCell;
pub use js_class::JsClass;
pub use js_promise::js_promise as js_promise_mod;
pub use js_promise::{AnyPromise, JSInternalPromise, JSPromise, PromiseResult};
pub use js_terminated::JsTerminated;
pub use js_type::JSType;
pub use js_value::JSValue;
pub use js_value::RawJSValue;
pub use strong::{JSPromiseStrong, Strong, StrongOptional};
pub use value::JsValue;
pub use value::jsval_to_jsvalue;
pub use virtual_machine::VirtualMachine;
// mark_binding! is available as a #[macro_export] macro via the mark_binding module
pub use abort_signal::AbortSignal;
pub use bun_cpu_profiler::{BunCpuProfiler, CPUProfilerConfig};
pub use counters::{Counters, create_counters_object};
pub use fetch_headers::FetchHeaders;
pub use host_call::{from_js_host_call, to_js_host_call, to_js_host_fn_result};
pub use host_fn::JSHostFn;
pub use hot_reloader::{HotReloader, ImportWatcher};
pub use module_loader::{
    GlobalSetupFn, JobQueueDrainFn, ModuleLoader, PostEvalHook, ResolverFn, set_job_queue_drain,
};
pub use system_error::SysErrorJsc;
pub use system_error::SystemError;
// NOTE: WebWorker re-export removed per DEC-WK-001 BCE-20260627-008.
// Workers now route through servo's native Worker::Constructor via register_worker_scope_callback.
pub use async_module::{AsyncModule, InitOpts as AsyncModuleInitOpts, Queue as AsyncModuleQueue};
pub use c_api::c;
pub use code_coverage::CodeCoverage;
pub use create_utf::create_utf;
pub use format_tag::FormatTag;
pub use global_ref::{GlobalData, GlobalRef};
pub use initialize::{eval_and_print, initialize};
pub use resolved_source::{OwnedResolvedSource, ResolvedSource, Tag as ResolvedSourceTag};
pub use runtime_transpiler_cache::{
    Entry as TranspilerCacheEntry, IS_DISABLED as TRANSPILER_CACHE_IS_DISABLED,
    RuntimeTranspilerCache, RuntimeTranspilerStore, TranspilerCacheImplKind,
};
pub use thread_safe::ThreadSafe;
pub use unprotect::Unprotect;
pub use validation_scope::ValidationScope;
pub use virtual_machine::VirtualMachineRef;

// ─── Dispatch / event loop ──────────────────────────────────────────────
pub use dispatch_sm::BaoEventLoop;

// ─── Codegen / generated ────────────────────────────────────────────────
pub use codegen::{ClassDef, GeneratedBindings, ParseResult, PropertyDef, PropertyKind};
pub use generated::{BracesOptions, GenList, GenOpt, GenVal, ProcessConfigOptions};

// ─── Rare data ──────────────────────────────────────────────────────────
pub use rare_data::{HotMap, HotMapEntry, RareData};

// ─── IPC ────────────────────────────────────────────────────────────────
pub use ipc::{IpcChannel, IpcDirection, IpcError, IpcMessage, IpcState};

// ─── WebCore types ──────────────────────────────────────────────────────
pub use webcore_types::{
    CacheMode, DomNodeType, EventPhase, RedirectMode, ReferrerPolicy, RequestMode, ResponseType,
};

// ─── JS object wrappers ─────────────────────────────────────────────────
pub use js_object::{JSArray, JSArrayIterator, JSBigInt, JSFunction, JSObject, JSString};

// ─── Weak references ────────────────────────────────────────────────────
pub use weak::{Weak, WeakRefType};

// ─── Regular Expression ─────────────────────────────────────────────────
pub use regular_expression::{Flags as RegExpFlags, RegularExpression};

// ─── Constants ────────────────────────────────────────────────────────────
pub const MAX_SAFE_INTEGER: f64 = 9007199254740991.0;
pub const MIN_SAFE_INTEGER: f64 = -9007199254740991.0;

// Test-binary link seams (root cause: BCE duplicate-symbol, 5d3c1f8a wave).
//
// The soft-link faces this crate's dependency closure references resolve
// against higher-tier providers (`bao_bundler`, `bun_runtime`) — every one of
// them depends on `bun_sm` itself. Referencing such a dev-dep from the lib
// test unit puts its whole closure on the link line, including a *second*
// copy of the `bun_sm` rlib next to the test compilation unit → lld-link/COFF
// dies on the duplicated `#[no_mangle]` dispatch bodies
// (`__bun_dispatch__JsEventLoop__Jsc__*` / `__bun_dispatch__EventLoopCtx__Js__*`).
// So the higher-tier dev-deps must stay OFF this link line and the faces are
// supplied locally — same shape as the `bun_core` lib.rs test seams
// (ErrnoNames absent-table / uv_get_osfhandle = UCRT `_get_osfhandle` / OOM
// abort) and the `io/tests/pipe_reader_reentrancy.rs` `__bun_get_vm_ctx` seam.
// Every body below mirrors its production owner 1:1 (all of these owners are
// themselves Phase-1 honest-degraded bodies: no plugins, no bake dev server,
// no bytecode cache, no transpiler cache); the owner file is named at each
// group. The link itself is the resolver proof (any missing face = loud
// undefined-symbol, not a silent fallback).
#[cfg(test)]
#[allow(non_snake_case)]
mod test_link_seams {
    use core::cell::Cell;
    use core::ffi::c_void;

    use bun_bundler::bundle_v2::JSBundlerPlugin;
    use bun_core::String as BunString;
    use bun_sys::{Fd, Mode};

    // ── webcore faces (2) ────────────────────────────────────────────────
    // Owner: `bun_runtime::webcore_faces` (event_loop::MiniEventLoop upward
    // externs).

    // Real provider boxes a `StdioBlobStore` with intrusive refcount = 2
    // (event-loop slot + runtime-side holder); `bun_core` only stores /
    // forwards the erased pointer. In this test binary the runtime-side
    // holder never exists, so the allocation intentionally lives for the
    // process lifetime — no tier-6 cast-back is reachable here.
    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_stdio_blob_store_new(fd: Fd, is_atty: bool, mode: Mode) -> *mut () {
        struct StdioBlobStore {
            ref_count: Cell<u32>,
            fd: Fd,
            is_atty: bool,
            mode: Mode,
        }
        let store = Box::new(StdioBlobStore {
            ref_count: Cell::new(2),
            fd,
            is_atty,
            mode,
        });
        Box::into_raw(store) as *mut ()
    }

    // Identical body to the real provider — SM's TLS `Runtime`/`JSContext`
    // erased to `*mut ()`; null = no Runtime on this thread (the caller's
    // documented degraded case).
    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_js_vm_get() -> *mut () {
        use mozjs::rust::Runtime;
        Runtime::get().map_or(::std::ptr::null_mut(), |runtime| runtime.as_ptr() as *mut ())
    }

    // ── JSBundlerPlugin faces (6) ────────────────────────────────────────
    // Owner: `bao_bundler::js_bundler_plugin_seam` (@trace REQ-ENG-006).
    // Bao registers no bundler plugins and every BundleV2 built through
    // build_api passes `plugins: None`, so these are the owner's honest
    // **no-plugins** answers, mirrored 1:1.

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

    // ── VmLoaderCtx[Runtime] faces (12 referenced in this closure) ───────
    // Owner: `bao_bundler::vm_loader` `link_impl_VmLoaderCtx!` (BaoVmLoaderCtx,
    // @trace REQ-ENG-005) — Phase-1 bodies mirrored 1:1. origin_host /
    // origin_path / main return the owner's thread-local defaults (and its
    // `set_vm_loader_ctx` is a Phase-1 no-op, so those constants ARE the
    // production values). `link_noop_VmLoaderCtx!` cannot supply this
    // interface (origin_host/origin_path/main/blob_shared_view return
    // `&'static [u8]`, which has no `Default`), so the bodies are spelled by
    // hand against the interface's own ret/arg aliases (exact ABI by
    // construction). The 13th method (`blob_deinit`) is not referenced by
    // this closure.

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

    // ── get_vm_ctx face ──────────────────────────────────────────────────
    // Owner: `bun_runtime::dispatch::__bun_get_vm_ctx`. Both arms are
    // mirrorable locally: the Js arm's `BaoEventLoop` lives in THIS crate
    // (`dispatch_sm`), and the Mini arm's `MiniEventLoop` is a normal dep
    // (`bun_event_loop`). Same body as the owner.

    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_get_vm_ctx(kind: bun_io::AllocatorType) -> bun_io::EventLoopCtx {
        match kind {
            bun_io::AllocatorType::Mini => {
                // Prefer an already-published Mini loop; otherwise init the
                // thread-local singleton (install / non-JS paths).
                let ptr = bun_event_loop::MiniEventLoop::GLOBAL.with(|g| g.get());
                let ptr = if ptr.is_null() {
                    bun_event_loop::MiniEventLoop::init_global(None, None)
                } else {
                    ptr
                };
                // SAFETY: GLOBAL / init_global guarantee a live MiniEventLoop
                // for this thread; EventLoopCtx holds a raw owner pointer only.
                unsafe { bun_io::EventLoopCtx::new(bun_io::EventLoopCtxKind::Mini, ptr) }
            }
            bun_io::AllocatorType::Js => {
                // SpiderMonkey BaoEventLoop (the `EventLoopCtx` Js arm is
                // registered by this crate's dispatch_sm).
                let cell = crate::dispatch_sm::BaoEventLoop::current();
                let owner_ptr = cell as *const _ as *mut core::ffi::c_void;
                // SAFETY: current() returns the live thread-local BaoEventLoop.
                unsafe { bun_io::EventLoopCtx::new(bun_io::EventLoopCtxKind::Js, owner_ptr) }
            }
        }
    }

    // ── bundler bytecode / HMR faces ─────────────────────────────────────
    // Owners: `bao_bundler::bytecode` / `bao_bundler::hmr` (@trace
    // REQ-CLI-001) — both Phase-1: no SM bytecode cache (bundler emits source
    // text) and no HMR watcher. Mirrored 1:1.

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

    // ── TranspilerCacheImpl[Jsc] face ────────────────────────────────────
    // Owner: `bao_bundler::transpiler_cache_seam` (@trace REQ-ENG-006) — the
    // honest disabled semantics: `put` stores nothing (Bao has no on-disk
    // transpiler cache and the `r#impl` slot stays `None`). Only `put` is
    // referenced by this closure.

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

    // ── macro-context faces (5) ──────────────────────────────────────────
    // Owner: `bao_bundler::macro_context` — the honest **no-macro-state**
    // semantics (Bao has no macro VM): init encodes a null-`data` context,
    // remap lookups short-circuit to None, call fails closed with an explicit
    // build error, deinit / gc are no-ops for that state. Mirrored 1:1.

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

    // ── package-name regex faces (3) ─────────────────────────────────────
    // Owner: `bun_runtime::product_native_symbols` — real `regex`-crate
    // bodies for PnpmMatcher. Mirrored 1:1 over the same `regex` version.

    #[unsafe(no_mangle)]
    pub fn __bun_regex_compile(pattern: BunString) -> Option<core::ptr::NonNull<()>> {
        let utf8 = pattern.to_utf8_without_ref();
        let bytes = utf8.slice();
        // SAFETY: package-name regex patterns are ASCII (escape_reg_exp output).
        let pat = core::str::from_utf8(bytes).ok()?;
        let re = regex::Regex::new(pat).ok()?;
        let boxed = Box::new(re);
        // SAFETY: freshly allocated; unique owner until __bun_regex_drop.
        Some(unsafe { core::ptr::NonNull::new_unchecked(Box::into_raw(boxed) as *mut ()) })
    }

    #[unsafe(no_mangle)]
    pub fn __bun_regex_matches(regex_handle: core::ptr::NonNull<()>, input: &BunString) -> bool {
        // SAFETY: regex was produced by __bun_regex_compile.
        let re = unsafe { &*(regex_handle.as_ptr() as *const regex::Regex) };
        let utf8 = input.to_utf8_without_ref();
        let Ok(s) = core::str::from_utf8(utf8.slice()) else {
            return false;
        };
        re.is_match(s)
    }

    #[unsafe(no_mangle)]
    pub fn __bun_regex_drop(regex_handle: core::ptr::NonNull<()>) {
        // SAFETY: unique owner from __bun_regex_compile.
        unsafe {
            drop(Box::from_raw(regex_handle.as_ptr() as *mut regex::Regex));
        }
    }

    // ── BufferedReaderParentLink faces (13 variants, full interface) ─────
    // Owner: `bun_runtime` / `bun_install` `buffered_reader_parent_link!`
    // registrations — those parent types don't exist in this test binary, and
    // once any dispatcher of this interface is codegen'd the match references
    // every variant arm, so the full closed set must resolve. Honest closed-
    // set "no parent wired" answers (the same bodies `link_noop_` would
    // generate, except `event_loop`, which `link_noop_` cannot supply —
    // `EventLoopCtx` has no `Default`): it hands back the thread's Mini loop
    // (same body as `__bun_get_vm_ctx`'s Mini arm) so any stray caller still
    // gets a live owner (io/tests/pipe_reader_reentrancy.rs precedent).

    fn mini_event_loop_ctx() -> bun_io::EventLoopCtx {
        let ptr = bun_event_loop::MiniEventLoop::GLOBAL.with(|g| g.get());
        let ptr = if ptr.is_null() {
            bun_event_loop::MiniEventLoop::init_global(None, None)
        } else {
            ptr
        };
        // SAFETY: GLOBAL / init_global guarantee a live MiniEventLoop for this
        // thread; EventLoopCtx holds a raw owner pointer only.
        unsafe { bun_io::EventLoopCtx::new(bun_io::EventLoopCtxKind::Mini, ptr) }
    }

    macro_rules! pl_event_loop {
        ($($f:ident),* $(,)?) => {
            $( #[unsafe(no_mangle)] unsafe fn $f(__owner: *mut ()) -> bun_io::EventLoopCtx {
                let _ = __owner;
                mini_event_loop_ctx()
            } )*
        };
    }
    macro_rules! pl_has_on_read_chunk {
        ($($f:ident),* $(,)?) => {
            $( #[unsafe(no_mangle)] unsafe fn $f(__owner: *mut ()) -> bool {
                let _ = __owner;
                false
            } )*
        };
    }
    macro_rules! pl_on_read_chunk {
        ($($f:ident),* $(,)?) => {
            $( #[unsafe(no_mangle)] unsafe fn $f(
                __owner: *mut (),
                __on_read_chunk_chunk: &[u8],
                __on_read_chunk_has_more:
                    bun_io::__BufferedReaderParentLink__on_read_chunk__arg_has_more<'_>,
            ) -> bool {
                let _ = (__owner, __on_read_chunk_chunk, __on_read_chunk_has_more);
                false
            } )*
        };
    }
    macro_rules! pl_on_reader_done {
        ($($f:ident),* $(,)?) => {
            $( #[unsafe(no_mangle)] unsafe fn $f(__owner: *mut ()) {
                let _ = __owner;
            } )*
        };
    }
    macro_rules! pl_on_reader_error {
        ($($f:ident),* $(,)?) => {
            $( #[unsafe(no_mangle)] unsafe fn $f(__owner: *mut (), __on_reader_error_err: bun_sys::Error) {
                let _ = (__owner, __on_reader_error_err);
            } )*
        };
    }
    macro_rules! pl_loop_ptr {
        ($($f:ident),* $(,)?) => {
            $( #[unsafe(no_mangle)] unsafe fn $f(
                __owner: *mut (),
            ) -> bun_io::__BufferedReaderParentLink__loop_ptr__ret {
                let _ = __owner;
                core::ptr::null_mut()
            } )*
        };
    }
    macro_rules! pl_on_max_buffer_overflow {
        ($($f:ident),* $(,)?) => {
            $( #[unsafe(no_mangle)] unsafe fn $f(
                __owner: *mut (),
                __on_max_buffer_overflow_maxbuf:
                    bun_io::__BufferedReaderParentLink__on_max_buffer_overflow__arg_maxbuf<'_>,
            ) {
                let _ = (__owner, __on_max_buffer_overflow_maxbuf);
            } )*
        };
    }

    pl_event_loop!(
        __bun_dispatch__BufferedReaderParentLink__SubprocessPipeReader__event_loop,
        __bun_dispatch__BufferedReaderParentLink__ShellPipeReader__event_loop,
        __bun_dispatch__BufferedReaderParentLink__ShellIoReader__event_loop,
        __bun_dispatch__BufferedReaderParentLink__FileReader__event_loop,
        __bun_dispatch__BufferedReaderParentLink__FileResponseStream__event_loop,
        __bun_dispatch__BufferedReaderParentLink__Terminal__event_loop,
        __bun_dispatch__BufferedReaderParentLink__CronRegister__event_loop,
        __bun_dispatch__BufferedReaderParentLink__CronRemove__event_loop,
        __bun_dispatch__BufferedReaderParentLink__FilterRunHandle__event_loop,
        __bun_dispatch__BufferedReaderParentLink__MultiRunPipeReader__event_loop,
        __bun_dispatch__BufferedReaderParentLink__TestParallelWorkerPipe__event_loop,
    );
    pl_has_on_read_chunk!(
        __bun_dispatch__BufferedReaderParentLink__SubprocessPipeReader__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__ShellPipeReader__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__ShellIoReader__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__FileReader__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__FileResponseStream__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__Terminal__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__CronRegister__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__CronRemove__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__FilterRunHandle__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__MultiRunPipeReader__has_on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__TestParallelWorkerPipe__has_on_read_chunk,
    );
    pl_on_read_chunk!(
        __bun_dispatch__BufferedReaderParentLink__SubprocessPipeReader__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__ShellPipeReader__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__ShellIoReader__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__FileReader__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__FileResponseStream__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__Terminal__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__CronRegister__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__CronRemove__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__FilterRunHandle__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__MultiRunPipeReader__on_read_chunk,
        __bun_dispatch__BufferedReaderParentLink__TestParallelWorkerPipe__on_read_chunk,
    );
    pl_on_reader_done!(
        __bun_dispatch__BufferedReaderParentLink__SubprocessPipeReader__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__ShellPipeReader__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__ShellIoReader__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__FileReader__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__FileResponseStream__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__Terminal__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__CronRegister__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__CronRemove__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__FilterRunHandle__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__MultiRunPipeReader__on_reader_done,
        __bun_dispatch__BufferedReaderParentLink__TestParallelWorkerPipe__on_reader_done,
    );
    pl_on_reader_error!(
        __bun_dispatch__BufferedReaderParentLink__SubprocessPipeReader__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__ShellPipeReader__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__ShellIoReader__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__FileReader__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__FileResponseStream__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__Terminal__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__CronRegister__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__CronRemove__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__FilterRunHandle__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__MultiRunPipeReader__on_reader_error,
        __bun_dispatch__BufferedReaderParentLink__TestParallelWorkerPipe__on_reader_error,
    );
    pl_loop_ptr!(
        __bun_dispatch__BufferedReaderParentLink__SubprocessPipeReader__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__ShellPipeReader__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__ShellIoReader__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__FileReader__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__FileResponseStream__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__Terminal__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__CronRegister__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__CronRemove__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__FilterRunHandle__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__MultiRunPipeReader__loop_ptr,
        __bun_dispatch__BufferedReaderParentLink__TestParallelWorkerPipe__loop_ptr,
    );
    pl_on_max_buffer_overflow!(
        __bun_dispatch__BufferedReaderParentLink__SubprocessPipeReader__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__ShellPipeReader__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__ShellIoReader__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__FileReader__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__FileResponseStream__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__Terminal__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__CronRegister__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__CronRemove__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__FilterRunHandle__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__MultiRunPipeReader__on_max_buffer_overflow,
        __bun_dispatch__BufferedReaderParentLink__TestParallelWorkerPipe__on_max_buffer_overflow,
    );

    // ── C-seam faces the uWS C++ archive references ──────────────────────
    // The uWS archive (bun_uws → bun_uws_sys) is in this closure and its
    // HttpContext members extern `Bun__NodeHTTP__onReadsResumable`, owned by
    // `bun_runtime::node_http` — whose production body is itself an empty
    // no-op, mirrored 1:1. (`BUN_DEFAULT_MAX_HTTP_HEADER_SIZE` is NOT
    // mirrored: bun_http comes onto this link line via the bun_install
    // dev-dep and owns the exported static.) C archives / loop symbols
    // (bao_uloop, lsquic, boringssl) come from the bao_native_stubs
    // test-linking dev-dep.

    #[unsafe(no_mangle)]
    pub extern "C" fn Bun__NodeHTTP__onReadsResumable(
        _ssl: core::ffi::c_int,
        _socket: *mut c_void,
    ) {
    }

    // ── ProcessExit faces (9 variants) ───────────────────────────────────
    // Owner: `bun_runtime::product_process_exit` (the real exit-propagation
    // handlers). LifecycleScript / SecurityScan / SyncWindows resolve from
    // bun_install / bun_spawn; the remaining nine have no parent object in
    // this test binary, so the closed-set no-op bodies (exactly what the
    // interface's own `link_noop_` machinery generates — its only method
    // returns `()`) are the honest absent-observer supply.

    bun_spawn::link_noop_ProcessExit!(
        Subprocess,
        Shell,
        ChromeProcess,
        HostProcess,
        CronRegister,
        CronRemove,
        FilterRunHandle,
        MultiRunHandle,
        TestParallelWorker
    );

    // ── DNS prefetch face ────────────────────────────────────────────────
    // Owner: `bun_runtime::dispatch::__bun_dns_prefetch` — a self-contained
    // std-only performance hint (fire-and-forget getaddrinfo). Mirrored 1:1.

    #[unsafe(no_mangle)]
    pub unsafe extern "Rust" fn __bun_dns_prefetch(
        _loop_: *mut c_void,
        hostname: *const u8,
        len: usize,
        port: u16,
    ) {
        if hostname.is_null() || len == 0 {
            return;
        }
        // SAFETY: bun_dns::prefetch passes a live NUL-or-length-bounded hostname slice.
        let bytes = unsafe { core::slice::from_raw_parts(hostname, len) };
        let Ok(host) = core::str::from_utf8(bytes) else {
            return;
        };
        // Skip empty / obviously invalid hosts.
        if host.is_empty() || host.contains('\0') {
            return;
        }
        let host = host.to_owned();
        // Fire-and-forget OS resolve into the process DNS cache (libc getaddrinfo).
        let _ = ::std::thread::Builder::new()
            .name("bao-dns-prefetch".into())
            .spawn(move || {
                use std::net::ToSocketAddrs;
                let _ = (host.as_str(), port).to_socket_addrs();
            });
    }

    // ── WindowsNamedPipe faces (12, windows only) ────────────────────────
    // Owner: `bun_runtime::socket` (the real named-pipe backend). No pipe is
    // ever constructed in this test binary, so the closed-pipe posture is the
    // honest degraded supply: queries answer "closed / never established",
    // writes report failure, mutators no-op, and TLS answers mirror the
    // owner's own never-TLS constants (pipes never TLS-upgrade → null / no
    // error). Signatures mirror `bun_uws_sys`'s extern block one-for-one.

    #[cfg(windows)]
    mod windows_named_pipe {
        use bun_boringssl_sys::SSL;
        use bun_uws_sys::{us_bun_verify_error_t, WindowsNamedPipe};

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__ssl(this: &WindowsNamedPipe) -> *mut SSL {
            let _ = this;
            // Pipes never TLS-upgrade (owner parity): null.
            ::std::ptr::null_mut()
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__ssl_error(this: &WindowsNamedPipe) -> us_bun_verify_error_t {
            let _ = this;
            // No TLS → no verification error (owner parity: zero-valued).
            us_bun_verify_error_t::default()
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__is_established(_this: &WindowsNamedPipe) -> bool {
            false
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__is_closed(_this: &WindowsNamedPipe) -> bool {
            true
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__is_shutdown(_this: &WindowsNamedPipe) -> bool {
            true
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__pause_stream(_this: &mut WindowsNamedPipe) -> bool {
            // No pipe → readStop "fails" (owner parity for the closed path).
            false
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__resume_stream(_this: &mut WindowsNamedPipe) -> bool {
            false
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__flush(_this: &mut WindowsNamedPipe) {}

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__set_timeout(
            _this: &mut WindowsNamedPipe,
            _seconds: u32,
        ) {
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__encode_and_write(
            _this: *mut WindowsNamedPipe,
            _ptr: *const u8,
            _len: usize,
        ) -> i32 {
            // Closed pipe: nothing accepted.
            -1
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__raw_write(
            _this: *mut WindowsNamedPipe,
            _ptr: *const u8,
            _len: usize,
        ) -> i32 {
            -1
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__shutdown(_this: &mut WindowsNamedPipe) {}

        #[unsafe(no_mangle)]
        pub extern "C" fn WindowsNamedPipe__close(_this: &mut WindowsNamedPipe) {}
    }

    // ── DevServerHandle[Bake] faces (11) ─────────────────────────────────
    // Owner: `bao_bundler::dev_server` `link_impl_DevServerHandle!`
    // (BaoDevServer, @trace REQ-CLI-001) — no bake dev server exists, so the
    // owner's bodies return no-op defaults and `Err(AllocError)` where the
    // interface demands `Result` (`link_noop_` cannot generate those, same
    // limitation documented at the owner). Mirrored 1:1, typed through the
    // interface's own aliases.

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

    // Link anchor (bao_bundler::force_link_test_seams twin): reference every
    // supplied face so they stay live in test binaries that never touch these
    // paths. With `-C link-arg=/OPT:NOREF` nothing is GC'd, but the anchor
    // keeps the contract explicit and independent of that flag.
    #[test]
    fn link_higher_tier_seams() {
        // Test-linking anchor: drags the C archives + bao_uloop seam symbols
        // onto the link line (bun_native-stubs leg, see Cargo.toml).
        bao_native_stubs::force_link();
        // Same leg as bun_runtime::force_link_bun_install: keeps the
        // bun_install compilation unit (owner of
        // `__bun_resolver_init_package_manager`) on the link line.
        let _ = bun_install::Subcommand::Install;
        let store_new: unsafe extern "Rust" fn(Fd, bool, Mode) -> *mut () =
            __bun_stdio_blob_store_new;
        let vm_get: unsafe extern "Rust" fn() -> *mut () = __bun_js_vm_get;
        let plugin_fns = (
            JSBundlerPlugin__anyMatches as *const (),
            JSBundlerPlugin__matchOnLoad as *const (),
            JSBundlerPlugin__matchOnResolve as *const (),
            JSBundlerPlugin__drainDeferred as *const (),
            JSBundlerPlugin__hasOnBeforeParsePlugins as *const (),
            JSBundlerPlugin__callOnBeforeParsePlugins as *const (),
        );
        let vm_loader_fns = (
            __bun_dispatch__VmLoaderCtx__Runtime__origin_host as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__origin_path as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__main as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__loaders as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__read_dir_info_package_json as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__is_blob_url as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__resolve_blob as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__eval_source as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__blob_loader as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__blob_file_name as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__blob_needs_read_file as *const (),
            __bun_dispatch__VmLoaderCtx__Runtime__blob_shared_view as *const (),
        );
        let dev_server_fns = (
            __bun_dispatch__DevServerHandle__Bake__barrel_needed_exports as *const (),
            __bun_dispatch__DevServerHandle__Bake__log_for_resolution_failures as *const (),
            __bun_dispatch__DevServerHandle__Bake__finalize_bundle as *const (),
            __bun_dispatch__DevServerHandle__Bake__handle_parse_task_failure as *const (),
            __bun_dispatch__DevServerHandle__Bake__put_or_overwrite_asset as *const (),
            __bun_dispatch__DevServerHandle__Bake__track_resolution_failure as *const (),
            __bun_dispatch__DevServerHandle__Bake__is_file_cached as *const (),
            __bun_dispatch__DevServerHandle__Bake__asset_hash as *const (),
            __bun_dispatch__DevServerHandle__Bake__current_bundle_start_data as *const (),
            __bun_dispatch__DevServerHandle__Bake__register_barrel_with_deferrals as *const (),
            __bun_dispatch__DevServerHandle__Bake__register_barrel_export as *const (),
        );
        let parent_link_fns = (
            __bun_dispatch__BufferedReaderParentLink__SubprocessPipeReader__event_loop as *const (),
            __bun_dispatch__BufferedReaderParentLink__ShellPipeReader__event_loop as *const (),
            __bun_dispatch__BufferedReaderParentLink__ShellIoReader__event_loop as *const (),
            __bun_dispatch__BufferedReaderParentLink__FileReader__event_loop as *const (),
            __bun_dispatch__BufferedReaderParentLink__FileResponseStream__event_loop as *const (),
            __bun_dispatch__BufferedReaderParentLink__Terminal__event_loop as *const (),
            __bun_dispatch__BufferedReaderParentLink__CronRegister__event_loop as *const (),
        );
        ::std::hint::black_box((
            store_new,
            vm_get,
            plugin_fns,
            vm_loader_fns,
            dev_server_fns,
            parent_link_fns,
            __bun_get_vm_ctx as *const (),
            __bun_jsc_generate_cached_bytecode as *const (),
            __bun_jsc_enable_hot_module_reloading_for_bundler as *const (),
            __bun_dispatch__TranspilerCacheImpl__Jsc__put as *const (),
            Bun__NodeHTTP__onReadsResumable as *const (),
        ));
    }
}
