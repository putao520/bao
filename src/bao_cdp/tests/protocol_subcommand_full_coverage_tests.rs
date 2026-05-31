// @trace TEST-CDP-026 [req:REQ-CDP-001,REQ-CDP-004,REQ-CDP-005,REQ-CDP-006,REQ-CDP-007] [level:unit]
// Protocol domain handler sub-command full coverage — every command path in
// Page/Runtime/DOM/Network/CSS/Emulation/Input/Overlay/Debugger/Log/Fetch/Target.
// Tests without bridge (None) to verify default/stub responses.

use bao_cdp::{handle_command, serialize_response, serialize_event, CDPMessage, CDPResponse, CDPEvent};
use serde_json::json;

fn dispatch(method: &str, params: Option<serde_json::Value>) -> CDPResponse {
    let p = params;
    let msg = CDPMessage { id: 1, method: method.to_string(), params: None, session_id: None };
    handle_command(msg, "test-target", &p, None)
}

fn ok_resp(method: &str, params: Option<serde_json::Value>) -> bool {
    let r = dispatch(method, params);
    r.result.is_some() && r.error.is_none()
}

fn err_code(method: &str) -> i64 {
    let r = dispatch(method, None);
    r.error.map(|e| e.code).unwrap_or(0)
}

// ---- Target domain ----

#[test]
fn test_target_get_targets() {
    let r = dispatch("Target.getTargets", None);
    let result = r.result.unwrap();
    assert!(result["targetInfos"].is_array());
}

#[test]
fn test_target_get_target_targets() {
    let r = dispatch("Target.getTargetTargets", None);
    assert!(r.result.unwrap()["targetInfos"].is_array());
}

#[test]
fn test_target_create_target() {
    let r = dispatch("Target.createTarget", Some(json!({"url":"http://test"})));
    assert_eq!(r.result.unwrap()["targetId"], "test-target");
}

#[test]
fn test_target_close_target() {
    let r = dispatch("Target.closeTarget", Some(json!({"targetId":"t1"})));
    assert_eq!(r.result.unwrap()["success"], true);
}

#[test]
fn test_target_set_auto_attach() {
    assert!(ok_resp("Target.setAutoAttach", Some(json!({"flatten":true}))));
}

#[test]
fn test_target_set_discover_targets() {
    assert!(ok_resp("Target.setDiscoverTargets", None));
}

#[test]
fn test_target_get_target_info() {
    let r = dispatch("Target.getTargetInfo", None);
    let info = r.result.unwrap()["targetInfo"].clone();
    assert_eq!(info["targetId"], "test-target");
    assert_eq!(info["type"], "page");
}

#[test]
fn test_target_attach_to_target() {
    let r = dispatch("Target.attachToTarget", None);
    assert!(r.result.unwrap()["sessionId"].is_string());
}

#[test]
fn test_target_detach_from_target() {
    assert!(ok_resp("Target.detachFromTarget", None));
}

#[test]
fn test_target_send_message_to_target() {
    assert!(ok_resp("Target.sendMessageToTarget", None));
}

#[test]
fn test_target_unknown() {
    assert_eq!(err_code("Target.nonexistent"), -32601);
}

// ---- Page domain sub-commands ----

#[test]
fn test_page_enable() { assert!(ok_resp("Page.enable", None)); }

#[test]
fn test_page_disable() { assert!(ok_resp("Page.disable", None)); }

#[test]
fn test_page_navigate_default_url() {
    let r = dispatch("Page.navigate", Some(json!({})));
    let result = r.result.unwrap();
    assert_eq!(result["frameId"], "0");
}

#[test]
fn test_page_navigate_with_url() {
    let r = dispatch("Page.navigate", Some(json!({"url":"https://example.com"})));
    let result = r.result.unwrap();
    assert_eq!(result["frameId"], "0");
}

#[test]
fn test_page_reload_default() {
    let r = dispatch("Page.reload", None);
    let result = r.result.unwrap();
    assert_eq!(result["frameId"], "0");
}

#[test]
fn test_page_reload_ignore_cache() {
    let r = dispatch("Page.reload", Some(json!({"ignoreCache":true})));
    assert!(r.result.is_some());
}

#[test]
fn test_page_get_frame_tree() {
    let r = dispatch("Page.getFrameTree", None);
    let tree = r.result.unwrap()["frameTree"]["frame"].clone();
    assert_eq!(tree["id"], "0");
}

#[test]
fn test_page_get_navigation_history() {
    let r = dispatch("Page.getNavigationHistory", None);
    let result = r.result.unwrap();
    assert_eq!(result["currentIndex"], 0);
    assert!(result["entries"].is_array());
}

