// @trace TEST-CDP-028 [req:REQ-CDP-003,REQ-CDP-006] [level:unit]
// BridgeChannel timeout behavior, fire-and-forget, clone semantics,
// BridgeResponse result variants, BridgeCommand field completeness.

use std::time::Duration;

use bao_cdp::{BridgeSender, BridgeReceiver, BridgeCommand, BridgeResponse, bridge_channel};
use serde_json::json;

// ---- bridge_channel creation ----

#[test]
fn test_bridge_channel_creates_pair() {
    let (_tx, _rx) = bridge_channel(Duration::from_secs(5));
}

#[test]
fn test_bridge_channel_short_timeout() {
    let (_tx, _rx) = bridge_channel(Duration::from_millis(1));
}

#[test]
fn test_bridge_channel_long_timeout() {
    let (_tx, _rx) = bridge_channel(Duration::from_secs(300));
}

// ---- BridgeSender::send + BridgeReceiver::try_process ----

#[test]
fn test_send_and_process_navigate() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let response = tx.send(BridgeCommand::Navigate { url: "http://test".into() });
    // No handler running, so this will timeout
    assert!(response.result.is_err());
    assert!(response.result.unwrap_err().contains("timeout"));
}

#[test]
fn test_send_with_handler() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));

    // Start handler thread
    let handler = std::thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::Navigate { url } => BridgeResponse {
                    result: Ok(json!({"url": url})),
                },
                _ => BridgeResponse { result: Err("unexpected".into()) },
            }
        })
    });

    // Give handler time to be ready
    std::thread::sleep(Duration::from_millis(10));

    // try_process on empty channel returns false
    let processed = handler.join().unwrap();
    assert!(!processed);
}

#[test]
fn test_send_and_recv_success() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));

    // Handler thread
    let handler = std::thread::spawn(move || {
        // Block until a request arrives
        loop {
            let processed = rx.try_process(|cmd| {
                BridgeResponse { result: Ok(json!({"handled": true})) }
            });
            if processed { break; }
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    let resp = tx.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["handled"], true);

    handler.join().unwrap();
}

#[test]
fn test_send_timeout_response() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(10));
    let resp = tx.send(BridgeCommand::GetUrl);
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("timeout"));
}

// ---- BridgeSender::send_fire_and_forget ----

#[test]
fn test_fire_and_forget_no_block() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    // Should return immediately
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx.send_fire_and_forget(BridgeCommand::GetUrl);
    tx.send_fire_and_forget(BridgeCommand::GetDocument);

    // Drain should pick up all 3
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 3);
}

// ---- BridgeSender::clone ----

#[test]
fn test_sender_clone_works() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();

    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx2.send_fire_and_forget(BridgeCommand::GetUrl);

    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 2);
}

#[test]
fn test_sender_clone_same_timeout() {
    let (tx, _rx) = bridge_channel(Duration::from_secs(7));
    let tx2 = tx.clone();
    // Both should work — just verify no panic
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx2.send_fire_and_forget(BridgeCommand::GetTitle);
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
    for i in 0..10 {
        tx.send_fire_and_forget(BridgeCommand::GetTitle);
    }
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 10);
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
    let cmd = BridgeCommand::Navigate { url: "http://example.com".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("Navigate"));
    assert!(debug.contains("http://example.com"));
}

#[test]
fn test_bridge_command_evaluate_js_debug() {
    let cmd = BridgeCommand::EvaluateJs { expression: "1+1".into(), return_by_value: true };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("EvaluateJs"));
}

#[test]
fn test_bridge_command_screenshot_debug() {
    let cmd = BridgeCommand::TakeScreenshot { format: "png".into(), quality: Some(80) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("TakeScreenshot"));
}

#[test]
fn test_bridge_command_query_selector_debug() {
    let cmd = BridgeCommand::QuerySelector { selector: "div.test".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("QuerySelector"));
}

#[test]
fn test_bridge_command_dispatch_mouse_debug() {
    let cmd = BridgeCommand::DispatchMouseEvent {
        event_type: "mousePressed".into(),
        x: 100.0, y: 200.0,
        button: Some(0),
        click_count: Some(1),
    };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DispatchMouseEvent"));
}

#[test]
fn test_bridge_command_dispatch_key_debug() {
    let cmd = BridgeCommand::DispatchKeyEvent {
        event_type: "keyDown".into(),
        key: "Enter".into(),
        code: "Enter".into(),
        text: None,
    };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DispatchKeyEvent"));
}

#[test]
fn test_bridge_command_set_viewport_debug() {
    let cmd = BridgeCommand::SetViewport { width: 1920, height: 1080, device_scale_factor: Some(2.0) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("SetViewport"));
}

#[test]
fn test_bridge_command_set_user_agent_debug() {
    let cmd = BridgeCommand::SetUserAgent { user_agent: "Bao/1.0".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("SetUserAgent"));
}

#[test]
fn test_bridge_command_get_cookies_debug() {
    let cmd = BridgeCommand::GetCookies { urls: vec!["http://a.com".into()] };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetCookies"));
}

#[test]
fn test_bridge_command_get_all_cookies_debug() {
    let cmd = BridgeCommand::GetAllCookies;
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetAllCookies"));
}

