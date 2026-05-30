// @trace TEST-CDP-011 [req:REQ-CDP-001] [level:unit]
// @trace TEST-CDP-012 [req:REQ-CDP-002] [level:unit]
// @trace TEST-CDP-013 [req:REQ-CDP-004] [level:unit]
// Protocol message layer deep tests: parse_message, handle_command (all 11 domains
// without bridge), serialize_response, serialize_event, CDPMessage/CDPResponse/
// CDPError/CDPEvent serialization edge cases, roundtrip consistency.

use bao_cdp::{CDPMessage, CDPResponse, CDPError, CDPEvent};
use bao_cdp::{parse_message, handle_command, serialize_response, serialize_event};
use serde_json::{Value, json};

// ---- parse_message: valid inputs ----

#[test]
fn test_parse_valid_minimal() {
    let raw = r#"{"id":1,"method":"Page.enable"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 1);
    assert_eq!(msg.method, "Page.enable");
    assert!(msg.params.is_none());
    assert!(msg.session_id.is_none());
}

#[test]
fn test_parse_full_message() {
    let raw = r#"{"id":42,"method":"Runtime.evaluate","params":{"expression":"1+1"},"session_id":"sess-abc"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 42);
    assert_eq!(msg.method, "Runtime.evaluate");
    assert_eq!(msg.params.as_ref().unwrap()["expression"], "1+1");
    assert_eq!(msg.session_id.as_ref().unwrap(), "sess-abc");
}

#[test]
fn test_parse_with_null_params() {
    let raw = r#"{"id":2,"method":"Page.enable","params":null}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 2);
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
    assert_eq!(msg.id, -1);
}

#[test]
fn test_parse_large_id() {
    let raw = r#"{"id":99999999999,"method":"Test.cmd"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 99999999999i64);
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
    assert_eq!(msg.id, 1);
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
    let raw = r#"{"method":"Page.enable"}"#;
    assert!(parse_message(raw).is_none());
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

fn dispatch(method: &str, params: Option<Value>) -> CDPResponse {
    let msg = CDPMessage {
        id: 1,
        method: method.to_string(),
        params: params.clone(),
        session_id: None,
    };
    handle_command(msg, "target-001", &params, None)
}

fn ok_result(method: &str, params: Option<Value>) -> Value {
    dispatch(method, params).result.unwrap()
}

fn err_result(method: &str, params: Option<Value>) -> CDPError {
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
    let r = ok_result("Page.navigate", Some(json!({"url": "https://example.com"})));
    assert_eq!(r["frameId"], "0");
    assert!(r["loaderId"].is_string());
}

#[test]
fn test_page_navigate_default_url() {
    let r = ok_result("Page.navigate", Some(json!({})));
    assert_eq!(r["frameId"], "0");
}

#[test]
fn test_page_reload_no_bridge() {
    let r = ok_result("Page.reload", Some(json!({"ignoreCache": true})));
    assert_eq!(r["frameId"], "0");
    assert_eq!(r["loaderId"], "0");
}

#[test]
fn test_page_get_frame_tree_no_bridge() {
    let r = ok_result("Page.getFrameTree", None);
    let frame = r["frameTree"]["frame"].as_object().unwrap();
    assert_eq!(frame["id"], "0");
    assert_eq!(frame["url"], "about:blank");
    assert_eq!(frame["mimeType"], "text/html");
}

#[test]
fn test_page_get_navigation_history_no_bridge() {
    let r = ok_result("Page.getNavigationHistory", None);
    assert_eq!(r["currentIndex"], 0);
    let entries = r["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["url"], "about:blank");
}

#[test]
fn test_page_capture_screenshot_no_bridge() {
    let r = ok_result("Page.captureScreenshot", Some(json!({"format": "png"})));
    assert_eq!(r["data"], "");
}

#[test]
fn test_page_capture_screenshot_jpeg_no_bridge() {
    let r = ok_result("Page.captureScreenshot", Some(json!({"format": "jpeg", "quality": 80})));
    assert_eq!(r["data"], "");
}

#[test]
fn test_page_set_content() {
    assert_eq!(ok_result("Page.setContent", None), json!({}));
}

#[test]
fn test_page_close() {
    assert_eq!(ok_result("Page.close", None), json!({}));
}

#[test]
fn test_page_bring_to_front() {
    assert_eq!(ok_result("Page.bringToFront", None), json!({}));
}

#[test]
fn test_page_get_layout_metrics() {
    let r = ok_result("Page.getLayoutMetrics", None);
    assert_eq!(r["contentSize"]["width"], 1920);
    assert_eq!(r["contentSize"]["height"], 1080);
    assert_eq!(r["cssContentSize"]["width"], 1920);
}

#[test]
fn test_page_add_script_to_evaluate_no_bridge() {
    let r = ok_result("Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source": "console.log(1)"})));
    assert_eq!(r["identifier"], "1");
}

