// @trace TEST-CDP-017 [req:REQ-CDP-002,REQ-CDP-003,REQ-CDP-005,REQ-CDP-007] [level:unit]
// Domain handler command-level exhaustive tests: Runtime, Debugger, CSS, Overlay,
// Log, Fetch — all static-response commands covered with param variations.

use bao_cdp::{bridge_channel, BridgeReceiver};
use bao_cdp::domains::register_all_domains_into;
use cdp_server::{DomainRegistry, EventSender, CdpError};
use serde_json::{json, Value};
use std::time::Duration;

struct NopSender;
impl EventSender for NopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

fn setup_registry() -> (DomainRegistry, BridgeReceiver) {
    let registry = DomainRegistry::new();
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    register_all_domains_into(tx, &registry);
    (registry, rx)
}

fn dispatch(registry: &DomainRegistry, method: &str, params: Value) -> Result<Value, CdpError> {
    registry.dispatch_command(method, params, &NopSender)
        .unwrap_or_else(|| Err(CdpError { code: -32601, message: "domain not found".into() }))
}

// ---- Runtime Domain ----

#[test]
fn test_runtime_enable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.enable", json!({}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["executionContextId"], 1);
}

#[test]
fn test_runtime_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_runtime_evaluate_empty() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.evaluate", json!({"expression": ""}));
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r["result"]["type"], "undefined");
}

#[test]
fn test_runtime_call_function_on() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.callFunctionOn", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_runtime_get_properties() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.getProperties", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_runtime_release_object() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.releaseObject", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_runtime_release_object_group() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.releaseObjectGroup", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_runtime_compile_script() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.compileScript", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_runtime_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Runtime.nonexistent", json!({}));
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

// ---- Debugger Domain ----

#[test]
fn test_debugger_enable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.enable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.setBreakpointByUrl", json!({"lineNumber": 10, "url": "test.js"}));
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r["breakpointId"], "1");
}

#[test]
fn test_debugger_remove_breakpoint() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.removeBreakpoint", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_pause() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.pause", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_resume() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.resume", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_step_over() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.stepOver", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_step_into() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.stepInto", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_step_out() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.stepOut", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_set_skip_all_pauses() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.setSkipAllPauses", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_set_breakpoints_active() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.setBreakpointsActive", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.evaluateOnCallFrame", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.getPossibleBreakpoints", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_get_script_source() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.getScriptSource", json!({}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["scriptSource"], "");
}

#[test]
fn test_debugger_set_pause_on_exceptions() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.setPauseOnExceptions", json!({"state": "all"}));
    assert!(result.is_ok());
}

#[test]
fn test_debugger_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Debugger.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- CSS Domain ----

#[test]
fn test_css_enable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "CSS.enable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_css_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "CSS.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_css_get_computed_style() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "CSS.getComputedStyleForNode", json!({"nodeId": 1}));
    assert!(result.is_ok());
    assert!(result.unwrap()["computedStyle"].as_array().unwrap().is_empty());
}

#[test]
fn test_css_get_matched_styles() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "CSS.getMatchedStylesForNode", json!({"nodeId": 1}));
    assert!(result.is_ok());
}

#[test]
fn test_css_get_inline_styles() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "CSS.getInlineStylesForNode", json!({"nodeId": 1}));
    assert!(result.is_ok());
}

#[test]
fn test_css_set_style_texts() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "CSS.setStyleTexts", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_css_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "CSS.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- Overlay Domain ----

#[test]
fn test_overlay_enable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Overlay.enable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_overlay_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Overlay.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_overlay_highlight_node() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Overlay.highlightNode", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_overlay_hide_highlight() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Overlay.hideHighlight", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_overlay_set_inspect_mode() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Overlay.setInspectMode", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_overlay_set_paused_message() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Overlay.setPausedInDebuggerMessage", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_overlay_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Overlay.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- Log Domain ----

#[test]
fn test_log_enable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Log.enable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_log_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Log.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_log_clear() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Log.clear", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_log_start_violations_report() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Log.startViolationsReport", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_log_stop_violations_report() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Log.stopViolationsReport", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_log_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Log.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- Fetch Domain ----

#[test]
fn test_fetch_enable_no_patterns() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.enable", json!({}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["patternCount"], 0);
}

#[test]
fn test_fetch_enable_with_patterns() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["patternCount"], 1);
}

#[test]
fn test_fetch_enable_multiple_patterns() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.enable", json!({"patterns": [{"urlPattern": "*"}, {"urlPattern": "*.js"}]}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["patternCount"], 2);
}

#[test]
fn test_fetch_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_fetch_continue_request() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.continueRequest", json!({"requestId": "r-1"}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["requestId"], "r-1");
}

#[test]
fn test_fetch_continue_with_response() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.continueWithResponse", json!({"requestId": "r-2"}));
    assert!(result.is_ok());
}

#[test]
fn test_fetch_fail_request() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.failRequest", json!({"requestId": "r-3", "reason": "Aborted"}));
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r["requestId"], "r-3");
    assert_eq!(r["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.fulfillRequest", json!({
        "requestId": "r-4",
        "responseCode": 200,
        "body": "SGVsbG8="
    }));
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r["responseCode"], 200);
    assert_eq!(r["bodyLength"], 8); // base64 "SGVsbG8=" decoded length hint
}

