// @trace TEST-CDP-031 [req:REQ-CDP-002,REQ-CDP-004,REQ-CDP-005,REQ-CDP-006] [level:unit]
// Domain handler command routing: PageHandler, RuntimeHandler, DomHandler,
// NetworkHandler. Tests cover: enable/disable, command recognition,
// unknown command error, domain_name, bridge-mediated commands.

use bao_cdp::domains::{PageHandler, RuntimeHandler, DomHandler, NetworkHandler};
use bao_cdp::{BridgeCommand, BridgeResponse, BridgeSender, BridgeReceiver, bridge_channel};
use cdp_server::{DomainHandler, EventSender};
use serde_json::{json, Value};
use std::time::Duration;

// Static noop event sender for tests
struct NoopSender;
impl EventSender for NoopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}
static NOOP: NoopSender = NoopSender;
fn noop_es() -> &'static dyn EventSender { &NOOP }

fn bridge(timeout_ms: u64) -> (BridgeSender, BridgeReceiver) {
    bridge_channel(Duration::from_millis(timeout_ms))
}

// Helper: process one command from bridge and return a canned response
fn process_bridge(rx: &BridgeReceiver, response: Value) {
    rx.try_process(|_cmd| BridgeResponse { result: Ok(response) });
}

// Helper: process one command and return error
fn process_bridge_err(rx: &BridgeReceiver, msg: &str) {
    rx.try_process(|_cmd| BridgeResponse { result: Err(msg.to_string()) });
}

// ============================================================================
// PageHandler
// ============================================================================

#[test]
fn test_page_domain_name() {
    let (tx, _rx) = bridge(50);
    let h = PageHandler::new(tx);
    assert_eq!(h.domain_name(), "Page");
}

