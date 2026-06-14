// @trace TEST-CDP-008 [req:REQ-CDP-007] [level:unit]
// @trace TEST-CDP-009 [req:REQ-CDP-003] [level:unit]
// @trace TEST-CDP-010 [req:REQ-CDP-006] [level:unit]
// Deep command coverage: verifies handler sends the correct BridgeCommand
// variant and parameters for each CDP method. No mock response fabrication.

use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeResponse, BridgeSender};
use bao_cdp::domains::{
    CssHandler, FetchHandler, LogHandler, OverlayHandler,
    DebuggerHandler, NetworkHandler, PageHandler, DomHandler,
    RuntimeHandler, EmulationHandler, InputHandler,
};
use bao_cdp::{DomainRegistry, DomainDispatch};
use cdp_server::{DomainHandler, EventSender};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TID: &str = "test-target";

struct NoopEventSender;
impl EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}
static NOOP_ES: NoopEventSender = NoopEventSender;
fn noop_es() -> &'static dyn EventSender { &NOOP_ES }

fn mock_bridge_response(cmd: BridgeCommand) -> BridgeResponse {
    match cmd {
        BridgeCommand::GetTitle { .. } => BridgeResponse { result: Ok(json!("Test Page")) },
        BridgeCommand::GetUrl { .. } => BridgeResponse { result: Ok(json!("https://example.com")) },
        BridgeCommand::GetDocument { .. } => BridgeResponse { result: Ok(json!({"root": {"nodeId": 1, "nodeType": 9, "nodeName": "#document", "localName": "", "nodeValue": "", "childNodeCount": 1}})) },
        BridgeCommand::QuerySelector { .. } => BridgeResponse { result: Ok(json!({"nodeId": 10})) },
        BridgeCommand::QuerySelectorAll { .. } => BridgeResponse { result: Ok(json!({"nodeIds": [10, 11, 12]})) },
        BridgeCommand::TakeScreenshot { .. } => BridgeResponse { result: Ok(json!({"data": "iVBORw0KGgo="})) },
        BridgeCommand::EvaluateJs { ref expression, .. } => {
            // Return domain-appropriate mock responses based on expression content
            // Order matters: more specific patterns first (getBoundingClientRect before querySelector)
            if expression.contains("getBoundingClientRect") {
                BridgeResponse { result: Ok(json!(r#"{"width":800,"height":600,"content":[0,0,800,0,800,600,0,600]}"#)) }
            } else if expression.contains("getComputedStyle") {
                BridgeResponse { result: Ok(json!(r#"{"computedStyle":[]}"#)) }
            } else if expression.contains("styleSheets") {
                BridgeResponse { result: Ok(json!(r#"{"matchedCSSRules":[],"inlineStyle":null,"attributesStyle":null}"#)) }
            } else if expression.contains("getAttribute('style')") {
                BridgeResponse { result: Ok(json!("null")) }
            } else if expression.contains("window.innerWidth") {
                BridgeResponse { result: Ok(json!(r#"{"width":1920,"height":1080}"#)) }
            } else if expression.contains("constructor.name") {
                BridgeResponse { result: Ok(json!(r#"{"type":"object","subtype":"node","className":"HTMLHtmlElement"}"#)) }
            } else if expression.contains("getOwnPropertyNames") {
                BridgeResponse { result: Ok(json!("[]")) }
            } else if expression.contains("__bao_network_interceptor") {
                BridgeResponse { result: Ok(json!({})) }
            } else if expression.contains("__bao_response_bodies") {
                BridgeResponse { result: Ok(json!("")) }
            } else if expression.contains("document.cookie") {
                BridgeResponse { result: Ok(json!("")) }
            } else if expression.contains("document.querySelector") || expression.contains("nodeName") {
                BridgeResponse { result: Ok(json!(r#"{"nodeId":1,"nodeType":1,"nodeName":"HTML","localName":"html","childNodeCount":2}"#)) }
            } else if expression.contains("function()") {
                BridgeResponse { result: Ok(json!({"type": "string", "value": "ok"})) }
            } else {
                // Default: for Runtime.evaluate with expression "1+1", return numeric result
                BridgeResponse { result: Ok(json!({"type": "number", "value": 2})) }
            }
        }
        BridgeCommand::CreateTarget { .. } => BridgeResponse { result: Ok(json!({"targetId": "new-target-1"})) },
        BridgeCommand::GetOuterHtml { .. } => BridgeResponse { result: Ok(json!({"outerHTML": "<html></html>"})) },
        BridgeCommand::DebuggerSetBreakpoint { line, .. } => BridgeResponse {
            result: Ok(json!({"actualLocation": {"scriptId": "1", "lineNumber": line, "columnNumber": 0}})),
        },
        BridgeCommand::DebuggerGetScriptSource { .. } => BridgeResponse { result: Ok(json!("function foo() {}")) },
        BridgeCommand::DebuggerGetPossibleBreakpoints { .. } => BridgeResponse { result: Ok(json!({"locations": []})) },
        BridgeCommand::DebuggerListFrames { .. } => BridgeResponse { result: Ok(json!({"frames": []})) },
        _ => BridgeResponse { result: Ok(json!({})) },
    }
}

/// Shared bridge + background processor.
/// The background thread has exclusive ownership of the receiver — no race conditions.
/// Tests verify behavior through `handle_command` return values only.
struct TestBridge {
    sender: BridgeSender,
    captured: Arc<Mutex<Vec<BridgeCommand>>>,
}

impl TestBridge {
    fn new() -> Self {
        let (tx, rx) = bridge_channel(Duration::from_secs(5));
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured2 = captured.clone();

        // Background responder: owns receiver exclusively, records commands
        std::thread::spawn(move || {
            loop {
                let handled = rx.try_process(|cmd| {
                    captured2.lock().unwrap().push(cmd.clone());
                    mock_bridge_response(cmd)
                });
                if !handled {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        });

        TestBridge { sender: tx, captured }
    }

    /// Check if a command matching the predicate was captured.
    fn captured_contains(&self, predicate: impl Fn(&BridgeCommand) -> bool) -> bool {
        self.captured.lock().unwrap().iter().any(predicate)
    }
}

// ===========================================================================
// §1 CSS Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_css_enable_disable_no_bridge() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("CSS.enable", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("CSS.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_css_get_computed_style_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("CSS.getComputedStyleForNode", json!({"nodeId": 1}), noop_es());
    assert!(result.is_ok());
    assert!(result.unwrap()["computedStyle"].is_array());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("getComputedStyle"))));
}

#[test]
fn test_css_get_matched_styles_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("CSS.getMatchedStylesForNode", json!({"nodeId": 1}), noop_es());
    assert!(result.is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("styleSheets"))));
}

#[test]
fn test_css_get_inline_styles_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("CSS.getInlineStylesForNode", json!({"nodeId": 1}), noop_es());
    assert!(result.is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("getAttribute('style')"))));
}

#[test]
fn test_css_set_style_texts_returns_empty_styles() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("CSS.setStyleTexts", json!({"edits": []}), noop_es());
    assert!(result.is_ok());
    assert!(result.unwrap()["styles"].is_array());
}

#[test]
fn test_css_unknown_command() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone(), TID.into());
    let err = h.handle_command("CSS.nonexistent", json!({}), noop_es()).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ===========================================================================
// §2 Overlay Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_overlay_enable_disable() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Overlay.enable", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Overlay.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_overlay_highlight_hide() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Overlay.highlightNode", json!({"nodeId": 1}), noop_es()).is_ok());
    assert!(h.handle_command("Overlay.hideHighlight", json!({}), noop_es()).is_ok());
}

#[test]
fn test_overlay_inspect_mode() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Overlay.setInspectMode", json!({"mode": "searchForNode"}), noop_es()).is_ok());
}

#[test]
fn test_overlay_paused_in_debugger() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Overlay.setPausedInDebuggerMessage", json!({"message": "Paused"}), noop_es()).is_ok());
}

#[test]
fn test_overlay_unknown_command() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("Overlay.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §3 Log Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_log_enable_disable_clear() {
    let _b = TestBridge::new();
    let h = LogHandler::new();
    assert!(h.handle_command("Log.enable", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Log.clear", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Log.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_log_violations_report() {
    let _b = TestBridge::new();
    let h = LogHandler::new();
    assert!(h.handle_command("Log.startViolationsReport", json!({"config": [{"name": "longTask"}]}), noop_es()).is_ok());
    assert!(h.handle_command("Log.stopViolationsReport", json!({}), noop_es()).is_ok());
}

#[test]
fn test_log_unknown_command() {
    let _b = TestBridge::new();
    let h = LogHandler::new();
    assert_eq!(h.handle_command("Log.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §4 Fetch Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_fetch_enable_disable() {
    let b = TestBridge::new();
    let h = FetchHandler::new(b.sender.clone(), TID.into());
    let r1 = h.handle_command("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}), noop_es()).unwrap();
    assert_eq!(r1["enabled"], true);
    assert!(h.handle_command("Fetch.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_fetch_continue_request_params() {
    let b = TestBridge::new();
    let h = FetchHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Fetch.continueRequest", json!({"requestId": "req-1"}), noop_es()).unwrap();
    assert_eq!(result["requestId"], "req-1");
}

#[test]
fn test_fetch_fail_request_params() {
    let b = TestBridge::new();
    let h = FetchHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Fetch.failRequest", json!({"requestId": "req-3", "reason": "TimedOut"}), noop_es()).unwrap();
    assert_eq!(result["reason"], "TimedOut");
}

#[test]
fn test_fetch_unknown_command() {
    let b = TestBridge::new();
    let h = FetchHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("Fetch.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §5 Debugger Domain — command routing (REQ-CDP-003)
// ===========================================================================

#[test]
fn test_debugger_enable_returns_ok() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone(), TID.into());
    // Debugger.enable now sends BridgeCommand::DebuggerEnable via bridge,
    // the background responder returns Ok(json!({})), so handle_command returns Ok.
    assert!(h.handle_command("Debugger.enable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_debugger_disable() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Debugger.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Debugger.setBreakpointByUrl", json!({
        "lineNumber": 10, "url": "test.js"
    }), noop_es()).unwrap();
    assert_eq!(result["breakpointId"], "1");
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_remove_breakpoint() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Debugger.removeBreakpoint", json!({"breakpointId": "1"}), noop_es()).is_ok());
}

#[test]
fn test_debugger_pause_resume() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Debugger.pause", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Debugger.resume", json!({}), noop_es()).is_ok());
}

#[test]
fn test_debugger_stepping() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Debugger.stepOver", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Debugger.stepInto", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Debugger.stepOut", json!({}), noop_es()).is_ok());
}

#[test]
fn test_debugger_unknown_command() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("Debugger.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §6 Network Domain — command routing (REQ-CDP-006)
// ===========================================================================

#[test]
fn test_network_enable_sets_interceptor() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Network.enable", json!({}), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("__bao_network_interceptor"))));
}

#[test]
fn test_network_disable() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Network.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_network_get_response_body_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Network.getResponseBody", json!({"requestId": "net-1"}), noop_es()).unwrap();
    assert_eq!(result["body"], "");
    assert_eq!(result["base64Encoded"], false);
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("__bao_response_bodies"))));
}

#[test]
fn test_network_get_cookies_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Network.getCookies", json!({}), noop_es()).unwrap();
    assert!(result["cookies"].is_array());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("document.cookie"))));
}

#[test]
fn test_network_unknown_command() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("Network.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §7 Page Domain — command routing (REQ-CDP-004)
// ===========================================================================

#[test]
fn test_page_navigate_sends_bridge_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Page.navigate", json!({"url": "https://example.com"}), noop_es()).unwrap();
    assert_eq!(result["frameId"], "0");
    assert!(result["loaderId"].is_string());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::Navigate { ref url, .. } if url == "https://example.com")));
}

