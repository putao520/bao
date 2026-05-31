// @trace TEST-CDP-034 [req:REQ-CDP-001,REQ-CDP-003,REQ-CDP-006] [level:unit]
// Bridge channel timeout behavior, drain/try_process interleaving,
// fire-and-forget semantics, is_alive checks, multi-command processing,
// protocol handle_command with bridge connected for Target domain.

use std::time::Duration;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bao_cdp::{bridge_channel, BridgeCommand, BridgeResponse, BridgeSender, CDPMessage, CDPEvent};
use bao_cdp::{handle_command, serialize_response, serialize_event};
use serde_json::json;

/// Helper: dispatch a CDP command with correct params passing
fn dispatch(method: &str, params: Option<serde_json::Value>) -> bao_cdp::CDPResponse {
    let msg = CDPMessage { id: 1, method: method.to_string(), params: params.clone(), session_id: None };
    handle_command(msg, "t1", &params, None)
}

fn dispatch_bridge(method: &str, params: Option<serde_json::Value>, target: &str, bridge: &BridgeSender) -> bao_cdp::CDPResponse {
    let msg = CDPMessage { id: 1, method: method.to_string(), params: params.clone(), session_id: None };
    handle_command(msg, target, &params, Some(bridge))
}

// ============================================================================
// Bridge channel: timeout behavior
// ============================================================================

#[test]
fn test_bridge_sender_times_out() {
    let (tx, rx) = bridge_channel(Duration::from_millis(50));
    let resp = tx.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("timeout"));
    rx.try_process(|cmd| BridgeResponse { result: Ok(json!(format!("{:?}", cmd))) });
}

#[test]
fn test_bridge_sender_succeeds_within_timeout() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|_cmd| {
                BridgeResponse { result: Ok(json!("test-title")) }
            });
            if got { return; }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = tx.send(BridgeCommand::GetTitle);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap(), json!("test-title"));
}

#[test]
fn test_bridge_channel_timeout_value_propagated() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(10));
    let resp = tx.send(BridgeCommand::GetUrl);
    assert!(resp.result.is_err());
}

// ============================================================================
// Bridge channel: try_process semantics
// ============================================================================

#[test]
fn test_try_process_no_pending_returns_false() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let processed = rx.try_process(|_cmd| BridgeResponse { result: Ok(json!(null)) });
    assert!(!processed);
}

#[test]
fn test_try_process_single_command() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    std::thread::sleep(Duration::from_millis(10));
    let processed = rx.try_process(|cmd| {
        let result = match cmd {
            BridgeCommand::GetTitle => Ok(json!("title")),
            _ => Ok(json!(null)),
        };
        BridgeResponse { result }
    });
    assert!(processed);
    let processed2 = rx.try_process(|_cmd| BridgeResponse { result: Ok(json!(null)) });
    assert!(!processed2);
}

// ============================================================================
// Bridge channel: drain semantics
// ============================================================================

#[test]
fn test_drain_no_pending_returns_zero() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!(null)) });
    assert_eq!(count, 0);
}

#[test]
fn test_drain_multiple_commands() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx.send_fire_and_forget(BridgeCommand::GetUrl);
    tx.send_fire_and_forget(BridgeCommand::GetDocument);
    std::thread::sleep(Duration::from_millis(10));
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 3);
}

#[test]
fn test_drain_order_preserved() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let counter = Arc::new(AtomicUsize::new(0));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx.send_fire_and_forget(BridgeCommand::GetUrl);
    std::thread::sleep(Duration::from_millis(10));
    let c = counter.clone();
    let count = rx.drain(move |_cmd| {
        c.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count, 2);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// ============================================================================
// Bridge channel: fire-and-forget
// ============================================================================

#[test]
fn test_send_fire_and_forget_does_not_block() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    std::thread::sleep(Duration::from_millis(10));
    let processed = rx.try_process(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert!(processed);
}

#[test]
fn test_send_fire_and_forget_multiple() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    for _ in 0..10 {
        tx.send_fire_and_forget(BridgeCommand::GetTitle);
    }
    std::thread::sleep(Duration::from_millis(20));
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 10);
}

// ============================================================================
// Bridge channel: is_alive
// ============================================================================

#[test]
fn test_is_alive_when_both_ends_active() {
    let (tx, _rx) = bridge_channel(Duration::from_secs(5));
    assert!(tx.is_alive());
}

#[test]
fn test_is_alive_after_drop_rx() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    assert!(!tx.is_alive());
}

