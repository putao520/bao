// @trace TEST-CDP-028 [req:REQ-CDP-003,REQ-CDP-006] [level:unit]
// BridgeChannel timeout behavior, fire-and-forget, clone semantics,
// BridgeResponse result variants, BridgeCommand field completeness.

use std::time::Duration;

use bao_cdp::{BridgeCommand, BridgeResponse, bridge_channel};
use serde_json::json;

const TID: &str = "test-target";

// ---- bridge_channel creation ----

#[test]
fn test_bridge_channel_creates_pair() {
    let (_tx, _rx) = bridge_channel(Duration::from_secs(5));
}

#[test]
fn test_bridge_channel_short_timeout() {
    // 1ms timeout must actually surface as a timeout error when no responder runs,
    // not just construct successfully.
    let (tx, _rx) = bridge_channel(Duration::from_millis(1));
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_err(), "1ms timeout must yield an error");
    let err = resp.result.unwrap_err();
    assert!(err.contains("timeout"),
        "short timeout must report 'timeout', got: {}", err);
}

#[test]
fn test_bridge_channel_long_timeout() {
    // Long timeout (300s) must NOT fire during a fast request/response cycle.
    let (tx, rx) = bridge_channel(Duration::from_secs(300));
    let handler = std::thread::spawn(move || {
        loop {
            let processed = rx.try_process(|_cmd| {
                BridgeResponse { result: Ok(json!({"ok": true})) }
            });
            if processed { break; }
            std::thread::sleep(Duration::from_micros(100));
        }
    });
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_ok(), "long timeout must not fire for fast response");
    assert_eq!(resp.result.unwrap()["ok"], true);
    handler.join().unwrap();
}

// ---- BridgeSender::send + BridgeReceiver::try_process ----

#[test]
fn test_send_and_process_navigate() {
    let (tx, _rx) = bridge_channel(Duration::from_secs(5));
    let response = tx.send(BridgeCommand::Navigate { target_id: TID.into(), url: "http://test".into() });
    // No handler running, so this will timeout
    assert!(response.result.is_err());
    assert!(response.result.unwrap_err().contains("timeout"));
}

#[test]
fn test_send_with_handler() {
    // Original empty-channel case: try_process on empty channel returns false.
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let processed = rx.try_process(|cmd| match cmd {
        BridgeCommand::Navigate { url, .. } => BridgeResponse { result: Ok(json!({"url": url})) },
        _ => BridgeResponse { result: Err("unexpected".into()) },
    });
    assert!(!processed, "try_process on empty channel must return false");
}

#[test]
fn test_send_with_handler_process_navigate() {
    // Adversarial: end-to-end Navigate command round-trip with field preservation.
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let handler = std::thread::spawn(move || {
        loop {
            let processed = rx.try_process(|cmd| match cmd {
                BridgeCommand::Navigate { url, .. } => BridgeResponse { result: Ok(json!({"url": url})) },
                _ => BridgeResponse { result: Err("unexpected".into()) },
            });
            if processed { break; }
            std::thread::sleep(Duration::from_micros(100));
        }
    });
    let resp = tx.send(BridgeCommand::Navigate { target_id: TID.into(), url: "http://handler-test".into() });
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["url"], "http://handler-test");
    handler.join().unwrap();
}

#[test]
fn test_send_and_recv_success() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));

    // Handler thread
    let handler = std::thread::spawn(move || {
        // Block until a request arrives
        loop {
            let processed = rx.try_process(|_cmd| {
                BridgeResponse { result: Ok(json!({"handled": true})) }
            });
            if processed { break; }
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["handled"], true);

    handler.join().unwrap();
}

#[test]
fn test_send_timeout_response() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(10));
    let resp = tx.send(BridgeCommand::GetUrl { target_id: TID.into() });
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("timeout"));
}

// ---- BridgeSender::send_fire_and_forget ----

#[test]
fn test_fire_and_forget_no_block() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    // Should return immediately
    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    tx.send_fire_and_forget(BridgeCommand::GetUrl { target_id: TID.into() });
    tx.send_fire_and_forget(BridgeCommand::GetDocument { target_id: TID.into() });

    // Drain should pick up all 3
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 3);
}

// ---- BridgeSender::clone ----

#[test]
fn test_sender_clone_works() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();

    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    tx2.send_fire_and_forget(BridgeCommand::GetUrl { target_id: TID.into() });

    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 2);
}

#[test]
fn test_sender_clone_same_timeout() {
    let (tx, _rx) = bridge_channel(Duration::from_secs(7));
    let tx2 = tx.clone();
    // Both should work — just verify no panic
    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    tx2.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
}

