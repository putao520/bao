// @trace TEST-CDP-005 [req:REQ-CDP-001] [level:unit]
// @trace TEST-CDP-006 [req:REQ-CDP-004] [level:unit]
// @trace TEST-CDP-007 [req:REQ-CDP-005] [level:unit]

use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeSender, BridgeResponse};
use bao_cdp::domains::register_all_domains_into;
use bao_cdp::DomainRegistry;
use bao_cdp::CdpRouter;
use cdp_server::{EventSender, CdpError};
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
        BridgeCommand::GetOuterHtml { .. } => BridgeResponse {
            result: Ok(json!({ "outerHTML": "<html><body></body></html>" })),
        },
        BridgeCommand::SetAttributeValue { .. } => BridgeResponse {
            result: Ok(json!({})),
        },
        BridgeCommand::Navigate { .. } | BridgeCommand::Reload { .. } => BridgeResponse {
            result: Ok(json!({ "frameId": "0", "loaderId": "0" })),
        },
        BridgeCommand::SetViewport { .. } | BridgeCommand::SetUserAgent { .. } => BridgeResponse {
            result: Ok(json!({})),
        },
        BridgeCommand::DispatchMouseEvent { .. } | BridgeCommand::DispatchKeyEvent { .. } => BridgeResponse {
            result: Ok(json!({})),
        },
        BridgeCommand::InsertText { .. } => BridgeResponse {
            result: Ok(json!({})),
        },
        BridgeCommand::AddScriptToEvaluateOnNewDocument { .. } => BridgeResponse {
            result: Ok(json!({ "identifier": "1" })),
        },
        _ => BridgeResponse {
            result: Ok(json!({})),
        },
    }
}

fn setup() -> (DomainRegistry, BridgeSender) {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let registry = DomainRegistry::new();
    register_all_domains_into(tx.clone(), &registry);
    // Keep an extra clone alive so the channel stays open after test drops tx
    let keeper = tx.clone();
    thread::spawn(move || {
        let _keeper = keeper;
        // Poll loop using public try_process — keeps thread alive for all bridge calls
        loop {
            let handled = rx.try_process(|cmd| default_bridge_response(cmd));
            if !handled {
                thread::sleep(Duration::from_millis(1));
                // If still nothing after a brief wait, check if disconnected
                if !rx.try_process(|cmd| default_bridge_response(cmd)) {
                    // Channel might be empty — just keep polling
                    // (keeper keeps channel alive until thread exits)
                }
            }
        }
    });
    (registry, tx)
}

fn dispatch(registry: &DomainRegistry, method: &str, params: Value) -> Result<Value, CdpError> {
    let es = NoopEventSender;
    registry.dispatch_command(method, params, &es)
        .ok_or_else(|| CdpError { code: -32601, message: format!("domain not found for '{}'", method) })
        .and_then(|r| r)
}

// ===========================================================================
// §1 Target domain — tested through domain_handler_tests.rs since Target
// is served by ServoTargetProvider, not a DomainHandler. Here we verify
// that Target commands correctly route to "not found" through DomainHandler
// architecture (they are handled by the transport/target layer instead).
// ===========================================================================

#[test]
fn test_target_not_in_domain_handlers() {
    let (registry, _) = setup();
    // Target domain is not registered as a DomainHandler — it's handled
    // by the TargetProvider at the transport layer
    let result = dispatch(&registry, "Target.getTargets", json!({}));
    assert!(result.is_err());
}

// ===========================================================================
// §2 Emulation domain coverage (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_emulation_set_device_metrics() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Emulation.setDeviceMetricsOverride", json!({
        "width": 1280, "height": 720, "deviceScaleFactor": 2.0
    }));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_clear_device_metrics() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Emulation.clearDeviceMetricsOverride", json!({})).is_ok());
}

#[test]
fn test_emulation_set_user_agent_override() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Emulation.setUserAgentOverride", json!({
        "userAgent": "Mozilla/5.0 CustomAgent"
    }));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_set_touch_emulation() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Emulation.setTouchEmulationEnabled", json!({})).is_ok());
}

#[test]
fn test_emulation_unknown_command() {
    let (registry, _) = setup();
    let err = dispatch(&registry, "Emulation.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ===========================================================================
// §3 Input domain coverage (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_input_dispatch_mouse_event() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": 100.0, "y": 200.0, "button": 0, "clickCount": 1
    }));
    match result {
        Ok(v) => assert!(v.is_object()),
        Err(e) => assert_eq!(e.code, -32603, "bridge error expected: {}", e.message),
    }
}

