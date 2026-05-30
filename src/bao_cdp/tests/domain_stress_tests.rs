// @trace TEST-CDP-012-DOMAIN-STRESS [req:REQ-CDP-001,REQ-CDP-002,REQ-CDP-003,REQ-CDP-005,REQ-CDP-006,REQ-CDP-007] [level:integration]
// Domain handler stress tests: concurrent command sequences, error recovery,
// boundary params, rapid enable/disable cycling, unknown command resilience.

use bao_cdp::CdpRouter;
use serde_json::json;

// ---- Rapid enable/disable cycling ----

#[test]
fn test_rapid_page_enable_disable_cycle() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("cycle-target");
    for i in 0..100 {
        let enable_cmd = if i % 2 == 0 { "Page.enable" } else { "Page.disable" };
        let result = session.send(&router, enable_cmd, None);
        assert!(result.is_ok(), "Cycle {} failed: {:?}", i, result);
    }
}

#[test]
fn test_rapid_runtime_enable_disable_cycle() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("rt-cycle");
    for _ in 0..50 {
        session.send(&router, "Runtime.enable", None).unwrap();
        session.send(&router, "Runtime.disable", None).unwrap();
    }
}

#[test]
fn test_rapid_dom_enable_disable_cycle() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("dom-cycle");
    for _ in 0..50 {
        session.send(&router, "DOM.enable", None).unwrap();
        session.send(&router, "DOM.disable", None).unwrap();
    }
}

// ---- Mixed domain command sequences ----

#[test]
fn test_mixed_domain_commands_interleaved() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("mixed");
    let commands = vec![
        ("Page.enable", None),
        ("Runtime.enable", None),
        ("DOM.enable", None),
        ("Page.navigate", Some(json!({"url": "https://example.com"}))),
        ("Runtime.evaluate", Some(json!({"expression": "1+1"}))),
        ("DOM.getDocument", None),
        ("Page.disable", None),
        ("Runtime.disable", None),
        ("DOM.disable", None),
    ];
    for (cmd, params) in &commands {
        let result = session.send(&router, cmd, params.clone());
        assert!(result.is_ok(), "Command '{}' failed: {:?}", cmd, result);
    }
}

#[test]
fn test_multiple_sessions_same_commands() {
    let router = CdpRouter::new();
    let sessions: Vec<_> = (0..5)
        .map(|i| router.create_internal_session(&format!("multi-{}", i)))
        .collect();
    for session in &sessions {
        session.send(&router, "Page.enable", None).unwrap();
        session.send(&router, "Runtime.enable", None).unwrap();
        session.send(&router, "DOM.enable", None).unwrap();
    }
    for session in &sessions {
        let result = session.send(&router, "Page.navigate", Some(json!({"url": "https://test.com"})));
        assert!(result.is_ok());
    }
}

// ---- Unknown command resilience ----

#[test]
fn test_unknown_commands_do_not_corrupt_state() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("resilient");
    session.send(&router, "Page.enable", None).unwrap();
    // Send many unknown commands
    for i in 0..20 {
        let _ = session.send(&router, &format!("FakeDomain.command{}", i), None);
    }
    // Known commands should still work
    let result = session.send(&router, "Page.navigate", Some(json!({"url": "https://ok.com"})));
    assert!(result.is_ok());
}

#[test]
fn test_unknown_domain_after_known_domain() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("unk-domain");
    session.send(&router, "Page.enable", None).unwrap();
    let _ = session.send(&router, "NonExistent.method", None);
    let result = session.send(&router, "Page.enable", None);
    assert!(result.is_ok());
}

// ---- Session isolation stress ----

#[test]
fn test_session_isolation_under_load() {
    let router = CdpRouter::new();
    let s1 = router.create_internal_session("iso-1");
    let s2 = router.create_internal_session("iso-2");
    s1.send(&router, "Page.enable", None).unwrap();
    s2.send(&router, "Runtime.enable", None).unwrap();
    // Interleaved commands
    for i in 0..20 {
        let r1 = s1.send(&router, "Page.navigate", Some(json!({"url": format!("https://s1-{}.com", i)})));
        let r2 = s2.send(&router, "Runtime.evaluate", Some(json!({"expression": format!("{}", i)})));
        assert!(r1.is_ok(), "s1 iteration {} failed", i);
        assert!(r2.is_ok(), "s2 iteration {} failed", i);
    }
    // Detach s1
    s1.detach(&router).unwrap();
    // s2 should still work
    let r = s2.send(&router, "Page.enable", None);
    assert!(r.is_ok());
}

