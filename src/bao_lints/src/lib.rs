//! `bao_lints` — BCE 防复发检测器集合(library)。
//!
//! 当前承载两类检测器(都遵循 BCE 协议,详见 `src/BUG-KNOWLEDGE.md`):
//!
//! 1. **`detector`** —— BCE-20260619-012 GC-unsafe Handle 构造(format-immune
//!    AST-based)。扫描 Rust 源码中 `Handle::<T> { ptr: &... }` 反模式。
//! 2. **`spec_id`** —— REQ-SPEC-001 SPEC API 元素 id 格式(method-path 退化
//!    防复发)。扫描 `.spec/*.html` 中带 `data-api=` 的 `<section>`/`<div>`,
//!    校验 id 是否符合 `API-{DOMAIN}-{N}`。
//!
//! 二进制入口见 `src/main.rs`(`--check` 跑 BCE-012,`spec-id` 跑 REQ-SPEC-001)。
//!
//! ## 工作流定位 — REQ-SPEC-002 的真实实现落点
//!
// @trace REQ-SPEC-002
//
//! 本 crate 是 REQ-SPEC-002 「确定性批量任务禁用 six-node-dev 多 epoch loop」
//! 的真实实现落点。REQ-SPEC-002 的契约是:对确定性、机械化、无架构决策的批量
//! 任务(如 SPEC id 迁移、属性重排、xref 修复、模式扫描),必须用一次性脚本/
//! 检测器门禁完成,而非启动 six-node-dev.js 多 epoch loop 流程(后者是为需要
//! 需求讨论/方案对比/架构决策的复杂任务设计的)。
//!
//! `bao_lints` + `scripts/bce_*.py` + `Makefile bce-*` 共同构成这套确定性门禁:
//! 每个 BCE 模式沉淀为一个独立的机械化检测器,CI 跑一次即可判定,无 epoch、
//! 无回跳、无 WF loop。新增 BCE 模式时也应按此契约扩展本 crate,而非引入 WF。

pub mod detector;
pub mod pattern;
pub mod spec_id;

pub use detector::{Finding, scan_source};
pub use spec_id::{Reason, SpecIdFinding, scan_html, scan_path};
