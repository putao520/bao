// @trace TEST-CDP-029 [req:REQ-CDP-001,REQ-CDP-005,REQ-LIB-002] [level:unit]
// CdpRouter connect_external error paths, ExternalBrowser fields,
// send_command routing branches, detach_session edge cases,
// session ID uniqueness, CDPError field verification.

use bao_cdp::{CdpRouter, BackendKind};

// ---- connect_external succeeds (no validation) ----

#[test]
fn test_connect_external_any_endpoint() {
    let router = CdpRouter::new();
    let result = router.connect_external("ws://localhost:9222/devtools");
    assert!(result.is_ok());
}

#[test]
fn test_connect_external_empty_string() {
    let router = CdpRouter::new();
    let result = router.connect_external("");
    // ExternalBackend::new always returns Ok
    assert!(result.is_ok());
}

#[test]
fn test_connect_external_returns_external_browser() {
    let router = CdpRouter::new();
    let ext = router.connect_external("ws://127.0.0.1:9222/devtools").unwrap();
    assert_eq!(ext.endpoint, "ws://127.0.0.1:9222/devtools");
    assert!(!ext.session_id.is_empty());
}

#[test]
fn test_external_browser_session_id_16_hex() {
    let router = CdpRouter::new();
    let ext = router.connect_external("ws://test").unwrap();
    assert_eq!(ext.session_id.len(), 16);
    assert!(ext.session_id.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_connect_external_creates_external_session() {
    let router = CdpRouter::new();
    let ext = router.connect_external("ws://test").unwrap();
    // Send command to the external session should fail (no real backend)
    let result = router.send_command(&ext.session_id, "Page.enable", None);
    // External backend send always returns error since no real connection
    assert!(result.is_err());
}

// ---- send_command with no sessions ----

#[test]
fn test_send_command_empty_session_id() {
    let router = CdpRouter::new();
    let result = router.send_command("", "Page.enable", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32602);
}

#[test]
fn test_send_command_whitespace_session_id() {
    let router = CdpRouter::new();
    let result = router.send_command("   ", "Page.enable", None);
    assert!(result.is_err());
}

#[test]
fn test_send_command_unicode_session_id() {
    let router = CdpRouter::new();
    let result = router.send_command("セッション", "Page.enable", None);
    assert!(result.is_err());
}

// ---- send_command with detached session ----

#[test]
fn test_send_command_after_detach() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let sid = session.session_id().to_string();
    session.detach(&router).unwrap();
    let result = router.send_command(&sid, "Page.enable", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32602);
}

// ---- detach_session edge cases ----

#[test]
fn test_detach_session_nonexistent() {
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

// ---- Session ID uniqueness ----

#[test]
fn test_session_ids_unique_across_many() {
    let router = CdpRouter::new();
    let mut ids = std::collections::HashSet::new();
    for i in 0..50 {
        let session = router.create_internal_session(&format!("t{}", i));
        let sid = session.session_id().to_string();
        assert!(ids.insert(sid), "Duplicate session ID detected");
    }
}

#[test]
fn test_session_id_length_16_hex() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let sid = session.session_id();
    assert_eq!(sid.len(), 16);
    assert!(sid.chars().all(|c| c.is_ascii_hexdigit()));
}

// ---- CdpSession target_id preservation ----

#[test]
fn test_target_id_long_string() {
    let router = CdpRouter::new();
    let long_id = "a".repeat(1000);
    let session = router.create_internal_session(&long_id);
    assert_eq!(session.target_id(), long_id);
}

#[test]
fn test_target_id_unicode() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("ターゲットID");
    assert_eq!(session.target_id(), "ターゲットID");
}

#[test]
fn test_target_id_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("");
    assert_eq!(session.target_id(), "");
}

#[test]
fn test_target_id_special_chars() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target/path?query=1&key=val#hash");
    assert_eq!(session.target_id(), "target/path?query=1&key=val#hash");
}

// ---- CdpSession backend_kind ----

#[test]
fn test_internal_session_backend_kind() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

