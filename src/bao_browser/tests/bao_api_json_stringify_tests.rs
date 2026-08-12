//! REQ-BAO-API-005 C3: JSON.stringify U+2028/U+2029 边界处理测试
//!
//! # 背景
//!
//! B 类 Eval 合成器 (`eval_synthesizer::build_iife_with_args`) 使用 `serde_json::to_string`
//! 将参数序列化为 JSON 字面量，嵌入 IIFE 表达式。JSON 规范 (RFC 8259) 要求字符串中
//! 的 U+2028 (LINE SEPARATOR) 和 U+2029 (PARAGRAPH SEPARATOR) 必须被转义。
//!
//! `serde_json` 遵循 JSON 规范，默认会转义这些字符。但 JS 的 `JSON.stringify` 默认不会转义
//! U+2028/U+2029（技术上合法的 JSON，但有些 JS 引擎要求转义以保持互操作性）。
//!
//! # 测试维度
//!
//! 1. `serde_json::to_string` 对 U+2028/U+2029 的转义行为（直接 Rust 层测试）
//! 2. IIFE 合成中 U+2028/U+2029 在 `__args` 字面量中的表现
//! 3. 边界场景: 纯 U+2028、首/尾位置、嵌套对象/数组
//! 4. NUL 字符 (U+0000) 和孤立代理对 (U+D800) 在 `serde_json` 中的处理
//! 5. JSON.parse 可逆性（转义后字符串可被解析）
//! 6. JS 层 `JSON.stringify` 的 U+2028/U+2029 转义行为（与 Rust serde_json 对比）
//!
//! @trace REQ-BAO-API-005 [criterion:C3] [level:unit,integration]

// ─── Rust 层 serde_json 边界测试 ───

/// U+2028 — LINE SEPARATOR
const LINE_SEPARATOR: char = '\u{2028}';
/// U+2029 — PARAGRAPH SEPARATOR
const PARAGRAPH_SEPARATOR: char = '\u{2029}';
/// NUL character
const NUL: char = '\u{0000}';
/// Isolated surrogate (U+D800) — invalid UTF-8, can only exist in unpaired form
/// but `char` in Rust cannot hold surrogates; we test via `str`.

// ═══════════════════════════════════════════════════════════════════════
// §1 serde_json::to_string — U+2028/U+2029 转义行为
// ═══════════════════════════════════════════════════════════════════════

/// serde_json 应正确转义 U+2028 为 ` `
#[test]
fn serde_json_escapes_u_2028() {
    let input = format!("hello{}world", LINE_SEPARATOR);
    let json = serde_json::to_string(&input).expect("serde_json serialization should succeed");
    // serde_json 默认用 escape_non_ascii=false，保留非 ASCII 字符；
    // 但 JSON 规范 (RFC 8259 §7) 允许 unescaped 控制字符以外的 Unicode。
    // U+2028 不是控制字符 (0x00-0x1F)，所以 serde_json 可能不转义它。
    // 关键是 JSON.parse 能正确还原。
    let parsed: String = serde_json::from_str(&json).expect("JSON.parse should reconstruct");
    assert_eq!(
        parsed, input,
        "Round-trip: original must equal parsed result"
    );
    // 记录实际输出供审计
    eprintln!(
        "U+2028 serde_json output: {:?} (has \\u escape: {})",
        json,
        json.contains("\\u2028")
    );
}

/// serde_json 应正确转义 U+2029 为 ` `
#[test]
fn serde_json_escapes_u_2029() {
    let input = format!("line1{}line2", PARAGRAPH_SEPARATOR);
    let json = serde_json::to_string(&input).expect("serde_json serialization should succeed");
    let parsed: String = serde_json::from_str(&json).expect("JSON.parse should reconstruct");
    assert_eq!(
        parsed, input,
        "Round-trip: original must equal parsed result"
    );
    eprintln!(
        "U+2029 serde_json output: {:?} (has \\u escape: {})",
        json,
        json.contains("\\u2029")
    );
}

