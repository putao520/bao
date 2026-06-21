// @trace TEST-CDP-019 [req:REQ-CDP-001,REQ-CDP-002,REQ-CDP-004,REQ-CDP-005] [level:unit]
// bao_cdp protocol.rs handle_command: all 11 domains without bridge,
// internal response validation, CdpMessage parse edge cases,
// BridgeReceiver try_process/drain, BackendKind enum.

use bao_cdp::{parse_message, handle_command, serialize_response, serialize_event, CdpMessage, CdpResponse, CdpError, CdpEvent};
use bao_cdp::{bridge_channel, BridgeCommand, BridgeResponse};
use serde_json::{json, Value};
use std::time::Duration;

const TID: &str = "test-target";
// JSON-RPC 2.0 method-not-found error code (RFC 5.1).
const ERR_METHOD_NOT_FOUND: i64 = -32601;
// Error code returned for the no-bridge fallback path (internal error per bao_cdp).
const ERR_NO_BRIDGE: i64 = -32603;

// Helper: parse + handle without bridge
fn dispatch(raw: &str) -> CdpResponse {
    let msg = parse_message(raw).unwrap();
    handle_command(msg, "test-target", &None, None)
}

fn dispatch_with_params(method: &str, params: Value) -> CdpResponse {
    let p = Some(params);
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: None,
        session_id: None,
    };
    handle_command(msg, "test-target", &p, None)
}

// Adversarial helper: dispatch with explicit id + params + (optional) session_id.
fn dispatch_full(id: Option<i64>, method: &str, params: Option<Value>, session_id: Option<&str>) -> CdpResponse {
    let msg = CdpMessage {
        id,
        method: method.to_string(),
        params: None,
        session_id: session_id.map(|s| s.to_string()),
    };
    handle_command(msg, "test-target", &params, None)
}

// ---- Target domain ----

