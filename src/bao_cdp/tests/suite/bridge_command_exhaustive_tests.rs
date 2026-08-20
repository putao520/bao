// @trace TEST-CDP-018 [req:REQ-CDP-001,REQ-CDP-004] [level:unit]
// BridgeCommand exhaustive construction + BridgeResponse edge cases:
// all 25 variants, clone/debug, field validation, response result types.

use bao_cdp::{bridge_channel, BridgeCommand, BridgeResponse};
use std::time::Duration;

const TID: &str = "test-target";

// ---- BridgeCommand construction: all 25 variants ----

#[test]
fn test_bridge_navigate() {
    let cmd = BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "https://example.com".into(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_evaluate_js() {
    let cmd = BridgeCommand::EvaluateJs {
        target_id: TID.into(),
        expression: "1+1".into(),
        return_by_value: true,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_take_screenshot_png() {
    let cmd = BridgeCommand::TakeScreenshot {
        target_id: TID.into(),
        format: "png".into(),
        quality: None,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_take_screenshot_jpeg_with_quality() {
    let cmd = BridgeCommand::TakeScreenshot {
        target_id: TID.into(),
        format: "jpeg".into(),
        quality: Some(80),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_get_title() {
    let _ = format!(
        "{:?}",
        BridgeCommand::GetTitle {
            target_id: TID.into()
        }
    );
}

#[test]
fn test_bridge_get_url() {
    let _ = format!(
        "{:?}",
        BridgeCommand::GetUrl {
            target_id: TID.into()
        }
    );
}

#[test]
fn test_bridge_get_document() {
    let _ = format!(
        "{:?}",
        BridgeCommand::GetDocument {
            target_id: TID.into()
        }
    );
}

#[test]
fn test_bridge_query_selector() {
    let cmd = BridgeCommand::QuerySelector {
        target_id: TID.into(),
        selector: "div.test".into(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_query_selector_all() {
    let cmd = BridgeCommand::QuerySelectorAll {
        target_id: TID.into(),
        selector: "a[href]".into(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_get_outer_html_with_node() {
    let cmd = BridgeCommand::GetOuterHtml {
        target_id: TID.into(),
        node_id: Some(42),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_get_outer_html_no_node() {
    let cmd = BridgeCommand::GetOuterHtml {
        target_id: TID.into(),
        node_id: None,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_set_attribute_value() {
    let cmd = BridgeCommand::SetAttributeValue {
        target_id: TID.into(),
        node_id: 1,
        name: "class".into(),
        value: "active".into(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_dispatch_mouse_event() {
    let cmd = BridgeCommand::DispatchMouseEvent {
        target_id: TID.into(),
        event_type: "mousePressed".into(),
        x: 100.0,
        y: 200.0,
        button: Some(0),
        click_count: Some(1),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_dispatch_mouse_move() {
    let cmd = BridgeCommand::DispatchMouseEvent {
        target_id: TID.into(),
        event_type: "mouseMoved".into(),
        x: 150.0,
        y: 250.0,
        button: None,
        click_count: None,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_dispatch_key_event() {
    let cmd = BridgeCommand::DispatchKeyEvent {
        target_id: TID.into(),
        event_type: "keyDown".into(),
        key: "Enter".into(),
        code: "Enter".into(),
        text: None,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_dispatch_key_event_with_text() {
    let cmd = BridgeCommand::DispatchKeyEvent {
        target_id: TID.into(),
        event_type: "keyDown".into(),
        key: "a".into(),
        code: "KeyA".into(),
        text: Some("a".into()),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_insert_text() {
    let cmd = BridgeCommand::InsertText {
        target_id: TID.into(),
        text: "hello world".into(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_set_viewport() {
    let cmd = BridgeCommand::SetViewport {
        target_id: TID.into(),
        width: 1920,
        height: 1080,
        device_scale_factor: Some(2.0),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_set_viewport_no_dsf() {
    let cmd = BridgeCommand::SetViewport {
        target_id: TID.into(),
        width: 800,
        height: 600,
        device_scale_factor: None,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_set_user_agent() {
    let cmd = BridgeCommand::SetUserAgent {
        target_id: TID.into(),
        user_agent: "Mozilla/5.0".into(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_get_cookies() {
    let cmd = BridgeCommand::GetCookies {
        target_id: TID.into(),
        urls: vec!["https://a.com".into(), "https://b.com".into()],
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_get_all_cookies() {
    let _ = format!(
        "{:?}",
        BridgeCommand::GetAllCookies {
            target_id: TID.into()
        }
    );
}

#[test]
fn test_bridge_delete_cookie() {
    let cmd = BridgeCommand::DeleteCookie {
        target_id: TID.into(),
        name: "session".into(),
        url: Some("https://x.com".into()),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_set_cookie() {
    let cmd = BridgeCommand::SetCookie {
        target_id: TID.into(),
        name: "token".into(),
        value: "abc123".into(),
        url: Some("https://example.com".into()),
        domain: None,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_get_response_body() {
    let cmd = BridgeCommand::GetResponseBody {
        target_id: TID.into(),
        request_id: "req-001".into(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_add_script() {
    let cmd = BridgeCommand::AddScriptToEvaluateOnNewDocument {
        target_id: TID.into(),
        source: "console.log('hi')".into(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_reload() {
    let cmd = BridgeCommand::Reload {
        target_id: TID.into(),
        ignore_cache: true,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_go_back() {
    let _ = format!(
        "{:?}",
        BridgeCommand::GoBack {
            target_id: TID.into()
        }
    );
}

#[test]
fn test_bridge_go_forward() {
    let _ = format!(
        "{:?}",
        BridgeCommand::GoForward {
            target_id: TID.into()
        }
    );
}

#[test]
fn test_bridge_stop_loading() {
    let _ = format!(
        "{:?}",
        BridgeCommand::StopLoading {
            target_id: TID.into()
        }
    );
}

#[test]
fn test_bridge_close_page() {
    let _ = format!(
        "{:?}",
        BridgeCommand::ClosePage {
            target_id: TID.into()
        }
    );
}

// ---- BridgeCommand debug output verification ----

#[test]
fn test_bridge_debug_contains_variant_name() {
    let debug = format!(
        "{:?}",
        BridgeCommand::GetTitle {
            target_id: TID.into()
        }
    );
    assert!(debug.contains("GetTitle"));
}

#[test]
fn test_bridge_debug_navigate_contains_url() {
    let debug = format!(
        "{:?}",
        BridgeCommand::Navigate {
            target_id: TID.into(),
            url: "https://test.com".into()
        }
    );
    assert!(debug.contains("test.com"));
}

#[test]
fn test_bridge_debug_evaluate_contains_expression() {
    let debug = format!(
        "{:?}",
        BridgeCommand::EvaluateJs {
            target_id: TID.into(),
            expression: "42".into(),
            return_by_value: false
        }
    );
    assert!(debug.contains("42"));
}

// ---- BridgeCommand with empty/edge values ----

#[test]
fn test_bridge_navigate_empty_url() {
    let cmd = BridgeCommand::Navigate {
        target_id: TID.into(),
        url: String::new(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_evaluate_js_empty() {
    let cmd = BridgeCommand::EvaluateJs {
        target_id: TID.into(),
        expression: String::new(),
        return_by_value: false,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_insert_text_empty() {
    let cmd = BridgeCommand::InsertText {
        target_id: TID.into(),
        text: String::new(),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_insert_text_unicode() {
    let cmd = BridgeCommand::InsertText {
        target_id: TID.into(),
        text: "日本語テスト".into(),
    };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("日本語"));
}

#[test]
fn test_bridge_query_selector_complex() {
    let cmd = BridgeCommand::QuerySelector {
        target_id: TID.into(),
        selector: "div.class > ul li:nth-child(2) a[href^='https://']".into(),
    };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("nth-child"));
}

#[test]
fn test_bridge_set_viewport_zero_dims() {
    let cmd = BridgeCommand::SetViewport {
        target_id: TID.into(),
        width: 0,
        height: 0,
        device_scale_factor: Some(0.0),
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_dispatch_mouse_negative_coords() {
    let cmd = BridgeCommand::DispatchMouseEvent {
        target_id: TID.into(),
        event_type: "mouseMoved".into(),
        x: -10.0,
        y: -20.0,
        button: None,
        click_count: None,
    };
    let _ = format!("{:?}", cmd);
}

#[test]
fn test_bridge_get_cookies_empty_urls() {
    let cmd = BridgeCommand::GetCookies {
        target_id: TID.into(),
        urls: vec![],
    };
    let _ = format!("{:?}", cmd);
}

// ---- BridgeResponse edge cases ----

#[test]
fn test_bridge_response_ok() {
    let resp = BridgeResponse {
        result: Ok(serde_json::json!({"ok": true})),
    };
    assert!(resp.result.is_ok());
    let val = resp.result.unwrap();
    assert_eq!(val["ok"], true);
}

#[test]
fn test_bridge_response_err() {
    let resp = BridgeResponse {
        result: Err("something failed".into()),
    };
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "something failed");
}

#[test]
fn test_bridge_response_debug_ok() {
    let resp = BridgeResponse {
        result: Ok(serde_json::json!(42)),
    };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("42") || debug.contains("Ok"));
}

#[test]
fn test_bridge_response_debug_err() {
    let resp = BridgeResponse {
        result: Err("error msg".into()),
    };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("error msg") || debug.contains("Err"));
}

#[test]
fn test_bridge_response_ok_null() {
    let resp = BridgeResponse {
        result: Ok(serde_json::Value::Null),
    };
    assert!(resp.result.is_ok());
    assert!(resp.result.unwrap().is_null());
}

#[test]
fn test_bridge_response_err_empty() {
    let resp = BridgeResponse {
        result: Err(String::new()),
    };
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().is_empty());
}

// ---- bridge_channel: send timeout behavior ----

#[test]
fn test_bridge_channel_send_timeout() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(100));
    let resp = tx.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    assert!(resp.result.is_err());
}

#[test]
fn test_bridge_channel_closed_receiver() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    let resp = tx.send(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    assert!(resp.result.is_err());
}

#[test]
fn test_bridge_sender_is_alive() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    assert!(tx.is_alive());
    drop(rx);
    assert!(!tx.is_alive());
}

#[test]
fn test_bridge_fire_and_forget() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::StopLoading {
        target_id: TID.into(),
    });
    drop(rx);
    tx.send_fire_and_forget(BridgeCommand::ClosePage {
        target_id: TID.into(),
    });
}
