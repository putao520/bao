// @trace TEST-CDP-CHAIN [req:REQ-CDP-001] [level:integration]
// CDP full-chain integration: DomainRegistry + DomainHandler command routing

use bao_cdp::domains::register_all_domains_into;
use bao_cdp::servo_bridge::bridge_channel;
use cdp_server::{DomainRegistry, EventSender, CdpError};
use serde_json::{json, Value};
use std::time::Duration;

struct NoopSender;
impl EventSender for NoopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

fn make_registry() -> DomainRegistry {
    let mut reg = DomainRegistry::new();
    let (tx, _rx) = bridge_channel(Duration::from_secs(5));
    register_all_domains_into(tx, &mut reg);
    reg
}

fn cmd(reg: &DomainRegistry, method: &str, params: Value) -> Option<Result<Value, CdpError>> {
    reg.dispatch_command(method, params, &NoopSender)
}

/// Returns true if the dispatch succeeded (Some(Ok)).
fn ok(reg: &DomainRegistry, method: &str, params: Value) -> bool {
    matches!(cmd(reg, method, params), Some(Ok(_)))
}

#[test]
fn test_all_domains_registered() {
    let reg = make_registry();
    for d in &["Page","DOM","Runtime","Network","Debugger",
               "Input","Emulation","CSS","Overlay","Log","Fetch"] {
        assert!(reg.has_domain(d), "domain '{}' missing", d);
    }
}

#[test]
fn test_page_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "Page.enable", json!({})));
    assert!(ok(&r, "Page.disable", json!({})));
}

#[test]
fn test_page_commands() {
    let r = make_registry();
    assert!(ok(&r, "Page.enable", json!({})));
    let _ = cmd(&r, "Page.navigate", json!({"url":"https://example.com"}));
    let _ = cmd(&r, "Page.getFrameTree", json!({}));
    let _ = cmd(&r, "Page.captureScreenshot", json!({}));
    let _ = cmd(&r, "Page.reload", json!({}));
    assert!(ok(&r, "Page.disable", json!({})));
}

#[test]
fn test_runtime_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "Runtime.enable", json!({})));
    assert!(ok(&r, "Runtime.disable", json!({})));
}

#[test]
fn test_runtime_commands() {
    let r = make_registry();
    let _ = cmd(&r, "Runtime.evaluate", json!({"expression":"1+1"}));
    let _ = cmd(&r, "Runtime.callFunctionOn", json!({"functionDeclaration":"function(){return 42;}","executionContextId":1}));
}

#[test]
fn test_dom_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "DOM.enable", json!({})));
    assert!(ok(&r, "DOM.disable", json!({})));
}

#[test]
fn test_dom_commands() {
    let r = make_registry();
    let _ = cmd(&r, "DOM.getDocument", json!({}));
    let _ = cmd(&r, "DOM.querySelector", json!({"nodeId":1,"selector":"div"}));
    let _ = cmd(&r, "DOM.querySelectorAll", json!({"nodeId":1,"selector":"span"}));
}

#[test]
fn test_network_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "Network.enable", json!({})));
    assert!(ok(&r, "Network.disable", json!({})));
}

#[test]
fn test_network_commands() {
    let r = make_registry();
    let _ = cmd(&r, "Network.setCacheDisabled", json!({"cacheDisabled":true}));
    let _ = cmd(&r, "Network.getCookies", json!({"urls":["https://example.com"]}));
    let _ = cmd(&r, "Network.setExtraHTTPHeaders", json!({"headers":{"X-Test":"1"}}));
}

#[test]
fn test_debugger_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "Debugger.enable", json!({})));
    assert!(ok(&r, "Debugger.disable", json!({})));
}

#[test]
fn test_debugger_commands() {
    let r = make_registry();
    let _ = cmd(&r, "Debugger.setBreakpointByUrl", json!({"lineNumber":10,"url":"test.js"}));
    let _ = cmd(&r, "Debugger.pause", json!({}));
    let _ = cmd(&r, "Debugger.resume", json!({}));
    let _ = cmd(&r, "Debugger.stepOver", json!({}));
}

