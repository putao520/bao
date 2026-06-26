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
pub mod value;
pub mod error;
pub mod dispatch_sm;
pub mod gc;

// ─── Core value types ────────────────────────────────────────────────────
pub mod js_value;
pub mod global_object;
pub mod virtual_machine;
pub mod call_frame;
pub mod js_type;

// ─── Error system ────────────────────────────────────────────────────────
pub mod error_code;
pub mod js_error;
pub mod js_terminated;

// ─── Promise system ──────────────────────────────────────────────────────
pub mod js_promise;

// ─── Host function ABI ──────────────────────────────────────────────────
pub mod host_fn;
pub mod js_class;
pub mod jsc_abi;

// ─── GC rooting ──────────────────────────────────────────────────────────
pub mod strong;
pub mod arguments;
pub mod ensure_alive;

// ─── String types ────────────────────────────────────────────────────────
pub mod string_jsc;
pub mod bun_string;

// ─── Cell / ArrayBuffer ──────────────────────────────────────────────────
pub mod js_cell;
pub mod array_buffer;

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
pub mod common_strings;
pub mod host_call;
pub mod debugger;
pub mod abort_signal;
pub mod fetch_headers;
pub mod rare_data;
pub mod ipc;
pub mod builtin_name;
pub mod webcore_types;
pub mod console_object;
pub mod node_path;
pub mod resolved_source;
pub mod system_error;
pub mod options_jsc;
pub mod method_jsc;
pub mod headers_jsc;
pub mod module_loader;
pub mod initialize;
pub mod c_api;
pub mod js_object;
pub mod format_tag;
pub mod thread_safe;
pub mod unprotect;
pub mod global_ref;
pub mod weak;
pub mod create_utf;
pub mod work_task;
pub mod hot_reloader;
pub mod web_worker;
pub mod runtime_transpiler_cache;
pub mod async_module;
pub mod counters;
pub mod code_coverage;
pub mod bun_cpu_profiler;
pub mod validation_scope;
pub mod regular_expression;

// ─── Re-export proc-macro attributes ────────────────────────────────────
pub use bao_engine_macros::host_fn;
pub use bao_engine_macros::codegen_cached_accessors;

// ─── Top-level type aliases for bun_jsc compatibility ────────────────────
pub use value::JsValue;
pub use value::jsval_to_jsvalue;
pub use error::JsError;
pub use js_value::JSValue;
pub use js_value::RawJSValue;
pub use global_object::JSGlobalObject;
pub use global_object::RangeErrorOptions;
pub use virtual_machine::VirtualMachine;
pub use call_frame::CallFrame;
pub use js_type::JSType;
pub use error_code::ErrorCode;
pub use js_terminated::JsTerminated;
pub use js_class::JsClass;
pub use strong::{Strong, StrongOptional, JSPromiseStrong};
pub use arguments::{ArgumentsSlice, MarkedArgumentBuffer};
pub use ensure_alive::EnsureStillAlive;
pub use string_jsc::{StringJsc, ZigStringJsc};
pub use js_cell::JsCell;
pub use array_buffer::ArrayBuffer;
pub use event_loop::{EventLoopHandle};
pub use event_loop::EventLoop as SmEventLoop;
pub use js_promise::{JSPromise, JSInternalPromise, AnyPromise, PromiseResult};
pub use js_promise::js_promise as js_promise_mod;
pub use builtin_name::BuiltinName;
pub use console_object::{ConsoleObject, ConsoleFormatter};
pub use common_strings::CommonStrings;
pub use concurrent::{ConcurrentTask, WorkPoolTask, AnyTask};
// mark_binding! is available as a #[macro_export] macro via the mark_binding module
pub use fetch_headers::FetchHeaders;
pub use abort_signal::AbortSignal;
pub use host_fn::JSHostFn;
pub use host_call::{to_js_host_call, from_js_host_call, to_js_host_fn_result};
pub use runtime_hooks::{RuntimeHooks, LoaderHooks, SqlRuntimeHooks};
pub use system_error::SystemError;
pub use system_error::SysErrorJsc;
pub use module_loader::{ModuleLoader, ResolverFn, GlobalSetupFn, PostEvalHook, JobQueueDrainFn, set_job_queue_drain};
pub use counters::{Counters, create_counters_object};
pub use bun_cpu_profiler::{BunCpuProfiler, CPUProfilerConfig};
pub use hot_reloader::{HotReloader, ImportWatcher};
pub use web_worker::WebWorker;
// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:8]
// ScopeInitFn: callback for installing DedicatedWorkerGlobalScope API
// and stealth properties on Worker global objects.
pub use web_worker::ScopeInitFn;
pub use runtime_transpiler_cache::{RuntimeTranspilerCache, RuntimeTranspilerStore, IS_DISABLED as TRANSPILER_CACHE_IS_DISABLED, Entry as TranspilerCacheEntry, TranspilerCacheImplKind};
pub use async_module::{AsyncModule, Queue as AsyncModuleQueue, InitOpts as AsyncModuleInitOpts};
pub use validation_scope::ValidationScope;
pub use format_tag::FormatTag;
pub use thread_safe::ThreadSafe;
pub use unprotect::Unprotect;
pub use global_ref::{GlobalRef, GlobalData};
pub use code_coverage::CodeCoverage;
pub use options_jsc::{OptionsJsc, VMOptions};
pub use method_jsc::MethodJsc;
pub use headers_jsc::HeadersJsc;
pub use create_utf::create_utf;
pub use resolved_source::{ResolvedSource, OwnedResolvedSource, Tag as ResolvedSourceTag};
pub use initialize::{initialize, eval_and_print};
pub use virtual_machine::VirtualMachineRef;
pub use c_api::c;

// ─── Dispatch / event loop ──────────────────────────────────────────────
pub use dispatch_sm::BaoEventLoop;

// ─── Codegen / generated ────────────────────────────────────────────────
pub use codegen::{ClassDef, PropertyDef, PropertyKind, ParseResult, GeneratedBindings};
pub use generated::{GenOpt, GenVal, GenList, BracesOptions, ProcessConfigOptions};

// ─── Debugger ───────────────────────────────────────────────────────────
pub use debugger::{Debugger, DebuggerError, Breakpoint, SourceInfo};

// ─── Rare data ──────────────────────────────────────────────────────────
pub use rare_data::{RareData, HotMap, HotMapEntry};

// ─── IPC ────────────────────────────────────────────────────────────────
pub use ipc::{IpcDirection, IpcMessage, IpcState, IpcChannel, IpcError};

// ─── WebCore types ──────────────────────────────────────────────────────
pub use webcore_types::{DomNodeType, EventPhase, RequestMode, ResponseType, RedirectMode, ReferrerPolicy, CacheMode};

// ─── JS object wrappers ─────────────────────────────────────────────────
pub use js_object::{JSObject, JSFunction, JSString, JSArrayIterator, JSArray, JSBigInt};

// ─── Weak references ────────────────────────────────────────────────────
pub use weak::{Weak, WeakRefType};

// ─── Regular Expression ─────────────────────────────────────────────────
pub use regular_expression::{RegularExpression, Flags as RegExpFlags};

// ─── Constants ────────────────────────────────────────────────────────────
pub const MAX_SAFE_INTEGER: f64 = 9007199254740991.0;
pub const MIN_SAFE_INTEGER: f64 = -9007199254740991.0;
