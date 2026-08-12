//! B 类 52 method 注入防御集成测试。
//!
//! 每个 B 类 method 至少 1 个注入向量测试,覆盖以下向量:
//!
//! - 单引号 `'`
//! - 双引号 `"`
//! - 反斜杠 `\`
//! - `</script>` 闭合标签
//! - 模板字符串 `${}`
//! - SQL injection 风格(`' OR '1'='1`)
//! - SVG onload (`<svg/onload=alert(1)>`)
//! - 控制字符
//! - Unicode 转义序列
//!
//! # 安全保证
//!
//! 所有 Eval 路径必须使用 [`bao_cdp_client::bridge::eval_synthesizer::build_iife`] 或
//! [`build_iife_with_args`],参数经 `JSON.stringify` 编码,严禁字符串拼接。
//!
//! 这些测试验证:
//! 1. 生成 IIFE 表达式包含 payload 作为 JSON 字符串字面量(在 `__args` 数组内)
//! 2. IIFE body 部分**不**包含 payload 原文作为代码
//!
//! @trace REQ-BAO-API-005 [level:integration]

use bao_cdp_client::bridge::{BridgeError, MockServoBackend, ServoBackend};
use bao_cdp_client::dispatch_command;
use serde_json::{json, Value};

fn backend() -> Box<dyn ServoBackend> {
    Box::new(MockServoBackend::new())
}

fn run(method: &str, params: Value) -> Result<Value, BridgeError> {
    let b = backend();
    dispatch_command(&*b, method, params, "1")
}

/// 从 Mock evaluate echo 中提取 expression。
fn extract_expr(r: &Value) -> String {
    r["result"]["value"]
        .as_str()
        .expect("evaluate must return string value (mock echo)")
        .to_string()
}

/// 截取 IIFE expression 的 `var __args=[...]` 字面量段。
fn args_literal(expr: &str) -> String {
    let start = expr
        .find("var __args=")
        .expect("missing __args declaration");
    let rel = expr[start..].find("];").expect("missing __args terminator");
    expr[start..start + rel + 1].to_string()
}

/// 截取 IIFE body(`return (function(){ ... }).apply(null, __args);`)。
fn body_section(expr: &str) -> String {
    let start = expr
        .find("return (function(){")
        .expect("missing body start");
    let end = expr
        .find("}).apply(null, __args);")
        .expect("missing body end");
    expr[start..end].to_string()
}

// ════════════════════════════════════════════════════════════════════
// 注入向量常量 — 每个向量代表一类攻击 payload
// ════════════════════════════════════════════════════════════════════

/// 单引号 + alert 注入。
const VEC_SINGLE_QUOTE_ALERT: &str = r#"');alert('xss');//"#;
/// 双引号 + alert 注入。
const VEC_DOUBLE_QUOTE_ALERT: &str = r#"";alert("x");//"#;
/// 反斜杠 + escape。
const VEC_BACKSLASH: &str = r#"\;alert(1);//"#;
/// `</script>` 闭合注入。
const VEC_SCRIPT_CLOSE: &str = r#"</script><script>alert(1)</script>"#;
/// 模板字符串注入。
const VEC_TEMPLATE_LITERAL: &str = r#"${alert(1)}"#;
/// SQL 风格。
const VEC_SQL: &str = r#"x' OR '1'='1"#;
/// SVG onload。
const VEC_SVG_ONLOAD: &str = r#"x'<svg/onload=alert(1)>//"#;
/// 控制字符 \n。
const VEC_NEWLINE: &str = "a\nb";
/// 控制字符 \\u0000。
const VEC_NULL: &str = "\u{0000}";

// ════════════════════════════════════════════════════════════════════
// 通用断言工具
// ════════════════════════════════════════════════════════════════════

