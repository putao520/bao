//! Shared suite helpers (e157 M3 consolidation of e152 audit §1.2
//! DUP-TEST-HELPERS for the bao_engine suite).
//!
//! Include from a sibling test file at `tests/suite/*.rs` (top of file,
//! after the `use` block):
//!
//! ```ignore
//! #[path = "common/mod.rs"]
//! mod common;
//! use common::eval_string;
//! ```
//!
//! The bodies mirror `src/bao_runtime/tests/suite/common/mod.rs` verbatim
//! (identical masked-variant hashes — the two suites cannot share one
//! module file because integration tests only link their own crate's
//! dependencies, and bun_runtime depends on bao_engine, not vice versa).
//! Provenance hashes recorded per fn; the e157 plan holds the per-file
//! diff audit.

#![allow(dead_code)]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

/// Minimal stringifier: String/Number/Bool formatted, everything else `""`.
/// (masked variant 4f3ea37e, 2 verbatim copies in this suite)
pub fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

/// Rich stringifier: explicit null/undefined/object arms, `ERROR:{message}`.
/// (masked variant f3479490, 2 verbatim copies in this suite)
pub fn eval_string_full(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Debug-fallback stringifier with filename attribution preserved per file.
/// (masked variants 84791136 + 673688f1 — identical modulo parameter and
/// binding names; both call sites keep their unique filenames)
pub fn eval_string_debug_named(ctx: &mut JsContext, source: &str, file: &str) -> String {
    match ctx.eval(source, file) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(other) => format!("{:?}", other),
        Err(e) => format!("<error: {}>", e.message),
    }
}

/// Number extractor: non-Number results (including errors) yield `NaN`.
/// (masked variant 3fe28cb6, 4 verbatim copies in this suite)
pub fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}
