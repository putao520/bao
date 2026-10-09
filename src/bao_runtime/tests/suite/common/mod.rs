//! Shared suite helpers (e157 M2 consolidation of e152 audit §1.2
//! DUP-TEST-HELPERS: the per-file inline copies of the eval family).
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
//! The `#[path]` attribute is required in non-root files; from nested
//! directories use `#[path = "../common/mod.rs"]`.
//!
//! Provenance: every body below is a verbatim lift of the dominant inline
//! copy it replaces (masked-variant hash recorded per fn; the per-file
//! diff audit lives in the e157 B1 plan). `*_named` variants expose the
//! only parameter that legitimately differed between copies — the script
//! filename literal fed to `ctx.eval` — so files with unique filenames
//! keep their attribution via one-line delegators.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

/// Minimal stringifier: String/Number/Bool formatted, everything else `""`.
/// (masked variant 4f3ea37e, 81 verbatim copies + 2 block-bool merges)
pub fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

/// Rich stringifier: explicit null/undefined/object arms, `ERROR:{message}`.
/// (masked variant f3479490, 27 verbatim copies + 1 block-bool merge)
pub fn eval_string_full(ctx: &mut JsContext, source: &str) -> String {
    eval_string_full_named(ctx, source, "<test>")
}

/// Filename-parameterized [`eval_string_full`] for files whose inline copy
/// used a script-specific filename literal.
pub fn eval_string_full_named(ctx: &mut JsContext, source: &str, file: &str) -> String {
    match ctx.eval(source, file) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Rich stringifier with an explicit `[other]` catch-all arm.
/// (masked variant 8f93172e, 6 verbatim copies)
pub fn eval_string_dbg(ctx: &mut JsContext, source: &str) -> String {
    eval_string_dbg_named(ctx, source, "<test>")
}

/// Filename-parameterized [`eval_string_dbg`].
pub fn eval_string_dbg_named(ctx: &mut JsContext, source: &str, file: &str) -> String {
    match ctx.eval(source, file) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Ok(_) => "[other]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Debug-fallback stringifier: non-primitives via `{:?}`, errors via
/// `ERROR: {:?}`. (masked variant 1f8576ba, 10 verbatim copies + 1
/// inline-capture merge)
pub fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    eval_str_named(ctx, code, "<test>")
}

/// Filename-parameterized [`eval_str`].
pub fn eval_str_named(ctx: &mut JsContext, code: &str, file: &str) -> String {
    match ctx.eval(code, file) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

/// Integer-preserving stringifier: whole numbers must not print as `1.0`;
/// non-primitives become `""`, errors become `<eval-error>`.
/// (masked variant 0dcf157a, 4 copies, all with unique filenames)
pub fn eval_str_int_named(ctx: &mut JsContext, source: &str, file: &str) -> String {
    match ctx.eval(source, file) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => {
            // integral numbers must not print as "1.0"
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", n as i64)
            } else {
                format!("{}", n)
            }
        }
        Ok(JsValue::Bool(b)) => {
            if b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Ok(_) => String::new(),
        Err(_) => "<eval-error>".to_string(),
    }
}

/// Number extractor: non-Number results (including errors) yield `NaN`.
/// (masked variant 3fe28cb6, 9 verbatim copies)
pub fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}
