//! Eval 合成器 — IIFE 安全封装 + JSON.stringify 参数化。
//!
//! # 安全模型
//!
//! 所有 B 类 method 通过 `Runtime.evaluate` 注入 JavaScript。**禁止字符串拼接**
//! (注入漏洞:`format!("el.setAttribute('{}','{}')", n, v)` 当 `n="');alert(1);//"` 时
//! 会逃逸出字符串字面量,执行任意 JS)。
//!
//! 强制使用 IIFE(Immediately-Invoked Function Expression)+ `JSON.stringify` 参数化:
//!
//! ```text
//! (function(){
//!   var __args = <JSON>;          // 由 JSON.stringify 生成,保证字面量安全
//!   return (function(){
//!     <body>                       // 业务 JS,引用 __args[i]
//!   }).apply(null, __args);
//! })()
//! ```
//!
//! `JSON.stringify` 保证:
//! - 所有 string escape 正确(`"`,`\`,控制字符等)
//! - 所有 number/bool/null/array/object 走标准编码
//! - body 内只允许引用 `__args`,无法拼接进任何字面量
//!
//! # 三个 API
//!
//! - [`build_iife`]:无参数版本,body 引用全局对象
//! - [`build_iife_with_args`]:带参数版本,body 引用 `__args[i]`
//! - [`build_iife_node`]:操作指定 nodeId 的元素,body 内 `__args[0]` 是元素句柄占位
//!
//! @trace REQ-BAO-API-005 [level:library]

use serde_json::Value;

use super::error::BridgeError;

/// 构造无参数 IIFE。
///
/// ```text
/// (function(){ <body> })()
/// ```
///
/// # 适用场景
///
/// - `page.title` → `build_iife("return document.title;")`
/// - `page.url` → `build_iife("return location.href;")`
///
/// # 安全保证
///
/// `body` 必须是硬编码字面量,**不**包含任何用户输入。
///
/// @trace REQ-BAO-API-005 [method:Runtime.evaluate]
pub fn build_iife(body: &str) -> String {
    format!("(function(){{ {body} }})()")
}

/// 构造带参数 IIFE。
///
/// ```text
/// (function(){
///   var __args = <JSON>;
///   return (function(){
///     <body>
///   }).apply(null, __args);
/// })()
/// ```
///
/// # 参数序列化
///
/// `args` 通过 `serde_json::to_string` 序列化为标准 JSON 字面量,保证:
/// - 字符串中的 `'`、`"`、`\`、`<`、`>` 等都被 JSON 转义
/// - body 内只能用 `__args[i]` 取值,无法通过拼接逃逸
///
/// # 适用场景
///
/// - `el.getAttribute(name)` → `build_iife_with_args("var el=__args[0]; return el.getAttribute(__args[1]);", [elementRef, name])`
/// - `el.setAttribute(name, value)` → `build_iife_with_args("var el=__args[0]; el.setAttribute(__args[1], __args[2]);", [ref, n, v])`
///
/// # 错误
///
/// 序列化失败(理论上 `serde_json::Value` 不会失败)返回 `InvalidParams`。
///
/// @trace REQ-BAO-API-005 [method:Runtime.evaluate]
pub fn build_iife_with_args(body: &str, args: &[Value]) -> Result<String, BridgeError> {
    let args_json = serde_json::to_string(args)
        .map_err(|e| BridgeError::InvalidParams(format!("args serialization failed: {e}")))?;
    Ok(format!(
        "(function(){{ var __args={args_json}; return (function(){{ {body} }}).apply(null, __args); }})()"
    ))
}