#[test]
fn test_page_capture_screenshot_default() {
    let r = dispatch("Page.captureScreenshot", None);
    assert!(r.result.unwrap()["data"].is_string());
}

#[test]
fn test_page_capture_screenshot_jpeg() {
    let r = dispatch("Page.captureScreenshot", Some(json!({"format":"jpeg"})));
    assert!(r.result.is_some());
}

#[test]
fn test_page_set_content() { assert!(ok_resp("Page.setContent", None)); }

#[test]
fn test_page_close() { assert!(ok_resp("Page.close", None)); }

#[test]
fn test_page_bring_to_front() { assert!(ok_resp("Page.bringToFront", None)); }

#[test]
fn test_page_get_layout_metrics() {
    let r = dispatch("Page.getLayoutMetrics", None);
    let result = r.result.unwrap();
    assert!(result["contentSize"]["width"].is_number());
}

#[test]
fn test_page_add_script() {
    let r = dispatch("Page.addScriptToEvaluateOnNewDocument", Some(json!({"source":"console.log(1)"})));
    assert_eq!(r.result.unwrap()["identifier"], "1");
}

#[test]
fn test_page_remove_script() { assert!(ok_resp("Page.removeScriptToEvaluateOnNewDocument", None)); }

#[test]
fn test_page_unknown() { assert_eq!(err_code("Page.nonexistent"), -32601); }

// ---- Runtime domain ----

#[test]
fn test_runtime_enable() {
    let r = dispatch("Runtime.enable", None);
    assert!(r.result.unwrap()["executionContextId"].is_number());
}

#[test]
fn test_runtime_disable() { assert!(ok_resp("Runtime.disable", None)); }