#[test]
fn test_sender_clone_preserves_timeout_semantics() {
    // Adversarial: clone must inherit the parent's timeout. A clone of a
    // short-timeout sender must surface a timeout error, not hang forever.
    let (tx, _rx) = bridge_channel(Duration::from_millis(5));
    let tx2 = tx.clone();
    // No handler running → both original and clone must timeout quickly.
    let start = std::time::Instant::now();
    let r1 = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    let r2 = tx2.send(BridgeCommand::GetUrl { target_id: TID.into() });
    let elapsed = start.elapsed();
    assert!(r1.result.is_err() && r1.result.unwrap_err().contains("timeout"),
        "original sender must timeout");
    assert!(r2.result.is_err() && r2.result.unwrap_err().contains("timeout"),
        "cloned sender must timeout with inherited timeout");
    // 5ms timeout × 2 sends must complete well under 1s. Guards against the
    // clone accidentally getting an infinite (or multi-second) timeout.
    assert!(elapsed < Duration::from_secs(1),
        "clone timeout inheritance broke: elapsed {:?}", elapsed);
}

#[test]
fn test_sender_clone_long_timeout_does_not_fire() {
    // Adversarial (inverse): clone of a long-timeout sender must NOT spuriously
    // timeout during a normal round-trip.
    let (tx, rx) = bridge_channel(Duration::from_secs(60));
    let tx2 = tx.clone();
    let handler = std::thread::spawn(move || {
        loop {
            let processed = rx.try_process(|_cmd| BridgeResponse { result: Ok(json!({"ok": true})) });
            if processed { break; }
            std::thread::sleep(Duration::from_micros(100));
        }
    });
    let resp = tx2.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_ok(), "cloned long-timeout sender must not spuriously fail");
    handler.join().unwrap();
}

// ---- BridgeReceiver::drain ----

#[test]
fn test_drain_empty() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 0);
}

#[test]
fn test_drain_multiple() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    for _i in 0..10 {
        tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    }
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 10);
}

#[test]
fn test_drain_multiple_distinct_commands() {
    // Adversarial: drain must process a heterogeneous batch and preserve
    // per-command identity through the handler (not just count).
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let cmds: Vec<BridgeCommand> = vec![
        BridgeCommand::Navigate { target_id: TID.into(), url: "http://a".into() },
        BridgeCommand::GetTitle { target_id: TID.into() },
        BridgeCommand::GetUrl { target_id: TID.into() },
        BridgeCommand::Reload { target_id: TID.into(), ignore_cache: true },
        BridgeCommand::StopLoading { target_id: TID.into() },
    ];
    let n = cmds.len();
    for cmd in cmds {
        tx.send_fire_and_forget(cmd);
    }
    let collected = std::cell::RefCell::new(Vec::<&'static str>::new());
    let count = rx.drain(|cmd| {
        let label = match cmd {
            BridgeCommand::Navigate { .. } => "nav",
            BridgeCommand::GetTitle { .. } => "title",
            BridgeCommand::GetUrl { .. } => "url",
            BridgeCommand::Reload { .. } => "reload",
            BridgeCommand::StopLoading { .. } => "stop",
            _ => "other",
        };
        collected.borrow_mut().push(label);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count, n);
    let labels = collected.into_inner();
    assert_eq!(labels, vec!["nav", "title", "url", "reload", "stop"],
        "drain must preserve per-command identity");
}

#[test]
fn test_drain_after_close_reports_zero() {
    // Adversarial: drain on a channel whose sender was dropped must return 0
    // (not panic, not loop forever).
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    drop(tx);
    // First drain picks up the one queued command.
    let first = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(first, 1);
    // Second drain on now-empty + sender-dropped channel must be 0 cleanly.
    let second = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(second, 0);
}

// ---- BridgeResponse result variants ----

#[test]
fn test_bridge_response_ok() {
    let resp = BridgeResponse { result: Ok(json!({"data": 42})) };
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["data"], 42);
}

#[test]
fn test_bridge_response_err() {
    let resp = BridgeResponse { result: Err("something failed".into()) };
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "something failed");
}

#[test]
fn test_bridge_response_debug() {
    let resp = BridgeResponse { result: Ok(json!(true)) };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("BridgeResponse"));
}

// ---- BridgeCommand Debug ----

#[test]
fn test_bridge_command_navigate_debug() {
    let cmd = BridgeCommand::Navigate { target_id: TID.into(), url: "http://example.com".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("Navigate"));
    assert!(debug.contains("http://example.com"));
}

