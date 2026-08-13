//! Cold-path vtable hook structures for SpiderMonkey.
//!
//! These structs must have the same fn-pointer layout as the ones defined in
//! `bun_jsc` (`VirtualMachine::RuntimeHooks`, `ModuleLoader::LoaderHooks`,
//! `bun_sql_jsc::jsc::SqlRuntimeHooks`) so the runtime tier can populate the
//! static instances (upstream populator: Bun's `jsc_hooks.rs`; no Bao
//! populator is wired yet).
//!
//! # Layout compatibility
//!
//! Each struct is a POD fn-pointer table (no `#[repr(C)]` needed — Rust
//! representation is the same for all-pointer fields). The field names and
//! signatures match the JSC originals exactly; `bun_runtime` fills them at
//! link time via `#[no_mangle]` statics.

use core::ffi::c_void;

use crate::global_object::JSGlobalObject;
use crate::js_promise::JSInternalPromise;
use crate::js_value::JSValue;
use crate::virtual_machine::VirtualMachine;

/// Type alias for VM pointer used in hook signatures.
pub type VMPtr = *mut VirtualMachine;
/// Type alias for global pointer used in hook signatures.
pub type GlobalPtr = *mut JSGlobalObject;
/// Opaque per-VM state owned by `bao_runtime`.
/// Stored as `*mut c_void` in `VirtualMachine`; the high tier casts back.
pub type RuntimeState = *mut c_void;

/// Runtime hooks — 28+ function pointer slots.
///
/// Populated by the runtime tier at startup (upstream: Bun's
/// `jsc_hooks.rs`, ~5290 lines; no Bao populator is wired yet).
/// Each field mirrors the corresponding `bun_jsc::VirtualMachine::RuntimeHooks`
/// fn-pointer slot. Fields that reference JSC-specific types use SM-compatible
/// equivalents; the signatures are ABI-identical since all types are pointer-width.
pub struct RuntimeHooks {
    pub init_runtime_state:
        unsafe fn(vm: *mut VirtualMachine, opts: *mut core::ffi::c_void) -> RuntimeState,
    pub deinit_runtime_state: unsafe fn(vm: *mut VirtualMachine, state: RuntimeState),
    pub generate_entry_point: fn(vm: *const VirtualMachine, watch: bool, entry_path: &[u8]) -> bool,
    pub load_preloads:
        unsafe fn(vm: *mut VirtualMachine) -> Result<*mut JSInternalPromise, bun_core::Error>,
    pub ensure_debugger: unsafe fn(vm: *mut VirtualMachine, block_until_connected: bool),
    pub auto_tick: unsafe fn(vm: *mut VirtualMachine),
    pub auto_tick_active: unsafe fn(vm: *mut VirtualMachine),
    pub print_exception: fn(vm: *mut VirtualMachine, value: JSValue, exception_list: *mut c_void),
    pub timer_insert: unsafe fn(
        vm: *mut VirtualMachine,
        timer: *mut bun_event_loop::EventLoopTimer::EventLoopTimer,
    ),
    pub timer_remove: unsafe fn(
        vm: *mut VirtualMachine,
        timer: *mut bun_event_loop::EventLoopTimer::EventLoopTimer,
    ),
    pub default_client_ssl_ctx: unsafe fn(vm: *mut VirtualMachine) -> *mut c_void,
    pub ssl_ctx_cache_get_or_create: unsafe fn(
        vm: *mut VirtualMachine,
        opts: *const c_void,
        err: *mut c_void,
    ) -> Option<*mut c_void>,
    pub create_node_fs: unsafe fn(vm: *mut VirtualMachine) -> *mut c_void,
    pub has_blob_url: fn(blob_id: &[u8]) -> bool,
    pub body_mixin_get_blob:
        fn(value: JSValue, global: *mut JSGlobalObject) -> Result<Option<JSValue>, bun_core::Error>,
    pub process_exit: unsafe fn(global: *mut JSGlobalObject, code: u8),
    pub handle_ipc_internal_child: unsafe fn(global: *mut JSGlobalObject, data: JSValue),
    pub ipc_child_singleton_deinit: fn(),
    pub console_on_before_print: fn(),
    pub console_print_runtime_object: unsafe fn(
        formatter: *mut c_void,
        writer: *mut c_void,
        value: JSValue,
        name_buf: &[u8; 512],
        enable_ansi_colors: bool,
    ) -> Result<bool, bun_core::Error>,
    pub apply_standalone_runtime_flags: unsafe fn(transpiler: *mut c_void, graph: *const c_void),
    pub parse_worker_exec_argv_allow_addons: unsafe fn(exec_argv: *const c_void) -> Option<bool>,
    pub cron_clear_all_teardown: fn(vm: *mut VirtualMachine),
    pub terminate_all_workers_and_wait: fn(timeout_ms: u64),
    pub cron_clear_all_reload: fn(vm: *mut VirtualMachine),
    pub load_standalone_sourcemap: fn(path: &[u8]) -> Option<*const c_void>,
    pub bake_per_thread_source_map:
        unsafe fn(pt: *mut c_void, source_filename: &[u8]) -> Option<*const [u8]>,
    pub retroactively_report_discovered_tests: unsafe fn(agent: *mut c_void),
    pub cancel_all_timers: unsafe fn(vm: *mut VirtualMachine),
}

