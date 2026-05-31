// @trace TEST-CDP-014-EXHAUSTIVE [req:REQ-CDP-003,REQ-CDP-006] [level:unit]
// BridgeCommand exhaustive variant coverage: remaining edge cases for
// GetDocument, GetAllCookies, SetViewport without scale factor,
// DispatchMouseEvent without optional fields, DispatchKeyEvent without text,
// SetCookie with domain, DeleteCookie with url, GetOuterHtml without node_id,
// AddScriptToEvaluateOnNewDocument empty source, Navigate empty url,
// bridge_channel stress with rapid sends, receiver drop mid-drain.

use bao_cdp::{BridgeCommand, BridgeResponse, bridge_channel};
use serde_json::json;
use std::time::Duration;
use std::thread;

// ---- GetDocument ----

#[test]
fn test_send_get_document() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetDocument => {
                    BridgeResponse { result: Ok(json!({"root": {"nodeId": 1, "children": []}})) }
                }
                _ => panic!("Unexpected command"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetDocument);
    let result = resp.result.unwrap();
    assert!(result.get("root").is_some());
}

// ---- GetAllCookies ----

#[test]
fn test_send_get_all_cookies_empty() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetAllCookies => {
                    BridgeResponse { result: Ok(json!({"cookies": []})) }
                }
                _ => panic!("Unexpected command"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetAllCookies);
    let result = resp.result.unwrap();
    assert_eq!(result["cookies"].as_array().unwrap().len(), 0);
}

#[test]
fn test_send_get_all_cookies_with_data() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetAllCookies => {
                    BridgeResponse {
                        result: Ok(json!({"cookies": [
                            {"name": "a", "value": "1", "domain": ".example.com"},
                            {"name": "b", "value": "2", "domain": ".test.com"},
                        ]})),
                    }
                }
                _ => panic!("Unexpected command"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetAllCookies);
    let result = resp.result.unwrap();
    assert_eq!(result["cookies"].as_array().unwrap().len(), 2);
}

// ---- SetViewport without device_scale_factor ----

#[test]
fn test_send_set_viewport_no_scale_factor() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetViewport { width, height, device_scale_factor } => {
                    assert_eq!(width, 800);
                    assert_eq!(height, 600);
                    assert!(device_scale_factor.is_none());
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected command"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetViewport {
        width: 800, height: 600, device_scale_factor: None,
    });
    assert!(resp.result.is_ok());
}

// ---- DispatchMouseEvent without optional fields ----

#[test]
fn test_send_mouse_event_no_optional() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::DispatchMouseEvent { event_type, x, y, button, click_count } => {
                    assert_eq!(event_type, "mouseMoved");
                    assert_eq!(x, 50.0);
                    assert_eq!(y, 75.0);
                    assert!(button.is_none());
                    assert!(click_count.is_none());
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected command"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::DispatchMouseEvent {
        event_type: "mouseMoved".into(),
        x: 50.0, y: 75.0,
        button: None, click_count: None,
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_mouse_event_released() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::DispatchMouseEvent { event_type, .. } => {
                    assert_eq!(event_type, "mouseReleased");
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::DispatchMouseEvent {
        event_type: "mouseReleased".into(),
        x: 0.0, y: 0.0,
        button: Some(0), click_count: Some(1),
    });
    assert!(resp.result.is_ok());
}

// ---- DispatchKeyEvent without text ----

#[test]
fn test_send_key_event_no_text() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::DispatchKeyEvent { event_type, key, code, text } => {
                    assert_eq!(event_type, "keyUp");
                    assert_eq!(key, "Enter");
                    assert_eq!(code, "Enter");
                    assert!(text.is_none());
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::DispatchKeyEvent {
        event_type: "keyUp".into(),
        key: "Enter".into(),
        code: "Enter".into(),
        text: None,
    });
    assert!(resp.result.is_ok());
}

#[test]
fn test_send_key_event_special_key() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::DispatchKeyEvent { key, code, .. } => {
                    assert_eq!(key, "Tab");
                    assert_eq!(code, "Tab");
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::DispatchKeyEvent {
        event_type: "keyDown".into(),
        key: "Tab".into(),
        code: "Tab".into(),
        text: None,
    });
    assert!(resp.result.is_ok());
}

// ---- SetCookie with domain ----