#[test]
fn test_fetch_fulfill_request_custom_status() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.fulfillRequest", json!({
        "requestId": "r-5",
        "responseCode": 404,
        "body": ""
    }));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["responseCode"], 404);
}

#[test]
fn test_fetch_get_request_post_data() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.getRequestPostData", json!({"requestId": "r-6"}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["requestId"], "r-6");
}

#[test]
fn test_fetch_continue_with_auth() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.continueWithAuth", json!({"requestId": "r-7"}));
    assert!(result.is_ok());
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.takeResponseBodyAsStream", json!({"requestId": "r-8"}));
    assert!(result.is_ok());
    assert!(result.unwrap()["stream"].as_str().unwrap().contains("r-8"));
}

#[test]
fn test_fetch_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Fetch.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- Page Domain (static responses) ----

#[test]
fn test_page_enable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.enable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_page_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_page_set_content() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.setContent", json!({"html": "<h1>Test</h1>"}));
    assert!(result.is_ok());
}

#[test]
fn test_page_close() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.close", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_page_bring_to_front() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.bringToFront", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_page_get_layout_metrics() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.getLayoutMetrics", json!({}));
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r["contentSize"]["width"].as_f64().unwrap(), 1920.0);
    assert_eq!(r["contentSize"]["height"].as_f64().unwrap(), 1080.0);
}

#[test]
fn test_page_add_script_to_evaluate_no_backend() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.addScriptToEvaluateOnNewDocument", json!({"source": "console.log('hi')"}));
    // Bridge requires a real servo backend to respond; without one, returns error
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32603);
}

#[test]
fn test_page_add_script_empty_source() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.addScriptToEvaluateOnNewDocument", json!({"source": ""}));
    assert!(result.is_ok());
}

#[test]
fn test_page_remove_script() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.removeScriptToEvaluateOnNewDocument", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_page_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Page.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- DOM Domain (static responses) ----

#[test]
fn test_dom_enable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.enable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_describe_node() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.describeNode", json!({"nodeId": 1}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["node"]["nodeName"], "HTML");
}

#[test]
fn test_dom_query_selector_empty() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.querySelector", json!({}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_empty() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.querySelectorAll", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_get_box_model() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.getBoxModel", json!({"nodeId": 1}));
    assert!(result.is_ok());
    let model = result.unwrap()["model"].clone();
    // Without bridge responder, getBoundingClientRect returns fallback 0.0
    assert!(model["width"].is_number(), "model should have width field");
}

#[test]
fn test_dom_remove_attribute() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.removeAttribute", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_set_outer_html() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.setOuterHTML", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_insert_before() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.insertBefore", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_remove_node() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.removeNode", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_resolve_node() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.resolveNode", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_push_nodes_by_backend_ids() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.pushNodesByBackendIdsToFrontend", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_dom_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "DOM.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- Network Domain ----

#[test]
fn test_network_enable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Network.enable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_network_disable() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Network.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_network_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Network.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- Emulation Domain ----

#[test]
fn test_emulation_set_device_metrics_override_no_backend() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Emulation.setDeviceMetricsOverride", json!({"width": 1920, "height": 1080, "deviceScaleFactor": 1, "mobile": false}));
    // Bridge requires a real servo backend to respond; without one, returns error
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, -32603);
}

#[test]
fn test_emulation_clear_device_metrics_override() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Emulation.clearDeviceMetricsOverride", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_set_touch_emulation_enabled() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Emulation.setTouchEmulationEnabled", json!({"enabled": true}));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_set_user_agent_override_no_backend() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Emulation.setUserAgentOverride", json!({"userAgent": "Test"}));
    // Bridge requires a real servo backend to respond; without one, returns error
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, -32603);
}

#[test]
fn test_emulation_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Emulation.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- Input Domain (static dispatch) ----

#[test]
fn test_input_dispatch_touch_event() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Input.dispatchTouchEvent", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_input_insert_text_empty() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Input.insertText", json!({"text": ""}));
    assert!(result.is_ok());
}

#[test]
fn test_input_set_ignore_input_events() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Input.setIgnoreInputEvents", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_input_set_intercept_drags() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Input.setInterceptDrags", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_input_unknown_method() {
    let (reg, _) = setup_registry();
    let result = dispatch(&reg, "Input.nonexistent", json!({}));
    assert!(result.is_err());
}

// ---- Registry completeness ----

#[test]
fn test_registry_all_domains_registered() {
    let (reg, _) = setup_registry();
    for domain in ["Page", "Runtime", "DOM", "Network", "Debugger", "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch"] {
        assert!(reg.has_domain(domain), "Domain '{}' not registered", domain);
    }
}

#[test]
fn test_registry_11_domains() {
    let (reg, _) = setup_registry();
    let domains = ["Page", "Runtime", "DOM", "Network", "Debugger", "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch"];
    assert_eq!(domains.len(), 11);
    for d in &domains {
        assert!(reg.has_domain(d));
    }
}

// ---- Unknown domain ----

#[test]
fn test_unknown_domain_returns_none() {
    let (reg, _) = setup_registry();
    assert!(reg.dispatch_command("Unknown.method", json!({}), &NopSender).is_none());
}
