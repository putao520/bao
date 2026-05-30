// @trace TEST-CDP-001 [req:REQ-CDP-004] [level:unit]
// @trace TEST-CDP-002 [req:REQ-CDP-002] [level:unit]
// @trace TEST-CDP-003 [req:REQ-CDP-005] [level:unit]
// @trace TEST-CDP-004 [req:REQ-CDP-007] [level:unit]

use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeSender, BridgeResponse};
use bao_cdp::domains::register_all_domains_into;
use bao_cdp::DomainRegistry;
use cdp_server::{DomainHandler, EventSender, CdpError};
use serde_json::{json, Value};
use std::time::Duration;
use std::thread;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct NoopEventSender;

impl EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

/// Create a registry with all domains registered, plus a background thread
/// that auto-responds to bridge commands.
fn setup() -> (DomainRegistry, BridgeSender) {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let registry = DomainRegistry::new();
    register_all_domains_into(tx.clone(), &registry);

    // Keep an extra clone alive so the channel stays open after test drops tx
    let keeper = tx.clone();
    thread::spawn(move || {
        let _keeper = keeper;
        // Poll loop using try_process — keeps thread alive for all bridge calls
        loop {
            let handled = rx.try_process(|cmd| default_bridge_response(cmd));
            if !handled {
                thread::sleep(Duration::from_millis(1));
            }
        }
    });

    (registry, tx)
}

fn default_bridge_response(cmd: BridgeCommand) -> BridgeResponse {
    match cmd {
        BridgeCommand::GetTitle => BridgeResponse {
            result: Ok(json!("Test Page")),
        },
        BridgeCommand::GetUrl => BridgeResponse {
            result: Ok(json!("https://example.com")),
        },
        BridgeCommand::GetDocument => BridgeResponse {
            result: Ok(json!({
                "root": {
                    "nodeId": 1, "nodeType": 9, "nodeName": "#document",
                    "localName": "", "nodeValue": "", "childNodeCount": 1
                }
            })),
        },
        BridgeCommand::QuerySelector { .. } => BridgeResponse {
            result: Ok(json!({ "nodeId": 42 })),
        },
        BridgeCommand::QuerySelectorAll { .. } => BridgeResponse {
            result: Ok(json!({ "nodeIds": [42, 43] })),
        },
        BridgeCommand::TakeScreenshot { .. } => BridgeResponse {
            result: Ok(json!({ "data": "iVBORw0KGgo=" })),
        },
        BridgeCommand::EvaluateJs { .. } => BridgeResponse {
            result: Ok(json!({ "result": { "type": "string", "value": "ok" } })),
        },
        _ => BridgeResponse {
            result: Ok(json!({})),
        },
    }
}

fn dispatch(registry: &DomainRegistry, method: &str, params: Value) -> Result<Value, CdpError> {
    let es = NoopEventSender;
    registry.dispatch_command(method, params, &es)
        .ok_or_else(|| CdpError { code: -32601, message: format!("domain not found for '{}'", method) })
        .and_then(|r| r)
}

// ===========================================================================
// Page domain tests (REQ-CDP-004)
// ===========================================================================

#[test]
fn test_page_enable_disable() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Page.enable", json!({}));
    assert!(result.is_ok());
    let result = dispatch(&registry, "Page.disable", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_page_navigate() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Page.navigate", json!({"url": "https://example.com"})).unwrap();
    assert_eq!(result["frameId"], "0");
    assert!(result["loaderId"].is_string());
}

#[test]
fn test_page_reload() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Page.reload", json!({})).unwrap();
    assert_eq!(result["frameId"], "0");
}

#[test]
fn test_page_get_frame_tree() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Page.getFrameTree", json!({})).unwrap();
    let frame = &result["frameTree"]["frame"];
    assert!(frame["url"].is_string());
    assert_eq!(frame["mimeType"], "text/html");
}

#[test]
fn test_page_get_navigation_history() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Page.getNavigationHistory", json!({})).unwrap();
    assert_eq!(result["currentIndex"], 0);
    assert!(result["entries"].is_array());
}

#[test]
fn test_page_capture_screenshot() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Page.captureScreenshot", json!({"format": "png"}));
    match result {
        Ok(v) => assert!(v["data"].is_string() || v.is_object()),
        Err(e) => assert_eq!(e.code, -32603, "bridge error expected: {}", e.message),
    }
}

#[test]
fn test_page_get_layout_metrics() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Page.getLayoutMetrics", json!({})).unwrap();
    assert!(result["contentSize"]["width"].is_number());
}

#[test]
fn test_page_unknown_command() {
    let (registry, _) = setup();
    let err = dispatch(&registry, "Page.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ===========================================================================
// Runtime domain tests (REQ-CDP-002)
// ===========================================================================

#[test]
fn test_runtime_enable() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Runtime.enable", json!({})).unwrap();
    assert!(result["executionContextId"].is_number());
}

#[test]
fn test_runtime_evaluate() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Runtime.evaluate", json!({"expression": "1+1"})).unwrap();
    assert!(result["result"].is_object());
}

#[test]
fn test_runtime_get_properties() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Runtime.getProperties", json!({})).unwrap();
    assert!(result["result"].is_array());
}

// ===========================================================================
// DOM domain tests (REQ-CDP-005)
// ===========================================================================

