// @trace REQ-CDP-001  REQ-CDP-003: Bridge handler — routes BridgeCommand to servo WebView operations
// Runs on the main thread during the event loop to process CDP commands.

use bao_cdp::servo_bridge::{BridgeCommand, BridgeResponse};
use base64::Engine;
use serde_json::Value;

use crate::error::BrowserError;
use crate::page::PageHandle;
use crate::screenshot::ScreenshotFormat;

/// Process a single bridge command by dispatching to the active page.
pub fn handle_bridge_command(cmd: BridgeCommand, page: &PageHandle) -> BridgeResponse {
    let result = match cmd {
        BridgeCommand::Navigate { url } => cmd_navigate(page, &url),
        BridgeCommand::EvaluateJs { expression, return_by_value } => cmd_evaluate(page, &expression, return_by_value),
        BridgeCommand::TakeScreenshot { format, quality: _ } => cmd_screenshot(page, &format),
        BridgeCommand::GetTitle => cmd_get_title(page),
        BridgeCommand::GetUrl => cmd_get_url(page),
        BridgeCommand::GetDocument => cmd_get_document(page),
        BridgeCommand::QuerySelector { selector } => cmd_query_selector(page, &selector),
        BridgeCommand::QuerySelectorAll { selector } => cmd_query_selector_all(page, &selector),
        BridgeCommand::GetOuterHtml { .. } => cmd_get_outer_html(page),
        BridgeCommand::SetAttributeValue { node_id: _, name, value } => cmd_set_attribute(page, &name, &value),
        BridgeCommand::DispatchMouseEvent { event_type, x, y, button, click_count } => {
            cmd_mouse_event(page, &event_type, x, y, button, click_count)
        }
        BridgeCommand::DispatchKeyEvent { event_type, key, code, text } => {
            cmd_key_event(page, &event_type, &key, &code, text.as_deref())
        }
        BridgeCommand::InsertText { text } => cmd_insert_text(page, &text),
        BridgeCommand::SetViewport { width, height, device_scale_factor: _ } => cmd_set_viewport(page, width, height),
        BridgeCommand::SetUserAgent { user_agent } => cmd_set_user_agent(page, &user_agent),
        BridgeCommand::AddScriptToEvaluateOnNewDocument { source } => cmd_add_script(page, &source),
        BridgeCommand::Reload { ignore_cache: _ } => cmd_reload(page),
        BridgeCommand::GoBack | BridgeCommand::GoForward | BridgeCommand::StopLoading => {
            Ok(serde_json::json!({}))
        }
        BridgeCommand::ClosePage => {
            let _ = page.close();
            Ok(serde_json::json!({}))
        }
        _ => Err("unsupported bridge command".into()),
    };
    BridgeResponse { result }
}

fn to_browser_error(e: BrowserError) -> String {
    format!("{e}")
}

fn cmd_navigate(page: &PageHandle, url: &str) -> Result<Value, String> {
    page.navigate(url).map_err(to_browser_error)?;
    Ok(serde_json::json!({
        "frameId": "0",
        "loaderId": format!("{:016x}", url.len() as u64)
    }))
}

fn cmd_evaluate(page: &PageHandle, expression: &str, return_by_value: bool) -> Result<Value, String> {
    let result = page.evaluate_js(expression).map_err(to_browser_error)?;
    if return_by_value {
        let parsed: Result<Value, _> = serde_json::from_str(&result);
        let (value_type, value) = match parsed {
            Ok(v) => (json_type(&v), v),
            Err(_) => (json_type_string(&result), serde_json::json!(result)),
        };
        Ok(serde_json::json!({
            "result": {
                "type": value_type,
                "value": value,
            },
            "exceptionDetails": null
        }))
    } else {
        Ok(serde_json::json!({
            "result": {
                "type": json_type_string(&result),
                "description": result,
            },
            "exceptionDetails": null
        }))
    }
}

fn cmd_screenshot(page: &PageHandle, format: &str) -> Result<Value, String> {
    let fmt = match format {
        "jpeg" => ScreenshotFormat::Jpeg,
        _ => ScreenshotFormat::Png,
    };
    let bytes = page.take_screenshot(fmt).map_err(to_browser_error)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(serde_json::json!({ "data": b64 }))
}

fn cmd_get_title(page: &PageHandle) -> Result<Value, String> {
    let title = page.page_title().unwrap_or_default();
    Ok(serde_json::json!(title))
}

fn cmd_get_url(page: &PageHandle) -> Result<Value, String> {
    let url = page.current_url().unwrap_or_else(|| "about:blank".into());
    Ok(serde_json::json!(url))
}

