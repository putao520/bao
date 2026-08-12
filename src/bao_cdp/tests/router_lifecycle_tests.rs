// @trace TEST-CDP-010-ROUTER [req:REQ-CDP-001,REQ-CDP-005,REQ-LIB-002] [level:integration]
// CdpRouter lifecycle + CdpSession + BackendKind + bridge channel integration tests

use bao_cdp::{bridge_channel, BackendKind, BridgeCommand, BridgeResponse, CdpRouter};
use serde_json::json;

const TID: &str = "test-target";

// ---- CdpRouter internal session lifecycle ----

#[test]
fn test_router_default_creates_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    assert_eq!(session.target_id(), "target-1");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

#[test]
fn test_router_session_has_unique_id() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t1");
    let s2 = router.create_internal_session("t2");
    assert_ne!(s1.session_id(), s2.session_id());
}

#[test]
fn test_router_session_id_is_hex() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target");
    let id = session.session_id();
    assert!(
        id.len() == 16,
        "Session ID should be 16 hex chars, got {}",
        id.len()
    );
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit()),
        "Session ID should be hex: {}",
        id
    );
}

#[test]
fn test_router_send_command_unknown_session() {
    let router = CdpRouter::new();
    let result = router.send_command(
        "nonexistent-session",
        "Page.navigate",
        Some(json!({"url": "https://example.com"})),
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("session not found"));
}

#[test]
fn test_router_send_command_internal_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    // Internal backend handles known CDP commands
    let result = session.send(&router, "Page.enable", None);
    assert!(result.is_ok(), "Page.enable should succeed: {:?}", result);
}

#[test]
fn test_router_send_command_via_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");

    // Use session.send which auto-registers domain
    let result = session.send(&router, "Runtime.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_router_detach_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    let session_id = session.session_id().to_string();

    let detach_result = session.detach(&router);
    assert!(detach_result.is_ok());

    // After detach, commands to this session should fail
    let result = router.send_command(&session_id, "Page.enable", None);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("session not found"));
}

#[test]
fn test_router_detach_unknown_session() {
    let router = CdpRouter::new();
    let result = router.detach_session("nonexistent");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32602);
}

#[test]
fn test_router_multiple_sessions_independent() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t1");
    let s2 = router.create_internal_session("t2");

    let r1 = s1.send(&router, "Page.enable", None);
    let r2 = s2.send(&router, "Runtime.enable", None);
    assert!(r1.is_ok());
    assert!(r2.is_ok());

    // Detach s1 should not affect s2
    s1.detach(&router).unwrap();
    let r2_after = s2.send(&router, "Page.enable", None);
    assert!(r2_after.is_ok());
}

#[test]
fn test_router_session_send_registers_domain() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    // session.send auto-registers domain via enabled_domains insert
    let _ = session.send(&router, "Page.enable", None);
    let _ = session.send(&router, "Runtime.enable", None);
    // Sending again should still work (domain already registered)
    let result = session.send(
        &router,
        "Page.navigate",
        Some(json!({"url": "https://example.com"})),
    );
    assert!(result.is_ok());
}

// ---- BackendKind ----

#[test]
fn test_backend_kind_equality() {
    assert_eq!(BackendKind::Internal, BackendKind::Internal);
    assert_eq!(BackendKind::External, BackendKind::External);
    assert_ne!(BackendKind::Internal, BackendKind::External);
}

#[test]
fn test_backend_kind_debug() {
    assert!(format!("{:?}", BackendKind::Internal).contains("Internal"));
    assert!(format!("{:?}", BackendKind::External).contains("External"));
}

#[test]
fn test_backend_kind_copy() {
    let a = BackendKind::Internal;
    let b = a;
    assert_eq!(a, b);
}

// ---- Bridge channel ----

#[test]
fn test_bridge_channel_creation() {
    let (sender, receiver) = bridge_channel(std::time::Duration::from_secs(5));
    assert!(sender.is_alive());
    // is_alive sends a probe command; drain it
    let count = receiver.drain(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(count, 1, "is_alive probe should leave 1 command");
}

#[test]
fn test_bridge_channel_send_fire_and_forget() {
    let (sender, _receiver) = bridge_channel(std::time::Duration::from_secs(5));
    // Should not panic
    sender.send_fire_and_forget(BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "https://example.com".into(),
    });
}

#[test]
fn test_bridge_channel_send_timeout() {
    let (sender, _receiver) = bridge_channel(std::time::Duration::from_millis(50));
    let response = sender.send(BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "https://example.com".into(),
    });
    // No handler processes, so should timeout
    assert!(response.result.is_err());
    assert!(response.result.unwrap_err().contains("timeout"));
}

#[test]
fn test_bridge_receiver_try_process() {
    let (sender, receiver) = bridge_channel(std::time::Duration::from_secs(5));
    sender.send_fire_and_forget(BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "https://test.com".into(),
    });

    let processed = receiver.try_process(|cmd| match cmd {
        BridgeCommand::Navigate { url, .. } => BridgeResponse {
            result: Ok(json!({"url": url})),
        },
        _ => BridgeResponse {
            result: Err("unexpected".into()),
        },
    });
    assert!(processed, "Should have processed the command");
}

#[test]
fn test_bridge_receiver_try_process_empty() {
    let (_sender, receiver) = bridge_channel(std::time::Duration::from_secs(5));
    let processed = receiver.try_process(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert!(!processed, "No command to process");
}

#[test]
fn test_bridge_receiver_drain() {
    let (sender, receiver) = bridge_channel(std::time::Duration::from_secs(5));
    sender.send_fire_and_forget(BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "https://a.com".into(),
    });
    sender.send_fire_and_forget(BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "https://b.com".into(),
    });
    sender.send_fire_and_forget(BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "https://c.com".into(),
    });

    let count = receiver.drain(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(count, 3);
}

#[test]
fn test_bridge_receiver_drain_empty() {
    let (_sender, receiver) = bridge_channel(std::time::Duration::from_secs(5));
    let count = receiver.drain(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(count, 0);
}

// ---- BridgeCommand variants ----

#[test]
fn test_bridge_command_debug() {
    let cmd = BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "https://example.com".into(),
    };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("Navigate"));
    assert!(debug.contains("example.com"));
}

#[test]
fn test_bridge_response_debug() {
    let ok_resp = BridgeResponse {
        result: Ok(json!({"status": "ok"})),
    };
    let debug = format!("{:?}", ok_resp);
    assert!(debug.contains("result"));

    let err_resp = BridgeResponse {
        result: Err("test error".into()),
    };
    let debug = format!("{:?}", err_resp);
    assert!(debug.contains("test error"));
}

// ---- CDPServer event sender ----
// (Removed in TASK-4-CDP: the dead synchronous CDPServer entry was deleted;
// production + tests use cdp_server::CdpServer instead.)