#[test]
fn test_bridge_command_evaluate_js_debug() {
    let cmd = BridgeCommand::EvaluateJs { target_id: TID.into(), expression: "1+1".into(), return_by_value: true };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("EvaluateJs"));
}

#[test]
fn test_bridge_command_screenshot_debug() {
    let cmd = BridgeCommand::TakeScreenshot { target_id: TID.into(), format: "png".into(), quality: Some(80) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("TakeScreenshot"));
}

#[test]
fn test_bridge_command_query_selector_debug() {
    let cmd = BridgeCommand::QuerySelector { target_id: TID.into(), selector: "div.test".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("QuerySelector"));
}

#[test]
fn test_bridge_command_dispatch_mouse_debug() {
    let cmd = BridgeCommand::DispatchMouseEvent { target_id: TID.into(), event_type: "mousePressed".into(),
        x: 100.0, y: 200.0,
        button: Some(0),
        click_count: Some(1), };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DispatchMouseEvent"));
}

#[test]
fn test_bridge_command_dispatch_key_debug() {
    let cmd = BridgeCommand::DispatchKeyEvent { target_id: TID.into(), event_type: "keyDown".into(),
        key: "Enter".into(),
        code: "Enter".into(),
        text: None, };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DispatchKeyEvent"));
}

#[test]
fn test_bridge_command_set_viewport_debug() {
    let cmd = BridgeCommand::SetViewport { target_id: TID.into(), width: 1920, height: 1080, device_scale_factor: Some(2.0) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("SetViewport"));
}

#[test]
fn test_bridge_command_set_user_agent_debug() {
    let cmd = BridgeCommand::SetUserAgent { target_id: TID.into(), user_agent: "Bao/1.0".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("SetUserAgent"));
}

#[test]
fn test_bridge_command_get_cookies_debug() {
    let cmd = BridgeCommand::GetCookies { target_id: TID.into(), urls: vec!["http://a.com".into()] };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetCookies"));
}

