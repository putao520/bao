// @trace TEST-CDP-034 [req:REQ-CDP-001,REQ-CDP-003,REQ-CDP-006] [level:unit]
// Bridge channel timeout behavior, drain/try_process interleaving,
// fire-and-forget semantics, is_alive checks, multi-command processing,
// protocol handle_command with bridge connected for Target domain.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bao_cdp::{bridge_channel, BridgeCommand, BridgeResponse, BridgeSender, CdpEvent, CdpMessage};
use bao_cdp::{handle_command, serialize_event, serialize_response};
use serde_json::json;

const TID: &str = "test-target";

/// Helper: dispatch a CDP command with correct params passing
fn dispatch(method: &str, params: Option<serde_json::Value>) -> bao_cdp::CdpResponse {
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: params.clone(),
        session_id: None,
    };
    handle_command(msg, "t1", &params, None)
}

fn dispatch_bridge(
    method: &str,
    params: Option<serde_json::Value>,
    target: &str,
    bridge: &BridgeSender,
) -> bao_cdp::CdpResponse {
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: params.clone(),
        session_id: None,
    };
    handle_command(msg, target, &params, Some(bridge))
}

// ============================================================================
// Bridge channel: timeout behavior
// ============================================================================

#[test]
fn test_bridge_sender_times_out() {
    let (tx, rx) = bridge_channel(Duration::from_millis(50));
    let resp = tx.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("timeout"));
    rx.try_process(|cmd| BridgeResponse {
        result: Ok(json!(format!("{:?}", cmd))),
    });
}

#[test]
fn test_bridge_sender_succeeds_within_timeout() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|_cmd| BridgeResponse {
                result: Ok(json!("test-title")),
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = tx.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap(), json!("test-title"));
}

#[test]
fn test_bridge_channel_timeout_value_propagated() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(10));
    let resp = tx.send(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    assert!(resp.result.is_err());
}

// ============================================================================
// Bridge channel: try_process semantics
// ============================================================================

#[test]
fn test_try_process_no_pending_returns_false() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let processed = rx.try_process(|_cmd| BridgeResponse {
        result: Ok(json!(null)),
    });
    assert!(!processed);
}

#[test]
fn test_try_process_single_command() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    std::thread::sleep(Duration::from_millis(10));
    let processed = rx.try_process(|cmd| {
        let result = match cmd {
            BridgeCommand::GetTitle { .. } => Ok(json!("title")),
            _ => Ok(json!(null)),
        };
        BridgeResponse { result }
    });
    assert!(processed);
    let processed2 = rx.try_process(|_cmd| BridgeResponse {
        result: Ok(json!(null)),
    });
    assert!(!processed2);
}

// ============================================================================
// Bridge channel: drain semantics
// ============================================================================

#[test]
fn test_drain_no_pending_returns_zero() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let count = rx.drain(|_cmd| BridgeResponse {
        result: Ok(json!(null)),
    });
    assert_eq!(count, 0);
}

