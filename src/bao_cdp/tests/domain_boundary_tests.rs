// @trace TEST-CDP-001~008-BND [req:REQ-CDP-001~008] [level:unit]
// CDP domain handler boundary tests: enable/disable lifecycle, unknown commands, error codes

use bao_cdp::servo_bridge::bridge_channel;
use bao_cdp::domains::register_all_domains_into;
use cdp_server::{CdpError, DomainRegistry, EventSender};
use serde_json::{json, Value};
use std::time::Duration;

struct NoopSender;
impl EventSender for NoopSender {
    fn send_event(&self, _: &str, _: Value) {}
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

fn ok_cmd(reg: &DomainRegistry, method: &str, params: Value) -> Value {
    match cmd(reg, method, params) {
        Some(Ok(v)) => v,
        other => panic!("expected ok for {}, got: {:?}", method, other),
    }
}

fn err_cmd(reg: &DomainRegistry, method: &str, params: Value) -> CdpError {
    match cmd(reg, method, params) {
        Some(Err(e)) => e,
        other => panic!("expected error for {}, got: {:?}", method, other),
    }
}

// ---- Enable/Disable lifecycle for all domains ----

#[test]
fn test_page_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "Page.enable", json!({}));
    ok_cmd(&reg, "Page.disable", json!({}));
}

#[test]
fn test_runtime_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "Runtime.enable", json!({}));
    ok_cmd(&reg, "Runtime.disable", json!({}));
}

#[test]
fn test_network_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "Network.enable", json!({}));
    ok_cmd(&reg, "Network.disable", json!({}));
}

#[test]
fn test_dom_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "DOM.enable", json!({}));
    ok_cmd(&reg, "DOM.disable", json!({}));
}

#[test]
fn test_debugger_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "Debugger.enable", json!({}));
    ok_cmd(&reg, "Debugger.disable", json!({}));
}

#[test]
fn test_emulation_has_commands() {
    let reg = make_registry();
    // Emulation domain has no enable/disable; test actual stub commands
    ok_cmd(&reg, "Emulation.clearDeviceMetricsOverride", json!({}));
    ok_cmd(&reg, "Emulation.setTouchEmulationEnabled", json!({}));
}

#[test]
fn test_css_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "CSS.enable", json!({}));
    ok_cmd(&reg, "CSS.disable", json!({}));
}

#[test]
fn test_overlay_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "Overlay.enable", json!({}));
    ok_cmd(&reg, "Overlay.disable", json!({}));
}

#[test]
fn test_log_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "Log.enable", json!({}));
    ok_cmd(&reg, "Log.disable", json!({}));
}

#[test]
fn test_fetch_enable_disable() {
    let reg = make_registry();
    ok_cmd(&reg, "Fetch.enable", json!({}));
    ok_cmd(&reg, "Fetch.disable", json!({}));
}

// ---- Unknown command returns proper error ----

#[test]
fn test_page_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Page.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601, "unknown method should return -32601");
}

#[test]
fn test_runtime_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Runtime.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

#[test]
fn test_dom_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "DOM.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

#[test]
fn test_network_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Network.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

#[test]
fn test_debugger_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Debugger.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

#[test]
fn test_input_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Input.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

#[test]
fn test_emulation_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Emulation.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

#[test]
fn test_css_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "CSS.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

#[test]
fn test_overlay_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Overlay.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

#[test]
fn test_log_unknown_command() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Log.nonexistentMethod", json!({}));
    assert_eq!(err.code, -32601);
}

// ---- Unknown domain returns None ----

#[test]
fn test_unknown_domain_returns_none() {
    let reg = make_registry();
    assert!(cmd(&reg, "UnknownDomain.method", json!({})).is_none(),
        "unknown domain should return None");
}

#[test]
fn test_empty_domain_returns_none() {
    let reg = make_registry();
    assert!(cmd(&reg, "", json!({})).is_none());
}

#[test]
fn test_no_dot_returns_none() {
    let reg = make_registry();
    assert!(cmd(&reg, "PageNoDot", json!({})).is_none());
}

// ---- Domain-specific command results ----

#[test]
fn test_page_needs_bridge_for_navigate() {
    let reg = make_registry();
    // Page.navigate requires active bridge; without servo it returns -32603
    let err = err_cmd(&reg, "Page.navigate", json!({"url": "https://example.com"}));
    assert_eq!(err.code, -32603, "bridge command should return internal error without servo");
}

#[test]
fn test_runtime_needs_bridge_for_evaluate() {
    let reg = make_registry();
    // Runtime.evaluate requires active bridge; returns -32603 without servo
    let err = err_cmd(&reg, "Runtime.evaluate", json!({"expression": "1+1"}));
    assert_eq!(err.code, -32603, "bridge command should return internal error without servo");
}

#[test]
fn test_dom_needs_bridge_for_get_document() {
    let reg = make_registry();
    // DOM.getDocument requires active bridge; returns -32603 without servo
    let err = err_cmd(&reg, "DOM.getDocument", json!({}));
    assert_eq!(err.code, -32603, "bridge command should return internal error without servo");
}

#[test]
fn test_network_get_cookies_returns_array() {
    let reg = make_registry();
    let result = ok_cmd(&reg, "Network.getCookies", json!({}));
    assert!(result.get("cookies").is_some(), "Network.getCookies should return cookies");
}

#[test]
fn test_runtime_call_function_on() {
    let reg = make_registry();
    let result = ok_cmd(&reg, "Runtime.callFunctionOn", json!({
        "functionDeclaration": "function() { return 42; }",
        "executionContextId": 1
    }));
    assert!(result.is_object());
}

// ---- Multiple enable calls are idempotent ----

#[test]
fn test_double_enable_is_idempotent() {
    let reg = make_registry();
    ok_cmd(&reg, "Page.enable", json!({}));
    ok_cmd(&reg, "Page.enable", json!({}));
    ok_cmd(&reg, "Page.disable", json!({}));
}

#[test]
fn test_disable_without_enable_ok() {
    let reg = make_registry();
    ok_cmd(&reg, "Page.disable", json!({}));
    ok_cmd(&reg, "Runtime.disable", json!({}));
    ok_cmd(&reg, "Network.disable", json!({}));
}

// ---- Error message format ----

#[test]
fn test_error_message_contains_method_name() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Page.nonexistentMethod", json!({}));
    assert!(err.message.contains("nonexistentMethod") || err.message.contains("wasn't found"),
        "error message should reference the unknown method, got: {}", err.message);
}

#[test]
fn test_error_message_contains_domain_name() {
    let reg = make_registry();
    let err = err_cmd(&reg, "Runtime.nonexistentMethod", json!({}));
    assert!(err.message.contains("Runtime") || err.message.contains("nonexistentMethod"),
        "error should reference domain or method");
}

// ---- has_domain for registered domains ----

#[test]
fn test_has_registered_domains() {
    let reg = make_registry();
    let domains = ["Page", "Runtime", "DOM", "Network", "Debugger", "Input",
                   "Emulation", "CSS", "Overlay", "Log", "Fetch"];
    for d in &domains {
        assert!(reg.has_domain(d), "domain '{}' should be registered", d);
    }
}

#[test]
fn test_has_unregistered_domains() {
    let reg = make_registry();
    assert!(!reg.has_domain("Unknown"));
    assert!(!reg.has_domain("Accessibility"));
    assert!(!reg.has_domain("HeapProfiler"));
}