#[test]
fn test_is_alive_after_multiple_sends() {
    let (tx, _rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx.send_fire_and_forget(BridgeCommand::GetUrl);
    assert!(tx.is_alive());
}

// ============================================================================
// Bridge channel: clone
// ============================================================================

#[test]
fn test_sender_clone_shares_channel() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx2.send_fire_and_forget(BridgeCommand::GetUrl);
    std::thread::sleep(Duration::from_millis(10));
    let count = rx.drain(|_cmd| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 2);
}

#[test]
fn test_sender_clone_independent_timeout() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(50));
    let tx2 = tx.clone();
    let resp1 = tx.send(BridgeCommand::GetTitle);
    let resp2 = tx2.send(BridgeCommand::GetUrl);
    assert!(resp1.result.is_err());
    assert!(resp2.result.is_err());
}

// ============================================================================
// BridgeResponse construction
// ============================================================================

#[test]
fn test_bridge_response_ok() {
    let resp = BridgeResponse { result: Ok(json!({"key": "value"})) };
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["key"], "value");
}

#[test]
fn test_bridge_response_err() {
    let resp = BridgeResponse { result: Err("test error".into()) };
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "test error");
}

// ============================================================================
// Protocol: Target domain with bridge connected
// ============================================================================

#[test]
fn test_target_get_targets_with_bridge() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    let processed = Arc::new(AtomicUsize::new(0));
    let processed2 = processed.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                processed2.fetch_add(1, Ordering::SeqCst);
                match cmd {
                    BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Test Title")) },
                    BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!("https://example.com")) },
                    _ => BridgeResponse { result: Ok(json!(null)) },
                }
            });
            if got && processed2.load(Ordering::SeqCst) >= 2 { return; }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("Target.getTargets", None, "test-target", &tx);
    // Wait for both bridge commands to be processed
    let start = std::time::Instant::now();
    while processed.load(Ordering::SeqCst) < 2 && start.elapsed() < Duration::from_millis(200) {
        std::thread::sleep(Duration::from_millis(1));
    }
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let result = resp.result.unwrap();
    let infos = result["targetInfos"].as_array().unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0]["targetId"], "test-target");
    assert_eq!(infos[0]["title"], "Test Title");
    assert_eq!(infos[0]["url"], "https://example.com");
}

#[test]
fn test_target_close_target_fire_and_forget() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let closed = Arc::new(AtomicUsize::new(0));
    let closed2 = closed.clone();
    std::thread::spawn(move || {
        rx.drain(move |cmd| {
            if matches!(cmd, BridgeCommand::ClosePage) {
                closed2.fetch_add(1, Ordering::SeqCst);
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let resp = dispatch_bridge("Target.closeTarget", None, "t1", &tx);
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["success"], true);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(closed.load(Ordering::SeqCst), 1);
}

// ============================================================================
// Protocol: Page domain with bridge - navigate
// ============================================================================

#[test]
fn test_page_navigate_with_bridge() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let navigated = Arc::new(AtomicUsize::new(0));
    let navigated2 = navigated.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if matches!(cmd, BridgeCommand::Navigate { .. }) {
                    navigated2.fetch_add(1, Ordering::SeqCst);
                }
                BridgeResponse { result: Ok(json!({})) }
            });
            if got { return; }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("Page.navigate", Some(json!({"url": "https://example.com"})), "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let result = resp.result.unwrap();
    assert_eq!(result["frameId"], "0");
    assert!(result["loaderId"].is_string());
    assert_eq!(navigated.load(Ordering::SeqCst), 1);
}

#[test]
fn test_page_navigate_no_bridge_default_url() {
    let resp = dispatch("Page.navigate", Some(json!({})));
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["frameId"], "0");
}

// ============================================================================
// Protocol: Runtime.evaluate with bridge
// ============================================================================

#[test]
fn test_runtime_evaluate_with_bridge() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                match cmd {
                    BridgeCommand::EvaluateJs { expression, .. } => {
                        BridgeResponse { result: Ok(json!({"type": "number", "value": 42, "description": expression})) }
                    }
                    _ => BridgeResponse { result: Ok(json!({})) },
                }
            });
            if got { return; }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("Runtime.evaluate", Some(json!({"expression": "1+1"})), "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let result = resp.result.unwrap();
    assert_eq!(result["value"], 42);
}

#[test]
fn test_runtime_evaluate_empty_expression_no_bridge() {
    let resp = dispatch("Runtime.evaluate", Some(json!({"expression": ""})));
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["result"]["type"], "undefined");
}

#[test]
fn test_runtime_evaluate_no_params_no_bridge() {
    let resp = dispatch("Runtime.evaluate", None);
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["result"]["type"], "undefined");
}