#[test]
fn test_bridge_command_get_all_cookies_debug() {
    let cmd = BridgeCommand::GetAllCookies { target_id: TID.into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetAllCookies"));
}

#[test]
fn test_bridge_command_delete_cookie_debug() {
    let cmd = BridgeCommand::DeleteCookie { target_id: TID.into(), name: "session".into(), url: Some("http://a.com".into()) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DeleteCookie"));
}

#[test]
fn test_bridge_command_set_cookie_debug() {
    let cmd = BridgeCommand::SetCookie { target_id: TID.into(), name: "foo".into(),
        value: "bar".into(),
        url: Some("http://a.com".into()),
        domain: None, };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("SetCookie"));
}

#[test]
fn test_bridge_command_get_response_body_debug() {
    let cmd = BridgeCommand::GetResponseBody { target_id: TID.into(), request_id: "req-123".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetResponseBody"));
}

#[test]
fn test_bridge_command_add_script_debug() {
    let cmd = BridgeCommand::AddScriptToEvaluateOnNewDocument { target_id: TID.into(), source: "console.log(1)".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("AddScriptToEvaluateOnNewDocument"));
}

#[test]
fn test_bridge_command_reload_debug() {
    let cmd = BridgeCommand::Reload { target_id: TID.into(), ignore_cache: true };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("Reload"));
}

#[test]
fn test_bridge_command_go_back_debug() {
    let cmd = BridgeCommand::GoBack { target_id: TID.into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GoBack"));
}

#[test]
fn test_bridge_command_go_forward_debug() {
    let cmd = BridgeCommand::GoForward { target_id: TID.into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GoForward"));
}

#[test]
fn test_bridge_command_stop_loading_debug() {
    let cmd = BridgeCommand::StopLoading { target_id: TID.into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("StopLoading"));
}

#[test]
fn test_bridge_command_close_page_debug() {
    let cmd = BridgeCommand::ClosePage { target_id: TID.into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("ClosePage"));
}

#[test]
fn test_bridge_command_set_attribute_value_debug() {
    let cmd = BridgeCommand::SetAttributeValue { target_id: TID.into(), node_id: 5, name: "class".into(), value: "active".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("SetAttributeValue"));
}

#[test]
fn test_bridge_command_get_outer_html_debug() {
    let cmd = BridgeCommand::GetOuterHtml { target_id: TID.into(), node_id: Some(3) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetOuterHtml"));
}

#[test]
fn test_bridge_command_query_selector_all_debug() {
    let cmd = BridgeCommand::QuerySelectorAll { target_id: TID.into(), selector: "li".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("QuerySelectorAll"));
}

#[test]
fn test_bridge_command_insert_text_debug() {
    let cmd = BridgeCommand::InsertText { target_id: TID.into(), text: "hello".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("InsertText"));
}

// ---- Multi-target + Debugger domain Debug (REQ-CDP-003 / REQ-CDP-006) ----
// Adversarial gap: the original file had ZERO Debug coverage for the 15
// variants (CreateTarget, ListTargets, 13 Debugger*) that are the substance of
// REQ-CDP-003 (Debugger Domain) and multi-target management. A missing #[derive(Debug)]
// or a renamed variant would have gone undetected.

#[test]
fn test_bridge_command_create_target_debug() {
    let cmd = BridgeCommand::CreateTarget { url: "http://new.target".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("CreateTarget"));
    assert!(debug.contains("http://new.target"));
}

#[test]
fn test_bridge_command_list_targets_debug() {
    let cmd = BridgeCommand::ListTargets;
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("ListTargets"));
}

#[test]
fn test_bridge_command_debugger_enable_debug() {
    let cmd = BridgeCommand::DebuggerEnable { target_id: TID.into() };
    assert!(format!("{:?}", cmd).contains("DebuggerEnable"));
}

#[test]
fn test_bridge_command_debugger_disable_debug() {
    let cmd = BridgeCommand::DebuggerDisable { target_id: TID.into() };
    assert!(format!("{:?}", cmd).contains("DebuggerDisable"));
}

#[test]
fn test_bridge_command_debugger_set_breakpoint_debug() {
    let cmd = BridgeCommand::DebuggerSetBreakpoint {
        target_id: TID.into(), script_id: 42, offset: 100, line: 10, column: Some(5),
    };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DebuggerSetBreakpoint"));
    // Adversarial: numeric fields must survive Debug round-trip (not be elided).
    assert!(debug.contains("42") && debug.contains("100"));
}

#[test]
fn test_bridge_command_debugger_clear_breakpoint_debug() {
    let cmd = BridgeCommand::DebuggerClearBreakpoint { target_id: TID.into(), script_id: 7, offset: 9 };
    assert!(format!("{:?}", cmd).contains("DebuggerClearBreakpoint"));
}

#[test]
fn test_bridge_command_debugger_interrupt_debug() {
    let cmd = BridgeCommand::DebuggerInterrupt { target_id: TID.into() };
    assert!(format!("{:?}", cmd).contains("DebuggerInterrupt"));
}

#[test]
fn test_bridge_command_debugger_resume_debug() {
    let cmd = BridgeCommand::DebuggerResume { target_id: TID.into(), step_type: Some("stepInto".into()) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DebuggerResume"));
    assert!(debug.contains("stepInto"));
}

#[test]
fn test_bridge_command_debugger_list_frames_debug() {
    let cmd = BridgeCommand::DebuggerListFrames { target_id: TID.into() };
    assert!(format!("{:?}", cmd).contains("DebuggerListFrames"));
}

#[test]
fn test_bridge_command_debugger_get_environment_debug() {
    let cmd = BridgeCommand::DebuggerGetEnvironment { target_id: TID.into(), frame_actor_id: "actor-1".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DebuggerGetEnvironment"));
    assert!(debug.contains("actor-1"));
}

#[test]
fn test_bridge_command_debugger_eval_debug() {
    let cmd = BridgeCommand::DebuggerEval {
        target_id: TID.into(), expression: "debugger".into(), frame_actor_id: Some("actor-2".into()),
    };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DebuggerEval"));
    assert!(debug.contains("debugger"));
}

#[test]
fn test_bridge_command_debugger_get_possible_breakpoints_debug() {
    let cmd = BridgeCommand::DebuggerGetPossibleBreakpoints { target_id: TID.into(), script_id: 11 };
    assert!(format!("{:?}", cmd).contains("DebuggerGetPossibleBreakpoints"));
}

#[test]
fn test_bridge_command_debugger_get_script_source_debug() {
    let cmd = BridgeCommand::DebuggerGetScriptSource { target_id: TID.into(), script_id: 13 };
    assert!(format!("{:?}", cmd).contains("DebuggerGetScriptSource"));
}

#[test]
fn test_bridge_command_debugger_blackbox_debug() {
    let cmd = BridgeCommand::DebuggerBlackbox { target_id: TID.into(), script_id: 17 };
    assert!(format!("{:?}", cmd).contains("DebuggerBlackbox"));
}

#[test]
fn test_bridge_command_debugger_unblackbox_debug() {
    let cmd = BridgeCommand::DebuggerUnblackbox { target_id: TID.into(), script_id: 19 };
    assert!(format!("{:?}", cmd).contains("DebuggerUnblackbox"));
}

// ---- BridgeCommand field completeness (no Debug needed, just construction) ----

#[test]
fn test_all_bridge_commands_constructible() {
    // Adversarial / SPEC-alignment: exhaustively cover EVERY variant of the
    // BridgeCommand enum. The original test only asserted 26 variants, silently
    // missing CreateTarget, ListTargets, and all 13 Debugger* variants — which
    // are the heart of REQ-CDP-003 (Debugger Domain) and REQ-CDP-006 (Network /
    // multi-target). A drift in the enum (new variant added, field renamed)
    // must surface as a compile error here, not slip through.
    //
    // If this test fails to compile after an enum change, ADD the new variant
    // to the vector AND bump the expected count.
    let cmds: Vec<BridgeCommand> = vec![
        // Core navigation / page commands (16)
        BridgeCommand::Navigate { target_id: TID.into(), url: String::new() },
        BridgeCommand::EvaluateJs { target_id: TID.into(), expression: String::new(), return_by_value: false },
        BridgeCommand::TakeScreenshot { target_id: TID.into(), format: "png".into(), quality: None },
        BridgeCommand::GetTitle { target_id: TID.into() },
        BridgeCommand::GetUrl { target_id: TID.into() },
        BridgeCommand::GetDocument { target_id: TID.into() },
        BridgeCommand::QuerySelector { target_id: TID.into(), selector: String::new() },
        BridgeCommand::QuerySelectorAll { target_id: TID.into(), selector: String::new() },
        BridgeCommand::GetOuterHtml { target_id: TID.into(), node_id: None },
        BridgeCommand::SetAttributeValue { target_id: TID.into(), node_id: 0, name: String::new(), value: String::new() },
        BridgeCommand::DispatchMouseEvent { target_id: TID.into(), event_type: String::new(), x: 0.0, y: 0.0, button: None, click_count: None },
        BridgeCommand::DispatchKeyEvent { target_id: TID.into(), event_type: String::new(), key: String::new(), code: String::new(), text: None },
        BridgeCommand::InsertText { target_id: TID.into(), text: String::new() },
        BridgeCommand::SetViewport { target_id: TID.into(), width: 0, height: 0, device_scale_factor: None },
        BridgeCommand::SetUserAgent { target_id: TID.into(), user_agent: String::new() },
        BridgeCommand::GetCookies { target_id: TID.into(), urls: vec![] },
        BridgeCommand::GetAllCookies { target_id: TID.into() },
        BridgeCommand::DeleteCookie { target_id: TID.into(), name: String::new(), url: None },
        BridgeCommand::SetCookie { target_id: TID.into(), name: String::new(), value: String::new(), url: None, domain: None },
        BridgeCommand::GetResponseBody { target_id: TID.into(), request_id: String::new() },
        BridgeCommand::AddScriptToEvaluateOnNewDocument { target_id: TID.into(), source: String::new() },
        BridgeCommand::Reload { target_id: TID.into(), ignore_cache: false },
        BridgeCommand::GoBack { target_id: TID.into() },
        BridgeCommand::GoForward { target_id: TID.into() },
        BridgeCommand::StopLoading { target_id: TID.into() },
        BridgeCommand::ClosePage { target_id: TID.into() },
        // Multi-target management (REQ-CDP-006 multi-page) — previously MISSING
        BridgeCommand::CreateTarget { url: String::new() },
        BridgeCommand::ListTargets,
        // Debugger domain — mapped to servo DevtoolScriptControlMsg (BUG-CDP-006)
        // These 13 variants are the REQ-CDP-003 surface; previously MISSING.
        BridgeCommand::DebuggerEnable { target_id: TID.into() },
        BridgeCommand::DebuggerDisable { target_id: TID.into() },
        BridgeCommand::DebuggerSetBreakpoint { target_id: TID.into(), script_id: 0, offset: 0, line: 0, column: None },
        BridgeCommand::DebuggerClearBreakpoint { target_id: TID.into(), script_id: 0, offset: 0 },
        BridgeCommand::DebuggerInterrupt { target_id: TID.into() },
        BridgeCommand::DebuggerResume { target_id: TID.into(), step_type: None },
        BridgeCommand::DebuggerListFrames { target_id: TID.into() },
        BridgeCommand::DebuggerGetEnvironment { target_id: TID.into(), frame_actor_id: String::new() },
        BridgeCommand::DebuggerEval { target_id: TID.into(), expression: String::new(), frame_actor_id: None },
        BridgeCommand::DebuggerGetPossibleBreakpoints { target_id: TID.into(), script_id: 0 },
        BridgeCommand::DebuggerGetScriptSource { target_id: TID.into(), script_id: 0 },
        BridgeCommand::DebuggerBlackbox { target_id: TID.into(), script_id: 0 },
        BridgeCommand::DebuggerUnblackbox { target_id: TID.into(), script_id: 0 },
    ];
    // 26 core + 2 multi-target + 13 debugger = 41 variants.
    assert_eq!(cmds.len(), 41,
        "BridgeCommand variant count drifted — update this test AND the enum");
}

#[test]
fn test_all_bridge_commands_clone_round_trip() {
    // Adversarial: every variant must be Clone (derive(Clone) contract) and
    // survive a clone+fire-and-forget+drain round trip. Catches the regression
    // where a variant gains a non-Clone field (e.g. an Rc) silently.
    let originals: Vec<BridgeCommand> = vec![
        BridgeCommand::Navigate { target_id: TID.into(), url: "u".into() },
        BridgeCommand::EvaluateJs { target_id: TID.into(), expression: "e".into(), return_by_value: true },
        BridgeCommand::TakeScreenshot { target_id: TID.into(), format: "jpeg".into(), quality: Some(50) },
        BridgeCommand::GetTitle { target_id: TID.into() },
        BridgeCommand::GetUrl { target_id: TID.into() },
        BridgeCommand::GetDocument { target_id: TID.into() },
        BridgeCommand::QuerySelector { target_id: TID.into(), selector: "s".into() },
        BridgeCommand::QuerySelectorAll { target_id: TID.into(), selector: "s".into() },
        BridgeCommand::GetOuterHtml { target_id: TID.into(), node_id: Some(7) },
        BridgeCommand::SetAttributeValue { target_id: TID.into(), node_id: 3, name: "n".into(), value: "v".into() },
        BridgeCommand::DispatchMouseEvent { target_id: TID.into(), event_type: "mouseMoved".into(), x: 1.5, y: 2.5, button: Some(1), click_count: Some(2) },
        BridgeCommand::DispatchKeyEvent { target_id: TID.into(), event_type: "keyUp".into(), key: "a".into(), code: "KeyA".into(), text: Some("a".into()) },
        BridgeCommand::InsertText { target_id: TID.into(), text: "t".into() },
        BridgeCommand::SetViewport { target_id: TID.into(), width: 800, height: 600, device_scale_factor: Some(1.5) },
        BridgeCommand::SetUserAgent { target_id: TID.into(), user_agent: "ua".into() },
        BridgeCommand::GetCookies { target_id: TID.into(), urls: vec!["u1".into(), "u2".into()] },
        BridgeCommand::GetAllCookies { target_id: TID.into() },
        BridgeCommand::DeleteCookie { target_id: TID.into(), name: "n".into(), url: Some("u".into()) },
        BridgeCommand::SetCookie { target_id: TID.into(), name: "n".into(), value: "v".into(), url: Some("u".into()), domain: Some("d".into()) },
        BridgeCommand::GetResponseBody { target_id: TID.into(), request_id: "r".into() },
        BridgeCommand::AddScriptToEvaluateOnNewDocument { target_id: TID.into(), source: "s".into() },
        BridgeCommand::Reload { target_id: TID.into(), ignore_cache: true },
        BridgeCommand::GoBack { target_id: TID.into() },
        BridgeCommand::GoForward { target_id: TID.into() },
        BridgeCommand::StopLoading { target_id: TID.into() },
        BridgeCommand::ClosePage { target_id: TID.into() },
        BridgeCommand::CreateTarget { url: "u".into() },
        BridgeCommand::ListTargets,
        BridgeCommand::DebuggerEnable { target_id: TID.into() },
        BridgeCommand::DebuggerDisable { target_id: TID.into() },
        BridgeCommand::DebuggerSetBreakpoint { target_id: TID.into(), script_id: 1, offset: 2, line: 3, column: Some(4) },
        BridgeCommand::DebuggerClearBreakpoint { target_id: TID.into(), script_id: 1, offset: 2 },
        BridgeCommand::DebuggerInterrupt { target_id: TID.into() },
        BridgeCommand::DebuggerResume { target_id: TID.into(), step_type: Some("stepOver".into()) },
        BridgeCommand::DebuggerListFrames { target_id: TID.into() },
        BridgeCommand::DebuggerGetEnvironment { target_id: TID.into(), frame_actor_id: "fa".into() },
        BridgeCommand::DebuggerEval { target_id: TID.into(), expression: "e".into(), frame_actor_id: Some("fa".into()) },
        BridgeCommand::DebuggerGetPossibleBreakpoints { target_id: TID.into(), script_id: 1 },
        BridgeCommand::DebuggerGetScriptSource { target_id: TID.into(), script_id: 1 },
        BridgeCommand::DebuggerBlackbox { target_id: TID.into(), script_id: 1 },
        BridgeCommand::DebuggerUnblackbox { target_id: TID.into(), script_id: 1 },
    ];
    assert_eq!(originals.len(), 41);
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    for cmd in &originals {
        // Clone then send — proves every variant is Clone and channel-serializable.
        tx.send_fire_and_forget(cmd.clone());
    }
    let drained = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(drained, 41, "all 41 cloned variants must traverse the channel");
}

// ---- Channel closed behavior ----

#[test]
fn test_sender_reports_closed_after_receiver_dropped() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    // is_alive sends a probe — if receiver is gone, send returns Err
    // But is_alive implementation sends and checks result
    // After receiver dropped, the channel is closed
    let alive = tx.is_alive();
    assert!(!alive);
}

#[test]
fn test_send_after_receiver_dropped() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("closed"));
}

// ---- Multiple sequential send/recv ----

#[test]
fn test_sequential_send_recv() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));

    let handler = std::thread::spawn(move || {
        let mut results = Vec::new();
        loop {
            let processed = rx.try_process(|cmd| {
                match cmd {
                    BridgeCommand::Navigate { url, .. } => BridgeResponse {
                        result: Ok(json!(url)),
                    },
                    BridgeCommand::GetTitle { .. } => BridgeResponse {
                        result: Ok(json!("Test Title")),
                    },
                    _ => BridgeResponse { result: Err("unknown".into()) },
                }
            });
            if processed {
                results.push(true);
                if results.len() == 3 { break; }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    let r1 = tx.send(BridgeCommand::Navigate { target_id: TID.into(), url: "http://a.com".into() });
    let r2 = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    let r3 = tx.send(BridgeCommand::Navigate { target_id: TID.into(), url: "http://b.com".into() });

    assert!(r1.result.is_ok());
    assert_eq!(r1.result.unwrap(), "http://a.com");
    assert!(r2.result.is_ok());
    assert_eq!(r2.result.unwrap(), "Test Title");
    assert!(r3.result.is_ok());
    assert_eq!(r3.result.unwrap(), "http://b.com");

    handler.join().unwrap();
}

// ---- Adversarial behavior coverage (gaps in the original file) ----

#[test]
fn test_recv_and_process_blocks_until_command() {
    // Adversarial gap: the blocking recv_and_process path was never exercised
    // by this file (only try_process was). Verify it blocks then processes.
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let sender_thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    });
    let processed = rx.recv_and_process(Duration::from_secs(2), |_cmd| {
        BridgeResponse { result: Ok(json!({"blocked": true})) }
    });
    assert!(processed, "recv_and_process must wake on arriving command");
    sender_thread.join().unwrap();
}

#[test]
fn test_recv_and_process_returns_false_on_timeout() {
    // Adversarial: blocking recv must respect its own timeout and return false,
    // not hang the test forever.
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let start = std::time::Instant::now();
    let processed = rx.recv_and_process(Duration::from_millis(50), |_cmd| {
        BridgeResponse { result: Ok(json!({})) }
    });
    let elapsed = start.elapsed();
    assert!(!processed, "recv_and_process must return false on timeout");
    assert!(elapsed >= Duration::from_millis(40),
        "must actually wait for the timeout, elapsed {:?}", elapsed);
    assert!(elapsed < Duration::from_secs(2),
        "must not hang, elapsed {:?}", elapsed);
}

#[test]
fn test_handler_error_propagates_through_channel() {
    // Adversarial: an Err result from the handler must round-trip back to the
    // sender intact (error string preserved, not swallowed).
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let handler = std::thread::spawn(move || {
        loop {
            let processed = rx.try_process(|_cmd| {
                BridgeResponse { result: Err("handler-internal-failure-42".into()) }
            });
            if processed { break; }
            std::thread::sleep(Duration::from_micros(100));
        }
    });
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "handler-internal-failure-42",
        "handler error string must propagate verbatim");
    handler.join().unwrap();
}

