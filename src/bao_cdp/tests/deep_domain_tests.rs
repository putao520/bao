// @trace TEST-CDP-008 [req:REQ-CDP-007] [level:unit]
// @trace TEST-CDP-009 [req:REQ-CDP-003] [level:unit]
// @trace TEST-CDP-010 [req:REQ-CDP-006] [level:unit]
// Deep command coverage tests for all CDP domains.

use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeResponse};
use bao_cdp::domains::register_all_domains_into;
use bao_cdp::DomainRegistry;
use cdp_server::{EventSender, CdpError};
use serde_json::{json, Value};
use std::time::Duration;
use std::thread;

struct NoopEventSender;
impl EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

fn bridge_response(cmd: BridgeCommand) -> BridgeResponse {
    match cmd {
        BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Test")) },
        BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!("https://test.local")) },
        BridgeCommand::GetDocument => BridgeResponse {
            result: Ok(json!({
                "root": { "nodeId": 1, "nodeType": 9, "nodeName": "#document",
                           "localName": "", "nodeValue": "", "childNodeCount": 2 }
            })),
        },
        BridgeCommand::QuerySelector { .. } => BridgeResponse { result: Ok(json!({ "nodeId": 10 })) },
        BridgeCommand::QuerySelectorAll { .. } => BridgeResponse { result: Ok(json!({ "nodeIds": [10, 11, 12] })) },
        BridgeCommand::EvaluateJs { .. } => BridgeResponse {
            result: Ok(json!({ "result": { "type": "string", "value": "evaluated" } })),
        },
        BridgeCommand::TakeScreenshot { .. } => BridgeResponse { result: Ok(json!({ "data": "c2NyZWVuc2hvdA==" })) },
        BridgeCommand::GetOuterHtml { .. } => BridgeResponse { result: Ok(json!({ "outerHTML": "<html></html>" })) },
        BridgeCommand::SetAttributeValue { .. } => BridgeResponse { result: Ok(json!({})) },
        BridgeCommand::Navigate { .. } | BridgeCommand::Reload { .. } => BridgeResponse {
            result: Ok(json!({ "frameId": "0", "loaderId": "0" })),
        },
        BridgeCommand::SetViewport { .. } | BridgeCommand::SetUserAgent { .. } => BridgeResponse { result: Ok(json!({})) },
        BridgeCommand::DispatchMouseEvent { .. } | BridgeCommand::DispatchKeyEvent { .. } => BridgeResponse { result: Ok(json!({})) },
        BridgeCommand::InsertText { .. } => BridgeResponse { result: Ok(json!({})) },
        BridgeCommand::AddScriptToEvaluateOnNewDocument { .. } => BridgeResponse {
            result: Ok(json!({ "identifier": "1" })),
        },
        _ => BridgeResponse { result: Ok(json!({})) },
    }
}

fn setup() -> DomainRegistry {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let registry = DomainRegistry::new();
    register_all_domains_into(tx.clone(), &registry);
    thread::spawn(move || {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            let count = rx.drain(|cmd| bridge_response(cmd));
            if count == 0 {
                thread::sleep(Duration::from_millis(1));
            }
        }
    });
    std::mem::forget(tx);
    registry
}

fn dispatch(r: &DomainRegistry, method: &str, params: Value) -> Result<Value, CdpError> {
    let es = NoopEventSender;
    r.dispatch_command(method, params, &es)
        .ok_or_else(|| CdpError { code: -32601, message: format!("not found: {}", method) })
        .and_then(|r| r)
}

// ===========================================================================
// §1 CSS Domain — all commands (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_css_enable_disable() {
    let r = setup();
    assert!(dispatch(&r, "CSS.enable", json!({})).is_ok());
    assert!(dispatch(&r, "CSS.disable", json!({})).is_ok());
}

