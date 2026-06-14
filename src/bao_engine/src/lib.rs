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
//! - **下游 crate 必须通过 `bao_engine::XXX` 访问类型，禁止直接 `use bun_sm::XXX`**
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
pub use bun_sm::module_loader;
pub use bun_sm::module_loader::{GlobalSetupFn, PostEvalHook, JobQueueDrainFn, set_job_queue_drain};

// ─── Modules still owned by bao_engine ───────────────────────────────────
pub mod context;
pub mod job_queue;

// Re-export proc-macros from bao_engine_macros
pub use bao_engine_macros::codegen_cached_accessors;

// #[macro_export] macros from bun_sm (define_host_fn) are automatically
// available when bao_engine depends on bun_sm — they live at the crate root.
// No explicit `pub use` needed for #[macro_export] macros.