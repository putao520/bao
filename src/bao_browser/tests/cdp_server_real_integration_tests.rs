// @trace TEST-BRW-015 [req:REQ-BRW-001,REQ-CDP-001~008,REQ-CDS-001~008] [level:integration]
// Real CdpServer integration: 12-domain registry dispatch, TargetProvider, bridge channel.

use std::sync::Arc;
use std::time::Duration;

use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeResponse};
use bao_cdp::domains::{register_all_domains_with_target, ServoTargetProvider};
use cdp_server::{CdpServer, ServerConfig, TargetProvider};
use serde_json::json;

fn mock_responder(
    receiver: bao_cdp::servo_bridge::BridgeReceiver,
    title: &'static str,
    url: &'static str,
    done: Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for _ in 0..500 {
            if done.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let _ = receiver.try_process(|cmd| match cmd {
                BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!(title)) },
                BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!(url)) },
                BridgeCommand::Navigate { .. } => BridgeResponse { result: Ok(json!({ "frameId": "0" })) },
                BridgeCommand::EvaluateJs { .. } => BridgeResponse { result: Ok(json!({ "result": { "type": "number", "value": 42 } })) },
                BridgeCommand::ClosePage => BridgeResponse { result: Ok(json!({})) },
                _ => BridgeResponse { result: Ok(json!({})) },
            });
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    })
}

struct NoopSender;
impl cdp_server::EventSender for NoopSender {
    fn send_event(&self, _method: &str, _params: serde_json::Value) {}
}

// ---------------------------------------------------------------------------
// Test 1: Registry has all 12 domains after server creation
// ---------------------------------------------------------------------------

#[test]
fn cdp_server_registry_has_all_12_domains() {
    let (bridge_tx, _rx) = bridge_channel(Duration::from_millis(100));
    let config = ServerConfig::builder().host("127.0.0.1").port(0).build();
    let server = CdpServer::new(config);
    register_all_domains_with_target(bridge_tx, "registry-test".into(), server.registry());

    let expected = [
        "Page", "Runtime", "DOM", "Network", "Debugger",
        "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch", "Target",
    ];
    for domain in &expected {
        assert!(server.registry().has_domain(domain), "Missing domain: {}", domain);
    }
}

// ---------------------------------------------------------------------------
// Test 2: ServoTargetProvider returns consistent target_id via CdpServer
// ---------------------------------------------------------------------------

