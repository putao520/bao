// @trace TEST-IMPL-06 [req:REQ-IMPL-06] [level:integration]
// Integration tests: cdp-server generic layer + bao_cdp DomainHandlers.

use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeResponse};
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
            result: Ok(json!("Integration Page")),
        },
        BridgeCommand::GetUrl => BridgeResponse {
            result: Ok(json!("https://integration.test")),
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
            result: Ok(json!({ "result": { "type": "number", "value": 42 } })),
        },
        BridgeCommand::GetOuterHtml { .. } => BridgeResponse {
            result: Ok(json!({ "outerHTML": "<html><body>Integration</body></html>" })),
        },
        BridgeCommand::SetAttributeValue { .. } => BridgeResponse {
            result: Ok(json!({})),
        },
        BridgeCommand::Navigate { .. } | BridgeCommand::Reload { .. } => BridgeResponse {
            result: Ok(json!({ "frameId": "int-frame", "loaderId": "int-loader" })),
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

fn setup_registry() -> DomainRegistry {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let registry = DomainRegistry::new();
    register_all_domains_into(tx.clone(), &registry);
    thread::spawn(move || {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            let count = rx.drain(|cmd| default_bridge_response(cmd));
            if count == 0 {
                thread::sleep(Duration::from_millis(1));
            }
        }
    });
    // Keep tx alive for the duration of the test
    std::mem::forget(tx);
    registry
}

fn dispatch(registry: &DomainRegistry, method: &str, params: Value) -> Result<Value, CdpError> {
    let es = NoopEventSender;
    registry.dispatch_command(method, params, &es)
        .ok_or_else(|| CdpError { code: -32601, message: format!("domain not found for '{}'", method) })
        .and_then(|r| r)
}

// ===========================================================================
// §1 All 11 DomainHandlers registered (REQ-IMPL-06)
// ===========================================================================

#[test]
fn test_all_domains_registered() {
    let registry = setup_registry();
    let domains = [
        "Page", "Runtime", "DOM", "Network", "Debugger",
        "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch",
    ];
    for domain in &domains {
        assert!(registry.has_domain(domain), "Domain '{}' should be registered", domain);
    }
}

#[test]
fn test_unknown_domain_not_registered() {
    let registry = setup_registry();
    assert!(!registry.has_domain("NonExistent"));
    assert!(!registry.has_domain("Target"));
}

// ===========================================================================
// §2 Cross-domain command routing (REQ-IMPL-06)
// ===========================================================================

#[test]
fn test_page_navigate_routes_correctly() {
    let registry = setup_registry();
    let result = dispatch(&registry, "Page.navigate", json!({"url": "https://example.com"})).unwrap();
    assert_eq!(result["frameId"], "0");
    assert!(result["loaderId"].is_string());
}

#[test]
fn test_runtime_evaluate_routes_correctly() {
    let registry = setup_registry();
    let result = dispatch(&registry, "Runtime.evaluate", json!({"expression": "1+1"})).unwrap();
    assert!(result["result"].is_object());
}

#[test]
fn test_dom_get_document_routes_correctly() {
    let registry = setup_registry();
    let result = dispatch(&registry, "DOM.getDocument", json!({})).unwrap();
    assert!(result["root"].is_object());
}

#[test]
fn test_network_enable_routes_correctly() {
    let registry = setup_registry();
    assert!(dispatch(&registry, "Network.enable", json!({})).is_ok());
}

#[test]
fn test_debugger_pause_resume_sequence() {
    let registry = setup_registry();
    assert!(dispatch(&registry, "Debugger.enable", json!({})).is_ok());
    assert!(dispatch(&registry, "Debugger.pause", json!({})).is_ok());
    assert!(dispatch(&registry, "Debugger.resume", json!({})).is_ok());
}

#[test]
fn test_input_mouse_and_key_sequence() {
    let registry = setup_registry();
    assert!(dispatch(&registry, "Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": 100.0, "y": 200.0, "button": 0, "clickCount": 1
    })).is_ok());
    assert!(dispatch(&registry, "Input.dispatchKeyEvent", json!({
        "type": "keyDown", "key": "Enter", "code": "Enter"
    })).is_ok());
}