fn cmd_get_document(page: &PageHandle) -> Result<Value, String> {
    // Use evaluate_js to extract DOM structure via JS
    let js = r#"
        (function() {
            function walk(node, id) {
                var result = {
                    nodeId: id,
                    backendNodeId: id,
                    nodeType: node.nodeType,
                    nodeName: node.nodeName,
                    localName: node.localName || '',
                    nodeValue: node.nodeValue || '',
                };
                if (node.childNodes && node.childNodes.length > 0) {
                    result.childNodeCount = node.childNodes.length;
                    result.children = [];
                    for (var i = 0; i < Math.min(node.childNodes.length, 20); i++) {
                        result.children.push(walk(node.childNodes[i], id * 100 + i + 1));
                    }
                }
                return result;
            }
            return JSON.stringify(walk(document, 1));
        })()
    "#;
    let doc_str = page.evaluate_js(js).map_err(to_browser_error)?;
    let doc_val: Value = serde_json::from_str(&doc_str).unwrap_or_else(|_| serde_json::json!({}));
    Ok(serde_json::json!({ "root": doc_val }))
}

fn cmd_query_selector(page: &PageHandle, selector: &str) -> Result<Value, String> {
    let js = format!(
        "(function() {{ var e = document.querySelector({}); return e ? 1 : 0; }})()",
        serde_json::to_string(selector).unwrap_or_default()
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    let node_id: i64 = result.trim().parse().unwrap_or(0);
    Ok(serde_json::json!({ "nodeId": node_id }))
}

fn cmd_query_selector_all(page: &PageHandle, selector: &str) -> Result<Value, String> {
    let js = format!(
        "(function() {{ return document.querySelectorAll({}).length; }})()",
        serde_json::to_string(selector).unwrap_or_default()
    );
    let count_str = page.evaluate_js(&js).map_err(to_browser_error)?;
    let count: i64 = count_str.trim().parse().unwrap_or(0);
    let ids: Vec<i64> = (1..=count).collect();
    Ok(serde_json::json!({ "nodeIds": ids }))
}

fn cmd_get_outer_html(page: &PageHandle) -> Result<Value, String> {
    let js = "document.documentElement.outerHTML";
    let html = page.evaluate_js(js).map_err(to_browser_error)?;
    Ok(serde_json::json!({ "outerHTML": html }))
}

fn cmd_set_attribute(page: &PageHandle, name: &str, value: &str) -> Result<Value, String> {
    let js = format!(
        "(function() {{ document.querySelector('[data-cdp]')?.setAttribute({}, {}); }})()",
        serde_json::to_string(name).unwrap_or_default(),
        serde_json::to_string(value).unwrap_or_default(),
    );
    let _ = page.evaluate_js(&js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_mouse_event(_page: &PageHandle, _event_type: &str, _x: f64, _y: f64, _button: Option<i64>, _click_count: Option<i64>) -> Result<Value, String> {
    // Mouse event dispatch through servo requires InputEvent API
    // For now, acknowledge the command
    Ok(serde_json::json!({}))
}

fn cmd_key_event(_page: &PageHandle, _event_type: &str, _key: &str, _code: &str, _text: Option<&str>) -> Result<Value, String> {
    Ok(serde_json::json!({}))
}

fn cmd_insert_text(page: &PageHandle, text: &str) -> Result<Value, String> {
    let js = format!(
        "(function() {{ var el = document.activeElement; if (el && 'value' in el) el.value += {}; }})()",
        serde_json::to_string(text).unwrap_or_default(),
    );
    let _ = page.evaluate_js(&js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_set_viewport(_page: &PageHandle, _width: u32, _height: u32) -> Result<Value, String> {
    // Viewport resize requires re-creating the rendering context
    Ok(serde_json::json!({}))
}

fn cmd_set_user_agent(page: &PageHandle, ua: &str) -> Result<Value, String> {
    let js = format!(
        "Object.defineProperty(navigator, 'userAgent', {{ get: function() {{ return {}; }} }});",
        serde_json::to_string(ua).unwrap_or_default(),
    );
    let _ = page.evaluate_js(&js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_add_script(page: &PageHandle, source: &str) -> Result<Value, String> {
    let _ = page.evaluate_js(source).map_err(to_browser_error)?;
    Ok(serde_json::json!({ "identifier": "1" }))
}

fn cmd_reload(page: &PageHandle) -> Result<Value, String> {
    let url = page.current_url().unwrap_or_else(|| "about:blank".into());
    page.navigate(&url).map_err(to_browser_error)?;
    Ok(serde_json::json!({ "frameId": "0", "loaderId": "0" }))
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "undefined",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "object",
        Value::Object(_) => "object",
    }
}

fn json_type_string(s: &str) -> &'static str {
    if s.is_empty() || s == "undefined" {
        "undefined"
    } else if s == "null" {
        "object"
    } else if s == "true" || s == "false" {
        "boolean"
    } else if s.parse::<f64>().is_ok() {
        "number"
    } else if s.starts_with('{') || s.starts_with('[') {
        "object"
    } else {
        "string"
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn json_type_null_returns_undefined() {
        assert_eq!(super::json_type(&json!(null)), "undefined");
    }

    #[test]
    fn json_type_bool_returns_boolean() {
        assert_eq!(super::json_type(&json!(true)), "boolean");
        assert_eq!(super::json_type(&json!(false)), "boolean");
    }

    #[test]
    fn json_type_number_returns_number() {
        assert_eq!(super::json_type(&json!(42)), "number");
        assert_eq!(super::json_type(&json!(3.14)), "number");
        assert_eq!(super::json_type(&json!(0)), "number");
        assert_eq!(super::json_type(&json!(-1)), "number");
    }

    #[test]
    fn json_type_string_returns_string() {
        assert_eq!(super::json_type(&json!("hello")), "string");
        assert_eq!(super::json_type(&json!("")), "string");
    }

    #[test]
    fn json_type_array_returns_object() {
        assert_eq!(super::json_type(&json!([1, 2, 3])), "object");
        assert_eq!(super::json_type(&json!([])), "object");
    }

    #[test]
    fn json_type_object_returns_object() {
        assert_eq!(super::json_type(&json!({"a": 1})), "object");
        assert_eq!(super::json_type(&json!({})), "object");
    }

    #[test]
    fn json_type_string_empty_returns_undefined() {
        assert_eq!(super::json_type_string(""), "undefined");
    }

    #[test]
    fn json_type_string_undefined_returns_undefined() {
        assert_eq!(super::json_type_string("undefined"), "undefined");
    }

    #[test]
    fn json_type_string_null_returns_object() {
        assert_eq!(super::json_type_string("null"), "object");
    }

    #[test]
    fn json_type_string_true_returns_boolean() {
        assert_eq!(super::json_type_string("true"), "boolean");
    }

    #[test]
    fn json_type_string_false_returns_boolean() {
        assert_eq!(super::json_type_string("false"), "boolean");
    }

    #[test]
    fn json_type_string_integer_returns_number() {
        assert_eq!(super::json_type_string("42"), "number");
        assert_eq!(super::json_type_string("0"), "number");
        assert_eq!(super::json_type_string("-7"), "number");
    }

    #[test]
    fn json_type_string_float_returns_number() {
        assert_eq!(super::json_type_string("3.14"), "number");
        assert_eq!(super::json_type_string("-0.5"), "number");
    }

    #[test]
    fn json_type_string_object_brace_returns_object() {
        assert_eq!(super::json_type_string("{\"a\":1}"), "object");
    }

    #[test]
    fn json_type_string_array_bracket_returns_object() {
        assert_eq!(super::json_type_string("[1,2,3]"), "object");
    }

    #[test]
    fn json_type_string_regular_text_returns_string() {
        assert_eq!(super::json_type_string("hello world"), "string");
        assert_eq!(super::json_type_string("some result"), "string");
    }

    // ─── json_type edge cases ─────────────────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn json_type_large_number() {
        assert_eq!(super::json_type(&json!(i64::MAX)), "number");
        assert_eq!(super::json_type(&json!(f64::MAX)), "number");
    }

    #[test]
    fn json_type_nested_object() {
        assert_eq!(super::json_type(&json!({"a": {"b": 1}})), "object");
    }

    #[test]
    fn json_type_nested_array() {
        assert_eq!(super::json_type(&json!([[1, 2], [3, 4]])), "object");
    }

    // ─── json_type_string edge cases ──────────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn json_type_string_scientific_notation() {
        assert_eq!(super::json_type_string("1e10"), "number");
        assert_eq!(super::json_type_string("-2.5e-3"), "number");
    }

    #[test]
    fn json_type_string_whitespace_is_string() {
        assert_eq!(super::json_type_string("  "), "string");
        assert_eq!(super::json_type_string(" 42"), "string");
    }

    #[test]
    fn json_type_string_special_strings() {
        // NaN and Infinity parse as f64, so they're "number"
        assert_eq!(super::json_type_string("NaN"), "number");
        assert_eq!(super::json_type_string("Infinity"), "number");
        assert_eq!(super::json_type_string("[object Object]"), "object");
    }

    #[test]
    fn json_type_string_negative_zero() {
        assert_eq!(super::json_type_string("-0"), "number");
        assert_eq!(super::json_type_string("0.0"), "number");
    }
}
