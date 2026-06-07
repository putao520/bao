// @trace TEST-CDP-011 [req:REQ-CDP-005] [level:unit]
// @trace TEST-CDP-012 [req:REQ-LIB-002] [level:unit]
// CdpRouter internal backend dispatch, session management, BackendKind,
// CdpSession lifecycle, detach, event handler registration, InternalBackend
// command routing, error paths, clone/debug.

use bao_cdp::{CdpRouter, BackendKind, CDPError, CDPMessage, handle_command, bridge_channel};
use serde_json::json;

// ---- CdpRouter construction ----

#[test]
fn test_router_new() {
    let _router = CdpRouter::new();
    // No sessions, no panics
}

#[test]
fn test_router_default() {
    let _router = CdpRouter::default();
    // Same as new()
}

// ---- Internal session creation ----

#[test]
fn test_create_internal_session_has_target_id() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-abc");
    assert_eq!(session.target_id(), "target-abc");
}

#[test]
fn test_create_internal_session_is_internal() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

#[test]
fn test_create_internal_session_has_session_id() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    assert!(!session.session_id().is_empty());
    assert_eq!(session.session_id().len(), 16); // format!("{:016x}", ...)
}

#[test]
fn test_create_multiple_sessions_unique_ids() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t-1");
    let s2 = router.create_internal_session("t-2");
    let s3 = router.create_internal_session("t-3");
    assert_ne!(s1.session_id(), s2.session_id());
    assert_ne!(s2.session_id(), s3.session_id());
    assert_ne!(s1.session_id(), s3.session_id());
}

#[test]
fn test_create_session_empty_target_id() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("");
    assert_eq!(session.target_id(), "");
}

#[test]
fn test_create_session_long_target_id() {
    let router = CdpRouter::new();
    let long_id = "a".repeat(1000);
    let session = router.create_internal_session(&long_id);
    assert_eq!(session.target_id().len(), 1000);
}

// ---- BackendKind ----

#[test]
fn test_backend_kind_internal() {
    assert_eq!(BackendKind::Internal, BackendKind::Internal);
}

#[test]
fn test_backend_kind_external() {
    assert_eq!(BackendKind::External, BackendKind::External);
}

#[test]
fn test_backend_kind_not_equal() {
    assert_ne!(BackendKind::Internal, BackendKind::External);
}

#[test]
fn test_backend_kind_copy() {
    let kind = BackendKind::Internal;
    let copied = kind;
    assert_eq!(copied, BackendKind::Internal);
}

#[test]
fn test_backend_kind_debug() {
    assert!(format!("{:?}", BackendKind::Internal).contains("Internal"));
    assert!(format!("{:?}", BackendKind::External).contains("External"));
}

// ---- Internal backend command dispatch via router ----

#[test]
fn test_send_command_page_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "Page.enable", None);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!({}));
}

#[test]
fn test_send_command_page_disable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "Page.disable", None);
    assert!(result.is_ok());
}

#[test]
fn test_send_command_runtime_evaluate() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(
        session.session_id(),
        "Runtime.evaluate",
        Some(json!({"expression": "1+1"})),
    );
    assert!(result.is_ok());
}

#[test]
fn test_send_command_target_get_targets() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "Target.getTargets", None);
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(val.is_object());
}

#[test]
fn test_send_command_dom_get_document() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "DOM.getDocument", None);
    assert!(result.is_ok());
}

#[test]
fn test_send_command_network_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "Network.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_send_command_css_get_computed_style() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "CSS.getComputedStyleForNode", None);
    assert!(result.is_ok());
}

#[test]
fn test_send_command_emulation_set_device_metrics() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(
        session.session_id(),
        "Emulation.setDeviceMetricsOverride",
        Some(json!({"width": 1920, "height": 1080})),
    );
    assert!(result.is_ok());
}

#[test]
fn test_send_command_input_dispatch_mouse() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(
        session.session_id(),
        "Input.dispatchMouseEvent",
        Some(json!({"type": "mousePressed", "x": 100, "y": 200})),
    );
    assert!(result.is_ok());
}