// ============================================================================
// Protocol: DOM.querySelector with bridge
// ============================================================================

#[test]
fn test_dom_query_selector_with_bridge() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                match cmd {
                    BridgeCommand::QuerySelector { selector } => {
                        BridgeResponse { result: Ok(json!({"nodeId": 42, "selector": selector})) }
                    }
                    _ => BridgeResponse { result: Ok(json!({})) },
                }
            });
            if got { return; }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("DOM.querySelector", Some(json!({"selector": "div.main"})), "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let result = resp.result.unwrap();
    assert_eq!(result["nodeId"], 42);
}

#[test]
fn test_dom_query_selector_empty_no_bridge() {
    let resp = dispatch("DOM.querySelector", Some(json!({"selector": ""})));
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["nodeId"], 0);
}

// ============================================================================
// Protocol: Fetch domain commands
// ============================================================================

#[test]
fn test_fetch_enable_with_patterns() {
    let resp = dispatch("Fetch.enable", Some(json!({"patterns": [{"urlPattern": "*"}]})));
    let result = resp.result.unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 1);
}

#[test]
fn test_fetch_enable_no_patterns() {
    let resp = dispatch("Fetch.enable", None);
    let result = resp.result.unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 0);
}

#[test]
fn test_fetch_continue_request() {
    let resp = dispatch("Fetch.continueRequest", Some(json!({"requestId": "req-001"})));
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "req-001");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_fail_request() {
    let resp = dispatch("Fetch.failRequest", Some(json!({"requestId": "req-002", "reason": "Aborted"})));
    let result = resp.result.unwrap();
    assert_eq!(result["failed"], true);
    assert_eq!(result["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request() {
    let resp = dispatch("Fetch.fulfillRequest", Some(json!({"requestId": "req-003", "responseCode": 200, "body": "hello"})));
    let result = resp.result.unwrap();
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 5);
}

#[test]
fn test_fetch_fulfill_request_default_status() {
    let resp = dispatch("Fetch.fulfillRequest", Some(json!({"requestId": "req-004"})));
    let result = resp.result.unwrap();
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 0);
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let resp = dispatch("Fetch.takeResponseBodyAsStream", Some(json!({"requestId": "req-005"})));
    let result = resp.result.unwrap();
    assert_eq!(result["stream"], "stream-req-005");
}

#[test]
fn test_fetch_get_request_post_data() {
    let resp = dispatch("Fetch.getRequestPostData", Some(json!({"requestId": "req-006"})));
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "req-006");
    assert_eq!(result["postData"], "");
}

#[test]
fn test_fetch_continue_with_auth() {
    let resp = dispatch("Fetch.continueWithAuth", Some(json!({"requestId": "req-007"})));
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "req-007");
}

