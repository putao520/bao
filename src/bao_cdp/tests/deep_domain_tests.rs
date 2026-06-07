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
use bao_cdp::DomainRegistry;
use cdp_server::{DomainHandler, EventSender};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct NoopEventSender;
impl EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}
static NOOP_ES: NoopEventSender = NoopEventSender;
fn noop_es() -> &'static dyn EventSender { &NOOP_ES }

/// Process a single bridge command: verify the command via `check`,
/// then return a generic OK response.
fn process_one(rx: &bao_cdp::servo_bridge::BridgeReceiver, check: impl FnOnce(&BridgeCommand)) {
    rx.recv_and_process(Duration::from_secs(5), |cmd| {
        check(&cmd);
        BridgeResponse { result: Ok(json!({})) }
    });
}

/// Process a single bridge command with a custom response.
fn process_one_with(rx: &bao_cdp::servo_bridge::BridgeReceiver, resp: BridgeResponse) {
    rx.recv_and_process(Duration::from_secs(5), |_cmd| resp);
}

/// Shared bridge + background processor: spawn a thread that drains
/// bridge commands for the test lifetime.
struct TestBridge {
    #[allow(dead_code)]
    sender: BridgeSender,
    receiver: Arc<Mutex<bao_cdp::servo_bridge::BridgeReceiver>>,
}

impl TestBridge {
    fn new() -> Self {
        let (tx, rx) = bridge_channel(Duration::from_secs(5));
        TestBridge {
            sender: tx,
            receiver: Arc::new(Mutex::new(rx)),
        }
    }

    /// Run `f` with the receiver lock, giving test code access to recv_and_process.
    fn with_rx<F, R>(&self, f: F) -> R
    where F: FnOnce(&bao_cdp::servo_bridge::BridgeReceiver) -> R {
        let guard = self.receiver.lock().unwrap();
        f(&guard)
    }
}

// ===========================================================================
// §1 CSS Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_css_enable_disable_no_bridge() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone());
    assert!(h.handle_command("CSS.enable", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("CSS.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_css_get_computed_style_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, return_by_value } => {
                    assert!(expression.contains("getComputedStyle"));
                    assert!(return_by_value);
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!(r#"{"computedStyle":[]}"#)) }
        });
    });
    let result = h.handle_command("CSS.getComputedStyleForNode", json!({"nodeId": 1}), noop_es());
    t.join().unwrap();
    assert!(result.is_ok());
    assert!(result.unwrap()["computedStyle"].is_array());
}

#[test]
fn test_css_get_matched_styles_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("styleSheets"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!(r#"{"matchedCSSRules":[],"inlineStyle":null,"attributesStyle":null}"#)) }
        });
    });
    let result = h.handle_command("CSS.getMatchedStylesForNode", json!({"nodeId": 1}), noop_es());
    t.join().unwrap();
    assert!(result.is_ok());
}

#[test]
fn test_css_get_inline_styles_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("getAttribute('style')"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!("null")) }
        });
    });
    let result = h.handle_command("CSS.getInlineStylesForNode", json!({"nodeId": 1}), noop_es());
    t.join().unwrap();
    assert!(result.is_ok());
}

#[test]
fn test_css_set_style_texts_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            assert!(matches!(cmd, BridgeCommand::EvaluateJs { .. }));
            BridgeResponse { result: Ok(json!(r#"{"styles":[]}"#)) }
        });
    });
    let result = h.handle_command("CSS.setStyleTexts", json!({"edits": []}), noop_es());
    t.join().unwrap();
    assert!(result.is_ok());
}

#[test]
fn test_css_unknown_command() {
    let b = TestBridge::new();
    let h = CssHandler::new(b.sender.clone());
    let err = h.handle_command("CSS.nonexistent", json!({}), noop_es()).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ===========================================================================
// §2 Overlay Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_overlay_enable_disable() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone());
    assert!(h.handle_command("Overlay.enable", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Overlay.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_overlay_highlight_hide() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone());
    assert!(h.handle_command("Overlay.highlightNode", json!({"nodeId": 1}), noop_es()).is_ok());
    assert!(h.handle_command("Overlay.hideHighlight", json!({}), noop_es()).is_ok());
}

#[test]
fn test_overlay_inspect_mode() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone());
    assert!(h.handle_command("Overlay.setInspectMode", json!({"mode": "searchForNode"}), noop_es()).is_ok());
}

