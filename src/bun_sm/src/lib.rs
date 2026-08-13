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
pub mod bun_string;
pub mod string_jsc;

// ─── Cell / ArrayBuffer ──────────────────────────────────────────────────
pub mod array_buffer;
pub mod js_cell;

// ─── Event loop ──────────────────────────────────────────────────────────
pub mod event_loop;

// ─── Runtime hooks (cold-path vtables) ──────────────────────────────────
pub mod runtime_hooks;

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
pub mod debugger;
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
pub use js_cell::JsCell;
pub use js_class::JsClass;
pub use js_promise::js_promise as js_promise_mod;
pub use js_promise::{AnyPromise, JSInternalPromise, JSPromise, PromiseResult};
pub use js_terminated::JsTerminated;
pub use js_type::JSType;
pub use js_value::JSValue;
pub use js_value::RawJSValue;
pub use string_jsc::{StringJsc, ZigStringJsc};
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
pub use runtime_hooks::{LoaderHooks, RuntimeHooks, SqlRuntimeHooks};
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

// ─── Debugger ───────────────────────────────────────────────────────────
pub use debugger::{Breakpoint, Debugger, DebuggerError, SourceInfo};

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
