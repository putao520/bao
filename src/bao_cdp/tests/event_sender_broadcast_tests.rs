// @trace REQ-CDP-002 [api:EventSender broadcast]
// Tests for SessionEventSender replacing NoopEventSender — CDP event broadcasting

use bao_cdp::{CDPCommand, SessionEventSender};
use bao_cdp::domains::{
    RuntimeHandler, DebuggerHandler, PageHandler, NetworkHandler,
    TargetHandler, DomHandler, EmulationHandler, InputHandler,
};
use bao_cdp::servo_bridge::bridge_channel;
use cdp_server::{DomainRegistry, EventSender};
use serde_json::json;
use std::sync::mpsc::channel;
use std::time::Duration;

// --- SessionEventSender basic tests ---

#[test]
fn session_event_sender_queues_send_event_command() {
    let (cmd_tx, cmd_rx) = channel();
    let sender = SessionEventSender { cmd_tx };
    sender.send_event("Runtime.executionContextCreated", json!({"context": {"id": 1}}));
    let cmd = cmd_rx.try_recv().unwrap();
    match cmd {
        CDPCommand::SendEvent(ev) => {
            assert_eq!(ev.method, "Runtime.executionContextCreated");
            assert!(ev.params.is_some());
        }
        _ => panic!("Expected SendEvent, got {:?}", cmd),
    }
}

#[test]
fn session_event_sender_multiple_events_queued() {
    let (cmd_tx, cmd_rx) = channel();
    let sender = SessionEventSender { cmd_tx };
    sender.send_event("Page.loadEventFired", json!({}));
    sender.send_event("Debugger.scriptParsed", json!({}));
    let cmd1 = cmd_rx.try_recv().unwrap();
    let cmd2 = cmd_rx.try_recv().unwrap();
    match cmd1 {
        CDPCommand::SendEvent(ev) => assert_eq!(ev.method, "Page.loadEventFired"),
        _ => panic!("Expected SendEvent"),
    }
    match cmd2 {
        CDPCommand::SendEvent(ev) => assert_eq!(ev.method, "Debugger.scriptParsed"),
        _ => panic!("Expected SendEvent"),
    }
}

// --- Domain handler event broadcasting tests ---

fn setup_registry_with_all_domains() -> DomainRegistry {
    let registry = DomainRegistry::new();
    let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
    registry.register(Box::new(PageHandler::new(bridge.clone()))).unwrap();
    registry.register(Box::new(RuntimeHandler::new(bridge.clone()))).unwrap();
    registry.register(Box::new(DomHandler::new(bridge.clone()))).unwrap();
    registry.register(Box::new(NetworkHandler)).unwrap();
    registry.register(Box::new(DebuggerHandler)).unwrap();
    registry.register(Box::new(InputHandler::new(bridge.clone()))).unwrap();
    registry.register(Box::new(EmulationHandler::new(bridge.clone()))).unwrap();
    registry.register(Box::new(TargetHandler::new(bridge, "test-target-id".to_string()))).unwrap();
    registry
}

fn make_event_sender() -> (SessionEventSender, std::sync::mpsc::Receiver<CDPCommand>) {
    let (cmd_tx, cmd_rx) = channel();
    (SessionEventSender { cmd_tx }, cmd_rx)
}

#[test]
fn runtime_enable_broadcasts_execution_context_created() {
    let registry = setup_registry_with_all_domains();
    let (sender, cmd_rx) = make_event_sender();
    let result = registry.dispatch_command("Runtime.enable", json!({}), &sender);
    assert!(result.is_some());
    let response = result.unwrap();
    assert!(response.is_ok());
    let cmd = cmd_rx.try_recv().unwrap();
    match cmd {
        CDPCommand::SendEvent(ev) => {
            assert_eq!(ev.method, "Runtime.executionContextCreated");
            let context_id = ev.params.unwrap()["context"]["id"].as_i64().unwrap();
            assert_eq!(context_id, 1);
        }
        _ => panic!("Expected SendEvent"),
    }
}

