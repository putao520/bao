//! CSS domain conformance 审计 — 2 method。
//!
//! 对照 CDP 官方规范(CSS domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/CSS/
//!
//! # 覆盖 method
//!
//! getComputedStyleForNode, getMatchedStylesForNode
//!
//! @trace REQ-CDP-001 [domain:CSS] [level:integration]
//! @trace REQ-BAO-API-004 [domain:CSS] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// CSS.getComputedStyleForNode — CDP spec: returns {computedStyle: [{name, value}]}
// https://chromedevtools.github.io/devtools-protocol/tot/CSS/#method-getComputedStyleForNode
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn css_get_computed_style_for_node_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {computedStyle: [{name: string, value: string}]}
    // @trace REQ-CDP-001 [domain:CSS] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "CSS.getComputedStyleForNode", json!({"nodeId":1}), "1").unwrap();

    // Assert
    assert!(
        result["computedStyle"].is_array(),
        "CDP spec: computedStyle must be array, got: {:?}",
        result["computedStyle"]
    );
    for prop in result["computedStyle"].as_array().unwrap() {
        assert!(prop["name"].is_string(), "CSSProperty.name must be string");
        assert!(prop["value"].is_string(), "CSSProperty.value must be string");
    }
}

#[test]
fn css_get_computed_style_for_node_missing_node_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:CSS] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "CSS.getComputedStyleForNode", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// CSS.getMatchedStylesForNode — CDP spec: returns {matchedCSSRules?,
//   inlineStyle?, attributesStyle?, ...}
// bao 实现: {matchedRules: [{rule: {selectorList, style}}], inlineStyle?, attributesStyle?}
// 偏差:字段名 matchedRules vs CDP 的 matchedCSSRules
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn css_get_matched_styles_for_node_returns_object() {
    // Arrange — bao 实现返回 {matchedRules, inlineStyle?, attributesStyle?}
    // @trace REQ-CDP-001 [domain:CSS] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "CSS.getMatchedStylesForNode", json!({"nodeId":1}), "1").unwrap();

    // Assert — bao 自定义 schema(matchedRules)
    assert!(
        result["matchedRules"].is_array(),
        "bao spec: matchedRules must be array"
    );
}

#[test]
fn css_get_matched_styles_for_node_field_name_documented_deviation() {
    // Arrange — CDP 规范: 字段名为 matchedCSSRules(bao 用 matchedRules → 偏差)
    // 此测试断言"当前字段名偏差",修复后会 fail → 提示更新报告
    // @trace REQ-CDP-001 [domain:CSS] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "CSS.getMatchedStylesForNode", json!({"nodeId":1}), "1").unwrap();

    // Assert — 记录偏差:bao 用 matchedRules 而非 CDP 规范的 matchedCSSRules
    assert!(
        result.get("matchedCSSRules").is_none(),
        "DEV-NOTE: bao uses `matchedRules` instead of CDP spec `matchedCSSRules` (naming deviation). \
         If this fails, bao has aligned — update CONFORMANCE_REPORT."
    );
}

#[test]
fn css_get_matched_styles_for_node_missing_node_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:CSS] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "CSS.getMatchedStylesForNode", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn css_get_matched_styles_for_node_inline_style_optional_schema() {
    // Arrange — CDP 规范: inlineStyle / attributesStyle 可选
    // 当存在时,inlineStyle 是 CSSStyle:{styleSheetId, cssProperties: [{name, value, important}]}
    // @trace REQ-CDP-001 [domain:CSS] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "CSS.getMatchedStylesForNode", json!({"nodeId":1}), "1").unwrap();

    // 当 Mock 返回 None 时,字段缺失 — 不做严格断言
    if let Some(inline) = result.get("inlineStyle") {
        if !inline.is_null() {
            assert!(inline.is_object());
            if let Some(props) = inline["cssProperties"].as_array() {
                for p in props {
                    assert!(p["name"].is_string());
                    assert!(p["value"].is_string());
                    assert!(p["important"].is_boolean());
                }
            }
        }
    }
}
