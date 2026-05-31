// @trace TEST-CDP-035 [req:REQ-CDP-001~008] [level:unit]
// Domain handler response field boundary verification:
// Every handler's known command returns correct JSON structure,
// unknown command error code -32601, empty/null/missing param handling.

use bao_cdp::CdpRouter;
use serde_json::json;

// ============================================================================
// Page handler: response field structure verification
// ============================================================================

#[test]
fn test_page_enable_response_structure() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Page.enable", None).unwrap(), json!({}));
}

#[test]
fn test_page_disable_response_structure() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Page.disable", None).unwrap(), json!({}));
}

#[test]
fn test_page_close_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Page.close", None).unwrap(), json!({}));
}

#[test]
fn test_page_bring_to_front_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Page.bringToFront", None).unwrap(), json!({}));
}

#[test]
fn test_page_set_content_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Page.setContent", None).unwrap(), json!({}));
}

#[test]
fn test_page_get_layout_metrics_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Page.getLayoutMetrics", None).unwrap();
    assert!(result.get("contentSize").is_some(), "missing contentSize");
    assert!(result["contentSize"]["width"].is_number());
    assert!(result["contentSize"]["height"].is_number());
    assert!(result.get("cssContentSize").is_some(), "missing cssContentSize");
}

#[test]
fn test_page_add_script_response_has_identifier() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Page.addScriptToEvaluateOnNewDocument", Some(json!({"source": "console.log(1)"}))).unwrap();
    assert!(result.get("identifier").is_some(), "missing identifier");
}

#[test]
fn test_page_add_script_empty_source() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Page.addScriptToEvaluateOnNewDocument", Some(json!({"source": ""}))).unwrap();
    assert!(result.get("identifier").is_some());
}

#[test]
fn test_page_remove_script_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Page.removeScriptToEvaluateOnNewDocument", None).unwrap(), json!({}));
}

#[test]
fn test_page_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Page.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("wasn't found"));
}

#[test]
fn test_page_navigate_has_frame_and_loader_id() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Page.navigate", Some(json!({"url": "http://example.com"}))).unwrap();
    assert!(result.get("frameId").is_some(), "missing frameId");
    assert!(result.get("loaderId").is_some(), "missing loaderId");
}

#[test]
fn test_page_navigate_empty_url_defaults() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "Page.navigate", Some(json!({}))).is_ok());
}

#[test]
fn test_page_reload_no_params() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "Page.reload", None).is_ok());
}

#[test]
fn test_page_reload_with_ignore_cache() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "Page.reload", Some(json!({"ignoreCache": true}))).is_ok());
}

// ============================================================================
// Runtime handler: response field structure verification
// ============================================================================

#[test]
fn test_runtime_enable_returns_execution_context() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Runtime.enable", None).unwrap();
    assert_eq!(result["executionContextId"], 1);
}

#[test]
fn test_runtime_disable_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Runtime.disable", None).unwrap(), json!({}));
}

#[test]
fn test_runtime_call_function_on_returns_undefined() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Runtime.callFunctionOn", None).unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_get_properties_returns_empty_array() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Runtime.getProperties", None).unwrap();
    assert_eq!(result["result"], json!([]));
}

#[test]
fn test_runtime_release_object_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Runtime.releaseObject", None).unwrap(), json!({}));
}

#[test]
fn test_runtime_release_object_group_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Runtime.releaseObjectGroup", None).unwrap(), json!({}));
}

#[test]
fn test_runtime_compile_script_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Runtime.compileScript", None).unwrap(), json!({}));
}

#[test]
fn test_runtime_evaluate_empty_expression() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Runtime.evaluate", Some(json!({"expression": ""}))).unwrap();
    assert_eq!(result["result"]["type"], "undefined");
    assert!(result.get("exceptionDetails").is_some());
}

#[test]
fn test_runtime_evaluate_no_expression_key() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Runtime.evaluate", Some(json!({}))).unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_run_script_returns_undefined() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Runtime.runScript", None).unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Runtime.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// DOM handler: response field structure verification
// ============================================================================

