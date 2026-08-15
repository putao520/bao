// @trace TEST-CDP-011 [req:REQ-CDP-001] [level:unit]
// @trace TEST-CDP-012 [req:REQ-CDP-002] [level:unit]
// @trace TEST-CDP-013 [req:REQ-CDP-004] [level:unit]
// Protocol message layer deep tests: parse_message, handle_command (all 11 domains
// without bridge), serialize_response, serialize_event, CdpMessage/CdpResponse/
// CdpError/CdpEvent serialization edge cases, roundtrip consistency.

use bao_cdp::{CdpError, CdpEvent, CdpMessage, CdpResponse};

const TID: &str = "test-target";
use bao_cdp::{handle_command, parse_message, serialize_event, serialize_response};

use serde_json::{json, Value};

// ---- parse_message: valid inputs ----

#[test]
fn test_parse_valid_minimal() {
    let raw = r#"{"id":1,"method":"Page.enable"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.enable");
    assert!(msg.params.is_none());
    assert!(msg.session_id.is_none());
}

#[test]
fn test_parse_full_message() {
    let raw = r#"{"id":42,"method":"Runtime.evaluate","params":{"expression":"1+1"},"session_id":"sess-abc"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(42));
    assert_eq!(msg.method, "Runtime.evaluate");
    assert_eq!(msg.params.as_ref().unwrap()["expression"], "1+1");
    assert_eq!(msg.session_id.as_ref().unwrap(), "sess-abc");
}

#[test]
fn test_parse_with_null_params() {
    let raw = r#"{"id":2,"method":"Page.enable","params":null}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(2));
    assert!(msg.params.is_none());
}

#[test]
fn test_parse_with_empty_params() {
    let raw = r#"{"id":3,"method":"Log.enable","params":{}}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.params.is_some());
    assert!(msg.params.unwrap().is_object());
}

#[test]
fn test_parse_with_array_params() {
    let raw = r#"{"id":4,"method":"Fetch.enable","params":{"patterns":[{},{}]}}"#;
    let msg = parse_message(raw).unwrap();
    let p = msg.params.unwrap();
    let patterns = p["patterns"].as_array().unwrap();
    assert_eq!(patterns.len(), 2);
}

#[test]
fn test_parse_negative_id() {
    let raw = r#"{"id":-1,"method":"Test.cmd"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(-1));
}

#[test]
fn test_parse_large_id() {
    let raw = r#"{"id":99999999999,"method":"Test.cmd"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(99999999999i64));
}

#[test]
fn test_parse_string_id_fails() {
    let raw = r#"{"id":"abc","method":"Test.cmd"}"#;
    assert!(parse_message(raw).is_none());
}

#[test]
fn test_parse_float_id_fails() {
    let raw = r#"{"id":1.5,"method":"Test.cmd"}"#;
    assert!(parse_message(raw).is_none());
}

#[test]
fn test_parse_extra_fields_ignored() {
    let raw = r#"{"id":1,"method":"Test.cmd","extra":"data","another":123}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Test.cmd");
}

// ---- parse_message: invalid inputs ----

#[test]
fn test_parse_empty_string() {
    assert!(parse_message("").is_none());
}

#[test]
fn test_parse_plain_text() {
    assert!(parse_message("hello world").is_none());
}

#[test]
fn test_parse_html() {
    assert!(parse_message("<html><body>test</body></html>").is_none());
}

#[test]
fn test_parse_invalid_json() {
    assert!(parse_message("{invalid}").is_none());
    assert!(parse_message("{").is_none());
    assert!(parse_message("}").is_none());
    assert!(parse_message(r#"{"id":1"#).is_none());
}

#[test]
fn test_parse_array_instead_of_object() {
    assert!(parse_message("[1,2,3]").is_none());
}

#[test]
fn test_parse_number() {
    assert!(parse_message("42").is_none());
}

#[test]
fn test_parse_null() {
    assert!(parse_message("null").is_none());
}

#[test]
fn test_parse_missing_id() {
    // JSON-RPC 2.0 permits notifications (no id). With CdpMessage.id as
    // Option<i64> (TASK-4-CDP), a missing id deserializes to None and the
    // message parses successfully (it is a valid notification).
    let raw = r#"{"method":"Page.enable"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, None);
    assert_eq!(msg.method, "Page.enable");
}

#[test]
fn test_parse_missing_method() {
    let raw = r#"{"id":1}"#;
    assert!(parse_message(raw).is_none());
}

#[test]
fn test_parse_unicode_method() {
    let raw = r#"{"id":1,"method":"页面.启用"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.method, "页面.启用");
}

// ---- handle_command helpers ----

fn dispatch(method: &str, params: Option<Value>) -> CdpResponse {
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: params.clone(),
        session_id: None,
    };
    handle_command(msg, "target-001", &params, None)
}

fn ok_result(method: &str, params: Option<Value>) -> Value {
    dispatch(method, params).result.unwrap()
}

fn err_result(method: &str, params: Option<Value>) -> CdpError {
    dispatch(method, params).error.unwrap()
}

// ---- handle_command: Target domain (no bridge) ----

#[test]
fn test_target_get_targets_no_bridge() {
    let r = ok_result("Target.getTargets", None);
    let infos = r["targetInfos"].as_array().unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0]["targetId"], "target-001");
    assert_eq!(infos[0]["type"], "page");
    assert_eq!(infos[0]["title"], "Bao");
    assert_eq!(infos[0]["url"], "about:blank");
}

#[test]
fn test_target_get_target_targets() {
    let r = ok_result("Target.getTargetTargets", None);
    assert!(r["targetInfos"].is_array());
}

#[test]
fn test_target_create_target() {
    let r = ok_result("Target.createTarget", None);
    assert_eq!(r["targetId"], "target-001");
}

#[test]
fn test_target_close_target() {
    let r = ok_result("Target.closeTarget", None);
    assert_eq!(r["success"], true);
}

#[test]
fn test_target_set_auto_attach() {
    assert_eq!(ok_result("Target.setAutoAttach", None), json!({}));
}

#[test]
fn test_target_set_discover_targets() {
    assert_eq!(ok_result("Target.setDiscoverTargets", None), json!({}));
}

#[test]
fn test_target_get_target_info() {
    let r = ok_result("Target.getTargetInfo", None);
    let info = r["targetInfo"].as_object().unwrap();
    assert_eq!(info["targetId"], "target-001");
    assert_eq!(info["attached"], true);
}