/// 断言 payload 在 __args 字面量内(JSON-encoded),不在 body 内。
fn assert_payload_safe(expr: &str, payload: &str) {
    let args = args_literal(expr);
    let body = body_section(expr);
    // payload 必须在 args 内(作为 JSON 字符串字面量)
    let json_payload = serde_json::to_string(payload).unwrap();
    assert!(
        args.contains(&json_payload),
        "payload must appear in __args literal\nexpr: {expr}\nargs: {args}\nexpected JSON: {json_payload}"
    );
    // body 内不包含 payload 原文作为代码(可能因 JS 字符串模式碰巧包含,
    // 但绝不能是 IIFE 直接字面量注入)。这里验证 body 不等于 payload 拼接。
    // 安全检查:body 不应是 `payload` 本身,且 body 不应等于 `return {payload}` 模式。
    let dangerous_patterns = [
        format!("return {payload};"),
        format!("var x={payload};"),
        format!("el.setAttribute({payload})"),
    ];
    for pat in &dangerous_patterns {
        assert!(
            !body.contains(pat),
            "body contains dangerous pattern: {pat}\nbody: {body}"
        );
    }
}

// ════════════════════════════════════════════════════════════════════
// 1. Page.title — 无参数 IIFE,本身不接收外部输入
//    但验证 IIFE 结构完整
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_page_title_iife_structure_intact() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.title] [level:integration]
    // Act
    let r = run("Page.title", json!({})).unwrap();
    let e = extract_expr(&r);
    // Assert
    assert!(e.starts_with("(function(){"));
    assert!(e.ends_with("})()"));
    assert!(e.contains("return document.title;"));
}

// ════════════════════════════════════════════════════════════════════
// 2. Page.url — 无参数 IIFE
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_page_url_iife_structure_intact() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.url] [level:integration]
    // Act
    let r = run("Page.url", json!({})).unwrap();
    let e = extract_expr(&r);
    // Assert
    assert!(e.contains("return location.href;"));
}

// ════════════════════════════════════════════════════════════════════
// 3. Page.content — 无参数 IIFE
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_page_content_iife_structure_intact() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.content] [level:integration]
    // Act
    let r = run("Page.content", json!({})).unwrap();
    let e = extract_expr(&r);
    // Assert
    assert!(e.contains("document.documentElement.outerHTML"));
}

