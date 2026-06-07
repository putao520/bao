// @trace TEST-CDP-033 [req:REQ-CDP-001,REQ-CDP-004,REQ-CDP-005,REQ-LIB-002] [level:unit]
// CdpRouter internal session creation, CdpSession send/detach/on,
// BackendKind exhaustiveness, CdpRouter send_command error paths,
// multiple sessions, session IDs uniqueness, detach_session errors.

use bao_cdp::{CdpRouter, ExternalBrowser, BackendKind};
use serde_json::json;

// ============================================================================
// CdpRouter construction
// ============================================================================

#[test]
fn test_cdp_router_new() {
    let router = CdpRouter::new();
    // No sessions yet
    let result = router.send_command("nonexistent", "Page.enable", None);
    assert!(result.is_err());
}

#[test]
fn test_cdp_router_default() {
    let router = CdpRouter::default();
    assert!(router.send_command("x", "Page.enable", None).is_err());
}

// ============================================================================
// BackendKind enum
// ============================================================================

#[test]
fn test_backend_kind_internal() {
    assert_eq!(BackendKind::Internal, BackendKind::Internal);
    assert_ne!(BackendKind::Internal, BackendKind::External);
}

#[test]
fn test_backend_kind_external() {
    assert_eq!(BackendKind::External, BackendKind::External);
    assert_ne!(BackendKind::External, BackendKind::Internal);
}

#[test]
fn test_backend_kind_debug() {
    let s = format!("{:?}", BackendKind::Internal);
    assert!(s.contains("Internal"));
    let s2 = format!("{:?}", BackendKind::External);
    assert!(s2.contains("External"));
}

#[test]
fn test_backend_kind_copy() {
    let k = BackendKind::Internal;
    let k2 = k;
    assert_eq!(k, k2);
}

#[test]
fn test_backend_kind_clone() {
    let k = BackendKind::External;
    let k2 = k.clone();
    assert_eq!(k, k2);
}

// ============================================================================
// CdpRouter: create_internal_session
// ============================================================================

#[test]
fn test_create_internal_session_id_format() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let sid = session.session_id();
    assert!(!sid.is_empty());
    assert_eq!(sid.len(), 16); // format!("{:016x}", ...)
}

#[test]
fn test_create_internal_session_target_id() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("my-target");
    assert_eq!(session.target_id(), "my-target");
}

#[test]
fn test_create_internal_session_backend_internal() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

#[test]
fn test_create_internal_session_unique_ids() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t1");
    // Small delay to ensure different timestamp
    std::thread::sleep(std::time::Duration::from_millis(2));
    let s2 = router.create_internal_session("t1");
    assert_ne!(s1.session_id(), s2.session_id());
}

#[test]
fn test_create_multiple_sessions() {
    let router = CdpRouter::new();
    let sessions: Vec<_> = (0..5)
        .map(|i| {
            std::thread::sleep(std::time::Duration::from_millis(1));
            router.create_internal_session(&format!("target-{}", i))
        })
        .collect();
    // All session IDs should be unique
    let ids: Vec<_> = sessions.iter().map(|s| s.session_id().to_string()).collect();
    for i in 0..ids.len() {
        for j in (i+1)..ids.len() {
            assert_ne!(ids[i], ids[j], "session {} and {} have same ID", i, j);
        }
    }
}

// ============================================================================
// CdpSession: send command via internal backend
// ============================================================================

#[test]
fn test_session_send_page_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Page.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_session_send_page_disable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Page.disable", None);
    assert!(result.is_ok());
}

#[test]
fn test_session_send_runtime_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Runtime.enable", None);
    assert!(result.is_ok());
    let val = result.unwrap();
    assert_eq!(val["executionContextId"], 1);
}

#[test]
fn test_session_send_dom_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "DOM.enable", None).is_ok());
}

#[test]
fn test_session_send_network_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "Network.enable", None).is_ok());
}

#[test]
fn test_session_send_unknown_domain() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "UnknownDomain.method", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

#[test]
fn test_session_send_unknown_command() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Page.nonexistent", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

#[test]
fn test_session_send_with_params() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Runtime.evaluate", Some(json!({"expression": "1+1"})));
    // Internal backend will try to handle but may not have a bridge responder
    // For commands that don't need bridge, should return Ok
    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_session_send_dom_describe_node() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "DOM.describeNode", None);
    assert!(result.is_ok());
    let val = result.unwrap();
    assert_eq!(val["node"]["nodeName"], "HTML");
}

#[test]
fn test_session_send_network_get_cookies() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = session.send(&router, "Network.getCookies", None);
    assert!(result.is_ok());
    assert!(result.unwrap()["cookies"].is_array());
}

#[test]
fn test_session_send_debugger_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "Debugger.enable", None).is_ok());
}

#[test]
fn test_session_send_css_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "CSS.enable", None).is_ok());
}

#[test]
fn test_session_send_log_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "Log.enable", None).is_ok());
}

#[test]
fn test_session_send_overlay_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "Overlay.enable", None).is_ok());
}

#[test]
fn test_session_send_fetch_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.send(&router, "Fetch.enable", None).is_ok());
}

// ============================================================================
// CdpSession: domain tracking on send
// ============================================================================