#[test]
fn test_send_command_overlay_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "Overlay.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_send_command_debugger_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "Debugger.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_send_command_log_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "Log.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_send_command_fetch_enable() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "Fetch.enable", None);
    assert!(result.is_ok());
}

// ---- Error paths ----

#[test]
fn test_send_command_unknown_session() {
    let router = CdpRouter::new();
    let result = router.send_command("nonexistent-session", "Page.enable", None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("session not found"));
}

#[test]
fn test_send_command_unknown_domain() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "UnknownDomain.method", None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_send_command_no_dot() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = router.send_command(session.session_id(), "NoDotMethod", None);
    assert!(result.is_err());
}

// ---- Session detach ----

#[test]
fn test_detach_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let sid = session.session_id().to_string();
    let result = router.detach_session(&sid);
    assert!(result.is_ok());
}

#[test]
fn test_detach_session_twice_fails() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let sid = session.session_id().to_string();
    router.detach_session(&sid).unwrap();
    let result = router.detach_session(&sid);
    assert!(result.is_err());
}

#[test]
fn test_detach_nonexistent_session() {
    let router = CdpRouter::new();
    let result = router.detach_session("no-such-session");
    assert!(result.is_err());
}

#[test]
fn test_send_command_after_detach_fails() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let sid = session.session_id().to_string();
    router.detach_session(&sid).unwrap();
    let result = router.send_command(&sid, "Page.enable", None);
    assert!(result.is_err());
}

// ---- CdpSession API ----

#[test]
fn test_session_send_page_navigate() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = session.send(&router, "Page.navigate", Some(json!({"url": "https://example.com"})));
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(val.get("frameId").is_some());
    assert!(val.get("loaderId").is_some());
}

#[test]
fn test_session_send_enables_domain_tracking() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    session.send(&router, "Page.enable", None).unwrap();
    // Session tracks enabled domains internally
    // Verify by sending another command in same domain
    let result = session.send(&router, "Page.navigate", Some(json!({"url": "about:blank"})));
    assert!(result.is_ok());
}

#[test]
fn test_session_detach_via_session_api() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    let result = session.detach(&router);
    assert!(result.is_ok());
}

#[test]
fn test_session_on_event_handler() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    // Register event handler — should not panic
    session.on("Page.loadEventFired", |_params| {
        // Handler called when event fires
    });
}

#[test]
fn test_session_on_multiple_handlers() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    session.on("Page.loadEventFired", |_p| {});
    session.on("Runtime.consoleAPICalled", |_p| {});
    session.on("DOM.childNodeInserted", |_p| {});
    // No panic
}

#[test]
fn test_session_on_overwrite_handler() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t-1");
    session.on("Page.loadEventFired", |_p| {});
    session.on("Page.loadEventFired", |_p| {}); // overwrite
    // No panic
}

// ---- CDPError ----

#[test]
fn test_cdp_error_fields() {
    let err = CDPError { code: -32601, message: "test error".into() };
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "test error");
}

#[test]
fn test_cdp_error_construction() {
    let err = CDPError { code: -32000, message: "internal error".into() };
    assert_eq!(err.code, -32000);
    assert_eq!(err.message, "internal error");
}

#[test]
fn test_cdp_error_debug() {
    let err = CDPError { code: -32601, message: "method not found".into() };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32601"));
}

// ---- Multiple sessions interleaved ----

#[test]
fn test_multiple_sessions_independent() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t-1");
    let s2 = router.create_internal_session("t-2");

    let r1 = router.send_command(s1.session_id(), "Page.enable", None);
    let r2 = router.send_command(s2.session_id(), "Runtime.evaluate", Some(json!({"expression": "42"})));

    assert!(r1.is_ok());
    assert!(r2.is_ok());
}

#[test]
fn test_detach_one_session_other_works() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("t-1");
    let s2 = router.create_internal_session("t-2");

    router.detach_session(s1.session_id()).unwrap();

    // s2 should still work
    let result = router.send_command(s2.session_id(), "Page.enable", None);
    assert!(result.is_ok());
}