#[test]
fn test_drain_multiple_commands() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    tx.send_fire_and_forget(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    tx.send_fire_and_forget(BridgeCommand::GetDocument {
        target_id: TID.into(),
    });
    std::thread::sleep(Duration::from_millis(10));
    let count = rx.drain(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(count, 3);
}

#[test]
fn test_drain_order_preserved() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let counter = Arc::new(AtomicUsize::new(0));
    tx.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    tx.send_fire_and_forget(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    std::thread::sleep(Duration::from_millis(10));
    let c = counter.clone();
    let count = rx.drain(move |_cmd| {
        c.fetch_add(1, Ordering::SeqCst);
        BridgeResponse {
            result: Ok(json!({})),
        }
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
    tx.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    std::thread::sleep(Duration::from_millis(10));
    let processed = rx.try_process(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert!(processed);
}

#[test]
fn test_send_fire_and_forget_multiple() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    for _ in 0..10 {
        tx.send_fire_and_forget(BridgeCommand::GetTitle {
            target_id: TID.into(),
        });
    }
    std::thread::sleep(Duration::from_millis(20));
    let count = rx.drain(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
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
    tx.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    tx.send_fire_and_forget(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    assert!(tx.is_alive());
}

// ============================================================================
// Bridge channel: clone
// ============================================================================

#[test]
fn test_sender_clone_shares_channel() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();
    tx.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    tx2.send_fire_and_forget(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    std::thread::sleep(Duration::from_millis(10));
    let count = rx.drain(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(count, 2);
}

#[test]
fn test_sender_clone_independent_timeout() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(50));
    let tx2 = tx.clone();
    let resp1 = tx.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    let resp2 = tx2.send(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    assert!(resp1.result.is_err());
    assert!(resp2.result.is_err());
}

// ============================================================================
// BridgeResponse construction
// ============================================================================

#[test]
fn test_bridge_response_ok() {
    let resp = BridgeResponse {
        result: Ok(json!({"key": "value"})),
    };
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["key"], "value");
}

#[test]
fn test_bridge_response_err() {
    let resp = BridgeResponse {
        result: Err("test error".into()),
    };
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
            rx.try_process(|cmd| {
                processed2.fetch_add(1, Ordering::SeqCst);
                match cmd {
                    // getTargets first enumerates pages via ListTargets (the
                    // real PagePool face), then resolves live title/url per id.
                    BridgeCommand::ListTargets => BridgeResponse {
                        result: Ok(json!([
                            { "id": "test-target", "title": "Test Title", "url": "https://example.com" }
                        ])),
                    },
                    BridgeCommand::GetTitle { .. } => BridgeResponse {
                        result: Ok(json!("Test Title")),
                    },
                    BridgeCommand::GetUrl { .. } => BridgeResponse {
                        result: Ok(json!("https://example.com")),
                    },
                    _ => BridgeResponse {
                        result: Ok(json!(null)),
                    },
                }
            });
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
            if matches!(cmd, BridgeCommand::ClosePage { .. }) {
                closed2.fetch_add(1, Ordering::SeqCst);
            }
            BridgeResponse {
                result: Ok(json!({})),
            }
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
    // New contract (6983871b): the bridge response is the truth — the real
    // frameId (page id) and fresh loaderId come from the bridge handler and
    // must pass through verbatim, never fabricated by the protocol layer.
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
                BridgeResponse {
                    result: Ok(json!({"frameId": "t1", "loaderId": "load-7f3a"})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "Page.navigate",
        Some(json!({"url": "https://example.com"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let result = resp.result.unwrap();
    assert_eq!(result["frameId"], "t1");
    assert!(result["loaderId"].is_string());
    assert_eq!(navigated.load(Ordering::SeqCst), 1);
}

#[test]
fn test_page_navigate_no_bridge_explicit_error() {
    // New contract (6983871b): navigation requires the servo bridge —
    // explicit -32603, never a fabricated frameId:"0" success.
    let resp = dispatch("Page.navigate", Some(json!({})));
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge"));
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
            let got = rx.try_process(|cmd| match cmd {
                BridgeCommand::EvaluateJs { expression, .. } => BridgeResponse {
                    result: Ok(json!({"type": "number", "value": 42, "description": expression})),
                },
                _ => BridgeResponse {
                    result: Ok(json!({})),
                },
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "Runtime.evaluate",
        Some(json!({"expression": "1+1"})),
        "t1",
        &tx,
    );
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
            let got = rx.try_process(|cmd| match cmd {
                BridgeCommand::QuerySelector { selector, .. } => BridgeResponse {
                    result: Ok(json!({"nodeId": 42, "selector": selector})),
                },
                _ => BridgeResponse {
                    result: Ok(json!({})),
                },
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "DOM.querySelector",
        Some(json!({"selector": "div.main"})),
        "t1",
        &tx,
    );
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
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.enable", Some(json!({"patterns": [{"urlPattern": "*"}]})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_enable_no_patterns() {
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.enable", None);
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_continue_request() {
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.continueRequest", Some(json!({"requestId": "r1"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_fail_request() {
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.failRequest", Some(json!({"requestId": "r2", "reason": "Aborted"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_fulfill_request() {
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.fulfillRequest", Some(json!({"requestId": "r3", "responseCode": 200, "body": "hi"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_fulfill_request_default_status() {
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.fulfillRequest", Some(json!({"requestId": "r3", "body": "hi"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_take_response_body_as_stream() {
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.takeResponseBodyAsStream", Some(json!({"requestId": "r6"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_get_request_post_data() {
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.getRequestPostData", Some(json!({"requestId": "r4"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_continue_with_auth() {
    // REQ-CDP contract: bao has no request interception facility —
    // explicit error, never a canned success.
    let resp = dispatch("Fetch.continueWithAuth", Some(json!({"requestId": "r5"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

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
fn test_network_get_response_body_no_bridge_explicit_error() {
    // New contract (6983871b): servo exposes no response-body store —
    // explicit -32603 without a bridge, never an empty-body fake success.
    let resp = dispatch("Network.getResponseBody", None);
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none(), "error response must not carry a body");
}

#[test]
fn test_network_set_cache_disabled() {
    let resp = dispatch(
        "Network.setCacheDisabled",
        Some(json!({"cacheDisabled": true})),
    );
    assert!(resp.result.is_some());
}

#[test]
fn test_network_set_extra_http_headers_no_bridge_explicit_error() {
    // New contract (6983871b): servo has no per-target extra-headers API —
    // the bridge reports real support; without one it is an explicit -32603,
    // never a silent header drop masquerading as success.
    let resp = dispatch(
        "Network.setExtraHTTPHeaders",
        Some(json!({"headers": {"X-Custom": "value"}})),
    );
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none());
}

#[test]
fn test_network_delete_cookies() {
    let resp = dispatch("Network.deleteCookies", Some(json!({"name": "session"})));
    assert!(resp.result.is_some());
}

#[test]
fn test_network_set_cookie() {
    let resp = dispatch(
        "Network.setCookie",
        Some(json!({"name": "test", "value": "1"})),
    );
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
    let resp = dispatch(
        "Debugger.setBreakpointByUrl",
        Some(json!({"lineNumber": 10})),
    );
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
    let resp = dispatch(
        "Debugger.evaluateOnCallFrame",
        Some(json!({"callFrameId": "0", "expression": "1+1"})),
    );
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
    let ev = CdpEvent {
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
    let ev = CdpEvent {
        method: "Runtime.executionContextDestroyed".into(),
        params: None,
    };
    let serialized = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["method"], "Runtime.executionContextDestroyed");
}

// ============================================================================
// ADVERSARIAL: Bridge timeout error message specificity
// ============================================================================

#[test]
fn test_bridge_send_timeout_distinguishes_response_timeout() {
    // Receiver alive but slow → "bridge response timeout" (recv_timeout deadline).
    // Build queue first (so send succeeds on the request channel), then never
    // drain the response — recv_timeout must fire.
    let (tx, _rx) = bridge_channel(Duration::from_millis(20));
    // Pre-seed the queue with fire-and-forget so the request channel stays open
    // and `send` returns via the response channel (which we never service).
    tx.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    let resp = tx.send(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    assert!(resp.result.is_err());
    let err = resp.result.unwrap_err();
    assert_eq!(
        err, "bridge response timeout",
        "alive-but-slow receiver must produce 'bridge response timeout', got: {err}"
    );
    assert!(
        !err.contains("closed"),
        "response-timeout must not be confused with channel-closed"
    );
}

#[test]
fn test_bridge_send_distinguishes_channel_closed() {
    // Receiver dropped before send → "bridge channel closed" (send() on mpsc err).
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    let resp = tx.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    assert!(resp.result.is_err());
    let err = resp.result.unwrap_err();
    assert_eq!(
        err, "bridge channel closed",
        "dropped receiver must produce 'bridge channel closed', got: {err}"
    );
    assert!(
        !err.contains("timeout"),
        "channel-closed must not be confused with response-timeout"
    );
}

#[test]
fn test_bridge_send_zero_duration_timeout() {
    // Boundary: Duration::ZERO must still return a usable error (recv_timeout(0)).
    let (tx, _rx) = bridge_channel(Duration::from_nanos(1));
    let resp = tx.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    assert!(
        resp.result.is_err(),
        "Duration::ZERO must yield timeout error, not hang"
    );
    assert_eq!(resp.result.unwrap_err(), "bridge response timeout");
}

#[test]
fn test_bridge_send_after_drop_then_clone() {
    // Drop one clone, sender via the other clone must observe closed channel.
    let (tx, rx) = bridge_channel(Duration::from_millis(50));
    let tx_clone = tx.clone();
    drop(rx);
    let resp = tx_clone.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "bridge channel closed");
    // Both clones observe the same closed state.
    let resp2 = tx.send(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    assert_eq!(resp2.result.unwrap_err(), "bridge channel closed");
}

#[test]
fn test_bridge_fire_and_forget_silent_on_dropped_receiver() {
    // Fire-and-forget must never panic when receiver is gone (best-effort send).
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    tx.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    // If we reach here without panic, the contract holds.
}

#[test]
fn test_bridge_is_alive_false_after_drop_then_send_observes_closed() {
    // After dropping rx, is_alive must be false (its probe send errors).
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    assert!(
        !tx.is_alive(),
        "is_alive must be false once receiver is dropped"
    );
    // And a real send reflects the closed state with the right error string.
    let resp = tx.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    assert_eq!(resp.result.unwrap_err(), "bridge channel closed");
}

#[test]
fn test_bridge_send_returns_responder_value_not_fab() {
    // Sanity: send() returns the value produced by the handler, not a default.
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let t = std::thread::spawn(move || {
        rx.recv_and_process(Duration::from_secs(2), |cmd| match cmd {
            BridgeCommand::GetTitle { .. } => BridgeResponse {
                result: Ok(json!({"nested": {"arr": [1, 2, 3]}, "n": -7})),
            },
            _ => BridgeResponse {
                result: Ok(json!(null)),
            },
        });
    });
    let resp = tx.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    t.join().unwrap();
    assert!(resp.result.is_ok());
    let v = resp.result.unwrap();
    assert_eq!(v["nested"]["arr"][2], 3);
    assert_eq!(v["n"], -7);
}

#[test]
fn test_bridge_drain_delivers_to_send_responders() {
    // `send` (not fire-and-forget) blocks on a responder; drain must service it
    // so the blocking send unblocks with the handler's value. Adversarial:
    // naive drain implementations can drop responder handles.
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let join = std::thread::spawn(move || {
        tx.send(BridgeCommand::GetUrl {
            target_id: TID.into(),
        })
    });
    std::thread::sleep(Duration::from_millis(20));
    let n = rx.drain(|cmd| match cmd {
        BridgeCommand::GetUrl { .. } => BridgeResponse {
            result: Ok(json!("https://drained.example")),
        },
        _ => BridgeResponse {
            result: Ok(json!(null)),
        },
    });
    assert_eq!(
        n, 1,
        "drain must service the pending responder-bearing request"
    );
    let resp = join.join().unwrap();
    assert_eq!(resp.result.unwrap(), json!("https://drained.example"));
}

#[test]
fn test_bridge_try_process_delivers_to_send_responder() {
    // try_process must also service a blocking send (responder round-trip).
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let join = std::thread::spawn(move || {
        tx.send(BridgeCommand::GetTitle {
            target_id: TID.into(),
        })
    });
    // Wait until the request has been enqueued.
    let mut processed = false;
    for _ in 0..200 {
        let got = rx.try_process(|cmd| match cmd {
            BridgeCommand::GetTitle { .. } => BridgeResponse {
                result: Ok(json!("title-ok")),
            },
            _ => BridgeResponse {
                result: Ok(json!(null)),
            },
        });
        if got {
            processed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        processed,
        "try_process must service the responder-bearing request"
    );
    let resp = join.join().unwrap();
    assert_eq!(resp.result.unwrap(), json!("title-ok"));
}

// ============================================================================
// ADVERSARIAL: Target domain full command coverage (REQ-CDP-001)
// ============================================================================

#[test]
fn test_target_create_target_echoes_target_id() {
    // createTarget requires the servo bridge (a real page must be created) —
    // without one it is an explicit error, never an echo of the current id.
    let resp = dispatch("Target.createTarget", Some(json!({"url": "http://x"})));
    let err = resp.error.expect("createTarget must fail without a bridge");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge connected"));

}

#[test]
fn test_target_get_target_info_no_bridge_defaults() {
    let resp = dispatch("Target.getTargetInfo", None);
    let result = resp.result.unwrap();
    let info = &result["targetInfo"];
    assert_eq!(info["type"], "page");
    assert_eq!(info["attached"], true);
    // No bridge → title defaults to "Bao", url defaults to "about:blank".
    assert_eq!(info["title"], "Bao");
    assert_eq!(info["url"], "about:blank");
}

#[test]
fn test_target_get_targets_no_bridge_uses_default_title_url() {
    let resp = dispatch("Target.getTargets", None);
    let info = &resp.result.unwrap()["targetInfos"][0];
    assert_eq!(info["title"], "Bao");
    assert_eq!(info["url"], "about:blank");
    assert_eq!(info["type"], "page");
    assert_eq!(info["attached"], true);
}

#[test]
fn test_target_get_target_targets_alias_works() {
    // The handler matches both "getTargets" and "getTargetTargets" aliases.
    let resp = dispatch("Target.getTargetTargets", None);
    assert!(resp.result.unwrap()["targetInfos"].is_array());
}

#[test]
fn test_target_attach_to_target_session_id_deterministic() {
    // Session minting lives in the WS session registry (bao_browser) — the
    // stateless dispatch must refuse explicitly, never fabricate a
    // deterministic hash sessionId.
    let resp = dispatch("Target.attachToTarget", Some(json!({"targetId": "t1"})));
    let err = resp.error.expect("attachToTarget must fail without the WS registry");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("WS session registry"));

}

#[test]
fn test_target_attach_to_target_empty_target_id_session_id() {
    // Boundary preserved: empty target_id also refuses explicitly (the
    // fabricated all-zeros sessionId is eradicated).
    let resp = dispatch("Target.attachToTarget", Some(json!({"targetId": ""})));
    let err = resp.error.expect("attachToTarget must fail without the WS registry");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("WS session registry"));

}

#[test]
fn test_target_set_auto_attach_and_discover_empty_result() {
    // setAutoAttach/setDiscoverTargets are subscription acks (events flow via
    // the broadcaster); the session-table commands refuse explicitly.
    for cmd in ["setAutoAttach", "setDiscoverTargets"] {
        let resp = dispatch(&format!("Target.{cmd}"), None);
        assert!(resp.result.is_some(), "{cmd} must return a result");
        let r = resp.result.unwrap();
        assert!(
            r.as_object().map(|o| o.is_empty()).unwrap_or(false),
            "{cmd} must return an empty object, got: {r}"
        );
        assert!(resp.error.is_none(), "{cmd} must not error");
    }
    for cmd in ["detachFromTarget", "sendMessageToTarget"] {
        let resp = dispatch(&format!("Target.{cmd}"), None);
        let err = resp.error.expect("{cmd} must fail without the WS registry");
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("WS session registry"), "{cmd}");
    }

}

#[test]
fn test_target_close_target_no_bridge_still_success() {
    // Closing a page is a blocking bridge round-trip — without a bridge it
    // is an explicit error, never a fire-and-forget fake success.
    let resp = dispatch("Target.closeTarget", Some(json!({"targetId": "t1"})));
    let err = resp.error.expect("closeTarget must fail without a bridge");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge connected"));

}

#[test]
fn test_target_unknown_subcommand_method_not_found() {
    let resp = dispatch("Target.bogusSubcommand", None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert!(
        err.message.contains("Target.bogusSubcommand"),
        "error message must echo the full method: {}",
        err.message
    );
}

// ============================================================================
// ADVERSARIAL: Page domain full coverage (REQ-CDP-001/003)
// ============================================================================

#[test]
fn test_page_enable_disable_empty_result() {
    for cmd in ["enable", "disable"] {
        let resp = dispatch(&format!("Page.{cmd}"), None);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_page_navigate_loader_id_from_bridge_response() {
    // New contract (6983871b): the old fabricated rule
    // `loaderId = format!("{:016x}", url.len())` is eradicated — loaderIds
    // are per-load values from the bridge handler. Adversarial: a distinctive
    // loaderId that CANNOT be derived from the url length must pass through
    // verbatim ("https://example.com".len()=19 → the old canned rule would
    // produce 0000000000000013, not this value).
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|_| BridgeResponse {
                result: Ok(json!({"frameId": "t1", "loaderId": "load-not-url-len"})),
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "Page.navigate",
        Some(json!({"url": "https://example.com"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let r = resp.result.expect("bridge response must pass through");
    assert_eq!(r["loaderId"], "load-not-url-len");
    assert_eq!(r["frameId"], "t1");
}

#[test]
fn test_page_navigate_empty_url_defaults_to_about_blank() {
    // BCE-20260621-EMPTY-STR guard: empty/missing url must fall back to
    // "about:blank" in the Navigate command the bridge receives (verified by
    // capturing the actual bridge command, not a canned response).
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<String>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::Navigate { url, .. } = cmd {
                    *captured2.lock().unwrap() = Some(url);
                }
                BridgeResponse {
                    result: Ok(json!({"frameId": "t1", "loaderId": "load-1"})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("Page.navigate", Some(json!({})), "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let url = captured
        .lock()
        .unwrap()
        .take()
        .expect("Navigate bridge command must fire");
    assert_eq!(url, "about:blank", "empty url must default to about:blank");
}

#[test]
fn test_page_navigate_non_string_url_falls_back_to_default() {
    // Boundary: url present but not a string → as_str() is None → the bridge
    // must still receive the "about:blank" default.
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<String>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::Navigate { url, .. } = cmd {
                    *captured2.lock().unwrap() = Some(url);
                }
                BridgeResponse {
                    result: Ok(json!({"frameId": "t1", "loaderId": "load-1"})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("Page.navigate", Some(json!({"url": 42})), "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let url = captured
        .lock()
        .unwrap()
        .take()
        .expect("Navigate bridge command must fire");
    assert_eq!(url, "about:blank", "non-string url must fall back to about:blank");
}

#[test]
fn test_page_navigate_with_bridge_timeout_surfaces_32603_error() {
    // Adversarial: when bridge is set but the Navigate bridge call times out,
    // the error must propagate as JSON-RPC -32603 (internal error) per bridge_send.
    let (tx, _rx) = bridge_channel(Duration::from_millis(10));
    let resp = dispatch_bridge(
        "Page.navigate",
        Some(json!({"url": "https://slow.example"})),
        "t1",
        &tx,
    );
    assert!(
        resp.result.is_none(),
        "navigate must NOT succeed when bridge times out"
    );
    let err = resp
        .error
        .as_ref()
        .expect("navigate must produce an error on bridge timeout");
    assert_eq!(
        err.code, -32603,
        "bridge timeout must surface as -32603, got {}",
        err.code
    );
    assert_eq!(err.message, "bridge response timeout");
}

#[test]
fn test_page_navigate_missing_url_no_bridge_explicit_error() {
    // New contract (6983871b): navigate without a bridge is an explicit
    // -32603 regardless of the url defaulting logic.
    let resp = dispatch("Page.navigate", None);
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge"));
}

#[test]
fn test_page_reload_no_bridge_explicit_error() {
    // New contract (6983871b): reload goes through the real WebView::reload
    // path via the bridge — without one it is an explicit -32603, never the
    // canned frameId/loaderId "0" success.
    let resp = dispatch("Page.reload", Some(json!({"ignoreCache": true})));
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none());
}

#[test]
fn test_page_reload_with_bridge_routes_reload_command() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(Vec::<bool>::new()));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::Reload { ignore_cache, .. } = cmd {
                    captured2.lock().unwrap().push(ignore_cache);
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("Page.reload", Some(json!({"ignoreCache": true})), "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let guard = captured.lock().unwrap();
    assert_eq!(
        guard.len(),
        1,
        "Reload bridge command must be dispatched exactly once"
    );
    assert_eq!(
        guard[0], true,
        "ignoreCache must propagate to BridgeCommand::Reload"
    );
}

#[test]
fn test_page_get_frame_tree_no_bridge_explicit_error() {
    // New contract (6983871b): frame url/mimeType/name/origin are read from
    // the live document via the bridge; without one it is an explicit -32603,
    // never a fabricated about:blank frame tree.
    let resp = dispatch("Page.getFrameTree", None);
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge"));
}

#[test]
fn test_page_get_navigation_history_not_supported() {
    // New contract (6983871b): servo WebView exposes no session-history
    // enumeration — explicit -32000, never a fabricated single-entry history.
    let resp = dispatch("Page.getNavigationHistory", None);
    let err = resp.error.expect("history enumeration must fail loudly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("not supported"));
    assert!(resp.result.is_none());
}

#[test]
fn test_page_capture_screenshot_no_bridge_explicit_error() {
    // New contract (6983871b): no renderer without the bridge — explicit
    // -32603, never the canned {"data":""} success.
    let resp = dispatch("Page.captureScreenshot", None);
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none(), "no fabricated image data");
}

#[test]
fn test_page_capture_screenshot_with_bridge_carries_format_quality() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<(String, Option<u8>)>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::TakeScreenshot {
                    format, quality, ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some((format, quality));
                }
                BridgeResponse {
                    result: Ok(json!({"data": "base64png"})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "Page.captureScreenshot",
        Some(json!({"format": "jpeg", "quality": 80})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    assert_eq!(resp.result.unwrap()["data"], "base64png");
    let (fmt, q) = captured
        .lock()
        .unwrap()
        .take()
        .expect("screenshot bridge command must fire");
    assert_eq!(fmt, "jpeg");
    assert_eq!(q, Some(80));
}

#[test]
fn test_page_capture_screenshot_default_format_png_quality_none() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<(String, Option<u8>)>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::TakeScreenshot {
                    format, quality, ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some((format, quality));
                }
                BridgeResponse {
                    result: Ok(json!({"data": ""})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("Page.captureScreenshot", None, "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    let (fmt, q) = captured.lock().unwrap().take().unwrap();
    assert_eq!(fmt, "png", "default format must be png");
    assert!(q.is_none(), "default quality must be None");
}

#[test]
fn test_page_add_script_empty_source_skips_bridge() {
    // Chrome-compatible: an empty init script registers as a no-op with a
    // fresh identifier (Playwright's placeholder registration) — no bridge
    // round-trip, no rejection.
    let resp = dispatch(
        "Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source": ""})),
    );
    let result = resp.result.expect("empty source registers as a no-op");
    assert!(result["identifier"].as_str().unwrap().starts_with("script-"));
}

#[test]
fn test_page_add_script_nonempty_source_dispatches_bridge() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<String>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::AddScriptToEvaluateOnNewDocument { source, .. } = cmd {
                    *captured2.lock().unwrap() = Some(source);
                }
                BridgeResponse {
                    result: Ok(json!({"identifier": "41"})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source": "console.log('hi')"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    // New contract (6983871b): the identifier is genuinely generated behind
    // the bridge — its response is the truth (no hardcoded "1").
    assert_eq!(resp.result.unwrap()["identifier"], "41");
    let src = captured
        .lock()
        .unwrap()
        .take()
        .expect("non-empty source must fire bridge command");
    assert_eq!(src, "console.log('hi')");
}

#[test]
fn test_page_remove_script_to_evaluate_on_new_document_empty() {
    // New contract (6983871b): a missing identifier param is -32602 invalid
    // params — the old silent ok is eradicated.
    let resp = dispatch("Page.removeScriptToEvaluateOnNewDocument", None);
    let err = resp.error.expect("missing identifier must be rejected");
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("identifier"));
}

#[test]
fn test_page_misc_commands_no_bridge_explicit_errors() {
    // New contract (6983871b): all three are real paths now —
    // setContent validates its html param (-32602), close and bringToFront
    // require the bridge (-32603). No silent empty successes remain.
    let resp = dispatch("Page.setContent", None);
    let err = resp.error.expect("setContent without html must be rejected");
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("html"));

    for cmd in ["close", "bringToFront"] {
        let resp = dispatch(&format!("Page.{cmd}"), None);
        let err = resp.error.expect("{cmd} requires the bridge");
        assert_eq!(err.code, -32603, "{cmd} must surface -32603 without a bridge");
        assert!(resp.result.is_none(), "{cmd} must not fake success");
    }
}

#[test]
fn test_page_get_layout_metrics_no_bridge_explicit_error() {
    // New contract (6983871b): layout metrics are computed live from the
    // document via the bridge; without one it is an explicit -32603 — the
    // hardcoded 1920x1080 constant is eradicated.
    let resp = dispatch("Page.getLayoutMetrics", None);
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none(), "no fabricated dimensions");
}

#[test]
fn test_page_unknown_subcommand_method_not_found() {
    let resp = dispatch("Page.doesNotExist", None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Page.doesNotExist"));
}

// ============================================================================
// ADVERSARIAL: Runtime domain full coverage (REQ-CDP-001/003)
// ============================================================================

#[test]
fn test_runtime_enable_returns_chrome_empty_object() {
    // New contract (6983871b): Chrome semantics — Runtime.enable returns {}
    // and fires executionContextCreated events; no fabricated
    // executionContextId in the response.
    let resp = dispatch("Runtime.enable", None);
    assert_eq!(resp.result.unwrap(), json!({}));
}

#[test]
fn test_runtime_disable_empty_result() {
    let resp = dispatch("Runtime.disable", None);
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_runtime_evaluate_empty_expression_skips_bridge() {
    // Boundary: empty expression must NOT dispatch the bridge (handler guards).
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let dispatched = Arc::new(AtomicUsize::new(0));
    let dispatched2 = dispatched.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|_cmd| {
                dispatched2.fetch_add(1, Ordering::SeqCst);
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "Runtime.evaluate",
        Some(json!({"expression": ""})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    assert_eq!(resp.result.unwrap()["result"]["type"], "undefined");
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        dispatched.load(Ordering::SeqCst),
        0,
        "empty expression must NOT fire the EvaluateJs bridge command"
    );
}

#[test]
fn test_runtime_evaluate_nonempty_expression_uses_bridge_when_present() {
    // Adversarial: when expression is non-empty AND bridge present → bridge path.
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<(String, bool)>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::EvaluateJs {
                    expression,
                    return_by_value,
                    ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some((expression, return_by_value));
                }
                BridgeResponse {
                    result: Ok(json!({"type": "number", "value": 7})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "Runtime.evaluate",
        Some(json!({"expression": "3+4", "returnByValue": false})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    assert_eq!(resp.result.unwrap()["value"], 7);
    let (expr, rbv) = captured.lock().unwrap().take().unwrap();
    assert_eq!(expr, "3+4");
    assert_eq!(rbv, false, "returnByValue=false must propagate to bridge");
}

#[test]
fn test_runtime_evaluate_return_by_value_defaults_true() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<bool>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::EvaluateJs {
                    return_by_value, ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some(return_by_value);
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "Runtime.evaluate",
        Some(json!({"expression": "x"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let rbv = captured.lock().unwrap().take().unwrap();
    assert!(rbv, "returnByValue default must be true");
}

#[test]
fn test_runtime_evaluate_with_bridge_timeout_surfaces_32603() {
    let (tx, _rx) = bridge_channel(Duration::from_millis(10));
    let resp = dispatch_bridge(
        "Runtime.evaluate",
        Some(json!({"expression": "x"})),
        "t1",
        &tx,
    );
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32603);
    assert_eq!(err.message, "bridge response timeout");
}

#[test]
fn test_runtime_misc_commands_return_undefined_result() {
    // callFunctionOn/evaluateAsync/runScript return {result: {type: undefined}};
    // getProperties returns {result: []} (empty property array).
    for (method, params) in [
        ("Runtime.callFunctionOn", json!({})),
        ("Runtime.evaluateAsync", json!({})),
        ("Runtime.runScript", json!({})),
    ] {
        let resp = dispatch(method, Some(params));
        let r = resp.result.unwrap();
        assert_eq!(
            r["result"]["type"], "undefined",
            "{method} must return undefined-typed result"
        );
    }
    let resp = dispatch("Runtime.getProperties", Some(json!({})));
    let r = resp.result.unwrap();
    assert!(
        r["result"].is_array(),
        "Runtime.getProperties must return an array"
    );
    assert_eq!(r["result"].as_array().unwrap().len(), 0);
}

#[test]
fn test_runtime_noop_commands_empty_result() {
    for cmd in [
        "releaseObject",
        "releaseObjectGroup",
        "compileScript",
        "callArgument",
    ] {
        let resp = dispatch(&format!("Runtime.{cmd}"), None);
        assert!(resp.result.is_some(), "{cmd} must return a result");
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_runtime_unknown_subcommand_method_not_found() {
    let resp = dispatch("Runtime.notARealMethod", None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Runtime.notARealMethod"));
}

// ============================================================================
// ADVERSARIAL: DOM domain full coverage (REQ-CDP-001/003)
// ============================================================================

#[test]
fn test_dom_enable_disable_empty_result() {
    for cmd in ["enable", "disable"] {
        let resp = dispatch(&format!("DOM.{cmd}"), None);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_dom_get_document_no_bridge_returns_default_tree() {
    let resp = dispatch("DOM.getDocument", None);
    let root = &resp.result.unwrap()["root"];
    assert_eq!(root["nodeId"], 1);
    assert_eq!(root["nodeName"], "#document");
    assert_eq!(root["children"][0]["nodeName"], "HTML");
    assert_eq!(root["children"][0]["nodeType"], 1);
}

#[test]
fn test_dom_get_document_with_bridge_routes_command() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let dispatched = Arc::new(AtomicUsize::new(0));
    let dispatched2 = dispatched.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if matches!(cmd, BridgeCommand::GetDocument { .. }) {
                    dispatched2.fetch_add(1, Ordering::SeqCst);
                }
                BridgeResponse {
                    result: Ok(json!({"root": {"nodeId": 999}})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("DOM.getDocument", None, "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert_eq!(resp.result.unwrap()["root"]["nodeId"], 999);
    assert_eq!(dispatched.load(Ordering::SeqCst), 1);
}

#[test]
fn test_dom_query_selector_empty_selector_skips_bridge() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let dispatched = Arc::new(AtomicUsize::new(0));
    let dispatched2 = dispatched.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|_cmd| {
                dispatched2.fetch_add(1, Ordering::SeqCst);
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "DOM.querySelector",
        Some(json!({"selector": ""})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    assert_eq!(resp.result.unwrap()["nodeId"], 0);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[test]
fn test_dom_query_selector_all_empty_selector_default_empty() {
    let resp = dispatch("DOM.querySelectorAll", Some(json!({"selector": ""})));
    let result = resp.result.unwrap();
    let node_ids = result["nodeIds"].as_array().unwrap();
    assert_eq!(node_ids.len(), 0);
}

#[test]
fn test_dom_query_selector_all_with_bridge_routes() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<String>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::QuerySelectorAll { selector, .. } = cmd {
                    *captured2.lock().unwrap() = Some(selector);
                }
                BridgeResponse {
                    result: Ok(json!({"nodeIds": [10, 20]})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "DOM.querySelectorAll",
        Some(json!({"selector": "li.item"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let ids = resp.result.unwrap()["nodeIds"].as_array().unwrap().clone();
    assert_eq!(ids[0], 10);
    assert_eq!(ids[1], 20);
    assert_eq!(captured.lock().unwrap().take().unwrap(), "li.item");
}

#[test]
fn test_dom_describe_node_constant_shape() {
    let resp = dispatch("DOM.describeNode", None);
    let node = &resp.result.unwrap()["node"];
    assert_eq!(node["nodeId"], 1);
    assert_eq!(node["nodeType"], 1);
    assert_eq!(node["nodeName"], "HTML");
}

#[test]
fn test_dom_get_box_model_constant_geometry() {
    let resp = dispatch("DOM.getBoxModel", None);
    let model = &resp.result.unwrap()["model"];
    assert_eq!(model["width"], 1920);
    assert_eq!(model["height"], 1080);
    let content = model["content"].as_array().unwrap();
    assert_eq!(content.len(), 8, "content must be 8-element quad");
}

#[test]
fn test_dom_set_attribute_value_no_bridge_empty() {
    let resp = dispatch(
        "DOM.setAttributeValue",
        Some(json!({"nodeId": 5, "name": "class", "value": "x"})),
    );
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_dom_set_attribute_value_with_bridge_routes_command() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<(i64, String, String)>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::SetAttributeValue {
                    node_id,
                    name,
                    value,
                    ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some((node_id, name, value));
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "DOM.setAttributeValue",
        Some(json!({"nodeId": 42, "name": "data-x", "value": "v"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let (nid, name, value) = captured.lock().unwrap().take().unwrap();
    assert_eq!(nid, 42);
    assert_eq!(name, "data-x");
    assert_eq!(value, "v");
}

#[test]
fn test_dom_set_attribute_value_default_node_id_zero() {
    // Boundary: missing nodeId must default to 0 (unwrap_or(0)).
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<i64>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::SetAttributeValue { node_id, .. } = cmd {
                    *captured2.lock().unwrap() = Some(node_id);
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "DOM.setAttributeValue",
        Some(json!({"name": "a", "value": "b"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let nid = captured.lock().unwrap().take().unwrap();
    assert_eq!(nid, 0, "missing nodeId must default to 0");
}

#[test]
fn test_dom_get_outer_html_no_bridge_explicit_error() {
    // New contract (6983871b): outerHTML is read from the live document via
    // the bridge; without one it is an explicit -32603, never canned html.
    let resp = dispatch("DOM.getOuterHTML", None);
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none(), "no canned outerHTML payload");
}

#[test]
fn test_dom_get_outer_html_with_bridge_routes_command() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<Option<i64>>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::GetOuterHtml { node_id, .. } = cmd {
                    *captured2.lock().unwrap() = Some(node_id);
                }
                BridgeResponse {
                    result: Ok(json!({"outerHTML": "<p/>"})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("DOM.getOuterHTML", Some(json!({"nodeId": 7})), "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert_eq!(resp.result.unwrap()["outerHTML"], "<p/>");
    assert_eq!(captured.lock().unwrap().take().unwrap(), Some(7));
}

#[test]
fn test_dom_misc_noop_commands_empty_result() {
    for cmd in [
        "removeAttribute",
        "setOuterHTML",
        "insertBefore",
        "removeNode",
    ] {
        let resp = dispatch(&format!("DOM.{cmd}"), None);
        assert!(resp.result.is_some(), "{cmd} must return a result");
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_dom_resolve_node_and_push_nodes_default_shapes() {
    let resp = dispatch("DOM.resolveNode", None);
    assert_eq!(resp.result.unwrap()["object"]["type"], "node");
    let resp = dispatch("DOM.pushNodesByBackendIdsToFrontend", None);
    assert_eq!(resp.result.unwrap()["nodeIds"].as_array().unwrap().len(), 0);
}

#[test]
fn test_dom_unknown_subcommand_method_not_found() {
    let resp = dispatch("DOM.wrongCommand", None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("DOM.wrongCommand"));
}

// ============================================================================
// ADVERSARIAL: Emulation domain (REQ-CDP-001/003)
// ============================================================================

#[test]
fn test_emulation_set_device_metrics_no_bridge_empty_result() {
    let resp = dispatch(
        "Emulation.setDeviceMetricsOverride",
        Some(json!({"width": 800, "height": 600, "deviceScaleFactor": 2.0})),
    );
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_emulation_set_device_metrics_defaults_width_height() {
    // Boundary: missing width/height must default to 1920x1080.
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<(u32, u32, Option<f64>)>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::SetViewport {
                    width,
                    height,
                    device_scale_factor,
                    ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some((width, height, device_scale_factor));
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "Emulation.setDeviceMetricsOverride",
        Some(json!({})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let (w, h, dsf) = captured.lock().unwrap().take().unwrap();
    assert_eq!(w, 1920);
    assert_eq!(h, 1080);
    assert!(dsf.is_none());
}

#[test]
fn test_emulation_set_device_metrics_propagates_all_fields() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<(u32, u32, Option<f64>)>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::SetViewport {
                    width,
                    height,
                    device_scale_factor,
                    ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some((width, height, device_scale_factor));
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "Emulation.setDeviceMetricsOverride",
        Some(json!({"width": 1366, "height": 768, "deviceScaleFactor": 1.5})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let (w, h, dsf) = captured.lock().unwrap().take().unwrap();
    assert_eq!(w, 1366);
    assert_eq!(h, 768);
    assert_eq!(dsf, Some(1.5));
}

#[test]
fn test_emulation_clear_device_metrics_override_empty() {
    let resp = dispatch("Emulation.clearDeviceMetricsOverride", None);
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_emulation_set_user_agent_override_empty_skips_bridge() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let dispatched = Arc::new(AtomicUsize::new(0));
    let dispatched2 = dispatched.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|_cmd| {
                dispatched2.fetch_add(1, Ordering::SeqCst);
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge(
        "Emulation.setUserAgentOverride",
        Some(json!({"userAgent": ""})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        dispatched.load(Ordering::SeqCst),
        0,
        "empty UA must not fire bridge command"
    );
}

#[test]
fn test_emulation_set_user_agent_override_nonempty_dispatches() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<String>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::SetUserAgent { user_agent, .. } = cmd {
                    *captured2.lock().unwrap() = Some(user_agent);
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "Emulation.setUserAgentOverride",
        Some(json!({"userAgent": "Mozilla/5.0 Bao"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    assert_eq!(captured.lock().unwrap().take().unwrap(), "Mozilla/5.0 Bao");
}

#[test]
fn test_emulation_misc_noop_commands_empty_result() {
    for cmd in [
        "setTouchEmulationEnabled",
        "setScriptExecutionDisabled",
        "setFocusEmulationEnabled",
        "setCPUThrottlingRate",
        "setDefaultBackgroundColorOverride",
    ] {
        let resp = dispatch(&format!("Emulation.{cmd}"), None);
        assert!(resp.result.is_some(), "{cmd} must return a result");
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_emulation_unknown_subcommand_method_not_found() {
    let resp = dispatch("Emulation.unknownCommand", None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Emulation.unknownCommand"));
}

// ============================================================================
// ADVERSARIAL: Input domain (REQ-CDP-001/003)
// ============================================================================

#[test]
fn test_input_dispatch_mouse_event_no_bridge_empty_result() {
    let resp = dispatch(
        "Input.dispatchMouseEvent",
        Some(json!({"type": "mousePressed", "x": 10.0, "y": 20.0, "button": 0, "clickCount": 1})),
    );
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_input_dispatch_mouse_event_with_bridge_propagates_fields() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(
        None::<(String, f64, f64, Option<i64>, Option<i64>)>,
    ));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::DispatchMouseEvent {
                    event_type,
                    x,
                    y,
                    button,
                    click_count,
                    ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some((event_type, x, y, button, click_count));
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "Input.dispatchMouseEvent",
        Some(json!({"type": "mouseReleased", "x": 12.5, "y": -3.0, "button": 2, "clickCount": 3})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let (et, x, y, b, c) = captured.lock().unwrap().take().unwrap();
    assert_eq!(et, "mouseReleased");
    assert_eq!(x, 12.5);
    assert_eq!(y, -3.0);
    assert_eq!(b, Some(2));
    assert_eq!(c, Some(3));
}

#[test]
fn test_input_dispatch_mouse_event_defaults_x_y_zero() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<(f64, f64)>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::DispatchMouseEvent { x, y, .. } = cmd {
                    *captured2.lock().unwrap() = Some((x, y));
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "Input.dispatchMouseEvent",
        Some(json!({"type": "mouseMoved"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let (x, y) = captured.lock().unwrap().take().unwrap();
    assert_eq!(x, 0.0);
    assert_eq!(y, 0.0);
}

#[test]
fn test_input_dispatch_key_event_with_bridge_propagates_fields() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(
        None::<(String, String, String, Option<String>)>,
    ));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::DispatchKeyEvent {
                    event_type,
                    key,
                    code,
                    text,
                    ..
                } = cmd
                {
                    *captured2.lock().unwrap() = Some((event_type, key, code, text));
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "Input.dispatchKeyEvent",
        Some(json!({"type": "keyDown", "key": "Enter", "code": "Enter", "text": "\r"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    let (et, k, c, t) = captured.lock().unwrap().take().unwrap();
    assert_eq!(et, "keyDown");
    assert_eq!(k, "Enter");
    assert_eq!(c, "Enter");
    assert_eq!(t, Some("\r".into()));
}

#[test]
fn test_input_dispatch_touch_event_empty_result() {
    let resp = dispatch("Input.dispatchTouchEvent", Some(json!({})));
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_input_insert_text_empty_skips_bridge() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let dispatched = Arc::new(AtomicUsize::new(0));
    let dispatched2 = dispatched.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|_cmd| {
                dispatched2.fetch_add(1, Ordering::SeqCst);
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resp = dispatch_bridge("Input.insertText", Some(json!({"text": ""})), "t1", &tx);
    done.store(1, Ordering::Relaxed);
    assert!(resp.result.is_some());
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[test]
fn test_input_insert_text_nonempty_dispatches() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let captured = Arc::new(std::sync::Mutex::new(None::<String>));
    let captured2 = captured.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let done2 = done.clone();
    std::thread::spawn(move || {
        while done2.load(Ordering::Relaxed) == 0 {
            let got = rx.try_process(|cmd| {
                if let BridgeCommand::InsertText { text, .. } = cmd {
                    *captured2.lock().unwrap() = Some(text);
                }
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
            if got {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let _ = dispatch_bridge(
        "Input.insertText",
        Some(json!({"text": "hello"})),
        "t1",
        &tx,
    );
    done.store(1, Ordering::Relaxed);
    assert_eq!(captured.lock().unwrap().take().unwrap(), "hello");
}

#[test]
fn test_input_unknown_subcommand_method_not_found() {
    let resp = dispatch("Input.notAnInputCommand", None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Input.notAnInputCommand"));
}

// ============================================================================
// ADVERSARIAL: Network domain full coverage (REQ-CDP-006)
// ============================================================================

#[test]
fn test_network_enable_disable_empty_result() {
    for cmd in ["enable", "disable"] {
        let resp = dispatch(&format!("Network.{cmd}"), None);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_network_get_response_body_with_request_id_no_bridge_explicit_error() {
    // New contract (6983871b): the old "constant shape"
    // ({"body":"", "base64Encoded":false}) was a canned success —
    // servo exposes no response-body store, so this is an explicit -32603.
    let resp = dispatch("Network.getResponseBody", Some(json!({"requestId": "r1"})));
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none(), "no fake empty body");
}

#[test]
fn test_network_cache_disabled_ok_headers_require_bridge() {
    // New contract (6983871b): setCacheDisabled without a bridge is a genuine
    // no-op ok (nothing to disable); setExtraHTTPHeaders is NOT — headers are
    // never silently dropped, it surfaces -32603 without a bridge.
    let resp = dispatch("Network.setCacheDisabled", None);
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());

    let resp = dispatch("Network.setExtraHTTPHeaders", None);
    let err = resp.error.expect("headers must not be silently dropped");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none());
}

#[test]
fn test_network_emulate_and_intercept_noop() {
    for cmd in [
        "emulateNetworkConditions",
        "setRequestInterception",
        "continueInterceptedRequest",
    ] {
        let resp = dispatch(&format!("Network.{cmd}"), None);
        assert!(resp.result.is_some(), "{cmd} must return a result");
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_network_get_all_cookies_returns_array() {
    let resp = dispatch("Network.getAllCookies", None);
    let r = resp.result.unwrap();
    assert!(r["cookies"].is_array());
    assert_eq!(r["cookies"].as_array().unwrap().len(), 0);
}

#[test]
fn test_network_delete_and_set_cookie_noop() {
    for cmd in ["deleteCookies", "setCookie"] {
        let resp = dispatch(&format!("Network.{cmd}"), None);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }
}

// ============================================================================
// ADVERSARIAL: CSS, Overlay, Log, Debugger, Fetch completeness (REQ-CDP-001/003/006)
// ============================================================================

#[test]
fn test_css_enable_disable_empty_result() {
    for cmd in ["enable", "disable"] {
        let resp = dispatch(&format!("CSS.{cmd}"), None);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }
}

#[test]
fn test_css_get_inline_styles_for_node_null_inline_style() {
    let resp = dispatch("CSS.getInlineStylesForNode", Some(json!({"nodeId": 1})));
    assert!(resp.result.unwrap()["inlineStyle"].is_null());
}

#[test]
fn test_overlay_set_show_overlays_default_behavior() {
    // Overlay domain only exposes highlightNode/hideHighlight/setInspectMode/
    // setPausedInDebuggerMessage; any other Overlay.* must error with -32601.
    let resp = dispatch("Overlay.setShowOverlays", None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_log_domain_enable_disable_noop_shape() {
    // Adversarial: Log.enable/disable/clear/start/stop all return ok_empty.
    for cmd in [
        "enable",
        "disable",
        "clear",
        "startViolationsReport",
        "stopViolationsReport",
    ] {
        let resp = dispatch(&format!("Log.{cmd}"), None);
        assert!(resp.result.is_some(), "Log.{cmd} must return a result");
        assert!(resp.error.is_none(), "Log.{cmd} must not error");
    }
}

#[test]
fn test_debugger_enable_and_misc_default_shapes() {
    // Debugger.enable/disable/pause/resume/step* map to BridgeCommand via the
    // bridge path; without a bridge they must NOT be handled by the stub here
    // (the Debugger handler is a thin dispatch returning method-not-found for
    // unknown). Verify the documented Debugger.* stubs used by this layer.
    for cmd in [
        "setBreakpointByUrl",
        "getPossibleBreakpoints",
        "getScriptSource",
        "evaluateOnCallFrame",
        "setPauseOnExceptions",
    ] {
        let resp = dispatch(&format!("Debugger.{cmd}"), None);
        // All listed Debugger commands must produce a result (no error).
        assert!(resp.result.is_some(), "{cmd} must return a result");
        assert!(resp.error.is_none(), "{cmd} must not error");
    }
}

#[test]
fn test_fetch_enable_disable_coverage() {
    // Fetch.disable is an idempotent ok; Fetch.enable is an explicit error
    // (no request interception facility).
    let resp = dispatch("Fetch.disable", None);
    assert!(resp.result.is_some(), "Fetch.disable must return a result");
    assert!(resp.error.is_none());
    let resp = dispatch(
        "Fetch.enable",
        Some(json!({"patterns": [{"urlPattern": "*"}, {"urlPattern": "*.js"}]})),
    );
    let err = resp.error.expect("Fetch.enable must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_get_request_post_data_constant_shape() {
    // REQ-CDP contract: no request interception facility — explicit error.
    let resp = dispatch("Fetch.getRequestPostData", Some(json!({"requestId": "r-x"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_continue_with_auth_constant_shape() {
    // REQ-CDP contract: no request interception facility — explicit error.
    let resp = dispatch("Fetch.continueWithAuth", Some(json!({"requestId": "r-y"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

#[test]
fn test_fetch_take_response_bodyAsStream_naming() {
    // REQ-CDP contract: no request interception facility — explicit error.
    let resp = dispatch("Fetch.takeResponseBodyAsStream", Some(json!({"requestId": "r-z"})));
    let err = resp.error.expect("must fail explicitly");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));

}

// ============================================================================
// ADVERSARIAL: protocol dispatch edge cases (REQ-CDP-001)
// ============================================================================

#[test]
fn test_handle_command_preserves_request_id() {
    // JSON-RPC 2.0: response.id must echo request.id.
    let msg = CdpMessage {
        id: Some(999),
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t1", &None, None);
    assert_eq!(resp.id, Some(999));
    assert!(resp.result.is_some());
}

#[test]
fn test_handle_command_none_id_notification_style() {
    // Boundary: id = None (notification) → response.id must also be None.
    let msg = CdpMessage {
        id: None,
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t1", &None, None);
    assert_eq!(resp.id, None);
    assert!(resp.result.is_some());
}

#[test]
fn test_handle_command_error_carries_request_id() {
    let msg = CdpMessage {
        id: Some(-5),
        method: "Nope.nope".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t1", &None, None);
    assert_eq!(resp.id, Some(-5));
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
}

#[test]
fn test_handle_command_dot_only_method_errors() {
    // Boundary: method = "Target." → domain "Target", command "" → method-not-found.
    let msg = CdpMessage {
        id: Some(1),
        method: "Target.".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t1", &None, None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_handle_command_method_with_multiple_dots() {
    // splitn(2, '.') → "A.B.C" → domain "A", command "B.C" → unknown.
    let msg = CdpMessage {
        id: Some(1),
        method: "Target.getTargets.extra".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t1", &None, None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_handle_command_session_id_ignored_for_dispatch() {
    // session_id is not used by handle_command; dispatch must succeed regardless.
    let msg = CdpMessage {
        id: Some(1),
        method: "Page.enable".into(),
        params: None,
        session_id: Some("sess-1".into()),
    };
    let resp = handle_command(msg, "t1", &None, None);
    assert!(resp.result.is_some());
}

#[test]
fn test_unknown_method_message_echoes_full_method_name() {
    // Adversarial: error message uses msg.method (not parsed parts), so a
    // method without a dot echoes verbatim.
    let msg = CdpMessage {
        id: Some(1),
        method: "justoneword".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t1", &None, None);
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "'justoneword' wasn't found");
}

// ============================================================================
// ADVERSARIAL: serialize_response / serialize_event edge cases (REQ-CDP-001)
// ============================================================================

#[test]
fn test_serialize_response_with_none_id() {
    let resp = bao_cdp::CdpResponse {
        id: None,
        result: Some(json!({"ok": 1})),
        error: None,
    };
    let s = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(
        parsed["id"].is_null(),
        "response with id=None must serialize id as null"
    );
    assert_eq!(parsed["result"]["ok"], 1);
}

#[test]
fn test_serialize_response_error_with_none_id() {
    let resp = bao_cdp::CdpResponse {
        id: None,
        result: None,
        error: Some(bao_cdp::CdpError {
            code: -32601,
            message: "x".into(),
        }),
    };
    let s = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(parsed["id"].is_null());
    assert_eq!(parsed["error"]["code"], -32601);
    assert_eq!(parsed["error"]["message"], "x");
}

#[test]
fn test_serialize_event_with_complex_params() {
    let ev = CdpEvent {
        method: "Network.responseReceived".into(),
        params: Some(json!({
            "requestId": "req",
            "response": {
                "url": "https://example.com",
                "status": 200,
                "headers": {"Content-Type": "text/html"},
            }
        })),
    };
    let s = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["method"], "Network.responseReceived");
    assert_eq!(parsed["params"]["response"]["status"], 200);
    assert_eq!(
        parsed["params"]["response"]["headers"]["Content-Type"],
        "text/html"
    );
}

#[test]
fn test_serialize_response_round_trip_idempotent() {
    let resp = bao_cdp::CdpResponse {
        id: Some(42),
        result: Some(json!({"a": [1, 2, {"b": true}]})),
        error: None,
    };
    let s1 = serialize_response(&resp);
    let s2 = serialize_response(&resp);
    assert_eq!(
        s1, s2,
        "serialization must be deterministic for the same input"
    );
    let parsed: serde_json::Value = serde_json::from_str(&s1).unwrap();
    let s3 = serde_json::to_string(&parsed).unwrap();
    assert_eq!(
        s1, s3,
        "serialization must be idempotent across round-trips"
    );
}

// ============================================================================
// ADVERSARIAL: bridge_send error path (-32603) when bridge absent (REQ-CDP-003)
// ============================================================================

#[test]
fn test_bridge_dependent_command_no_bridge_yields_32603() {
    // New contract (6983871b): Page.navigate is genuinely bridge-dependent —
    // the frameId/loaderId come from the bridge response, so without a bridge
    // it must surface -32603 "no servo bridge connected". The old no-bridge
    // fallback success (fabricated frameId "0") is eradicated.
    let resp = dispatch("Page.navigate", Some(json!({"url": "https://example.com"})));
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(
        err.message.contains("no servo bridge"),
        "error must name the missing bridge, got: {}",
        err.message
    );
    assert!(resp.result.is_none());
}

#[test]
fn test_bridge_send_internal_error_code_is_32603() {
    // JSON-RPC internal error code is -32603. Adversarial: confirm bridge
    // timeouts surface as -32603 (not -32601 method-not-found).
    let (tx, _rx) = bridge_channel(Duration::from_millis(5));
    let resp = dispatch_bridge(
        "Page.navigate",
        Some(json!({"url": "https://timeout.example"})),
        "t1",
        &tx,
    );
    let err = resp
        .error
        .as_ref()
        .expect("bridge timeout must produce error");
    assert_eq!(err.code, -32603);
    assert_ne!(
        err.code, -32601,
        "bridge timeout must not be confused with method-not-found"
    );
}

// ============================================================================
// ADVERSARIAL: bridge_channel concurrency & resource behavior
// ============================================================================

#[test]
fn test_bridge_concurrent_senders_interleave_correctly() {
    // Multiple sender clones across threads must all reach the same receiver,
    // and drain must process exactly the number sent.
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let counter = Arc::new(AtomicUsize::new(0));
    let mut joins = Vec::new();
    for _ in 0..4 {
        let tx_c = tx.clone();
        let c = counter.clone();
        joins.push(std::thread::spawn(move || {
            for _ in 0..25 {
                tx_c.send_fire_and_forget(BridgeCommand::GetTitle {
                    target_id: TID.into(),
                });
                c.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for j in joins {
        j.join().unwrap();
    }
    std::thread::sleep(Duration::from_millis(20));
    let drained = rx.drain(|_| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(drained, 100, "all 4x25 commands must be drained");
    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

#[test]
fn test_bridge_drain_is_idempotent_when_empty() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    assert_eq!(
        rx.drain(|_| BridgeResponse {
            result: Ok(json!({}))
        }),
        0
    );
    // Second drain on still-empty channel must also return 0.
    assert_eq!(
        rx.drain(|_| BridgeResponse {
            result: Ok(json!({}))
        }),
        0
    );
}

#[test]
fn test_bridge_recv_and_process_false_on_short_timeout() {
    // Boundary: recv_and_process with very short timeout and no sender activity.
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let processed = rx.recv_and_process(Duration::from_millis(5), |_| BridgeResponse {
        result: Ok(json!(null)),
    });
    assert!(!processed);
}

#[test]
fn test_bridge_recv_and_process_returns_true_on_command() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let join = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        tx.send_fire_and_forget(BridgeCommand::GetTitle {
            target_id: TID.into(),
        });
    });
    let processed = rx.recv_and_process(Duration::from_secs(2), |cmd| {
        assert!(matches!(cmd, BridgeCommand::GetTitle { .. }));
        BridgeResponse {
            result: Ok(json!("handled")),
        }
    });
    join.join().unwrap();
    assert!(processed);
}

// ============================================================================
// ADVERSARIAL: SPEC criterion alignment — JSON-RPC 2.0 error codes (REQ-CDP-001-C2)
// ============================================================================

#[test]
fn test_jsonrpc_method_not_found_code_is_minus_32601() {
    // JSON-RPC 2.0 §5.1: method not found = -32601. Adversarial: confirm
    // numeric value (not just "is_some").
    let resp = dispatch("Nope.nope", None);
    let code = resp.error.as_ref().unwrap().code;
    assert_eq!(
        code, -32601,
        "method-not-found MUST be -32601 per JSON-RPC 2.0 §5.1"
    );
}

#[test]
fn test_jsonrpc_success_envelope_has_result_no_error() {
    // JSON-RPC 2.0 §4.2: success response has `result` and MUST NOT have `error`.
    let resp = dispatch("Page.enable", None);
    assert!(resp.result.is_some());
    assert!(
        resp.error.is_none(),
        "success response MUST NOT carry error"
    );
}

#[test]
fn test_jsonrpc_error_envelope_has_error_no_result() {
    // JSON-RPC 2.0 §4.2: error response has `error` and MUST NOT have `result`.
    let resp = dispatch("Nope.nope", None);
    assert!(resp.error.is_some());
    assert!(
        resp.result.is_none(),
        "error response MUST NOT carry result"
    );
}