#[test]
fn test_overlay_paused_in_debugger() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone());
    assert!(h.handle_command("Overlay.setPausedInDebuggerMessage", json!({"message": "Paused"}), noop_es()).is_ok());
}

#[test]
fn test_overlay_unknown_command() {
    let b = TestBridge::new();
    let h = OverlayHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("Overlay.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §3 Log Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_log_enable_disable_clear() {
    let b = TestBridge::new();
    let h = LogHandler::new();
    assert!(h.handle_command("Log.enable", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Log.clear", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Log.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_log_violations_report() {
    let b = TestBridge::new();
    let h = LogHandler::new();
    assert!(h.handle_command("Log.startViolationsReport", json!({"config": [{"name": "longTask"}]}), noop_es()).is_ok());
    assert!(h.handle_command("Log.stopViolationsReport", json!({}), noop_es()).is_ok());
}

#[test]
fn test_log_unknown_command() {
    let b = TestBridge::new();
    let h = LogHandler::new();
    assert_eq!(h.handle_command("Log.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §4 Fetch Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_fetch_enable_disable() {
    let b = TestBridge::new();
    let h = FetchHandler::new(b.sender.clone());
    let r1 = h.handle_command("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}), noop_es()).unwrap();
    assert_eq!(r1["enabled"], true);
    assert!(h.handle_command("Fetch.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_fetch_continue_request_params() {
    let b = TestBridge::new();
    let h = FetchHandler::new(b.sender.clone());
    let result = h.handle_command("Fetch.continueRequest", json!({"requestId": "req-1"}), noop_es()).unwrap();
    assert_eq!(result["requestId"], "req-1");
}

#[test]
fn test_fetch_fail_request_params() {
    let b = TestBridge::new();
    let h = FetchHandler::new(b.sender.clone());
    let result = h.handle_command("Fetch.failRequest", json!({"requestId": "req-3", "reason": "TimedOut"}), noop_es()).unwrap();
    assert_eq!(result["reason"], "TimedOut");
}

#[test]
fn test_fetch_unknown_command() {
    let b = TestBridge::new();
    let h = FetchHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("Fetch.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §5 Debugger Domain — command routing (REQ-CDP-003)
// ===========================================================================

#[test]
fn test_debugger_enable_sets_spidermonkey_debugger() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("__bao_debugger_active"));
                }
                _ => panic!("Expected EvaluateJs for debugger setup, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Debugger.enable", json!({}), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_debugger_disable() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone());
    assert!(h.handle_command("Debugger.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone());
    let result = h.handle_command("Debugger.setBreakpointByUrl", json!({
        "lineNumber": 10, "url": "test.js"
    }), noop_es()).unwrap();
    assert_eq!(result["breakpointId"], "1");
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_remove_breakpoint() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone());
    assert!(h.handle_command("Debugger.removeBreakpoint", json!({"breakpointId": "1"}), noop_es()).is_ok());
}

#[test]
fn test_debugger_pause_resume() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone());
    assert!(h.handle_command("Debugger.pause", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Debugger.resume", json!({}), noop_es()).is_ok());
}

#[test]
fn test_debugger_stepping() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone());
    assert!(h.handle_command("Debugger.stepOver", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Debugger.stepInto", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Debugger.stepOut", json!({}), noop_es()).is_ok());
}

#[test]
fn test_debugger_unknown_command() {
    let b = TestBridge::new();
    let h = DebuggerHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("Debugger.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §6 Network Domain — command routing (REQ-CDP-006)
// ===========================================================================

#[test]
fn test_network_enable_sets_interceptor() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("__bao_network_interceptor"));
                }
                _ => panic!("Expected EvaluateJs for network interceptor, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Network.enable", json!({}), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_network_disable() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone());
    assert!(h.handle_command("Network.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_network_get_response_body_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("__bao_response_bodies"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!("")) }
        });
    });
    let result = h.handle_command("Network.getResponseBody", json!({"requestId": "net-1"}), noop_es()).unwrap();
    t.join().unwrap();
    assert_eq!(result["body"], "");
    assert_eq!(result["base64Encoded"], false);
}

#[test]
fn test_network_get_cookies_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("document.cookie"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!("")) }
        });
    });
    let result = h.handle_command("Network.getCookies", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["cookies"].is_array());
}