#[test]
fn test_runtime_evaluate_default() {
    let r = dispatch("Runtime.evaluate", None);
    let result = r.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_evaluate_expression() {
    let r = dispatch("Runtime.evaluate", Some(json!({"expression":"1+1"})));
    assert!(r.result.is_some());
}

#[test]
fn test_runtime_call_function_on() {
    let r = dispatch("Runtime.callFunctionOn", None);
    assert_eq!(r.result.unwrap()["result"]["type"], "undefined");
}

#[test]
fn test_runtime_get_properties() {
    let r = dispatch("Runtime.getProperties", None);
    assert!(r.result.unwrap()["result"].is_array());
}

#[test]
fn test_runtime_run_script() { assert!(ok_resp("Runtime.runScript", None)); }

#[test]
fn test_runtime_release_object() { assert!(ok_resp("Runtime.releaseObject", None)); }

#[test]
fn test_runtime_release_object_group() { assert!(ok_resp("Runtime.releaseObjectGroup", None)); }

#[test]
fn test_runtime_compile_script() { assert!(ok_resp("Runtime.compileScript", None)); }

#[test]
fn test_runtime_unknown() { assert_eq!(err_code("Runtime.nonexistent"), -32601); }

// ---- DOM domain ----

#[test]
fn test_dom_enable() { assert!(ok_resp("DOM.enable", None)); }

#[test]
fn test_dom_disable() { assert!(ok_resp("DOM.disable", None)); }

#[test]
fn test_dom_get_document() {
    let r = dispatch("DOM.getDocument", None);
    let root = r.result.unwrap()["root"].clone();
    assert_eq!(root["nodeType"], 9);
    assert_eq!(root["nodeName"], "#document");
}

#[test]
fn test_dom_describe_node() {
    let r = dispatch("DOM.describeNode", None);
    assert!(r.result.unwrap()["node"]["nodeName"].is_string());
}

#[test]
fn test_dom_query_selector_default() {
    let r = dispatch("DOM.querySelector", None);
    assert_eq!(r.result.unwrap()["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_default() {
    let r = dispatch("DOM.querySelectorAll", None);
    assert!(r.result.unwrap()["nodeIds"].is_array());
}

#[test]
fn test_dom_get_box_model() {
    let r = dispatch("DOM.getBoxModel", None);
    let model = r.result.unwrap()["model"].clone();
    assert!(model["width"].is_number());
}

#[test]
fn test_dom_set_attribute_value() {
    assert!(ok_resp("DOM.setAttributeValue", Some(json!({"nodeId":1,"name":"class","value":"active"}))));
}

#[test]
fn test_dom_remove_attribute() { assert!(ok_resp("DOM.removeAttribute", None)); }

#[test]
fn test_dom_set_outer_html() { assert!(ok_resp("DOM.setOuterHTML", None)); }

#[test]
fn test_dom_insert_before() { assert!(ok_resp("DOM.insertBefore", None)); }

#[test]
fn test_dom_remove_node() { assert!(ok_resp("DOM.removeNode", None)); }

#[test]
fn test_dom_get_outer_html_default() {
    let r = dispatch("DOM.getOuterHTML", None);
    assert!(r.result.unwrap()["outerHTML"].is_string());
}

#[test]
fn test_dom_resolve_node() {
    let r = dispatch("DOM.resolveNode", None);
    assert_eq!(r.result.unwrap()["object"]["type"], "node");
}

#[test]
fn test_dom_push_nodes() {
    let r = dispatch("DOM.pushNodesByBackendIdsToFrontend", None);
    assert!(r.result.unwrap()["nodeIds"].is_array());
}

#[test]
fn test_dom_unknown() { assert_eq!(err_code("DOM.nonexistent"), -32601); }

// ---- Network domain ----

#[test]
fn test_network_enable() { assert!(ok_resp("Network.enable", None)); }

#[test]
fn test_network_disable() { assert!(ok_resp("Network.disable", None)); }

#[test]
fn test_network_get_response_body() {
    let r = dispatch("Network.getResponseBody", None);
    assert_eq!(r.result.unwrap()["base64Encoded"], false);
}

#[test]
fn test_network_set_cache_disabled() { assert!(ok_resp("Network.setCacheDisabled", None)); }

#[test]
fn test_network_set_extra_http_headers() { assert!(ok_resp("Network.setExtraHTTPHeaders", None)); }

#[test]
fn test_network_emulate_conditions() { assert!(ok_resp("Network.emulateNetworkConditions", None)); }

#[test]
fn test_network_set_request_interception() { assert!(ok_resp("Network.setRequestInterception", None)); }

#[test]
fn test_network_continue_intercepted() { assert!(ok_resp("Network.continueInterceptedRequest", None)); }

#[test]
fn test_network_get_cookies() {
    let r = dispatch("Network.getCookies", None);
    assert!(r.result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies() {
    let r = dispatch("Network.getAllCookies", None);
    assert!(r.result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_delete_cookies() { assert!(ok_resp("Network.deleteCookies", None)); }

#[test]
fn test_network_set_cookie() { assert!(ok_resp("Network.setCookie", None)); }

#[test]
fn test_network_unknown() { assert_eq!(err_code("Network.nonexistent"), -32601); }

// ---- CSS domain ----

#[test]
fn test_css_enable() { assert!(ok_resp("CSS.enable", None)); }

#[test]
fn test_css_disable() { assert!(ok_resp("CSS.disable", None)); }

#[test]
fn test_css_get_computed_style() {
    let r = dispatch("CSS.getComputedStyleForNode", None);
    assert!(r.result.unwrap()["computedStyle"].is_array());
}

#[test]
fn test_css_get_matched_styles() {
    let r = dispatch("CSS.getMatchedStylesForNode", None);
    let result = r.result.unwrap();
    assert!(result["matchedCSSRules"].is_array());
}

#[test]
fn test_css_get_inline_styles() {
    let r = dispatch("CSS.getInlineStylesForNode", None);
    assert!(r.result.unwrap()["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    let r = dispatch("CSS.setStyleTexts", None);
    assert!(r.result.unwrap()["styles"].is_array());
}

#[test]
fn test_css_unknown() { assert_eq!(err_code("CSS.nonexistent"), -32601); }

// ---- Emulation domain ----

#[test]
fn test_emulation_set_device_metrics() {
    let r = dispatch("Emulation.setDeviceMetricsOverride", Some(json!({"width":1280,"height":720})));
    assert!(r.result.is_some());
}

#[test]
fn test_emulation_set_device_metrics_default() {
    assert!(ok_resp("Emulation.setDeviceMetricsOverride", None));
}

#[test]
fn test_emulation_clear_device_metrics() { assert!(ok_resp("Emulation.clearDeviceMetricsOverride", None)); }

#[test]
fn test_emulation_set_user_agent() {
    assert!(ok_resp("Emulation.setUserAgentOverride", Some(json!({"userAgent":"TestBot"}))));
}

#[test]
fn test_emulation_set_user_agent_empty() {
    assert!(ok_resp("Emulation.setUserAgentOverride", None));
}

#[test]
fn test_emulation_set_touch() { assert!(ok_resp("Emulation.setTouchEmulationEnabled", None)); }

#[test]
fn test_emulation_set_script_disabled() { assert!(ok_resp("Emulation.setScriptExecutionDisabled", None)); }

#[test]
fn test_emulation_set_focus() { assert!(ok_resp("Emulation.setFocusEmulationEnabled", None)); }

#[test]
fn test_emulation_set_cpu_throttle() { assert!(ok_resp("Emulation.setCPUThrottlingRate", None)); }

#[test]
fn test_emulation_set_bg_color() { assert!(ok_resp("Emulation.setDefaultBackgroundColorOverride", None)); }

#[test]
fn test_emulation_unknown() { assert_eq!(err_code("Emulation.nonexistent"), -32601); }

// ---- Input domain ----

#[test]
fn test_input_dispatch_mouse() {
    assert!(ok_resp("Input.dispatchMouseEvent", Some(json!({"type":"mousePressed","x":10,"y":20}))));
}

#[test]
fn test_input_dispatch_mouse_default() {
    assert!(ok_resp("Input.dispatchMouseEvent", None));
}

#[test]
fn test_input_dispatch_key() {
    assert!(ok_resp("Input.dispatchKeyEvent", Some(json!({"type":"keyDown","key":"a","code":"KeyA"}))));
}

#[test]
fn test_input_dispatch_touch() { assert!(ok_resp("Input.dispatchTouchEvent", None)); }

#[test]
fn test_input_insert_text() {
    assert!(ok_resp("Input.insertText", Some(json!({"text":"hello"}))));
}

#[test]
fn test_input_insert_text_empty() {
    assert!(ok_resp("Input.insertText", None));
}

#[test]
fn test_input_set_ignore() { assert!(ok_resp("Input.setIgnoreInputEvents", None)); }

#[test]
fn test_input_set_intercept_drags() { assert!(ok_resp("Input.setInterceptDrags", None)); }

#[test]
fn test_input_unknown() { assert_eq!(err_code("Input.nonexistent"), -32601); }

// ---- Overlay domain ----

#[test]
fn test_overlay_enable() { assert!(ok_resp("Overlay.enable", None)); }

#[test]
fn test_overlay_disable() { assert!(ok_resp("Overlay.disable", None)); }

#[test]
fn test_overlay_highlight_node() { assert!(ok_resp("Overlay.highlightNode", None)); }

#[test]
fn test_overlay_hide_highlight() { assert!(ok_resp("Overlay.hideHighlight", None)); }

#[test]
fn test_overlay_set_inspect_mode() { assert!(ok_resp("Overlay.setInspectMode", None)); }

#[test]
fn test_overlay_set_paused_message() { assert!(ok_resp("Overlay.setPausedInDebuggerMessage", None)); }

#[test]
fn test_overlay_unknown() { assert_eq!(err_code("Overlay.nonexistent"), -32601); }

// ---- Debugger domain ----

#[test]
fn test_debugger_enable() { assert!(ok_resp("Debugger.enable", None)); }

#[test]
fn test_debugger_disable() { assert!(ok_resp("Debugger.disable", None)); }

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let r = dispatch("Debugger.setBreakpointByUrl", None);
    assert!(r.result.unwrap()["breakpointId"].is_string());
}

#[test]
fn test_debugger_remove_breakpoint() { assert!(ok_resp("Debugger.removeBreakpoint", None)); }

#[test]
fn test_debugger_pause() { assert!(ok_resp("Debugger.pause", None)); }

#[test]
fn test_debugger_resume() { assert!(ok_resp("Debugger.resume", None)); }

#[test]
fn test_debugger_step_over() { assert!(ok_resp("Debugger.stepOver", None)); }

#[test]
fn test_debugger_step_into() { assert!(ok_resp("Debugger.stepInto", None)); }

#[test]
fn test_debugger_step_out() { assert!(ok_resp("Debugger.stepOut", None)); }

#[test]
fn test_debugger_set_skip_all() { assert!(ok_resp("Debugger.setSkipAllPauses", None)); }

#[test]
fn test_debugger_set_breakpoints_active() { assert!(ok_resp("Debugger.setBreakpointsActive", None)); }

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let r = dispatch("Debugger.evaluateOnCallFrame", None);
    assert_eq!(r.result.unwrap()["result"]["type"], "undefined");
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let r = dispatch("Debugger.getPossibleBreakpoints", None);
    assert!(r.result.unwrap()["locations"].is_array());
}

#[test]
fn test_debugger_get_script_source() {
    let r = dispatch("Debugger.getScriptSource", None);
    assert!(r.result.unwrap()["scriptSource"].is_string());
}

#[test]
fn test_debugger_set_pause_on_exceptions() { assert!(ok_resp("Debugger.setPauseOnExceptions", None)); }

#[test]
fn test_debugger_unknown() { assert_eq!(err_code("Debugger.nonexistent"), -32601); }

// ---- Log domain ----

#[test]
fn test_log_enable() { assert!(ok_resp("Log.enable", None)); }

#[test]
fn test_log_disable() { assert!(ok_resp("Log.disable", None)); }

#[test]
fn test_log_clear() { assert!(ok_resp("Log.clear", None)); }

#[test]
fn test_log_start_violations() { assert!(ok_resp("Log.startViolationsReport", None)); }

#[test]
fn test_log_stop_violations() { assert!(ok_resp("Log.stopViolationsReport", None)); }

#[test]
fn test_log_unknown() { assert_eq!(err_code("Log.nonexistent"), -32601); }

// ---- Fetch domain ----

#[test]
fn test_fetch_enable() {
    let r = dispatch("Fetch.enable", None);
    assert_eq!(r.result.unwrap()["enabled"], true);
}

#[test]
fn test_fetch_enable_with_patterns() {
    let r = dispatch("Fetch.enable", Some(json!({"patterns":[{"urlPattern":"*"}]})));
    assert_eq!(r.result.unwrap()["patternCount"], 1);
}

#[test]
fn test_fetch_disable() { assert!(ok_resp("Fetch.disable", None)); }

#[test]
fn test_fetch_continue_request() {
    let r = dispatch("Fetch.continueRequest", Some(json!({"requestId":"req-1"})));
    assert_eq!(r.result.unwrap()["requestId"], "req-1");
}

#[test]
fn test_fetch_continue_with_response() {
    let r = dispatch("Fetch.continueWithResponse", Some(json!({"requestId":"r2"})));
    assert_eq!(r.result.unwrap()["continued"], true);
}

#[test]
fn test_fetch_fail_request() {
    let r = dispatch("Fetch.failRequest", Some(json!({"requestId":"r3","reason":"Aborted"})));
    let result = r.result.unwrap();
    assert_eq!(result["failed"], true);
    assert_eq!(result["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request() {
    let r = dispatch("Fetch.fulfillRequest", Some(json!({"requestId":"r4","responseCode":200,"body":"hello"})));
    let result = r.result.unwrap();
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
}

#[test]
fn test_fetch_get_request_post_data() {
    let r = dispatch("Fetch.getRequestPostData", Some(json!({"requestId":"r5"})));
    assert_eq!(r.result.unwrap()["requestId"], "r5");
}

#[test]
fn test_fetch_continue_with_auth() {
    let r = dispatch("Fetch.continueWithAuth", Some(json!({"requestId":"r6"})));
    assert_eq!(r.result.unwrap()["requestId"], "r6");
}

#[test]
fn test_fetch_take_response_body() {
    let r = dispatch("Fetch.takeResponseBodyAsStream", Some(json!({"requestId":"r7"})));
    assert!(r.result.unwrap()["stream"].is_string());
}

#[test]
fn test_fetch_unknown() { assert_eq!(err_code("Fetch.nonexistent"), -32601); }

// ---- serialize_response / serialize_event helpers ----

#[test]
fn test_serialize_ok_response() {
    let resp = CDPResponse { id: 42, result: Some(json!({"ok":true})), error: None };
    let s = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["id"], 42);
    assert_eq!(parsed["result"]["ok"], true);
}

#[test]
fn test_serialize_error_response() {
    let resp = CDPResponse { id: 1, result: None, error: Some(bao_cdp::CDPError { code: -32601, message: "not found".into() }) };
    let s = serialize_response(&resp);
    assert!(s.contains("-32601"));
}

#[test]
fn test_serialize_event() {
    let ev = CDPEvent { method: "Page.load".into(), params: Some(json!({"ts":1})) };
    let s = serialize_event(&ev);
    assert!(s.contains("Page.load"));
}

// ---- Edge cases ----

#[test]
fn test_empty_domain() {
    assert_eq!(err_code(""), -32601);
}

#[test]
fn test_domain_only_no_command() {
    assert_eq!(err_code("Page"), -32601);
}

#[test]
fn test_response_id_matches_input() {
    let msg = CDPMessage { id: 999, method: "Page.enable".into(), params: None, session_id: None };
    let resp = handle_command(msg, "t", &None, None);
    assert_eq!(resp.id, 999);
}

#[test]
fn test_negative_id_preserved() {
    let msg = CDPMessage { id: -42, method: "Page.enable".into(), params: None, session_id: None };
    let resp = handle_command(msg, "t", &None, None);
    assert_eq!(resp.id, -42);
}