#[test]
fn test_dom_enable_disable_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "DOM.enable", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "DOM.disable", None).unwrap(), json!({}));
}

#[test]
fn test_dom_describe_node_has_node_object() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "DOM.describeNode", None).unwrap();
    assert!(result.get("node").is_some());
    assert_eq!(result["node"]["nodeName"], "HTML");
    assert_eq!(result["node"]["nodeType"], 1);
    assert!(result["node"]["nodeId"].is_number());
}

#[test]
fn test_dom_get_box_model_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "DOM.getBoxModel", None).unwrap();
    assert!(result.get("model").is_some());
    assert!(result["model"]["width"].is_number());
    assert!(result["model"]["height"].is_number());
    assert!(result["model"]["content"].is_array());
}

#[test]
fn test_dom_query_selector_empty_selector() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "DOM.querySelector", Some(json!({}))).unwrap();
    assert_eq!(result["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_empty_selector() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "DOM.querySelectorAll", Some(json!({}))).unwrap();
    assert_eq!(result["nodeIds"], json!([]));
}

#[test]
fn test_dom_resolve_node_response() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "DOM.resolveNode", None).unwrap();
    assert_eq!(result["object"]["type"], "node");
}

#[test]
fn test_dom_push_nodes_response() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "DOM.pushNodesByBackendIdsToFrontend", None).unwrap();
    assert_eq!(result["nodeIds"], json!([]));
}

#[test]
fn test_dom_remove_attribute_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "DOM.removeAttribute", None).unwrap(), json!({}));
}

#[test]
fn test_dom_set_outer_html_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "DOM.setOuterHTML", None).unwrap(), json!({}));
}

#[test]
fn test_dom_insert_before_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "DOM.insertBefore", None).unwrap(), json!({}));
}

#[test]
fn test_dom_remove_node_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "DOM.removeNode", None).unwrap(), json!({}));
}

#[test]
fn test_dom_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "DOM.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// Network handler: response field structure verification
// ============================================================================

#[test]
fn test_network_enable_disable_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Network.enable", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "Network.disable", None).unwrap(), json!({}));
}

#[test]
fn test_network_get_response_body_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Network.getResponseBody", None).unwrap();
    assert!(result.get("body").is_some());
    assert!(result.get("base64Encoded").is_some());
    assert_eq!(result["body"], "");
    assert_eq!(result["base64Encoded"], false);
}

#[test]
fn test_network_get_cookies_returns_cookies_array() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Network.getCookies", None).unwrap();
    assert!(result["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies_returns_cookies_array() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Network.getAllCookies", None).unwrap();
    assert!(result["cookies"].is_array());
}

#[test]
fn test_network_delete_cookies_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Network.deleteCookies", None).unwrap(), json!({}));
}

#[test]
fn test_network_set_cookie_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Network.setCookie", None).unwrap(), json!({}));
}

#[test]
fn test_network_set_cache_disabled_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Network.setCacheDisabled", None).unwrap(), json!({}));
}

#[test]
fn test_network_set_extra_http_headers_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Network.setExtraHTTPHeaders", None).unwrap(), json!({}));
}

#[test]
fn test_network_emulate_network_conditions_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Network.emulateNetworkConditions", None).unwrap(), json!({}));
}

#[test]
fn test_network_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Network.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// Debugger handler: response field structure verification
// ============================================================================

#[test]
fn test_debugger_enable_disable_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Debugger.enable", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "Debugger.disable", None).unwrap(), json!({}));
}

#[test]
fn test_debugger_set_breakpoint_by_url_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Debugger.setBreakpointByUrl", Some(json!({"lineNumber": 0}))).unwrap();
    assert!(result.get("breakpointId").is_some());
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_evaluate_on_call_frame_returns_undefined() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Debugger.evaluateOnCallFrame", None).unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_debugger_get_possible_breakpoints_returns_locations() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Debugger.getPossibleBreakpoints", None).unwrap();
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_get_script_source_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Debugger.getScriptSource", None).unwrap();
    assert!(result.get("scriptSource").is_some());
}

