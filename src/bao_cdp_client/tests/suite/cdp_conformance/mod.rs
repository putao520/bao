//! CDP conformance 审计入口 — 对照 Chrome DevTools Protocol 官方规范审计
//! `bao_cdp_client` 193 method 的返回值 schema / 错误码 / 事件序列。
//!
//! # 测试组织
//!
//! 按域分文件,每个文件覆盖该域全部 method:
//! - `page_conformance.rs` — Page domain 11 method
//! - `runtime_conformance.rs` — Runtime domain 6 method
//! - `dom_conformance.rs` — DOM domain 11 method
//! - `network_conformance.rs` — Network domain 4 method
//! - `input_conformance.rs` — Input domain 4 method
//! - `emulation_conformance.rs` — Emulation domain 4 method
//! - `target_conformance.rs` — Target domain 6 method
//! - `css_conformance.rs` — CSS domain 2 method
//! - `log_conformance.rs` — Log 事件
//! - `debugger_conformance.rs` — Debugger 事件
//!
//! # conformance 检查点
//!
//! 每个 method 对照 CDP 官方 schema:
//! - **params schema**:必填/可选参数是否符合规范
//! - **result schema**:返回值字段是否符合规范(字段名/类型)
//! - **error code**:错误情况返回的 code 是否符合
//!   (-32601 MethodNotFound / -32602 InvalidParams / -32000 ServerError)
//! - **事件序列**:触发命令后预期的事件序列
//!
//! # 偏差清单
//!
//! 所有发现的实现偏差汇总在 `CONFORMANCE_REPORT.md`。
//!
//! @trace REQ-CDP-001 [level:integration]
//! @trace REQ-CDP-002 [level:integration]
//! @trace REQ-CDP-003 [level:integration]
//! @trace REQ-BAO-API-004 [level:integration]
//! @trace REQ-BAO-API-007 [level:integration]

mod css_conformance;
mod debugger_conformance;
mod dom_conformance;
mod emulation_conformance;
mod input_conformance;
mod log_conformance;
mod network_conformance;
mod page_conformance;
mod runtime_conformance;
mod target_conformance;