#[test]
fn test_target_attach_to_target() {
    let r = ok_result("Target.attachToTarget", None);
    let sid = r["sessionId"].as_str().unwrap();
    assert!(!sid.is_empty());
    assert!(sid.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_target_detach_from_target() {
    assert_eq!(ok_result("Target.detachFromTarget", None), json!({}));
}

#[test]
fn test_target_send_message_to_target() {
    assert_eq!(ok_result("Target.sendMessageToTarget", None), json!({}));
}

#[test]
fn test_target_unknown_command() {
    let err = err_result("Target.nonexistent", None);
    assert_eq!(err.code, -32601);
}

// ---- handle_command: Page domain (no bridge) ----

#[test]
fn test_page_enable() {
    assert_eq!(ok_result("Page.enable", None), json!({}));
}

#[test]
fn test_page_disable() {
    assert_eq!(ok_result("Page.disable", None), json!({}));
}

#[test]
fn test_page_navigate_no_bridge() {
    // New contract (6983871b): the bridge response carries the real
    // frameId/loaderId — without a bridge navigate is an explicit -32603.
    let e = err_result("Page.navigate", Some(json!({"url": "https://example.com"})));
    assert_eq!(e.code, -32603);
    assert!(e.message.contains("no servo bridge"));
}

#[test]
fn test_page_navigate_default_url() {
    let e = err_result("Page.navigate", Some(json!({})));
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_reload_no_bridge() {
    // New contract (6983871b): reload goes through WebView::reload via the
    // bridge — explicit -32603, never fabricated frameId/loaderId "0".
    let e = err_result("Page.reload", Some(json!({"ignoreCache": true})));
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_get_frame_tree_no_bridge() {
    // New contract (6983871b): frame data is read from the live document via
    // the bridge — explicit -32603, never a fabricated about:blank tree.
    let e = err_result("Page.getFrameTree", None);
    assert_eq!(e.code, -32603);
    assert!(e.message.contains("no servo bridge"));
}

#[test]
fn test_page_get_navigation_history_no_bridge() {
    // New contract (6983871b): servo exposes no session-history enumeration —
    // explicit -32000, never a fabricated single-entry history.
    let e = err_result("Page.getNavigationHistory", None);
    assert_eq!(e.code, -32000);
    assert!(e.message.contains("not supported"));
}

#[test]
fn test_page_capture_screenshot_no_bridge() {
    // New contract (6983871b): no renderer without the bridge — explicit
    // -32603, never the canned {"data":""} success.
    let e = err_result("Page.captureScreenshot", Some(json!({"format": "png"})));
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_capture_screenshot_jpeg_no_bridge() {
    let e = err_result(
        "Page.captureScreenshot",
        Some(json!({"format": "jpeg", "quality": 80})),
    );
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_set_content() {
    // New contract (6983871b): setContent validates its html param —
    // missing html is -32602, never a silent ok.
    let e = err_result("Page.setContent", None);
    assert_eq!(e.code, -32602);
    assert!(e.message.contains("html"));
}

#[test]
fn test_page_close() {
    // New contract (6983871b): real close via PagePool — requires the bridge.
    let e = err_result("Page.close", None);
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_bring_to_front() {
    // New contract (6983871b): real focus path — requires the bridge.
    let e = err_result("Page.bringToFront", None);
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_get_layout_metrics() {
    // New contract (6983871b): metrics are computed live from the document —
    // explicit -32603, the hardcoded 1920x1080 is eradicated.
    let e = err_result("Page.getLayoutMetrics", None);
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_add_script_to_evaluate_no_bridge() {
    // New contract (6983871b): identifier generation lives behind the
    // bridge — no bridge is an explicit -32603, no hardcoded "1".
    let e = err_result(
        "Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source": "console.log(1)"})),
    );
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_add_script_empty_source_no_bridge() {
    // New contract (6983871b): empty source is rejected with -32602 before
    // any bridge dispatch.
    let e = err_result(
        "Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source": ""})),
    );
    assert_eq!(e.code, -32602);
    assert!(e.message.contains("source"));
}

#[test]
fn test_page_remove_script() {
    // New contract (6983871b): a missing identifier param is -32602.
    let e = err_result("Page.removeScriptToEvaluateOnNewDocument", None);
    assert_eq!(e.code, -32602);
    assert!(e.message.contains("identifier"));
}

#[test]
fn test_page_unknown_command() {
    assert_eq!(err_result("Page.nonexistent", None).code, -32601);
}

// ---- handle_command: Runtime domain (no bridge) ----

#[test]
fn test_runtime_enable() {
    // New contract (6983871b): Chrome semantics — Runtime.enable returns {}
    // (no fabricated executionContextId).
    assert_eq!(ok_result("Runtime.enable", None), json!({}));
}

#[test]
fn test_runtime_disable() {
    assert_eq!(ok_result("Runtime.disable", None), json!({}));
}

#[test]
fn test_runtime_evaluate_no_bridge_no_expression() {
    let r = ok_result("Runtime.evaluate", Some(json!({})));
    assert_eq!(r["result"]["type"], "undefined");
    assert!(r["exceptionDetails"].is_null());
}

#[test]
fn test_runtime_evaluate_no_bridge_empty_expression() {
    let r = ok_result("Runtime.evaluate", Some(json!({"expression": ""})));
    assert_eq!(r["result"]["type"], "undefined");
}

#[test]
fn test_runtime_call_function_on() {
    let r = ok_result("Runtime.callFunctionOn", None);
    assert_eq!(r["result"]["type"], "undefined");
}

#[test]
fn test_runtime_get_properties() {
    let r = ok_result("Runtime.getProperties", None);
    assert!(r["result"].is_array());
    assert_eq!(r["result"].as_array().unwrap().len(), 0);
}

#[test]
fn test_runtime_evaluate_async() {
    let r = ok_result("Runtime.evaluateAsync", None);
    assert_eq!(r["result"]["type"], "undefined");
}

#[test]
fn test_runtime_run_script() {
    let r = ok_result("Runtime.runScript", None);
    assert_eq!(r["result"]["type"], "undefined");
}

#[test]
fn test_runtime_release_object() {
    assert_eq!(ok_result("Runtime.releaseObject", None), json!({}));
}

#[test]
fn test_runtime_release_object_group() {
    assert_eq!(ok_result("Runtime.releaseObjectGroup", None), json!({}));
}

#[test]
fn test_runtime_compile_script() {
    assert_eq!(ok_result("Runtime.compileScript", None), json!({}));
}

#[test]
fn test_runtime_call_argument() {
    assert_eq!(ok_result("Runtime.callArgument", None), json!({}));
}

#[test]
fn test_runtime_unknown_command() {
    assert_eq!(err_result("Runtime.nonexistent", None).code, -32601);
}

// ---- handle_command: DOM domain (no bridge) ----

#[test]
fn test_dom_enable() {
    assert_eq!(ok_result("DOM.enable", None), json!({}));
}

#[test]
fn test_dom_disable() {
    assert_eq!(ok_result("DOM.disable", None), json!({}));
}

#[test]
fn test_dom_get_document_no_bridge() {
    let r = ok_result("DOM.getDocument", None);
    let root = r["root"].as_object().unwrap();
    assert_eq!(root["nodeId"], 1);
    assert_eq!(root["nodeType"], 9);
    assert_eq!(root["nodeName"], "#document");
    let children = root["children"].as_array().unwrap();
    assert_eq!(children[0]["nodeName"], "HTML");
}

#[test]
fn test_dom_describe_node() {
    let r = ok_result("DOM.describeNode", None);
    let node = r["node"].as_object().unwrap();
    assert_eq!(node["nodeId"], 1);
    assert_eq!(node["nodeName"], "HTML");
}

#[test]
fn test_dom_query_selector_no_bridge() {
    let r = ok_result("DOM.querySelector", Some(json!({"selector": "div"})));
    assert_eq!(r["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_no_selector() {
    let r = ok_result("DOM.querySelector", Some(json!({})));
    assert_eq!(r["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_no_bridge() {
    let r = ok_result("DOM.querySelectorAll", Some(json!({"selector": "div"})));
    assert_eq!(r["nodeIds"].as_array().unwrap().len(), 0);
}

#[test]
fn test_dom_get_box_model() {
    let r = ok_result("DOM.getBoxModel", None);
    let model = r["model"].as_object().unwrap();
    assert_eq!(model["width"], 1920);
    assert_eq!(model["height"], 1080);
}

#[test]
fn test_dom_set_attribute_value_no_bridge() {
    assert_eq!(
        ok_result(
            "DOM.setAttributeValue",
            Some(json!({"nodeId": 5, "name": "class", "value": "test"}))
        ),
        json!({})
    );
}

#[test]
fn test_dom_remove_attribute() {
    assert_eq!(ok_result("DOM.removeAttribute", None), json!({}));
}

#[test]
fn test_dom_set_outer_html() {
    assert_eq!(ok_result("DOM.setOuterHTML", None), json!({}));
}

#[test]
fn test_dom_insert_before() {
    assert_eq!(ok_result("DOM.insertBefore", None), json!({}));
}

#[test]
fn test_dom_remove_node() {
    assert_eq!(ok_result("DOM.removeNode", None), json!({}));
}

#[test]
fn test_dom_get_outer_html_no_bridge() {
    // New contract (6983871b): outerHTML is read from the live document —
    // explicit -32603, never canned html.
    let e = err_result("DOM.getOuterHTML", Some(json!({"nodeId": 1})));
    assert_eq!(e.code, -32603);
}

#[test]
fn test_dom_resolve_node() {
    let r = ok_result("DOM.resolveNode", None);
    assert_eq!(r["object"]["type"], "node");
}

#[test]
fn test_dom_push_nodes_by_backend_ids() {
    let r = ok_result("DOM.pushNodesByBackendIdsToFrontend", None);
    assert_eq!(r["nodeIds"].as_array().unwrap().len(), 0);
}

#[test]
fn test_dom_unknown_command() {
    assert_eq!(err_result("DOM.nonexistent", None).code, -32601);
}

// ---- handle_command: Network domain ----

#[test]
fn test_network_enable() {
    assert_eq!(ok_result("Network.enable", None), json!({}));
}

#[test]
fn test_network_disable() {
    assert_eq!(ok_result("Network.disable", None), json!({}));
}

#[test]
fn test_network_get_response_body() {
    // New contract (6983871b): servo exposes no response-body store —
    // explicit -32603, never an empty-body fake success.
    let e = err_result("Network.getResponseBody", None);
    assert_eq!(e.code, -32603);
}

#[test]
fn test_network_set_cache_disabled() {
    assert_eq!(ok_result("Network.setCacheDisabled", None), json!({}));
}

#[test]
fn test_network_set_extra_http_headers() {
    // New contract (6983871b): headers are never silently dropped —
    // explicit -32603 without a bridge.
    let e = err_result("Network.setExtraHTTPHeaders", None);
    assert_eq!(e.code, -32603);
}

#[test]
fn test_network_emulate_network_conditions() {
    assert_eq!(
        ok_result("Network.emulateNetworkConditions", None),
        json!({})
    );
}

#[test]
fn test_network_set_request_interception() {
    assert_eq!(ok_result("Network.setRequestInterception", None), json!({}));
}

#[test]
fn test_network_continue_intercepted_request() {
    assert_eq!(
        ok_result("Network.continueInterceptedRequest", None),
        json!({})
    );
}

#[test]
fn test_network_get_cookies() {
    let r = ok_result("Network.getCookies", None);
    assert_eq!(r["cookies"].as_array().unwrap().len(), 0);
}

#[test]
fn test_network_get_all_cookies() {
    let r = ok_result("Network.getAllCookies", None);
    assert_eq!(r["cookies"].as_array().unwrap().len(), 0);
}

#[test]
fn test_network_delete_cookies() {
    assert_eq!(ok_result("Network.deleteCookies", None), json!({}));
}

#[test]
fn test_network_set_cookie() {
    assert_eq!(ok_result("Network.setCookie", None), json!({}));
}

#[test]
fn test_network_unknown_command() {
    assert_eq!(err_result("Network.nonexistent", None).code, -32601);
}

// ---- handle_command: CSS domain ----

#[test]
fn test_css_enable() {
    assert_eq!(ok_result("CSS.enable", None), json!({}));
}

#[test]
fn test_css_disable() {
    assert_eq!(ok_result("CSS.disable", None), json!({}));
}

#[test]
fn test_css_get_computed_style() {
    let r = ok_result("CSS.getComputedStyleForNode", None);
    assert_eq!(r["computedStyle"].as_array().unwrap().len(), 0);
}

#[test]
fn test_css_get_matched_styles() {
    let r = ok_result("CSS.getMatchedStylesForNode", None);
    assert!(r["matchedCSSRules"].is_array());
    assert!(r["inlineStyle"].is_null());
    assert!(r["attributesStyle"].is_null());
}

#[test]
fn test_css_get_inline_styles() {
    let r = ok_result("CSS.getInlineStylesForNode", None);
    assert!(r["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    let r = ok_result("CSS.setStyleTexts", None);
    assert_eq!(r["styles"].as_array().unwrap().len(), 0);
}

#[test]
fn test_css_unknown_command() {
    assert_eq!(err_result("CSS.nonexistent", None).code, -32601);
}

// ---- handle_command: Emulation domain (no bridge) ----

#[test]
fn test_emulation_set_device_metrics_no_bridge() {
    assert_eq!(
        ok_result(
            "Emulation.setDeviceMetricsOverride",
            Some(json!({"width": 800, "height": 600, "deviceScaleFactor": 1.5}))
        ),
        json!({})
    );
}

#[test]
fn test_emulation_set_device_metrics_defaults() {
    assert_eq!(
        ok_result("Emulation.setDeviceMetricsOverride", Some(json!({}))),
        json!({})
    );
}

#[test]
fn test_emulation_clear_device_metrics() {
    assert_eq!(
        ok_result("Emulation.clearDeviceMetricsOverride", None),
        json!({})
    );
}

#[test]
fn test_emulation_set_user_agent_no_bridge() {
    assert_eq!(
        ok_result(
            "Emulation.setUserAgentOverride",
            Some(json!({"userAgent": ""}))
        ),
        json!({})
    );
}

#[test]
fn test_emulation_set_touch_emulation() {
    assert_eq!(
        ok_result("Emulation.setTouchEmulationEnabled", None),
        json!({})
    );
}

#[test]
fn test_emulation_set_script_execution_disabled() {
    assert_eq!(
        ok_result("Emulation.setScriptExecutionDisabled", None),
        json!({})
    );
}

#[test]
fn test_emulation_set_focus_emulation() {
    assert_eq!(
        ok_result("Emulation.setFocusEmulationEnabled", None),
        json!({})
    );
}

#[test]
fn test_emulation_set_cpu_throttling_rate() {
    assert_eq!(ok_result("Emulation.setCPUThrottlingRate", None), json!({}));
}

#[test]
fn test_emulation_set_default_background_color_override() {
    assert_eq!(
        ok_result("Emulation.setDefaultBackgroundColorOverride", None),
        json!({})
    );
}

#[test]
fn test_emulation_unknown_command() {
    assert_eq!(err_result("Emulation.nonexistent", None).code, -32601);
}

// ---- handle_command: Input domain (no bridge) ----

#[test]
fn test_input_dispatch_mouse_event_no_bridge() {
    assert_eq!(
        ok_result(
            "Input.dispatchMouseEvent",
            Some(json!({"type": "mousePressed", "x": 100.0, "y": 200.0}))
        ),
        json!({})
    );
}

#[test]
fn test_input_dispatch_mouse_event_no_coords() {
    assert_eq!(
        ok_result(
            "Input.dispatchMouseEvent",
            Some(json!({"type": "mouseMoved"}))
        ),
        json!({})
    );
}

#[test]
fn test_input_dispatch_key_event_no_bridge() {
    assert_eq!(
        ok_result(
            "Input.dispatchKeyEvent",
            Some(json!({"type": "keyDown", "key": "a", "code": "KeyA"}))
        ),
        json!({})
    );
}

#[test]
fn test_input_dispatch_key_event_minimal() {
    assert_eq!(
        ok_result("Input.dispatchKeyEvent", Some(json!({}))),
        json!({})
    );
}

#[test]
fn test_input_dispatch_touch_event() {
    assert_eq!(ok_result("Input.dispatchTouchEvent", None), json!({}));
}

#[test]
fn test_input_insert_text_no_bridge() {
    assert_eq!(
        ok_result("Input.insertText", Some(json!({"text": ""}))),
        json!({})
    );
}

#[test]
fn test_input_set_ignore_input_events() {
    assert_eq!(ok_result("Input.setIgnoreInputEvents", None), json!({}));
}

#[test]
fn test_input_set_intercept_drags() {
    assert_eq!(ok_result("Input.setInterceptDrags", None), json!({}));
}

#[test]
fn test_input_unknown_command() {
    assert_eq!(err_result("Input.nonexistent", None).code, -32601);
}

// ---- handle_command: Overlay domain ----

#[test]
fn test_overlay_enable() {
    assert_eq!(ok_result("Overlay.enable", None), json!({}));
}

#[test]
fn test_overlay_disable() {
    assert_eq!(ok_result("Overlay.disable", None), json!({}));
}

#[test]
fn test_overlay_highlight_node() {
    assert_eq!(ok_result("Overlay.highlightNode", None), json!({}));
}

#[test]
fn test_overlay_hide_highlight() {
    assert_eq!(ok_result("Overlay.hideHighlight", None), json!({}));
}

#[test]
fn test_overlay_set_inspect_mode() {
    assert_eq!(ok_result("Overlay.setInspectMode", None), json!({}));
}

#[test]
fn test_overlay_set_paused_in_debugger_message() {
    assert_eq!(
        ok_result("Overlay.setPausedInDebuggerMessage", None),
        json!({})
    );
}

#[test]
fn test_overlay_unknown_command() {
    assert_eq!(err_result("Overlay.nonexistent", None).code, -32601);
}

// ---- handle_command: Debugger domain ----

#[test]
fn test_debugger_enable() {
    assert_eq!(ok_result("Debugger.enable", None), json!({}));
}

#[test]
fn test_debugger_disable() {
    assert_eq!(ok_result("Debugger.disable", None), json!({}));
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let r = ok_result("Debugger.setBreakpointByUrl", None);
    assert_eq!(r["breakpointId"], "1");
    assert!(r["locations"].is_array());
}

#[test]
fn test_debugger_remove_breakpoint() {
    assert_eq!(ok_result("Debugger.removeBreakpoint", None), json!({}));
}

#[test]
fn test_debugger_pause() {
    assert_eq!(ok_result("Debugger.pause", None), json!({}));
}

#[test]
fn test_debugger_resume() {
    assert_eq!(ok_result("Debugger.resume", None), json!({}));
}

#[test]
fn test_debugger_step_over() {
    assert_eq!(ok_result("Debugger.stepOver", None), json!({}));
}

#[test]
fn test_debugger_step_into() {
    assert_eq!(ok_result("Debugger.stepInto", None), json!({}));
}

#[test]
fn test_debugger_step_out() {
    assert_eq!(ok_result("Debugger.stepOut", None), json!({}));
}

#[test]
fn test_debugger_set_skip_all_pauses() {
    assert_eq!(ok_result("Debugger.setSkipAllPauses", None), json!({}));
}

#[test]
fn test_debugger_set_breakpoints_active() {
    assert_eq!(ok_result("Debugger.setBreakpointsActive", None), json!({}));
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let r = ok_result("Debugger.evaluateOnCallFrame", None);
    assert_eq!(r["result"]["type"], "undefined");
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let r = ok_result("Debugger.getPossibleBreakpoints", None);
    assert_eq!(r["locations"].as_array().unwrap().len(), 0);
}

#[test]
fn test_debugger_get_script_source() {
    let r = ok_result("Debugger.getScriptSource", None);
    assert_eq!(r["scriptSource"], "");
}

#[test]
fn test_debugger_set_pause_on_exceptions() {
    assert_eq!(ok_result("Debugger.setPauseOnExceptions", None), json!({}));
}

#[test]
fn test_debugger_unknown_command() {
    assert_eq!(err_result("Debugger.nonexistent", None).code, -32601);
}

// ---- handle_command: Log domain ----

#[test]
fn test_log_enable() {
    assert_eq!(ok_result("Log.enable", None), json!({}));
}

#[test]
fn test_log_disable() {
    assert_eq!(ok_result("Log.disable", None), json!({}));
}

#[test]
fn test_log_clear() {
    assert_eq!(ok_result("Log.clear", None), json!({}));
}

#[test]
fn test_log_start_violations_report() {
    assert_eq!(ok_result("Log.startViolationsReport", None), json!({}));
}

#[test]
fn test_log_stop_violations_report() {
    assert_eq!(ok_result("Log.stopViolationsReport", None), json!({}));
}

#[test]
fn test_log_unknown_command() {
    assert_eq!(err_result("Log.nonexistent", None).code, -32601);
}

// ---- handle_command: Fetch domain ----

#[test]
fn test_fetch_enable_no_patterns() {
    let r = ok_result("Fetch.enable", Some(json!({})));
    assert_eq!(r["enabled"], true);
    assert_eq!(r["patternCount"], 0);
}

#[test]
fn test_fetch_enable_with_patterns() {
    let r = ok_result(
        "Fetch.enable",
        Some(json!({
            "patterns": [{"urlPattern": "*"}, {"urlPattern": "https://*"}]
        })),
    );
    assert_eq!(r["enabled"], true);
    assert_eq!(r["patternCount"], 2);
}

#[test]
fn test_fetch_disable() {
    assert_eq!(ok_result("Fetch.disable", None), json!({}));
}

#[test]
fn test_fetch_continue_request() {
    let r = ok_result("Fetch.continueRequest", Some(json!({"requestId": "req-1"})));
    assert_eq!(r["requestId"], "req-1");
    assert_eq!(r["continued"], true);
}

#[test]
fn test_fetch_continue_with_response() {
    let r = ok_result(
        "Fetch.continueWithResponse",
        Some(json!({"requestId": "req-2"})),
    );
    assert_eq!(r["requestId"], "req-2");
    assert_eq!(r["continued"], true);
}

#[test]
fn test_fetch_fail_request() {
    let r = ok_result(
        "Fetch.failRequest",
        Some(json!({"requestId": "req-3", "reason": "Aborted"})),
    );
    assert_eq!(r["requestId"], "req-3");
    assert_eq!(r["failed"], true);
    assert_eq!(r["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request() {
    let r = ok_result(
        "Fetch.fulfillRequest",
        Some(json!({
            "requestId": "req-4", "responseCode": 404, "body": "not found"
        })),
    );
    assert_eq!(r["requestId"], "req-4");
    assert_eq!(r["fulfilled"], true);
    assert_eq!(r["responseCode"], 404);
    assert_eq!(r["bodyLength"], 9);
}

#[test]
fn test_fetch_fulfill_request_default_code() {
    let r = ok_result(
        "Fetch.fulfillRequest",
        Some(json!({"requestId": "r1", "body": ""})),
    );
    assert_eq!(r["responseCode"], 200);
    assert_eq!(r["bodyLength"], 0);
}

#[test]
fn test_fetch_get_request_post_data() {
    let r = ok_result(
        "Fetch.getRequestPostData",
        Some(json!({"requestId": "req-5"})),
    );
    assert_eq!(r["requestId"], "req-5");
    assert_eq!(r["postData"], "");
}

#[test]
fn test_fetch_continue_with_auth() {
    let r = ok_result(
        "Fetch.continueWithAuth",
        Some(json!({"requestId": "req-6"})),
    );
    assert_eq!(r["requestId"], "req-6");
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let r = ok_result(
        "Fetch.takeResponseBodyAsStream",
        Some(json!({"requestId": "req-7"})),
    );
    assert_eq!(r["stream"], "stream-req-7");
}

#[test]
fn test_fetch_unknown_command() {
    assert_eq!(err_result("Fetch.nonexistent", None).code, -32601);
}

// ---- handle_command: unknown domain ----

#[test]
fn test_unknown_domain() {
    let err = err_result("Unknown.method", None);
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Unknown.method"));
}

#[test]
fn test_empty_method() {
    let err = err_result("", None);
    assert_eq!(err.code, -32601);
}

#[test]
fn test_no_dot_in_method() {
    let err = err_result("NoDotMethod", None);
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("NoDotMethod"));
}

// ---- serialize_response ----

#[test]
fn test_serialize_ok_response() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"value": 42})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["id"], 1);
    assert_eq!(p["result"]["value"], 42);
    assert!(p.get("error").is_none());
}

#[test]
fn test_serialize_error_response() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError {
            code: -32601,
            message: "not found".into(),
        }),
    };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["id"], 2);
    assert!(p.get("result").is_none());
    assert_eq!(p["error"]["code"], -32601);
    assert_eq!(p["error"]["message"], "not found");
}

#[test]
fn test_serialize_empty_result() {
    let resp = CdpResponse {
        id: Some(3),
        result: Some(json!({})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["result"], json!({}));
}

#[test]
fn test_serialize_negative_id() {
    let resp = CdpResponse {
        id: Some(-100),
        result: Some(json!(null)),
        error: None,
    };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["id"], -100);
}

#[test]
fn test_serialize_zero_id() {
    let resp = CdpResponse {
        id: Some(0),
        result: Some(json!({})),
        error: None,
    };
    let raw = serialize_response(&resp);
    assert!(serde_json::from_str::<Value>(&raw).is_ok());
}

// ---- serialize_event ----

#[test]
fn test_serialize_event_with_params() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345})),
    };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["method"], "Page.loadEventFired");
    assert_eq!(p["params"]["timestamp"], 12345);
}