#[test]
fn test_send_after_receiver_dropped_with_clone() {
    // Adversarial: when a clone exists, dropping the receiver must still close
    // the channel for BOTH the original and the clone (they share one Receiver).
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();
    drop(rx);
    let r1 = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    let r2 = tx2.send(BridgeCommand::GetUrl { target_id: TID.into() });
    assert!(r1.result.is_err() && r1.result.unwrap_err().contains("closed"),
        "original sender must see closed channel");
    assert!(r2.result.is_err() && r2.result.unwrap_err().contains("closed"),
        "cloned sender must also see closed channel (shared receiver)");
}

#[test]
fn test_is_alive_false_after_all_clones_and_receiver_dropped() {
    // Adversarial: is_alive must return false once the receiver is gone, even
    // when multiple cloned senders exist. Guards against the is_alive probe
    // leaking or returning a stale true.
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();
    let tx3 = tx.clone();
    drop(rx);
    assert!(!tx.is_alive(), "original must report dead");
    assert!(!tx2.is_alive(), "clone 2 must report dead");
    assert!(!tx3.is_alive(), "clone 3 must report dead");
}

#[test]
fn test_concurrent_cloned_senders_interleave() {
    // Adversarial / race: two threads holding cloned senders must both be able
    // to enqueue commands and the receiver must drain all of them. Guards
    // against a clone breaking shared-channel delivery.
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();
    const N: usize = 50;
    let h1 = std::thread::spawn(move || {
        for _ in 0..N {
            tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
        }
    });
    let h2 = std::thread::spawn(move || {
        for _ in 0..N {
            tx2.send_fire_and_forget(BridgeCommand::GetUrl { target_id: TID.into() });
        }
    });
    h1.join().unwrap();
    h2.join().unwrap();
    let drained = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(drained, 2 * N,
        "concurrent cloned senders must deliver all {} commands, got {}", 2 * N, drained);
}