#[test]
fn debugger_enable_broadcasts_script_parsed() {
    let registry = setup_registry_with_all_domains();
    let (sender, cmd_rx) = make_event_sender();
    let result = registry.dispatch_command("Debugger.enable", json!({}), &sender);
    assert!(result.is_some());
    let cmd = cmd_rx.try_recv().unwrap();
    match cmd {
        CDPCommand::SendEvent(ev) => {
            assert_eq!(ev.method, "Debugger.scriptParsed");
            let params = ev.params.unwrap();
            assert_eq!(params["scriptId"], "1");
            assert_eq!(params["executionContextId"], 1);
        }
        _ => panic!("Expected SendEvent"),
    }
}

#[test]
fn target_set_discover_targets_true_broadcasts_target_created() {
    let registry = setup_registry_with_all_domains();
    let (sender, cmd_rx) = make_event_sender();
    let result = registry.dispatch_command(
        "Target.setDiscoverTargets",
        json!({ "discover": true }),
        &sender,
    );
    assert!(result.is_some());
    let cmd = cmd_rx.try_recv().unwrap();
    match cmd {
        CDPCommand::SendEvent(ev) => {
            assert_eq!(ev.method, "Target.targetCreated");
            let params = ev.params.unwrap();
            let tid = params["targetInfo"]["targetId"].as_str().unwrap().to_string();
            assert_eq!(tid, "test-target-id");
        }
        _ => panic!("Expected SendEvent"),
    }
}

#[test]
fn target_set_discover_targets_false_no_event() {
    let registry = setup_registry_with_all_domains();
    let (sender, cmd_rx) = make_event_sender();
    let result = registry.dispatch_command(
        "Target.setDiscoverTargets",
        json!({ "discover": false }),
        &sender,
    );
    assert!(result.is_some());
    assert!(cmd_rx.try_recv().is_err(), "No event should be sent when discover=false");
}

#[test]
fn network_enable_broadcasts_request_will_be_sent() {
    let registry = setup_registry_with_all_domains();
    let (sender, cmd_rx) = make_event_sender();
    let result = registry.dispatch_command("Network.enable", json!({}), &sender);
    assert!(result.is_some());
    let cmd = cmd_rx.try_recv().unwrap();
    match cmd {
        CDPCommand::SendEvent(ev) => assert_eq!(ev.method, "Network.requestWillBeSent"),
        _ => panic!("Expected SendEvent"),
    }
}

#[test]
fn target_set_auto_attach_true_broadcasts_attached_to_target() {
    let registry = setup_registry_with_all_domains();
    let (sender, cmd_rx) = make_event_sender();
    let result = registry.dispatch_command(
        "Target.setAutoAttach",
        json!({ "autoAttach": true, "flatten": true }),
        &sender,
    );
    assert!(result.is_some());
    let cmd = cmd_rx.try_recv().unwrap();
    match cmd {
        CDPCommand::SendEvent(ev) => {
            assert_eq!(ev.method, "Target.attachedToTarget");
            let params = ev.params.unwrap();
            assert!(params["sessionId"].is_string());
            assert_eq!(params["targetInfo"]["targetId"], "test-target-id");
            assert_eq!(params["waitingForDebuggerOnStart"], false);
        }
        _ => panic!("Expected SendEvent"),
    }
}

#[test]
fn target_set_auto_attach_false_no_event() {
    let registry = setup_registry_with_all_domains();
    let (sender, cmd_rx) = make_event_sender();
    let result = registry.dispatch_command(
        "Target.setAutoAttach",
        json!({ "autoAttach": false }),
        &sender,
    );
    assert!(result.is_some());
    assert!(cmd_rx.try_recv().is_err(), "No event should be sent when autoAttach=false");
}

#[test]
fn runtime_disable_no_event() {
    let registry = setup_registry_with_all_domains();
    let (sender, cmd_rx) = make_event_sender();
    let result = registry.dispatch_command("Runtime.disable", json!({}), &sender);
    assert!(result.is_some());
    assert!(cmd_rx.try_recv().is_err(), "Runtime.disable should not broadcast events");
}