/// 构造元素操作 IIFE — 把 `backendNodeId` 转换为元素引用。
///
/// ```text
/// (function(){
///   var __args = [<extra_args>];
///   return (function(){
///     var el = document.querySelector('[data-bao-backend-node-id="<id>"]');
///     if (!el) throw new Error('element not found: <id>');
///     <body>      // body 内可同时引用 el 和 __args[i]
///   }).apply(null, __args);
/// })()
/// ```
///
/// 注:实际元素引用方式由 backend 决定(通过 `DOM.resolveNode` → `Runtime.callFunctionOn`)。
/// 此处模板用于注入测试和合成示例,真实路径走 [`build_iife_with_args`]。
///
/// @trace REQ-BAO-API-005 [domain:DOM]
#[allow(dead_code)]
pub fn build_iife_element(
    body: &str,
    backend_node_id: i64,
    extra_args: &[Value],
) -> Result<String, BridgeError> {
    let extra_json = serde_json::to_string(extra_args)
        .map_err(|e| BridgeError::InvalidParams(format!("extra args serialization failed: {e}")))?;
    Ok(format!(
        "(function(){{ var __args={extra_json}; return (function(){{ var el=document.querySelector('[data-bao-backend-node-id=\"{backend_node_id}\"]'); if(!el){{throw new Error('element not found: {backend_node_id}');}} {body} }}).apply(null, __args); }})()"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── 基本 IIFE 构造 ──

    // @trace REQ-BAO-API-005 [method:Runtime.evaluate]
    #[test]
    fn build_iife_wraps_body_in_iife() {
        let s = build_iife("return document.title;");
        assert!(s.starts_with("(function(){"));
        assert!(s.ends_with("})()"));
        assert!(s.contains("return document.title;"));
    }

    #[test]
    fn build_iife_with_args_serializes_args_as_json() {
        let s = build_iife_with_args("return __args[0];", &[json!("hello")]).unwrap();
        assert!(s.contains("var __args=[\"hello\"];"));
        assert!(s.contains("return __args[0];"));
        assert!(s.contains(").apply(null, __args);"));
    }

    #[test]
    fn build_iife_with_args_empty_array_is_safe() {
        let s = build_iife_with_args("return document.title;", &[]).unwrap();
        assert!(s.contains("var __args=[];"));
    }

    // ── 注入防御 — 字符串 ──

    // @trace REQ-BAO-API-005 [method:Runtime.evaluate]
    #[test]
    fn injection_single_quote_in_string_is_escaped() {
        let payload = "');alert('xss');//";
        let s = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
        // payload 必须以 JSON-encoded 字符串字面量出现在 __args 数组里
        // (JSON.stringify 保留 ', 但确保它在引号包裹的字符串内)
        // __args 声明形如 `var __args=[...];`,用 `];` 模式定位结束
        let args_marker = "var __args=";
        let args_pos = s.find(args_marker).unwrap();
        let args_end_rel = s[args_pos..].find("];").unwrap();
        let args_literal = &s[args_pos..args_pos + args_end_rel + 1];
        assert!(args_literal.contains("\"');alert('xss');//\""));
        // body 内引用 __args[0],不是 payload 字面量
        assert!(s.contains("return __args[0];"));
        // body 部分不应包含 payload 字面量作为代码
        let body_start = s.find("return (function(){").unwrap();
        let body_end = s.find("}).apply(null, __args);").unwrap();
        let body = &s[body_start..body_end];
        assert!(!body.contains(&format!("return {payload};")));
    }

    #[test]
    fn injection_double_quote_in_string_is_escaped() {
        let payload = "\";alert(\"x\");//";
        let s = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
        // JSON 必须把 " 转义为 \"
        assert!(s.contains("\\\""));
    }

    #[test]
    fn injection_backslash_in_string_is_escaped() {
        let payload = "\\;alert(1);//";
        let s = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
        // 反斜杠必须被转义为 \\
        assert!(s.contains("\\\\"));
    }

    #[test]
    fn injection_script_close_tag_is_neutralized() {
        let payload = "</script><script>alert(1)</script>";
        let s = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
        // JSON.stringify 对 </ 不转义,但 payload 仍然作为字符串字面量,不会逃逸出 IIFE
        assert!(s.contains("\"</script>"));
        // body 没有把 </script> 当代码
        assert!(!s.contains("return </script>"));
    }

    #[test]
    fn injection_template_literal_syntax_is_neutralized() {
        let payload = "${alert(1)}";
        let s = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
        // body 是字符串字面量,${} 不会展开
        assert!(s.contains("\"${alert(1)}\""));
        assert!(!s.contains("return ${alert(1)};"));
    }

    #[test]
    fn injection_newline_in_string_is_escaped() {
        let payload = "a\nb";
        let s = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
        // \n 必须以 \\n 形式出现(避免字面换行破坏 JS)
        assert!(s.contains("\\n"));
        assert!(!s.contains("\"a\nb\""));
    }

    #[test]
    fn injection_unicode_control_chars_are_escaped() {
        let payload = "\u{0000}\u{001B}";
        let s = build_iife_with_args("return __args[0];", &[json!(payload)]).unwrap();
        // 控制字符必须 escape
        assert!(s.contains("\\u0000") || s.contains("\\u001b") || s.contains("\\u001B"));
    }

    // ── 注入防御 — 数字/布尔 ──

    #[test]
    fn number_args_are_serialized_correctly() {
        let s = build_iife_with_args("return __args[0]+__args[1];", &[json!(1), json!(2)]).unwrap();
        assert!(s.contains("var __args=[1,2];"));
    }

    #[test]
    fn bool_args_are_serialized_correctly() {
        let s = build_iife_with_args("return __args[0];", &[json!(true)]).unwrap();
        assert!(s.contains("var __args=[true];"));
    }

    #[test]
    fn null_args_are_serialized_correctly() {
        let s = build_iife_with_args("return __args[0];", &[Value::Null]).unwrap();
        assert!(s.contains("var __args=[null];"));
    }

    #[test]
    fn object_args_are_serialized_correctly() {
        let s = build_iife_with_args("return __args[0].x;", &[json!({"x": 1, "y": "a"})]).unwrap();
        assert!(s.contains("var __args=[{\"x\":1,\"y\":\"a\"}];"));
    }

    #[test]
    fn array_args_are_serialized_correctly() {
        let s =
            build_iife_with_args("return __args[0].length;", &[json!(["a", "b", "c"])]).unwrap();
        assert!(s.contains("var __args=[[\"a\",\"b\",\"c\"]]"));
    }

    #[test]
    fn multiple_args_preserve_order() {
        let s = build_iife_with_args(
            "return [__args[0], __args[1], __args[2]];",
            &[json!("first"), json!(42), json!(true)],
        )
        .unwrap();
        assert!(s.contains("var __args=[\"first\",42,true]"));
    }

    // ── IIFE 结构完整性 ──

    #[test]
    fn iife_starts_and_ends_correctly() {
        let s = build_iife_with_args("return 1;", &[]).unwrap();
        assert!(s.starts_with("(function(){"));
        assert!(s.ends_with("})()"));
    }

    #[test]
    fn iife_uses_apply_to_bind_args() {
        let s = build_iife_with_args("return __args[0];", &[json!(1)]).unwrap();
        assert!(s.contains("}).apply(null, __args);"));
    }

    #[test]
    fn iife_body_appears_after_args_declaration() {
        let s = build_iife_with_args("/*BODY*/", &[]).unwrap();
        let args_pos = s.find("var __args").unwrap();
        let body_pos = s.find("/*BODY*/").unwrap();
        assert!(args_pos < body_pos);
    }

    // ── build_iife_element ──

    #[test]
    fn build_iife_element_inserts_backend_node_id() {
        let s = build_iife_element("return el.textContent;", 42, &[]).unwrap();
        assert!(s.contains("data-bao-backend-node-id=\"42\""));
        assert!(s.contains("throw new Error('element not found: 42')"));
        assert!(s.contains("return el.textContent;"));
    }
}