#[test]
fn test_debugger_step_commands_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Debugger.stepOver", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "Debugger.stepInto", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "Debugger.stepOut", None).unwrap(), json!({}));
}

#[test]
fn test_debugger_pause_resume_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Debugger.pause", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "Debugger.resume", None).unwrap(), json!({}));
}

#[test]
fn test_debugger_remove_breakpoint_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Debugger.removeBreakpoint", None).unwrap(), json!({}));
}

#[test]
fn test_debugger_set_skip_all_pauses_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Debugger.setSkipAllPauses", None).unwrap(), json!({}));
}

#[test]
fn test_debugger_set_breakpoints_active_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Debugger.setBreakpointsActive", None).unwrap(), json!({}));
}

#[test]
fn test_debugger_set_pause_on_exceptions_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Debugger.setPauseOnExceptions", None).unwrap(), json!({}));
}

#[test]
fn test_debugger_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Debugger.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// CSS handler: response field structure verification
// ============================================================================

#[test]
fn test_css_enable_disable_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "CSS.enable", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "CSS.disable", None).unwrap(), json!({}));
}

#[test]
fn test_css_get_computed_style_returns_array() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "CSS.getComputedStyleForNode", None).unwrap();
    assert!(result["computedStyle"].is_array());
}

#[test]
fn test_css_get_matched_styles_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "CSS.getMatchedStylesForNode", None).unwrap();
    assert!(result["matchedCSSRules"].is_array());
    assert!(result.get("inlineStyle").is_some());
    assert!(result.get("attributesStyle").is_some());
}

#[test]
fn test_css_get_inline_styles_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "CSS.getInlineStylesForNode", None).unwrap();
    assert!(result.get("inlineStyle").is_some());
}

#[test]
fn test_css_set_style_texts_returns_styles_array() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "CSS.setStyleTexts", None).unwrap();
    assert!(result["styles"].is_array());
}

#[test]
fn test_css_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "CSS.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// Overlay handler: response field structure verification
// ============================================================================

#[test]
fn test_overlay_enable_disable_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Overlay.enable", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "Overlay.disable", None).unwrap(), json!({}));
}

#[test]
fn test_overlay_highlight_node_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Overlay.highlightNode", None).unwrap(), json!({}));
}

#[test]
fn test_overlay_hide_highlight_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Overlay.hideHighlight", None).unwrap(), json!({}));
}

#[test]
fn test_overlay_set_inspect_mode_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Overlay.setInspectMode", None).unwrap(), json!({}));
}

#[test]
fn test_overlay_set_paused_in_debugger_message_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Overlay.setPausedInDebuggerMessage", None).unwrap(), json!({}));
}

#[test]
fn test_overlay_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Overlay.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// Log handler: response field structure verification
// ============================================================================

#[test]
fn test_log_enable_disable_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Log.enable", None).unwrap(), json!({}));
    assert_eq!(session.send(&router, "Log.disable", None).unwrap(), json!({}));
}

#[test]
fn test_log_clear_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Log.clear", None).unwrap(), json!({}));
}

#[test]
fn test_log_start_violations_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Log.startViolationsReport", None).unwrap(), json!({}));
}

#[test]
fn test_log_stop_violations_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Log.stopViolationsReport", None).unwrap(), json!({}));
}

#[test]
fn test_log_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Log.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// Fetch handler: response field structure verification
// ============================================================================

#[test]
fn test_fetch_enable_no_patterns() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.enable", None).unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 0);
}

#[test]
fn test_fetch_enable_with_patterns() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.enable", Some(json!({
        "patterns": [{"urlPattern": "*"}, {"urlPattern": "*.js"}]
    }))).unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 2);
}

#[test]
fn test_fetch_disable_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Fetch.disable", None).unwrap(), json!({}));
}

