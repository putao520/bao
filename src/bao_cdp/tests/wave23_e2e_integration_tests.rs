// @trace TEST-WAVE23-E2E [req:REQ-CDP-001~008,REQ-CDS-001~008] [level:integration]
// Wave 2-3 E2E integration: DomainHandler registry dispatch, Target domain,
// cross-domain routing, event broadcasting, bridge channel relay.

use bao_cdp::domains::{register_all_domains_with_target, ServoTargetProvider};
use bao_cdp::{BridgeCommand, BridgeResponse, bridge_channel};
use cdp_server::{DomainRegistry, EventBroadcaster, TargetProvider};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(500);

struct NoopSender;
impl cdp_server::EventSender for NoopSender {
    fn send_event(&self, _method: &str, _params: serde_json::Value) {}
}

fn mock_responder(
    receiver: bao_cdp::BridgeReceiver,
    done: Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for _ in 0..100 {
            if done.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let _ = receiver.try_process(|cmd| match cmd {
                BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Wave23 Page")) },
                BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!("https://wave23.test")) },
                BridgeCommand::Navigate { .. } => BridgeResponse { result: Ok(json!({ "frameId": "f1" })) },
                BridgeCommand::EvaluateJs { .. } => BridgeResponse { result: Ok(json!({ "result": { "type": "number", "value": 42 } })) },
                BridgeCommand::ClosePage => BridgeResponse { result: Ok(json!({})) },
                _ => BridgeResponse { result: Ok(json!({})) },
            });
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    })
}

// ---------------------------------------------------------------------------
// Test 1: Full 12-domain registry dispatch chain
// ---------------------------------------------------------------------------

#[test]
fn full_12_domain_registry_dispatch_chain() {
    let (bridge, rx) = bridge_channel(TIMEOUT);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let responder = mock_responder(rx, done.clone());

    let registry = DomainRegistry::new();
    register_all_domains_with_target(bridge, "e2e-target-id".into(), &registry);

    let expected = [
        "Page", "Runtime", "DOM", "Network", "Debugger",
        "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch", "Target",
    ];
    for domain in &expected {
        assert!(registry.has_domain(domain), "domain '{}' should be registered", domain);
    }

    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = responder.join();
}

// ---------------------------------------------------------------------------
// Test 2: Cross-domain command routing with -32601 for unknown
// ---------------------------------------------------------------------------

#[test]
fn cross_domain_command_routing_unknown_returns_32601() {
    let (bridge, rx) = bridge_channel(TIMEOUT);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let responder = mock_responder(rx, done.clone());

    let registry = DomainRegistry::new();
    register_all_domains_with_target(bridge, "routing-target".into(), &registry);

    let known_commands: &[(&str, serde_json::Value)] = &[
        ("Page.enable", json!({})),
        ("Runtime.enable", json!({})),
        ("DOM.enable", json!({})),
        ("Network.enable", json!({})),
        ("Debugger.enable", json!({})),
        ("Input.setIgnoreInputEvents", json!({})),
        ("Emulation.clearDeviceMetricsOverride", json!({})),
        ("CSS.enable", json!({})),
        ("Overlay.enable", json!({})),
        ("Log.enable", json!({})),
        ("Fetch.disable", json!({})),
        ("Target.getTargets", json!({})),
    ];

    for (cmd, params) in known_commands {
        let result = registry.dispatch_command(cmd, params.clone(), &NoopSender);
        assert!(result.is_some(), "{} should return Some", cmd);
        assert!(result.unwrap().is_ok(), "{} should return Ok", cmd);
    }

    let unknown_commands = &["Page.nonExistent", "Target.nonExistent", "Runtime.badMethod"];
    for cmd in unknown_commands {
        let result = registry.dispatch_command(cmd, json!({}), &NoopSender);
        assert!(result.is_some(), "{} should return Some (domain exists)", cmd);
        let err = result.unwrap().unwrap_err();
        assert_eq!(err.code, -32601, "{} should return -32601", cmd);
    }

    let result = registry.dispatch_command("HeapProfiler.takeHeapSnapshot", json!({}), &NoopSender);
    assert!(result.is_none(), "unregistered domain should return None");

    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = responder.join();
}

// ---------------------------------------------------------------------------
// Test 3: Target domain end-to-end (shared target_id)
// ---------------------------------------------------------------------------

