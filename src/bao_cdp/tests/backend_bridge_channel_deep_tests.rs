// @trace TEST-CDP-025 [req:REQ-CDP-001,REQ-CDP-003,REQ-CDP-004] [level:unit]
// BridgeChannel send/recv/drain lifecycle, BridgeCommand variants debug,
// BridgeResponse result handling, InternalBackend indirect test via handle_command.

use std::time::Duration;

use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeResponse};
use bao_cdp::{handle_command, CdpMessage, CdpResponse};
use serde_json::json;

const TID: &str = "test-target";

// ---- InternalBackend indirect tests (via handle_command) ----

fn dispatch(method: &str, params: Option<serde_json::Value>) -> CdpResponse {
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: None,
        session_id: None,
    };
    handle_command(msg, "test-target", &params, None)
}

#[test]
fn test_internal_page_navigate_no_bridge_explicit_error() {
    // New contract (6983871b): the bridge response carries the real
    // frameId/loaderId — without a bridge navigate is an explicit -32603,
    // never a fabricated frameId:"0" success.
    let resp = dispatch("Page.navigate", Some(json!({"url":"http://test"})));
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge"));
    assert!(resp.result.is_none(), "error response must not carry result");
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
    let resp = dispatch(
        "Emulation.setDeviceMetricsOverride",
        Some(json!({"width":1920,"height":1080})),
    );
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
    let resp = dispatch(
        "Input.dispatchMouseEvent",
        Some(json!({"type":"mousePressed","x":100,"y":200})),
    );
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
    let cmd = BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "http://test".into(),
    };
    assert!(format!("{:?}", cmd).contains("Navigate"));
}

#[test]
fn test_bridge_cmd_evaluate_debug() {
    let cmd = BridgeCommand::EvaluateJs {
        target_id: TID.into(),
        expression: "1+1".into(),
        return_by_value: true,
    };
    assert!(format!("{:?}", cmd).contains("EvaluateJs"));
}

#[test]
fn test_bridge_cmd_screenshot_debug() {
    let cmd = BridgeCommand::TakeScreenshot {
        target_id: TID.into(),
        format: "png".into(),
        quality: Some(90),
    };
    assert!(format!("{:?}", cmd).contains("TakeScreenshot"));
}

#[test]
fn test_bridge_cmd_get_title_debug() {
    assert!(format!(
        "{:?}",
        BridgeCommand::GetTitle {
            target_id: TID.into()
        }
    )
    .contains("GetTitle"));
}

#[test]
fn test_bridge_cmd_get_url_debug() {
    assert!(format!(
        "{:?}",
        BridgeCommand::GetUrl {
            target_id: TID.into()
        }
    )
    .contains("GetUrl"));
}

#[test]
fn test_bridge_cmd_get_document_debug() {
    assert!(format!(
        "{:?}",
        BridgeCommand::GetDocument {
            target_id: TID.into()
        }
    )
    .contains("GetDocument"));
}

#[test]
fn test_bridge_cmd_query_selector_debug() {
    let cmd = BridgeCommand::QuerySelector {
        target_id: TID.into(),
        selector: "div".into(),
    };
    assert!(format!("{:?}", cmd).contains("QuerySelector"));
}

#[test]
fn test_bridge_cmd_query_selector_all_debug() {
    let cmd = BridgeCommand::QuerySelectorAll {
        target_id: TID.into(),
        selector: "div.cls".into(),
    };
    assert!(format!("{:?}", cmd).contains("QuerySelectorAll"));
}

#[test]
fn test_bridge_cmd_mouse_event_debug() {
    let cmd = BridgeCommand::DispatchMouseEvent {
        target_id: TID.into(),
        event_type: "mousePressed".into(),
        x: 100.0,
        y: 200.0,
        button: Some(0),
        click_count: Some(1),
    };
    assert!(format!("{:?}", cmd).contains("DispatchMouseEvent"));
}

#[test]
fn test_bridge_cmd_key_event_debug() {
    let cmd = BridgeCommand::DispatchKeyEvent {
        target_id: TID.into(),
        event_type: "keyDown".into(),
        key: "a".into(),
        code: "KeyA".into(),
        text: Some("a".into()),
    };
    assert!(format!("{:?}", cmd).contains("DispatchKeyEvent"));
}

#[test]
fn test_bridge_cmd_insert_text_debug() {
    let cmd = BridgeCommand::InsertText {
        target_id: TID.into(),
        text: "hello".into(),
    };
    assert!(format!("{:?}", cmd).contains("InsertText"));
}

#[test]
fn test_bridge_cmd_set_viewport_debug() {
    let cmd = BridgeCommand::SetViewport {
        target_id: TID.into(),
        width: 1920,
        height: 1080,
        device_scale_factor: Some(2.0),
    };
    assert!(format!("{:?}", cmd).contains("SetViewport"));
}

#[test]
fn test_bridge_cmd_set_user_agent_debug() {
    let cmd = BridgeCommand::SetUserAgent {
        target_id: TID.into(),
        user_agent: "TestBot/1.0".into(),
    };
    assert!(format!("{:?}", cmd).contains("SetUserAgent"));
}

#[test]
fn test_bridge_cmd_get_cookies_debug() {
    let cmd = BridgeCommand::GetCookies {
        target_id: TID.into(),
        urls: vec!["http://a.com".into()],
    };
    assert!(format!("{:?}", cmd).contains("GetCookies"));
}