#[test]
fn test_network_unknown_command() {
    let b = TestBridge::new();
    let h = NetworkHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("Network.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §7 Page Domain — command routing (REQ-CDP-004)
// ===========================================================================

#[test]
fn test_page_navigate_sends_bridge_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::Navigate { ref url } => {
                    assert_eq!(url, "https://example.com");
                }
                _ => panic!("Expected Navigate, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let result = h.handle_command("Page.navigate", json!({"url": "https://example.com"}), noop_es()).unwrap();
    t.join().unwrap();
    assert_eq!(result["frameId"], "0");
    assert!(result["loaderId"].is_string());
}

#[test]
fn test_page_navigate_default_url() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::Navigate { ref url } => {
                    assert_eq!(url, "about:blank");
                }
                _ => panic!("Expected Navigate with default URL, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Page.navigate", json!({}), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_page_reload_sends_bridge_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::Reload { ref ignore_cache } => {
                    assert!(ignore_cache);
                }
                _ => panic!("Expected Reload, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Page.reload", json!({"ignoreCache": true}), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_page_get_frame_tree_sends_get_url() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            assert!(matches!(cmd, BridgeCommand::GetUrl));
            BridgeResponse { result: Ok(json!("https://example.com")) }
        });
    });
    let result = h.handle_command("Page.getFrameTree", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert_eq!(result["frameTree"]["frame"]["url"], "https://example.com");
}

#[test]
fn test_page_capture_screenshot_sends_bridge_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::TakeScreenshot { ref format, ref quality } => {
                    assert_eq!(format, "png");
                    assert_eq!(quality, &None);
                }
                _ => panic!("Expected TakeScreenshot, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({"data": "c2NyZWVuc2hvdA=="})) }
        });
    });
    let result = h.handle_command("Page.captureScreenshot", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["data"].is_string());
}

#[test]
fn test_page_get_layout_metrics_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("window.innerWidth"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!(r#"{"width":1920,"height":1080}"#)) }
        });
    });
    let result = h.handle_command("Page.getLayoutMetrics", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert_eq!(result["contentSize"]["width"].as_f64().unwrap(), 1920.0);
    assert_eq!(result["contentSize"]["height"].as_f64().unwrap(), 1080.0);
}

#[test]
fn test_page_add_script_sends_bridge_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::AddScriptToEvaluateOnNewDocument { ref source } => {
                    assert_eq!(source, "console.log('injected')");
                }
                _ => panic!("Expected AddScriptToEvaluateOnNewDocument, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let result = h.handle_command(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": "console.log('injected')"}),
        noop_es(),
    ).unwrap();
    t.join().unwrap();
    assert_eq!(result["identifier"], "1");
}

#[test]
fn test_page_unknown_command() {
    let b = TestBridge::new();
    let h = PageHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("Page.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §8 DOM Domain — command routing (REQ-CDP-005)
// ===========================================================================

#[test]
fn test_dom_get_document_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            assert!(matches!(cmd, BridgeCommand::GetDocument));
            BridgeResponse { result: Ok(json!({
                "root": { "nodeId": 1, "nodeType": 9, "nodeName": "#document",
                           "localName": "", "nodeValue": "", "childNodeCount": 2 }
            })) }
        });
    });
    let result = h.handle_command("DOM.getDocument", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["root"].is_object());
    assert_eq!(result["root"]["nodeType"], 9);
}

#[test]
fn test_dom_query_selector_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::QuerySelector { ref selector } => {
                    assert_eq!(selector, "div");
                }
                _ => panic!("Expected QuerySelector, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({"nodeId": 10})) }
        });
    });
    let result = h.handle_command("DOM.querySelector", json!({"nodeId": 1, "selector": "div"}), noop_es()).unwrap();
    t.join().unwrap();
    assert_eq!(result["nodeId"], 10);
}