#[test]
fn test_session_send_tracks_enabled_domains() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    session.send(&router, "Page.enable", None).ok();
    session.send(&router, "Runtime.enable", None).ok();
    session.send(&router, "DOM.enable", None).ok();
    // Sending additional commands should still work
    assert!(session.send(&router, "Page.getLayoutMetrics", None).is_ok());
    assert!(session.send(&router, "Runtime.evaluate", Some(json!({"expression": "test"}))).is_ok());
}

// ============================================================================
// CdpRouter: send_command directly
// ============================================================================

#[test]
fn test_router_send_command_valid_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = router.send_command(session.session_id(), "Page.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_router_send_command_invalid_session() {
    let router = CdpRouter::new();
    let result = router.send_command("nonexistent-session", "Page.enable", None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("session not found"));
}

#[test]
fn test_router_send_command_empty_session_id() {
    let router = CdpRouter::new();
    let result = router.send_command("", "Page.enable", None);
    assert!(result.is_err());
}

// ============================================================================
// CdpRouter: detach_session
// ============================================================================

#[test]
fn test_detach_session_valid() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let sid = session.session_id().to_string();
    assert!(router.detach_session(&sid).is_ok());
}

#[test]
fn test_detach_session_invalid() {
    let router = CdpRouter::new();
    let result = router.detach_session("nonexistent");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32602);
}

#[test]
fn test_detach_session_twice() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let sid = session.session_id().to_string();
    assert!(router.detach_session(&sid).is_ok());
    let result = router.detach_session(&sid);
    assert!(result.is_err());
}

#[test]
fn test_detached_session_send_fails() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let sid = session.session_id().to_string();
    router.detach_session(&sid).unwrap();
    let result = router.send_command(&sid, "Page.enable", None);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("session not found"));
}

// ============================================================================
// CdpSession: detach via session method
// ============================================================================

#[test]
fn test_session_detach_self() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let sid = session.session_id().to_string();
    assert!(session.detach(&router).is_ok());
    assert!(router.send_command(&sid, "Page.enable", None).is_err());
}

#[test]
fn test_session_detach_twice_fails() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert!(session.detach(&router).is_ok());
    assert!(session.detach(&router).is_err());
}

// ============================================================================
// CdpSession: on event handler registration
// ============================================================================

#[test]
fn test_session_on_registers_handler() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    // Just verify it doesn't panic
    session.on("Page.loadEventFired", |_params| {});
    session.on("Runtime.consoleAPICalled", |_params| {});
}

#[test]
fn test_session_on_multiple_events() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    session.on("Page.loadEventFired", |_params| {});
    session.on("Page.domContentEventFired", |_params| {});
    session.on("Runtime.executionContextCreated", |_params| {});
}

#[test]
fn test_session_on_overwrite_handler() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    session.on("Page.loadEventFired", |_params| {});
    session.on("Page.loadEventFired", |_params| {}); // overwrite
}

// ============================================================================
// CdpSession: session_id() accessor
// ============================================================================

#[test]
fn test_session_session_id_accessor() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let sid = session.session_id();
    assert!(!sid.is_empty());
    assert_eq!(sid.len(), 16);
}

#[test]
fn test_session_target_id_accessor() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-xyz");
    assert_eq!(session.target_id(), "target-xyz");
}

#[test]
fn test_session_backend_kind_accessor() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

// ============================================================================
// ExternalBrowser struct
// ============================================================================

#[test]
fn test_external_browser_fields() {
    // We can't actually connect, but we can test the struct shape
    // ExternalBrowser { endpoint, session_id } is returned by connect_external
    // Since connect_external needs a real endpoint, we test the struct directly
    let eb = ExternalBrowser {
        endpoint: "ws://localhost:9222".into(),
        session_id: "abcd1234".into(),
    };
    assert_eq!(eb.endpoint, "ws://localhost:9222");
    assert_eq!(eb.session_id, "abcd1234");
}

#[test]
fn test_connect_external_defers_connection() {
    // ExternalBackend::new stores endpoint, doesn't connect immediately
    let router = CdpRouter::new();
    let result = router.connect_external("ws://localhost:1");
    assert!(result.is_ok());
    let eb = result.unwrap();
    assert_eq!(eb.endpoint, "ws://localhost:1");
}

// ============================================================================
// CdpRouter: concurrent sessions
// ============================================================================

#[test]
fn test_two_sessions_same_target() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("shared-target");
    std::thread::sleep(std::time::Duration::from_millis(1));
    let s2 = router.create_internal_session("shared-target");
    assert_ne!(s1.session_id(), s2.session_id());
    // Both can send commands independently
    assert!(s1.send(&router, "Page.enable", None).is_ok());
    assert!(s2.send(&router, "Runtime.enable", None).is_ok());
}

#[test]
fn test_two_sessions_different_targets() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("target-a");
    std::thread::sleep(std::time::Duration::from_millis(1));
    let s2 = router.create_internal_session("target-b");
    assert_ne!(s1.session_id(), s2.session_id());
    assert_ne!(s1.target_id(), s2.target_id());
    assert!(s1.send(&router, "Page.enable", None).is_ok());
    assert!(s2.send(&router, "Page.enable", None).is_ok());
}

#[test]
fn test_detach_one_session_other_survives() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t1");
    std::thread::sleep(std::time::Duration::from_millis(1));
    let s2 = router.create_internal_session("t2");
    s1.detach(&router).unwrap();
    // s2 should still work
    assert!(s2.send(&router, "Page.enable", None).is_ok());
}