#[test]
fn test_input_commands() {
    let r = make_registry();
    let _ = cmd(&r, "Input.dispatchMouseEvent", json!({"type":"mousePressed","x":100,"y":200,"button":"left","clickCount":1}));
    let _ = cmd(&r, "Input.dispatchKeyEvent", json!({"type":"keyDown","key":"Enter","code":"Enter"}));
    let _ = cmd(&r, "Input.insertText", json!({"text":"hello"}));
}

#[test]
fn test_emulation_commands() {
    let r = make_registry();
    let _ = cmd(&r, "Emulation.setDeviceMetricsOverride", json!({"width":1920,"height":1080,"deviceScaleFactor":1.0,"mobile":false}));
    let _ = cmd(&r, "Emulation.setUserAgentOverride", json!({"userAgent":"Test/1.0"}));
    let _ = cmd(&r, "Emulation.clearDeviceMetricsOverride", json!({}));
}

#[test]
fn test_css_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "CSS.enable", json!({})));
    assert!(ok(&r, "CSS.disable", json!({})));
}

#[test]
fn test_overlay_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "Overlay.enable", json!({})));
    assert!(ok(&r, "Overlay.disable", json!({})));
}

#[test]
fn test_log_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "Log.enable", json!({})));
    assert!(ok(&r, "Log.disable", json!({})));
    let _ = cmd(&r, "Log.clear", json!({}));
}

#[test]
fn test_fetch_enable_disable() {
    let r = make_registry();
    assert!(ok(&r, "Fetch.enable", json!({})));
    assert!(ok(&r, "Fetch.disable", json!({})));
}

#[test]
fn test_fetch_commands() {
    let r = make_registry();
    let _ = cmd(&r, "Fetch.continueRequest", json!({"requestId":"test-123"}));
    let _ = cmd(&r, "Fetch.failRequest", json!({"requestId":"test-456","reason":"Aborted"}));
}

#[test]
fn test_unknown_command_returns_error() {
    let r = make_registry();
    let res = cmd(&r, "Page.nonexistentMethod", json!({}));
    assert!(matches!(res, Some(Err(_))), "unknown command should error");
}

#[test]
fn test_unknown_domain_returns_none() {
    let r = make_registry();
    let res = cmd(&r, "NonExistentDomain.method", json!({}));
    assert!(res.is_none(), "unknown domain should return None");
}

#[test]
fn test_page_full_lifecycle() {
    let r = make_registry();
    assert!(ok(&r, "Page.enable", json!({})));
    let _ = cmd(&r, "Page.navigate", json!({"url":"https://test.com"}));
    let _ = cmd(&r, "Page.getFrameTree", json!({}));
    let _ = cmd(&r, "Page.captureScreenshot", json!({}));
    let _ = cmd(&r, "Page.getLayoutMetrics", json!({}));
    assert!(ok(&r, "Page.disable", json!({})));
}

#[test]
fn test_dom_full_lifecycle() {
    let r = make_registry();
    assert!(ok(&r, "DOM.enable", json!({})));
    let _ = cmd(&r, "DOM.getDocument", json!({}));
    let _ = cmd(&r, "DOM.querySelector", json!({"nodeId":1,"selector":"body"}));
    let _ = cmd(&r, "DOM.getOuterHTML", json!({"nodeId":1}));
    assert!(ok(&r, "DOM.disable", json!({})));
}

#[test]
fn test_network_full_lifecycle() {
    let r = make_registry();
    assert!(ok(&r, "Network.enable", json!({})));
    let _ = cmd(&r, "Network.setCacheDisabled", json!({"cacheDisabled":true}));
    let _ = cmd(&r, "Network.getCookies", json!({}));
    let _ = cmd(&r, "Network.emulateNetworkConditions", json!({"offline":false,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1}));
    assert!(ok(&r, "Network.disable", json!({})));
}