#[test]
fn test_serialize_event_without_params() {
    let ev = CdpEvent {
        method: "DOM.documentUpdated".into(),
        params: None,
    };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["method"], "DOM.documentUpdated");
    assert!(p.get("params").is_none());
}

#[test]
fn test_serialize_event_empty_params() {
    let ev = CdpEvent {
        method: "Log.entryAdded".into(),
        params: Some(json!({})),
    };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["params"], json!({}));
}

#[test]
fn test_serialize_event_complex_params() {
    let ev = CdpEvent {
        method: "Runtime.consoleAPICalled".into(),
        params: Some(
            json!({"type": "log", "timestamp": 999, "args": [{"type": "string", "value": "hello"}]}),
        ),
    };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["params"]["args"][0]["value"], "hello");
}

// ---- Roundtrip: parse → handle → serialize ----

#[test]
fn test_roundtrip_page_enable() {
    let raw = r#"{"id":10,"method":"Page.enable"}"#;
    let msg = parse_message(raw).unwrap();
    let resp = handle_command(msg, "t-1", &None, None);
    let resp_json = serialize_response(&resp);
    let p: Value = serde_json::from_str(&resp_json).unwrap();
    assert_eq!(p["id"], 10);
    assert_eq!(p["result"], json!({}));
}

#[test]
fn test_roundtrip_runtime_evaluate() {
    let raw = r#"{"id":20,"method":"Runtime.evaluate","params":{"expression":"1+1"}}"#;
    let msg = parse_message(raw).unwrap();
    let params = msg.params.clone();
    let resp = handle_command(msg, "t-1", &params, None);
    let resp_json = serialize_response(&resp);
    let p: Value = serde_json::from_str(&resp_json).unwrap();
    assert_eq!(p["id"], 20);
    assert_eq!(p["result"]["result"]["type"], "undefined");
}