#[test]
fn test_page_enable() {
    let (tx, _rx) = bridge(50);
    let h = PageHandler::new(tx);
    let result = h.handle_command("Page.enable", json!({}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_page_disable() {
    let (tx, _rx) = bridge(50);
    let h = PageHandler::new(tx);
    let result = h.handle_command("Page.disable", json!({}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_page_navigate() {
    let (tx, rx) = bridge(500);
    let h = PageHandler::new(tx);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            let done = {
                let guard = rx2.lock().unwrap();
                guard.try_process(|cmd| {
                    assert!(matches!(cmd, BridgeCommand::Navigate { .. }));
                    BridgeResponse { result: Ok(json!({})) }
                })
            };
            if done { return; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    let result = h.handle_command("Page.navigate", json!({"url": "https://example.com"}), noop_es());
    assert!(result.is_ok());
    let val = result.unwrap();
    assert_eq!(val["frameId"], "0");
    assert!(val["loaderId"].is_string());
}

#[test]
fn test_page_navigate_default_url() {
    let (tx, rx) = bridge(500);
    let h = PageHandler::new(tx);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            let done = {
                let guard = rx2.lock().unwrap();
                guard.try_process(|cmd| {
                    if let BridgeCommand::Navigate { url } = cmd {
                        assert_eq!(url, "about:blank");
                    }
                    BridgeResponse { result: Ok(json!({})) }
                })
            };
            if done { return; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    let result = h.handle_command("Page.navigate", json!({}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_page_reload() {
    let (tx, rx) = bridge(500);
    let h = PageHandler::new(tx);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            let done = {
                let guard = rx2.lock().unwrap();
                guard.try_process(|cmd| {
                    assert!(matches!(cmd, BridgeCommand::Reload { .. }));
                    BridgeResponse { result: Ok(json!({})) }
                })
            };
            if done { return; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    let result = h.handle_command("Page.reload", json!({"ignoreCache": true}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_page_get_frame_tree() {
    let (tx, rx) = bridge(500);
    let h = PageHandler::new(tx);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            let done = {
                let guard = rx2.lock().unwrap();
                guard.try_process(|cmd| {
                    assert!(matches!(cmd, BridgeCommand::GetUrl));
                    BridgeResponse { result: Ok(json!("https://example.com")) }
                })
            };
            if done { return; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    let result = h.handle_command("Page.getFrameTree", json!({}), noop_es());
    assert!(result.is_ok());
    let val = result.unwrap();
    assert_eq!(val["frameTree"]["frame"]["url"], "https://example.com");
}

#[test]
fn test_page_get_layout_metrics() {
    let (tx, _rx) = bridge(50);
    let h = PageHandler::new(tx);
    let result = h.handle_command("Page.getLayoutMetrics", json!({}), noop_es());
    assert!(result.is_ok());
    let val = result.unwrap();
    assert_eq!(val["contentSize"]["width"], 1920);
    assert_eq!(val["contentSize"]["height"], 1080);
}

#[test]
fn test_page_capture_screenshot() {
    let (tx, rx) = bridge(500);
    let h = PageHandler::new(tx);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            let done = {
                let guard = rx2.lock().unwrap();
                guard.try_process(|cmd| {
                    if let BridgeCommand::TakeScreenshot { format, quality } = cmd {
                        assert_eq!(format, "png");
                        assert_eq!(quality, Some(80));
                    }
                    BridgeResponse { result: Ok(json!({"data": "base64data"})) }
                })
            };
            if done { return; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    let result = h.handle_command("Page.captureScreenshot", json!({"format": "png", "quality": 80}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_page_add_script() {
    let (tx, rx) = bridge(500);
    let h = PageHandler::new(tx);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            let done = {
                let guard = rx2.lock().unwrap();
                guard.try_process(|cmd| {
                    assert!(matches!(cmd, BridgeCommand::AddScriptToEvaluateOnNewDocument { .. }));
                    BridgeResponse { result: Ok(json!({})) }
                })
            };
            if done { return; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    let result = h.handle_command("Page.addScriptToEvaluateOnNewDocument", json!({"source": "console.log(1)"}), noop_es());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["identifier"], "1");
}

#[test]
fn test_page_set_content() {
    let (tx, _rx) = bridge(50);
    let h = PageHandler::new(tx);
    let result = h.handle_command("Page.setContent", json!({}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_page_unknown_command() {
    let (tx, _rx) = bridge(50);
    let h = PageHandler::new(tx);
    let result = h.handle_command("Page.nonexistent", json!({}), noop_es());
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

// ============================================================================
// RuntimeHandler
// ============================================================================

#[test]
fn test_runtime_domain_name() {
    let (tx, _rx) = bridge(50);
    let h = RuntimeHandler::new(tx);
    assert_eq!(h.domain_name(), "Runtime");
}

#[test]
fn test_runtime_enable() {
    let (tx, _rx) = bridge(50);
    let h = RuntimeHandler::new(tx);
    let result = h.handle_command("Runtime.enable", json!({}), noop_es());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["executionContextId"], 1);
}

#[test]
fn test_runtime_disable() {
    let (tx, _rx) = bridge(50);
    let h = RuntimeHandler::new(tx);
    assert!(h.handle_command("Runtime.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_runtime_evaluate_empty() {
    let (tx, _rx) = bridge(50);
    let h = RuntimeHandler::new(tx);
    let result = h.handle_command("Runtime.evaluate", json!({}), noop_es());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["result"]["type"], "undefined");
}

#[test]
fn test_runtime_evaluate_with_expression() {
    let (tx, rx) = bridge(500);
    let h = RuntimeHandler::new(tx);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            let processed = {
                let guard = rx2.lock().unwrap();
                guard.try_process(|cmd| {
                    if let BridgeCommand::EvaluateJs { expression, return_by_value } = cmd {
                        assert_eq!(expression, "1+1");
                        assert!(return_by_value);
                    }
                    BridgeResponse { result: Ok(json!({"type": "number", "value": 2})) }
                })
            };
            if processed { return; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    let result = h.handle_command("Runtime.evaluate", json!({"expression": "1+1"}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_runtime_call_function_on() {
    let (tx, _rx) = bridge(50);
    let h = RuntimeHandler::new(tx);
    let result = h.handle_command("Runtime.callFunctionOn", json!({}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_runtime_get_properties() {
    let (tx, _rx) = bridge(50);
    let h = RuntimeHandler::new(tx);
    let result = h.handle_command("Runtime.getProperties", json!({}), noop_es());
    assert!(result.is_ok());
    assert!(result.unwrap()["result"].is_array());
}

#[test]
fn test_runtime_release_object() {
    let (tx, _rx) = bridge(50);
    let h = RuntimeHandler::new(tx);
    assert!(h.handle_command("Runtime.releaseObject", json!({}), noop_es()).is_ok());
}

#[test]
fn test_runtime_unknown() {
    let (tx, _rx) = bridge(50);
    let h = RuntimeHandler::new(tx);
    let err = h.handle_command("Runtime.fake", json!({}), noop_es()).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// DomHandler
// ============================================================================

#[test]
fn test_dom_domain_name() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    assert_eq!(h.domain_name(), "DOM");
}

#[test]
fn test_dom_enable_disable() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    assert!(h.handle_command("DOM.enable", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("DOM.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_dom_describe_node() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    let result = h.handle_command("DOM.describeNode", json!({}), noop_es());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["node"]["nodeName"], "HTML");
}

#[test]
fn test_dom_query_selector_empty() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    let result = h.handle_command("DOM.querySelector", json!({}), noop_es());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_empty() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    let result = h.handle_command("DOM.querySelectorAll", json!({}), noop_es());
    assert!(result.is_ok());
    assert!(result.unwrap()["nodeIds"].is_array());
}

#[test]
fn test_dom_get_box_model() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    let result = h.handle_command("DOM.getBoxModel", json!({}), noop_es());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["model"]["width"], 1920);
}

#[test]
fn test_dom_remove_attribute() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    assert!(h.handle_command("DOM.removeAttribute", json!({}), noop_es()).is_ok());
}

#[test]
fn test_dom_resolve_node() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    let result = h.handle_command("DOM.resolveNode", json!({}), noop_es());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["object"]["type"], "node");
}

#[test]
fn test_dom_unknown() {
    let (tx, _rx) = bridge(50);
    let h = DomHandler::new(tx);
    let err = h.handle_command("DOM.fakeCommand", json!({}), noop_es()).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ============================================================================
// NetworkHandler
// ============================================================================

#[test]
fn test_network_domain_name() {
    let h = NetworkHandler;
    assert_eq!(h.domain_name(), "Network");
}

#[test]
fn test_network_enable_disable() {
    let h = NetworkHandler;
    assert!(h.handle_command("Network.enable", json!({}), noop_es()).is_ok());
    assert!(h.handle_command("Network.disable", json!({}), noop_es()).is_ok());
}

#[test]
fn test_network_get_response_body() {
    let h = NetworkHandler;
    let result = h.handle_command("Network.getResponseBody", json!({}), noop_es());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["base64Encoded"], false);
}

#[test]
fn test_network_get_cookies() {
    let h = NetworkHandler;
    let result = h.handle_command("Network.getCookies", json!({}), noop_es());
    assert!(result.is_ok());
    assert!(result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies() {
    let h = NetworkHandler;
    let result = h.handle_command("Network.getAllCookies", json!({}), noop_es());
    assert!(result.is_ok());
}

#[test]
fn test_network_set_cache_disabled() {
    let h = NetworkHandler;
    assert!(h.handle_command("Network.setCacheDisabled", json!({}), noop_es()).is_ok());
}

#[test]
fn test_network_emulate_conditions() {
    let h = NetworkHandler;
    assert!(h.handle_command("Network.emulateNetworkConditions", json!({}), noop_es()).is_ok());
}

#[test]
fn test_network_unknown() {
    let h = NetworkHandler;
    let err = h.handle_command("Network.nonexistent", json!({}), noop_es()).unwrap_err();
    assert_eq!(err.code, -32601);
}
