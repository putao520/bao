// @trace TEST-CDP-033 [req:REQ-CDP-001,REQ-CDP-002,REQ-CDP-003] [level:unit]
// InternalBackend + protocol.rs handle_command all 11 domains without bridge.
// Tests cover: every command path in handle_command with bridge=None,
// CDPMessage parse edge cases, serialize_response/serialize_event,
// InternalBackend send_command, CDPResponse/CDPError construction.

use bao_cdp::{CDPMessage, CDPResponse, CDPError, CDPEvent};
use bao_cdp::{parse_message, handle_command, serialize_response, serialize_event};
use serde_json::json;

// ---- CDPMessage parse edge cases ----

#[test]
fn test_parse_valid_message() {
    let msg = parse_message(r#"{"id":1,"method":"Page.enable","params":{}}"#).unwrap();
    assert_eq!(msg.id, 1);
    assert_eq!(msg.method, "Page.enable");
    assert!(msg.params.is_some());
    assert!(msg.session_id.is_none());
}

#[test]
fn test_parse_minimal_message() {
    let msg = parse_message(r#"{"id":0,"method":"Test"}"#).unwrap();
    assert_eq!(msg.id, 0);
    assert_eq!(msg.method, "Test");
    assert!(msg.params.is_none());
}

#[test]
fn test_parse_with_session_id() {
    // serde deserializes snake_case field names, so "sessionId" won't match
    // unless there's a #[serde(rename)]. Test with snake_case.
    let msg = parse_message(r#"{"id":5,"method":"Runtime.evaluate","session_id":"sess1"}"#).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("sess1"));
}

#[test]
fn test_parse_camel_session_id_not_matched() {
    // "sessionId" (camelCase) doesn't match the snake_case field without rename
    let msg = parse_message(r#"{"id":5,"method":"Runtime.evaluate","sessionId":"sess1"}"#).unwrap();
    assert!(msg.session_id.is_none());
}

#[test]
fn test_parse_invalid_json() {
    assert!(parse_message("not json").is_none());
}

#[test]
fn test_parse_empty_string() {
    assert!(parse_message("").is_none());
}

#[test]
fn test_parse_missing_method() {
    // method is required by struct definition
    assert!(parse_message(r#"{"id":1}"#).is_none());
}

#[test]
fn test_parse_negative_id() {
    let msg = parse_message(r#"{"id":-1,"method":"X"}"#).unwrap();
    assert_eq!(msg.id, -1);
}

#[test]
fn test_parse_large_id() {
    let msg = parse_message(r#"{"id":9999999999,"method":"X"}"#).unwrap();
    assert_eq!(msg.id, 9999999999);
}

#[test]
fn test_parse_string_id_fails() {
    // id must be i64, not string
    assert!(parse_message(r#"{"id":"abc","method":"X"}"#).is_none());
}

#[test]
fn test_parse_params_null() {
    let msg = parse_message(r#"{"id":1,"method":"X","params":null}"#).unwrap();
    // serde default on Option means null → None
    assert!(msg.params.is_none());
}

#[test]
fn test_parse_params_array() {
    let msg = parse_message(r#"{"id":1,"method":"X","params":[1,2]}"#).unwrap();
    assert!(msg.params.unwrap().is_array());
}

// ---- serialize_response ----

#[test]
fn test_serialize_response_ok() {
    let resp = CDPResponse { id: 1, result: Some(json!({"status": "ok"})), error: None };
    let s = serialize_response(&resp);
    assert!(s.contains(r#""id":1"#));
    assert!(s.contains(r#""status":"ok""#));
    assert!(!s.contains("error"));
}

#[test]
fn test_serialize_response_error() {
    let resp = CDPResponse { id: 2, result: None, error: Some(CDPError { code: -32601, message: "not found".into() }) };
    let s = serialize_response(&resp);
    assert!(s.contains(r#""id":2"#));
    assert!(s.contains("-32601"));
    assert!(s.contains("not found"));
    assert!(!s.contains("result"));
}

#[test]
fn test_serialize_response_empty_result() {
    let resp = CDPResponse { id: 3, result: Some(json!({})), error: None };
    let s = serialize_response(&resp);
    assert!(s.contains(r#""id":3"#));
}

#[test]
fn test_serialize_response_null_result() {
    let resp = CDPResponse { id: 4, result: Some(json!(null)), error: None };
    let s = serialize_response(&resp);
    assert!(s.contains(r#""id":4"#));
}

// ---- serialize_event ----

#[test]
fn test_serialize_event_with_params() {
    let ev = CDPEvent { method: "Page.loadEventFired".into(), params: Some(json!({"timestamp": 123})) };
    let s = serialize_event(&ev);
    assert!(s.contains("Page.loadEventFired"));
    assert!(s.contains("123"));
}

#[test]
fn test_serialize_event_no_params() {
    let ev = CDPEvent { method: "Runtime.executionContextCreated".into(), params: None };
    let s = serialize_event(&ev);
    assert!(s.contains("Runtime.executionContextCreated"));
    assert!(!s.contains("params"));
}

// ---- CDPError construction ----

#[test]
fn test_cdp_error_debug() {
    let err = CDPError { code: -32601, message: "test".into() };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32601"));
    assert!(debug.contains("test"));
}

#[test]
fn test_cdp_error_serialize() {
    let err = CDPError { code: -32700, message: "parse error".into() };
    let s = serde_json::to_string(&err).unwrap();
    assert!(s.contains("-32700"));
    assert!(s.contains("parse error"));
}

// ---- handle_command all domains without bridge ----

fn handle(method: &str) -> CDPResponse {
    let msg = CDPMessage { id: 42, method: method.into(), params: None, session_id: None };
    handle_command(msg, "t1", &None, None)
}

fn handle_params(method: &str, params: serde_json::Value) -> CDPResponse {
    let msg = CDPMessage { id: 42, method: method.into(), params: Some(params.clone()), session_id: None };
    handle_command(msg, "t1", &Some(params), None)
}

#[test]
fn test_handle_no_dot_in_method() {
    let resp = handle("NoDomain");
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn test_handle_empty_method() {
    let resp = handle("");
    assert!(resp.error.is_some());
}

// ---- Target domain ----

#[test]
fn test_target_get_targets() {
    let resp = handle("Target.getTargets");
    assert!(resp.result.is_some());
    let val = resp.result.unwrap();
    assert!(val["targetInfos"].is_array());
    assert_eq!(val["targetInfos"][0]["type"], "page");
}

#[test]
fn test_target_get_target_targets() {
    let resp = handle("Target.getTargetTargets");
    assert!(resp.result.is_some());
}

#[test]
fn test_target_create_target() {
    let resp = handle("Target.createTarget");
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["targetId"], "t1");
}

#[test]
fn test_target_close_target() {
    let resp = handle("Target.closeTarget");
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["success"], true);
}

#[test]
fn test_target_set_auto_attach() {
    let resp = handle("Target.setAutoAttach");
    assert!(resp.result.is_some());
}

#[test]
fn test_target_set_discover_targets() {
    let resp = handle("Target.setDiscoverTargets");
    assert!(resp.result.is_some());
}

#[test]
fn test_target_get_target_info() {
    let resp = handle("Target.getTargetInfo");
    assert!(resp.result.is_some());
    let val = resp.result.unwrap();
    assert!(val["targetInfo"]["type"] == "page");
}

#[test]
fn test_target_attach_to_target() {
    let resp = handle("Target.attachToTarget");
    assert!(resp.result.is_some());
    assert!(resp.result.unwrap()["sessionId"].is_string());
}

#[test]
fn test_target_detach() {
    let resp = handle("Target.detachFromTarget");
    assert!(resp.result.is_some());
}

#[test]
fn test_target_send_message() {
    let resp = handle("Target.sendMessageToTarget");
    assert!(resp.result.is_some());
}

#[test]
fn test_target_unknown() {
    let resp = handle("Target.nonexistent");
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

// ---- Page domain (no bridge) ----

#[test]
fn test_page_enable_disable() {
    assert!(handle("Page.enable").result.is_some());
    assert!(handle("Page.disable").result.is_some());
}

#[test]
fn test_page_navigate_default_url() {
    let resp = handle("Page.navigate");
    assert!(resp.result.is_some());
    let val = resp.result.unwrap();
    assert_eq!(val["frameId"], "0");
    assert!(val["loaderId"].is_string());
}

#[test]
fn test_page_navigate_with_url() {
    let resp = handle_params("Page.navigate", json!({"url": "https://example.com"}));
    assert!(resp.result.is_some());
}

#[test]
fn test_page_reload() {
    let resp = handle("Page.reload");
    assert!(resp.result.is_some());
}

#[test]
fn test_page_get_frame_tree() {
    let resp = handle("Page.getFrameTree");
    assert!(resp.result.is_some());
}

#[test]
fn test_page_get_navigation_history() {
    let resp = handle("Page.getNavigationHistory");
    assert!(resp.result.is_some());
    let val = resp.result.unwrap();
    assert_eq!(val["currentIndex"], 0);
}

#[test]
fn test_page_capture_screenshot_no_bridge() {
    let resp = handle("Page.captureScreenshot");
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["data"], "");
}

#[test]
fn test_page_set_content() {
    assert!(handle("Page.setContent").result.is_some());
}

#[test]
fn test_page_close() {
    assert!(handle("Page.close").result.is_some());
}

#[test]
fn test_page_bring_to_front() {
    assert!(handle("Page.bringToFront").result.is_some());
}

#[test]
fn test_page_get_layout_metrics() {
    let resp = handle("Page.getLayoutMetrics");
    assert!(resp.result.is_some());
    let val = resp.result.unwrap();
    assert_eq!(val["contentSize"]["width"], 1920);
}

#[test]
fn test_page_add_script_no_bridge() {
    let resp = handle("Page.addScriptToEvaluateOnNewDocument");
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["identifier"], "1");
}

#[test]
fn test_page_remove_script() {
    assert!(handle("Page.removeScriptToEvaluateOnNewDocument").result.is_some());
}

#[test]
fn test_page_unknown() {
    assert!(handle("Page.nonexistent").error.is_some());
}

// ---- Runtime domain (no bridge) ----

#[test]
fn test_runtime_enable() {
    let resp = handle("Runtime.enable");
    assert_eq!(resp.result.unwrap()["executionContextId"], 1);
}

#[test]
fn test_runtime_disable() {
    assert!(handle("Runtime.disable").result.is_some());
}

#[test]
fn test_runtime_evaluate_empty() {
    let resp = handle("Runtime.evaluate");
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["result"]["type"], "undefined");
}

#[test]
fn test_runtime_call_function_on() {
    let resp = handle("Runtime.callFunctionOn");
    assert!(resp.result.is_some());
}

#[test]
fn test_runtime_get_properties() {
    let resp = handle("Runtime.getProperties");
    assert!(resp.result.is_some());
    assert!(resp.result.unwrap()["result"].is_array());
}

#[test]
fn test_runtime_evaluate_async() {
    assert!(handle("Runtime.evaluateAsync").result.is_some());
}

#[test]
fn test_runtime_run_script() {
    assert!(handle("Runtime.runScript").result.is_some());
}

#[test]
fn test_runtime_release_object() {
    assert!(handle("Runtime.releaseObject").result.is_some());
}

#[test]
fn test_runtime_release_object_group() {
    assert!(handle("Runtime.releaseObjectGroup").result.is_some());
}

#[test]
fn test_runtime_compile_script() {
    assert!(handle("Runtime.compileScript").result.is_some());
}

#[test]
fn test_runtime_call_argument() {
    assert!(handle("Runtime.callArgument").result.is_some());
}

#[test]
fn test_runtime_unknown() {
    assert!(handle("Runtime.nonexistent").error.is_some());
}

// ---- DOM domain (no bridge) ----

#[test]
fn test_dom_enable_disable() {
    assert!(handle("DOM.enable").result.is_some());
    assert!(handle("DOM.disable").result.is_some());
}

#[test]
fn test_dom_get_document() {
    let resp = handle("DOM.getDocument");
    let val = resp.result.unwrap();
    assert_eq!(val["root"]["nodeName"], "#document");
    assert_eq!(val["root"]["nodeType"], 9);
}

#[test]
fn test_dom_describe_node() {
    let resp = handle("DOM.describeNode");
    assert_eq!(resp.result.unwrap()["node"]["nodeName"], "HTML");
}

#[test]
fn test_dom_query_selector_no_bridge() {
    let resp = handle("DOM.querySelector");
    assert_eq!(resp.result.unwrap()["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_no_bridge() {
    let resp = handle("DOM.querySelectorAll");
    assert!(resp.result.unwrap()["nodeIds"].is_array());
}

#[test]
fn test_dom_get_box_model() {
    let resp = handle("DOM.getBoxModel");
    assert_eq!(resp.result.unwrap()["model"]["width"], 1920);
}

#[test]
fn test_dom_set_attribute_value_no_bridge() {
    assert!(handle("DOM.setAttributeValue").result.is_some());
}

#[test]
fn test_dom_remove_attribute() {
    assert!(handle("DOM.removeAttribute").result.is_some());
}

#[test]
fn test_dom_set_outer_html() {
    assert!(handle("DOM.setOuterHTML").result.is_some());
}

#[test]
fn test_dom_insert_before() {
    assert!(handle("DOM.insertBefore").result.is_some());
}

#[test]
fn test_dom_remove_node() {
    assert!(handle("DOM.removeNode").result.is_some());
}

#[test]
fn test_dom_get_outer_html_no_bridge() {
    let resp = handle("DOM.getOuterHTML");
    assert_eq!(resp.result.unwrap()["outerHTML"], "<html><body></body></html>");
}

#[test]
fn test_dom_resolve_node() {
    assert_eq!(handle("DOM.resolveNode").result.unwrap()["object"]["type"], "node");
}

#[test]
fn test_dom_push_nodes() {
    assert!(handle("DOM.pushNodesByBackendIdsToFrontend").result.is_some());
}

#[test]
fn test_dom_unknown() {
    assert!(handle("DOM.nonexistent").error.is_some());
}

// ---- Network domain ----

#[test]
fn test_network_enable_disable() {
    assert!(handle("Network.enable").result.is_some());
    assert!(handle("Network.disable").result.is_some());
}

#[test]
fn test_network_get_response_body() {
    let resp = handle("Network.getResponseBody");
    assert_eq!(resp.result.unwrap()["base64Encoded"], false);
}

#[test]
fn test_network_set_cache_disabled() {
    assert!(handle("Network.setCacheDisabled").result.is_some());
}

#[test]
fn test_network_set_extra_http_headers() {
    assert!(handle("Network.setExtraHTTPHeaders").result.is_some());
}

#[test]
fn test_network_emulate_conditions() {
    assert!(handle("Network.emulateNetworkConditions").result.is_some());
}

#[test]
fn test_network_set_request_interception() {
    assert!(handle("Network.setRequestInterception").result.is_some());
}

#[test]
fn test_network_continue_intercepted() {
    assert!(handle("Network.continueInterceptedRequest").result.is_some());
}

#[test]
fn test_network_get_cookies() {
    assert!(handle("Network.getCookies").result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies() {
    assert!(handle("Network.getAllCookies").result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_delete_cookies() {
    assert!(handle("Network.deleteCookies").result.is_some());
}

#[test]
fn test_network_set_cookie() {
    assert!(handle("Network.setCookie").result.is_some());
}

#[test]
fn test_network_unknown() {
    assert!(handle("Network.nonexistent").error.is_some());
}

// ---- Emulation domain (no bridge) ----

#[test]
fn test_emulation_set_metrics_no_bridge() {
    assert!(handle("Emulation.setDeviceMetricsOverride").result.is_some());
}

#[test]
fn test_emulation_clear_metrics() {
    assert!(handle("Emulation.clearDeviceMetricsOverride").result.is_some());
}

#[test]
fn test_emulation_set_ua_no_bridge() {
    assert!(handle("Emulation.setUserAgentOverride").result.is_some());
}

#[test]
fn test_emulation_set_touch() {
    assert!(handle("Emulation.setTouchEmulationEnabled").result.is_some());
}

#[test]
fn test_emulation_set_script_disabled() {
    assert!(handle("Emulation.setScriptExecutionDisabled").result.is_some());
}

#[test]
fn test_emulation_set_focus() {
    assert!(handle("Emulation.setFocusEmulationEnabled").result.is_some());
}

#[test]
fn test_emulation_set_cpu_throttle() {
    assert!(handle("Emulation.setCPUThrottlingRate").result.is_some());
}

#[test]
fn test_emulation_set_default_bg() {
    assert!(handle("Emulation.setDefaultBackgroundColorOverride").result.is_some());
}

#[test]
fn test_emulation_unknown() {
    assert!(handle("Emulation.nonexistent").error.is_some());
}

// ---- Input domain (no bridge) ----

#[test]
fn test_input_dispatch_mouse_no_bridge() {
    assert!(handle("Input.dispatchMouseEvent").result.is_some());
}

#[test]
fn test_input_dispatch_key_no_bridge() {
    assert!(handle("Input.dispatchKeyEvent").result.is_some());
}

#[test]
fn test_input_dispatch_touch() {
    assert!(handle("Input.dispatchTouchEvent").result.is_some());
}

#[test]
fn test_input_insert_text_no_bridge() {
    assert!(handle("Input.insertText").result.is_some());
}

#[test]
fn test_input_set_ignore() {
    assert!(handle("Input.setIgnoreInputEvents").result.is_some());
}

#[test]
fn test_input_set_intercept_drags() {
    assert!(handle("Input.setInterceptDrags").result.is_some());
}

#[test]
fn test_input_unknown() {
    assert!(handle("Input.nonexistent").error.is_some());
}

// ---- Overlay domain ----

#[test]
fn test_overlay_enable_disable() {
    assert!(handle("Overlay.enable").result.is_some());
    assert!(handle("Overlay.disable").result.is_some());
}

#[test]
fn test_overlay_highlight() {
    assert!(handle("Overlay.highlightNode").result.is_some());
}

#[test]
fn test_overlay_hide_highlight() {
    assert!(handle("Overlay.hideHighlight").result.is_some());
}

#[test]
fn test_overlay_set_inspect_mode() {
    assert!(handle("Overlay.setInspectMode").result.is_some());
}

#[test]
fn test_overlay_set_paused_debugger() {
    assert!(handle("Overlay.setPausedInDebuggerMessage").result.is_some());
}

#[test]
fn test_overlay_unknown() {
    assert!(handle("Overlay.nonexistent").error.is_some());
}

// ---- Debugger domain ----

#[test]
fn test_debugger_enable_disable() {
    assert!(handle("Debugger.enable").result.is_some());
    assert!(handle("Debugger.disable").result.is_some());
}

#[test]
fn test_debugger_set_breakpoint() {
    let resp = handle("Debugger.setBreakpointByUrl");
    assert_eq!(resp.result.unwrap()["breakpointId"], "1");
}

#[test]
fn test_debugger_remove_breakpoint() {
    assert!(handle("Debugger.removeBreakpoint").result.is_some());
}

#[test]
fn test_debugger_pause_resume() {
    assert!(handle("Debugger.pause").result.is_some());
    assert!(handle("Debugger.resume").result.is_some());
}

#[test]
fn test_debugger_steps() {
    assert!(handle("Debugger.stepOver").result.is_some());
    assert!(handle("Debugger.stepInto").result.is_some());
    assert!(handle("Debugger.stepOut").result.is_some());
}

#[test]
fn test_debugger_skip_all_pauses() {
    assert!(handle("Debugger.setSkipAllPauses").result.is_some());
}

#[test]
fn test_debugger_set_breakpoints_active() {
    assert!(handle("Debugger.setBreakpointsActive").result.is_some());
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let resp = handle("Debugger.evaluateOnCallFrame");
    assert_eq!(resp.result.unwrap()["result"]["type"], "undefined");
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    assert!(handle("Debugger.getPossibleBreakpoints").result.unwrap()["locations"].is_array());
}

#[test]
fn test_debugger_get_script_source() {
    let resp = handle("Debugger.getScriptSource");
    assert!(resp.result.unwrap()["scriptSource"].is_string());
}

#[test]
fn test_debugger_set_pause_on_exceptions() {
    assert!(handle("Debugger.setPauseOnExceptions").result.is_some());
}

#[test]
fn test_debugger_unknown() {
    assert!(handle("Debugger.nonexistent").error.is_some());
}

// ---- Log domain ----

#[test]
fn test_log_enable_disable() {
    assert!(handle("Log.enable").result.is_some());
    assert!(handle("Log.disable").result.is_some());
}

#[test]
fn test_log_clear() {
    assert!(handle("Log.clear").result.is_some());
}

#[test]
fn test_log_start_violations() {
    assert!(handle("Log.startViolationsReport").result.is_some());
}

#[test]
fn test_log_stop_violations() {
    assert!(handle("Log.stopViolationsReport").result.is_some());
}

#[test]
fn test_log_unknown() {
    assert!(handle("Log.nonexistent").error.is_some());
}

// ---- Fetch domain ----

#[test]
fn test_fetch_enable() {
    let resp = handle("Fetch.enable");
    assert_eq!(resp.result.unwrap()["enabled"], true);
}

#[test]
fn test_fetch_enable_with_patterns() {
    let resp = handle_params("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}));
    assert_eq!(resp.result.unwrap()["patternCount"], 1);
}

#[test]
fn test_fetch_disable() {
    assert!(handle("Fetch.disable").result.is_some());
}

#[test]
fn test_fetch_continue_request() {
    let resp = handle_params("Fetch.continueRequest", json!({"requestId": "r1"}));
    assert_eq!(resp.result.unwrap()["requestId"], "r1");
}

#[test]
fn test_fetch_fail_request() {
    let resp = handle_params("Fetch.failRequest", json!({"requestId": "r2", "reason": "Aborted"}));
    let val = resp.result.unwrap();
    assert_eq!(val["requestId"], "r2");
    assert_eq!(val["failed"], true);
    assert_eq!(val["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request() {
    let resp = handle_params("Fetch.fulfillRequest", json!({"requestId": "r3", "responseCode": 200, "body": "hi"}));
    let val = resp.result.unwrap();
    assert_eq!(val["fulfilled"], true);
    assert_eq!(val["bodyLength"], 2);
}

#[test]
fn test_fetch_get_post_data() {
    let resp = handle_params("Fetch.getRequestPostData", json!({"requestId": "r4"}));
    assert_eq!(resp.result.unwrap()["requestId"], "r4");
}

#[test]
fn test_fetch_continue_with_auth() {
    let resp = handle_params("Fetch.continueWithAuth", json!({"requestId": "r5"}));
    assert_eq!(resp.result.unwrap()["requestId"], "r5");
}

#[test]
fn test_fetch_take_response_body() {
    let resp = handle_params("Fetch.takeResponseBodyAsStream", json!({"requestId": "r6"}));
    assert_eq!(resp.result.unwrap()["stream"], "stream-r6");
}

#[test]
fn test_fetch_continue_with_response() {
    let resp = handle_params("Fetch.continueWithResponse", json!({"requestId": "r7"}));
    assert_eq!(resp.result.unwrap()["requestId"], "r7");
}

#[test]
fn test_fetch_unknown() {
    assert!(handle("Fetch.nonexistent").error.is_some());
}

// ---- CSS domain ----

#[test]
fn test_css_enable_disable() {
    assert!(handle("CSS.enable").result.is_some());
    assert!(handle("CSS.disable").result.is_some());
}

#[test]
fn test_css_get_computed_style() {
    assert!(handle("CSS.getComputedStyleForNode").result.unwrap()["computedStyle"].is_array());
}

#[test]
fn test_css_get_matched_styles() {
    let val = handle("CSS.getMatchedStylesForNode").result.unwrap();
    assert!(val["matchedCSSRules"].is_array());
    assert!(val["inlineStyle"].is_null());
}

#[test]
fn test_css_get_inline_styles() {
    assert!(handle("CSS.getInlineStylesForNode").result.unwrap()["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    assert!(handle("CSS.setStyleTexts").result.unwrap()["styles"].is_array());
}

#[test]
fn test_css_unknown() {
    assert!(handle("CSS.nonexistent").error.is_some());
}

// ---- Response id propagation ----

#[test]
fn test_response_id_propagated() {
    let msg = CDPMessage { id: 12345, method: "Page.enable".into(), params: None, session_id: None };
    let resp = handle_command(msg, "t1", &None, None);
    assert_eq!(resp.id, 12345);
}

#[test]
fn test_response_id_negative() {
    let msg = CDPMessage { id: -999, method: "Runtime.enable".into(), params: None, session_id: None };
    let resp = handle_command(msg, "t1", &None, None);
    assert_eq!(resp.id, -999);
}