#[test]
fn test_roundtrip_unknown_domain() {
    let raw = r#"{"id":30,"method":"Foo.bar"}"#;
    let msg = parse_message(raw).unwrap();
    let resp = handle_command(msg, "t-1", &None, None);
    assert!(resp.error.is_some());
    let resp_json = serialize_response(&resp);
    let p: Value = serde_json::from_str(&resp_json).unwrap();
    assert_eq!(p["error"]["code"], -32601);
}

#[test]
fn test_roundtrip_fetch_enable_with_patterns() {
    let raw = r#"{"id":40,"method":"Fetch.enable","params":{"patterns":[{"urlPattern":"*"}]}}"#;
    let msg = parse_message(raw).unwrap();
    let params = msg.params.clone();
    let resp = handle_command(msg, "t-1", &params, None);
    let r = resp.result.unwrap();
    assert_eq!(r["enabled"], true);
    assert_eq!(r["patternCount"], 1);
}

#[test]
fn test_roundtrip_dom_get_document() {
    let raw = r#"{"id":50,"method":"DOM.getDocument"}"#;
    let msg = parse_message(raw).unwrap();
    let resp = handle_command(msg, "t-1", &None, None);
    let r = resp.result.unwrap();
    let root = r["root"].as_object().unwrap();
    assert_eq!(root["nodeName"], "#document");
    assert_eq!(root["children"][0]["nodeName"], "HTML");
}

// ---- CdpError Debug/Serialize ----