#[test]
fn test_send_set_cookie_with_domain() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetCookie { name, value, url, domain } => {
                    assert_eq!(name, "sid");
                    assert_eq!(value, "xyz789");
                    assert!(url.is_none());
                    assert_eq!(domain, Some(".example.com".into()));
                    BridgeResponse { result: Ok(json!({"success": true})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetCookie {
        name: "sid".into(),
        value: "xyz789".into(),
        url: None,
        domain: Some(".example.com".into()),
    });
    assert!(resp.result.is_ok());
}

// ---- DeleteCookie with url ----

#[test]
fn test_send_delete_cookie_with_url() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::DeleteCookie { name, url } => {
                    assert_eq!(name, "session");
                    assert_eq!(url, Some("https://example.com".into()));
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::DeleteCookie {
        name: "session".into(),
        url: Some("https://example.com".into()),
    });
    assert!(resp.result.is_ok());
}

// ---- GetOuterHtml without node_id ----

#[test]
fn test_send_get_outer_html_no_node_id() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetOuterHtml { node_id } => {
                    assert!(node_id.is_none());
                    BridgeResponse { result: Ok(json!({"outerHTML": "<html><body></body></html>"})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetOuterHtml { node_id: None });
    assert!(resp.result.is_ok());
}

// ---- AddScriptToEvaluateOnNewDocument empty source ----

#[test]
fn test_send_add_script_empty_source() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::AddScriptToEvaluateOnNewDocument { source } => {
                    assert!(source.is_empty());
                    BridgeResponse { result: Ok(json!({"identifier": "2"})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::AddScriptToEvaluateOnNewDocument {
        source: String::new(),
    });
    assert!(resp.result.is_ok());
}

// ---- Navigate empty url ----

#[test]
fn test_send_navigate_empty_url() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::Navigate { url } => {
                    assert!(url.is_empty());
                    BridgeResponse { result: Ok(json!({"frameId": "main"})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::Navigate { url: String::new() });
    assert!(resp.result.is_ok());
}

// ---- Navigate very long url ----

#[test]
fn test_send_navigate_long_url() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let long_url = format!("https://example.com/{}", "a".repeat(10000));
    let url_clone = long_url.clone();
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::Navigate { url } => {
                    assert_eq!(url.len(), url_clone.len());
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::Navigate { url: long_url });
    assert!(resp.result.is_ok());
}

// ---- Reload without cache ----

#[test]
fn test_send_reload_without_ignore_cache() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::Reload { ignore_cache } => {
                    assert!(!ignore_cache);
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::Reload { ignore_cache: false });
    assert!(resp.result.is_ok());
}

// ---- SetUserAgent ----

#[test]
fn test_send_set_user_agent_chrome() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetUserAgent { user_agent } => {
                    assert!(user_agent.contains("Chrome"));
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetUserAgent {
        user_agent: "Mozilla/5.0 Chrome/120.0".into(),
    });
    assert!(resp.result.is_ok());
}

// ---- QuerySelector with complex selector ----

#[test]
fn test_send_query_selector_complex() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::QuerySelector { selector } => {
                    assert!(selector.contains("#main"));
                    assert!(selector.contains(".content"));
                    BridgeResponse { result: Ok(json!({"nodeId": 99})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::QuerySelector {
        selector: "#main .content > p:first-child".into(),
    });
    assert!(resp.result.is_ok());
}

// ---- EvaluateJs with return_by_value false ----

#[test]
fn test_send_evaluate_js_no_return_by_value() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::EvaluateJs { expression, return_by_value } => {
                    assert_eq!(expression, "document.body");
                    assert!(!return_by_value);
                    BridgeResponse { result: Ok(json!({"result": {"type": "object", "objectId": "obj-1"}})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::EvaluateJs {
        expression: "document.body".into(),
        return_by_value: false,
    });
    assert!(resp.result.is_ok());
}

// ---- GetCookies with multiple urls ----

#[test]
fn test_send_get_cookies_multiple_urls() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetCookies { urls } => {
                    assert_eq!(urls.len(), 3);
                    BridgeResponse { result: Ok(json!({"cookies": [{"name": "c"}]})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetCookies {
        urls: vec![
            "https://a.com".into(),
            "https://b.com".into(),
            "https://c.com".into(),
        ],
    });
    assert!(resp.result.is_ok());
}

// ---- GetCookies empty urls ----

#[test]
fn test_send_get_cookies_empty_urls() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetCookies { urls } => {
                    assert!(urls.is_empty());
                    BridgeResponse { result: Ok(json!({"cookies": []})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetCookies { urls: vec![] });
    assert!(resp.result.is_ok());
}

// ---- SetAttributeValue ----

#[test]
fn test_send_set_attribute_value_id() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetAttributeValue { node_id, name, value } => {
                    assert_eq!(node_id, 10);
                    assert_eq!(name, "id");
                    assert_eq!(value, "header");
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetAttributeValue {
        node_id: 10, name: "id".into(), value: "header".into(),
    });
    assert!(resp.result.is_ok());
}

// ---- InsertText empty ----

#[test]
fn test_send_insert_text_empty() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::InsertText { text } => {
                    assert!(text.is_empty());
                    BridgeResponse { result: Ok(json!({})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::InsertText { text: String::new() });
    assert!(resp.result.is_ok());
}

// ---- Stress: rapid fire-and-forget + drain ----

#[test]
fn test_rapid_fire_and_forget_then_drain() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let total = 50;
    for i in 0..total {
        tx.send_fire_and_forget(BridgeCommand::Navigate { url: format!("{}", i) });
    }
    let count = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, total);
}

// ---- Stress: mixed command types in drain ----

#[test]
fn test_drain_mixed_commands() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::Navigate { url: "a".into() });
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx.send_fire_and_forget(BridgeCommand::GetUrl);
    tx.send_fire_and_forget(BridgeCommand::Reload { ignore_cache: true });
    tx.send_fire_and_forget(BridgeCommand::StopLoading);

    let counts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut expected = 0;

    let total = rx.drain(|_cmd| {
        counts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });

    // All 5 were sent and drained
    assert_eq!(total, 5);
}