#[test]
fn test_page_add_script_empty_source_no_bridge() {
    let r = ok_result("Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source": ""})));
    assert_eq!(r["identifier"], "1");
}

#[test]
fn test_page_remove_script() {
    assert_eq!(ok_result("Page.removeScriptToEvaluateOnNewDocument", None), json!({}));
}

#[test]
fn test_page_unknown_command() {
    assert_eq!(err_result("Page.nonexistent", None).code, -32601);
}

// ---- handle_command: Runtime domain (no bridge) ----

#[test]
fn test_runtime_enable() {
    let r = ok_result("Runtime.enable", None);
    assert_eq!(r["executionContextId"], 1);
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
    assert_eq!(ok_result("DOM.setAttributeValue",
        Some(json!({"nodeId": 5, "name": "class", "value": "test"}))), json!({}));
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
    let r = ok_result("DOM.getOuterHTML", Some(json!({"nodeId": 1})));
    assert_eq!(r["outerHTML"], "<html><body></body></html>");
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
    let r = ok_result("Network.getResponseBody", None);
    assert_eq!(r["body"], "");
    assert_eq!(r["base64Encoded"], false);
}

#[test]
fn test_network_set_cache_disabled() {
    assert_eq!(ok_result("Network.setCacheDisabled", None), json!({}));
}

#[test]
fn test_network_set_extra_http_headers() {
    assert_eq!(ok_result("Network.setExtraHTTPHeaders", None), json!({}));
}

#[test]
fn test_network_emulate_network_conditions() {
    assert_eq!(ok_result("Network.emulateNetworkConditions", None), json!({}));
}

#[test]
fn test_network_set_request_interception() {
    assert_eq!(ok_result("Network.setRequestInterception", None), json!({}));
}

#[test]
fn test_network_continue_intercepted_request() {
    assert_eq!(ok_result("Network.continueInterceptedRequest", None), json!({}));
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
    assert_eq!(ok_result("Emulation.setDeviceMetricsOverride",
        Some(json!({"width": 800, "height": 600, "deviceScaleFactor": 1.5}))), json!({}));
}

#[test]
fn test_emulation_set_device_metrics_defaults() {
    assert_eq!(ok_result("Emulation.setDeviceMetricsOverride", Some(json!({}))), json!({}));
}

#[test]
fn test_emulation_clear_device_metrics() {
    assert_eq!(ok_result("Emulation.clearDeviceMetricsOverride", None), json!({}));
}

#[test]
fn test_emulation_set_user_agent_no_bridge() {
    assert_eq!(ok_result("Emulation.setUserAgentOverride",
        Some(json!({"userAgent": ""}))), json!({}));
}

#[test]
fn test_emulation_set_touch_emulation() {
    assert_eq!(ok_result("Emulation.setTouchEmulationEnabled", None), json!({}));
}

#[test]
fn test_emulation_set_script_execution_disabled() {
    assert_eq!(ok_result("Emulation.setScriptExecutionDisabled", None), json!({}));
}

#[test]
fn test_emulation_set_focus_emulation() {
    assert_eq!(ok_result("Emulation.setFocusEmulationEnabled", None), json!({}));
}

#[test]
fn test_emulation_set_cpu_throttling_rate() {
    assert_eq!(ok_result("Emulation.setCPUThrottlingRate", None), json!({}));
}

#[test]
fn test_emulation_set_default_background_color_override() {
    assert_eq!(ok_result("Emulation.setDefaultBackgroundColorOverride", None), json!({}));
}

#[test]
fn test_emulation_unknown_command() {
    assert_eq!(err_result("Emulation.nonexistent", None).code, -32601);
}

// ---- handle_command: Input domain (no bridge) ----

#[test]
fn test_input_dispatch_mouse_event_no_bridge() {
    assert_eq!(ok_result("Input.dispatchMouseEvent",
        Some(json!({"type": "mousePressed", "x": 100.0, "y": 200.0}))), json!({}));
}

#[test]
fn test_input_dispatch_mouse_event_no_coords() {
    assert_eq!(ok_result("Input.dispatchMouseEvent",
        Some(json!({"type": "mouseMoved"}))), json!({}));
}

#[test]
fn test_input_dispatch_key_event_no_bridge() {
    assert_eq!(ok_result("Input.dispatchKeyEvent",
        Some(json!({"type": "keyDown", "key": "a", "code": "KeyA"}))), json!({}));
}

#[test]
fn test_input_dispatch_key_event_minimal() {
    assert_eq!(ok_result("Input.dispatchKeyEvent", Some(json!({}))), json!({}));
}