#[test]
fn test_cdp_error_debug() {
    let err = CdpError {
        code: -32601,
        message: "test error".into(),
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32601"));
    assert!(debug.contains("test error"));
}

#[test]
fn test_cdp_error_serialization() {
    let err = CdpError {
        code: -32000,
        message: "internal".into(),
    };
    let json_str = serde_json::to_string(&err).unwrap();
    let p: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(p["code"], -32000);
    assert_eq!(p["message"], "internal");
}

// ---- CdpMessage Clone/Debug ----

#[test]
fn test_cdp_message_clone() {
    let msg = CdpMessage {
        id: Some(1),
        method: "Page.enable".into(),
        params: Some(json!({"key": "val"})),
        session_id: Some("sess-1".into()),
    };
    let cloned = msg.clone();
    assert_eq!(cloned.id, Some(1));
    assert_eq!(cloned.method, "Page.enable");
    assert_eq!(cloned.params.unwrap()["key"], "val");
    assert_eq!(cloned.session_id.unwrap(), "sess-1");
}

#[test]
fn test_cdp_message_debug() {
    let msg = CdpMessage {
        id: Some(1),
        method: "Test.cmd".into(),
        params: None,
        session_id: None,
    };
    let debug = format!("{:?}", msg);
    assert!(debug.contains("Test.cmd"));
}

// ---- CdpEvent Clone/Debug ----

#[test]
fn test_cdp_event_clone() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"ts": 1})),
    };
    let cloned = ev.clone();
    assert_eq!(cloned.method, "Page.loadEventFired");
    assert_eq!(cloned.params.unwrap()["ts"], 1);
}

#[test]
fn test_cdp_event_debug() {
    let ev = CdpEvent {
        method: "Test.evt".into(),
        params: None,
    };
    let debug = format!("{:?}", ev);
    assert!(debug.contains("Test.evt"));
}

// ---- ID preservation through handle_command ----

#[test]
fn test_response_preserves_request_id() {
    for id in [0i64, 1, -1, 999, i64::MAX, i64::MIN] {
        let msg = CdpMessage {
            id: Some(id),
            method: "Page.enable".into(),
            params: None,
            session_id: None,
        };
        let resp = handle_command(msg, "t-1", &None, None);
        assert_eq!(
            resp.id,
            Some(id),
            "Response ID should match request ID {}",
            id
        );
    }
}

// ---- Target sessionId deterministic ----

#[test]
fn test_attach_to_target_session_id_deterministic() {
    let msg1 = CdpMessage {
        id: Some(1),
        method: "Target.attachToTarget".into(),
        params: None,
        session_id: None,
    };
    let r1 = handle_command(msg1, "target-abc", &None, None)
        .result
        .unwrap();
    let sid1 = r1["sessionId"].as_str().unwrap().to_string();

    let msg2 = CdpMessage {
        id: Some(2),
        method: "Target.attachToTarget".into(),
        params: None,
        session_id: None,
    };
    let r2 = handle_command(msg2, "target-abc", &None, None)
        .result
        .unwrap();
    let sid2 = r2["sessionId"].as_str().unwrap().to_string();

    assert_eq!(sid1, sid2);
}

#[test]
fn test_different_targets_different_session_ids() {
    let msg1 = CdpMessage {
        id: Some(1),
        method: "Target.attachToTarget".into(),
        params: None,
        session_id: None,
    };
    let r1 = handle_command(msg1, "target-A", &None, None)
        .result
        .unwrap();
    let sid1 = r1["sessionId"].as_str().unwrap().to_string();

    let msg2 = CdpMessage {
        id: Some(2),
        method: "Target.attachToTarget".into(),
        params: None,
        session_id: None,
    };
    let r2 = handle_command(msg2, "target-B", &None, None)
        .result
        .unwrap();
    let sid2 = r2["sessionId"].as_str().unwrap().to_string();

    assert_ne!(sid1, sid2);
}

// ---- Page navigate loaderId source (6983871b: bridge truth) ----

#[test]
fn test_navigate_loader_id_from_url_length() {
    // New contract (6983871b): the fabricated rule
    // `loaderId = format!("{:016x}", url.len())` is eradicated — loaderIds
    // are per-load values from the bridge. Without a bridge, navigate is an
    // explicit -32603 and no loaderId is ever derived from the url.
    let url = "https://example.com/page";
    let msg = CdpMessage {
        id: Some(1),
        method: "Page.navigate".into(),
        params: Some(json!({"url": url})),
        session_id: None,
    };
    let resp = handle_command(msg, "t-1", &Some(json!({"url": url})), None);
    let e = resp.error.expect("no bridge must yield an error");
    assert_eq!(e.code, -32603);
    assert!(resp.result.is_none(), "no url-length-derived loaderId");
}

// ---- All 12 domain error paths ----

#[test]
fn test_all_domains_unknown_command() {
    let domains = [
        "Target",
        "Page",
        "Runtime",
        "DOM",
        "Network",
        "CSS",
        "Emulation",
        "Input",
        "Overlay",
        "Debugger",
        "Log",
        "Fetch",
    ];
    for domain in &domains {
        let method = format!("{}.completelyUnknownCommand12345", domain);
        let msg = CdpMessage {
            id: Some(1),
            method: method.clone(),
            params: None,
            session_id: None,
        };
        let resp = handle_command(msg, "t-1", &None, None);
        let err = resp
            .error
            .expect(&format!("{} unknown should error", domain));
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("completelyUnknownCommand12345"));
        // JSON-RPC 2.0 §5.1: error responses MUST NOT carry a `result` member
        assert!(
            resp.result.is_none(),
            "{} error response must not carry result",
            domain
        );
        // Error message format: known-domain unknown-command embeds the domain
        assert!(
            err.message.contains(domain),
            "{} unknown-command message should name the domain",
            domain
        );
    }
}

// ===========================================================================
// Adversarial verification gaps — missing assertions · boundary · SPEC alignment
//
// Coverage additions:
//   A. parse_message: params non-object scalars (string/number/bool/null),
//      session_id empty string, method whitespace, extreme id ranges.
//   B. serialize_response: id:None (JSON-RPC notification echo), mutual
//      exclusivity of result/error per JSON-RPC 2.0 §5.1, deterministic
//      well-formed JSON output, serialize fallback path (parse-error envelope).
//   C. serialize_event: empty method, null params, array params, compact form.
//   D. handle_command: multiple-dot method splitn(2,'.') semantics, empty
//      target_id, Target.attachToTarget sessionId formula self-consistency,
//      Page.navigate empty-url boundary, Fetch non-array patterns guard,
//      Fetch.fulfillRequest bodyLength byte-semantics, Emulation UA empty,
//      Input negative coords, Debugger empty breakpointId type.
//   E. Roundtrip JSON-RPC 2.0 wire-shape invariants: success has no `error`
//      key, error has no `result` key, id is preserved, method-less / no-dot
//      inputs surface as -32601 not panic.
//   F. REQ-CDP-001 criterion C2 (JSON-RPC 2.0 codec correctness) alignment.
//   G. Trait completeness (Clone/Eq/Debug/serde roundtrip for wire types).
// ===========================================================================

// ---- A. parse_message: params scalar types & session_id boundary ----

#[test]
fn test_parse_params_string_scalar() {
    // CdpMessage.params: Option<Value> — any valid JSON value accepted;
    // a bare string params survives deserialization.
    let raw = r#"{"id":1,"method":"X.y","params":"scalar"}"#;
    let msg = parse_message(raw).unwrap();
    let p = msg.params.unwrap();
    assert_eq!(p, json!("scalar"));
    assert!(p.is_string());
}

#[test]
fn test_parse_params_number_scalar() {
    let raw = r#"{"id":1,"method":"X.y","params":42}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.params.unwrap(), json!(42));
}

#[test]
fn test_parse_params_bool_scalar() {
    let raw = r#"{"id":1,"method":"X.y","params":true}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.params.unwrap(), json!(true));
}

#[test]
fn test_parse_params_explicit_json_null_maps_none() {
    // params: null must deserialize to None (per Option<Value> serde semantics)
    let raw = r#"{"id":1,"method":"X.y","params":null}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.params.is_none(), "params:null must map to None");
}

#[test]
fn test_parse_empty_session_id_string() {
    // session_id: "" is a valid (if unusual) string — must round-trip as Some("")
    let raw = r#"{"id":1,"method":"X.y","session_id":""}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some(""));
}

#[test]
fn test_parse_unicode_session_id() {
    let raw = r#"{"id":1,"method":"X.y","session_id":"会话-001"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("会话-001"));
}

#[test]
fn test_parse_method_with_surrounding_whitespace() {
    // JSON allows insignificant whitespace; serde_json tolerates it.
    let raw = r#"  {"id":1,"method":"X.y"}  "#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.method, "X.y");
}

#[test]
fn test_parse_id_i64_min_max_boundary() {
    // i64 boundaries (per CdpMessage.id: Option<i64>)
    let raw = r#"{"id":-9223372036854775808,"method":"X.y"}"#;
    assert_eq!(parse_message(raw).unwrap().id, Some(i64::MIN));
    let raw = r#"{"id":9223372036854775807,"method":"X.y"}"#;
    assert_eq!(parse_message(raw).unwrap().id, Some(i64::MAX));
}