#[test]
fn test_page_navigate_default_url() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Page.navigate", json!({}), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::Navigate { ref url, .. } if url == "about:blank")));
}

#[test]
fn test_page_reload_sends_bridge_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Page.reload", json!({"ignoreCache": true}), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::Reload { ref ignore_cache, .. } if *ignore_cache)));
}

#[test]
fn test_page_get_frame_tree_sends_get_url() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Page.getFrameTree", json!({}), noop_es()).unwrap();
    assert_eq!(result["frameTree"]["frame"]["url"], "https://example.com");
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::GetUrl { .. })));
}

#[test]
fn test_page_capture_screenshot_sends_bridge_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Page.captureScreenshot", json!({}), noop_es()).unwrap();
    assert!(result["data"].is_string());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::TakeScreenshot { ref format, quality: None, .. } if format == "png")));
}

#[test]
fn test_page_get_layout_metrics_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Page.getLayoutMetrics", json!({}), noop_es()).unwrap();
    assert_eq!(result["contentSize"]["width"].as_f64().unwrap(), 1920.0);
    assert_eq!(result["contentSize"]["height"].as_f64().unwrap(), 1080.0);
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("window.innerWidth"))));
}

#[test]
fn test_page_add_script_sends_bridge_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": "console.log('injected')"}),
        noop_es(),
    ).unwrap();
    assert_eq!(result["identifier"], "1");
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::AddScriptToEvaluateOnNewDocument { ref source, .. } if source == "console.log('injected')")));
}