// ---- Receiver drop during drain ----

#[test]
fn test_drain_after_close_page_command() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::ClosePage);
    let count = rx.drain(|cmd| {
        match cmd {
            BridgeCommand::ClosePage => BridgeResponse { result: Ok(json!({})) },
            _ => panic!("Unexpected"),
        }
    });
    assert_eq!(count, 1);
}

// ---- SetCookie all fields populated ----

#[test]
fn test_send_set_cookie_all_fields() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::SetCookie { name, value, url, domain } => {
                    assert_eq!(name, "test");
                    assert_eq!(value, "val");
                    assert_eq!(url, Some("https://a.com".into()));
                    assert_eq!(domain, Some(".a.com".into()));
                    BridgeResponse { result: Ok(json!({"success": true})) }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::SetCookie {
        name: "test".into(),
        value: "val".into(),
        url: Some("https://a.com".into()),
        domain: Some(".a.com".into()),
    });
    assert!(resp.result.is_ok());
}

// ---- GetResponseBody with different request_id ----

#[test]
fn test_send_get_response_body_base64() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    thread::spawn(move || {
        rx.try_process(|cmd| {
            match cmd {
                BridgeCommand::GetResponseBody { request_id } => {
                    assert_eq!(request_id, "req-base64");
                    BridgeResponse {
                        result: Ok(json!({"body": "SGVsbG8=", "base64Encoded": true})),
                    }
                }
                _ => panic!("Unexpected"),
            }
        });
    });
    let resp = tx.send(BridgeCommand::GetResponseBody { request_id: "req-base64".into() });
    let result = resp.result.unwrap();
    assert_eq!(result["base64Encoded"], true);
}

// ---- BridgeResponse result values ----

#[test]
fn test_bridge_response_ok_null() {
    let resp = BridgeResponse { result: Ok(json!(null)) };
    assert!(resp.result.is_ok());
    assert!(resp.result.unwrap().is_null());
}

#[test]
fn test_bridge_response_ok_string() {
    let resp = BridgeResponse { result: Ok(json!("hello")) };
    assert_eq!(resp.result.unwrap(), json!("hello"));
}

#[test]
fn test_bridge_response_ok_number() {
    let resp = BridgeResponse { result: Ok(json!(42)) };
    assert_eq!(resp.result.unwrap(), 42);
}

#[test]
fn test_bridge_response_ok_array() {
    let resp = BridgeResponse { result: Ok(json!([1, 2, 3])) };
    assert_eq!(resp.result.unwrap().as_array().unwrap().len(), 3);
}

// ---- is_alive after receiver dropped ----

#[test]
fn test_is_alive_after_receiver_drop() {
    let (tx, rx) = bridge_channel(Duration::from_secs(1));
    drop(rx);
    // is_alive sends a probe internally; after rx dropped, channel closed
    assert!(!tx.is_alive());
}

// ---- is_alive while receiver active ----

#[test]
fn test_is_alive_with_active_receiver() {
    let (tx, rx) = bridge_channel(Duration::from_secs(1));
    // is_alive sends a probe command; receiver needs to consume it
    let alive = tx.is_alive();
    assert!(alive);
    // Consume the probe
    rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    drop(rx);
}