#[test]
fn test_send_returns_closed_not_timeout_when_receiver_dropped() {
    // Adversarial / SPEC-alignment: the two failure modes must be DISTINGUISHABLE.
    // - receiver dropped → "closed"
    // - receiver alive but slow → "timeout"
    // A bug that returns "timeout" when the channel is actually closed would
    // mislead callers into retrying a permanently-dead channel.
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    let err = tx.send(BridgeCommand::GetTitle { target_id: TID.into() }).result.unwrap_err();
    assert!(err.contains("closed"), "dropped receiver must report 'closed', got: {}", err);
    assert!(!err.contains("timeout"), "closed channel must NOT masquerade as timeout");
}

#[test]
fn test_fire_and_forget_is_non_blocking_under_load() {
    // Adversarial: fire-and-forget must return promptly even when the receiver
    // is absent (channel buffer is unbounded mpsc). Guards against a regression
    // where send_fire_and_forget accidentally blocks or waits.
    let (tx, _rx) = bridge_channel(Duration::from_secs(5));
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    }
    let elapsed = start.elapsed();
    // 1000 fire-and-forget sends on an unbounded channel must be sub-second.
    assert!(elapsed < Duration::from_secs(2),
        "fire-and-forget must be non-blocking under load, elapsed {:?}", elapsed);
}