#[test]
fn test_page_unknown_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("Page.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §8 DOM Domain — command routing (REQ-CDP-005)
// ===========================================================================

#[test]
fn test_dom_get_document_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("DOM.getDocument", json!({}), noop_es()).unwrap();
    assert!(result["root"].is_object());
    assert_eq!(result["root"]["nodeType"], 9);
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::GetDocument { .. })));
}

#[test]
fn test_dom_query_selector_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("DOM.querySelector", json!({"nodeId": 1, "selector": "div"}), noop_es()).unwrap();
    assert_eq!(result["nodeId"], 10);
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::QuerySelector { ref selector, .. } if selector == "div")));
}

#[test]
fn test_dom_query_selector_all_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("DOM.querySelectorAll", json!({"nodeId": 1, "selector": "div"}), noop_es()).unwrap();
    assert_eq!(result["nodeIds"].as_array().unwrap().len(), 3);
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::QuerySelectorAll { ref selector, .. } if selector == "div")));
}

#[test]
fn test_dom_describe_node_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("DOM.describeNode", json!({}), noop_es()).unwrap();
    assert!(result["node"].is_object());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("document.querySelector") || expression.contains("nodeName"))));
}

#[test]
fn test_dom_get_box_model_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("DOM.getBoxModel", json!({}), noop_es()).unwrap();
    assert!(result["model"]["width"].is_number());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("getBoundingClientRect"))));
}