#[test]
fn test_many_sessions_create_detach() {
    let router = CdpRouter::new();
    let mut ids = Vec::new();
    for i in 0..20 {
        let session = router.create_internal_session(&format!("bulk-{}", i));
        session.send(&router, "Page.enable", None).unwrap();
        ids.push(session.session_id().to_string());
    }
    assert_eq!(ids.len(), 20);
    // Detach all
    for id in &ids {
        let result = router.detach_session(id);
        assert!(result.is_ok(), "Failed to detach {}", id);
    }
    // All should be gone
    for id in &ids {
        let result = router.send_command(id, "Page.enable", None);
        assert!(result.is_err());
    }
}

// ---- Boundary params ----

#[test]
fn test_navigate_with_empty_url() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("empty-url");
    session.send(&router, "Page.enable", None).unwrap();
    let result = session.send(&router, "Page.navigate", Some(json!({"url": ""})));
    assert!(result.is_ok());
}

#[test]
fn test_navigate_with_very_long_url() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("long-url");
    session.send(&router, "Page.enable", None).unwrap();
    let long_url = format!("https://example.com/{}", "a".repeat(10000));
    let result = session.send(&router, "Page.navigate", Some(json!({"url": long_url})));
    assert!(result.is_ok());
}

#[test]
fn test_navigate_with_unicode_url() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("unicode-url");
    session.send(&router, "Page.enable", None).unwrap();
    let result = session.send(&router, "Page.navigate", Some(json!({"url": "https://example.com/日本語/パス?値=🎉"})));
    assert!(result.is_ok());
}

#[test]
fn test_evaluate_with_empty_expression() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("empty-expr");
    session.send(&router, "Runtime.enable", None).unwrap();
    let result = session.send(&router, "Runtime.evaluate", Some(json!({"expression": ""})));
    assert!(result.is_ok());
}

#[test]
fn test_evaluate_with_long_expression() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("long-expr");
    session.send(&router, "Runtime.enable", None).unwrap();
    let long_expr = "1+1;".repeat(2000);
    let result = session.send(&router, "Runtime.evaluate", Some(json!({"expression": long_expr})));
    assert!(result.is_ok());
}

#[test]
fn test_evaluate_with_special_chars() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("special-expr");
    session.send(&router, "Runtime.enable", None).unwrap();
    let exprs = vec![
        r#"'ABC'"#,
        r#"'hello\nworld'"#,
        r#"`template ${1+2} literal`"#,
        r#"Buffer.from('test').toString('base64')"#,
    ];
    for expr in &exprs {
        let result = session.send(&router, "Runtime.evaluate", Some(json!({"expression": expr})));
        assert!(result.is_ok(), "Expression '{}' should not fail", expr);
    }
}

// ---- Network domain boundary ----

#[test]
fn test_network_enable_disable_rapid() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("net-rapid");
    for _ in 0..50 {
        session.send(&router, "Network.enable", None).unwrap();
        session.send(&router, "Network.disable", None).unwrap();
    }
}

#[test]
fn test_network_get_cookies_empty() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("net-cookies");
    let result = session.send(&router, "Network.getCookies", None);
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(val["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("net-all-cookies");
    let result = session.send(&router, "Network.getAllCookies", None);
    assert!(result.is_ok());
}

#[test]
fn test_network_get_response_body() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("net-body");
    let result = session.send(&router, "Network.getResponseBody", Some(json!({"requestId": "1234"})));
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(val["body"].is_string());
}

// ---- Debugger domain boundary ----

