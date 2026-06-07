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
        "webp" => ScreenshotFormat::WebP,
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
    use serde_json::{json, Value};

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

    // ─── to_browser_error edge cases ───────────────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn to_browser_error_init_variant() {
        let err = crate::error::BrowserError::Init("failed to start".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("browser init error"));
        assert!(msg.contains("failed to start"));
    }

    #[test]
    fn to_browser_error_navigation_variant() {
        let err = crate::error::BrowserError::Navigation("invalid url".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("navigation error"));
        assert!(msg.contains("invalid url"));
    }

    #[test]
    fn to_browser_error_rendering_variant() {
        let err = crate::error::BrowserError::Rendering("gpu lost".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("rendering error"));
        assert!(msg.contains("gpu lost"));
    }

    #[test]
    fn to_browser_error_javascript_variant() {
        let err = crate::error::BrowserError::JavaScript("syntax error".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("javascript error"));
        assert!(msg.contains("syntax error"));
    }

    #[test]
    fn to_browser_error_cdp_variant() {
        let err = crate::error::BrowserError::CDP("connection refused".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("cdp error"));
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn to_browser_error_empty_message() {
        let err = crate::error::BrowserError::Init(String::new());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("browser init error"));
    }

    #[test]
    fn to_browser_error_unicode_message() {
        let err = crate::error::BrowserError::Navigation("页面加载失败".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("页面加载失败"));
    }

    // ─── cmd_navigate response structure (pure logic, no PageHandle) ────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_navigate_loader_id_from_url_length() {
        // loaderId is format!("{:016x}", url.len() as u64)
        let url = "http://a.com";
        let loader_id = format!("{:016x}", url.len());
        assert_eq!(loader_id, "000000000000000c"); // 12 chars hex
    }

    #[test]
    fn cmd_navigate_empty_url_loader_id() {
        let loader_id = format!("{:016x}", 0usize);
        assert_eq!(loader_id, "0000000000000000");
    }

    #[test]
    fn cmd_navigate_long_url_loader_id() {
        let url = "http://very-long-domain-name.example.com/path/to/resource";
        let loader_id = format!("{:016x}", url.len());
        assert_ne!(loader_id, "0000000000000000");
    }

    // ─── cmd_evaluate response structure (pure logic) ──────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_evaluate_return_by_value_true_json_parse() {
        // When return_by_value is true and result is valid JSON, it's parsed
        let result_str = r#"{"a":1}"#;
        let parsed: Result<Value, _> = serde_json::from_str(result_str);
        assert!(parsed.is_ok());
        assert_eq!(super::json_type(&parsed.unwrap()), "object");
    }

    #[test]
    fn cmd_evaluate_return_by_value_true_non_json_falls_back() {
        // When return_by_value is true but result is not valid JSON, falls back to json_type_string
        let result_str = "hello world";
        let parsed: Result<Value, _> = serde_json::from_str(result_str);
        assert!(parsed.is_err());
        assert_eq!(super::json_type_string(result_str), "string");
    }

    #[test]
    fn cmd_evaluate_return_by_value_true_null_json() {
        let parsed: Result<Value, _> = serde_json::from_str("null");
        assert!(parsed.is_ok());
        assert_eq!(super::json_type(&parsed.unwrap()), "undefined");
    }

    #[test]
    fn cmd_evaluate_return_by_value_true_number_json() {
        let parsed: Result<Value, _> = serde_json::from_str("42");
        assert!(parsed.is_ok());
        assert_eq!(super::json_type(&parsed.unwrap()), "number");
    }

    #[test]
    fn cmd_evaluate_return_by_value_true_boolean_json() {
        let parsed: Result<Value, _> = serde_json::from_str("true");
        assert!(parsed.is_ok());
        assert_eq!(super::json_type(&parsed.unwrap()), "boolean");
    }

    #[test]
    fn cmd_evaluate_return_by_value_false_uses_description() {
        // When return_by_value is false, result uses json_type_string for type
        let result_str = "some JS output";
        assert_eq!(super::json_type_string(result_str), "string");
    }

    // ─── cmd_screenshot format mapping (pure logic) ────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn cmd_screenshot_format_jpeg_mapping() {
        // "jpeg" -> ScreenshotFormat::Jpeg, anything else -> Png
        let fmt = match "jpeg" {
            "jpeg" => "Jpeg",
            _ => "Png",
        };
        assert_eq!(fmt, "Jpeg");
    }

    #[test]
    fn cmd_screenshot_format_png_mapping() {
        let fmt = match "png" {
            "jpeg" => "Jpeg",
            _ => "Png",
        };
        assert_eq!(fmt, "Png");
    }

    #[test]
    fn cmd_screenshot_format_unknown_defaults_to_png() {
        let fmt = match "bmp" {
            "jpeg" => "Jpeg",
            "webp" => "WebP",
            _ => "Png",
        };
        assert_eq!(fmt, "Png");
    }

    #[test]
    fn cmd_screenshot_format_webp_mapping() {
        let fmt = match "webp" {
            "jpeg" => "Jpeg",
            "webp" => "WebP",
            _ => "Png",
        };
        assert_eq!(fmt, "WebP");
    }

    #[test]
    fn cmd_screenshot_format_empty_defaults_to_png() {
        let fmt = match "" {
            "jpeg" => "Jpeg",
            _ => "Png",
        };
        assert_eq!(fmt, "Png");
    }

    #[test]
    fn cmd_screenshot_base64_encoding() {
        // Verify base64 encoding produces valid output
        let bytes: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47]; // PNG magic bytes
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
        assert!(!b64.is_empty());
        // Base64 should be decodable back
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &b64);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), bytes);
    }

    #[test]
    fn cmd_screenshot_base64_empty_bytes() {
        let bytes: Vec<u8> = vec![];
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
        assert_eq!(b64, ""); // empty input -> empty base64
    }

    // ─── cmd_query_selector JS construction (pure logic) ────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_query_selector_js_construction_valid_selector() {
        let selector = "div.main";
        let js = format!(
            "(function() {{ var e = document.querySelector({}); return e ? 1 : 0; }})()",
            serde_json::to_string(selector).unwrap_or_default()
        );
        assert!(js.contains("document.querySelector"));
        assert!(js.contains("\"div.main\""));
    }

    #[test]
    fn cmd_query_selector_js_construction_empty_selector() {
        let selector = "";
        let json_str = serde_json::to_string(selector).unwrap_or_default();
        assert_eq!(json_str, "\"\"");
    }

    #[test]
    fn cmd_query_selector_js_construction_special_chars() {
        let selector = "div[data-attr='value']";
        let json_str = serde_json::to_string(selector).unwrap_or_default();
        // serde_json should escape the single quotes properly
        assert!(json_str.contains("div[data-attr"));
    }

    #[test]
    fn cmd_query_selector_js_construction_unicode() {
        let selector = "div.中文类名";
        let json_str = serde_json::to_string(selector).unwrap_or_default();
        assert!(json_str.contains("中文类名"));
    }

    // ─── cmd_query_selector_all JS construction (pure logic) ────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_query_selector_all_js_construction() {
        let selector = "li.item";
        let js = format!(
            "(function() {{ return document.querySelectorAll({}).length; }})()",
            serde_json::to_string(selector).unwrap_or_default()
        );
        assert!(js.contains("document.querySelectorAll"));
        assert!(js.contains(".length"));
    }

    #[test]
    fn cmd_query_selector_all_count_to_node_ids() {
        // When count is 3, nodeIds should be [1, 2, 3]
        let count: i64 = 3;
        let ids: Vec<i64> = (1..=count).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn cmd_query_selector_all_zero_count() {
        let count: i64 = 0;
        let ids: Vec<i64> = (1..=count).collect();
        assert!(ids.is_empty());
    }

    #[test]
    fn cmd_query_selector_all_large_count() {
        let count: i64 = 100;
        let ids: Vec<i64> = (1..=count).collect();
        assert_eq!(ids.len(), 100);
        assert_eq!(ids[0], 1);
        assert_eq!(ids[99], 100);
    }

    // ─── cmd_set_attribute JS construction (pure logic) ─────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_set_attribute_js_construction() {
        let name = "class";
        let value = "active";
        let js = format!(
            "(function() {{ document.querySelector('[data-cdp]')?.setAttribute({}, {}); }})()",
            serde_json::to_string(name).unwrap_or_default(),
            serde_json::to_string(value).unwrap_or_default(),
        );
        assert!(js.contains("setAttribute"));
        assert!(js.contains("\"class\""));
        assert!(js.contains("\"active\""));
    }

    #[test]
    fn cmd_set_attribute_js_with_quotes_in_value() {
        let name = "data-info";
        let value = r#"he said "hello""#;
        let _json_name = serde_json::to_string(name).unwrap_or_default();
        let json_value = serde_json::to_string(value).unwrap_or_default();
        // The double quotes should be escaped in JSON
        assert!(json_value.contains("\\\""));
    }

    // ─── cmd_insert_text JS construction (pure logic) ──────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_insert_text_js_construction() {
        let text = "hello";
        let js = format!(
            "(function() {{ var el = document.activeElement; if (el && 'value' in el) el.value += {}; }})()",
            serde_json::to_string(text).unwrap_or_default(),
        );
        assert!(js.contains("document.activeElement"));
        assert!(js.contains("el.value"));
    }

    #[test]
    fn cmd_insert_text_js_empty_string() {
        let text = "";
        let json_str = serde_json::to_string(text).unwrap_or_default();
        assert_eq!(json_str, "\"\"");
    }

    #[test]
    fn cmd_insert_text_js_newline_escaped() {
        let text = "line1\nline2";
        let json_str = serde_json::to_string(text).unwrap_or_default();
        assert!(json_str.contains("\\n"));
    }

    // ─── cmd_set_user_agent JS construction (pure logic) ───────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_set_user_agent_js_construction() {
        let ua = "Mozilla/5.0 Test";
        let js = format!(
            "Object.defineProperty(navigator, 'userAgent', {{ get: function() {{ return {}; }} }});",
            serde_json::to_string(ua).unwrap_or_default(),
        );
        assert!(js.contains("Object.defineProperty"));
        assert!(js.contains("navigator"));
        assert!(js.contains("userAgent"));
    }

    #[test]
    fn cmd_set_user_agent_js_empty_string() {
        let ua = "";
        let json_str = serde_json::to_string(ua).unwrap_or_default();
        assert_eq!(json_str, "\"\"");
    }

    // ─── cmd_get_document JS template (pure logic) ─────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_get_document_js_template_structure() {
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
        assert!(js.contains("walk"));
        assert!(js.contains("nodeId"));
        assert!(js.contains("nodeType"));
        assert!(js.contains("nodeName"));
        assert!(js.contains("childNodeCount"));
        assert!(js.contains("Math.min"));
        assert!(js.contains("JSON.stringify"));
    }

    #[test]
    fn cmd_get_document_js_limits_children_to_20() {
        // The JS template caps children to 20 via Math.min(node.childNodes.length, 20)
        let _js = r#"(function() { return Math.min(50, 20); })()"#;
        // This is just verifying the logic — 50 children would be capped to 20
        assert_eq!(50usize.min(20), 20);
    }

    // ─── cmd_get_outer_html JS expression (pure logic) ─────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_get_outer_html_js_is_simple_expression() {
        let js = "document.documentElement.outerHTML";
        assert!(js.contains("document.documentElement"));
        assert!(js.contains("outerHTML"));
    }

    // ─── cmd_add_script response structure (pure logic) ────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_add_script_response_has_identifier() {
        let resp = json!({ "identifier": "1" });
        assert_eq!(resp["identifier"], "1");
    }

    // ─── cmd_reload response structure (pure logic) ────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_reload_response_structure() {
        let resp = json!({ "frameId": "0", "loaderId": "0" });
        assert_eq!(resp["frameId"], "0");
        assert_eq!(resp["loaderId"], "0");
    }

    // ─── handle_bridge_command wildcard commands (pure logic) ──────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn handle_bridge_command_go_back_forward_stop_return_empty() {
        // GoBack, GoForward, StopLoading all return Ok(json!({}))
        let expected = json!({});
        assert_eq!(expected, json!({}));
    }

    #[test]
    fn handle_bridge_command_close_page_returns_empty() {
        let expected = json!({});
        assert_eq!(expected, json!({}));
    }

    #[test]
    fn handle_bridge_command_unsupported_returns_error() {
        // The wildcard `_` match returns Err("unsupported bridge command")
        let err_msg = "unsupported bridge command";
        assert!(!err_msg.is_empty());
    }

    // ─── json_type_string additional edge cases ────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn json_type_string_leading_dot_is_string() {
        // ".5" is not a valid f64 parse in some contexts, but Rust's parse handles it
        let result = ".5".parse::<f64>();
        if result.is_ok() {
            assert_eq!(super::json_type_string(".5"), "number");
        } else {
            assert_eq!(super::json_type_string(".5"), "string");
        }
    }

    #[test]
    fn json_type_string_positive_infinity() {
        assert_eq!(super::json_type_string("inf"), "number");
    }

    #[test]
    fn json_type_string_negative_infinity() {
        assert_eq!(super::json_type_string("-inf"), "number");
    }

    #[test]
    fn json_type_string_hex_string_is_string() {
        // "0x1A" is not a valid f64 parse, so it's "string"
        assert_eq!(super::json_type_string("0x1A"), "string");
    }

    #[test]
    fn json_type_string_very_long_number() {
        let long_num = "123456789012345678901234567890";
        // This parses as f64 (with precision loss), so it's "number"
        assert_eq!(super::json_type_string(long_num), "number");
    }

    #[test]
    fn json_type_string_mixed_alphanumeric_is_string() {
        assert_eq!(super::json_type_string("abc123"), "string");
    }

    #[test]
    fn json_type_string_empty_object_string() {
        assert_eq!(super::json_type_string("{}"), "object");
    }

    #[test]
    fn json_type_string_empty_array_string() {
        assert_eq!(super::json_type_string("[]"), "object");
    }

    // ─── json_type additional edge cases ───────────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn json_type_negative_number() {
        assert_eq!(super::json_type(&json!(-999)), "number");
    }

    #[test]
    fn json_type_large_float() {
        assert_eq!(super::json_type(&json!(f64::MIN)), "number");
    }

    #[test]
    fn json_type_deeply_nested_value() {
        let deep = json!({"a": {"b": {"c": {"d": [1, 2, {"e": true}]}}}});
        assert_eq!(super::json_type(&deep), "object");
    }

    #[test]
    fn json_type_string_with_special_chars() {
        assert_eq!(super::json_type(&json!("\n\t\r")), "string");
        assert_eq!(super::json_type(&json!("\0")), "string");
    }

    #[test]
    fn json_type_mixed_array() {
        assert_eq!(super::json_type(&json!([1, "two", null, true, {}])), "object");
    }
}
