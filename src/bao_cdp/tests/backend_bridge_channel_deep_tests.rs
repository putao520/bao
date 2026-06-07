// @trace TEST-CDP-025 [req:REQ-CDP-001,REQ-CDP-003,REQ-CDP-004] [level:unit]
// BridgeChannel send/recv/drain lifecycle, BridgeCommand variants debug,
// BridgeResponse result handling, InternalBackend indirect test via handle_command.

use std::time::Duration;

use bao_cdp::servo_bridge::{BridgeCommand, BridgeResponse, bridge_channel};
use bao_cdp::{handle_command, CDPMessage, CDPResponse};
use serde_json::json;

// ---- InternalBackend indirect tests (via handle_command) ----

fn dispatch(method: &str, params: Option<serde_json::Value>) -> CDPResponse {
    let msg = CDPMessage { id: 1, method: method.to_string(), params: None, session_id: None };
    handle_command(msg, "test-target", &params, None)
}

#[test]
fn test_internal_page_navigate() {
    let resp = dispatch("Page.navigate", Some(json!({"url":"http://test"})));
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["frameId"], "0");
}

#[test]
fn test_internal_runtime_evaluate() {
    let resp = dispatch("Runtime.evaluate", Some(json!({"expression":"1+1"})));
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_page_enable() {
    let resp = dispatch("Page.enable", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_page_disable() {
    let resp = dispatch("Page.disable", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_unknown_domain() {
    let resp = dispatch("UnknownDomain.doSomething", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn test_internal_unknown_command() {
    let resp = dispatch("Page.nonexistentCommand", None);
    assert!(resp.error.is_some());
}

#[test]
fn test_internal_empty_method() {
    let resp = dispatch("", None);
    assert!(resp.error.is_some());
}

#[test]
fn test_internal_dom_get_document() {
    let resp = dispatch("DOM.getDocument", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_network_enable() {
    let resp = dispatch("Network.enable", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_css_enable() {
    let resp = dispatch("CSS.enable", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_emulation_set_metrics() {
    let resp = dispatch("Emulation.setDeviceMetricsOverride",
        Some(json!({"width":1920,"height":1080})));
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_debugger_enable() {
    let resp = dispatch("Debugger.enable", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_log_enable() {
    let resp = dispatch("Log.enable", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_overlay_enable() {
    let resp = dispatch("Overlay.enable", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_input_dispatch_mouse() {
    let resp = dispatch("Input.dispatchMouseEvent",
        Some(json!({"type":"mousePressed","x":100,"y":200})));
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_fetch_enable() {
    let resp = dispatch("Fetch.enable", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_internal_target_set_auto_attach() {
    let resp = dispatch("Target.setAutoAttach", Some(json!({"flatten":true})));
    // Target domain may not handle this without bridge, verify no crash
    assert!(resp.result.is_some() || resp.error.is_some());
}

#[test]
fn test_internal_empty_params_object() {
    let resp = dispatch("Page.enable", Some(json!({})));
    assert!(resp.result.is_some());
}

// ---- BridgeCommand debug format ----

#[test]
fn test_bridge_cmd_navigate_debug() {
    let cmd = BridgeCommand::Navigate { url: "http://test".into() };
    assert!(format!("{:?}", cmd).contains("Navigate"));
}

#[test]
fn test_bridge_cmd_evaluate_debug() {
    let cmd = BridgeCommand::EvaluateJs { expression: "1+1".into(), return_by_value: true };
    assert!(format!("{:?}", cmd).contains("EvaluateJs"));
}

#[test]
fn test_bridge_cmd_screenshot_debug() {
    let cmd = BridgeCommand::TakeScreenshot { format: "png".into(), quality: Some(90) };
    assert!(format!("{:?}", cmd).contains("TakeScreenshot"));
}

#[test]
fn test_bridge_cmd_get_title_debug() {
    assert!(format!("{:?}", BridgeCommand::GetTitle).contains("GetTitle"));
}

#[test]
fn test_bridge_cmd_get_url_debug() {
    assert!(format!("{:?}", BridgeCommand::GetUrl).contains("GetUrl"));
}

#[test]
fn test_bridge_cmd_get_document_debug() {
    assert!(format!("{:?}", BridgeCommand::GetDocument).contains("GetDocument"));
}

#[test]
fn test_bridge_cmd_query_selector_debug() {
    let cmd = BridgeCommand::QuerySelector { selector: "div".into() };
    assert!(format!("{:?}", cmd).contains("QuerySelector"));
}

#[test]
fn test_bridge_cmd_query_selector_all_debug() {
    let cmd = BridgeCommand::QuerySelectorAll { selector: "div.cls".into() };
    assert!(format!("{:?}", cmd).contains("QuerySelectorAll"));
}

#[test]
fn test_bridge_cmd_mouse_event_debug() {
    let cmd = BridgeCommand::DispatchMouseEvent {
        event_type: "mousePressed".into(), x: 100.0, y: 200.0,
        button: Some(0), click_count: Some(1),
    };
    assert!(format!("{:?}", cmd).contains("DispatchMouseEvent"));
}

#[test]
fn test_bridge_cmd_key_event_debug() {
    let cmd = BridgeCommand::DispatchKeyEvent {
        event_type: "keyDown".into(), key: "a".into(),
        code: "KeyA".into(), text: Some("a".into()),
    };
    assert!(format!("{:?}", cmd).contains("DispatchKeyEvent"));
}

#[test]
fn test_bridge_cmd_insert_text_debug() {
    let cmd = BridgeCommand::InsertText { text: "hello".into() };
    assert!(format!("{:?}", cmd).contains("InsertText"));
}

#[test]
fn test_bridge_cmd_set_viewport_debug() {
    let cmd = BridgeCommand::SetViewport { width: 1920, height: 1080, device_scale_factor: Some(2.0) };
    assert!(format!("{:?}", cmd).contains("SetViewport"));
}

#[test]
fn test_bridge_cmd_set_user_agent_debug() {
    let cmd = BridgeCommand::SetUserAgent { user_agent: "TestBot/1.0".into() };
    assert!(format!("{:?}", cmd).contains("SetUserAgent"));
}

#[test]
fn test_bridge_cmd_get_cookies_debug() {
    let cmd = BridgeCommand::GetCookies { urls: vec!["http://a.com".into()] };
    assert!(format!("{:?}", cmd).contains("GetCookies"));
}

#[test]
fn test_bridge_cmd_get_all_cookies_debug() {
    assert!(format!("{:?}", BridgeCommand::GetAllCookies).contains("GetAllCookies"));
}

#[test]
fn test_bridge_cmd_set_cookie_debug() {
    let cmd = BridgeCommand::SetCookie {
        name: "session".into(), value: "abc".into(),
        url: Some("http://test".into()), domain: None,
    };
    assert!(format!("{:?}", cmd).contains("SetCookie"));
}

#[test]
fn test_bridge_cmd_delete_cookie_debug() {
    let cmd = BridgeCommand::DeleteCookie { name: "session".into(), url: None };
    assert!(format!("{:?}", cmd).contains("DeleteCookie"));
}

#[test]
fn test_bridge_cmd_get_response_body_debug() {
    let cmd = BridgeCommand::GetResponseBody { request_id: "req-1".into() };
    assert!(format!("{:?}", cmd).contains("GetResponseBody"));
}

#[test]
fn test_bridge_cmd_add_script_debug() {
    let cmd = BridgeCommand::AddScriptToEvaluateOnNewDocument { source: "console.log(1)".into() };
    assert!(format!("{:?}", cmd).contains("AddScript"));
}

#[test]
fn test_bridge_cmd_reload_debug() {
    let cmd = BridgeCommand::Reload { ignore_cache: true };
    assert!(format!("{:?}", cmd).contains("Reload"));
}

#[test]
fn test_bridge_cmd_go_back_debug() {
    assert!(format!("{:?}", BridgeCommand::GoBack).contains("GoBack"));
}

#[test]
fn test_bridge_cmd_go_forward_debug() {
    assert!(format!("{:?}", BridgeCommand::GoForward).contains("GoForward"));
}

#[test]
fn test_bridge_cmd_stop_loading_debug() {
    assert!(format!("{:?}", BridgeCommand::StopLoading).contains("StopLoading"));
}

#[test]
fn test_bridge_cmd_close_page_debug() {
    assert!(format!("{:?}", BridgeCommand::ClosePage).contains("ClosePage"));
}

#[test]
fn test_bridge_cmd_get_outer_html_debug() {
    let cmd = BridgeCommand::GetOuterHtml { node_id: Some(1) };
    assert!(format!("{:?}", cmd).contains("GetOuterHtml"));
}

#[test]
fn test_bridge_cmd_set_attribute_debug() {
    let cmd = BridgeCommand::SetAttributeValue { node_id: 5, name: "class".into(), value: "active".into() };
    assert!(format!("{:?}", cmd).contains("SetAttributeValue"));
}

// ---- BridgeResponse ----

#[test]
fn test_bridge_response_ok() {
    let resp = BridgeResponse { result: Ok(json!({"ok": true})) };
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["ok"], true);
}

#[test]
fn test_bridge_response_err() {
    let resp = BridgeResponse { result: Err("error msg".into()) };
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "error msg");
}

#[test]
fn test_bridge_response_debug() {
    let resp = BridgeResponse { result: Ok(json!(42)) };
    assert!(format!("{:?}", resp).contains("42"));
}

// ---- BridgeChannel send/recv/drain ----

#[test]
fn test_channel_send_recv() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        receiver.try_process(|cmd| match cmd {
            BridgeCommand::Navigate { url } => BridgeResponse {
                result: Ok(json!({"navigated": url})),
            },
            _ => BridgeResponse { result: Err("unexpected".into()) },
        });
    });

    let resp = sender.send(BridgeCommand::Navigate { url: "http://test".into() });
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["navigated"], "http://test");
}

#[test]
fn test_channel_closed_sender() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(1));
    drop(receiver);
    let resp = sender.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("closed"));
}

#[test]
fn test_channel_fire_and_forget() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    sender.send_fire_and_forget(BridgeCommand::GetTitle);
    let processed = receiver.try_process(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert!(processed);
}

#[test]
fn test_channel_drain_multiple() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    sender.send_fire_and_forget(BridgeCommand::GetTitle);
    sender.send_fire_and_forget(BridgeCommand::GetUrl);
    sender.send_fire_and_forget(BridgeCommand::GetDocument);
    let count = receiver.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 3);
}

#[test]
fn test_channel_drain_empty() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let _ = sender;
    let count = receiver.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 0);
}

#[test]
fn test_channel_timeout() {
    let (sender, _receiver) = bridge_channel(Duration::from_millis(10));
    let resp = sender.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("timeout"));
}

#[test]
fn test_channel_sender_is_alive() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    assert!(sender.is_alive());
    drop(receiver);
    assert!(!sender.is_alive());
}

#[test]
fn test_channel_sender_clone_shared() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let cloned = sender.clone();

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        receiver.drain(|_cmd| {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            BridgeResponse { result: Ok(json!({})) }
        });
    });

    sender.send_fire_and_forget(BridgeCommand::GetTitle);
    cloned.send_fire_and_forget(BridgeCommand::GetUrl);

    let _ = handle.join();
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn test_channel_try_process_no_pending() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let _ = sender;
    let processed = receiver.try_process(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert!(!processed);
}

#[test]
fn test_channel_send_with_response_match() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));

    std::thread::spawn(move || {
        loop {
            let got = receiver.try_process(|cmd| match cmd {
                BridgeCommand::GetTitle => BridgeResponse {
                    result: Ok(json!("Test Title")),
                },
                _ => BridgeResponse { result: Ok(json!({})) },
            });
            if !got {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });

    let resp = sender.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap(), json!("Test Title"));
}