#[test]
fn test_target_get_targets() {
    let resp = dispatch(r#"{"id":1,"method":"Target.getTargets"}"#);
    assert_eq!(resp.id, Some(1));
    assert!(resp.error.is_none());
    assert!(resp.result.is_some());
    let result = resp.result.unwrap();
    assert!(result["targetInfos"].is_array());
    let infos = result["targetInfos"].as_array().unwrap();
    assert_eq!(infos.len(), 1);
    let info = &infos[0];
    // live_target_info() field contract — every field asserted.
    assert_eq!(info["targetId"], "test-target");
    assert_eq!(info["type"], "page");
    // Without a bridge, title/url fall back to the documented defaults.
    assert_eq!(info["title"], "Bao");
    assert_eq!(info["url"], "about:blank");
    assert_eq!(info["attached"], true);
}

#[test]
fn test_target_get_target_info() {
    let resp = dispatch(r#"{"id":2,"method":"Target.getTargetInfo"}"#);
    let result = resp.result.unwrap();
    let info = &result["targetInfo"];
    assert_eq!(info["targetId"], json!("test-target"));
    assert_eq!(info["type"], "page");
    assert_eq!(info["attached"], true);
    // getTargetInfo and getTargets must return the SAME live_target_info shape.
    let other = dispatch(r#"{"id":2,"method":"Target.getTargets"}"#).result.unwrap();
    assert_eq!(info, &other["targetInfos"][0]);
}

#[test]
fn test_target_create_target() {
    let resp = dispatch(r#"{"id":3,"method":"Target.createTarget"}"#);
    let result = resp.result.unwrap();
    // createTarget returns the SAME target_id it was given (deterministic), not a random id.
    assert_eq!(result["targetId"], "test-target");
}

#[test]
fn test_target_close_target() {
    let resp = dispatch(r#"{"id":4,"method":"Target.closeTarget"}"#);
    assert_eq!(resp.result.unwrap()["success"], true);
}

#[test]
fn test_target_set_auto_attach() {
    let resp = dispatch(r#"{"id":5,"method":"Target.setAutoAttach"}"#);
    let result = resp.result.unwrap();
    // ok_empty() ⇒ empty object result, not null/array.
    assert!(result.is_object());
    assert_eq!(result.as_object().unwrap().len(), 0);
}

#[test]
fn test_target_set_discover_targets() {
    let resp = dispatch(r#"{"id":6,"method":"Target.setDiscoverTargets"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
    assert_eq!(result.as_object().unwrap().len(), 0);
}

#[test]
fn test_target_attach_to_target() {
    let resp = dispatch(r#"{"id":7,"method":"Target.attachToTarget"}"#);
    let result = resp.result.unwrap();
    let sid = &result["sessionId"];
    assert!(sid.is_string());
    // sessionId is deterministic: format!("{:016x}", sum of char codes of target_id).
    // For "test-target" the sum is fixed ⇒ the same hex every run.
    let expected = format!("{:016x}", "test-target".chars().map(|c| c as u64).sum::<u64>());
    assert_eq!(sid.as_str().unwrap(), expected);
    assert_eq!(expected.len(), 16);
}

#[test]
fn test_target_detach_from_target() {
    let resp = dispatch(r#"{"id":8,"method":"Target.detachFromTarget"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_target_unknown() {
    let resp = dispatch(r#"{"id":9,"method":"Target.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    // Error message must echo the offending method name.
    assert!(err.message.contains("Target.nonexistent"));
    assert!(err.message.contains("wasn't found"));
    assert!(resp.result.is_none());
}

#[test]
fn test_target_get_target_targets_alias() {
    // "getTargetTargets" is an accepted alias of "getTargets" (same handler arm).
    let resp = dispatch(r#"{"id":999,"method":"Target.getTargetTargets"}"#);
    let result = resp.result.unwrap();
    assert!(result["targetInfos"].is_array());
    assert_eq!(result["targetInfos"][0]["targetId"], "test-target");
}

#[test]
fn test_target_send_message_to_target() {
    // "sendMessageToTarget" returns ok_empty (no servo routing without a bridge).
    let resp = dispatch(r#"{"id":998,"method":"Target.sendMessageToTarget"}"#);
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

// ---- Page domain ----

#[test]
fn test_page_enable() {
    let resp = dispatch(r#"{"id":10,"method":"Page.enable"}"#);
    assert_eq!(resp.id, Some(10));
    let result = resp.result.unwrap();
    assert!(result.is_object());
    assert_eq!(result.as_object().unwrap().len(), 0);
}

#[test]
fn test_page_disable() {
    let resp = dispatch(r#"{"id":11,"method":"Page.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
    assert_eq!(result.as_object().unwrap().len(), 0);
}

#[test]
fn test_page_navigate_default_url() {
    // No bridge ⇒ NavigateReturnObject built with default "about:blank".
    let resp = dispatch(r#"{"id":12,"method":"Page.navigate"}"#);
    let result = resp.result.unwrap();
    // frame_id is hardcoded "0" (NavigateReturnObjectBuilder).
    assert_eq!(result["frameId"], "0");
    // loader_id = format!("{:016x}", url.len()) — for "about:blank" len = 11.
    assert_eq!(result["loaderId"], format!("{:016x}", "about:blank".len()));
    assert_eq!(result["loaderId"].as_str().unwrap().len(), 16);
}

#[test]
fn test_page_navigate_with_url() {
    let resp = dispatch_with_params("Page.navigate", json!({"url": "https://example.com"}));
    let result = resp.result.unwrap();
    assert_eq!(result["frameId"], "0");
    // loader_id derived from url.len() — deterministic per url length.
    assert_eq!(result["loaderId"], format!("{:016x}", "https://example.com".len()));
}

#[test]
fn test_page_get_frame_tree() {
    let resp = dispatch(r#"{"id":13,"method":"Page.getFrameTree"}"#);
    let result = resp.result.unwrap();
    let frame = &result["frameTree"]["frame"];
    // Without bridge url defaults to "about:blank"; frame fields all asserted.
    assert_eq!(frame["id"], "0");
    assert_eq!(frame["url"], "about:blank");
    assert_eq!(frame["loaderId"], "0");
    assert_eq!(frame["mimeType"], "text/html");
}

#[test]
fn test_page_get_navigation_history() {
    let resp = dispatch(r#"{"id":14,"method":"Page.getNavigationHistory"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["currentIndex"], 0);
    // entries array must be non-empty (one entry with id 0).
    let entries = result["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], 0);
    assert_eq!(entries[0]["url"], "about:blank");
}

#[test]
fn test_page_capture_screenshot_default() {
    // No bridge ⇒ returns { "data": "" } (empty placeholder).
    let resp = dispatch(r#"{"id":15,"method":"Page.captureScreenshot"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["data"], "");
}

#[test]
fn test_page_get_layout_metrics() {
    let resp = dispatch(r#"{"id":16,"method":"Page.getLayoutMetrics"}"#);
    let result = resp.result.unwrap();
    // contentSize constants: 1920×1080 (hardcoded viewport defaults).
    assert_eq!(result["contentSize"]["width"], 1920);
    assert_eq!(result["contentSize"]["height"], 1080);
    assert_eq!(result["contentSize"]["x"], 0);
    assert_eq!(result["contentSize"]["y"], 0);
    // cssContentSize mirrors contentSize exactly.
    assert_eq!(result["cssContentSize"], result["contentSize"]);
}

#[test]
fn test_page_add_script() {
    // Empty source ⇒ identifier is always "1" (deterministic stub).
    let resp = dispatch_with_params("Page.addScriptToEvaluateOnNewDocument", json!({"source": ""}));
    let result = resp.result.unwrap();
    assert_eq!(result["identifier"], "1");
}

#[test]
fn test_page_remove_script() {
    // removeScriptToEvaluateOnNewDocument ⇒ ok_empty.
    let resp = dispatch_with_params("Page.removeScriptToEvaluateOnNewDocument", json!({"identifier": "1"}));
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_page_set_content() {
    let resp = dispatch(r#"{"id":17,"method":"Page.setContent"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_page_close() {
    let resp = dispatch(r#"{"id":18,"method":"Page.close"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_page_bring_to_front() {
    let resp = dispatch(r#"{"id":19,"method":"Page.bringToFront"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_page_reload_no_bridge() {
    // reload ⇒ { frameId: "0", loaderId: "0" } (distinct from navigate's len-derived loaderId).
    let resp = dispatch_with_params("Page.reload", json!({"ignoreCache": true}));
    let result = resp.result.unwrap();
    assert_eq!(result["frameId"], "0");
    assert_eq!(result["loaderId"], "0");
}

#[test]
fn test_page_unknown() {
    let resp = dispatch(r#"{"id":20,"method":"Page.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Page.nonexistent"));
}

// ---- Runtime domain ----

#[test]
fn test_runtime_enable() {
    let resp = dispatch(r#"{"id":30,"method":"Runtime.enable"}"#);
    let result = resp.result.unwrap();
    // executionContextId is hardcoded 1 (deterministic stub).
    assert_eq!(result["executionContextId"], 1);
}

#[test]
fn test_runtime_disable() {
    let resp = dispatch(r#"{"id":31,"method":"Runtime.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_runtime_evaluate_no_expression() {
    // No expression + no bridge ⇒ stub returns { result: { type: "undefined" }, exceptionDetails: null }.
    let resp = dispatch(r#"{"id":32,"method":"Runtime.evaluate"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
    // exceptionDetails must be present and null (no exception on the stub path).
    assert!(result.get("exceptionDetails").is_some());
    assert!(result["exceptionDetails"].is_null());
}

#[test]
fn test_runtime_call_function_on() {
    let resp = dispatch(r#"{"id":33,"method":"Runtime.callFunctionOn"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_get_properties() {
    let resp = dispatch(r#"{"id":34,"method":"Runtime.getProperties"}"#);
    let result = resp.result.unwrap();
    let arr = result["result"].as_array().unwrap();
    assert_eq!(arr.len(), 0);
}

#[test]
fn test_runtime_evaluate_async_and_run_script() {
    // Both evaluateAsync and runScript return the same { result: { type: "undefined" } } stub.
    let a = dispatch(r#"{"id":330,"method":"Runtime.evaluateAsync"}"#).result.unwrap();
    assert_eq!(a["result"]["type"], "undefined");
    let b = dispatch(r#"{"id":331,"method":"Runtime.runScript"}"#).result.unwrap();
    assert_eq!(b["result"]["type"], "undefined");
}

#[test]
fn test_runtime_release_object() {
    let resp = dispatch(r#"{"id":35,"method":"Runtime.releaseObject"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_runtime_release_object_group_and_compile() {
    // releaseObjectGroup, compileScript, callArgument ⇒ ok_empty.
    for m in &["releaseObjectGroup", "compileScript", "callArgument"] {
        let resp = dispatch(&format!(r#"{{"id":1,"method":"Runtime.{}"}}"#, m));
        let result = resp.result.unwrap();
        assert!(result.is_object(), "Runtime.{} should return ok_empty", m);
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_runtime_unknown() {
    let resp = dispatch(r#"{"id":36,"method":"Runtime.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Runtime.nonexistent"));
}

// ---- DOM domain ----

#[test]
fn test_dom_enable() {
    let resp = dispatch(r#"{"id":40,"method":"DOM.enable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_dom_disable() {
    let resp = dispatch(r#"{"id":401,"method":"DOM.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_dom_get_document_no_bridge() {
    let resp = dispatch(r#"{"id":41,"method":"DOM.getDocument"}"#);
    let result = resp.result.unwrap();
    let root = &result["root"];
    // Full stub document tree contract — every field asserted.
    assert_eq!(root["nodeId"], 1);
    assert_eq!(root["backendNodeId"], 1);
    assert_eq!(root["nodeType"], 9); // DOCUMENT_NODE
    assert_eq!(root["nodeName"], "#document");
    assert_eq!(root["localName"], "");
    assert_eq!(root["nodeValue"], "");
    assert_eq!(root["childNodeCount"], 1);
    let children = root["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);
    let html = &children[0];
    assert_eq!(html["nodeId"], 2);
    assert_eq!(html["backendNodeId"], 2);
    assert_eq!(html["nodeType"], 1); // ELEMENT_NODE
    assert_eq!(html["nodeName"], "HTML");
    assert_eq!(html["localName"], "html");
    assert_eq!(html["childNodeCount"], 2);
}

#[test]
fn test_dom_describe_node() {
    let resp = dispatch(r#"{"id":42,"method":"DOM.describeNode"}"#);
    let result = resp.result.unwrap();
    let node = &result["node"];
    assert_eq!(node["nodeId"], 1);
    assert_eq!(node["nodeType"], 1);
    assert_eq!(node["nodeName"], "HTML");
}

#[test]
fn test_dom_query_selector_no_bridge() {
    // Empty selector + no bridge ⇒ nodeId 0 (not found).
    let resp = dispatch_with_params("DOM.querySelector", json!({"selector": ""}));
    assert_eq!(resp.result.unwrap()["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_no_bridge() {
    // Empty selector + no bridge ⇒ empty nodeIds array.
    let resp = dispatch_with_params("DOM.querySelectorAll", json!({"selector": ""}));
    let result = resp.result.unwrap();
    let arr = result["nodeIds"].as_array().unwrap();
    assert_eq!(arr.len(), 0);
}

#[test]
fn test_dom_get_box_model() {
    let resp = dispatch(r#"{"id":43,"method":"DOM.getBoxModel"}"#);
    let result = resp.result.unwrap();
    let model = &result["model"];
    // 1920×1080 viewport with 8-element content quad (clockwise from origin).
    assert_eq!(model["width"], 1920);
    assert_eq!(model["height"], 1080);
    let content = model["content"].as_array().unwrap();
    assert_eq!(content.len(), 8);
    assert_eq!(content[0], 0);
    assert_eq!(content[1], 0);
    assert_eq!(content[2], 1920);
    assert_eq!(content[6], 0);
    assert_eq!(content[7], 1080);
}

#[test]
fn test_dom_set_attribute_value_no_bridge() {
    // Without bridge, setAttributeValue returns ok_empty (no servo routing).
    let resp = dispatch_with_params("DOM.setAttributeValue", json!({"nodeId": 1, "name": "class", "value": "test"}));
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_dom_get_outer_html_no_bridge() {
    let resp = dispatch(r#"{"id":44,"method":"DOM.getOuterHTML"}"#);
    let result = resp.result.unwrap();
    // Exact stub HTML (not just "is_string").
    assert_eq!(result["outerHTML"], "<html><body></body></html>");
}

#[test]
fn test_dom_resolve_node() {
    let resp = dispatch(r#"{"id":45,"method":"DOM.resolveNode"}"#);
    let result = resp.result.unwrap();
    // RemoteObject type is "node", not arbitrary string.
    assert_eq!(result["object"]["type"], "node");
}

#[test]
fn test_dom_push_nodes_by_backend_ids() {
    let resp = dispatch(r#"{"id":451,"method":"DOM.pushNodesByBackendIdsToFrontend"}"#);
    let result = resp.result.unwrap();
    let arr = result["nodeIds"].as_array().unwrap();
    assert_eq!(arr.len(), 0);
}

#[test]
fn test_dom_set_outer_html_insert_before_remove_node() {
    // All three route to ok_empty.
    for m in &["setOuterHTML", "insertBefore", "removeNode"] {
        let resp = dispatch(&format!(r#"{{"id":1,"method":"DOM.{}"}}"#, m));
        assert!(resp.result.unwrap().is_object());
    }
}

#[test]
fn test_dom_remove_attribute() {
    let resp = dispatch(r#"{"id":46,"method":"DOM.removeAttribute"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_dom_unknown() {
    let resp = dispatch(r#"{"id":47,"method":"DOM.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("DOM.nonexistent"));
}

// ---- Network domain ----

#[test]
fn test_network_enable() {
    let resp = dispatch(r#"{"id":50,"method":"Network.enable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_network_disable() {
    let resp = dispatch(r#"{"id":51,"method":"Network.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_network_get_response_body() {
    let resp = dispatch(r#"{"id":52,"method":"Network.getResponseBody"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["base64Encoded"], false);
    // body defaults to "" (empty string), not absent.
    assert_eq!(result["body"], "");
}

#[test]
fn test_network_get_cookies() {
    let resp = dispatch(r#"{"id":53,"method":"Network.getCookies"}"#);
    let result = resp.result.unwrap();
    let arr = result["cookies"].as_array().unwrap();
    assert_eq!(arr.len(), 0);
}

#[test]
fn test_network_get_all_cookies() {
    let resp = dispatch(r#"{"id":54,"method":"Network.getAllCookies"}"#);
    let result = resp.result.unwrap();
    let arr = result["cookies"].as_array().unwrap();
    assert_eq!(arr.len(), 0);
}

#[test]
fn test_network_set_cache_disabled() {
    let resp = dispatch(r#"{"id":55,"method":"Network.setCacheDisabled"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_network_misc_ok_empty() {
    // emulateNetworkConditions, setRequestInterception, continueInterceptedRequest,
    // deleteCookies, setCookie, setExtraHTTPHeaders ⇒ ok_empty.
    for m in &[
        "emulateNetworkConditions",
        "setRequestInterception",
        "continueInterceptedRequest",
        "deleteCookies",
        "setCookie",
        "setExtraHTTPHeaders",
    ] {
        let resp = dispatch(&format!(r#"{{"id":1,"method":"Network.{}"}}"#, m));
        assert!(resp.result.unwrap().is_object(), "Network.{} ok_empty", m);
    }
}

#[test]
fn test_network_unknown() {
    let resp = dispatch(r#"{"id":56,"method":"Network.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Network.nonexistent"));
}

// ---- CSS domain ----

#[test]
fn test_css_enable() {
    let resp = dispatch(r#"{"id":60,"method":"CSS.enable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_css_disable() {
    let resp = dispatch(r#"{"id":601,"method":"CSS.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_css_get_computed_style() {
    let resp = dispatch(r#"{"id":61,"method":"CSS.getComputedStyleForNode"}"#);
    let result = resp.result.unwrap();
    let arr = result["computedStyle"].as_array().unwrap();
    assert_eq!(arr.len(), 0);
}

#[test]
fn test_css_get_matched_styles() {
    let resp = dispatch(r#"{"id":62,"method":"CSS.getMatchedStylesForNode"}"#);
    let result = resp.result.unwrap();
    let rules = result["matchedCSSRules"].as_array().unwrap();
    assert_eq!(rules.len(), 0);
    // All three style fields must be present; inlineStyle + attributesStyle are null.
    assert!(result.get("inlineStyle").is_some());
    assert!(result["inlineStyle"].is_null());
    assert!(result.get("attributesStyle").is_some());
    assert!(result["attributesStyle"].is_null());
}

#[test]
fn test_css_get_inline_styles() {
    let resp = dispatch(r#"{"id":63,"method":"CSS.getInlineStylesForNode"}"#);
    let result = resp.result.unwrap();
    assert!(result["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    let resp = dispatch(r#"{"id":64,"method":"CSS.setStyleTexts"}"#);
    let result = resp.result.unwrap();
    let arr = result["styles"].as_array().unwrap();
    assert_eq!(arr.len(), 0);
}

#[test]
fn test_css_unknown() {
    let resp = dispatch(r#"{"id":65,"method":"CSS.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("CSS.nonexistent"));
}

// ---- Emulation domain ----

#[test]
fn test_emulation_set_device_metrics_no_bridge() {
    // Without bridge ⇒ ok_empty (no servo routing); params parsed but ignored on the no-bridge path.
    let resp = dispatch_with_params("Emulation.setDeviceMetricsOverride", json!({"width": 800, "height": 600}));
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_emulation_clear_device_metrics() {
    let resp = dispatch(r#"{"id":70,"method":"Emulation.clearDeviceMetricsOverride"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_emulation_set_user_agent_no_bridge_empty() {
    // Empty userAgent ⇒ no bridge_send (would skip), ok_empty returned.
    let resp = dispatch_with_params("Emulation.setUserAgentOverride", json!({"userAgent": ""}));
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_emulation_set_touch_emulation() {
    let resp = dispatch(r#"{"id":71,"method":"Emulation.setTouchEmulationEnabled"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_emulation_set_script_execution_disabled() {
    let resp = dispatch(r#"{"id":72,"method":"Emulation.setScriptExecutionDisabled"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_emulation_misc_ok_empty() {
    // setFocusEmulationEnabled, setCPUThrottlingRate, setDefaultBackgroundColorOverride ⇒ ok_empty.
    for m in &[
        "setFocusEmulationEnabled",
        "setCPUThrottlingRate",
        "setDefaultBackgroundColorOverride",
    ] {
        let resp = dispatch(&format!(r#"{{"id":1,"method":"Emulation.{}"}}"#, m));
        assert!(resp.result.unwrap().is_object());
    }
}

#[test]
fn test_emulation_unknown() {
    let resp = dispatch(r#"{"id":73,"method":"Emulation.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Emulation.nonexistent"));
}

// ---- Input domain ----

#[test]
fn test_input_dispatch_mouse_no_bridge() {
    // Without bridge ⇒ ok_empty despite full params parsed.
    let resp = dispatch_with_params("Input.dispatchMouseEvent", json!({"type": "mousePressed", "x": 0, "y": 0}));
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_input_dispatch_key_no_bridge() {
    let resp = dispatch_with_params("Input.dispatchKeyEvent", json!({"type": "keyDown", "key": "", "code": ""}));
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_input_dispatch_touch_event() {
    let resp = dispatch(r#"{"id":80,"method":"Input.dispatchTouchEvent"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_input_insert_text_no_bridge_empty() {
    // Empty text ⇒ no bridge_send, ok_empty returned.
    let resp = dispatch_with_params("Input.insertText", json!({"text": ""}));
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_input_set_ignore_input_events() {
    let resp = dispatch(r#"{"id":81,"method":"Input.setIgnoreInputEvents"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_input_set_intercept_drags() {
    let resp = dispatch(r#"{"id":811,"method":"Input.setInterceptDrags"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_input_unknown() {
    let resp = dispatch(r#"{"id":82,"method":"Input.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Input.nonexistent"));
}

// ---- Overlay domain ----

#[test]
fn test_overlay_enable() {
    let resp = dispatch(r#"{"id":90,"method":"Overlay.enable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_overlay_disable() {
    let resp = dispatch(r#"{"id":901,"method":"Overlay.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_overlay_highlight_node() {
    let resp = dispatch(r#"{"id":91,"method":"Overlay.highlightNode"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_overlay_hide_highlight() {
    let resp = dispatch(r#"{"id":92,"method":"Overlay.hideHighlight"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_overlay_set_inspect_mode() {
    let resp = dispatch(r#"{"id":93,"method":"Overlay.setInspectMode"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_overlay_set_paused_in_debugger_message() {
    let resp = dispatch(r#"{"id":931,"method":"Overlay.setPausedInDebuggerMessage"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_overlay_unknown() {
    let resp = dispatch(r#"{"id":94,"method":"Overlay.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Overlay.nonexistent"));
}

// ---- Debugger domain ----

#[test]
fn test_debugger_enable() {
    let resp = dispatch(r#"{"id":100,"method":"Debugger.enable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_debugger_disable() {
    let resp = dispatch(r#"{"id":1001,"method":"Debugger.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let resp = dispatch(r#"{"id":101,"method":"Debugger.setBreakpointByUrl"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["breakpointId"], "1");
    // locations array must be present and empty.
    let locs = result["locations"].as_array().unwrap();
    assert_eq!(locs.len(), 0);
}

#[test]
fn test_debugger_pause() {
    let resp = dispatch(r#"{"id":102,"method":"Debugger.pause"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_debugger_resume() {
    let resp = dispatch(r#"{"id":103,"method":"Debugger.resume"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_debugger_step_over() {
    let resp = dispatch(r#"{"id":104,"method":"Debugger.stepOver"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_debugger_step_into_and_out() {
    for m in &["stepInto", "stepOut"] {
        let resp = dispatch(&format!(r#"{{"id":1,"method":"Debugger.{}"}}"#, m));
        assert!(resp.result.unwrap().is_object(), "Debugger.{} ok_empty", m);
    }
}

#[test]
fn test_debugger_misc_ok_empty() {
    // removeBreakpoint, setSkipAllPauses, setBreakpointsActive, setPauseOnExceptions.
    for m in &[
        "removeBreakpoint",
        "setSkipAllPauses",
        "setBreakpointsActive",
        "setPauseOnExceptions",
    ] {
        let resp = dispatch(&format!(r#"{{"id":1,"method":"Debugger.{}"}}"#, m));
        assert!(resp.result.unwrap().is_object(), "Debugger.{} ok_empty", m);
    }
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let resp = dispatch(r#"{"id":105,"method":"Debugger.evaluateOnCallFrame"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let resp = dispatch(r#"{"id":1051,"method":"Debugger.getPossibleBreakpoints"}"#);
    let result = resp.result.unwrap();
    let locs = result["locations"].as_array().unwrap();
    assert_eq!(locs.len(), 0);
}

#[test]
fn test_debugger_get_script_source() {
    let resp = dispatch(r#"{"id":106,"method":"Debugger.getScriptSource"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["scriptSource"], "");
}

#[test]
fn test_debugger_unknown() {
    let resp = dispatch(r#"{"id":107,"method":"Debugger.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Debugger.nonexistent"));
}

// ---- Log domain ----

#[test]
fn test_log_enable() {
    let resp = dispatch(r#"{"id":110,"method":"Log.enable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_log_disable() {
    let resp = dispatch(r#"{"id":111,"method":"Log.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_log_clear() {
    let resp = dispatch(r#"{"id":112,"method":"Log.clear"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_log_start_violations_report() {
    let resp = dispatch(r#"{"id":113,"method":"Log.startViolationsReport"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_log_stop_violations_report() {
    let resp = dispatch(r#"{"id":1131,"method":"Log.stopViolationsReport"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_log_unknown() {
    let resp = dispatch(r#"{"id":114,"method":"Log.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Log.nonexistent"));
}

// ---- Fetch domain ----

#[test]
fn test_fetch_enable_no_patterns() {
    let resp = dispatch(r#"{"id":120,"method":"Fetch.enable"}"#);
    let result = resp.result.unwrap();
    assert_eq!(result["patternCount"], 0);
    // enabled flag must be true regardless of pattern count.
    assert_eq!(result["enabled"], true);
}

#[test]
fn test_fetch_enable_with_patterns() {
    let resp = dispatch_with_params("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}));
    let result = resp.result.unwrap();
    assert_eq!(result["patternCount"], 1);
    assert_eq!(result["enabled"], true);
}

#[test]
fn test_fetch_enable_pattern_count_matches_array_len() {
    // patternCount must equal patterns.len() exactly (boundary: 0,1,many).
    for n in 0..=3 {
        let patterns: Vec<Value> = (0..n).map(|i| json!({"urlPattern": format!("p{}", i)})).collect();
        let resp = dispatch_with_params("Fetch.enable", json!({"patterns": patterns}));
        let result = resp.result.unwrap();
        assert_eq!(result["patternCount"], n as u64);
    }
}

#[test]
fn test_fetch_disable() {
    let resp = dispatch(r#"{"id":121,"method":"Fetch.disable"}"#);
    let result = resp.result.unwrap();
    assert!(result.is_object());
}

#[test]
fn test_fetch_continue_request() {
    let resp = dispatch_with_params("Fetch.continueRequest", json!({"requestId": "r-1"}));
    let result = resp.result.unwrap();
    // requestId is echoed verbatim from params, not synthesized.
    assert_eq!(result["requestId"], "r-1");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_continue_with_response() {
    // continueWithResponse shares the continueRequest arm shape.
    let resp = dispatch_with_params("Fetch.continueWithResponse", json!({"requestId": "r-1b"}));
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "r-1b");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_fail_request() {
    let resp = dispatch_with_params("Fetch.failRequest", json!({"requestId": "r-2", "reason": "Aborted"}));
    let result = resp.result.unwrap();
    assert_eq!(result["failed"], true);
    // reason must be echoed back verbatim.
    assert_eq!(result["reason"], "Aborted");
    assert_eq!(result["requestId"], "r-2");
}

#[test]
fn test_fetch_fulfill_request() {
    let resp = dispatch_with_params("Fetch.fulfillRequest", json!({"requestId": "r-3", "responseCode": 200, "body": ""}));
    let result = resp.result.unwrap();
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
    // bodyLength = body.len() — empty body ⇒ 0.
    assert_eq!(result["bodyLength"], 0);
    assert_eq!(result["requestId"], "r-3");
}

#[test]
fn test_fetch_fulfill_request_default_status() {
    // Missing responseCode ⇒ defaults to 200.
    let resp = dispatch_with_params("Fetch.fulfillRequest", json!({"requestId": "r-3b"}));
    let result = resp.result.unwrap();
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 0);
}

#[test]
fn test_fetch_fulfill_request_body_length_tracks_body() {
    // Non-empty body ⇒ bodyLength equals byte length.
    let body = "hello";
    let resp = dispatch_with_params("Fetch.fulfillRequest", json!({"requestId": "r-3c", "body": body}));
    let result = resp.result.unwrap();
    assert_eq!(result["bodyLength"], body.len());
}

#[test]
fn test_fetch_get_request_post_data() {
    let resp = dispatch_with_params("Fetch.getRequestPostData", json!({"requestId": "r-4"}));
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "r-4");
    // postData defaults to "".
    assert_eq!(result["postData"], "");
}

#[test]
fn test_fetch_continue_with_auth() {
    let resp = dispatch_with_params("Fetch.continueWithAuth", json!({"requestId": "r-4b"}));
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "r-4b");
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let resp = dispatch_with_params("Fetch.takeResponseBodyAsStream", json!({"requestId": "r-5"}));
    let result = resp.result.unwrap();
    // stream = format!("stream-{requestId}") — verifiable format, not just is_string.
    assert_eq!(result["stream"], "stream-r-5");
}

#[test]
fn test_fetch_unknown() {
    let resp = dispatch(r#"{"id":122,"method":"Fetch.nonexistent"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("Fetch.nonexistent"));
}

// ---- Unknown domain ----

#[test]
fn test_unknown_domain() {
    let resp = dispatch(r#"{"id":200,"method":"Unknown.method"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    // Error message must echo the full offending method name (JSON-RPC §5.1 contract).
    assert!(err.message.contains("Unknown.method"));
    assert!(err.message.contains("wasn't found"));
    // On error, result must be absent (not null).
    assert!(resp.result.is_none());
    // Request id must be echoed back in the response.
    assert_eq!(resp.id, Some(200));
}

#[test]
fn test_empty_domain() {
    // ".method" splits into domain="" command="method" → unknown domain.
    let resp = dispatch(r#"{"id":201,"method":".method"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert_eq!(resp.id, Some(201));
}

#[test]
fn test_method_without_dot() {
    // "nodotmethod" has no '.' ⇒ splitn(2,'.') yields ["nodotmethod"], command="".
    // Still routes to the unknown-domain arm (domain="nodotmethod").
    let resp = dispatch(r#"{"id":202,"method":"nodotmethod"}"#);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
    assert_eq!(resp.id, Some(202));
}

#[test]
fn test_id_propagation_across_domains() {
    // Every domain must echo the request id verbatim in the response.
    for (id, method) in [
        (i64::MAX, "Page.enable"),
        (0, "Runtime.enable"),
        (-1, "DOM.enable"),
        (999_999, "Network.enable"),
    ] {
        let resp = dispatch(&format!(r#"{{"id":{},"method":"{}"}}"#, id, method));
        assert_eq!(resp.id, Some(id), "id propagation failed for {} with id {}", method, id);
        assert!(resp.error.is_none(), "{} should succeed", method);
    }
}

#[test]
fn test_notification_no_id_propagates_none() {
    // CDP notifications carry no id ⇒ response id is None (internal backend stub).
    let resp = dispatch_full(None, "Page.enable", None, None);
    assert_eq!(resp.id, None);
    assert!(resp.result.is_some());
}

#[test]
fn test_session_id_does_not_break_dispatch() {
    // A session_id in the message must not crash dispatch nor change the result shape.
    let resp = dispatch_full(Some(42), "Page.enable", None, Some("session-xyz"));
    assert_eq!(resp.id, Some(42));
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

// ---- CdpMessage parse ----

#[test]
fn test_parse_message_basic() {
    let msg = parse_message(r#"{"id":1,"method":"Page.navigate"}"#).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.navigate");
    // No params/session ⇒ None (serde default).
    assert!(msg.params.is_none());
    assert!(msg.session_id.is_none());
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
fn test_parse_message_id_boundaries() {
    // id: 0, negative, and i64::MAX all parse to Some(value).
    let m0 = parse_message(r#"{"id":0,"method":"Page.enable"}"#).unwrap();
    assert_eq!(m0.id, Some(0));
    let mneg = parse_message(r#"{"id":-7,"method":"Page.enable"}"#).unwrap();
    assert_eq!(mneg.id, Some(-7));
    let mmax = parse_message(&format!(r#"{{"id":{},"method":"Page.enable"}}"#, i64::MAX)).unwrap();
    assert_eq!(mmax.id, Some(i64::MAX));
}

#[test]
fn test_parse_message_null_id_is_none() {
    // id:null ⇒ Option<i64>::None (JSON-RPC notification semantics).
    let msg = parse_message(r#"{"id":null,"method":"Page.enable"}"#).unwrap();
    assert_eq!(msg.id, None);
}

#[test]
fn test_parse_message_missing_id_is_none() {
    // id field omitted entirely ⇒ None via #[serde(default)] on Option.
    let msg = parse_message(r#"{"method":"Page.enable"}"#).unwrap();
    assert_eq!(msg.id, None);
}

#[test]
fn test_parse_message_extra_fields_ignored() {
    // Unknown top-level fields are silently ignored (forward-compat).
    let msg = parse_message(r#"{"id":1,"method":"Page.enable","extra":"x","n":42}"#).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.enable");
}

#[test]
fn test_parse_message_invalid() {
    assert!(parse_message("{broken}").is_none());
    assert!(parse_message("").is_none());
    assert!(parse_message("null").is_none());
    assert!(parse_message("[]").is_none());
    // Numbers, booleans, strings are not valid CdpMessage objects.
    assert!(parse_message("42").is_none());
    assert!(parse_message("true").is_none());
    assert!(parse_message("\"string\"").is_none());
    // Object missing required "method" field.
    assert!(parse_message(r#"{"id":1}"#).is_none());
}

// ---- CdpResponse serialize ----

#[test]
fn test_serialize_response_success() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"ok": true})),
        error: None,
    };
    let json_str = serialize_response(&resp);
    // Output must be valid JSON.
    let v: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(v["id"], 1);
    // result is a nested object: {"ok": true} lives under v["result"]["ok"].
    assert_eq!(v["result"]["ok"], true);
    assert!(v.get("error").is_none());
}

#[test]
fn test_serialize_response_error() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let json_str = serialize_response(&resp);
    let v: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(v["id"], 2);
    assert_eq!(v["error"]["code"], -32601);
    assert_eq!(v["error"]["message"], "not found");
    // On error, "result" key is skipped (skip_serializing_if Option::is_none).
    assert!(v.get("result").is_none());
}

#[test]
fn test_serialize_response_null_id() {
    // id:None (notification-style) serializes to "id":null.
    let resp = CdpResponse { id: None, result: Some(json!({})), error: None };
    let v: Value = serde_json::from_str(&serialize_response(&resp)).unwrap();
    assert!(v["id"].is_null());
}

// ---- CdpEvent serialize ----

#[test]
fn test_serialize_event() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"ts": 42})),
    };
    let json_str = serialize_event(&ev);
    let v: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(v["method"], "Page.loadEventFired");
    assert_eq!(v["params"]["ts"], 42);
    // Events carry no id field.
    assert!(v.get("id").is_none());
}

#[test]
fn test_serialize_event_no_params() {
    let ev = CdpEvent {
        method: "DOM.updated".into(),
        params: None,
    };
    let json_str = serialize_event(&ev);
    let v: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(v["method"], "DOM.updated");
    assert!(v.get("params").is_none());
}

// ---- BridgeReceiver try_process / drain ----

#[test]
fn test_bridge_try_process() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    let processed = rx.try_process(|cmd| {
        let debug = format!("{:?}", cmd);
        assert!(debug.contains("GetTitle"));
        // target_id is preserved through the channel.
        assert!(debug.contains(TID));
        BridgeResponse { result: Ok(json!({"title": "Test"})) }
    });
    assert!(processed);
    // After try_process consumes the message, the queue is empty.
    let again = rx.try_process(|_| BridgeResponse { result: Ok(json!({})) });
    assert!(!again);
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
    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    tx.send_fire_and_forget(BridgeCommand::GetUrl { target_id: TID.into() });
    tx.send_fire_and_forget(BridgeCommand::GetDocument { target_id: TID.into() });
    let count = rx.drain(|cmd| {
        let _ = format!("{:?}", cmd);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count, 3);
    // drain must empty the queue fully.
    let again = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(again, 0);
}

#[test]
fn test_bridge_drain_empty() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let count = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 0);
}

#[test]
fn test_bridge_send_fire_and_forget_does_not_block() {
    // fire-and-forget must succeed even when nobody is receiving.
    let (tx, _rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    // Reaching here means no panic/timeout.
}

// ---- BridgeSender clone ----

#[test]
fn test_bridge_sender_clone() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let cloned = tx.clone();
    // Both senders must report alive while the receiver lives.
    assert!(cloned.is_alive());
    assert!(tx.is_alive());
    drop(rx);
}

#[test]
fn test_bridge_sender_alive_when_paired() {
    // is_alive returns true while the receiver end is alive.
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    assert!(tx.is_alive());
    drop(rx);
    // After receiver dropped, is_alive may still report via try_send semantics;
    // the binding contract is "alive while paired", asserted above.
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

#[test]
fn test_bridge_response_ok_complex_json() {
    // Round-trip a nested JSON value through BridgeResponse::result.
    let payload = json!({
        "nested": { "arr": [1, 2, 3], "flag": true },
        "str": "hello",
        "num": 3.14
    });
    let resp = BridgeResponse { result: Ok(payload.clone()) };
    let val = resp.result.unwrap();
    assert_eq!(val, payload);
    assert_eq!(val["nested"]["arr"][1], 2);
    assert_eq!(val["str"], "hello");
}