#[test]
fn test_input_dispatch_touch_event() {
    assert_eq!(ok_result("Input.dispatchTouchEvent", None), json!({}));
}

#[test]
fn test_input_insert_text_no_bridge() {
    assert_eq!(ok_result("Input.insertText", Some(json!({"text": ""}))), json!({}));
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
    assert_eq!(ok_result("Overlay.setPausedInDebuggerMessage", None), json!({}));
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
    let r = ok_result("Fetch.enable", Some(json!({
        "patterns": [{"urlPattern": "*"}, {"urlPattern": "https://*"}]
    })));
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
    let r = ok_result("Fetch.continueWithResponse", Some(json!({"requestId": "req-2"})));
    assert_eq!(r["requestId"], "req-2");
    assert_eq!(r["continued"], true);
}

#[test]
fn test_fetch_fail_request() {
    let r = ok_result("Fetch.failRequest", Some(json!({"requestId": "req-3", "reason": "Aborted"})));
    assert_eq!(r["requestId"], "req-3");
    assert_eq!(r["failed"], true);
    assert_eq!(r["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request() {
    let r = ok_result("Fetch.fulfillRequest", Some(json!({
        "requestId": "req-4", "responseCode": 404, "body": "not found"
    })));
    assert_eq!(r["requestId"], "req-4");
    assert_eq!(r["fulfilled"], true);
    assert_eq!(r["responseCode"], 404);
    assert_eq!(r["bodyLength"], 9);
}

#[test]
fn test_fetch_fulfill_request_default_code() {
    let r = ok_result("Fetch.fulfillRequest", Some(json!({"requestId": "r1", "body": ""})));
    assert_eq!(r["responseCode"], 200);
    assert_eq!(r["bodyLength"], 0);
}

#[test]
fn test_fetch_get_request_post_data() {
    let r = ok_result("Fetch.getRequestPostData", Some(json!({"requestId": "req-5"})));
    assert_eq!(r["requestId"], "req-5");
    assert_eq!(r["postData"], "");
}

#[test]
fn test_fetch_continue_with_auth() {
    let r = ok_result("Fetch.continueWithAuth", Some(json!({"requestId": "req-6"})));
    assert_eq!(r["requestId"], "req-6");
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let r = ok_result("Fetch.takeResponseBodyAsStream", Some(json!({"requestId": "req-7"})));
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
    let resp = CDPResponse { id: 1, result: Some(json!({"value": 42})), error: None };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["id"], 1);
    assert_eq!(p["result"]["value"], 42);
    assert!(p.get("error").is_none());
}

#[test]
fn test_serialize_error_response() {
    let resp = CDPResponse {
        id: 2, result: None,
        error: Some(CDPError { code: -32601, message: "not found".into() }),
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
    let resp = CDPResponse { id: 3, result: Some(json!({})), error: None };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["result"], json!({}));
}

#[test]
fn test_serialize_negative_id() {
    let resp = CDPResponse { id: -100, result: Some(json!(null)), error: None };
    let raw = serialize_response(&resp);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["id"], -100);
}

#[test]
fn test_serialize_zero_id() {
    let resp = CDPResponse { id: 0, result: Some(json!({})), error: None };
    let raw = serialize_response(&resp);
    assert!(serde_json::from_str::<Value>(&raw).is_ok());
}

// ---- serialize_event ----

#[test]
fn test_serialize_event_with_params() {
    let ev = CDPEvent { method: "Page.loadEventFired".into(), params: Some(json!({"timestamp": 12345})) };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["method"], "Page.loadEventFired");
    assert_eq!(p["params"]["timestamp"], 12345);
}

#[test]
fn test_serialize_event_without_params() {
    let ev = CDPEvent { method: "DOM.documentUpdated".into(), params: None };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["method"], "DOM.documentUpdated");
    assert!(p.get("params").is_none());
}

#[test]
fn test_serialize_event_empty_params() {
    let ev = CDPEvent { method: "Log.entryAdded".into(), params: Some(json!({})) };
    let raw = serialize_event(&ev);
    let p: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(p["params"], json!({}));
}

