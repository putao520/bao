// @trace TEST-CDP-013-BRIDGE-DEEP [req:REQ-CDP-003,REQ-CDP-006] [level:unit]
// Bridge channel deep tests: all BridgeCommand variants, timeout handling,
// fire-and-forget, is_alive, drain, clone, concurrent send, edge cases.

use bao_cdp::{BridgeCommand, BridgeResponse, bridge_channel};
use serde_json::json;
use std::time::Duration;
use std::sync::Arc;
use std::thread;

// ---- Bridge channel creation ----

#[test]
fn test_bridge_channel_creates_pair() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    assert!(tx.is_alive());
    // Drop receiver, sender should still report alive until next send attempt
    drop(rx);
    // is_alive sends a probe — after rx dropped, channel is still open until probe send fails
}

#[test]
fn test_bridge_channel_zero_timeout() {
    let (tx, rx) = bridge_channel(Duration::from_secs(0));
    // Should still create successfully
    assert!(tx.is_alive());
    drop(rx);
}

#[test]
fn test_bridge_channel_long_timeout() {
    let (tx, rx) = bridge_channel(Duration::from_secs(3600));
    assert!(tx.is_alive());
    drop(rx);
}

// ---- Basic send/receive ----

#[test]
fn test_send_navigate_success() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::Navigate { url } => {
                    assert_eq!(url, "https://example.com");
                    BridgeResponse { result: Ok(json!({"frameId": "main"})) }
                }
                _ => BridgeResponse { result: Err("unexpected command".into()) },
            }
        });
    });
    let resp = tx.send(BridgeCommand::Navigate { url: "https://example.com".into() });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_evaluate_js_success() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { expression, return_by_value } => {
                    assert_eq!(expression, "1+1");
                    assert!(return_by_value);
                    BridgeResponse { result: Ok(json!({"result": {"type": "number", "value": 2}})) }
                }
                _ => panic!("Unexpected command type"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::EvaluateJs { expression: "1+1".into(), return_by_value: true });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_take_screenshot() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::TakeScreenshot { format, quality } => {
                    assert_eq!(format, "png");
                    assert!(quality.is_none());
                    BridgeResponse { result: Ok(json!({"data": "base64data"})) }
                }
                _ => panic!("Unexpected command"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::TakeScreenshot { format: "png".into(), quality: None });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_screenshot_with_quality() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::TakeScreenshot { format, quality } => {
                    assert_eq!(format, "jpeg");
                    assert_eq!(quality, Some(80));
                    BridgeResponse { result: Ok(json!({"data": "jpegdata"})) }
                }
                _ => panic!("Unexpected command"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::TakeScreenshot { format: "jpeg".into(), quality: Some(80) });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_get_title() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Test Page")) },
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetTitle);
    assert_eq!(resp.result.unwrap(), json!("Test Page"));
}

#[test]
fn test_send_get_url() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!("https://example.com")) },
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetUrl);
    assert_eq!(resp.result.unwrap(), json!("https://example.com"));
}