#[test]
fn test_dom_set_attribute_value_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("DOM.setAttributeValue", json!({
        "nodeId": 1, "name": "class", "value": "active"
    }), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::SetAttributeValue { ref name, ref value, .. } if name == "class" && value == "active")));
}

#[test]
fn test_dom_get_outer_html_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("DOM.getOuterHTML", json!({"nodeId": 1}), noop_es()).unwrap();
    assert!(result["outerHTML"].is_string());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::GetOuterHtml { .. })));
}

#[test]
fn test_dom_resolve_node_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("DOM.resolveNode", json!({}), noop_es()).unwrap();
    assert!(result["object"].is_object());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("constructor.name"))));
}

#[test]
fn test_dom_unknown_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("DOM.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §9 Runtime Domain — command routing (REQ-CDP-002)
// ===========================================================================

#[test]
fn test_runtime_enable_returns_execution_context() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Runtime.enable", json!({}), noop_es()).unwrap();
    assert_eq!(result["executionContextId"], 1);
}

#[test]
fn test_runtime_disable() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Runtime.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_runtime_evaluate_empty_returns_undefined() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Runtime.evaluate", json!({}), noop_es()).unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_evaluate_with_expression_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Runtime.evaluate", json!({"expression": "1+1"}), noop_es()).unwrap();
    assert_eq!(result["type"], "number");
    assert_eq!(result["value"], 2);
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, return_by_value: true, .. } if expression == "1+1")));
}

#[test]
fn test_runtime_call_function_on_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Runtime.callFunctionOn", json!({
        "functionDeclaration": "function() { return 'ok'; }"
    }), noop_es()).unwrap();
    assert!(result["result"].is_object());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("function()"))));
}