#[test]
fn test_dom_query_selector_all_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::QuerySelectorAll { ref selector } => {
                    assert_eq!(selector, "div");
                }
                _ => panic!("Expected QuerySelectorAll, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({"nodeIds": [10, 11, 12]})) }
        });
    });
    let result = h.handle_command("DOM.querySelectorAll", json!({"nodeId": 1, "selector": "div"}), noop_es()).unwrap();
    t.join().unwrap();
    assert_eq!(result["nodeIds"].as_array().unwrap().len(), 3);
}

#[test]
fn test_dom_describe_node_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("document.querySelector") || expression.contains("nodeName"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!(r#"{"nodeId":1,"nodeType":1,"nodeName":"HTML","localName":"html","childNodeCount":2}"#)) }
        });
    });
    let result = h.handle_command("DOM.describeNode", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["node"].is_object());
}

#[test]
fn test_dom_get_box_model_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("getBoundingClientRect"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!(r#"{"width":800,"height":600,"content":[0,0,800,0,800,600,0,600]}"#)) }
        });
    });
    let result = h.handle_command("DOM.getBoxModel", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["model"]["width"].is_number());
}

#[test]
fn test_dom_set_attribute_value_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::SetAttributeValue { node_id: _, ref name, ref value } => {
                    assert_eq!(name, "class");
                    assert_eq!(value, "active");
                }
                _ => panic!("Expected SetAttributeValue, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("DOM.setAttributeValue", json!({
        "nodeId": 1, "name": "class", "value": "active"
    }), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_dom_get_outer_html_sends_bridge_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            assert!(matches!(cmd, BridgeCommand::GetOuterHtml { node_id: _ }));
            BridgeResponse { result: Ok(json!({"outerHTML": "<html></html>"})) }
        });
    });
    let result = h.handle_command("DOM.getOuterHTML", json!({"nodeId": 1}), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["outerHTML"].is_string());
}

#[test]
fn test_dom_resolve_node_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("constructor.name"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!(r#"{"type":"object","subtype":"node","className":"HTMLHtmlElement"}"#)) }
        });
    });
    let result = h.handle_command("DOM.resolveNode", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["object"].is_object());
}

#[test]
fn test_dom_unknown_command() {
    let b = TestBridge::new();
    let h = DomHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("DOM.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §9 Runtime Domain — command routing (REQ-CDP-002)
// ===========================================================================

#[test]
fn test_runtime_enable_returns_execution_context() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone());
    let result = h.handle_command("Runtime.enable", json!({}), noop_es()).unwrap();
    assert_eq!(result["executionContextId"], 1);
}

#[test]
fn test_runtime_disable() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone());
    assert!(h.handle_command("Runtime.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_runtime_evaluate_empty_returns_undefined() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone());
    let result = h.handle_command("Runtime.evaluate", json!({}), noop_es()).unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_evaluate_with_expression_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, return_by_value } => {
                    assert_eq!(expression, "1+1");
                    assert!(return_by_value);
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({"type": "number", "value": 2})) }
        });
    });
    let result = h.handle_command("Runtime.evaluate", json!({"expression": "1+1"}), noop_es()).unwrap();
    t.join().unwrap();
    assert_eq!(result["type"], "number");
    assert_eq!(result["value"], 2);
}

#[test]
fn test_runtime_call_function_on_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("function()"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({"type": "string", "value": "ok"})) }
        });
    });
    let result = h.handle_command("Runtime.callFunctionOn", json!({
        "functionDeclaration": "function() { return 'ok'; }"
    }), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["result"].is_object());
}

#[test]
fn test_runtime_get_properties_sends_evaluate_js() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { ref expression, .. } => {
                    assert!(expression.contains("getOwnPropertyNames"));
                }
                _ => panic!("Expected EvaluateJs, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!("[]")) }
        });
    });
    let result = h.handle_command("Runtime.getProperties", json!({}), noop_es()).unwrap();
    t.join().unwrap();
    assert!(result["result"].is_array());
}

#[test]
fn test_runtime_release_object() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone());
    assert!(h.handle_command("Runtime.releaseObject", json!({}), noop_es()).is_ok());
}