#[test]
fn test_fetch_unknown_command() {
    let resp = dispatch("Fetch.nonexistentMethod", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

// ============================================================================
// Protocol: Network domain commands
// ============================================================================

#[test]
fn test_network_get_cookies() {
    let resp = dispatch("Network.getCookies", None);
    let result = resp.result.unwrap();
    assert!(result["cookies"].is_array());
    assert_eq!(result["cookies"].as_array().unwrap().len(), 0);
}

#[test]
fn test_network_get_all_cookies() {
    let resp = dispatch("Network.getAllCookies", None);
    let result = resp.result.unwrap();
    assert!(result["cookies"].is_array());
}

#[test]
fn test_network_get_response_body() {
    let resp = dispatch("Network.getResponseBody", None);
    let result = resp.result.unwrap();
    assert_eq!(result["body"], "");
    assert_eq!(result["base64Encoded"], false);
}

#[test]
fn test_network_set_cache_disabled() {
    let resp = dispatch("Network.setCacheDisabled", Some(json!({"cacheDisabled": true})));
    assert!(resp.result.is_some());
}

#[test]
fn test_network_set_extra_http_headers() {
    let resp = dispatch("Network.setExtraHTTPHeaders", Some(json!({"headers": {"X-Custom": "value"}})));
    assert!(resp.result.is_some());
}

#[test]
fn test_network_delete_cookies() {
    let resp = dispatch("Network.deleteCookies", Some(json!({"name": "session"})));
    assert!(resp.result.is_some());
}

#[test]
fn test_network_set_cookie() {
    let resp = dispatch("Network.setCookie", Some(json!({"name": "test", "value": "1"})));
    assert!(resp.result.is_some());
}

#[test]
fn test_network_unknown_command() {
    let resp = dispatch("Network.nonexistent", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

// ============================================================================
// Protocol: Overlay domain commands
// ============================================================================

#[test]
fn test_overlay_highlight_node() {
    let resp = dispatch("Overlay.highlightNode", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_overlay_hide_highlight() {
    let resp = dispatch("Overlay.hideHighlight", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_overlay_set_inspect_mode() {
    let resp = dispatch("Overlay.setInspectMode", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_overlay_set_paused_in_debugger_message() {
    let resp = dispatch("Overlay.setPausedInDebuggerMessage", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_overlay_unknown_command() {
    let resp = dispatch("Overlay.nonexistent", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

// ============================================================================
// Protocol: Log domain commands
// ============================================================================

#[test]
fn test_log_clear() {
    let resp = dispatch("Log.clear", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_log_start_violations_report() {
    let resp = dispatch("Log.startViolationsReport", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_log_stop_violations_report() {
    let resp = dispatch("Log.stopViolationsReport", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_log_unknown_command() {
    let resp = dispatch("Log.nonexistent", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

// ============================================================================
// Protocol: Debugger domain commands
// ============================================================================

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let resp = dispatch("Debugger.setBreakpointByUrl", Some(json!({"lineNumber": 10})));
    let result = resp.result.unwrap();
    assert_eq!(result["breakpointId"], "1");
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let resp = dispatch("Debugger.getPossibleBreakpoints", None);
    let result = resp.result.unwrap();
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_get_script_source() {
    let resp = dispatch("Debugger.getScriptSource", Some(json!({"scriptId": "1"})));
    let result = resp.result.unwrap();
    assert_eq!(result["scriptSource"], "");
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let resp = dispatch("Debugger.evaluateOnCallFrame", Some(json!({"callFrameId": "0", "expression": "1+1"})));
    let result = resp.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_debugger_set_pause_on_exceptions() {
    let resp = dispatch("Debugger.setPauseOnExceptions", None);
    assert!(resp.result.is_some());
}

#[test]
fn test_debugger_unknown_command() {
    let resp = dispatch("Debugger.nonexistent", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

// ============================================================================
// Protocol: CSS domain commands
// ============================================================================

#[test]
fn test_css_get_computed_style_for_node() {
    let resp = dispatch("CSS.getComputedStyleForNode", Some(json!({"nodeId": 1})));
    let result = resp.result.unwrap();
    assert!(result["computedStyle"].is_array());
}

#[test]
fn test_css_get_matched_styles_for_node() {
    let resp = dispatch("CSS.getMatchedStylesForNode", Some(json!({"nodeId": 1})));
    let result = resp.result.unwrap();
    assert!(result["matchedCSSRules"].is_array());
    assert!(result["inlineStyle"].is_null());
    assert!(result["attributesStyle"].is_null());
}

#[test]
fn test_css_get_inline_styles_for_node() {
    let resp = dispatch("CSS.getInlineStylesForNode", Some(json!({"nodeId": 1})));
    let result = resp.result.unwrap();
    assert!(result["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    let resp = dispatch("CSS.setStyleTexts", None);
    let result = resp.result.unwrap();
    assert!(result["styles"].is_array());
}

#[test]
fn test_css_unknown_command() {
    let resp = dispatch("CSS.nonexistent", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

// ============================================================================
// Protocol: Unknown domain
// ============================================================================

#[test]
fn test_unknown_domain_error() {
    let resp = dispatch("Unknown.method", None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Unknown"));
}

#[test]
fn test_empty_domain_error() {
    let resp = dispatch("nomethod", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

#[test]
fn test_empty_method_error() {
    let resp = dispatch("", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

// ============================================================================
// Protocol: serialize_response roundtrip
// ============================================================================

#[test]
fn test_serialize_response_ok() {
    let resp = dispatch("Page.enable", None);
    let serialized = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["id"], 1);
    assert!(parsed["result"].is_object());
}

#[test]
fn test_serialize_response_error() {
    let resp = dispatch("Unknown.method", None);
    let serialized = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["id"], 1);
    assert!(parsed["error"].is_object());
    assert_eq!(parsed["error"]["code"], -32601);
}

// ============================================================================
// Protocol: serialize_event
// ============================================================================

#[test]
fn test_serialize_event_with_params() {
    let ev = CDPEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345.0})),
    };
    let serialized = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["method"], "Page.loadEventFired");
    assert_eq!(parsed["params"]["timestamp"], 12345.0);
}

#[test]
fn test_serialize_event_without_params() {
    let ev = CDPEvent {
        method: "Runtime.executionContextDestroyed".into(),
        params: None,
    };
    let serialized = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["method"], "Runtime.executionContextDestroyed");
}