// ---- Multiple sessions isolation ----

#[test]
fn test_many_sessions_all_independent() {
    let router = CdpRouter::new();
    let sessions: Vec<_> = (0..20)
        .map(|i| router.create_internal_session(&format!("target-{}", i)))
        .collect();

    for (i, session) in sessions.iter().enumerate() {
        assert_eq!(session.target_id(), format!("target-{}", i));
        assert_eq!(session.backend_kind(), BackendKind::Internal);
    }
}

#[test]
fn test_detach_half_sessions_other_half_works() {
    let router = CdpRouter::new();
    let sessions: Vec<_> = (0..10)
        .map(|i| router.create_internal_session(&format!("t{}", i)))
        .collect();

    // Detach even-indexed sessions
    for (i, session) in sessions.iter().enumerate() {
        if i % 2 == 0 {
            assert!(session.detach(&router).is_ok());
        }
    }

    // Odd-indexed sessions should still work
    for (i, session) in sessions.iter().enumerate() {
        if i % 2 == 1 {
            assert!(session.send(&router, "Page.enable", None).is_ok());
        }
    }
}

// ---- send_command via router directly ----

#[test]
fn test_router_send_command_with_valid_session() {
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
    let result = router.send_command(sid, "Page.navigate",
        Some(serde_json::json!({"url": "https://example.com"})));
    assert!(result.is_ok());
}

#[test]
fn test_router_send_command_unknown_method() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let result = session.send(&router, "Fake.method", None);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

// ---- Event handler registration ----

#[test]
fn test_session_on_many_events() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    // Register handlers for many events
    session.on("Page.load", |_| {});
    session.on("Page.domContentEventFired", |_| {});
    session.on("Runtime.consoleAPICalled", |_| {});
    session.on("Network.requestWillBeSent", |_| {});
    session.on("DOM.childNodeInserted", |_| {});
    session.on("Debugger.paused", |_| {});
}

#[test]
fn test_session_on_overwrite_handler() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    session.on("Page.load", |_| {});
    // Overwrite with new handler — should not panic
    session.on("Page.load", |_| {});
}

// ---- CdpRouter default/new equivalence ----

#[test]
fn test_router_new_default_equivalent() {
    let r1 = CdpRouter::new();
    let r2 = CdpRouter::default();
    // Both should create sessions
    let s1 = r1.create_internal_session("t");
    let s2 = r2.create_internal_session("t");
    assert_ne!(s1.session_id(), s2.session_id());
}

// ---- BackendKind exhaustive checks ----

#[test]
fn test_backend_kind_variants() {
    let internal = BackendKind::Internal;
    let external = BackendKind::External;
    assert_ne!(internal, external);
    assert_eq!(internal, BackendKind::Internal);
    assert_eq!(external, BackendKind::External);
}

#[test]
fn test_backend_kind_copy() {
    let k1 = BackendKind::Internal;
    let k2 = k1;
    assert_eq!(k1, k2);
}

#[test]
fn test_backend_kind_all_debug_variants() {
    for kind in &[BackendKind::Internal, BackendKind::External] {
        let debug = format!("{:?}", kind);
        assert!(!debug.is_empty());
    }
}

// ---- CDPError field checks ----

#[test]
fn test_cdp_error_session_not_found() {
    let router = CdpRouter::new();
    let err = router.send_command("bad-id", "Page.enable", None).unwrap_err();
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("session not found"));
}

#[test]
fn test_cdp_error_method_not_found() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t");
    let err = session.send(&router, "InvalidDomain.nonexistent", None).unwrap_err();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("wasn't found"));
}

// ---- Router with many concurrent sessions ----

#[test]
fn test_router_100_sessions() {
    let router = CdpRouter::new();
    let sessions: Vec<_> = (0..100)
        .map(|i| router.create_internal_session(&format!("t{}", i)))
        .collect();

    assert_eq!(sessions.len(), 100);

    // All should be able to send commands
    for session in &sessions {
        assert!(session.send(&router, "Page.enable", None).is_ok());
    }
}