#[test]
fn test_runtime_unknown_command() {
    let b = TestBridge::new();
    let h = RuntimeHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("Runtime.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §10 Emulation Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_emulation_set_device_metrics_sends_set_viewport() {
    let b = TestBridge::new();
    let h = EmulationHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::SetViewport { width, height, device_scale_factor } => {
                    assert_eq!(width, 1920);
                    assert_eq!(height, 1080);
                    assert_eq!(device_scale_factor, Some(1.0));
                }
                _ => panic!("Expected SetViewport, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Emulation.setDeviceMetricsOverride", json!({
        "width": 1920, "height": 1080, "deviceScaleFactor": 1.0
    }), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_emulation_set_user_agent_sends_bridge_command() {
    let b = TestBridge::new();
    let h = EmulationHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::SetUserAgent { ref user_agent } => {
                    assert_eq!(user_agent, "Mozilla/5.0 Test");
                }
                _ => panic!("Expected SetUserAgent, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Emulation.setUserAgentOverride", json!({
        "userAgent": "Mozilla/5.0 Test"
    }), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_emulation_unknown_command() {
    let b = TestBridge::new();
    let h = EmulationHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("Emulation.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §11 Input Domain — command routing (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_input_dispatch_mouse_sends_bridge_command() {
    let b = TestBridge::new();
    let h = InputHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::DispatchMouseEvent { ref event_type, x, y, ref button, ref click_count } => {
                    assert_eq!(event_type, "mousePressed");
                    assert_eq!(x, 100.0);
                    assert_eq!(y, 200.0);
                    assert_eq!(button, &Some(0));
                    assert_eq!(click_count, &Some(1));
                }
                _ => panic!("Expected DispatchMouseEvent, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": 100.0, "y": 200.0, "button": 0, "clickCount": 1
    }), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_input_dispatch_key_sends_bridge_command() {
    let b = TestBridge::new();
    let h = InputHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::DispatchKeyEvent { ref event_type, ref key, ref code, .. } => {
                    assert_eq!(event_type, "keyDown");
                    assert_eq!(key, "a");
                    assert_eq!(code, "KeyA");
                }
                _ => panic!("Expected DispatchKeyEvent, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Input.dispatchKeyEvent", json!({
        "type": "keyDown", "key": "a", "code": "KeyA"
    }), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_input_insert_text_sends_bridge_command() {
    let b = TestBridge::new();
    let h = InputHandler::new(b.sender.clone());
    let rx = b.receiver.clone();
    let t = std::thread::spawn(move || {
        let guard = rx.lock().unwrap();
        guard.recv_and_process(Duration::from_secs(5), |cmd| {
            match cmd {
                BridgeCommand::InsertText { ref text } => {
                    assert_eq!(text, "hello");
                }
                _ => panic!("Expected InsertText, got {:?}", cmd),
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    assert!(h.handle_command("Input.insertText", json!({"text": "hello"}), noop_es()).is_ok());
    t.join().unwrap();
}

#[test]
fn test_input_unknown_command() {
    let b = TestBridge::new();
    let h = InputHandler::new(b.sender.clone());
    assert_eq!(h.handle_command("Input.nonexistent", json!({}), noop_es()).unwrap_err().code, -32601);
}

// ===========================================================================
// §12 Registry dispatch — verify domain routing works end-to-end
// ===========================================================================

#[test]
fn test_registry_dispatches_to_correct_domain() {
    let b = TestBridge::new();
    let registry = DomainRegistry::new();
    registry.register(Box::new(PageHandler::new(b.sender.clone()))).unwrap();
    registry.register(Box::new(RuntimeHandler::new(b.sender.clone()))).unwrap();
    registry.register(Box::new(DomHandler::new(b.sender.clone()))).unwrap();

    // Page.enable — no bridge needed, just routing
    assert!(registry.dispatch_command("Page.enable", json!({}), noop_es()).unwrap().is_ok());
    // Runtime.enable — no bridge needed, just routing
    assert!(registry.dispatch_command("Runtime.enable", json!({}), noop_es()).unwrap().is_ok());
    // DOM.enable — no bridge needed, just routing
    assert!(registry.dispatch_command("DOM.enable", json!({}), noop_es()).unwrap().is_ok());
    // Unknown domain
    assert!(registry.dispatch_command("Fake.method", json!({}), noop_es()).is_none());
}
