// @trace TEST-CDP-019 [req:REQ-CDP-001,REQ-CDP-002,REQ-CDP-004,REQ-CDP-005] [level:unit]
// bao_cdp protocol.rs handle_command: all 11 domains without bridge,
// internal response validation, CDPMessage parse edge cases,
// BridgeReceiver try_process/drain, BackendKind enum.

use bao_cdp::{parse_message, handle_command, serialize_response, serialize_event, CDPMessage, CDPResponse, CDPError, CDPEvent};
use bao_cdp::{bridge_channel, BridgeCommand, BridgeResponse};
use serde_json::{json, Value};
use std::time::Duration;

// Helper: parse + handle without bridge
fn dispatch(raw: &str) -> CDPResponse {
    let msg = parse_message(raw).unwrap();
    handle_command(msg, "test-target", &None, None)
}

fn dispatch_with_params(method: &str, params: Value) -> CDPResponse {
    let p = Some(params);
    let msg = CDPMessage {
        id: 1,
        method: method.to_string(),
        params: None,
        session_id: None,
    };
    handle_command(msg, "test-target", &p, None)
}

// ---- Target domain ----

#[test]
fn test_target_get_targets() {
    let resp = dispatch(r#"{"id":1,"method":"Target.getTargets"}"#);
    assert!(resp.result.is_some());
    let result = resp.result.unwrap();
    assert!(result["targetInfos"].is_array());
    assert_eq!(result["targetInfos"][0]["targetId"], "test-target");
}

#[test]
fn test_target_get_target_info() {
    let resp = dispatch(r#"{"id":2,"method":"Target.getTargetInfo"}"#);
    let result = resp.result.unwrap();
    assert!(result["targetInfo"]["targetId"] == json!("test-target"));
}

#[test]
fn test_target_create_target() {
    let resp = dispatch(r#"{"id":3,"method":"Target.createTarget"}"#);
    let result = resp.result.unwrap();
    assert!(result["targetId"].is_string());
}

#[test]
fn test_target_close_target() {
    let resp = dispatch(r#"{"id":4,"method":"Target.closeTarget"}"#);
    assert_eq!(resp.result.unwrap()["success"], true);
}

#[test]
fn test_target_set_auto_attach() {
    let resp = dispatch(r#"{"id":5,"method":"Target.setAutoAttach"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_target_set_discover_targets() {
    let resp = dispatch(r#"{"id":6,"method":"Target.setDiscoverTargets"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_target_attach_to_target() {
    let resp = dispatch(r#"{"id":7,"method":"Target.attachToTarget"}"#);
    let result = resp.result.unwrap();
    assert!(result["sessionId"].is_string());
}

#[test]
fn test_target_detach_from_target() {
    let resp = dispatch(r#"{"id":8,"method":"Target.detachFromTarget"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_target_unknown() {
    let resp = dispatch(r#"{"id":9,"method":"Target.nonexistent"}"#);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

// ---- Page domain ----

#[test]
fn test_page_enable() {
    let resp = dispatch(r#"{"id":10,"method":"Page.enable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_page_disable() {
    let resp = dispatch(r#"{"id":11,"method":"Page.disable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_page_navigate_default_url() {
    let resp = dispatch(r#"{"id":12,"method":"Page.navigate"}"#);
    let result = resp.result.unwrap();
    assert!(result["frameId"].is_string());
}

#[test]
fn test_page_navigate_with_url() {
    let resp = dispatch_with_params("Page.navigate", json!({"url": "https://example.com"}));
    let result = resp.result.unwrap();
    assert!(result["frameId"].is_string());
}

#[test]
fn test_page_get_frame_tree() {
    let resp = dispatch(r#"{"id":13,"method":"Page.getFrameTree"}"#);
    let result = resp.result.unwrap();
    assert!(result["frameTree"]["frame"]["id"].is_string());
}

#[test]
fn test_page_get_navigation_history() {
    let resp = dispatch(r#"{"id":14,"method":"Page.getNavigationHistory"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["currentIndex"], 0);
}

#[test]
fn test_page_capture_screenshot_default() {
    let resp = dispatch(r#"{"id":15,"method":"Page.captureScreenshot"}"#);
    let result = resp.result.unwrap();
    assert!(result["data"].is_string());
}

#[test]
fn test_page_get_layout_metrics() {
    let resp = dispatch(r#"{"id":16,"method":"Page.getLayoutMetrics"}"#);
    let result = resp.result.unwrap();
    assert!(result["contentSize"]["width"].is_number());
}

#[test]
fn test_page_add_script() {
    let resp = dispatch_with_params("Page.addScriptToEvaluateOnNewDocument", json!({"source": ""}));
    let result = resp.result.unwrap();
    assert!(result["identifier"].is_string());
}

#[test]
fn test_page_set_content() {
    let resp = dispatch(r#"{"id":17,"method":"Page.setContent"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_page_close() {
    let resp = dispatch(r#"{"id":18,"method":"Page.close"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_page_bring_to_front() {
    let resp = dispatch(r#"{"id":19,"method":"Page.bringToFront"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_page_unknown() {
    let resp = dispatch(r#"{"id":20,"method":"Page.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Runtime domain ----

#[test]
fn test_runtime_enable() {
    let resp = dispatch(r#"{"id":30,"method":"Runtime.enable"}"#);
    let result = resp.result.unwrap();
    assert!(result["executionContextId"].is_number());
}

#[test]
fn test_runtime_disable() {
    let resp = dispatch(r#"{"id":31,"method":"Runtime.disable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_runtime_evaluate_no_expression() {
    let resp = dispatch(r#"{"id":32,"method":"Runtime.evaluate"}"#);
    let result = resp.result.unwrap();
    assert!(result["result"]["type"].is_string());
}

#[test]
fn test_runtime_call_function_on() {
    let resp = dispatch(r#"{"id":33,"method":"Runtime.callFunctionOn"}"#);
    assert!(resp.result.unwrap()["result"]["type"].is_string());
}

#[test]
fn test_runtime_get_properties() {
    let resp = dispatch(r#"{"id":34,"method":"Runtime.getProperties"}"#);
    assert!(resp.result.unwrap()["result"].is_array());
}

#[test]
fn test_runtime_release_object() {
    let resp = dispatch(r#"{"id":35,"method":"Runtime.releaseObject"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_runtime_unknown() {
    let resp = dispatch(r#"{"id":36,"method":"Runtime.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- DOM domain ----

#[test]
fn test_dom_enable() {
    let resp = dispatch(r#"{"id":40,"method":"DOM.enable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_dom_get_document_no_bridge() {
    let resp = dispatch(r#"{"id":41,"method":"DOM.getDocument"}"#);
    let result = resp.result.unwrap();
    assert!(result["root"]["nodeId"].is_number());
    assert_eq!(result["root"]["nodeName"], "#document");
}

#[test]
fn test_dom_describe_node() {
    let resp = dispatch(r#"{"id":42,"method":"DOM.describeNode"}"#);
    let result = resp.result.unwrap();
    assert!(result["node"]["nodeName"].is_string());
}

#[test]
fn test_dom_query_selector_no_bridge() {
    let resp = dispatch_with_params("DOM.querySelector", json!({"selector": ""}));
    assert_eq!(resp.result.unwrap()["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_no_bridge() {
    let resp = dispatch_with_params("DOM.querySelectorAll", json!({"selector": ""}));
    assert!(resp.result.unwrap()["nodeIds"].is_array());
}

#[test]
fn test_dom_get_box_model() {
    let resp = dispatch(r#"{"id":43,"method":"DOM.getBoxModel"}"#);
    let result = resp.result.unwrap();
    assert!(result["model"]["width"].is_number());
}

#[test]
fn test_dom_set_attribute_value_no_bridge() {
    let resp = dispatch_with_params("DOM.setAttributeValue", json!({"nodeId": 1, "name": "class", "value": "test"}));
    assert!(resp.result.is_some());
}

#[test]
fn test_dom_get_outer_html_no_bridge() {
    let resp = dispatch(r#"{"id":44,"method":"DOM.getOuterHTML"}"#);
    let result = resp.result.unwrap();
    assert!(result["outerHTML"].is_string());
}

#[test]
fn test_dom_resolve_node() {
    let resp = dispatch(r#"{"id":45,"method":"DOM.resolveNode"}"#);
    assert!(resp.result.unwrap()["object"]["type"].is_string());
}

#[test]
fn test_dom_remove_attribute() {
    let resp = dispatch(r#"{"id":46,"method":"DOM.removeAttribute"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_dom_unknown() {
    let resp = dispatch(r#"{"id":47,"method":"DOM.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Network domain ----

#[test]
fn test_network_enable() {
    let resp = dispatch(r#"{"id":50,"method":"Network.enable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_network_disable() {
    let resp = dispatch(r#"{"id":51,"method":"Network.disable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_network_get_response_body() {
    let resp = dispatch(r#"{"id":52,"method":"Network.getResponseBody"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["base64Encoded"], false);
}

#[test]
fn test_network_get_cookies() {
    let resp = dispatch(r#"{"id":53,"method":"Network.getCookies"}"#);
    assert!(resp.result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies() {
    let resp = dispatch(r#"{"id":54,"method":"Network.getAllCookies"}"#);
    assert!(resp.result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_set_cache_disabled() {
    let resp = dispatch(r#"{"id":55,"method":"Network.setCacheDisabled"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_network_unknown() {
    let resp = dispatch(r#"{"id":56,"method":"Network.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- CSS domain ----

#[test]
fn test_css_enable() {
    let resp = dispatch(r#"{"id":60,"method":"CSS.enable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_css_get_computed_style() {
    let resp = dispatch(r#"{"id":61,"method":"CSS.getComputedStyleForNode"}"#);
    assert!(resp.result.unwrap()["computedStyle"].is_array());
}

#[test]
fn test_css_get_matched_styles() {
    let resp = dispatch(r#"{"id":62,"method":"CSS.getMatchedStylesForNode"}"#);
    let result = resp.result.unwrap();
    assert!(result["matchedCSSRules"].is_array());
}

#[test]
fn test_css_get_inline_styles() {
    let resp = dispatch(r#"{"id":63,"method":"CSS.getInlineStylesForNode"}"#);
    assert!(resp.result.unwrap()["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    let resp = dispatch(r#"{"id":64,"method":"CSS.setStyleTexts"}"#);
    assert!(resp.result.unwrap()["styles"].is_array());
}

#[test]
fn test_css_unknown() {
    let resp = dispatch(r#"{"id":65,"method":"CSS.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Emulation domain ----

#[test]
fn test_emulation_set_device_metrics_no_bridge() {
    let resp = dispatch_with_params("Emulation.setDeviceMetricsOverride", json!({"width": 800, "height": 600}));
    assert!(resp.result.is_some());
}

#[test]
fn test_emulation_clear_device_metrics() {
    let resp = dispatch(r#"{"id":70,"method":"Emulation.clearDeviceMetricsOverride"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_emulation_set_user_agent_no_bridge_empty() {
    let resp = dispatch_with_params("Emulation.setUserAgentOverride", json!({"userAgent": ""}));
    assert!(resp.result.is_some());
}

#[test]
fn test_emulation_set_touch_emulation() {
    let resp = dispatch(r#"{"id":71,"method":"Emulation.setTouchEmulationEnabled"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_emulation_set_script_execution_disabled() {
    let resp = dispatch(r#"{"id":72,"method":"Emulation.setScriptExecutionDisabled"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_emulation_unknown() {
    let resp = dispatch(r#"{"id":73,"method":"Emulation.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Input domain ----

#[test]
fn test_input_dispatch_mouse_no_bridge() {
    let resp = dispatch_with_params("Input.dispatchMouseEvent", json!({"type": "mousePressed", "x": 0, "y": 0}));
    assert!(resp.result.is_some());
}

#[test]
fn test_input_dispatch_key_no_bridge() {
    let resp = dispatch_with_params("Input.dispatchKeyEvent", json!({"type": "keyDown", "key": "", "code": ""}));
    assert!(resp.result.is_some());
}

#[test]
fn test_input_dispatch_touch_event() {
    let resp = dispatch(r#"{"id":80,"method":"Input.dispatchTouchEvent"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_input_insert_text_no_bridge_empty() {
    let resp = dispatch_with_params("Input.insertText", json!({"text": ""}));
    assert!(resp.result.is_some());
}

#[test]
fn test_input_set_ignore_input_events() {
    let resp = dispatch(r#"{"id":81,"method":"Input.setIgnoreInputEvents"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_input_unknown() {
    let resp = dispatch(r#"{"id":82,"method":"Input.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Overlay domain ----

#[test]
fn test_overlay_enable() {
    let resp = dispatch(r#"{"id":90,"method":"Overlay.enable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_overlay_highlight_node() {
    let resp = dispatch(r#"{"id":91,"method":"Overlay.highlightNode"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_overlay_hide_highlight() {
    let resp = dispatch(r#"{"id":92,"method":"Overlay.hideHighlight"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_overlay_set_inspect_mode() {
    let resp = dispatch(r#"{"id":93,"method":"Overlay.setInspectMode"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_overlay_unknown() {
    let resp = dispatch(r#"{"id":94,"method":"Overlay.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Debugger domain ----

#[test]
fn test_debugger_enable() {
    let resp = dispatch(r#"{"id":100,"method":"Debugger.enable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let resp = dispatch(r#"{"id":101,"method":"Debugger.setBreakpointByUrl"}"#);
    let result = resp.result.unwrap();
    assert!(result["breakpointId"].is_string());
}

#[test]
fn test_debugger_pause() {
    let resp = dispatch(r#"{"id":102,"method":"Debugger.pause"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_debugger_resume() {
    let resp = dispatch(r#"{"id":103,"method":"Debugger.resume"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_debugger_step_over() {
    let resp = dispatch(r#"{"id":104,"method":"Debugger.stepOver"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let resp = dispatch(r#"{"id":105,"method":"Debugger.evaluateOnCallFrame"}"#);
    assert!(resp.result.unwrap()["result"]["type"].is_string());
}

#[test]
fn test_debugger_get_script_source() {
    let resp = dispatch(r#"{"id":106,"method":"Debugger.getScriptSource"}"#);
    assert!(resp.result.unwrap()["scriptSource"].is_string());
}

#[test]
fn test_debugger_unknown() {
    let resp = dispatch(r#"{"id":107,"method":"Debugger.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Log domain ----

#[test]
fn test_log_enable() {
    let resp = dispatch(r#"{"id":110,"method":"Log.enable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_log_disable() {
    let resp = dispatch(r#"{"id":111,"method":"Log.disable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_log_clear() {
    let resp = dispatch(r#"{"id":112,"method":"Log.clear"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_log_start_violations_report() {
    let resp = dispatch(r#"{"id":113,"method":"Log.startViolationsReport"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_log_unknown() {
    let resp = dispatch(r#"{"id":114,"method":"Log.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Fetch domain ----

#[test]
fn test_fetch_enable_no_patterns() {
    let resp = dispatch(r#"{"id":120,"method":"Fetch.enable"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["patternCount"], 0);
}

#[test]
fn test_fetch_enable_with_patterns() {
    let resp = dispatch_with_params("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}));
    let result = resp.result.unwrap();
    assert_eq!(result["patternCount"], 1);
}

#[test]
fn test_fetch_disable() {
    let resp = dispatch(r#"{"id":121,"method":"Fetch.disable"}"#);
    assert!(resp.result.is_some());
}

#[test]
fn test_fetch_continue_request() {
    let resp = dispatch_with_params("Fetch.continueRequest", json!({"requestId": "r-1"}));
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "r-1");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_fail_request() {
    let resp = dispatch_with_params("Fetch.failRequest", json!({"requestId": "r-2", "reason": "Aborted"}));
    let result = resp.result.unwrap();
    assert_eq!(result["failed"], true);
}

#[test]
fn test_fetch_fulfill_request() {
    let resp = dispatch_with_params("Fetch.fulfillRequest", json!({"requestId": "r-3", "responseCode": 200, "body": ""}));
    let result = resp.result.unwrap();
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
}

#[test]
fn test_fetch_get_request_post_data() {
    let resp = dispatch_with_params("Fetch.getRequestPostData", json!({"requestId": "r-4"}));
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "r-4");
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let resp = dispatch_with_params("Fetch.takeResponseBodyAsStream", json!({"requestId": "r-5"}));
    let result = resp.result.unwrap();
    assert!(result["stream"].is_string());
}

#[test]
fn test_fetch_unknown() {
    let resp = dispatch(r#"{"id":122,"method":"Fetch.nonexistent"}"#);
    assert!(resp.error.is_some());
}

// ---- Unknown domain ----

#[test]
fn test_unknown_domain() {
    let resp = dispatch(r#"{"id":200,"method":"Unknown.method"}"#);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn test_empty_domain() {
    let resp = dispatch(r#"{"id":201,"method":".method"}"#);
    assert!(resp.error.is_some());
}

// ---- CDPMessage parse ----

#[test]
fn test_parse_message_basic() {
    let msg = parse_message(r#"{"id":1,"method":"Page.navigate"}"#).unwrap();
    assert_eq!(msg.id, 1);
    assert_eq!(msg.method, "Page.navigate");
}

#[test]
fn test_parse_message_with_params() {
    let msg = parse_message(r#"{"id":1,"method":"Page.navigate","params":{"url":"https://test.com"}}"#).unwrap();
    assert_eq!(msg.params.unwrap()["url"], "https://test.com");
}

#[test]
fn test_parse_message_with_session() {
    let msg = parse_message(r#"{"id":1,"method":"Test.run","session_id":"sess-1"}"#).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("sess-1"));
}

#[test]
fn test_parse_message_no_params() {
    let msg = parse_message(r#"{"id":1,"method":"Page.enable"}"#).unwrap();
    assert!(msg.params.is_none());
}

#[test]
fn test_parse_message_invalid() {
    assert!(parse_message("{broken}").is_none());
    assert!(parse_message("").is_none());
    assert!(parse_message("null").is_none());
    assert!(parse_message("[]").is_none());
}

// ---- CDPResponse serialize ----

#[test]
fn test_serialize_response_success() {
    let resp = CDPResponse {
        id: 1,
        result: Some(json!({"ok": true})),
        error: None,
    };
    let json_str = serialize_response(&resp);
    assert!(json_str.contains("\"id\":1"));
    assert!(json_str.contains("\"ok\":true"));
    assert!(!json_str.contains("\"error\""));
}

#[test]
fn test_serialize_response_error() {
    let resp = CDPResponse {
        id: 2,
        result: None,
        error: Some(CDPError { code: -32601, message: "not found".into() }),
    };
    let json_str = serialize_response(&resp);
    assert!(json_str.contains("-32601"));
    assert!(!json_str.contains("\"result\""));
}

// ---- CDPEvent serialize ----

#[test]
fn test_serialize_event() {
    let ev = CDPEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"ts": 42})),
    };
    let json_str = serialize_event(&ev);
    assert!(json_str.contains("Page.loadEventFired"));
}

#[test]
fn test_serialize_event_no_params() {
    let ev = CDPEvent {
        method: "DOM.updated".into(),
        params: None,
    };
    let json_str = serialize_event(&ev);
    assert!(!json_str.contains("params"));
}

// ---- BridgeReceiver try_process / drain ----

#[test]
fn test_bridge_try_process() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    let processed = rx.try_process(|cmd| {
        let debug = format!("{:?}", cmd);
        assert!(debug.contains("GetTitle"));
        BridgeResponse { result: Ok(json!({"title": "Test"})) }
    });
    assert!(processed);
}

#[test]
fn test_bridge_try_process_empty() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let processed = rx.try_process(|_| BridgeResponse { result: Ok(json!({})) });
    assert!(!processed);
}

#[test]
fn test_bridge_drain_multiple() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx.send_fire_and_forget(BridgeCommand::GetUrl);
    tx.send_fire_and_forget(BridgeCommand::GetDocument);
    let count = rx.drain(|cmd| {
        let _ = format!("{:?}", cmd);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count, 3);
}

#[test]
fn test_bridge_drain_empty() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let count = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 0);
}

// ---- BridgeSender clone ----

#[test]
fn test_bridge_sender_clone() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let cloned = tx.clone();
    assert!(cloned.is_alive());
    drop(rx);
}

// ---- BridgeResponse edge cases ----

#[test]
fn test_bridge_response_ok_value() {
    let resp = BridgeResponse { result: Ok(json!({"x": 42})) };
    let val = resp.result.unwrap();
    assert_eq!(val["x"], 42);
}

#[test]
fn test_bridge_response_err_value() {
    let resp = BridgeResponse { result: Err("failed".into()) };
    assert_eq!(resp.result.unwrap_err(), "failed");
}

#[test]
fn test_bridge_response_ok_null() {
    let resp = BridgeResponse { result: Ok(Value::Null) };
    assert!(resp.result.unwrap().is_null());
}