#[test]
fn test_bridge_cmd_get_all_cookies_debug() {
    assert!(format!(
        "{:?}",
        BridgeCommand::GetAllCookies {
            target_id: TID.into()
        }
    )
    .contains("GetAllCookies"));
}

#[test]
fn test_bridge_cmd_set_cookie_debug() {
    let cmd = BridgeCommand::SetCookie {
        target_id: TID.into(),
        name: "session".into(),
        value: "abc".into(),
        url: Some("http://test".into()),
        domain: None,
    };
    assert!(format!("{:?}", cmd).contains("SetCookie"));
}

#[test]
fn test_bridge_cmd_delete_cookie_debug() {
    let cmd = BridgeCommand::DeleteCookie {
        target_id: TID.into(),
        name: "session".into(),
        url: None,
    };
    assert!(format!("{:?}", cmd).contains("DeleteCookie"));
}

#[test]
fn test_bridge_cmd_get_response_body_debug() {
    let cmd = BridgeCommand::GetResponseBody {
        target_id: TID.into(),
        request_id: "req-1".into(),
    };
    assert!(format!("{:?}", cmd).contains("GetResponseBody"));
}

#[test]
fn test_bridge_cmd_add_script_debug() {
    let cmd = BridgeCommand::AddScriptToEvaluateOnNewDocument {
        target_id: TID.into(),
        source: "console.log(1)".into(),
    };
    assert!(format!("{:?}", cmd).contains("AddScript"));
}

#[test]
fn test_bridge_cmd_reload_debug() {
    let cmd = BridgeCommand::Reload {
        target_id: TID.into(),
        ignore_cache: true,
    };
    assert!(format!("{:?}", cmd).contains("Reload"));
}

#[test]
fn test_bridge_cmd_go_back_debug() {
    assert!(format!(
        "{:?}",
        BridgeCommand::GoBack {
            target_id: TID.into()
        }
    )
    .contains("GoBack"));
}

#[test]
fn test_bridge_cmd_go_forward_debug() {
    assert!(format!(
        "{:?}",
        BridgeCommand::GoForward {
            target_id: TID.into()
        }
    )
    .contains("GoForward"));
}

#[test]
fn test_bridge_cmd_stop_loading_debug() {
    assert!(format!(
        "{:?}",
        BridgeCommand::StopLoading {
            target_id: TID.into()
        }
    )
    .contains("StopLoading"));
}

#[test]
fn test_bridge_cmd_close_page_debug() {
    assert!(format!(
        "{:?}",
        BridgeCommand::ClosePage {
            target_id: TID.into()
        }
    )
    .contains("ClosePage"));
}

#[test]
fn test_bridge_cmd_get_outer_html_debug() {
    let cmd = BridgeCommand::GetOuterHtml {
        target_id: TID.into(),
        node_id: Some(1),
    };
    assert!(format!("{:?}", cmd).contains("GetOuterHtml"));
}

#[test]
fn test_bridge_cmd_set_attribute_debug() {
    let cmd = BridgeCommand::SetAttributeValue {
        target_id: TID.into(),
        node_id: 5,
        name: "class".into(),
        value: "active".into(),
    };
    assert!(format!("{:?}", cmd).contains("SetAttributeValue"));
}

// ---- BridgeResponse ----

#[test]
fn test_bridge_response_ok() {
    let resp = BridgeResponse {
        result: Ok(json!({"ok": true})),
    };
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["ok"], true);
}

#[test]
fn test_bridge_response_err() {
    let resp = BridgeResponse {
        result: Err("error msg".into()),
    };
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "error msg");
}

#[test]
fn test_bridge_response_debug() {
    let resp = BridgeResponse {
        result: Ok(json!(42)),
    };
    assert!(format!("{:?}", resp).contains("42"));
}

// ---- BridgeChannel send/recv/drain ----

#[test]
fn test_channel_send_recv() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        receiver.try_process(|cmd| match cmd {
            BridgeCommand::Navigate { url, .. } => BridgeResponse {
                result: Ok(json!({"navigated": url})),
            },
            _ => BridgeResponse {
                result: Err("unexpected".into()),
            },
        });
    });

    let resp = sender.send(BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "http://test".into(),
    });
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["navigated"], "http://test");
}

#[test]
fn test_channel_closed_sender() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(1));
    drop(receiver);
    let resp = sender.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("closed"));
}

#[test]
fn test_channel_fire_and_forget() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    sender.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    let processed = receiver.try_process(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert!(processed);
}

#[test]
fn test_channel_drain_multiple() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    sender.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    sender.send_fire_and_forget(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    sender.send_fire_and_forget(BridgeCommand::GetDocument {
        target_id: TID.into(),
    });
    let count = receiver.drain(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(count, 3);
}

#[test]
fn test_channel_drain_empty() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let _ = sender;
    let count = receiver.drain(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(count, 0);
}

#[test]
fn test_channel_timeout() {
    let (sender, _receiver) = bridge_channel(Duration::from_millis(10));
    let resp = sender.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
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
            BridgeResponse {
                result: Ok(json!({})),
            }
        });
    });

    sender.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    cloned.send_fire_and_forget(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });

    let _ = handle.join();
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn test_channel_try_process_no_pending() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let _ = sender;
    let processed = receiver.try_process(|_cmd| BridgeResponse {
        result: Ok(json!({})),
    });
    assert!(!processed);
}