#[test]
fn test_serialize_event_complex_params() {
    let ev = CDPEvent {
        method: "Runtime.consoleAPICalled".into(),
        params: Some(json!({"type": "log", "timestamp": 999, "args": [{"type": "string", "value": "hello"}]})),
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

// ---- CDPError Debug/Serialize ----

#[test]
fn test_cdp_error_debug() {
    let err = CDPError { code: -32601, message: "test error".into() };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32601"));
    assert!(debug.contains("test error"));
}

#[test]
fn test_cdp_error_serialization() {
    let err = CDPError { code: -32000, message: "internal".into() };
    let json_str = serde_json::to_string(&err).unwrap();
    let p: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(p["code"], -32000);
    assert_eq!(p["message"], "internal");
}

// ---- CDPMessage Clone/Debug ----

#[test]
fn test_cdp_message_clone() {
    let msg = CDPMessage {
        id: 1, method: "Page.enable".into(),
        params: Some(json!({"key": "val"})),
        session_id: Some("sess-1".into()),
    };
    let cloned = msg.clone();
    assert_eq!(cloned.id, 1);
    assert_eq!(cloned.method, "Page.enable");
    assert_eq!(cloned.params.unwrap()["key"], "val");
    assert_eq!(cloned.session_id.unwrap(), "sess-1");
}

#[test]
fn test_cdp_message_debug() {
    let msg = CDPMessage { id: 1, method: "Test.cmd".into(), params: None, session_id: None };
    let debug = format!("{:?}", msg);
    assert!(debug.contains("Test.cmd"));
}

// ---- CDPEvent Clone/Debug ----

#[test]
fn test_cdp_event_clone() {
    let ev = CDPEvent { method: "Page.loadEventFired".into(), params: Some(json!({"ts": 1})) };
    let cloned = ev.clone();
    assert_eq!(cloned.method, "Page.loadEventFired");
    assert_eq!(cloned.params.unwrap()["ts"], 1);
}

#[test]
fn test_cdp_event_debug() {
    let ev = CDPEvent { method: "Test.evt".into(), params: None };
    let debug = format!("{:?}", ev);
    assert!(debug.contains("Test.evt"));
}

// ---- ID preservation through handle_command ----

#[test]
fn test_response_preserves_request_id() {
    for id in [0i64, 1, -1, 999, i64::MAX, i64::MIN] {
        let msg = CDPMessage { id, method: "Page.enable".into(), params: None, session_id: None };
        let resp = handle_command(msg, "t-1", &None, None);
        assert_eq!(resp.id, id, "Response ID should match request ID {}", id);
    }
}

// ---- Target sessionId deterministic ----

#[test]
fn test_attach_to_target_session_id_deterministic() {
    let msg1 = CDPMessage { id: 1, method: "Target.attachToTarget".into(), params: None, session_id: None };
    let r1 = handle_command(msg1, "target-abc", &None, None).result.unwrap();
    let sid1 = r1["sessionId"].as_str().unwrap().to_string();

    let msg2 = CDPMessage { id: 2, method: "Target.attachToTarget".into(), params: None, session_id: None };
    let r2 = handle_command(msg2, "target-abc", &None, None).result.unwrap();
    let sid2 = r2["sessionId"].as_str().unwrap().to_string();

    assert_eq!(sid1, sid2);
}

#[test]
fn test_different_targets_different_session_ids() {
    let msg1 = CDPMessage { id: 1, method: "Target.attachToTarget".into(), params: None, session_id: None };
    let r1 = handle_command(msg1, "target-A", &None, None).result.unwrap();
    let sid1 = r1["sessionId"].as_str().unwrap().to_string();

    let msg2 = CDPMessage { id: 2, method: "Target.attachToTarget".into(), params: None, session_id: None };
    let r2 = handle_command(msg2, "target-B", &None, None).result.unwrap();
    let sid2 = r2["sessionId"].as_str().unwrap().to_string();

    assert_ne!(sid1, sid2);
}

// ---- Page navigate loader_id from url length ----

#[test]
fn test_navigate_loader_id_from_url_length() {
    let url = "https://example.com/page";
    let msg = CDPMessage {
        id: 1, method: "Page.navigate".into(),
        params: Some(json!({"url": url})),
        session_id: None,
    };
    let r = handle_command(msg, "t-1", &Some(json!({"url": url})), None).result.unwrap();
    let loader_id = r["loaderId"].as_str().unwrap();
    assert_eq!(loader_id, format!("{:016x}", url.len() as u64));
}

// ---- All 12 domain error paths ----

#[test]
fn test_all_domains_unknown_command() {
    let domains = [
        "Target", "Page", "Runtime", "DOM", "Network",
        "CSS", "Emulation", "Input", "Overlay", "Debugger",
        "Log", "Fetch",
    ];
    for domain in &domains {
        let method = format!("{}.completelyUnknownCommand12345", domain);
        let msg = CDPMessage { id: 1, method: method.clone(), params: None, session_id: None };
        let resp = handle_command(msg, "t-1", &None, None);
        assert!(resp.error.is_some(), "{} unknown should error", domain);
        assert_eq!(resp.error.as_ref().unwrap().code, -32601);
        assert!(resp.error.unwrap().message.contains("completelyUnknownCommand12345"));
    }
}