#[test]
fn test_bridge_command_delete_cookie_debug() {
    let cmd = BridgeCommand::DeleteCookie { name: "session".into(), url: Some("http://a.com".into()) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("DeleteCookie"));
}

#[test]
fn test_bridge_command_set_cookie_debug() {
    let cmd = BridgeCommand::SetCookie {
        name: "foo".into(),
        value: "bar".into(),
        url: Some("http://a.com".into()),
        domain: None,
    };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("SetCookie"));
}

#[test]
fn test_bridge_command_get_response_body_debug() {
    let cmd = BridgeCommand::GetResponseBody { request_id: "req-123".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetResponseBody"));
}

#[test]
fn test_bridge_command_add_script_debug() {
    let cmd = BridgeCommand::AddScriptToEvaluateOnNewDocument { source: "console.log(1)".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("AddScriptToEvaluateOnNewDocument"));
}

#[test]
fn test_bridge_command_reload_debug() {
    let cmd = BridgeCommand::Reload { ignore_cache: true };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("Reload"));
}

#[test]
fn test_bridge_command_go_back_debug() {
    let cmd = BridgeCommand::GoBack;
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GoBack"));
}

#[test]
fn test_bridge_command_go_forward_debug() {
    let cmd = BridgeCommand::GoForward;
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GoForward"));
}

#[test]
fn test_bridge_command_stop_loading_debug() {
    let cmd = BridgeCommand::StopLoading;
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("StopLoading"));
}

#[test]
fn test_bridge_command_close_page_debug() {
    let cmd = BridgeCommand::ClosePage;
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("ClosePage"));
}

#[test]
fn test_bridge_command_set_attribute_value_debug() {
    let cmd = BridgeCommand::SetAttributeValue { node_id: 5, name: "class".into(), value: "active".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("SetAttributeValue"));
}

#[test]
fn test_bridge_command_get_outer_html_debug() {
    let cmd = BridgeCommand::GetOuterHtml { node_id: Some(3) };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetOuterHtml"));
}

#[test]
fn test_bridge_command_query_selector_all_debug() {
    let cmd = BridgeCommand::QuerySelectorAll { selector: "li".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("QuerySelectorAll"));
}

#[test]
fn test_bridge_command_insert_text_debug() {
    let cmd = BridgeCommand::InsertText { text: "hello".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("InsertText"));
}

// ---- BridgeCommand field completeness (no Debug needed, just construction) ----

#[test]
fn test_all_bridge_commands_constructible() {
    // Verify all 24 variants compile
    let cmds: Vec<BridgeCommand> = vec![
        BridgeCommand::Navigate { url: String::new() },
        BridgeCommand::EvaluateJs { expression: String::new(), return_by_value: false },
        BridgeCommand::TakeScreenshot { format: "png".into(), quality: None },
        BridgeCommand::GetTitle,
        BridgeCommand::GetUrl,
        BridgeCommand::GetDocument,
        BridgeCommand::QuerySelector { selector: String::new() },
        BridgeCommand::QuerySelectorAll { selector: String::new() },
        BridgeCommand::GetOuterHtml { node_id: None },
        BridgeCommand::SetAttributeValue { node_id: 0, name: String::new(), value: String::new() },
        BridgeCommand::DispatchMouseEvent { event_type: String::new(), x: 0.0, y: 0.0, button: None, click_count: None },
        BridgeCommand::DispatchKeyEvent { event_type: String::new(), key: String::new(), code: String::new(), text: None },
        BridgeCommand::InsertText { text: String::new() },
        BridgeCommand::SetViewport { width: 0, height: 0, device_scale_factor: None },
        BridgeCommand::SetUserAgent { user_agent: String::new() },
        BridgeCommand::GetCookies { urls: vec![] },
        BridgeCommand::GetAllCookies,
        BridgeCommand::DeleteCookie { name: String::new(), url: None },
        BridgeCommand::SetCookie { name: String::new(), value: String::new(), url: None, domain: None },
        BridgeCommand::GetResponseBody { request_id: String::new() },
        BridgeCommand::AddScriptToEvaluateOnNewDocument { source: String::new() },
        BridgeCommand::Reload { ignore_cache: false },
        BridgeCommand::GoBack,
        BridgeCommand::GoForward,
        BridgeCommand::StopLoading,
        BridgeCommand::ClosePage,
    ];
    assert_eq!(cmds.len(), 26);
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
    let resp = tx.send(BridgeCommand::GetTitle);
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
                    BridgeCommand::Navigate { url } => BridgeResponse {
                        result: Ok(json!(url)),
                    },
                    BridgeCommand::GetTitle => BridgeResponse {
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

    let r1 = tx.send(BridgeCommand::Navigate { url: "http://a.com".into() });
    let r2 = tx.send(BridgeCommand::GetTitle);
    let r3 = tx.send(BridgeCommand::Navigate { url: "http://b.com".into() });

    assert!(r1.result.is_ok());
    assert_eq!(r1.result.unwrap(), "http://a.com");
    assert!(r2.result.is_ok());
    assert_eq!(r2.result.unwrap(), "Test Title");
    assert!(r3.result.is_ok());
    assert_eq!(r3.result.unwrap(), "http://b.com");

    handler.join().unwrap();
}