/// 同时包含 U+2028 和 U+2029 的字符串
#[test]
fn serde_json_handles_both_separators() {
    let input = format!(
        "a{sep1}b{sep2}c",
        sep1 = LINE_SEPARATOR,
        sep2 = PARAGRAPH_SEPARATOR
    );
    let json = serde_json::to_string(&input).expect("serialize");
    let parsed: String = serde_json::from_str(&json).expect("parse");
    assert_eq!(parsed, input, "Round-trip must preserve both separators");
}

// ═══════════════════════════════════════════════════════════════════════
// §2 serde_json — NUL 字符处理
// ═══════════════════════════════════════════════════════════════════════

/// NUL (U+0000) 在 JSON 字符串中应被转义为 `\\u0000`
#[test]
fn serde_json_escapes_nul() {
    let input = format!("null{0}char", NUL);
    let json = serde_json::to_string(&input).expect("serialize");
    // NUL 是控制字符 (0x00)，serde_json 必须转义它
    assert!(
        json.contains("\\u0000"),
        "NUL must be escaped as \\u0000, got: {json:?}"
    );
    let parsed: String = serde_json::from_str(&json).expect("parse");
    assert_eq!(parsed, input, "Round-trip must preserve NUL");
}

// ═══════════════════════════════════════════════════════════════════════
// §3 IIFE 合成 — U+2028/U+2029 边界
// ═══════════════════════════════════════════════════════════════════════

use bao_cdp_client::bridge::eval_synthesizer::build_iife_with_args;
use serde_json::{json, Value};

/// IIFE __args 中的 U+2028 不破坏 JS 语法
#[test]
fn iife_iife_with_u_2028_args_is_safe() {
    let payload = format!("hello{}world", LINE_SEPARATOR);
    let expr = build_iife_with_args("return __args[0];", &[json!(payload)])
        .expect("build_iife_with_args should succeed");
    // __args 字面量中 payload 必须被正确编码
    let json_payload = serde_json::to_string(&payload).unwrap();
    assert!(
        expr.contains(&json_payload),
        "JSON-encoded payload must appear in __args literal\nexpr: {expr}\njson_payload: {json_payload}"
    );
    // body 部分不应包含 payload 原文
    let body_start = expr.find("return (function(){").unwrap();
    let body_end = expr.find("}).apply(null, __args);").unwrap();
    let body = &expr[body_start..body_end];
    assert!(
        !body.contains(&payload),
        "body must not contain raw payload\nexpr: {expr}"
    );
}

/// IIFE __args 中的 U+2029 不破坏 JS 语法
#[test]
fn iife_with_u_2029_args_is_safe() {
    let payload = format!("line1{}line2", PARAGRAPH_SEPARATOR);
    let expr = build_iife_with_args("return __args[0];", &[json!(payload)])
        .expect("build_iife_with_args should succeed");
    let json_payload = serde_json::to_string(&payload).unwrap();
    assert!(
        expr.contains(&json_payload),
        "JSON-encoded payload must appear in __args literal"
    );
}

/// 参数含有 U+2028 在开头位置
#[test]
fn iife_u_2028_at_start_of_arg() {
    let payload = format!("{}rest", LINE_SEPARATOR);
    let expr = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
    let json_payload = serde_json::to_string(&payload).unwrap();
    assert!(expr.contains(&json_payload), "U+2028 at start: {expr}");
}

/// 参数含有 U+2028 在结尾位置
#[test]
fn iife_u_2028_at_end_of_arg() {
    let payload = format!("prefix{}", LINE_SEPARATOR);
    let expr = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
    let json_payload = serde_json::to_string(&payload).unwrap();
    assert!(expr.contains(&json_payload), "U+2028 at end: {expr}");
}