#[test]
fn target_provider_consistent_id_via_server() {
    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(2));
    let target_id = "consistency-check-id".to_string();

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _responder = mock_responder(bridge_rx, "Consistent", "https://consistent.test", done.clone());

    let _server = CdpServer::new(ServerConfig::default());
    let provider = Arc::new(ServoTargetProvider::new(
        bridge_tx, target_id.clone(), "127.0.0.1".into(), 9222,
    ));

    let targets = provider.list_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, target_id);
    assert_eq!(targets[0].title, "Consistent");
    assert_eq!(targets[0].url, "https://consistent.test");

    done.store(true, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Test 3: Multiple commands through registry dispatch
// ---------------------------------------------------------------------------

#[test]
fn multiple_commands_through_registry_dispatch() {
    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(2));
    let target_id = "multi-cmd-target".to_string();

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _responder = mock_responder(bridge_rx, "Multi", "https://multi.test", done.clone());

    let server = CdpServer::new(ServerConfig::default());
    register_all_domains_with_target(bridge_tx, target_id, server.registry());

    // Page.enable
    let r = server.registry().dispatch_command("Page.enable", json!({}), &NoopSender);
    assert!(r.unwrap().is_ok());

    // Page.navigate
    let r = server.registry().dispatch_command("Page.navigate", json!({ "url": "https://multi.test/page" }), &NoopSender);
    let resp = r.unwrap().unwrap();
    assert!(resp.get("frameId").is_some());
    assert!(resp.get("loaderId").is_some());

    // Runtime.evaluate
    let r = server.registry().dispatch_command("Runtime.evaluate", json!({ "expression": "42" }), &NoopSender);
    let resp = r.unwrap().unwrap();
    assert_eq!(resp["result"]["value"].as_i64().unwrap(), 42);

    // Target.getTargets
    let r = server.registry().dispatch_command("Target.getTargets", json!({}), &NoopSender);
    let resp = r.unwrap().unwrap();
    let infos = resp["targetInfos"].as_array().unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0]["title"].as_str().unwrap(), "Multi");

    // Network.enable
    let r = server.registry().dispatch_command("Network.enable", json!({}), &NoopSender);
    assert!(r.unwrap().is_ok());

    // DOM.enable
    let r = server.registry().dispatch_command("DOM.enable", json!({}), &NoopSender);
    assert!(r.unwrap().is_ok());

    // CSS.enable
    let r = server.registry().dispatch_command("CSS.enable", json!({}), &NoopSender);
    assert!(r.unwrap().is_ok());

    done.store(true, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Test 4: TargetProvider close_target integration
// ---------------------------------------------------------------------------

#[test]
fn target_provider_close_target() {
    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(2));
    let target_id = "close-target-test".to_string();

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _responder = mock_responder(bridge_rx, "CloseTest", "https://close.test", done.clone());

    let provider = ServoTargetProvider::new(
        bridge_tx, target_id.clone(), "127.0.0.1".into(), 9222,
    );

    let targets = provider.list_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, target_id);

    let result = provider.close_target(&target_id);
    assert!(result.is_ok());

    done.store(true, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Test 5: CdpServer config builder + target_provider integration
// ---------------------------------------------------------------------------

#[test]
fn cdp_server_config_builder_with_target_provider() {
    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(2));
    let target_id = "ws-url-test".to_string();

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _responder = mock_responder(bridge_rx, "WS Test", "https://ws.test", done.clone());

    let config = ServerConfig::builder().host("127.0.0.1").port(9333).build();
    let mut server = CdpServer::new(config);
    register_all_domains_with_target(bridge_tx.clone(), target_id.clone(), server.registry());

    let provider = Arc::new(ServoTargetProvider::new(
        bridge_tx,
        target_id.clone(),
        "127.0.0.1".into(),
        9333,
    ));

    // Verify provider directly before setting on server
    let targets = provider.list_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, target_id);

    server.set_target_provider(provider);

    done.store(true, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Test 6: CdpServer default config creates successfully
// ---------------------------------------------------------------------------

#[test]
fn cdp_server_default_config_creates() {
    let server = CdpServer::new(ServerConfig::default());
    assert!(!server.registry().has_domain("Page"));
}

// ---------------------------------------------------------------------------
// Test 7: All domain enable/disable cycle via registry
// ---------------------------------------------------------------------------

#[test]
fn all_domain_enable_disable_cycle() {
    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(2));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _responder = mock_responder(bridge_rx, "Cycle", "https://cycle.test", done.clone());

    let server = CdpServer::new(ServerConfig::default());
    register_all_domains_with_target(bridge_tx, "cycle-target".into(), server.registry());

    let enable_commands = [
        "Page.enable", "Runtime.enable", "DOM.enable", "Network.enable",
        "Debugger.enable", "Emulation.clearDeviceMetricsOverride",
        "CSS.enable", "Overlay.enable", "Log.enable", "Fetch.disable",
    ];
    for cmd in &enable_commands {
        let r = server.registry().dispatch_command(cmd, json!({}), &NoopSender);
        assert!(r.is_some(), "{} should return Some", cmd);
        assert!(r.unwrap().is_ok(), "{} should return Ok", cmd);
    }

    done.store(true, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Test 8: Target domain returns consistent IDs across getTargets + attachToTarget
// ---------------------------------------------------------------------------

#[test]
fn target_domain_consistent_ids_across_commands() {
    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(2));
    let shared_id = "shared-id-0xdeadbeef".to_string();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _responder = mock_responder(bridge_rx, "Shared", "https://shared.test", done.clone());

    let server = CdpServer::new(ServerConfig::default());
    register_all_domains_with_target(bridge_tx, shared_id.clone(), server.registry());

    let r = server.registry().dispatch_command("Target.getTargets", json!({}), &NoopSender);
    let resp = r.unwrap().unwrap();
    let infos = resp["targetInfos"].as_array().unwrap();
    assert_eq!(infos[0]["targetId"].as_str().unwrap(), shared_id);

    let r = server.registry().dispatch_command(
        "Target.attachToTarget",
        json!({ "targetId": shared_id }),
        &NoopSender,
    );
    let resp = r.unwrap().unwrap();
    let session_id = resp["sessionId"].as_str().unwrap();
    assert!(!session_id.is_empty());

    done.store(true, std::sync::atomic::Ordering::Relaxed);
}