#[test]
fn test_channel_send_with_response_match() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));

    std::thread::spawn(move || loop {
        let got = receiver.try_process(|cmd| match cmd {
            BridgeCommand::GetTitle { .. } => BridgeResponse {
                result: Ok(json!("Test Title")),
            },
            _ => BridgeResponse {
                result: Ok(json!({})),
            },
        });
        if !got {
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    let resp = sender.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap(), json!("Test Title"));
}

// ===========================================================================
// Adversarial verification gap coverage (SPEC alignment + boundaries)
// @trace TEST-CDP-025 [req:REQ-CDP-001,REQ-CDP-003,REQ-CDP-004]
// ===========================================================================

// ---- SPEC alignment: CdpResponse.id propagation (JSON-RPC 2.0) ----
// REQ-CDP-001: JSON-RPC 2.0 message handling — id MUST round-trip into response.

#[test]
fn test_response_id_propagation_positive() {
    // id present in CdpMessage → response must echo same id back.
    let msg = CdpMessage {
        id: Some(42),
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, TID, &None, None);
    assert_eq!(resp.id, Some(42), "CdpResponse.id must echo CdpMessage.id");
    assert!(resp.result.is_some());
    assert!(
        resp.error.is_none(),
        "successful command must not carry error"
    );
}

#[test]
fn test_response_id_propagation_none_notification() {
    // JSON-RPC notification (id: None) → response.id is None (no id to echo).
    let msg = CdpMessage {
        id: None,
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, TID, &None, None);
    assert_eq!(resp.id, None, "notification id stays None");
    assert!(resp.result.is_some());
}

#[test]
fn test_response_id_propagation_on_error() {
    // Unknown method → error response must STILL echo the id (JSON-RPC 2.0 §5).
    let msg = CdpMessage {
        id: Some(-7),
        method: "Nope.nothing".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, TID, &None, None);
    assert_eq!(resp.id, Some(-7), "error response must preserve id");
    assert!(resp.error.is_some());
    assert!(
        resp.result.is_none(),
        "error response must not carry result"
    );
}

#[test]
fn test_response_id_propagation_negative_and_large() {
    for id in [i64::MIN, -1, 0, 1, i64::MAX] {
        let msg = CdpMessage {
            id: Some(id),
            method: "Page.enable".into(),
            params: None,
            session_id: None,
        };
        let resp = handle_command(msg, TID, &None, None);
        assert_eq!(resp.id, Some(id), "id round-trip failed for id={}", id);
    }
}

// ---- SPEC alignment: error codes (JSON-RPC 2.0 reserved range) ----
// ERR_METHOD_NOT_FOUND = -32601 per cdp-server/src/protocol.rs.

#[test]
fn test_unknown_command_error_code_is_method_not_found() {
    // Unknown subcommand within a known domain → -32601.
    let resp = dispatch("Page.nonexistentCommand", None);
    let err = resp.error.expect("unknown command must yield error");
    assert_eq!(
        err.code, -32601,
        "method-not-found MUST be -32601 (JSON-RPC 2.0)"
    );
    assert!(
        err.message.contains("Page.nonexistentCommand"),
        "error message must name the offending method, got: {}",
        err.message
    );
}

#[test]
fn test_unknown_domain_error_message_format() {
    let resp = dispatch("MysteryDomain.foo", None);
    let err = resp.error.expect("unknown domain must yield error");
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("MysteryDomain.foo"));
}

#[test]
fn test_empty_method_error_code() {
    // Empty method splits into ("","") → unknown domain "" → -32601, not crash.
    let resp = dispatch("", None);
    let err = resp
        .error
        .expect("empty method must yield error, not success");
    assert_eq!(err.code, -32601);
}