#[test]
fn test_emulation_device_metrics_and_ua() {
    let registry = setup_registry();
    assert!(dispatch(&registry, "Emulation.setDeviceMetricsOverride", json!({
        "width": 1280, "height": 720, "deviceScaleFactor": 2.0
    })).is_ok());
    assert!(dispatch(&registry, "Emulation.setUserAgentOverride", json!({
        "userAgent": "Integration/1.0"
    })).is_ok());
}

#[test]
fn test_fetch_domain_lifecycle() {
    let registry = setup_registry();
    let result = dispatch(&registry, "Fetch.continueRequest", json!({"requestId": "req-int-1"})).unwrap();
    assert_eq!(result["continued"], true);
    let result = dispatch(&registry, "Fetch.failRequest", json!({
        "requestId": "req-int-2", "reason": "Aborted"
    })).unwrap();
    assert_eq!(result["failed"], true);
}

// ===========================================================================
// §3 CdpRouter session + DomainRegistry integration (REQ-IMPL-06)
// ===========================================================================

#[test]
fn test_router_session_full_lifecycle() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-integration");

    assert_eq!(session.target_id(), "target-integration");
    assert_eq!(session.backend_kind(), bao_cdp::BackendKind::Internal);
    assert!(!session.session_id().is_empty());

    let r1 = session.send(&router, "Page.enable", None);
    assert!(r1.is_ok());

    let r2 = session.send(&router, "Runtime.evaluate", Some(json!({"expression": "2+2"})));
    assert!(r2.is_ok());

    assert!(session.detach(&router).is_ok());

    let r3 = router.send_command(session.session_id(), "Page.enable", None);
    assert!(r3.is_err());
}

#[test]
fn test_router_multiple_sessions_isolation() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("target-1");
    let s2 = router.create_internal_session("target-2");
    let s3 = router.create_internal_session("target-3");

    assert_ne!(s1.session_id(), s2.session_id());
    assert_ne!(s2.session_id(), s3.session_id());

    assert!(s1.send(&router, "Page.enable", None).is_ok());
    assert!(s2.send(&router, "Runtime.enable", None).is_ok());
    assert!(s3.send(&router, "DOM.getDocument", None).is_ok());

    assert!(s1.detach(&router).is_ok());
    assert!(s2.detach(&router).is_ok());
    assert!(s3.detach(&router).is_ok());
}

// ===========================================================================
// §4 Bridge channel under load (REQ-IMPL-06)
// ===========================================================================

#[test]
fn test_bridge_rapid_fire_commands() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let handle = thread::spawn(move || {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            let count = rx.drain(|cmd| BridgeResponse {
                result: Ok(match cmd {
                    BridgeCommand::GetTitle => json!("rapid"),
                    BridgeCommand::GetUrl => json!("https://rapid.test"),
                    _ => json!({}),
                }),
            });
            if count == 0 {
                thread::sleep(Duration::from_millis(1));
            }
        }
    });

    for i in 0..10 {
        let cmd = if i % 2 == 0 { BridgeCommand::GetTitle } else { BridgeCommand::GetUrl };
        let resp = tx.send(cmd);
        assert!(resp.result.is_ok(), "Command {} should succeed", i);
    }

    drop(tx);
    let _ = handle.join();
}

// ===========================================================================
// §5 Error path coverage (REQ-IMPL-06)
// ===========================================================================

#[test]
fn test_domain_handler_returns_error_for_unknown_command() {
    let registry = setup_registry();
    let err = dispatch(&registry, "Page.nonexistentMethod", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_router_unknown_command_returns_error() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-err");
    let result = session.send(&router, "FakeDomain.method", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
    session.detach(&router).unwrap();
}

// ===========================================================================
// §6 Overlay and Log domain integration (REQ-IMPL-06)
// ===========================================================================

#[test]
fn test_overlay_and_log_commands() {
    let registry = setup_registry();
    assert!(dispatch(&registry, "Overlay.highlightNode", json!({})).is_ok());
    assert!(dispatch(&registry, "Overlay.hideHighlight", json!({})).is_ok());
    assert!(dispatch(&registry, "Log.startViolationsReport", json!({})).is_ok());
    assert!(dispatch(&registry, "Log.stopViolationsReport", json!({})).is_ok());
}