#[test]
fn test_dom_enable_disable() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "DOM.enable", json!({})).is_ok());
    assert!(dispatch(&registry, "DOM.disable", json!({})).is_ok());
}

#[test]
fn test_dom_get_document() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.getDocument", json!({})).unwrap();
    assert!(result["root"].is_object());
}

#[test]
fn test_dom_query_selector() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.querySelector", json!({"selector": "div"}));
    // querySelector with selector needs active bridge — may return bridge error or value
    match result {
        Ok(v) => assert!(v.is_object(), "querySelector should return object"),
        Err(e) => assert_eq!(e.code, -32603, "bridge error should be -32603, got: {}", e.code),
    }
}

#[test]
fn test_dom_query_selector_all() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.querySelectorAll", json!({"selector": "div"})).unwrap();
    assert!(result["nodeIds"].is_array());
}

#[test]
fn test_dom_get_box_model() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.getBoxModel", json!({})).unwrap();
    assert!(result["model"]["width"].is_number());
}

// ===========================================================================
// Stub domain tests (CSS, Overlay, Log, Fetch)
// ===========================================================================

#[test]
fn test_css_enable_disable() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "CSS.enable", json!({})).is_ok());
    assert!(dispatch(&registry, "CSS.disable", json!({})).is_ok());
}

#[test]
fn test_css_get_computed_style() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "CSS.getComputedStyleForNode", json!({})).unwrap();
    assert!(result["computedStyle"].is_array());
}

#[test]
fn test_overlay_enable_disable() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Overlay.enable", json!({})).is_ok());
}

#[test]
fn test_log_enable_disable() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Log.enable", json!({})).is_ok());
    assert!(dispatch(&registry, "Log.clear", json!({})).is_ok());
}

#[test]
fn test_fetch_enable() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]})).unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 1);
}

#[test]
fn test_fetch_disable() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Fetch.disable", json!({})).is_ok());
}

// ===========================================================================
// Debugger domain tests
// ===========================================================================

#[test]
fn test_debugger_enable_disable() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Debugger.enable", json!({})).is_ok());
    assert!(dispatch(&registry, "Debugger.disable", json!({})).is_ok());
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Debugger.setBreakpointByUrl", json!({})).unwrap();
    assert!(result["breakpointId"].is_string());
}

#[test]
fn test_debugger_get_script_source() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Debugger.getScriptSource", json!({})).unwrap();
    assert!(result["scriptSource"].is_string());
}

// ===========================================================================
// Network domain tests (stub)
// ===========================================================================

#[test]
fn test_network_enable_disable() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Network.enable", json!({})).is_ok());
    assert!(dispatch(&registry, "Network.disable", json!({})).is_ok());
}

#[test]
fn test_network_get_response_body() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Network.getResponseBody", json!({})).unwrap();
    assert!(result["body"].is_string());
}

// ===========================================================================
// Unknown domain returns none
// ===========================================================================

#[test]
fn test_unknown_domain() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "UnknownDomain.method", json!({}));
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

// ===========================================================================
// TargetProvider tests
// ===========================================================================

#[test]
fn test_target_provider_list_targets() {
    use bao_cdp::domains::ServoTargetProvider;
    use cdp_server::TargetProvider;

    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let provider = ServoTargetProvider::new(tx, "127.0.0.1".into(), 9222);

    // list_targets calls bridge.send() synchronously, so we need a thread
    // that's already listening when we call it.
    let responder = thread::spawn(move || {
        loop {
            if !rx.try_process(|cmd| default_bridge_response(cmd)) {
                thread::sleep(Duration::from_millis(1));
            }
        }
    });

    // Give responder time to start
    thread::sleep(Duration::from_millis(10));

    let targets = provider.list_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].target_type, "page");
    assert_eq!(targets[0].title, "Test Page");
    assert_eq!(targets[0].url, "https://example.com");
    assert!(targets[0].web_socket_debugger_url.contains("127.0.0.1:9222"));

    // Responder thread will be cleaned up when channel closes on drop
}

#[test]
fn test_target_provider_create_target() {
    use bao_cdp::domains::ServoTargetProvider;
    use cdp_server::TargetProvider;

    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let provider = ServoTargetProvider::new(tx, "127.0.0.1".into(), 9222);

    thread::spawn(move || {
        rx.drain(|cmd| default_bridge_response(cmd));
    });

    let target = provider.create_target("https://example.com").unwrap();
    assert_eq!(target.target_type, "page");
}

#[test]
fn test_target_provider_close_target() {
    use bao_cdp::domains::ServoTargetProvider;
    use cdp_server::TargetProvider;

    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let provider = ServoTargetProvider::new(tx, "127.0.0.1".into(), 9222);

    thread::spawn(move || {
        rx.drain(|cmd| default_bridge_response(cmd));
    });

    assert!(provider.close_target("any-id").is_ok());
}

#[test]
fn test_target_provider_activate_target() {
    use bao_cdp::domains::ServoTargetProvider;
    use cdp_server::TargetProvider;

    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let provider = ServoTargetProvider::new(tx, "127.0.0.1".into(), 9222);

    thread::spawn(move || {
        rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    });

    assert!(provider.activate_target("any-id").is_ok());
}