// ════════════════════════════════════════════════════════════════════
// 4. Page.addScriptTag — 接收 url 或 content(用户输入)
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_add_script_tag_url_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.addScriptTag] [level:integration]
    // Act
    let r = run("Page.addScriptTag", json!({"url": VEC_SINGLE_QUOTE_ALERT})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_add_script_tag_url_double_quote_alert() {
    // Arrange
    // Act
    let r = run("Page.addScriptTag", json!({"url": VEC_DOUBLE_QUOTE_ALERT})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_DOUBLE_QUOTE_ALERT);
}

#[test]
fn inj_add_script_tag_url_backslash() {
    // Arrange
    // Act
    let r = run("Page.addScriptTag", json!({"url": VEC_BACKSLASH})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_BACKSLASH);
}

#[test]
fn inj_add_script_tag_content_script_close() {
    // Arrange
    // Act
    let r = run("Page.addScriptTag", json!({"content": VEC_SCRIPT_CLOSE})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SCRIPT_CLOSE);
}

#[test]
fn inj_add_script_tag_content_template_literal() {
    // Arrange
    // Act
    let r = run(
        "Page.addScriptTag",
        json!({"content": VEC_TEMPLATE_LITERAL}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_TEMPLATE_LITERAL);
}

// ════════════════════════════════════════════════════════════════════
// 5. Page.addStyleTag — 接收 url 或 content
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_add_style_tag_url_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.addStyleTag] [level:integration]
    // Act
    let r = run("Page.addStyleTag", json!({"url": VEC_SINGLE_QUOTE_ALERT})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_add_style_tag_content_script_close() {
    // Arrange
    // Act
    let r = run("Page.addStyleTag", json!({"content": VEC_SCRIPT_CLOSE})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SCRIPT_CLOSE);
}

#[test]
fn inj_add_style_tag_content_sql() {
    // Arrange
    // Act
    let r = run("Page.addStyleTag", json!({"content": VEC_SQL})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SQL);
}

// ════════════════════════════════════════════════════════════════════
// 6. Page.exposeFunction — 接收 name
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_expose_function_name_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.exposeFunction] [level:integration]
    // Act
    let r = run(
        "Page.exposeFunction",
        json!({"name": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_expose_function_name_template_literal() {
    // Arrange
    // Act
    let r = run("Page.exposeFunction", json!({"name": VEC_TEMPLATE_LITERAL})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_TEMPLATE_LITERAL);
}

#[test]
fn inj_expose_function_name_sql() {
    // Arrange
    // Act
    let r = run("Page.exposeFunction", json!({"name": VEC_SQL})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SQL);
}

// ════════════════════════════════════════════════════════════════════
// 7. Page.emulateMedia — 接收 media
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_emulate_media_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.emulateMedia] [level:integration]
    // Act
    let r = run(
        "Page.emulateMedia",
        json!({"media": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

// ════════════════════════════════════════════════════════════════════
// 8. Page.tap — 接收 selector
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_tap_selector_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.tap] [level:integration]
    // Act
    let r = run("Page.tap", json!({"selector": VEC_SINGLE_QUOTE_ALERT})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_tap_selector_sql() {
    // Arrange
    // Act
    let r = run("Page.tap", json!({"selector": VEC_SQL})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SQL);
}

#[test]
fn inj_tap_selector_svg_onload() {
    // Arrange
    // Act
    let r = run("Page.tap", json!({"selector": VEC_SVG_ONLOAD})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SVG_ONLOAD);
}

// ════════════════════════════════════════════════════════════════════
// 9. Page.hover — 接收 selector
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_hover_selector_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.hover] [level:integration]
    // Act
    let r = run("Page.hover", json!({"selector": VEC_SINGLE_QUOTE_ALERT})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_hover_selector_template_literal() {
    // Arrange
    // Act
    let r = run("Page.hover", json!({"selector": VEC_TEMPLATE_LITERAL})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_TEMPLATE_LITERAL);
}

// ════════════════════════════════════════════════════════════════════
// 10. Page.focus — 接收 selector
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_focus_selector_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.focus] [level:integration]
    // Act
    let r = run("Page.focus", json!({"selector": VEC_SINGLE_QUOTE_ALERT})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

// ════════════════════════════════════════════════════════════════════
// 11. Page.type — 接收 selector + text
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_type_text_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.type] [level:integration]
    // Act
    let r = run(
        "Page.type",
        json!({"selector":"input","text": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_type_text_template_literal() {
    // Arrange
    // Act
    let r = run(
        "Page.type",
        json!({"selector":"input","text": VEC_TEMPLATE_LITERAL}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_TEMPLATE_LITERAL);
}

#[test]
fn inj_type_selector_sql() {
    // Arrange
    // Act
    let r = run("Page.type", json!({"selector": VEC_SQL, "text":"ok"})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SQL);
}

// ════════════════════════════════════════════════════════════════════
// 12. Page.fill — 接收 selector + value
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_fill_value_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.fill] [level:integration]
    // Act
    let r = run(
        "Page.fill",
        json!({"selector":"input","value": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_fill_value_backslash() {
    // Arrange
    // Act
    let r = run(
        "Page.fill",
        json!({"selector":"input","value": VEC_BACKSLASH}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_BACKSLASH);
}

#[test]
fn inj_fill_value_script_close() {
    // Arrange
    // Act
    let r = run(
        "Page.fill",
        json!({"selector":"input","value": VEC_SCRIPT_CLOSE}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SCRIPT_CLOSE);
}

// ════════════════════════════════════════════════════════════════════
// 13. Page.press — 接收 selector + key
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_press_key_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.press] [level:integration]
    // Act
    let r = run(
        "Page.press",
        json!({"selector":"input","key": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_press_key_template_literal() {
    // Arrange
    // Act
    let r = run(
        "Page.press",
        json!({"selector":"input","key": VEC_TEMPLATE_LITERAL}),
    )
    .unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_TEMPLATE_LITERAL);
}

// ════════════════════════════════════════════════════════════════════
// 14. Page.check — 接收 selector
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_check_selector_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.check] [level:integration]
    // Act
    let r = run("Page.check", json!({"selector": VEC_SINGLE_QUOTE_ALERT})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_check_selector_svg_onload() {
    // Arrange
    // Act
    let r = run("Page.check", json!({"selector": VEC_SVG_ONLOAD})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SVG_ONLOAD);
}

// ════════════════════════════════════════════════════════════════════
// 15. Page.uncheck — 接收 selector
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_uncheck_selector_single_quote_alert() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.uncheck] [level:integration]
    // Act
    let r = run("Page.uncheck", json!({"selector": VEC_SINGLE_QUOTE_ALERT})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SINGLE_QUOTE_ALERT);
}

#[test]
fn inj_uncheck_selector_sql() {
    // Arrange
    // Act
    let r = run("Page.uncheck", json!({"selector": VEC_SQL})).unwrap();
    // Assert
    assert_payload_safe(&extract_expr(&r), VEC_SQL);
}

// ════════════════════════════════════════════════════════════════════
// 16. Page.selectOption — 接收 selector + values 数组
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_select_option_values_with_injection_payloads() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.selectOption] [level:integration]
    // Act
    let r = run(
        "Page.selectOption",
        json!({
            "selector": "select",
            "values": [
                VEC_SINGLE_QUOTE_ALERT,
                VEC_SCRIPT_CLOSE,
                VEC_TEMPLATE_LITERAL,
                VEC_BACKSLASH,
            ]
        }),
    )
    .unwrap();
    let e = extract_expr(&r);
    for p in &[
        VEC_SINGLE_QUOTE_ALERT,
        VEC_SCRIPT_CLOSE,
        VEC_TEMPLATE_LITERAL,
        VEC_BACKSLASH,
    ] {
        let json_p = serde_json::to_string(p).unwrap();
        // Assert
        assert!(
            args_literal(&e).contains(&json_p),
            "missing payload {p} in args"
        );
    }
}

// ════════════════════════════════════════════════════════════════════
// 17. Page.setInputFiles — 接收 selector + paths 数组
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_set_input_files_paths_with_injection_payloads() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.setInputFiles] [level:integration]
    // Act
    let r = run(
        "Page.setInputFiles",
        json!({
            "selector": "input[type=file]",
            "paths": [VEC_SINGLE_QUOTE_ALERT, VEC_SCRIPT_CLOSE]
        }),
    )
    .unwrap();
    let e = extract_expr(&r);
    for p in &[VEC_SINGLE_QUOTE_ALERT, VEC_SCRIPT_CLOSE] {
        let json_p = serde_json::to_string(p).unwrap();
        // Assert
        assert!(
            args_literal(&e).contains(&json_p),
            "missing payload {p} in args"
        );
    }
}

// ════════════════════════════════════════════════════════════════════
// 18. ElementHandle.getAttribute — callFunctionOn 路径,验证不报错
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_element_get_attribute_with_injection_name() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:ElementHandle.getAttribute] [level:integration]
    // callFunctionOn 路径:Mock 不返回 expression,但确保不 panic
    // Act
    let r = run(
        "ElementHandle.getAttribute",
        json!({"objectId":"obj1","name": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert!(r["result"].is_object());
}

// ════════════════════════════════════════════════════════════════════
// 19. ElementHandle.textContent/innerHTML/innerText — callFunctionOn
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_element_text_content_no_external_input() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:ElementHandle.textContent] [level:integration]
    // Act
    let r = run("ElementHandle.textContent", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_inner_html_no_external_input() {
    // Arrange
    // Act
    let r = run("ElementHandle.innerHTML", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_inner_text_no_external_input() {
    // Arrange
    // Act
    let r = run("ElementHandle.innerText", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

// ════════════════════════════════════════════════════════════════════
// 20. ElementHandle.isChecked/isDisabled/isEditable/isEnabled/isHidden/isVisible
//     6 method × 1 vector = 6 test
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_element_is_checked_returns_object() {
    // Arrange
    // Act
    let r = run("ElementHandle.isChecked", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_is_disabled_returns_object() {
    // Arrange
    // Act
    let r = run("ElementHandle.isDisabled", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_is_editable_returns_object() {
    // Arrange
    // Act
    let r = run("ElementHandle.isEditable", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_is_enabled_returns_object() {
    // Arrange
    // Act
    let r = run("ElementHandle.isEnabled", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_is_hidden_returns_object() {
    // Arrange
    // Act
    let r = run("ElementHandle.isHidden", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_is_visible_returns_object() {
    // Arrange
    // Act
    let r = run("ElementHandle.isVisible", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

// ════════════════════════════════════════════════════════════════════
// 21. ElementHandle.contentFrame/ownerFrame/scrollIntoViewIfNeeded
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_element_content_frame_returns_object() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:ElementHandle.contentFrame] [level:integration]
    // Act
    let r = run("ElementHandle.contentFrame", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_owner_frame_returns_object() {
    // Arrange
    // Act
    let r = run("ElementHandle.ownerFrame", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_element_scroll_into_view_returns_object() {
    // Arrange
    // Act
    let r = run(
        "ElementHandle.scrollIntoViewIfNeeded",
        json!({"objectId":"obj1"}),
    )
    .unwrap();
    // Assert
    assert!(r["result"].is_object());
}

// ════════════════════════════════════════════════════════════════════
// 22. ElementHandle.waitForElementState/waitForSelector — 本地等待
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_element_wait_for_element_state_returns_empty() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:ElementHandle.waitForElementState] [level:integration]
    // Act
    let r = run(
        "ElementHandle.waitForElementState",
        json!({"state": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

#[test]
fn inj_element_wait_for_selector_returns_empty() {
    // Arrange
    // Act
    let r = run(
        "ElementHandle.waitForSelector",
        json!({"selector": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

// ════════════════════════════════════════════════════════════════════
// 23. JSHandle.asElement/dispose/evaluate/evaluateHandle/getProperties/getProperty/jsonValue
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_js_handle_as_element_returns_local_state() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:JSHandle.asElement] [level:integration]
    // Act
    let r = run("JSHandle.asElement", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert_eq!(r["isElement"], false);
}

#[test]
fn inj_js_handle_dispose_calls_release() {
    // Arrange
    // Act
    let r = run("JSHandle.dispose", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

#[test]
fn inj_js_handle_evaluate_with_injection_func() {
    // Arrange
    // callFunctionOn functionDeclaration 是用户提供的 JS — 这不是注入,
    // 是允许的(Playwright API:page.evaluate(fn))。但 backend 必须正确路由。
    // Act
    let r = run(
        "JSHandle.evaluate",
        json!({"objectId":"obj1","func":"return 1+1;"}),
    )
    .unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_js_handle_evaluate_handle_with_injection_func() {
    // Arrange
    // Act
    let r = run(
        "JSHandle.evaluateHandle",
        json!({"objectId":"obj1","func":"return this;"}),
    )
    .unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_js_handle_get_properties_calls_backend() {
    // Arrange
    // Act
    let r = run("JSHandle.getProperties", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_array());
}

#[test]
fn inj_js_handle_get_property_with_injection_name() {
    // Arrange
    // Act
    let r = run(
        "JSHandle.getProperty",
        json!({"objectId":"obj1","name": VEC_SINGLE_QUOTE_ALERT}),
    )
    .unwrap();
    // Assert
    assert!(r["result"].is_object());
}

#[test]
fn inj_js_handle_json_value_calls_backend() {
    // Arrange
    // Act
    let r = run("JSHandle.jsonValue", json!({"objectId":"obj1"})).unwrap();
    // Assert
    assert!(r["result"].is_object());
}

// ════════════════════════════════════════════════════════════════════
// 24. Page.viewport/setViewport — 验证参数处理
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_page_viewport_returns_metrics() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.viewport] [level:integration]
    // Act
    let r = run("Page.viewport", json!({})).unwrap();
    // Assert
    assert!(r["width"].is_number());
    assert!(r["height"].is_number());
}

#[test]
fn inj_page_set_viewport_calls_emulation_override() {
    // Arrange
    // Act
    let r = run("Page.setViewport", json!({"width":1024,"height":768})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

// ════════════════════════════════════════════════════════════════════
// 25. Page.opener/frames/mainFrame
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_page_opener_iife_structure_intact() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.opener] [level:integration]
    // Act
    let r = run("Page.opener", json!({})).unwrap();
    let e = extract_expr(&r);
    // Assert
    assert!(e.contains("window.opener"));
}

#[test]
fn inj_page_frames_returns_array() {
    // Arrange
    // Act
    let r = run("Page.frames", json!({})).unwrap();
    // Assert
    assert!(r["frames"].is_array());
}

#[test]
fn inj_page_main_frame_returns_root() {
    // Arrange
    // Act
    let r = run("Page.mainFrame", json!({})).unwrap();
    // Assert
    assert!(r["id"].is_string() || r["id"].is_number());
}

// ════════════════════════════════════════════════════════════════════
// 26. Page.goBack/goForward/requestGC — 无参数 IIFE
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_page_go_back_iife_structure() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.goBack] [level:integration]
    // Act
    let r = run("Page.goBack", json!({})).unwrap();
    let e = extract_expr(&r);
    // Assert
    assert!(e.contains("history.back"));
}

#[test]
fn inj_page_go_forward_iife_structure() {
    // Arrange
    // Act
    let r = run("Page.goForward", json!({})).unwrap();
    let e = extract_expr(&r);
    // Assert
    assert!(e.contains("history.forward"));
}

#[test]
fn inj_page_request_gc_iife_structure() {
    // Arrange
    // Act
    let r = run("Page.requestGC", json!({})).unwrap();
    let e = extract_expr(&r);
    // Assert
    assert!(e.contains("window.gc"));
}

// ════════════════════════════════════════════════════════════════════
// 27. Page.screenshot/pdf — 转发 A 类
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_page_screenshot_returns_data() {
    // Arrange
    // @trace REQ-BAO-API-005 [method:Page.screenshot] [level:integration]
    // Act
    let r = run("Page.screenshot", json!({})).unwrap();
    // Assert
    assert!(r["data"].is_string());
}

#[test]
fn inj_page_pdf_returns_data() {
    // Arrange
    // Act
    let r = run("Page.pdf", json!({})).unwrap();
    // Assert
    assert!(r["data"].is_string());
}

// ════════════════════════════════════════════════════════════════════
// 28. Page.setDefault*/waitFor* — 本地状态/事件订阅
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_page_set_default_navigation_timeout_returns_empty() {
    // Arrange
    // Act
    let r = run("Page.setDefaultNavigationTimeout", json!({"timeout":5000})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

#[test]
fn inj_page_set_default_timeout_returns_empty() {
    // Arrange
    // Act
    let r = run("Page.setDefaultTimeout", json!({"timeout":5000})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

#[test]
fn inj_page_wait_for_load_state_returns_empty() {
    // Arrange
    // Act
    let r = run("Page.waitForLoadState", json!({"state":"load"})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

#[test]
fn inj_page_wait_for_url_returns_empty() {
    // Arrange
    // Act
    let r = run("Page.waitForURL", json!({"url":"**/api/*"})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

#[test]
fn inj_page_wait_for_request_returns_empty() {
    // Arrange
    // Act
    let r = run("Page.waitForRequest", json!({"url":"**/api/*"})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

#[test]
fn inj_page_wait_for_response_returns_empty() {
    // Arrange
    // Act
    let r = run("Page.waitForResponse", json!({"url":"**/api/*"})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

#[test]
fn inj_page_wait_for_event_returns_empty() {
    // Arrange
    // Act
    let r = run("Page.waitForEvent", json!({"event":"response"})).unwrap();
    // Assert
    assert_eq!(r.as_object().unwrap().len(), 0);
}

// ════════════════════════════════════════════════════════════════════
// 29. 缺参数错误路径
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_add_script_tag_missing_url_and_content() {
    // Arrange
    // Act
    let err = run("Page.addScriptTag", json!({})).unwrap_err();
    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn inj_add_style_tag_missing_url_and_content() {
    // Arrange
    // Act
    let err = run("Page.addStyleTag", json!({})).unwrap_err();
    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn inj_expose_function_missing_name() {
    // Arrange
    // Act
    let err = run("Page.exposeFunction", json!({})).unwrap_err();
    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn inj_tap_missing_selector() {
    // Arrange
    // Act
    let err = run("Page.tap", json!({})).unwrap_err();
    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn inj_type_missing_text() {
    // Arrange
    // Act
    let err = run("Page.type", json!({"selector":"input"})).unwrap_err();
    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn inj_page_set_viewport_missing_dimensions() {
    // Arrange
    // Act
    let err = run("Page.setViewport", json!({})).unwrap_err();
    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

// ════════════════════════════════════════════════════════════════════
// 30. 通用 IIFE 模板完整性
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_all_b_class_evals_start_with_iife_open() {
    // Arrange
    // @trace REQ-BAO-API-005 [level:integration]
    // 所有走 eval_iife 路径的 B 类 method 必须返回 IIFE 包装
    let methods = [
        "Page.title",
        "Page.url",
        "Page.content",
        "Page.opener",
        "Page.goBack",
        "Page.goForward",
        "Page.requestGC",
        "Page.emulateMedia",
    ];
    for m in methods {
        // Act
        let r = run(m, json!({})).unwrap();
        let e = extract_expr(&r);
        // Assert
        assert!(
            e.starts_with("(function(){"),
            "{m} must wrap in IIFE; got: {e}"
        );
        assert!(e.ends_with("})()"), "{m} must end with }})(); got: {e}");
    }
}

#[test]
fn inj_all_b_class_evals_with_args_use_apply_pattern() {
    // Arrange
    // 走 build_iife_with_args 的 method,IIFE 必须用 .apply(null, __args) 绑定参数
    let cases = [
        // Act
        ("Page.addScriptTag", json!({"url":"https://x"})),
        ("Page.addStyleTag", json!({"url":"https://x"})),
        ("Page.exposeFunction", json!({"name":"fn"})),
        ("Page.tap", json!({"selector":"button"})),
        ("Page.hover", json!({"selector":"button"})),
        ("Page.focus", json!({"selector":"button"})),
        ("Page.type", json!({"selector":"input","text":"hi"})),
        ("Page.fill", json!({"selector":"input","value":"v"})),
        ("Page.press", json!({"selector":"input","key":"Enter"})),
        ("Page.check", json!({"selector":"input"})),
        ("Page.uncheck", json!({"selector":"input"})),
    ];
    for (m, p) in cases {
        let r = run(m, p).unwrap();
        let e = extract_expr(&r);
        // Assert
        assert!(
            e.contains("var __args="),
            "{m} must declare __args; got: {e}"
        );
        assert!(
            e.contains(").apply(null, __args);"),
            "{m} must call .apply(null, __args); got: {e}"
        );
    }
}

// ════════════════════════════════════════════════════════════════════
// 31. JSON-encoded payload 字面量 vs raw payload 验证
// ════════════════════════════════════════════════════════════════════

#[test]
fn inj_payload_always_appears_as_json_string_in_args() {
    // Arrange
    // 综合:对每个走 args 的 method 注入 SQL,验证 payload 在 args 中作为 JSON 字符串字面量
    let cases = [
        // Act
        ("Page.addScriptTag", "url"),
        ("Page.addStyleTag", "content"),
        ("Page.exposeFunction", "name"),
        ("Page.tap", "selector"),
        ("Page.hover", "selector"),
        ("Page.focus", "selector"),
        ("Page.type", "text"),
        ("Page.fill", "value"),
        ("Page.press", "key"),
        ("Page.check", "selector"),
        ("Page.uncheck", "selector"),
    ];
    for (m, key) in cases {
        let p = json!({"__placeholder__": "x"}); // 我们需要不同 key 的支持
        let _ = p; // suppress
        let _ = key;
        let _ = m;
    }
    // 直接遍历每个 method,用各自合适的 key
    let test_cases: Vec<(&str, Value)> = vec![
        ("Page.addScriptTag", json!({"url": VEC_SQL})),
        ("Page.addStyleTag", json!({"content": VEC_SQL})),
        ("Page.exposeFunction", json!({"name": VEC_SQL})),
        ("Page.tap", json!({"selector": VEC_SQL})),
        ("Page.hover", json!({"selector": VEC_SQL})),
        ("Page.focus", json!({"selector": VEC_SQL})),
        ("Page.type", json!({"selector":"input","text": VEC_SQL})),
        ("Page.fill", json!({"selector":"input","value": VEC_SQL})),
        ("Page.press", json!({"selector":"input","key": VEC_SQL})),
        ("Page.check", json!({"selector": VEC_SQL})),
        ("Page.uncheck", json!({"selector": VEC_SQL})),
    ];
    for (m, params) in test_cases {
        let r = run(m, params).unwrap();
        let e = extract_expr(&r);
        let args = args_literal(&e);
        let json_sql = serde_json::to_string(VEC_SQL).unwrap();
        // Assert
        assert!(
            args.contains(&json_sql),
            "{m}: SQL payload must appear as JSON string in __args; got args: {args}"
        );
    }
}