#[test]
fn test_debugger_enable_disable_rapid() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("dbg-rapid");
    for _ in 0..50 {
        session.send(&router, "Debugger.enable", None).unwrap();
        session.send(&router, "Debugger.disable", None).unwrap();
    }
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("dbg-bp");
    let result = session.send(&router, "Debugger.setBreakpointByUrl", Some(json!({
        "lineNumber": 10,
        "url": "https://example.com/app.js",
        "condition": ""
    })));
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(val["breakpointId"].is_string());
}

#[test]
fn test_debugger_step_commands() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("dbg-step");
    let commands = vec!["Debugger.stepOver", "Debugger.stepInto", "Debugger.stepOut", "Debugger.pause", "Debugger.resume"];
    for cmd in &commands {
        let result = session.send(&router, cmd, None);
        assert!(result.is_ok(), "{} should work", cmd);
    }
}

#[test]
fn test_debugger_get_script_source() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("dbg-src");
    let result = session.send(&router, "Debugger.getScriptSource", Some(json!({"scriptId": "1"})));
    assert!(result.is_ok());
}

// ---- Emulation domain ----

#[test]
fn test_emulation_set_device_metrics() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("emu-metrics");
    let result = session.send(&router, "Emulation.setDeviceMetricsOverride", Some(json!({
        "width": 375, "height": 812, "deviceScaleFactor": 3, "mobile": true
    })));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_set_user_agent_override() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("emu-ua");
    let result = session.send(&router, "Emulation.setUserAgentOverride", Some(json!({
        "userAgent": "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)"
    })));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_set_touch_emulation() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("emu-touch");
    let result = session.send(&router, "Emulation.setTouchEmulationEnabled", Some(json!({
        "enabled": true, "maxTouchPoints": 5
    })));
    assert!(result.is_ok());
}

// ---- Full multi-session lifecycle ----

#[test]
fn test_full_session_lifecycle_stress() {
    let router = CdpRouter::new();
    let mut alive = Vec::new();
    // Create 10 sessions
    for i in 0..10 {
        let s = router.create_internal_session(&format!("life-{}", i));
        s.send(&router, "Page.enable", None).unwrap();
        s.send(&router, "Runtime.enable", None).unwrap();
        s.send(&router, "Network.enable", None).unwrap();
        alive.push(s);
    }
    // Execute commands on all
    for (i, s) in alive.iter().enumerate() {
        s.send(&router, "Page.navigate", Some(json!({"url": format!("https://page{}.com", i)}))).unwrap();
        s.send(&router, "Runtime.evaluate", Some(json!({"expression": format!("{}*2", i)}))).unwrap();
    }
    // Detach half
    for i in (0..10).step_by(2) {
        alive[i].detach(&router).unwrap();
    }
    // Remaining should still work
    for i in (1..10).step_by(2) {
        let result = alive[i].send(&router, "Page.enable", None);
        assert!(result.is_ok(), "Session {} should still work after detach of others", i);
    }
}

// ---- CDP command via router.send_command ----

#[test]
fn test_router_send_command_to_valid_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("send-cmd");
    session.send(&router, "Page.enable", None).unwrap();
    let sid = session.session_id();
    let result = router.send_command(sid, "Page.navigate", Some(json!({"url": "https://direct.com"})));
    assert!(result.is_ok());
}

#[test]
fn test_router_send_command_all_domains() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("all-domains");
    let sid = session.session_id();
    let commands = vec![
        ("Page.enable", None),
        ("Runtime.enable", None),
        ("DOM.enable", None),
        ("Network.enable", None),
        ("Debugger.enable", None),
    ];
    for (cmd, params) in &commands {
        let result = router.send_command(sid, cmd, params.clone());
        assert!(result.is_ok(), "{} should succeed via send_command", cmd);
    }
}

// ---- Session ID uniqueness under stress ----

#[test]
fn test_session_ids_unique_under_stress() {
    let router = CdpRouter::new();
    let sessions: Vec<_> = (0..50)
        .map(|i| router.create_internal_session(&format!("uniq-{}", i)))
        .collect();
    let ids: Vec<_> = sessions.iter().map(|s| s.session_id().to_string()).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "All session IDs should be unique");
}