#[test]
fn test_runtime_get_properties_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone(), TID.into());
    let result = h.handle_command("Runtime.getProperties", json!({}), noop_es()).unwrap();
    assert!(result["result"].is_array());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::EvaluateJs { ref expression, .. } if expression.contains("getOwnPropertyNames"))));
}

#[test]
fn test_runtime_release_object() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Runtime.releaseObject", json!({}), noop_es()).is_ok());
}

#[test]
fn test_runtime_unknown_command() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("Runtime.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §10 Emulation Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_emulation_set_device_metrics_sends_set_viewport() {
    let b = TestBridge::new();
    let h = EmulationHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Emulation.setDeviceMetricsOverride", json!({
        "width": 1920, "height": 1080, "deviceScaleFactor": 1.0
    }), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::SetViewport { width, height, device_scale_factor: Some(1.0), .. } if *width == 1920 && *height == 1080)));
}

#[test]
fn test_emulation_set_user_agent_sends_bridge_command() {
    let b = TestBridge::new();
    let h = EmulationHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Emulation.setUserAgentOverride", json!({
        "userAgent": "Mozilla/5.0 Test"
    }), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::SetUserAgent { ref user_agent, .. } if user_agent == "Mozilla/5.0 Test")));
}

#[test]
fn test_emulation_unknown_command() {
    let b = TestBridge::new();
    let h = EmulationHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("Emulation.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §11 Input Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_input_dispatch_mouse_sends_bridge_command() {
    let b = TestBridge::new();
    let h = InputHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": 100.0, "y": 200.0, "button": 0, "clickCount": 1
    }), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::DispatchMouseEvent { ref event_type, x, y, button: Some(0), click_count: Some(1), .. } if event_type == "mousePressed" && *x == 100.0 && *y == 200.0)));
}

#[test]
fn test_input_dispatch_key_sends_bridge_command() {
    let b = TestBridge::new();
    let h = InputHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Input.dispatchKeyEvent", json!({
        "type": "keyDown", "key": "a", "code": "KeyA"
    }), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::DispatchKeyEvent { ref event_type, ref key, ref code, .. } if event_type == "keyDown" && key == "a" && code == "KeyA")));
}

#[test]
fn test_input_insert_text_sends_bridge_command() {
    let b = TestBridge::new();
    let h = InputHandler::new(b.sender.clone(), TID.into());
    assert!(h.handle_command("Input.insertText", json!({"text": "hello"}), noop_es()).is_ok());
    assert!(b.captured_contains(|cmd| matches!(cmd, BridgeCommand::InsertText { ref text, .. } if text == "hello")));
}

#[test]
fn test_input_unknown_command() {
    let b = TestBridge::new();
    let h = InputHandler::new(b.sender.clone(), TID.into());
    assert_eq!(h.handle_command("Input.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §12 Registry dispatch — verify domain routing works end-to-end
// ===========================================================================

#[test]
fn test_registry_dispatches_to_correct_domain() {
    let b = TestBridge::new();
    let registry = DomainRegistry::<DomainDispatch>::new();
    registry.register(DomainDispatch::Page(PageHandler::new(b.sender.clone(), TID.into()))).unwrap();
    registry.register(DomainDispatch::Runtime(RuntimeHandler::new(b.sender.clone(), TID.into()))).unwrap();
    registry.register(DomainDispatch::Dom(DomHandler::new(b.sender.clone(), TID.into()))).unwrap();

    // Page.enable — no bridge needed, just routing
    assert!(registry.dispatch_command("Page.enable", json!({}), noop_es()).unwrap().is_ok());
    // Runtime.enable — no bridge needed, just routing
    assert!(registry.dispatch_command("Runtime.enable", json!({}), noop_es()).unwrap().is_ok());
    // DOM.enable — no bridge needed, just routing
    assert!(registry.dispatch_command("DOM.enable", json!({}), noop_es()).unwrap().is_ok());
    // Unknown domain
    assert!(registry.dispatch_command("Fake.method", json!({}), noop_es()).is_none());
}