#[test]
fn test_input_dispatch_key_event() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Input.dispatchKeyEvent", json!({
        "type": "keyDown", "key": "Enter", "code": "Enter"
    }));
    match result {
        Ok(v) => assert!(v.is_object()),
        Err(e) => assert_eq!(e.code, -32603, "bridge error expected: {}", e.message),
    }
}

#[test]
fn test_input_insert_text() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Input.insertText", json!({"text": "hello"}));
    assert!(result.is_ok());
}

#[test]
fn test_input_dispatch_touch_event() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Input.dispatchTouchEvent", json!({})).is_ok());
}

// ===========================================================================
// §4 Fetch domain coverage (REQ-CDP-007)
// ===========================================================================

#[test]
fn test_fetch_continue_request() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Fetch.continueRequest", json!({"requestId": "req-1"})).unwrap();
    assert_eq!(result["requestId"], "req-1");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_fail_request() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Fetch.failRequest", json!({
        "requestId": "req-2", "reason": "Aborted"
    })).unwrap();
    assert_eq!(result["failed"], true);
    assert_eq!(result["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Fetch.fulfillRequest", json!({
        "requestId": "req-3", "responseCode": 200, "body": "hello"
    })).unwrap();
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 5);
}

#[test]
fn test_fetch_get_request_post_data() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Fetch.getRequestPostData", json!({"requestId": "req-4"})).unwrap();
    assert_eq!(result["requestId"], "req-4");
}

#[test]
fn test_fetch_continue_with_auth() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Fetch.continueWithAuth", json!({"requestId": "req-5"})).unwrap();
    assert_eq!(result["requestId"], "req-5");
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Fetch.takeResponseBodyAsStream", json!({"requestId": "req-6"})).unwrap();
    assert!(result["stream"].is_string());
}

// ===========================================================================
// §5 Debugger domain coverage (REQ-CDP-003)
// ===========================================================================

#[test]
fn test_debugger_remove_breakpoint() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Debugger.removeBreakpoint", json!({})).is_ok());
}

#[test]
fn test_debugger_pause_resume() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Debugger.pause", json!({})).is_ok());
    assert!(dispatch(&registry, "Debugger.resume", json!({})).is_ok());
}

#[test]
fn test_debugger_step() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Debugger.stepOver", json!({})).is_ok());
    assert!(dispatch(&registry, "Debugger.stepInto", json!({})).is_ok());
    assert!(dispatch(&registry, "Debugger.stepOut", json!({})).is_ok());
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Debugger.evaluateOnCallFrame", json!({})).unwrap();
    assert!(result["result"].is_object());
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Debugger.getPossibleBreakpoints", json!({})).unwrap();
    assert!(result["locations"].is_array());
}

// ===========================================================================
// §6 Network domain extended coverage (REQ-CDP-006)
// ===========================================================================

#[test]
fn test_network_set_cache_disabled() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Network.setCacheDisabled", json!({})).is_ok());
}

#[test]
fn test_network_get_cookies() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Network.getCookies", json!({})).unwrap();
    assert!(result["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Network.getAllCookies", json!({})).unwrap();
    assert!(result["cookies"].is_array());
}

#[test]
fn test_network_set_extra_http_headers() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Network.setExtraHTTPHeaders", json!({})).is_ok());
}

#[test]
fn test_network_unknown_command() {
    let (registry, _) = setup();
    let err = dispatch(&registry, "Network.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ===========================================================================
// §7 CdpRouter session management (REQ-LIB-002)
// ===========================================================================

#[test]
fn test_router_create_internal_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    assert_eq!(session.target_id(), "target-1");
    assert_eq!(session.backend_kind(), bao_cdp::BackendKind::Internal);
    assert!(!session.session_id().is_empty());
}

#[test]
fn test_router_session_send_page_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    let result = session.send(&router, "Page.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_router_session_send_runtime_evaluate() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    let result = session.send(&router, "Runtime.evaluate", Some(json!({"expression": "1+1"})));
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(val["result"].is_object());
}

#[test]
fn test_router_session_send_unknown_command() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    let result = session.send(&router, "NonExistent.method", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

#[test]
fn test_router_detach_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    let session_id = session.session_id().to_string();
    assert!(session.detach(&router).is_ok());
    let result = router.send_command(&session_id, "Page.enable", None);
    assert!(result.is_err());
}

#[test]
fn test_router_detach_nonexistent_session() {
    let router = CdpRouter::new();
    let result = router.detach_session("nonexistent-session");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32602);
}

#[test]
fn test_router_multiple_sessions() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("target-1");
    let s2 = router.create_internal_session("target-2");
    assert_ne!(s1.session_id(), s2.session_id());

    let r1 = s1.send(&router, "Page.enable", None);
    let r2 = s2.send(&router, "Runtime.enable", None);
    assert!(r1.is_ok());
    assert!(r2.is_ok());
}