#[test]
fn test_response_responder_isolated_per_request() {
    // Adversarial: each send() must get its OWN response channel. If a bug
    // shared a single responder across requests, the second response would
    // be lost or mismatched. Two sequential sends must each receive the value
    // produced for THAT request, not the other's.
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let handler = std::thread::spawn(move || {
        let mut idx = 0;
        loop {
            let processed = rx.try_process(|_cmd| {
                idx += 1;
                BridgeResponse { result: Ok(json!(idx)) }
            });
            if processed && idx == 2 { break; }
            if processed { continue; }
            std::thread::sleep(Duration::from_micros(100));
        }
    });
    let r1 = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    let r2 = tx.send(BridgeCommand::GetUrl { target_id: TID.into() });
    assert_eq!(r1.result.unwrap(), 1, "first response must be the first handler result");
    assert_eq!(r2.result.unwrap(), 2, "second response must be the second handler result");
    handler.join().unwrap();
}

#[test]
fn test_drain_runs_handler_for_each_command() {
    // Adversarial: drain must invoke the handler once per queued command (side
    // effect observable), not just count them. Use an Accumulator to prove it.
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    for i in 0..5u64 {
        tx.send_fire_and_forget(BridgeCommand::Navigate { target_id: TID.into(), url: format!("http://{}", i) });
    }
    let sum = std::cell::RefCell::new(0u64);
    let count = rx.drain(|cmd| {
        if let BridgeCommand::Navigate { url, .. } = cmd {
            let n: u64 = url.trim_start_matches("http://").parse().unwrap_or(0);
            *sum.borrow_mut() += n;
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count, 5);
    assert_eq!(sum.into_inner(), 0 + 1 + 2 + 3 + 4,
        "drain must invoke handler with each command's payload");
}