#[test]
fn test_parse_id_overflow_i64_rejected() {
    // 2^63 must NOT deserialize as i64 — parse_message returns None.
    let raw = r#"{"id":9223372036854775808,"method":"X.y"}"#;
    assert!(
        parse_message(raw).is_none(),
        "2^63 must not be accepted as i64 id"
    );
}

#[test]
fn test_parse_duplicate_keys_rejected() {
    // serde_json rejects duplicate JSON object keys by default (entry conflict).
    // parse_message therefore returns None — this is strict, well-defined
    // behavior (NOT last-wins). Documents the codec's duplicate-key stance.
    let raw = r#"{"id":1,"id":2,"method":"X.y"}"#;
    assert!(
        parse_message(raw).is_none(),
        "duplicate keys must be rejected (serde_json strict mode), not last-wins"
    );
}

#[test]
fn test_parse_method_only_dot() {
    // "." splits to ["", ""] → unknown domain → -32601 (parse itself succeeds)
    let raw = r#"{"id":1,"method":"."}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.method, ".");
}

#[test]
fn test_parse_deeply_nested_params() {
    // Stress: nested objects must not stack-overflow or truncate.
    let raw = r#"{"id":1,"method":"X.y","params":{"a":{"b":{"c":{"d":{"e":1}}}}}}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.params.unwrap()["a"]["b"]["c"]["d"]["e"], 1);
}

// ---- B. serialize_response: id:None, mutual exclusivity, wire shape ----

#[test]
fn test_serialize_response_id_none_notification_echo() {
    // JSON-RPC 2.0 notification (no id) → response.id must be null, not omitted.
    let resp = CdpResponse {
        id: None,
        result: Some(json!({})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert!(
        p["id"].is_null(),
        "notification response id must serialize as null"
    );
    assert!(p.get("result").is_some());
    assert!(p.get("error").is_none());
}

#[test]
fn test_serialize_response_success_excludes_error_key() {
    // Success path: result present, error MUST be absent in serialized output.
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"v": 1})),
        error: None,
    };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert!(p.get("result").is_some());
    assert!(
        p.get("error").is_none(),
        "success response must not carry error key"
    );
}

#[test]
fn test_serialize_response_error_excludes_result_key() {
    // Error path: error present, result MUST be absent in serialized output.
    let resp = CdpResponse {
        id: Some(1),
        result: None,
        error: Some(CdpError {
            code: -32601,
            message: "m".into(),
        }),
    };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert!(p.get("error").is_some());
    assert!(
        p.get("result").is_none(),
        "error response must not carry result key"
    );
}

#[test]
fn test_serialize_response_neither_result_nor_error() {
    // Degenerate response (both None). CdpResponse uses
    // #[serde(skip_serializing_if = "Option::is_none")] on both result & error,
    // so NEITHER key appears in the wire form — only {id}. Codec must be total
    // (no panic) and emit valid JSON.
    let resp = CdpResponse {
        id: Some(7),
        result: None,
        error: None,
    };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["id"], 7);
    assert!(
        p.get("result").is_none(),
        "skipped result must not appear in wire form"
    );
    assert!(
        p.get("error").is_none(),
        "skipped error must not appear in wire form"
    );
    // Only the id key survives
    let keys: Vec<&str> = p.as_object().unwrap().keys().map(|s| s.as_str()).collect();
    assert_eq!(keys, vec!["id"]);
}

#[test]
fn test_serialize_response_output_is_compact_single_line() {
    // serialize_response uses serde_json::to_string (compact, no whitespace).
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"a":1})),
        error: None,
    };
    let raw = serialize_response(&resp);
    assert!(
        !raw.contains('\n'),
        "serialized response must be single-line"
    );
}

#[test]
fn test_serialize_response_stable_under_reparse() {
    // serialize → parse → serialize must be idempotent.
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"k": "v"})),
        error: None,
    };
    let s1 = serialize_response(&resp);
    let reparsed: Value = serde_json::from_str(&s1).unwrap();
    let s2 = serde_json::to_string(&reparsed).unwrap();
    assert_eq!(s1, s2, "serialize must be stable under reparse");
}

// ---- C. serialize_event: empty method, null params, array params ----

#[test]
fn test_serialize_event_empty_method() {
    // Empty method string must still produce valid JSON (boundary).
    let ev = CdpEvent {
        method: "".into(),
        params: None,
    };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["method"], "");
    assert!(p.get("params").is_none());
}

#[test]
fn test_serialize_event_null_params_present() {
    // params: Some(Value::Null) — distinct from None — must serialize as null.
    let ev = CdpEvent {
        method: "X.y".into(),
        params: Some(Value::Null),
    };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["method"], "X.y");
    assert!(
        p.get("params").is_some(),
        "explicit null params must be present as null"
    );
    assert!(p["params"].is_null());
}

#[test]
fn test_serialize_event_array_params() {
    let ev = CdpEvent {
        method: "X.y".into(),
        params: Some(json!([1, 2, 3])),
    };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["params"].as_array().unwrap().len(), 3);
}

#[test]
fn test_serialize_event_output_compact() {
    let ev = CdpEvent {
        method: "X.y".into(),
        params: Some(json!({"a":1})),
    };
    let raw = serialize_event(&ev);
    assert!(
        !raw.contains('\n'),
        "event JSON must be compact (single-line wire form)"
    );
}

// ---- D. handle_command: boundary & SPEC-aligned cases ----

#[test]
fn test_handle_multiple_dot_method_splitn_first_dot_only() {
    // method "Page.navigate.foo" → splitn(2,'.') → domain="Page",
    // command="navigate.foo". Page handler exact-matches commands, so
    // "navigate.foo" != "navigate" → -32601 Page-unknown, NOT a crash.
    let msg = CdpMessage {
        id: Some(1),
        method: "Page.navigate.foo".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t-1", &None, None);
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32601);
    assert!(
        err.message.contains("navigate.foo"),
        "error message must carry the full command suffix after first dot"
    );
}

#[test]
fn test_handle_empty_target_id_get_targets() {
    // Empty target_id boundary — getTargets still returns exactly one entry
    // with targetId == "".
    let msg = CdpMessage {
        id: Some(1),
        method: "Target.getTargets".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "", &None, None);
    let r = resp.result.unwrap();
    let infos = r["targetInfos"].as_array().unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(
        infos[0]["targetId"], "",
        "empty target_id must round-trip verbatim"
    );
    assert_eq!(infos[0]["type"], "page");
}

#[test]
fn test_attach_to_target_session_id_formula_self_consistent() {
    // sessionId = format!("{:016x}", target_id.chars().map(|c| c as u64).sum())
    // Invariants: (1) deterministic per target, (2) 16 hex chars zero-padded,
    // (3) distinct targets yield distinct sessionIds for distinct char sums,
    // (4) empty target → all-zero sessionId.
    fn expected_sid(tid: &str) -> String {
        format!("{:016x}", tid.chars().map(|c| c as u64).sum::<u64>())
    }

    // dispatch helper uses "target-001"; recompute for sanity
    let r = ok_result("Target.attachToTarget", None);
    assert_eq!(r["sessionId"].as_str().unwrap(), expected_sid("target-001"));

    // distinct targets via direct handle_command
    let m1 = CdpMessage {
        id: Some(1),
        method: "Target.attachToTarget".into(),
        params: None,
        session_id: None,
    };
    let r1 = handle_command(m1, "AAAA", &None, None).result.unwrap();
    let m2 = CdpMessage {
        id: Some(2),
        method: "Target.attachToTarget".into(),
        params: None,
        session_id: None,
    };
    let r2 = handle_command(m2, "BBBB", &None, None).result.unwrap();
    assert_eq!(r1["sessionId"], expected_sid("AAAA"));
    assert_eq!(r2["sessionId"], expected_sid("BBBB"));
    assert_ne!(r1["sessionId"], r2["sessionId"]);

    // 16-char zero-padded hex
    let sid = r1["sessionId"].as_str().unwrap();
    assert_eq!(sid.len(), 16, "sessionId must be 16 hex chars");
    assert!(sid.chars().all(|c| c.is_ascii_hexdigit()));

    // empty target → all-zero sessionId
    let m3 = CdpMessage {
        id: Some(3),
        method: "Target.attachToTarget".into(),
        params: None,
        session_id: None,
    };
    let r3 = handle_command(m3, "", &None, None).result.unwrap();
    assert_eq!(r3["sessionId"], "0000000000000000");
}