#[test]
fn target_handler_and_provider_share_id_e2e() {
    let (sender, rx) = bridge_channel(TIMEOUT);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let responder = mock_responder(rx, done.clone());

    let shared_id = "shared-e2e-0000deadbeef".to_string();

    let provider = ServoTargetProvider::new(sender.clone(), shared_id.clone(), "127.0.0.1".into(), 9222);

    let registry = DomainRegistry::new();
    register_all_domains_with_target(sender, shared_id.clone(), &registry);

    let targets = provider.list_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, shared_id);

    let result = registry.dispatch_command("Target.getTargets", json!({}), &NoopSender);
    let resp = result.unwrap().unwrap();
    let infos = resp["targetInfos"].as_array().unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0]["targetId"].as_str().unwrap(), shared_id);
    assert_eq!(infos[0]["title"].as_str().unwrap(), "Wave23 Page");
    assert_eq!(infos[0]["url"].as_str().unwrap(), "https://wave23.test");

    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = responder.join();
}

// ---------------------------------------------------------------------------
// Test 4: Event broadcasting through registry dispatch
// ---------------------------------------------------------------------------

#[test]
fn event_broadcasting_through_registry_dispatch() {
    let (bridge, rx) = bridge_channel(TIMEOUT);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let responder = mock_responder(rx, done.clone());

    let registry = DomainRegistry::new();
    register_all_domains_with_target(bridge, "event-target".into(), &registry);

    let sessions: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<std::sync::Mutex<cdp_server::CdpSession>>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let broadcaster = EventBroadcaster::new(sessions);

    // Dispatch commands with the broadcaster — should not panic
    let sender = broadcaster.sender();
    let sender_ref: &dyn cdp_server::EventSender = sender.as_ref();
    let result = registry.dispatch_command("Page.enable", json!({}), sender_ref);
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());

    let result = registry.dispatch_command("Runtime.enable", json!({}), sender_ref);
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());

    let result = registry.dispatch_command("Target.getTargets", json!({}), sender_ref);
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());

    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = responder.join();
}

// ---------------------------------------------------------------------------
// Test 5: Bridge channel relay (navigate + evaluate + close fire-and-forget)
// ---------------------------------------------------------------------------

#[test]
fn bridge_channel_relay_navigate_evaluate_close() {
    let (bridge, rx) = bridge_channel(TIMEOUT);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let responder = mock_responder(rx, done.clone());

    let registry = DomainRegistry::new();
    register_all_domains_with_target(bridge.clone(), "relay-target".into(), &registry);

    // Page.navigate sends Navigate to bridge, returns synthetic frameId
    let result = registry.dispatch_command(
        "Page.navigate",
        json!({ "url": "https://wave23.test/page" }),
        &NoopSender,
    );
    let resp = result.unwrap().unwrap();
    assert!(resp.get("frameId").is_some());
    assert!(resp.get("loaderId").is_some());

    // Runtime.evaluate goes through bridge and returns mock response
    let result = registry.dispatch_command(
        "Runtime.evaluate",
        json!({ "expression": "1+1" }),
        &NoopSender,
    );
    let resp = result.unwrap().unwrap();
    assert_eq!(resp["result"]["value"].as_i64().unwrap(), 42);

    // Target.closeTarget is fire-and-forget via bridge
    let result = registry.dispatch_command("Target.closeTarget", json!({ "targetId": "relay-target" }), &NoopSender);
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());

    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = responder.join();
}

// ---------------------------------------------------------------------------
// Test 6: Target.attachToTarget generates deterministic session_id
// ---------------------------------------------------------------------------

#[test]
fn target_attach_generates_session_id() {
    let (bridge, rx) = bridge_channel(TIMEOUT);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let responder = mock_responder(rx, done.clone());

    let registry = DomainRegistry::new();
    register_all_domains_with_target(bridge, "attach-target-123".into(), &registry);

    let result = registry.dispatch_command("Target.attachToTarget", json!({ "targetId": "attach-target-123" }), &NoopSender);
    let resp = result.unwrap().unwrap();
    let session_id = resp["sessionId"].as_str().unwrap();
    assert!(!session_id.is_empty());
    assert_eq!(session_id.len(), 16); // format!("{:016x}", ...)

    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = responder.join();
}

// ---------------------------------------------------------------------------
// Test 7: Target.setAutoAttach + setDiscoverTargets are no-ops
// ---------------------------------------------------------------------------

#[test]
fn target_auto_attach_and_discover_are_noop() {
    let (bridge, rx) = bridge_channel(TIMEOUT);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let responder = mock_responder(rx, done.clone());

    let registry = DomainRegistry::new();
    register_all_domains_with_target(bridge, "noop-target".into(), &registry);

    let result = registry.dispatch_command("Target.setAutoAttach", json!({ "autoAttach": true, "waitForDebuggerOnStart": false }), &NoopSender);
    assert_eq!(result.unwrap().unwrap(), json!({}));

    let result = registry.dispatch_command("Target.setDiscoverTargets", json!({ "discover": true }), &NoopSender);
    assert_eq!(result.unwrap().unwrap(), json!({}));

    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = responder.join();
}