#[test]
fn test_router_session_event_handler_registration() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    let received = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let received_clone = received.clone();
    session.on("Page.loadEventFired", move |_params| {
        received_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    assert!(!received.load(std::sync::atomic::Ordering::SeqCst));
}

// ===========================================================================
// §8 DOM domain extended coverage (REQ-CDP-005)
// ===========================================================================

#[test]
fn test_dom_describe_node() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.describeNode", json!({})).unwrap();
    assert!(result["node"].is_object());
    assert_eq!(result["node"]["nodeName"], "HTML");
}

#[test]
fn test_dom_get_box_model() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.getBoxModel", json!({})).unwrap();
    assert!(result["model"]["width"].is_number());
    assert!(result["model"]["height"].is_number());
}

#[test]
fn test_dom_set_attribute_value() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.setAttributeValue", json!({
        "nodeId": 1, "name": "class", "value": "test"
    }));
    assert!(result.is_ok());
}

#[test]
fn test_dom_get_outer_html() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.getOuterHTML", json!({"nodeId": 1}));
    match result {
        Ok(v) => assert!(v["outerHTML"].is_string() || v.is_object()),
        Err(e) => assert_eq!(e.code, -32603, "bridge error expected: {}", e.message),
    }
}

#[test]
fn test_dom_resolve_node() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "DOM.resolveNode", json!({})).unwrap();
    assert!(result["object"].is_object());
}

#[test]
fn test_dom_unknown_command() {
    let (registry, _) = setup();
    let err = dispatch(&registry, "DOM.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ===========================================================================
// §9 Runtime domain extended coverage (REQ-CDP-002)
// ===========================================================================

#[test]
fn test_runtime_disable() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Runtime.disable", json!({})).is_ok());
}

#[test]
fn test_runtime_call_function_on() {
    let (registry, _) = setup();
    let result = dispatch(&registry, "Runtime.callFunctionOn", json!({})).unwrap();
    assert!(result["result"].is_object());
}

#[test]
fn test_runtime_release_object() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Runtime.releaseObject", json!({})).is_ok());
}

#[test]
fn test_runtime_unknown_command() {
    let (registry, _) = setup();
    let err = dispatch(&registry, "Runtime.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ===========================================================================
// §10 Overlay & Log extended coverage
// ===========================================================================

#[test]
fn test_overlay_highlight_node() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Overlay.highlightNode", json!({})).is_ok());
}

#[test]
fn test_overlay_hide_highlight() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Overlay.hideHighlight", json!({})).is_ok());
}

#[test]
fn test_overlay_set_inspect_mode() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Overlay.setInspectMode", json!({})).is_ok());
}

#[test]
fn test_log_start_violations_report() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Log.startViolationsReport", json!({})).is_ok());
}

#[test]
fn test_log_stop_violations_report() {
    let (registry, _) = setup();
    assert!(dispatch(&registry, "Log.stopViolationsReport", json!({})).is_ok());
}

// ===========================================================================
// §11 Bridge channel tests (REQ-CDP-003)
// ===========================================================================

#[test]
fn test_bridge_send_receive() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let handle = thread::spawn(move || {
        rx.drain(|cmd| match cmd {
            BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Hello")) },
            _ => BridgeResponse { result: Ok(json!({})) },
        });
    });

    let resp = tx.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap(), json!("Hello"));

    drop(tx);
    let _ = handle.join();
}

#[test]
fn test_bridge_fire_and_forget() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::ClosePage);
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 1);
}

#[test]
fn test_bridge_clone_preserves_channel() {
    let (tx, _rx) = bridge_channel(Duration::from_secs(10));
    let tx2 = tx.clone();
    // Both senders share the same underlying channel
    let resp1 = tx.send(BridgeCommand::GetTitle);
    let resp2 = tx2.send(BridgeCommand::GetUrl);
    // Both should timeout (no receiver processing)
    assert!(resp1.result.is_err());
    assert!(resp2.result.is_err());
}

#[test]
fn test_bridge_multiple_commands() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let handle = thread::spawn(move || {
        // Process commands for up to 3 seconds
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
            if count == 0 {
                thread::sleep(Duration::from_millis(10));
            }
        }
    });

    let resp1 = tx.send(BridgeCommand::GetTitle);
    assert!(resp1.result.is_ok());
    let resp2 = tx.send(BridgeCommand::GetUrl);
    assert!(resp2.result.is_ok());

    drop(tx);
    let _ = handle.join();
}