#[test]
fn test_css_get_computed_style() {
    let r = setup();
    let result = dispatch(&r, "CSS.getComputedStyleForNode", json!({"nodeId": 1})).unwrap();
    assert!(result["computedStyle"].is_array());
}

#[test]
fn test_css_get_matched_styles() {
    let r = setup();
    let result = dispatch(&r, "CSS.getMatchedStylesForNode", json!({"nodeId": 1})).unwrap();
    assert!(result["matchedCSSRules"].is_array());
    assert!(result["inlineStyle"].is_null());
    assert!(result["attributesStyle"].is_null());
}

#[test]
fn test_css_get_inline_styles() {
    let r = setup();
    let result = dispatch(&r, "CSS.getInlineStylesForNode", json!({"nodeId": 1})).unwrap();
    assert!(result["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    let r = setup();
    let result = dispatch(&r, "CSS.setStyleTexts", json!({"edits": []})).unwrap();
    assert!(result["styles"].is_array());
}

#[test]
fn test_css_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "CSS.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §2 Overlay Domain — all commands (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_overlay_enable_disable() {
    let r = setup();
    assert!(dispatch(&r, "Overlay.enable", json!({})).is_ok());
    assert!(dispatch(&r, "Overlay.disable", json!({})).is_ok());
}

#[test]
fn test_overlay_highlight_hide() {
    let r = setup();
    assert!(dispatch(&r, "Overlay.highlightNode", json!({"nodeId": 1})).is_ok());
    assert!(dispatch(&r, "Overlay.hideHighlight", json!({})).is_ok());
}

#[test]
fn test_overlay_inspect_mode() {
    let r = setup();
    assert!(dispatch(&r, "Overlay.setInspectMode", json!({"mode": "searchForNode"})).is_ok());
}

#[test]
fn test_overlay_paused_in_debugger() {
    let r = setup();
    assert!(dispatch(&r, "Overlay.setPausedInDebuggerMessage", json!({"message": "Paused"})).is_ok());
}

#[test]
fn test_overlay_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Overlay.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §3 Log Domain — all commands (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_log_enable_disable_clear() {
    let r = setup();
    assert!(dispatch(&r, "Log.enable", json!({})).is_ok());
    assert!(dispatch(&r, "Log.clear", json!({})).is_ok());
    assert!(dispatch(&r, "Log.disable", json!({})).is_ok());
}

#[test]
fn test_log_violations_report() {
    let r = setup();
    assert!(dispatch(&r, "Log.startViolationsReport", json!({"config": [{"name": "longTask"}]})).is_ok());
    assert!(dispatch(&r, "Log.stopViolationsReport", json!({})).is_ok());
}

#[test]
fn test_log_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Log.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §4 Fetch Domain — all commands (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_fetch_enable_with_patterns() {
    let r = setup();
    let result = dispatch(&r, "Fetch.enable", json!({
        "patterns": [{"urlPattern": "*"}, {"requestStage": "Response"}]
    })).unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 2);
}

#[test]
fn test_fetch_enable_no_patterns() {
    let r = setup();
    let result = dispatch(&r, "Fetch.enable", json!({})).unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 0);
}

#[test]
fn test_fetch_disable() {
    let r = setup();
    assert!(dispatch(&r, "Fetch.disable", json!({})).is_ok());
}

#[test]
fn test_fetch_continue_request() {
    let r = setup();
    let result = dispatch(&r, "Fetch.continueRequest", json!({"requestId": "req-1"})).unwrap();
    assert_eq!(result["requestId"], "req-1");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_continue_with_response() {
    let r = setup();
    let result = dispatch(&r, "Fetch.continueWithResponse", json!({"requestId": "req-2"})).unwrap();
    assert_eq!(result["requestId"], "req-2");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_fail_request() {
    let r = setup();
    let result = dispatch(&r, "Fetch.failRequest", json!({"requestId": "req-3", "reason": "TimedOut"})).unwrap();
    assert_eq!(result["failed"], true);
    assert_eq!(result["reason"], "TimedOut");
}

#[test]
fn test_fetch_fulfill_request() {
    let r = setup();
    let result = dispatch(&r, "Fetch.fulfillRequest", json!({
        "requestId": "req-4", "responseCode": 200, "body": "hello world"
    })).unwrap();
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 11);
}

#[test]
fn test_fetch_get_request_post_data() {
    let r = setup();
    let result = dispatch(&r, "Fetch.getRequestPostData", json!({"requestId": "req-5"})).unwrap();
    assert_eq!(result["requestId"], "req-5");
}

#[test]
fn test_fetch_continue_with_auth() {
    let r = setup();
    let result = dispatch(&r, "Fetch.continueWithAuth", json!({"requestId": "req-6"})).unwrap();
    assert_eq!(result["requestId"], "req-6");
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let r = setup();
    let result = dispatch(&r, "Fetch.takeResponseBodyAsStream", json!({"requestId": "req-7"})).unwrap();
    assert!(result["stream"].is_string());
    assert!(result["stream"].as_str().unwrap().contains("req-7"));
}

#[test]
fn test_fetch_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Fetch.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §5 Debugger Domain — full command coverage (REQ-CDP-003)
// ===========================================================================

#[test]
fn test_debugger_enable_disable() {
    let r = setup();
    assert!(dispatch(&r, "Debugger.enable", json!({})).is_ok());
    assert!(dispatch(&r, "Debugger.disable", json!({})).is_ok());
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let r = setup();
    let result = dispatch(&r, "Debugger.setBreakpointByUrl", json!({
        "lineNumber": 10, "url": "test.js"
    })).unwrap();
    assert_eq!(result["breakpointId"], "1");
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_remove_breakpoint() {
    let r = setup();
    assert!(dispatch(&r, "Debugger.removeBreakpoint", json!({"breakpointId": "1"})).is_ok());
}

#[test]
fn test_debugger_pause_resume() {
    let r = setup();
    assert!(dispatch(&r, "Debugger.pause", json!({})).is_ok());
    assert!(dispatch(&r, "Debugger.resume", json!({})).is_ok());
}

#[test]
fn test_debugger_stepping() {
    let r = setup();
    assert!(dispatch(&r, "Debugger.stepOver", json!({})).is_ok());
    assert!(dispatch(&r, "Debugger.stepInto", json!({})).is_ok());
    assert!(dispatch(&r, "Debugger.stepOut", json!({})).is_ok());
}

#[test]
fn test_debugger_skip_all_pauses() {
    let r = setup();
    assert!(dispatch(&r, "Debugger.setSkipAllPauses", json!({"skip": true})).is_ok());
}

#[test]
fn test_debugger_set_breakpoints_active() {
    let r = setup();
    assert!(dispatch(&r, "Debugger.setBreakpointsActive", json!({"active": true})).is_ok());
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let r = setup();
    let result = dispatch(&r, "Debugger.evaluateOnCallFrame", json!({
        "callFrameId": "cf-0", "expression": "1+1"
    })).unwrap();
    assert!(result["result"].is_object());
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let r = setup();
    let result = dispatch(&r, "Debugger.getPossibleBreakpoints", json!({})).unwrap();
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_get_script_source() {
    let r = setup();
    let result = dispatch(&r, "Debugger.getScriptSource", json!({"scriptId": "1"})).unwrap();
    assert!(result["scriptSource"].is_string());
}

#[test]
fn test_debugger_set_pause_on_exceptions() {
    let r = setup();
    assert!(dispatch(&r, "Debugger.setPauseOnExceptions", json!({"state": "all"})).is_ok());
}

#[test]
fn test_debugger_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Debugger.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §6 Network Domain — full command coverage (REQ-CDP-006)
// ===========================================================================

#[test]
fn test_network_enable_disable() {
    let r = setup();
    assert!(dispatch(&r, "Network.enable", json!({})).is_ok());
    assert!(dispatch(&r, "Network.disable", json!({})).is_ok());
}

#[test]
fn test_network_set_cache_disabled() {
    let r = setup();
    assert!(dispatch(&r, "Network.setCacheDisabled", json!({"cacheDisabled": true})).is_ok());
}

#[test]
fn test_network_get_cookies() {
    let r = setup();
    let result = dispatch(&r, "Network.getCookies", json!({})).unwrap();
    assert!(result["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies() {
    let r = setup();
    let result = dispatch(&r, "Network.getAllCookies", json!({})).unwrap();
    assert!(result["cookies"].is_array());
}

#[test]
fn test_network_set_extra_http_headers() {
    let r = setup();
    assert!(dispatch(&r, "Network.setExtraHTTPHeaders", json!({
        "headers": {"X-Test": "value"}
    })).is_ok());
}

#[test]
fn test_network_get_response_body() {
    let r = setup();
    let result = dispatch(&r, "Network.getResponseBody", json!({"requestId": "net-1"})).unwrap();
    assert_eq!(result["body"], "");
    assert_eq!(result["base64Encoded"], false);
}

#[test]
fn test_network_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Network.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §7 Page Domain — full command coverage (REQ-CDP-004)
// ===========================================================================

#[test]
fn test_page_enable_disable() {
    let r = setup();
    assert!(dispatch(&r, "Page.enable", json!({})).is_ok());
    assert!(dispatch(&r, "Page.disable", json!({})).is_ok());
}

#[test]
fn test_page_navigate() {
    let r = setup();
    let result = dispatch(&r, "Page.navigate", json!({"url": "https://example.com"})).unwrap();
    assert_eq!(result["frameId"], "0");
    assert!(result["loaderId"].is_string());
}

#[test]
fn test_page_navigate_default_url() {
    let r = setup();
    let result = dispatch(&r, "Page.navigate", json!({})).unwrap();
    assert_eq!(result["frameId"], "0");
}

#[test]
fn test_page_reload() {
    let r = setup();
    let result = dispatch(&r, "Page.reload", json!({})).unwrap();
    assert_eq!(result["frameId"], "0");
    assert_eq!(result["loaderId"], "0");
}

#[test]
fn test_page_reload_ignore_cache() {
    let r = setup();
    let result = dispatch(&r, "Page.reload", json!({"ignoreCache": true})).unwrap();
    assert_eq!(result["frameId"], "0");
}

#[test]
fn test_page_get_frame_tree() {
    let r = setup();
    let result = dispatch(&r, "Page.getFrameTree", json!({})).unwrap();
    assert!(result["frameTree"]["frame"].is_object());
    assert_eq!(result["frameTree"]["frame"]["mimeType"], "text/html");
}

#[test]
fn test_page_get_navigation_history() {
    let r = setup();
    let result = dispatch(&r, "Page.getNavigationHistory", json!({})).unwrap();
    assert_eq!(result["currentIndex"], 0);
    assert!(result["entries"].is_array());
}

#[test]
fn test_page_capture_screenshot() {
    let r = setup();
    let result = dispatch(&r, "Page.captureScreenshot", json!({})).unwrap();
    assert!(result["data"].is_string());
}

#[test]
fn test_page_set_content() {
    let r = setup();
    assert!(dispatch(&r, "Page.setContent", json!({"html": "<h1>Test</h1>"})).is_ok());
}

#[test]
fn test_page_close() {
    let r = setup();
    assert!(dispatch(&r, "Page.close", json!({})).is_ok());
}

#[test]
fn test_page_bring_to_front() {
    let r = setup();
    assert!(dispatch(&r, "Page.bringToFront", json!({})).is_ok());
}

#[test]
fn test_page_get_layout_metrics() {
    let r = setup();
    let result = dispatch(&r, "Page.getLayoutMetrics", json!({})).unwrap();
    assert_eq!(result["contentSize"]["width"], 1920);
    assert_eq!(result["contentSize"]["height"], 1080);
}

#[test]
fn test_page_add_script_to_evaluate_on_new_document() {
    let r = setup();
    let result = dispatch(&r, "Page.addScriptToEvaluateOnNewDocument", json!({
        "source": "console.log('injected')"
    })).unwrap();
    assert_eq!(result["identifier"], "1");
}

#[test]
fn test_page_add_script_empty_source() {
    let r = setup();
    let result = dispatch(&r, "Page.addScriptToEvaluateOnNewDocument", json!({})).unwrap();
    assert_eq!(result["identifier"], "1");
}

#[test]
fn test_page_remove_script() {
    let r = setup();
    assert!(dispatch(&r, "Page.removeScriptToEvaluateOnNewDocument", json!({"identifier": "1"})).is_ok());
}

#[test]
fn test_page_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Page.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §8 DOM Domain — full command coverage (REQ-CDP-005)
// ===========================================================================

#[test]
fn test_dom_get_document() {
    let r = setup();
    let result = dispatch(&r, "DOM.getDocument", json!({})).unwrap();
    assert!(result["root"].is_object());
    assert_eq!(result["root"]["nodeType"], 9);
}

#[test]
fn test_dom_query_selector() {
    let r = setup();
    let result = dispatch(&r, "DOM.querySelector", json!({"nodeId": 1, "selector": "div"})).unwrap();
    assert_eq!(result["nodeId"], 10);
}

#[test]
fn test_dom_query_selector_all() {
    let r = setup();
    let result = dispatch(&r, "DOM.querySelectorAll", json!({"nodeId": 1, "selector": "div"})).unwrap();
    assert_eq!(result["nodeIds"].as_array().unwrap().len(), 3);
}

#[test]
fn test_dom_describe_node() {
    let r = setup();
    let result = dispatch(&r, "DOM.describeNode", json!({})).unwrap();
    assert!(result["node"].is_object());
    assert_eq!(result["node"]["nodeName"], "HTML");
}

#[test]
fn test_dom_get_box_model() {
    let r = setup();
    let result = dispatch(&r, "DOM.getBoxModel", json!({})).unwrap();
    assert!(result["model"]["width"].is_number());
    assert!(result["model"]["height"].is_number());
}

#[test]
fn test_dom_set_attribute_value() {
    let r = setup();
    assert!(dispatch(&r, "DOM.setAttributeValue", json!({
        "nodeId": 1, "name": "class", "value": "active"
    })).is_ok());
}

#[test]
fn test_dom_get_outer_html() {
    let r = setup();
    let result = dispatch(&r, "DOM.getOuterHTML", json!({"nodeId": 1})).unwrap();
    assert!(result["outerHTML"].is_string());
}

#[test]
fn test_dom_resolve_node() {
    let r = setup();
    let result = dispatch(&r, "DOM.resolveNode", json!({})).unwrap();
    assert!(result["object"].is_object());
}

#[test]
fn test_dom_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "DOM.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §9 Runtime Domain — full command coverage (REQ-CDP-002)
// ===========================================================================

#[test]
fn test_runtime_evaluate() {
    let r = setup();
    let result = dispatch(&r, "Runtime.evaluate", json!({"expression": "1+1"})).unwrap();
    assert!(result["result"].is_object());
}

#[test]
fn test_runtime_call_function_on() {
    let r = setup();
    let result = dispatch(&r, "Runtime.callFunctionOn", json!({})).unwrap();
    assert!(result["result"].is_object());
}

#[test]
fn test_runtime_get_properties() {
    let r = setup();
    let result = dispatch(&r, "Runtime.getProperties", json!({})).unwrap();
    assert!(result["result"].is_array());
}

#[test]
fn test_runtime_enable_disable() {
    let r = setup();
    assert!(dispatch(&r, "Runtime.enable", json!({})).is_ok());
    assert!(dispatch(&r, "Runtime.disable", json!({})).is_ok());
}

#[test]
fn test_runtime_release_object() {
    let r = setup();
    assert!(dispatch(&r, "Runtime.releaseObject", json!({})).is_ok());
}

#[test]
fn test_runtime_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Runtime.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §10 Emulation Domain — full command coverage (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_emulation_set_device_metrics() {
    let r = setup();
    assert!(dispatch(&r, "Emulation.setDeviceMetricsOverride", json!({
        "width": 1920, "height": 1080, "deviceScaleFactor": 1.0
    })).is_ok());
}

#[test]
fn test_emulation_clear_device_metrics() {
    let r = setup();
    assert!(dispatch(&r, "Emulation.clearDeviceMetricsOverride", json!({})).is_ok());
}

#[test]
fn test_emulation_set_user_agent() {
    let r = setup();
    assert!(dispatch(&r, "Emulation.setUserAgentOverride", json!({
        "userAgent": "Mozilla/5.0 Test"
    })).is_ok());
}

#[test]
fn test_emulation_set_touch() {
    let r = setup();
    assert!(dispatch(&r, "Emulation.setTouchEmulationEnabled", json!({})).is_ok());
}

#[test]
fn test_emulation_script_execution_disabled() {
    let r = setup();
    assert!(dispatch(&r, "Emulation.setScriptExecutionDisabled", json!({"value": true})).is_ok());
}

#[test]
fn test_emulation_focus_emulation() {
    let r = setup();
    assert!(dispatch(&r, "Emulation.setFocusEmulationEnabled", json!({"enabled": true})).is_ok());
}

#[test]
fn test_emulation_cpu_throttling() {
    let r = setup();
    assert!(dispatch(&r, "Emulation.setCPUThrottlingRate", json!({"rate": 4.0})).is_ok());
}

#[test]
fn test_emulation_default_bg_color() {
    let r = setup();
    assert!(dispatch(&r, "Emulation.setDefaultBackgroundColorOverride", json!({})).is_ok());
}

#[test]
fn test_emulation_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Emulation.nonexistent", json!({})).unwrap_err().code, -32601);
}

// ===========================================================================
// §11 Input Domain — full command coverage (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_input_dispatch_mouse_press_release() {
    let r = setup();
    assert!(dispatch(&r, "Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": 100.0, "y": 200.0, "button": 0, "clickCount": 1
    })).is_ok());
    assert!(dispatch(&r, "Input.dispatchMouseEvent", json!({
        "type": "mouseReleased", "x": 100.0, "y": 200.0, "button": 0, "clickCount": 1
    })).is_ok());
}

#[test]
fn test_input_dispatch_mouse_move() {
    let r = setup();
    assert!(dispatch(&r, "Input.dispatchMouseEvent", json!({
        "type": "mouseMoved", "x": 150.0, "y": 250.0
    })).is_ok());
}

#[test]
fn test_input_dispatch_key_down_up() {
    let r = setup();
    assert!(dispatch(&r, "Input.dispatchKeyEvent", json!({
        "type": "keyDown", "key": "a", "code": "KeyA"
    })).is_ok());
    assert!(dispatch(&r, "Input.dispatchKeyEvent", json!({
        "type": "keyUp", "key": "a", "code": "KeyA"
    })).is_ok());
}

#[test]
fn test_input_dispatch_touch_event() {
    let r = setup();
    assert!(dispatch(&r, "Input.dispatchTouchEvent", json!({})).is_ok());
}

#[test]
fn test_input_insert_text() {
    let r = setup();
    assert!(dispatch(&r, "Input.insertText", json!({"text": "hello"})).is_ok());
}

#[test]
fn test_input_unknown_command() {
    let r = setup();
    assert_eq!(dispatch(&r, "Input.nonexistent", json!({})).unwrap_err().code, -32601);
}