/// Loader hooks — 7 function pointer slots.
///
/// Mirrors `bun_jsc::ModuleLoader::LoaderHooks`. Populated by the runtime
/// tier (upstream: Bun's `jsc_hooks.rs`; no Bao populator is wired yet).
pub struct LoaderHooks {
    pub transpile_source_code:
        unsafe fn(vm: *mut VirtualMachine, args: *const c_void, ret: *mut c_void) -> bool,
    pub fetch_builtin_module: unsafe fn(
        vm: *mut VirtualMachine,
        global: *mut JSGlobalObject,
        specifier: *const bun_core::String,
        referrer: *const bun_core::String,
        out: *mut c_void,
    ) -> u8,
    pub get_hardcoded_module: unsafe fn(
        vm: *mut VirtualMachine,
        specifier: *const bun_core::String,
        hardcoded: u32,
        out: *mut c_void,
    ) -> bool,
    pub resolve_embedded_node_file:
        unsafe fn(vm: *mut VirtualMachine, in_out_str: *mut bun_core::String) -> bool,
    pub resolve: unsafe fn(
        res: *mut c_void,
        global: *mut JSGlobalObject,
        specifier: bun_core::String,
        source: bun_core::String,
        query_string: *mut bun_core::String,
        is_esm: bool,
        is_a_file_path: bool,
        is_user_require_resolve: bool,
    ) -> bool,
    pub transpile_virtual_module: unsafe fn(
        global: *mut JSGlobalObject,
        specifier: *const bun_core::String,
        referrer: *const bun_core::String,
        source_code: *mut c_void,
        loader: u8,
        ret: *mut c_void,
    ) -> bool,
    pub transpile_file: unsafe fn(
        vm: *mut VirtualMachine,
        global: *mut JSGlobalObject,
        specifier: *const bun_core::String,
        referrer: *const bun_core::String,
        type_attribute: *const bun_core::String,
        ret: *mut c_void,
        allow_promise: bool,
        is_commonjs_require: bool,
        force_loader: u8,
    ) -> *mut c_void,
}

/// SQL runtime hooks — 14 function pointer slots.
///
/// Mirrors `bun_sql_jsc::jsc::SqlRuntimeHooks`. Populated by
/// `bao_runtime::hw_exports.rs`.
pub struct SqlRuntimeHooks {
    pub sql_rare: unsafe fn(*mut VirtualMachine) -> *mut c_void,
    pub timer_heap: unsafe fn(*mut VirtualMachine) -> *mut c_void,
    pub timer_insert:
        unsafe fn(heap: *mut c_void, timer: *mut bun_event_loop::EventLoopTimer::EventLoopTimer),
    pub timer_remove:
        unsafe fn(heap: *mut c_void, timer: *mut bun_event_loop::EventLoopTimer::EventLoopTimer),
    pub ssl_ctx_cache: unsafe fn(*mut VirtualMachine) -> *mut c_void,
    pub ssl_ctx_get_or_create:
        unsafe fn(cache: *mut c_void, opts: *const c_void, err: *mut c_void) -> *mut c_void,
    pub ssl_config_from_js: unsafe fn(*mut JSGlobalObject, JSValue) -> *mut c_void,
    pub ssl_config_free: unsafe fn(*mut c_void),
    pub ssl_config_as_usockets_client: unsafe fn(*const c_void) -> *const c_void,
    pub ssl_config_server_name: unsafe fn(*const c_void) -> *const core::ffi::c_char,
    pub ssl_config_reject_unauthorized: unsafe fn(*const c_void) -> i32,
    pub blob_needs_to_read_file: unsafe fn(*const c_void) -> bool,
    pub blob_shared_view: unsafe fn(*const c_void, out_len: *mut usize) -> *const u8,
}

unsafe extern "Rust" {
    safe static __BUN_RUNTIME_HOOKS: RuntimeHooks;
    safe static __BUN_LOADER_HOOKS: LoaderHooks;
    safe static __BUN_SQL_RUNTIME_HOOKS: SqlRuntimeHooks;
}

/// Access the runtime hooks static instance.
#[inline]
pub fn runtime_hooks() -> &'static RuntimeHooks {
    &__BUN_RUNTIME_HOOKS
}

/// Access the loader hooks static instance.
#[inline]
pub fn loader_hooks() -> &'static LoaderHooks {
    &__BUN_LOADER_HOOKS
}

/// Access the SQL runtime hooks static instance.
#[inline]
pub fn sql_runtime_hooks() -> &'static SqlRuntimeHooks {
    &__BUN_SQL_RUNTIME_HOOKS
}