#[test]
fn test_fetch_continue_request_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.continueRequest", Some(json!({"requestId": "r1"}))).unwrap();
    assert_eq!(result["requestId"], "r1");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_continue_with_response_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.continueWithResponse", Some(json!({"requestId": "r2"}))).unwrap();
    assert_eq!(result["requestId"], "r2");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_fail_request_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.failRequest", Some(json!({"requestId": "r3", "reason": "Aborted"}))).unwrap();
    assert_eq!(result["requestId"], "r3");
    assert_eq!(result["failed"], true);
    assert_eq!(result["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.fulfillRequest", Some(json!({
        "requestId": "r4", "responseCode": 200, "body": "hello"
    }))).unwrap();
    assert_eq!(result["requestId"], "r4");
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 5);
}

#[test]
fn test_fetch_get_request_post_data_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.getRequestPostData", Some(json!({"requestId": "r5"}))).unwrap();
    assert_eq!(result["requestId"], "r5");
    assert!(result.get("postData").is_some());
}

#[test]
fn test_fetch_continue_with_auth_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.continueWithAuth", Some(json!({"requestId": "r6"}))).unwrap();
    assert_eq!(result["requestId"], "r6");
}

#[test]
fn test_fetch_take_response_body_as_stream_fields() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Fetch.takeResponseBodyAsStream", Some(json!({"requestId": "r7"}))).unwrap();
    assert!(result["stream"].as_str().unwrap().starts_with("stream-"));
}

#[test]
fn test_fetch_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Fetch.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// Emulation handler: response field structure verification
// ============================================================================

#[test]
fn test_emulation_clear_device_metrics_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Emulation.clearDeviceMetricsOverride", None).unwrap(), json!({}));
}

#[test]
fn test_emulation_set_touch_emulation_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Emulation.setTouchEmulationEnabled", None).unwrap(), json!({}));
}

#[test]
fn test_emulation_set_script_execution_disabled_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Emulation.setScriptExecutionDisabled", None).unwrap(), json!({}));
}

#[test]
fn test_emulation_set_focus_emulation_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Emulation.setFocusEmulationEnabled", None).unwrap(), json!({}));
}

#[test]
fn test_emulation_set_cpu_throttling_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Emulation.setCPUThrottlingRate", None).unwrap(), json!({}));
}

#[test]
fn test_emulation_set_default_bg_color_override_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Emulation.setDefaultBackgroundColorOverride", None).unwrap(), json!({}));
}

#[test]
fn test_emulation_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Emulation.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// Input handler: response structure verification
// ============================================================================

#[test]
fn test_input_dispatch_touch_event_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Input.dispatchTouchEvent", None).unwrap(), json!({}));
}

#[test]
fn test_input_set_ignore_input_events_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Input.setIgnoreInputEvents", None).unwrap(), json!({}));
}

#[test]
fn test_input_set_intercept_drags_response_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.send(&router, "Input.setInterceptDrags", None).unwrap(), json!({}));
}

#[test]
fn test_input_insert_text_empty_text() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Input.insertText", Some(json!({"text": ""}))).unwrap();
    assert_eq!(result, json!({}));
}

#[test]
fn test_input_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let err = session.send(&router, "Input.nonexistentMethod", None).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// Cross-domain: all 11 domains return -32601 for unknown commands
// ============================================================================

#[test]
fn test_all_domains_unknown_command_error_code() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");

    let domains = [
        "Page", "Runtime", "DOM", "Network", "Debugger",
        "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch",
    ];

    for domain in &domains {
        let cmd = format!("{}.nonexistentMethod12345", domain);
        let err = session.send(&router, &cmd, None).unwrap_err();
        assert_eq!(err.code, -32601, "domain {} should return -32601 for unknown command", domain);
        assert!(err.message.contains("wasn't found"), "domain {} error message format", domain);
    }
}

// ============================================================================
// Cross-domain: error message contains command name
// ============================================================================

#[test]
fn test_error_message_contains_command_name() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");

    let unknown_commands = [
        "Page.foo", "Runtime.bar", "DOM.baz", "Network.qux",
        "Debugger.xyz", "Input.abc", "Emulation.def", "CSS.ghi",
        "Overlay.jkl", "Log.mno", "Fetch.pqr",
    ];

    for cmd in &unknown_commands {
        let err = session.send(&router, cmd, None).unwrap_err();
        assert!(err.message.contains(cmd), "error message should contain '{}'", cmd);
    }
}
