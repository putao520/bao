// @trace REQ-ENG-001
//! SpiderMonkey engine wrapper. Core types are provided by `bun_sm`;
//! this crate retains only the modules that have complex inter-dependencies
//! (context, job_queue).
//!
//! # Re-export Architecture
//!
//! ```text
//! Layer order (bottom → top):
//!   bun_sm        — 底层：SpiderMonkey 值类型 + JSC API 兼容层 + module_loader
//!                    JSValue, JSGlobalObject, VirtualMachine, CallFrame, JSType, ModuleLoader
//!   bao_engine    — Re-export 层：从 bun_sm re-export 公共类型，保留自有模块
//!                    context, job_queue (复杂内部依赖)
//!   bao_runtime   — 消费层：pub use bao_engine::* (最终替换 bun_jsc::*)
//!   bao_browser   — 消费层：依赖 bao_engine 的 context + dispatch_sm
//! ```
//!
//! ## 迁移步骤 (bun_jsc → bun_sm)
//!
//! 1. **Phase 1 (已完成)**: bao_engine 依赖 bun_sm，re-export 核心类型
//! 2. **Phase 2 (进行中)**: bao_runtime 将 `pub use bun_jsc::*` 改为 `pub use bun_sm::*`
//! 3. **Phase 3 (计划中)**: 移除 bun_jsc 依赖，所有 8,500+ JSC 引用指向 bun_sm
//!
//! ## 约定
//!
//! - **下游 crate 必须通过 `bao_engine::Type` 访问类型，禁止直接 `use bun_sm::Type`**
//!   - 唯一例外：crate 确实需要 bun_sm 中未被 bao_engine re-export 的类型
//!   - 目的：维护单一依赖表面，避免下游 crate 同时依赖 bao_engine 和 bun_sm
//!
//! - 新代码直接使用 `bun_sm::JSValue` 等类型
//! - bao_engine 仅 re-export，不定义值类型
//! - 保留在 bao_engine 的模块（context/job_queue）因内部依赖复杂不迁移
//! - module_loader 已迁移到 bun_sm，通过回调模式（JobQueueDrainFn）避免循环依赖
//!
//! ## 迁移模块到 bun_sm 的步骤
//!
//! 1. 复制 `.rs` 文件到 `bun_sm/src/`
//! 2. 更新 `use crate::` 引用为 bun_sm 自身路径
//! 3. 从 `bao_engine/src/` 删除原文件
//! 4. 在下方 Re-exports 区域添加 `pub use bun_sm::xxx;`
//! 5. 验证：`cargo check -p bao_engine` 通过

// ─── Re-exports from bun_sm (types moved there) ──────────────────────────
// @trace REQ-ENG-001 [entity:BaoRuntime] — SpiderMonkey engine core types (value/error/dispatch) re-exported from bun_sm (mozjs v0.15.14 integration)
// @trace REQ-ENG-007 [api:GET /api/node-compat] — Node.js compat surface (node:fs/path/http/crypto/tls/Buffer/process) bridged via bao_engine re-exports
// @trace REQ-ENG-008 [api:POST /sqlite/open] [entity:SqliteDatabase] — bun:sqlite SpiderMonkey bridge types surfaced through bao_engine
// @trace REQ-ENG-009 [api:POST /ffi/load] [entity:FfiLibrary] — bun:ffi SpiderMonkey bridge types surfaced through bao_engine
// @trace REQ-ENG-010 [entity:FetchTasklet] [api:POST /fetch/async-tasklet] — async fetch/http/https/tls FetchTasklet integration re-export entry
// @trace REQ-ENG-011 [api:/vm/sandbox] [entity:VmSandboxContext] — node:vm sandbox Realm isolation re-export entry
pub use bun_sm::value;
pub use bun_sm::error;
pub use bun_sm::dispatch_sm;
pub use bun_sm::host_fn;
pub use bun_sm::codegen;
pub use bun_sm::abort_signal;
pub use bun_sm::builtin_name;
pub use bun_sm::common_strings;
pub use bun_sm::debugger;
pub use bun_sm::fetch_headers;
pub use bun_sm::gc;
pub use bun_sm::generated;
pub use bun_sm::ipc;
pub use bun_sm::rare_data;
pub use bun_sm::webcore_types;

// ─── Re-exported from bun_sm (migrated from bao_engine) ──────────────────
// @trace REQ-ENG-005 [entity:ModuleSource] [api:POST /module/resolve] — Module Loader bridge (SpiderMonkey ESM hooks → Bun resolver) re-exported from bun_sm
// @trace REQ-ENG-006 [api:GET /api/bun-compat] [entity:BaoRuntime] — Bun.* / Bao.* API adaptation re-export entry (serve/file/fetch/write)
pub use bun_sm::module_loader;
pub use bun_sm::module_loader::{GlobalSetupFn, PostEvalHook, JobQueueDrainFn, set_job_queue_drain};

// ─── Worker re-exports from bun_sm (REQ-BRW-004) ──────────────────────
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope]
pub use bun_sm::web_worker::{WebWorker, ScopeInitFn, StructuredCloneReceiver, StructuredCloneSender};

// ─── Modules still owned by bao_engine ───────────────────────────────────
// @trace REQ-ENG-003 [api:POST /host-fn/call] [entity:JsCallback] — host_fn safe FFI wrapper owned module surface (JS call / type conversion / GC root RAII)
// @trace REQ-ENG-004 [api:POST /event-loop/drain] [entity:BaoRuntime] — Event Loop bridge (SpiderMonkey JobQueue → uSockets I/O) owned module surface
pub mod context;
pub mod job_queue;

// @trace REQ-ENG-002 [api:POST /codegen/generate] [entity:CodegenBackend] — codegen backend rewrite (.classes.ts → SpiderMonkey bindings) re-exported via bun_sm::codegen / bun_sm::generated

// Re-export proc-macros from bao_engine_macros
pub use bao_engine_macros::codegen_cached_accessors;

// #[macro_export] macros from bun_sm (define_host_fn) are automatically
// available when bao_engine depends on bun_sm — they live at the crate root.
// No explicit `pub use` needed for #[macro_export] macros.