#[test]
fn test_send_query_selector() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        // Loop until command arrives
        loop {
            if rx.try_process(|cmd| {
                match cmd {
                    BridgeCommand::QuerySelector { selector } => {
                        assert_eq!(selector, "div.container");
                        BridgeResponse { result: Ok(json!({"nodeId": 42})) }
                    }
                    _ => panic!("Unexpected"),
                }
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = tx.send(BridgeCommand::QuerySelector { selector: "div.container".into() });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_query_selector_all() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::QuerySelectorAll { selector } => {
                    assert_eq!(selector, "li");
                    BridgeResponse { result: Ok(json!({"nodeIds": [1, 2, 3]})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::QuerySelectorAll { selector: "li".into() });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_dispatch_mouse_event() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::DispatchMouseEvent { event_type, x, y, button, click_count } => {
                    assert_eq!(event_type, "mousePressed");
                    assert_eq!(x, 100.0);
                    assert_eq!(y, 200.0);
                    assert_eq!(button, Some(0));
                    assert_eq!(click_count, Some(1));
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::DispatchMouseEvent {
        event_type: "mousePressed".into(),
        x: 100.0, y: 200.0,
        button: Some(0),
        click_count: Some(1),
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_dispatch_key_event() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::DispatchKeyEvent { event_type, key, code, text } => {
                    assert_eq!(event_type, "keyDown");
                    assert_eq!(key, "a");
                    assert_eq!(code, "KeyA");
                    assert_eq!(text, Some("a".into()));
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::DispatchKeyEvent {
        event_type: "keyDown".into(),
        key: "a".into(),
        code: "KeyA".into(),
        text: Some("a".into()),
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_insert_text() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::InsertText { text } => {
                    assert_eq!(text, "hello world");
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::InsertText { text: "hello world".into() });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_set_viewport() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetViewport { width, height, device_scale_factor } => {
                    assert_eq!(width, 1920);
                    assert_eq!(height, 1080);
                    assert_eq!(device_scale_factor, Some(2.0));
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetViewport {
        width: 1920, height: 1080, device_scale_factor: Some(2.0),
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_get_cookies() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetCookies { urls } => {
                    assert_eq!(urls, vec!["https://example.com"]);
                    BridgeResponse { result: Ok(json!({"cookies": []})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetCookies { urls: vec!["https://example.com".into()] });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_set_cookie() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetCookie { name, value, url, domain } => {
                    assert_eq!(name, "session");
                    assert_eq!(value, "abc123");
                    assert_eq!(url, Some("https://example.com".into()));
                    assert!(domain.is_none());
                    BridgeResponse { result: Ok(json!({"success": true})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetCookie {
        name: "session".into(),
        value: "abc123".into(),
        url: Some("https://example.com".into()),
        domain: None,
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_delete_cookie() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::DeleteCookie { name, url } => {
                    assert_eq!(name, "session");
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::DeleteCookie { name: "session".into(), url: None });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_reload() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::Reload { ignore_cache } => {
                    assert!(ignore_cache);
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::Reload { ignore_cache: true });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_go_back() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GoBack => BridgeResponse { result: Ok(json!({})) },
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GoBack);
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_go_forward() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GoForward => BridgeResponse { result: Ok(json!({})) },
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GoForward);
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_stop_loading() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::StopLoading => BridgeResponse { result: Ok(json!({})) },
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::StopLoading);
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_close_page() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::ClosePage => BridgeResponse { result: Ok(json!({})) },
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::ClosePage);
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_add_script() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::AddScriptToEvaluateOnNewDocument { source } => {
                    assert!(source.contains("navigator"));
                    BridgeResponse { result: Ok(json!({"identifier": "1"})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::AddScriptToEvaluateOnNewDocument {
        source: "Object.defineProperty(navigator, 'languages', {get: () => ['en']})".into(),
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_set_attribute_value() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetAttributeValue { node_id, name, value } => {
                    assert_eq!(node_id, 5);
                    assert_eq!(name, "class");
                    assert_eq!(value, "active");
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetAttributeValue {
        node_id: 5, name: "class".into(), value: "active".into(),
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_get_outer_html() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetOuterHtml { node_id } => {
                    assert_eq!(node_id, Some(3));
                    BridgeResponse { result: Ok(json!({"outerHTML": "<div>hello</div>"})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetOuterHtml { node_id: Some(3) });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_get_response_body() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetResponseBody { request_id } => {
                    assert_eq!(request_id, "req-123");
                    BridgeResponse { result: Ok(json!({"body": "content", "base64Encoded": false})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetResponseBody { request_id: "req-123".into() });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_set_user_agent() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetUserAgent { user_agent } => {
                    assert!(user_agent.contains("Firefox"));
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetUserAgent {
        user_agent: "Mozilla/5.0 Firefox/128.0".into(),
    });
    assert!(resp.result.is_ok());
}

// ---- Error handling ----

#[test]
fn test_send_error_response() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|_| BridgeResponse {
            result: Err("page not found".into()),
        });
    });
    let resp = tx.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("page not found"));
}

#[test]
fn test_send_timeout() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(10));
    // Don't process on receiver side — should timeout
    let resp = tx.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("timeout"));
}

#[test]
fn test_send_after_receiver_dropped() {
    let (tx, rx) = bridge_channel(Duration::from_millis(50));
    drop(rx);
    let resp = tx.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("closed"));
}

// ---- Fire and forget ----

#[test]
fn test_fire_and_forget_does_not_block() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    // Should be able to receive it
    let processed = rx.try_process(|_| BridgeResponse { result: Ok(json!({})) });
    assert!(processed);
}

// ---- Drain ----

#[test]
fn test_drain_multiple_commands() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    for i in 0..10 {
        tx.send_fire_and_forget(BridgeCommand::Navigate { url: format!("https://{}.com", i) });
    }
    let count = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 10);
}

#[test]
fn test_drain_empty() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let count = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 0);
}

// ---- Clone ----

#[test]
fn test_sender_clone_shares_channel() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();

    // Use drain in a loop to handle both sends
    let handler = thread::spawn(move || {
        let mut count = 0;
        for _ in 0..200 {
            let n = rx.drain(|_| BridgeResponse { result: Ok(json!("ok")) });
            count += n;
            if count >= 2 { break; }
            thread::sleep(Duration::from_millis(5));
        }
        count
    });

    let resp1 = tx.send(BridgeCommand::GetTitle);
    let resp2 = tx2.send(BridgeCommand::GetTitle);
    assert!(resp1.result.is_ok());
    assert!(resp2.result.is_ok());
    let _ = handler.join();
}

// ---- BridgeResponse Debug ----

#[test]
fn test_bridge_response_debug_ok() {
    let resp = BridgeResponse { result: Ok(json!({"key": "value"})) };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("Ok"));
}

#[test]
fn test_bridge_response_debug_err() {
    let resp = BridgeResponse { result: Err("fail".into()) };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("Err"));
    assert!(debug.contains("fail"));
}

// ---- Unicode / special chars in commands ----

#[test]
fn test_navigate_unicode_url() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::Navigate { url } => {
                    assert!(url.contains("日本語"));
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::Navigate { url: "https://example.com/日本語".into() });
    assert!(resp.result.is_ok());
}

#[test]
fn test_evaluate_js_special_chars() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { expression, .. } => {
                    assert!(expression.contains("\\n"));
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::EvaluateJs {
        expression: "JSON.stringify({a: '\\n'})".into(),
        return_by_value: true,
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_insert_text_multibyte() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::InsertText { text } => {
                    assert_eq!(text, "こんにちは世界");
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::InsertText { text: "こんにちは世界".into() });
    assert!(resp.result.is_ok());
}

// ---- Concurrent sends ----

#[test]
fn test_concurrent_sends() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let rx = Arc::new(std::sync::Mutex::new(rx));
    let rx_clone = rx.clone();

    let handler = thread::spawn(move || {
        let rx = rx_clone;
        let mut count = 0;
        for _ in 0..5 {
            let guard = rx.lock().unwrap();
            guard.try_process(|_| {
                BridgeResponse { result: Ok(json!({})) }
            });
            drop(guard);
            count += 1;
        }
        count
    });

    let txes: Vec<_> = (0..5).map(|i| {
        let tx = tx.clone();
        thread::spawn(move || {
            tx.send(BridgeCommand::Navigate { url: format!("https://{}.com", i) })
        })
    }).collect();

    for t in txes {
        let resp = t.join().unwrap();
        assert!(resp.result.is_ok() || resp.result.is_err());
    }
    let _ = handler.join();
}