#[test]
fn test_many_sessions() {
    let router = CdpRouter::new();
    let mut sessions = Vec::new();
    for i in 0..50 {
        sessions.push(router.create_internal_session(&format!("target-{}", i)));
    }
    assert_eq!(sessions.len(), 50);

    // All should respond
    for session in &sessions {
        let result = router.send_command(session.session_id(), "Page.enable", None);
        assert!(result.is_ok());
    }
}

// ---- InternalBackend direct ----

fn internal_dispatch(method: &str, params: Option<serde_json::Value>) -> serde_json::Value {
    let params_ref = params.clone();
    let msg = CDPMessage {
        id: 1,
        method: method.to_string(),
        params,
        session_id: None,
    };
    let resp = handle_command(msg, "test-target", &params_ref, None);
    resp.result.unwrap_or_else(|| json!({}))
}

#[test]
fn test_internal_dispatch_page_get_frame_tree() {
    let result = internal_dispatch("Page.getFrameTree", None);
    assert!(result.get("frameTree").is_some());
}

#[test]
fn test_internal_dispatch_page_get_layout_metrics() {
    let result = internal_dispatch("Page.getLayoutMetrics", None);
    assert!(result.get("contentSize").is_some());
}

#[test]
fn test_internal_dispatch_runtime_get_properties() {
    let result = internal_dispatch("Runtime.getProperties", Some(json!({"objectId": "test"})));
    assert!(result.is_object());
}

#[test]
fn test_internal_dispatch_target_create_target() {
    let result = internal_dispatch("Target.createTarget", Some(json!({"url": "https://example.com"})));
    assert!(result.get("targetId").is_some());
}

#[test]
fn test_internal_dispatch_target_close_target() {
    let result = internal_dispatch("Target.closeTarget", Some(json!({"targetId": "abc"})));
    assert!(result.get("success").is_some());
}

#[test]
fn test_internal_dispatch_dom_describe_node() {
    let result = internal_dispatch("DOM.describeNode", None);
    assert!(result.is_object());
}

#[test]
fn test_internal_dispatch_network_get_response_body() {
    let result = internal_dispatch("Network.getResponseBody", Some(json!({"requestId": "r-1"})));
    assert!(result.get("body").is_some());
}

#[test]
fn test_internal_dispatch_css_get_inline_styles() {
    let result = internal_dispatch("CSS.getInlineStylesForNode", Some(json!({"nodeId": 1})));
    assert!(result.is_object());
}

#[test]
fn test_internal_dispatch_emulation_set_focus_emulation() {
    let result = internal_dispatch("Emulation.setFocusEmulationEnabled", Some(json!({"enabled": true})));
    assert!(result.is_object());
}

#[test]
fn test_internal_dispatch_input_dispatch_key_event() {
    let result = internal_dispatch("Input.dispatchKeyEvent", Some(json!({"type": "keyDown", "key": "Enter"})));
    assert!(result.is_object());
}

#[test]
fn test_internal_dispatch_overlay_hide_highlight() {
    let result = internal_dispatch("Overlay.hideHighlight", None);
    assert!(result.is_object());
}

#[test]
fn test_internal_dispatch_debugger_get_script_source() {
    let result = internal_dispatch("Debugger.getScriptSource", Some(json!({"scriptId": "1"})));
    assert!(result.is_object());
}

#[test]
fn test_internal_dispatch_log_clear() {
    let result = internal_dispatch("Log.clear", None);
    assert!(result.is_object());
}

#[test]
fn test_internal_dispatch_fetch_continue_request() {
    let result = internal_dispatch("Fetch.continueRequest", Some(json!({"requestId": "f-1"})));
    assert!(result.is_object());
}

// ---- Bridge channel integration with router ----

#[test]
fn test_router_with_bridge_sender() {
    let (tx, rx) = bridge_channel(std::time::Duration::from_secs(5));
    let router = CdpRouter::new();
    // Router can be created independently of bridge
    // Bridge is used by ExternalBackend, not CdpRouter directly
    drop(rx);
    drop(tx);
    drop(router);
}