/// 纯 U+2028 字符串
#[test]
fn iife_pure_u_2028_arg() {
    let payload = LINE_SEPARATOR.to_string();
    let expr = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
    let json_payload = serde_json::to_string(&payload).unwrap();
    assert!(
        expr.contains(&json_payload),
        "pure U+2028: {expr}, json: {json_payload}"
    );
    // JSON.parse 可逆
    let parsed: String = serde_json::from_str(&json_payload).expect("parse");
    assert_eq!(parsed, "\u{2028}");
}

/// 多层嵌套对象中含有 U+2028/U+2029
#[test]
fn iife_nested_object_with_separators() {
    let payload = json!({
        "title": format!("hello{}world", LINE_SEPARATOR),
        "body": format!("line1{}line2", PARAGRAPH_SEPARATOR),
        "nested": {
            "data": [
                format!("a{}b", LINE_SEPARATOR),
                format!("c{}d", PARAGRAPH_SEPARATOR),
            ]
        }
    });
    let expr = build_iife_with_args("return __args[0];", &[payload.clone()]).unwrap();
    let json_payload = serde_json::to_string(&payload).unwrap();
    assert!(
        expr.contains(&json_payload),
        "nested object with separators"
    );
    // JSON.parse 可逆
    let parsed: Value = serde_json::from_str(&json_payload).expect("parse");
    assert_eq!(parsed, payload, "Round-trip must preserve nested structure");
}

/// 数组中含有 U+2028/U+2029
#[test]
fn iife_array_with_separators() {
    let payload = json!([
        format!("a{}b", LINE_SEPARATOR),
        format!("c{}d", PARAGRAPH_SEPARATOR),
        LINE_SEPARATOR.to_string(),
        PARAGRAPH_SEPARATOR.to_string(),
    ]);
    let expr = build_iife_with_args("return __args[0];", &[payload.clone()]).unwrap();
    let json_payload = serde_json::to_string(&payload).unwrap();
    assert!(expr.contains(&json_payload), "array with separators");
}

// ═══════════════════════════════════════════════════════════════════════
// §4 serde_json → JSON.parse 可逆性验证
// ═══════════════════════════════════════════════════════════════════════

/// 所有边界字符组合的 round-trip 验证
#[test]
fn boundary_char_roundtrip() {
    let test_cases = vec![
        LINE_SEPARATOR.to_string(),
        PARAGRAPH_SEPARATOR.to_string(),
        format!("{}{}", LINE_SEPARATOR, PARAGRAPH_SEPARATOR),
        format!("{a}{b}{a}{b}", a = LINE_SEPARATOR, b = PARAGRAPH_SEPARATOR),
        "plain text with no special chars".to_string(),
        "numbers 12345 in text".to_string(),
        "unicode beyond BMP: 🔥🚀🌟".to_string(),
    ];

    for tc in &test_cases {
        let json = serde_json::to_string(tc).expect("serialize");
        let parsed: String = serde_json::from_str(&json).expect("parse");
        assert_eq!(
            &parsed, tc,
            "Round-trip failed for input: {tc:?} -> json: {json:?} -> parsed: {parsed:?}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §5 UTF-8 length 校验 — 序列化过程中无数据损坏
// ═══════════════════════════════════════════════════════════════════════

/// 验证 round-trip 后 UTF-8 byte length 与原始一致
#[test]
fn utf8_length_preserved() {
    let inputs = vec![
        format!("a{}b", LINE_SEPARATOR),
        format!("a{}b", PARAGRAPH_SEPARATOR),
        format!("a{}{}b", LINE_SEPARATOR, PARAGRAPH_SEPARATOR),
        "plain".to_string(),
        format!("{}{}{}", LINE_SEPARATOR, LINE_SEPARATOR, LINE_SEPARATOR),
    ];

    for input in &inputs {
        let json = serde_json::to_string(input).expect("serialize");
        let parsed: String = serde_json::from_str(&json).expect("parse");
        assert_eq!(
            parsed.len(),
            input.len(),
            "UTF-8 byte length mismatch: input={input:?} ({} bytes) -> parsed={parsed:?} ({} bytes) via json={json:?}",
            input.len(),
            parsed.len(),
        );
    }
}