#[test]
fn test_dot_only_method_is_unknown_domain() {
    // Method "." → domain "" + command "" → -32601 (no panic on split).
    let resp = dispatch(".", None);
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn test_method_with_many_dots_splits_at_first() {
    // "A.b.c" → splitn(2,'.') → domain="A", command="b.c" → unknown → -32601.
    let resp = dispatch("A.b.c.d", None);
    let err = resp.error.expect("many-dot method must error");
    assert_eq!(err.code, -32601);
    // first segment is the domain; unknown domain → message references full method.
    assert!(err.message.contains("A.b.c.d"));
}

// ---- SPEC alignment: domain routing correctness (REQ-CDP-003/004) ----
// Each documented domain command returns Ok (no bridge needed for stubs).

#[test]
fn test_target_domain_create_target_returns_target_id() {
    let resp = dispatch("Target.createTarget", Some(json!({"url":"http://x"})));
    let result = resp.result.expect("createTarget must succeed");
    assert_eq!(
        result["targetId"], TID,
        "createTarget echoes routed target_id"
    );
}

#[test]
fn test_target_domain_attach_returns_session_id() {
    let resp = dispatch("Target.attachToTarget", Some(json!({"targetId":"t1"})));
    let result = resp.result.expect("attachToTarget must succeed");
    let sid = result["sessionId"]
        .as_str()
        .expect("sessionId must be a string");
    assert!(!sid.is_empty(), "sessionId must be non-empty hex");
    // sessionId is hex-formatted; verify hex-only characters.
    assert!(
        sid.chars().all(|c| c.is_ascii_hexdigit()),
        "sessionId not hex: {}",
        sid
    );
}

#[test]
fn test_target_domain_close_target_succeeds_without_bridge() {
    // No bridge → closeTarget still returns success (fire-and-forget is skipped).
    let resp = dispatch("Target.closeTarget", Some(json!({"targetId":"t1"})));
    let result = resp.result.expect("closeTarget must succeed");
    assert_eq!(result["success"], true);
}

#[test]
fn test_target_domain_get_targets_returns_array() {
    let resp = dispatch("Target.getTargets", None);
    let result = resp.result.expect("getTargets must succeed");
    let arr = result["targetInfos"]
        .as_array()
        .expect("targetInfos must be array");
    assert!(
        !arr.is_empty(),
        "getTargets returns at least the live target"
    );
    let info = &arr[0];
    assert_eq!(info["targetId"], TID);
    assert_eq!(info["type"], "page");
    assert_eq!(info["attached"], true);
}

#[test]
fn test_target_domain_unknown_subcommand_error() {
    let resp = dispatch("Target.bogusSubcommand", None);
    let err = resp.error.expect("unknown Target subcommand must error");
    assert_eq!(err.code, -32601);
}

#[test]
fn test_page_domain_navigate_no_bridge_explicit_error() {
    // New contract (6983871b): real frameId/loaderId come from the bridge
    // response; without a bridge navigate is an explicit -32603, never a
    // fabricated frameId:"0" success.
    let resp = dispatch("Page.navigate", Some(json!({"url":"http://test"})));
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge"));
    assert!(resp.result.is_none(), "error response must not carry result");
}

#[test]
fn test_page_domain_navigate_missing_url_no_bridge_explicit_error() {
    // No url param → the handler defaults the url to "about:blank" and still
    // needs the bridge; without one it is an explicit -32603 (the default-url
    // fallback itself is covered by the lib test for the bridge path).
    let resp = dispatch("Page.navigate", Some(json!({})));
    let err = resp
        .error
        .expect("navigate without url and without bridge must fail loudly");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge"));
}

#[test]
fn test_page_domain_capture_screenshot_no_bridge_explicit_error() {
    // New contract (6983871b): no renderer without the bridge — explicit
    // -32603, never the canned {"data":""} success.
    let resp = dispatch(
        "Page.captureScreenshot",
        Some(json!({"format":"jpeg","quality":50})),
    );
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge"));
    assert!(
        resp.result.is_none(),
        "error response must not carry fabricated image data"
    );
}

#[test]
fn test_page_domain_get_frame_tree_no_bridge_explicit_error() {
    // New contract (6983871b): frame url/mimeType/name/origin are read from
    // the live document via the bridge; without one it is an explicit -32603,
    // never a fabricated frame tree.
    let resp = dispatch("Page.getFrameTree", None);
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(err.message.contains("no servo bridge"));
}

#[test]
fn test_page_domain_unknown_subcommand_error() {
    let resp = dispatch("Page.totallyUnknown", None);
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn test_runtime_domain_enable_returns_chrome_empty_object() {
    // New contract (6983871b): Chrome semantics — Runtime.enable returns {}
    // and fires executionContextCreated events; the response carries no
    // executionContextId (that was a fabricated context id).
    let resp = dispatch("Runtime.enable", None);
    let result = resp.result.expect("Runtime.enable must succeed");
    assert_eq!(result, json!({}), "Runtime.enable result must be exactly {{}}");
    assert!(
        result.get("executionContextId").is_none(),
        "no fabricated executionContextId in the response"
    );
}

#[test]
fn test_runtime_domain_evaluate_empty_expression_no_bridge() {
    // Empty expression + no bridge → returns stub {result:{type:undefined}, exceptionDetails:null}.
    let resp = dispatch("Runtime.evaluate", Some(json!({"expression":""})));
    let result = resp.result.expect("evaluate must succeed");
    assert_eq!(result["result"]["type"], "undefined");
    assert_eq!(result["exceptionDetails"], serde_json::Value::Null);
}

#[test]
fn test_runtime_domain_call_function_on_stub() {
    let resp = dispatch("Runtime.callFunctionOn", None);
    let result = resp.result.expect("callFunctionOn must succeed");
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_dom_domain_get_document_no_bridge_structure() {
    let resp = dispatch("DOM.getDocument", None);
    let root = resp.result.unwrap()["root"].clone();
    assert_eq!(root["nodeId"], 1);
    assert_eq!(root["nodeName"], "#document");
    assert_eq!(root["nodeType"], 9); // Document node
    let children = root["children"].as_array().expect("children array");
    assert!(!children.is_empty());
    assert_eq!(children[0]["nodeName"], "HTML");
}

#[test]
fn test_dom_domain_query_selector_no_bridge_returns_zero() {
    let resp = dispatch("DOM.querySelector", Some(json!({"selector":"div"})));
    let result = resp.result.expect("querySelector without bridge succeeds");
    assert_eq!(result["nodeId"], 0);
}

#[test]
fn test_dom_domain_unknown_subcommand_error() {
    let resp = dispatch("DOM.noSuchDomCommand", None);
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn test_network_domain_get_response_body_no_bridge_explicit_error() {
    // New contract (6983871b): servo exposes no response-body store — the
    // bridge handler reports the real availability; without a bridge this is
    // an explicit -32603, never an empty-body fake success.
    let resp = dispatch("Network.getResponseBody", Some(json!({"requestId":"r-1"})));
    let err = resp.error.expect("no bridge must yield an error");
    assert_eq!(err.code, -32603);
    assert!(resp.result.is_none(), "error response must not carry a body");
}

#[test]
fn test_network_domain_get_cookies_returns_empty_array() {
    let resp = dispatch("Network.getCookies", None);
    let result = resp.result.expect("Network.getCookies must succeed");
    assert_eq!(result["cookies"].as_array().unwrap().len(), 0);
}

#[test]
fn test_network_domain_unknown_subcommand_error() {
    let resp = dispatch("Network.bogus", None);
    assert_eq!(resp.error.unwrap().code, -32601);
}

#[test]
fn test_css_domain_get_computed_style() {
    let resp = dispatch("CSS.getComputedStyleForNode", Some(json!({"nodeId":1})));
    let result = resp.result.expect("getComputedStyleForNode must succeed");
    assert!(result["computedStyle"].is_array());
}

#[test]
fn test_overlay_domain_highlight_node() {
    let resp = dispatch("Overlay.highlightNode", None);
    assert!(resp.result.is_some(), "highlightNode returns empty result");
}

#[test]
fn test_log_domain_clear() {
    let resp = dispatch("Log.clear", None);
    assert!(resp.result.is_some(), "Log.clear must succeed");
}

#[test]
fn test_debugger_domain_set_breakpoint_by_url_returns_id() {
    let resp = dispatch("Debugger.setBreakpointByUrl", None);
    let result = resp.result.expect("setBreakpointByUrl must succeed");
    assert_eq!(result["breakpointId"], "1");
    assert!(result["locations"].is_array());
}

#[test]
fn test_fetch_domain_enable_pattern_count_reflected() {
    let resp = dispatch(
        "Fetch.enable",
        Some(json!({"patterns":[{"urlPattern":"*"},{"requestStage":"Response"}]})),
    );
    let result = resp.result.expect("Fetch.enable must succeed");
    assert_eq!(result["enabled"], true);
    assert_eq!(
        result["patternCount"], 2,
        "patternCount reflects params.patterns.len()"
    );
}

#[test]
fn test_fetch_domain_fail_request_echoes_reason() {
    let resp = dispatch(
        "Fetch.failRequest",
        Some(json!({"requestId":"r-1","reason":"Failed"})),
    );
    let result = resp.result.unwrap();
    assert_eq!(result["requestId"], "r-1");
    assert_eq!(result["failed"], true);
    assert_eq!(result["reason"], "Failed");
}

#[test]
fn test_fetch_domain_enable_no_patterns_zero_count() {
    let resp = dispatch("Fetch.enable", None);
    let result = resp.result.unwrap();
    assert_eq!(result["patternCount"], 0);
}

// ---- BridgeCommand variant coverage: multi-target + Debugger (REQ-CDP-003) ----
// All variants must (1) construct, (2) be Clone, (3) produce recognizable Debug.

fn assert_clone_debug<T: std::fmt::Debug + Clone>(label: &str, v: &T) {
    let cloned = v.clone();
    let dbg = format!("{:?}", v);
    assert!(
        dbg.contains(label),
        "Debug for {} missing '{}': {}",
        label,
        label,
        dbg
    );
    let dbg2 = format!("{:?}", cloned);
    assert_eq!(dbg, dbg2, "clone must produce identical Debug output");
}

#[test]
fn test_bridge_cmd_create_target_clone_debug() {
    assert_clone_debug(
        "CreateTarget",
        &BridgeCommand::CreateTarget {
            url: "http://new".into(),
        },
    );
}

#[test]
fn test_bridge_cmd_list_targets_clone_debug() {
    assert_clone_debug("ListTargets", &BridgeCommand::ListTargets);
}

#[test]
fn test_bridge_cmd_debugger_enable_clone_debug() {
    assert_clone_debug(
        "DebuggerEnable",
        &BridgeCommand::DebuggerEnable {
            target_id: TID.into(),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_disable_clone_debug() {
    assert_clone_debug(
        "DebuggerDisable",
        &BridgeCommand::DebuggerDisable {
            target_id: TID.into(),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_set_breakpoint_clone_debug() {
    assert_clone_debug(
        "DebuggerSetBreakpoint",
        &BridgeCommand::DebuggerSetBreakpoint {
            target_id: TID.into(),
            url: Some("http://example.com".into()),
            url_regex: None,
            line: 5,
            column: Some(3),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_clear_breakpoint_clone_debug() {
    assert_clone_debug(
        "DebuggerRemoveBreakpoint",
        &BridgeCommand::DebuggerRemoveBreakpoint {
            target_id: TID.into(),
            breakpoint_id: "bp-1-5-0".into(),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_interrupt_clone_debug() {
    assert_clone_debug(
        "DebuggerInterrupt",
        &BridgeCommand::DebuggerInterrupt {
            target_id: TID.into(),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_resume_clone_debug() {
    assert_clone_debug(
        "DebuggerResume",
        &BridgeCommand::DebuggerResume {
            target_id: TID.into(),
            step_type: Some("into".into()),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_list_frames_clone_debug() {
    assert_clone_debug(
        "DebuggerListFrames",
        &BridgeCommand::DebuggerListFrames {
            target_id: TID.into(),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_get_environment_clone_debug() {
    assert_clone_debug(
        "DebuggerGetEnvironment",
        &BridgeCommand::DebuggerGetEnvironment {
            target_id: TID.into(),
            frame_actor_id: "f1".into(),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_eval_clone_debug() {
    assert_clone_debug(
        "DebuggerEval",
        &BridgeCommand::DebuggerEval {
            target_id: TID.into(),
            expression: "x".into(),
            frame_actor_id: None,
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_get_possible_breakpoints_clone_debug() {
    assert_clone_debug(
        "DebuggerGetPossibleBreakpoints",
        &BridgeCommand::DebuggerGetPossibleBreakpoints {
            target_id: TID.into(),
            start_script_id: "7".into(),
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_get_script_source_clone_debug() {
    assert_clone_debug(
        "DebuggerGetScriptSource",
        &BridgeCommand::DebuggerGetScriptSource {
            target_id: TID.into(),
            script_id: 7,
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_blackbox_clone_debug() {
    assert_clone_debug(
        "DebuggerBlackbox",
        &BridgeCommand::DebuggerBlackbox {
            target_id: TID.into(),
            script_id: 9,
        },
    );
}

#[test]
fn test_bridge_cmd_debugger_unblackbox_clone_debug() {
    assert_clone_debug(
        "DebuggerUnblackbox",
        &BridgeCommand::DebuggerUnblackbox {
            target_id: TID.into(),
            script_id: 9,
        },
    );
}

// ---- Boundary: empty/edge field values in BridgeCommand variants ----

#[test]
fn test_bridge_cmd_navigate_empty_url_debug() {
    let cmd = BridgeCommand::Navigate {
        target_id: "".into(),
        url: "".into(),
    };
    assert!(format!("{:?}", cmd).contains("Navigate"));
}

#[test]
fn test_bridge_cmd_evaluate_empty_expression_debug() {
    let cmd = BridgeCommand::EvaluateJs {
        target_id: TID.into(),
        expression: "".into(),
        return_by_value: false,
    };
    let dbg = format!("{:?}", cmd);
    assert!(dbg.contains("EvaluateJs"));
    assert!(
        dbg.contains("false"),
        "return_by_value=false must appear in Debug"
    );
}

#[test]
fn test_bridge_cmd_take_screenshot_quality_none_debug() {
    let cmd = BridgeCommand::TakeScreenshot {
        target_id: TID.into(),
        format: "jpeg".into(),
        quality: None,
    };
    assert!(format!("{:?}", cmd).contains("TakeScreenshot"));
}

#[test]
fn test_bridge_cmd_take_screenshot_quality_max_u8() {
    // u8 boundary: quality=255 (max). u8 boundary: quality=0 (min).
    for q in [0u8, 255u8] {
        let cmd = BridgeCommand::TakeScreenshot {
            target_id: TID.into(),
            format: "png".into(),
            quality: Some(q),
        };
        let dbg = format!("{:?}", cmd);
        assert!(
            dbg.contains(&q.to_string()),
            "quality={} must appear in Debug",
            q
        );
    }
}

#[test]
fn test_bridge_cmd_set_viewport_zero_dimensions() {
    let cmd = BridgeCommand::SetViewport {
        target_id: TID.into(),
        width: 0,
        height: 0,
        device_scale_factor: Some(0.0),
    };
    let dbg = format!("{:?}", cmd);
    assert!(dbg.contains("SetViewport"));
    assert!(dbg.contains("0"));
}

#[test]
fn test_bridge_cmd_set_viewport_no_dsf() {
    let cmd = BridgeCommand::SetViewport {
        target_id: TID.into(),
        width: 800,
        height: 600,
        device_scale_factor: None,
    };
    assert!(format!("{:?}", cmd).contains("SetViewport"));
}

#[test]
fn test_bridge_cmd_get_outer_html_node_id_none() {
    let cmd = BridgeCommand::GetOuterHtml {
        target_id: TID.into(),
        node_id: None,
    };
    assert!(format!("{:?}", cmd).contains("GetOuterHtml"));
}

#[test]
fn test_bridge_cmd_dispatch_mouse_no_button_no_click_count() {
    let cmd = BridgeCommand::DispatchMouseEvent {
        target_id: TID.into(),
        event_type: "mouseMoved".into(),
        x: -1.0,
        y: -1.0,
        button: None,
        click_count: None,
    };
    let dbg = format!("{:?}", cmd);
    assert!(dbg.contains("DispatchMouseEvent"));
    // negative coordinates must round-trip through Debug
    assert!(dbg.contains("-1"));
}

#[test]
fn test_bridge_cmd_dispatch_key_no_text() {
    let cmd = BridgeCommand::DispatchKeyEvent {
        target_id: TID.into(),
        event_type: "keyUp".into(),
        key: "Shift".into(),
        code: "ShiftLeft".into(),
        text: None,
    };
    assert!(format!("{:?}", cmd).contains("DispatchKeyEvent"));
}

#[test]
fn test_bridge_cmd_set_cookie_minimal() {
    // Both url and domain None — minimal cookie spec.
    let cmd = BridgeCommand::SetCookie {
        target_id: TID.into(),
        name: "".into(),
        value: "".into(),
        url: None,
        domain: None,
    };
    assert!(format!("{:?}", cmd).contains("SetCookie"));
}

#[test]
fn test_bridge_cmd_get_cookies_empty_urls_vec() {
    let cmd = BridgeCommand::GetCookies {
        target_id: TID.into(),
        urls: vec![],
    };
    let dbg = format!("{:?}", cmd);
    assert!(dbg.contains("GetCookies"));
}

#[test]
fn test_bridge_cmd_debugger_set_breakpoint_no_column() {
    let cmd = BridgeCommand::DebuggerSetBreakpoint {
        target_id: TID.into(),
        url: None,
        url_regex: None,
        line: 0,
        column: None,
    };
    let dbg = format!("{:?}", cmd);
    assert!(dbg.contains("DebuggerSetBreakpoint"));
    assert!(dbg.contains("0"));
}

#[test]
fn test_bridge_cmd_debugger_resume_no_step_type() {
    let cmd = BridgeCommand::DebuggerResume {
        target_id: TID.into(),
        step_type: None,
    };
    assert!(format!("{:?}", cmd).contains("DebuggerResume"));
}

#[test]
fn test_bridge_cmd_debugger_eval_with_frame_actor() {
    let cmd = BridgeCommand::DebuggerEval {
        target_id: TID.into(),
        expression: "var x".into(),
        frame_actor_id: Some("actor-42".into()),
    };
    let dbg = format!("{:?}", cmd);
    assert!(dbg.contains("DebuggerEval"));
    assert!(dbg.contains("actor-42"));
}

// ---- BridgeResponse field semantics ----

#[test]
fn test_bridge_response_ok_null_value() {
    let resp = BridgeResponse {
        result: Ok(serde_json::Value::Null),
    };
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap(), serde_json::Value::Null);
}

#[test]
fn test_bridge_response_err_empty_string() {
    let resp = BridgeResponse {
        result: Err(String::new()),
    };
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err().len(), 0);
}

#[test]
fn test_bridge_response_err_contains_unicode() {
    let msg = "通道关闭 💥".to_string();
    let resp = BridgeResponse {
        result: Err(msg.clone()),
    };
    assert_eq!(resp.result.unwrap_err(), msg);
}

#[test]
fn test_bridge_response_debug_err_variant() {
    let resp = BridgeResponse {
        result: Err("boom".into()),
    };
    let dbg = format!("{:?}", resp);
    assert!(
        dbg.contains("boom"),
        "Debug of Err must contain message: {}",
        dbg
    );
}

#[test]
fn test_bridge_response_ok_complex_value() {
    let val = json!({"nested":{"arr":[1,2,3]},"n":42.5});
    let resp = BridgeResponse {
        result: Ok(val.clone()),
    };
    assert_eq!(resp.result.unwrap(), val);
}

// ---- BridgeChannel: zero-duration timeout boundary ----

#[test]
fn test_channel_zero_duration_send_is_timeout_or_closed() {
    // timeout=0: recv_timeout(0) is effectively immediate.
    // Without an active responder thread, send must return Err
    // (either timeout or — if responder races — Ok from an instant reply).
    let (sender, _receiver) = bridge_channel(Duration::from_secs(0));
    let resp = sender.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    // No responder is draining → must be an error (timeout), never Ok from a ghost reply.
    assert!(
        resp.result.is_err(),
        "send with timeout=0 and no responder must be an error, got: {:?}",
        resp.result
    );
}

#[test]
fn test_channel_zero_duration_fire_and_forget_succeeds() {
    // fire-and-forget never blocks on the response — must succeed even at timeout=0.
    let (sender, receiver) = bridge_channel(Duration::from_secs(0));
    sender.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    let n = receiver.drain(|_| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(
        n, 1,
        "fire-and-forget at timeout=0 must still deliver the command"
    );
}

#[test]
fn test_channel_is_alive_uses_fire_and_forget_no_reply() {
    // is_alive internally sends a ListTargets request with a dropped responder.
    // It must NOT block and must reflect channel openness, not responder presence.
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    assert!(sender.is_alive(), "alive while receiver present");
    drop(receiver);
    assert!(!sender.is_alive(), "dead once receiver dropped");
}

// ---- BridgeChannel: clone preserves timeout value ----

#[test]
fn test_channel_clone_preserves_timeout_value() {
    // The Clone impl copies `timeout`; a cloned sender must enforce the SAME timeout.
    let (sender, _receiver) = bridge_channel(Duration::from_millis(5));
    let cloned = sender.clone();
    // Both senders have 5ms timeout; with no responder, both must time out.
    let r1 = sender.send(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    let r2 = cloned.send(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    assert!(r1.result.is_err(), "original sender must time out");
    assert!(
        r2.result.is_err(),
        "cloned sender must enforce same timeout"
    );
}

#[test]
fn test_channel_multiple_clones_single_receiver() {
    // 3 cloned senders all feed the same receiver; drain must collect all.
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let s1 = sender.clone();
    let s2 = sender.clone();

    s1.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    s2.send_fire_and_forget(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    sender.send_fire_and_forget(BridgeCommand::GetDocument {
        target_id: TID.into(),
    });

    let count = receiver.drain(|_| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(
        count, 3,
        "all three senders (orig + 2 clones) deliver to one receiver"
    );
}

#[test]
fn test_channel_recv_and_process_returns_command_value() {
    // recv_and_process must deliver the handler's response back to the sender.
    let (sender, receiver) = bridge_channel(Duration::from_secs(2));

    let h = std::thread::spawn(move || {
        receiver.recv_and_process(Duration::from_secs(2), |cmd| match cmd {
            BridgeCommand::Navigate { url, .. } => BridgeResponse {
                result: Ok(json!({"url": url})),
            },
            _ => BridgeResponse {
                result: Err("wrong cmd".into()),
            },
        })
    });

    let resp = sender.send(BridgeCommand::Navigate {
        target_id: TID.into(),
        url: "http://recv-proc".into(),
    });
    let processed = h.join().unwrap();
    assert!(processed, "recv_and_process must report processed=true");
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["url"], "http://recv-proc");
}

#[test]
fn test_channel_recv_and_process_timeout_false() {
    let (_sender, receiver) = bridge_channel(Duration::from_secs(5));
    let start = std::time::Instant::now();
    let processed = receiver.recv_and_process(Duration::from_millis(50), |_| BridgeResponse {
        result: Ok(json!({})),
    });
    let elapsed = start.elapsed();
    assert!(
        !processed,
        "recv_and_process with no sender must return false"
    );
    assert!(
        elapsed >= Duration::from_millis(40),
        "recv_and_process must actually block until timeout (elapsed={:?})",
        elapsed
    );
}

#[test]
fn test_channel_drain_returns_zero_after_consumer_already_emptied() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    sender.send_fire_and_forget(BridgeCommand::GetTitle {
        target_id: TID.into(),
    });
    let first = receiver.drain(|_| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(first, 1);
    let second = receiver.drain(|_| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(second, 0, "second drain on empty channel must return 0");
}

// ---- handle_command: bridge-backed paths return error without bridge ----
// Several handlers check `bridge.is_some()` and stub when None; others REQUIRE bridge.
// Verify the bridge-required paths surface -32603 when bridge is None.

#[test]
fn test_runtime_evaluate_with_expression_no_bridge_returns_stub_not_error() {
    // Per handle_runtime: bridge None + non-empty expression → stub (Ok), NOT error.
    let resp = dispatch("Runtime.evaluate", Some(json!({"expression":"1+1"})));
    assert!(
        resp.result.is_some(),
        "Runtime.evaluate without bridge returns stub, not error: {:?}",
        resp.error
    );
}

#[test]
fn test_emulation_set_ua_empty_no_bridge_no_send() {
    // Empty userAgent + no bridge → ok_empty (no send attempted).
    let resp = dispatch(
        "Emulation.setUserAgentOverride",
        Some(json!({"userAgent":""})),
    );
    assert!(
        resp.result.is_some(),
        "empty UA without bridge returns ok_empty"
    );
}

#[test]
fn test_input_insert_text_empty_no_bridge_ok() {
    let resp = dispatch("Input.insertText", Some(json!({"text":""})));
    assert!(
        resp.result.is_some(),
        "empty text without bridge returns ok_empty"
    );
}

#[test]
fn test_page_add_script_empty_source_rejected_invalid_params() {
    // New contract (6983871b): empty source is an explicit -32602
    // invalid-params error — identifier generation lives behind the bridge
    // (the old hardcoded {"identifier":"1"} stub is eradicated).
    let resp = dispatch(
        "Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source":""})),
    );
    let err = resp.error.expect("empty source must be rejected");
    assert_eq!(err.code, -32602);
    assert!(
        err.message.contains("source"),
        "error message must name the missing param, got: {}",
        err.message
    );
}

// ---- handle_command: CdpMessage.params field is ignored (params passed separately) ----
// The signature takes (msg, target_id, &params, bridge) — msg.params is NOT used.
// Adversarial: pass mismatched msg.params vs params arg → routing must use the `params` arg.

#[test]
fn test_handle_command_uses_external_params_not_msg_params() {
    // Adversarial intent preserved from the pre-6983871b test: routing must
    // read the `params` ARG, never CdpMessage.params. Page.navigate no longer
    // succeeds without a bridge (its canned frameId:"0" is eradicated), so the
    // observable carrier is Fetch.enable, whose patternCount is derived purely
    // from the params arg — mismatched msg.params must not leak into it.
    let msg = CdpMessage {
        id: Some(1),
        method: "Fetch.enable".into(),
        // Deliberately put a DIFFERENT pattern count in msg.params — it must
        // be IGNORED.
        params: Some(json!({"patterns":[{"urlPattern":"*"}]})),
        session_id: None,
    };
    // Pass the authoritative 2-pattern set via the `params` arg.
    let params = json!({"patterns":[{"urlPattern":"*"},{"requestStage":"Response"}]});
    let resp = handle_command(msg, TID, &Some(params), None);
    let result = resp
        .result
        .expect("dispatch must succeed using the params arg, not msg.params");
    assert_eq!(
        result["patternCount"], 2,
        "patternCount must reflect the params ARG (2), not msg.params (1)"
    );
}

// ---- handle_command: session_id field is accepted (does not crash dispatch) ----

#[test]
fn test_handle_command_with_session_id_does_not_crash() {
    let msg = CdpMessage {
        id: Some(1),
        method: "Page.enable".into(),
        params: None,
        session_id: Some("deadbeef-session".into()),
    };
    let resp = handle_command(msg, TID, &None, None);
    assert_eq!(resp.id, Some(1));
    assert!(resp.result.is_some());
}

// ---- Stress: rapid send/drain interleaving preserves count (REQ-CDP-006 channel ordering) ----

#[test]
fn test_channel_high_volume_drain_preserves_count() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    const N: usize = 500;
    for i in 0..N {
        sender.send_fire_and_forget(BridgeCommand::Navigate {
            target_id: TID.into(),
            url: format!("http://x/{}", i),
        });
    }
    let count = receiver.drain(|_| BridgeResponse {
        result: Ok(json!({})),
    });
    assert_eq!(
        count, N,
        "mpsc channel must deliver all {} commands in order",
        N
    );
}

#[test]
fn test_channel_send_then_drain_response_actually_delivered() {
    // Full round-trip: drain's handler response must reach the sender that called send().
    // This proves the responder oneshot channel is wired correctly end-to-end.
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));

    let h = std::thread::spawn(move || {
        receiver.drain(|cmd| match cmd {
            BridgeCommand::GetUrl { .. } => BridgeResponse {
                result: Ok(json!("http://drained-url")),
            },
            _ => BridgeResponse {
                result: Err("unexpected".into()),
            },
        })
    });

    let resp = sender.send(BridgeCommand::GetUrl {
        target_id: TID.into(),
    });
    let _ = h.join();
    assert!(
        resp.result.is_ok(),
        "drain must route response back to sender via responder"
    );
    assert_eq!(resp.result.unwrap(), json!("http://drained-url"));
}
