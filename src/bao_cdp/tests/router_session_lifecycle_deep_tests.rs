// @trace TEST-CDP-027 [req:REQ-CDP-001,REQ-CDP-005,REQ-LIB-002] [level:unit]
// CdpRouter session lifecycle: create/send/detach, CdpSession accessors,
// BackendKind, ExternalBrowser, multi-session, domain tracking.

use bao_cdp::{CdpRouter, BackendKind};

// ---- CdpRouter construction ----

#[test]
fn test_router_new() {
    let _router = CdpRouter::new();
}

#[test]
fn test_router_default() {
    let _router = CdpRouter::default();
}

// ---- CdpSession creation ----

#[test]
fn test_create_internal_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");
    assert!(!session.session_id().is_empty());
    assert_eq!(session.target_id(), "target-1");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

#[test]
fn test_session_id_format_hex() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let sid = session.session_id();
    assert_eq!(sid.len(), 16);
    assert!(sid.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_multiple_sessions_unique_ids() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t1");
    let s2 = router.create_internal_session("t2");
    assert_ne!(s1.session_id(), s2.session_id());
}

#[test]
fn test_session_target_id_preserved() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("my-special-target");
    assert_eq!(session.target_id(), "my-special-target");
}

#[test]
fn test_session_backend_kind_internal() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

// ---- Session send command (internal) ----

#[test]
fn test_session_send_page_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "Page.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_session_send_page_navigate() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "Page.navigate", Some(serde_json::json!({"url":"http://test"})));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["frameId"], "0");
}

#[test]
fn test_session_send_runtime_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "Runtime.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_session_send_dom_get_document() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "DOM.getDocument", None);
    assert!(result.is_ok());
}

#[test]
fn test_session_send_network_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "Network.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_session_send_unknown_domain() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "UnknownDomain.test", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

#[test]
fn test_session_send_emulation_set_metrics() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "Emulation.setDeviceMetricsOverride",
        Some(serde_json::json!({"width":1280,"height":720})));
    assert!(result.is_ok());
}

#[test]
fn test_session_send_input_dispatch_mouse() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "Input.dispatchMouseEvent",
        Some(serde_json::json!({"type":"mousePressed","x":100,"y":200})));
    assert!(result.is_ok());
}

#[test]
fn test_session_send_css_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    assert!(session.send(&router, "CSS.enable", None).is_ok());
}

#[test]
fn test_session_send_overlay_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    assert!(session.send(&router, "Overlay.enable", None).is_ok());
}

#[test]
fn test_session_send_debugger_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    assert!(session.send(&router, "Debugger.enable", None).is_ok());
}

#[test]
fn test_session_send_log_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    assert!(session.send(&router, "Log.enable", None).is_ok());
}

#[test]
fn test_session_send_fetch_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    assert!(session.send(&router, "Fetch.enable", None).is_ok());
}

#[test]
fn test_session_send_target_get_targets() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "Target.getTargets", None);
    assert!(result.is_ok());
}

// ---- Session detach ----

#[test]
fn test_session_detach() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let sid = session.session_id().to_string();
    assert!(session.detach(&router).is_ok());
    // After detach, send_command should fail
    let result = router.send_command(&sid, "Page.enable", None);
    assert!(result.is_err());
}

#[test]
fn test_session_detach_twice_fails() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    assert!(session.detach(&router).is_ok());
    // Second detach fails because session is already removed
    let result = session.detach(&router);
    assert!(result.is_err());
}

// ---- send_command with invalid session ----

#[test]
fn test_send_command_invalid_session() {
    let router = CdpRouter::new();
    let result = router.send_command("nonexistent-session", "Page.enable", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32602);
}

// ---- Domain tracking via send ----

#[test]
fn test_send_tracks_domain() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    // Send tracks the domain from method string
    let _ = session.send(&router, "Page.enable", None);
    let _ = session.send(&router, "Runtime.enable", None);
    // These should succeed because they're internal dispatches
    assert!(session.send(&router, "Page.navigate", Some(serde_json::json!({"url":"http://test"}))).is_ok());
}

// ---- Multiple sessions concurrent ----

#[test]
fn test_two_sessions_same_router() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t1");
    let s2 = router.create_internal_session("t2");

    let r1 = s1.send(&router, "Page.enable", None);
    let r2 = s2.send(&router, "Runtime.enable", None);
    assert!(r1.is_ok());
    assert!(r2.is_ok());
}

#[test]
fn test_two_sessions_independent_targets() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("target-alpha");
    let s2 = router.create_internal_session("target-beta");
    assert_ne!(s1.target_id(), s2.target_id());
}

#[test]
fn test_detach_one_session_other_works() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t1");
    let s2 = router.create_internal_session("t2");
    assert!(s1.detach(&router).is_ok());
    // s2 should still work
    assert!(s2.send(&router, "Page.enable", None).is_ok());
}

// ---- Event handler registration ----

#[test]
fn test_session_on_event_handler() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    // Just verify the on() method doesn't panic
    session.on("Page.load", |_val| {});
    session.on("Runtime.consoleAPICalled", |_val| {});
}

#[test]
fn test_session_on_multiple_events() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    session.on("Page.load", |_v| {});
    session.on("Page.domContentEventFired", |_v| {});
    session.on("Network.requestWillBeSent", |_v| {});
}

// ---- BackendKind enum ----

#[test]
fn test_backend_kind_internal_eq() {
    assert_eq!(BackendKind::Internal, BackendKind::Internal);
}

#[test]
fn test_backend_kind_external_eq() {
    assert_eq!(BackendKind::External, BackendKind::External);
}

#[test]
fn test_backend_kind_differ() {
    assert_ne!(BackendKind::Internal, BackendKind::External);
}

#[test]
fn test_backend_kind_debug() {
    assert!(format!("{:?}", BackendKind::Internal).contains("Internal"));
    assert!(format!("{:?}", BackendKind::External).contains("External"));
}

#[test]
fn test_backend_kind_clone() {
    let kind = BackendKind::Internal;
    let cloned = kind.clone();
    assert_eq!(kind, cloned);
}

#[test]
fn test_backend_kind_copy() {
    let kind = BackendKind::External;
    let copied = kind;
    assert_eq!(kind, copied);
}

// ---- Router send_command directly ----

#[test]
fn test_router_send_command_page_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let sid = session.session_id();
    let result = router.send_command(sid, "Page.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_router_send_command_with_params() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let sid = session.session_id();
    let result = router.send_command(sid, "Page.navigate", Some(serde_json::json!({"url":"http://a.com"})));
    assert!(result.is_ok());
}