#[test]
fn test_page_navigate_empty_url_defaults_to_about_blank() {
    // BCE-20260621-EMPTY-STR: empty url "" falls back to "about:blank" for the
    // bridge command; without a bridge the url defaulting still resolves to
    // the same explicit -32603 as any other navigate (no canned success).
    let e = err_result("Page.navigate", Some(json!({"url": ""})));
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_navigate_missing_params_field() {
    // No params at all → default url "about:blank" path → explicit -32603.
    let e = err_result("Page.navigate", None);
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_navigate_url_non_string_ignored() {
    // url given as number (invalid type) → as_str() is None → default url
    // path → explicit -32603.
    let e = err_result("Page.navigate", Some(json!({"url": 123})));
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_capture_screenshot_default_format_png() {
    // No format param → format defaults to "png", but a renderer is still
    // required — explicit -32603, never empty data.
    let e = err_result("Page.captureScreenshot", None);
    assert_eq!(e.code, -32603);
}

#[test]
fn test_page_capture_screenshot_quality_zero() {
    let e = err_result(
        "Page.captureScreenshot",
        Some(json!({"format": "jpeg", "quality": 0})),
    );
    assert_eq!(e.code, -32603);
}

#[test]
fn test_fetch_enable_patterns_non_array_treated_as_zero() {
    // patterns as non-array (string/object) → unwrap_or(0) → patternCount 0.
    let r1 = ok_result("Fetch.enable", Some(json!({"patterns": "not-an-array"})));
    assert_eq!(r1["patternCount"], 0);
    assert_eq!(r1["enabled"], true);

    let r2 = ok_result(
        "Fetch.enable",
        Some(json!({"patterns": {"urlPattern": "*"}})),
    );
    assert_eq!(r2["patternCount"], 0);
}

#[test]
fn test_fetch_enable_patterns_null() {
    let r = ok_result("Fetch.enable", Some(json!({"patterns": null})));
    assert_eq!(r["patternCount"], 0);
    assert_eq!(r["enabled"], true);
}

#[test]
fn test_fetch_fulfill_request_body_length_byte_semantics() {
    // bodyLength is byte length, not char count. Multi-byte UTF-8 counts bytes.
    // "héllo" → h(1) é(2) l(1) l(1) o(1) = 6 bytes
    let r = ok_result(
        "Fetch.fulfillRequest",
        Some(json!({
            "requestId": "r1", "body": "héllo"
        })),
    );
    assert_eq!(r["bodyLength"], "héllo".len());
    assert_eq!(
        r["bodyLength"], 6,
        "multi-byte UTF-8 body must count bytes not chars"
    );
}

#[test]
fn test_fetch_fulfill_request_unicode_body() {
    // Pure CJK body — 3 chars × 3 bytes = 9 bytes.
    let r = ok_result(
        "Fetch.fulfillRequest",
        Some(json!({
            "requestId": "r1", "body": "你好吗"
        })),
    );
    assert_eq!(r["bodyLength"], 9);
}

#[test]
fn test_fetch_continue_request_missing_request_id() {
    // No requestId param → empty string echoed back (params_str default).
    let r = ok_result("Fetch.continueRequest", None);
    assert_eq!(r["requestId"], "");
    assert_eq!(r["continued"], true);
}

#[test]
fn test_fetch_fail_request_missing_reason() {
    let r = ok_result("Fetch.failRequest", Some(json!({"requestId": "r1"})));
    assert_eq!(r["requestId"], "r1");
    assert_eq!(r["failed"], true);
    assert_eq!(r["reason"], "", "missing reason defaults to empty string");
}

#[test]
fn test_emulation_set_user_agent_empty_no_bridge() {
    // Empty UA + no bridge → ok_empty (UA bridge_send only when non-empty).
    let r = ok_result(
        "Emulation.setUserAgentOverride",
        Some(json!({"userAgent": ""})),
    );
    assert_eq!(r, json!({}));
}

#[test]
fn test_emulation_set_device_metrics_negative_width_no_panic() {
    // Negative width via as_u64() → None → default 1920. No panic.
    let r = ok_result(
        "Emulation.setDeviceMetricsOverride",
        Some(json!({"width": -1, "height": -1})),
    );
    assert_eq!(
        r,
        json!({}),
        "negative width/height must not panic; defaults applied"
    );
}

#[test]
fn test_input_dispatch_mouse_negative_coords() {
    // Negative x/y are valid f64; no bridge → ok_empty, no panic.
    let r = ok_result(
        "Input.dispatchMouseEvent",
        Some(json!({"type": "mouseMoved", "x": -100.5, "y": -200.5})),
    );
    assert_eq!(r, json!({}));
}

#[test]
fn test_input_dispatch_key_event_no_type() {
    // Missing type → empty string, still ok_empty.
    let r = ok_result("Input.dispatchKeyEvent", Some(json!({})));
    assert_eq!(r, json!({}));
}

#[test]
fn test_dom_set_attribute_value_missing_name_value() {
    // No name/value → both default to "" — still ok_empty (no bridge).
    let r = ok_result("DOM.setAttributeValue", Some(json!({"nodeId": 5})));
    assert_eq!(r, json!({}));
}

#[test]
fn test_dom_get_outer_html_missing_node_id() {
    // No nodeId → node_id None → still requires the live document via the
    // bridge — explicit -32603, never canned html.
    let e = err_result("DOM.getOuterHTML", None);
    assert_eq!(e.code, -32603);
}

#[test]
fn test_debugger_set_breakpoint_returns_empty_locations_array() {
    let r = ok_result("Debugger.setBreakpointByUrl", None);
    assert_eq!(r["breakpointId"], "1");
    let locs = r["locations"].as_array().unwrap();
    assert_eq!(
        locs.len(),
        0,
        "no-bridge breakpoint locations must be empty array"
    );
}

#[test]
fn test_runtime_evaluate_no_bridge_with_expression_still_undefined() {
    // No bridge: even WITH a non-empty expression, returns undefined (bridge
    // path gated by bridge.is_some()). Documented no-bridge stub.
    let r = ok_result("Runtime.evaluate", Some(json!({"expression": "1+1"})));
    assert_eq!(r["result"]["type"], "undefined");
    assert!(r["exceptionDetails"].is_null());
}

#[test]
fn test_runtime_evaluate_return_by_value_param_accepted() {
    // returnByValue must not change no-bridge stub output but must be accepted.
    let r = ok_result(
        "Runtime.evaluate",
        Some(json!({"expression": "x", "returnByValue": false})),
    );
    assert_eq!(r["result"]["type"], "undefined");
}

// ---- E. Roundtrip wire-shape invariants (REQ-CDP-001 C2 alignment) ----

#[test]
fn test_roundtrip_success_response_has_no_error_key() {
    // REQ-CDP-001-C2: JSON-RPC 2.0 success response shape = {id, result}
    let raw = r#"{"id":100,"method":"Page.enable"}"#;
    let msg = parse_message(raw).unwrap();
    let resp = handle_command(msg, "t-1", &None, None);
    let s = serialize_response(&resp);
    let p: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(p["id"], 100);
    assert!(p.get("result").is_some(), "success must include result");
    assert!(
        p.get("error").is_none(),
        "success must NOT include error key"
    );
    // codec adds no jsonrpc field (no version negotiation in this impl)
    assert!(p.get("jsonrpc").is_none());
}

#[test]
fn test_roundtrip_error_response_has_no_result_key() {
    // REQ-CDP-001-C2: JSON-RPC 2.0 error response shape = {id, error}
    let raw = r#"{"id":200,"method":"Nope.nope"}"#;
    let msg = parse_message(raw).unwrap();
    let resp = handle_command(msg, "t-1", &None, None);
    let s = serialize_response(&resp);
    let p: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(p["id"], 200);
    assert!(p.get("error").is_some(), "error must include error key");
    assert!(
        p.get("result").is_none(),
        "error must NOT include result key"
    );
    // error object shape = {code (int), message (str)}
    assert!(p["error"]["code"].is_i64());
    assert!(p["error"]["message"].is_string());
}

#[test]
fn test_roundtrip_notification_no_id_preserved_as_null() {
    // JSON-RPC 2.0 notification: request without id.
    // handle_command echoes msg.id (None) → serialized id == null.
    let raw = r#"{"method":"Page.enable"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, None);
    let resp = handle_command(msg, "t-1", &None, None);
    let s = serialize_response(&resp);
    let p: Value = serde_json::from_str(&s).unwrap();
    assert!(
        p["id"].is_null(),
        "notification response id must serialize as JSON null"
    );
}

#[test]
fn test_roundtrip_method_no_dot_returns_method_not_found() {
    // Method without "." → domain = whole method, command = "" → unknown domain.
    let raw = r#"{"id":1,"method":"NoDot"}"#;
    let msg = parse_message(raw).unwrap();
    let resp = handle_command(msg, "t-1", &None, None);
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32601);
    assert!(
        err.message.contains("NoDot"),
        "error message must echo the malformed method"
    );
}

#[test]
fn test_roundtrip_all_12_domains_success_have_no_error_key() {
    // Exhaustive REQ-CDP-001-C2 invariant: every domain's canonical success
    // command must serialize to a response WITHOUT an error key.
    let cases: &[(&str, Option<Value>)] = &[
        ("Target.getTargets", None),
        ("Page.enable", None),
        ("Runtime.enable", None),
        ("DOM.enable", None),
        ("Network.enable", None),
        ("CSS.enable", None),
        ("Emulation.setDeviceMetricsOverride", Some(json!({}))),
        ("Input.dispatchMouseEvent", Some(json!({"type": "x"}))),
        ("Overlay.enable", None),
        ("Debugger.enable", None),
        ("Log.enable", None),
        ("Fetch.enable", None),
    ];
    for (method, params) in cases {
        let msg = CdpMessage {
            id: Some(1),
            method: method.to_string(),
            params: params.clone(),
            session_id: None,
        };
        let resp = handle_command(msg, "t-1", &params, None);
        assert!(resp.error.is_none(), "{} should succeed", method);
        assert!(resp.result.is_some(), "{} must carry result", method);
        let s = serialize_response(&resp);
        let p: Value = serde_json::from_str(&s).unwrap();
        assert!(
            p.get("error").is_none(),
            "{} serialized success must not have error key",
            method
        );
    }
}

#[test]
fn test_roundtrip_error_response_includes_code_and_message_fields() {
    // JSON-RPC 2.0 §5.1 error object: code (number) + message (string) required.
    let raw = r#"{"id":1,"method":"Unknown.cmd"}"#;
    let msg = parse_message(raw).unwrap();
    let resp = handle_command(msg, "t-1", &None, None);
    let s = serialize_response(&resp);
    let p: Value = serde_json::from_str(&s).unwrap();
    let err = &p["error"];
    assert!(err.is_object());
    assert!(err["code"].is_i64(), "error.code must be integer");
    assert!(err["message"].is_string(), "error.message must be string");
    assert!(
        !err["message"].as_str().unwrap().is_empty(),
        "error.message must be non-empty"
    );
}

// ---- F. Determinism & idempotence ----

#[test]
fn test_serialize_response_idempotent_across_calls() {
    // Same input → byte-identical output across multiple calls (no hidden state).
    let resp = CdpResponse {
        id: Some(42),
        result: Some(json!({"x": [1, 2, 3]})),
        error: None,
    };
    let s1 = serialize_response(&resp);
    let s2 = serialize_response(&resp);
    let s3 = serialize_response(&resp);
    assert_eq!(s1, s2);
    assert_eq!(s2, s3);
}

#[test]
fn test_serialize_event_idempotent_across_calls() {
    let ev = CdpEvent {
        method: "Page.frameNavigated".into(),
        params: Some(json!({"frame": {"id": "0"}})),
    };
    let s1 = serialize_event(&ev);
    let s2 = serialize_event(&ev);
    assert_eq!(s1, s2);
}

#[test]
fn test_handle_command_deterministic_same_input_same_output() {
    // Deterministic dispatch: same (method, target_id, params) → same response.
    // CdpError has no PartialEq, so compare via serialized form (it has Serialize).
    let mk = || CdpMessage {
        id: Some(1),
        method: "Page.navigate".into(),
        params: Some(json!({"url": "https://x.com"})),
        session_id: None,
    };
    let r1 = handle_command(mk(), "t-1", &Some(json!({"url": "https://x.com"})), None);
    let r2 = handle_command(mk(), "t-1", &Some(json!({"url": "https://x.com"})), None);
    assert_eq!(r1.id, r2.id);
    assert_eq!(r1.result, r2.result);
    // error comparison via serialized JSON (CdpError: no PartialEq, has Serialize)
    assert_eq!(
        serde_json::to_string(&r1.error).unwrap(),
        serde_json::to_string(&r2.error).unwrap(),
    );
}

#[test]
fn test_error_code_json_rpc_2_0_spec_value_minus_32601() {
    // JSON-RPC 2.0 §5.1: -32601 = "Method not found". All unknown-command
    // paths must emit exactly -32601, never -32700 (parse) or -32603 (internal).
    let cases = [
        "Target.x",
        "Page.x",
        "Runtime.x",
        "DOM.x",
        "Network.x",
        "CSS.x",
        "Emulation.x",
        "Input.x",
        "Overlay.x",
        "Debugger.x",
        "Log.x",
        "Fetch.x",
        "CompletelyUnknown.x",
        "",
    ];
    for method in &cases {
        let msg = CdpMessage {
            id: Some(1),
            method: method.to_string(),
            params: None,
            session_id: None,
        };
        let resp = handle_command(msg, "t-1", &None, None);
        let err = resp
            .error
            .unwrap_or_else(|| panic!("{} must error", method));
        assert_eq!(
            err.code, -32601,
            "method {} must yield -32601 not {}",
            method, err.code
        );
    }
}

// ---- G. Trait completeness (Clone/Debug/Serialize for wire types) ----
//
// NOTE on derives (from src/cdp-server/src/protocol.rs):
//   CdpMessage: Debug, Clone, Deserialize   (no PartialEq, no Serialize)
//   CdpResponse: Debug, Serialize           (no Clone, no PartialEq)
//   CdpError:    Debug, Clone, Serialize    (no PartialEq, no Deserialize)
//   CdpEvent:    Debug, Clone, Serialize    (no PartialEq, no Deserialize)
// These tests assert ONLY the traits actually derived — they document the
// public trait surface so future derive regressions are caught.

#[test]
fn test_cdp_error_clone_roundtrip_fields() {
    // CdpError derives Clone (no PartialEq) — verify via field-by-field.
    let a = CdpError {
        code: -32601,
        message: "m".into(),
    };
    let b = a.clone();
    assert_eq!(b.code, -32601);
    assert_eq!(b.message, "m");
    // clone is independent (mutating b must not affect a)
    let mut c = a.clone();
    c.code = -999;
    assert_eq!(a.code, -32601, "clone must be a deep copy");
    assert_eq!(c.code, -999);
}

#[test]
fn test_cdp_error_serialize_roundtrip_via_json() {
    // CdpError derives Serialize but NOT Deserialize — round-trip via Value.
    let err = CdpError {
        code: -32603,
        message: "internal err".into(),
    };
    let s = serde_json::to_string(&err).unwrap();
    let back: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(back["code"], -32603);
    assert_eq!(back["message"], "internal err");
}

#[test]
fn test_cdp_message_clone_preserves_all_fields() {
    // CdpMessage derives Clone — verify field-by-field (no PartialEq).
    let msg = CdpMessage {
        id: Some(5),
        method: "Page.enable".into(),
        params: Some(json!({"k": "v"})),
        session_id: Some("sess".into()),
    };
    let c = msg.clone();
    assert_eq!(c.id, Some(5));
    assert_eq!(c.method, "Page.enable");
    assert_eq!(c.params.as_ref().unwrap()["k"], "v");
    assert_eq!(c.session_id.as_deref(), Some("sess"));
}

#[test]
fn test_cdp_message_clone_preserves_none_variants() {
    let msg = CdpMessage {
        id: None,
        method: "X.y".into(),
        params: None,
        session_id: None,
    };
    let c = msg.clone();
    assert_eq!(c.id, None);
    assert!(c.params.is_none());
    assert!(c.session_id.is_none());
    assert_eq!(c.method, "X.y");
}

#[test]
fn test_cdp_event_clone_with_some_params() {
    let ev = CdpEvent {
        method: "X.y".into(),
        params: Some(json!({"a": 1})),
    };
    let c = ev.clone();
    assert_eq!(c.method, "X.y");
    assert_eq!(c.params.as_ref().unwrap()["a"], 1);
}

#[test]
fn test_cdp_event_clone_with_none_params() {
    let ev = CdpEvent {
        method: "X.y".into(),
        params: None,
    };
    let c = ev.clone();
    assert_eq!(c.method, "X.y");
    assert!(c.params.is_none());
}

#[test]
fn test_cdp_response_debug_format_present() {
    // CdpResponse derives Debug (no Clone) — assert Debug output is non-empty
    // and carries the id.
    let resp = CdpResponse {
        id: Some(42),
        result: Some(json!({"a": 1})),
        error: None,
    };
    let dbg = format!("{:?}", resp);
    assert!(dbg.contains("42"));
    assert!(dbg.contains("CdpResponse") || dbg.contains("id"));
}

#[test]
fn test_cdp_response_field_access_without_clone() {
    // CdpResponse lacks Clone — verify fields are directly accessible and that
    // moving result/error out is possible (documents the non-Clone constraint).
    let resp = CdpResponse {
        id: Some(7),
        result: Some(json!({"v": 9})),
        error: None,
    };
    assert_eq!(resp.id, Some(7));
    let r = resp.result.unwrap();
    assert_eq!(r["v"], 9);
    assert!(resp.error.is_none());
}

// ---- H. JSON-RPC 2.0 notification parse semantics ----

#[test]
fn test_notification_parses_with_none_id() {
    // Strictly per JSON-RPC 2.0: a notification carries no id.
    let raw = r#"{"method":"Page.frameNavigated","params":{"frame":{"id":"0"}}}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, None, "notification id must deserialize to None");
    assert_eq!(msg.method, "Page.frameNavigated");
    assert!(msg.params.is_some());
}

#[test]
fn test_notification_with_params_no_id() {
    let raw = r#"{"method":"Network.responseReceived","params":{"requestId":"r1"}}"#;
    let msg = parse_message(raw).unwrap();
    assert!(msg.id.is_none());
    assert_eq!(msg.params.unwrap()["requestId"], "r1");
}
