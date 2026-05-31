// @trace TEST-CDP-032 [req:REQ-CDP-003,REQ-CDP-007,REQ-CDP-008] [level:unit]
// DebuggerHandler, InputHandler, EmulationHandler, CssHandler, OverlayHandler,
// LogHandler, FetchHandler, ServoTargetProvider command routing and field verification.

use bao_cdp::domains::{
    PageHandler, RuntimeHandler, DomHandler, NetworkHandler,
};
use bao_cdp::{BridgeCommand, BridgeResponse, BridgeSender, BridgeReceiver, bridge_channel};
use cdp_server::{CdpError, DomainHandler, EventSender, TargetProvider};
use serde_json::{json, Value};
use std::time::Duration;

// Access stub types via register_all_domains_into (they're private modules)
// We test them through DomainRegistry dispatch.

struct NoopSender;
impl EventSender for NoopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}
static NOOP: NoopSender = NoopSender;
fn noop_es() -> &'static dyn EventSender { &NOOP }

fn bridge(timeout_ms: u64) -> (BridgeSender, BridgeReceiver) {
    bridge_channel(Duration::from_millis(timeout_ms))
}

// Helper: dispatch_command returns Option<Result>, flatten to Result for ergonomics
fn dispatch_cmd(registry: &cdp_server::DomainRegistry, cmd: &str, params: Value) -> Result<Value, CdpError> {
    registry.dispatch_command(cmd, params, noop_es())
        .unwrap_or(Err(CdpError { code: -32601, message: format!("Domain not found for '{}'", cmd) }))
}

// Helper: spawn thread to process one bridge command
fn spawn_bridge_ok(rx: BridgeReceiver, check: fn(BridgeCommand) -> bool) -> std::sync::Arc<std::sync::Mutex<BridgeReceiver>> {
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        let rx = rx2.lock().unwrap();
        rx.try_process(|cmd| {
            assert!(check(cmd), "bridge command check failed");
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    rx
}

// ============================================================================
// Test via DomainRegistry: register all domains and dispatch commands
// ============================================================================

fn setup_registry() -> (BridgeSender, cdp_server::DomainRegistry) {
    let (tx, _rx) = bridge(50);
    let registry = cdp_server::DomainRegistry::new();
    bao_cdp::domains::register_all_domains_into(tx.clone(), &registry);
    (tx, registry)
}

// ---- DebuggerHandler ----

#[test]
fn test_debugger_domain_name() {
    let (tx, _) = bridge(50);
    let registry = cdp_server::DomainRegistry::new();
    bao_cdp::domains::register_all_domains_into(tx, &registry);
    assert!(registry.has_domain("Debugger"));
}

#[test]
fn test_debugger_enable_disable() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Debugger.enable", json!({})).is_ok());
    assert!(dispatch_cmd(&registry,"Debugger.disable", json!({})).is_ok());
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Debugger.setBreakpointByUrl", json!({})).unwrap();
    assert_eq!(result["breakpointId"], "1");
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_remove_breakpoint() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Debugger.removeBreakpoint", json!({})).is_ok());
}

#[test]
fn test_debugger_pause_resume() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Debugger.pause", json!({})).is_ok());
    assert!(dispatch_cmd(&registry,"Debugger.resume", json!({})).is_ok());
}

#[test]
fn test_debugger_step_over_in_out() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Debugger.stepOver", json!({})).is_ok());
    assert!(dispatch_cmd(&registry,"Debugger.stepInto", json!({})).is_ok());
    assert!(dispatch_cmd(&registry,"Debugger.stepOut", json!({})).is_ok());
}

#[test]
fn test_debugger_set_skip_all_pauses() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Debugger.setSkipAllPauses", json!({})).is_ok());
}

#[test]
fn test_debugger_set_breakpoints_active() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Debugger.setBreakpointsActive", json!({})).is_ok());
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Debugger.evaluateOnCallFrame", json!({})).unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Debugger.getPossibleBreakpoints", json!({})).unwrap();
    assert!(result["locations"].is_array());
}

#[test]
fn test_debugger_get_script_source() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Debugger.getScriptSource", json!({})).unwrap();
    assert!(result["scriptSource"].is_string());
}

#[test]
fn test_debugger_set_pause_on_exceptions() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Debugger.setPauseOnExceptions", json!({})).is_ok());
}

#[test]
fn test_debugger_unknown() {
    let (_, registry) = setup_registry();
    let err = dispatch_cmd(&registry,"Debugger.fakeCommand", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ---- InputHandler ----

#[test]
fn test_input_domain_name() {
    let (_, registry) = setup_registry();
    assert!(registry.has_domain("Input"));
}

#[test]
fn test_input_dispatch_mouse_event() {
    let (tx, rx) = bridge(50);
    let registry = cdp_server::DomainRegistry::new();
    bao_cdp::domains::register_all_domains_into(tx, &registry);

    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        let rx = rx2.lock().unwrap();
        rx.try_process(|cmd| {
            if let BridgeCommand::DispatchMouseEvent { event_type, x, y, button, click_count } = cmd {
                assert_eq!(event_type, "mousePressed");
                assert!((x - 100.0).abs() < f64::EPSILON);
                assert!((y - 200.0).abs() < f64::EPSILON);
                assert_eq!(button, Some(0));
                assert_eq!(click_count, Some(1));
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let result = dispatch_cmd(&registry,"Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": 100, "y": 200, "button": 0, "clickCount": 1
    }));
    assert!(result.is_ok());
}

#[test]
fn test_input_dispatch_key_event() {
    let (tx, rx) = bridge(50);
    let registry = cdp_server::DomainRegistry::new();
    bao_cdp::domains::register_all_domains_into(tx, &registry);

    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        let rx = rx2.lock().unwrap();
        rx.try_process(|cmd| {
            if let BridgeCommand::DispatchKeyEvent { event_type, key, code, text } = cmd {
                assert_eq!(event_type, "keyDown");
                assert_eq!(key, "Enter");
                assert_eq!(code, "Enter");
                assert!(text.is_none());
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let result = dispatch_cmd(&registry,"Input.dispatchKeyEvent", json!({
        "type": "keyDown", "key": "Enter", "code": "Enter"
    }));
    assert!(result.is_ok());
}

#[test]
fn test_input_dispatch_touch_event() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Input.dispatchTouchEvent", json!({})).is_ok());
}

#[test]
fn test_input_insert_text() {
    let (tx, rx) = bridge(50);
    let registry = cdp_server::DomainRegistry::new();
    bao_cdp::domains::register_all_domains_into(tx, &registry);

    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        let rx = rx2.lock().unwrap();
        rx.try_process(|cmd| {
            if let BridgeCommand::InsertText { text } = cmd {
                assert_eq!(text, "hello world");
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let result = dispatch_cmd(&registry,"Input.insertText", json!({"text": "hello world"}));
    assert!(result.is_ok());
}

#[test]
fn test_input_insert_text_empty_ok() {
    let (_, registry) = setup_registry();
    // Empty text should return Ok without sending bridge command
    let result = dispatch_cmd(&registry,"Input.insertText", json!({"text": ""}));
    assert!(result.is_ok());
}

#[test]
fn test_input_insert_text_no_param_ok() {
    let (_, registry) = setup_registry();
    // Missing text param defaults to empty string → Ok
    let result = dispatch_cmd(&registry,"Input.insertText", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_input_set_ignore_input_events() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Input.setIgnoreInputEvents", json!({})).is_ok());
}

#[test]
fn test_input_set_intercept_drags() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Input.setInterceptDrags", json!({})).is_ok());
}

#[test]
fn test_input_unknown() {
    let (_, registry) = setup_registry();
    let err = dispatch_cmd(&registry,"Input.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ---- EmulationHandler ----

#[test]
fn test_emulation_domain_name() {
    let (_, registry) = setup_registry();
    assert!(registry.has_domain("Emulation"));
}

#[test]
fn test_emulation_set_device_metrics_override() {
    let (tx, rx) = bridge(50);
    let registry = cdp_server::DomainRegistry::new();
    bao_cdp::domains::register_all_domains_into(tx, &registry);

    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        let rx = rx2.lock().unwrap();
        rx.try_process(|cmd| {
            if let BridgeCommand::SetViewport { width, height, device_scale_factor } = cmd {
                assert_eq!(width, 1280);
                assert_eq!(height, 720);
                assert_eq!(device_scale_factor, Some(2.0));
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let result = dispatch_cmd(&registry,"Emulation.setDeviceMetricsOverride", json!({
        "width": 1280, "height": 720, "deviceScaleFactor": 2.0
    }));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_set_device_metrics_defaults() {
    let (tx, rx) = bridge(50);
    let registry = cdp_server::DomainRegistry::new();
    bao_cdp::domains::register_all_domains_into(tx, &registry);

    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        let rx = rx2.lock().unwrap();
        rx.try_process(|cmd| {
            if let BridgeCommand::SetViewport { width, height, device_scale_factor } = cmd {
                assert_eq!(width, 1920);  // default
                assert_eq!(height, 1080); // default
                assert!(device_scale_factor.is_none());
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let result = dispatch_cmd(&registry,"Emulation.setDeviceMetricsOverride", json!({}));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_clear_device_metrics() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Emulation.clearDeviceMetricsOverride", json!({})).is_ok());
}

#[test]
fn test_emulation_set_user_agent_override() {
    let (tx, rx) = bridge(50);
    let registry = cdp_server::DomainRegistry::new();
    bao_cdp::domains::register_all_domains_into(tx, &registry);

    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    std::thread::spawn(move || {
        let rx = rx2.lock().unwrap();
        rx.try_process(|cmd| {
            if let BridgeCommand::SetUserAgent { user_agent } = cmd {
                assert_eq!(user_agent, "TestBot/1.0");
            }
            BridgeResponse { result: Ok(json!({})) }
        });
    });
    let result = dispatch_cmd(&registry,"Emulation.setUserAgentOverride", json!({
        "userAgent": "TestBot/1.0"
    }));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_set_user_agent_empty_ok() {
    let (_, registry) = setup_registry();
    // Empty UA should return Ok without bridge
    let result = dispatch_cmd(&registry,"Emulation.setUserAgentOverride", json!({"userAgent": ""}));
    assert!(result.is_ok());
}

#[test]
fn test_emulation_set_touch_emulation() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Emulation.setTouchEmulationEnabled", json!({})).is_ok());
}

#[test]
fn test_emulation_set_script_execution_disabled() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Emulation.setScriptExecutionDisabled", json!({})).is_ok());
}

#[test]
fn test_emulation_set_focus_emulation() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Emulation.setFocusEmulationEnabled", json!({})).is_ok());
}

#[test]
fn test_emulation_set_cpu_throttling_rate() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Emulation.setCPUThrottlingRate", json!({})).is_ok());
}

#[test]
fn test_emulation_set_default_background_color() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Emulation.setDefaultBackgroundColorOverride", json!({})).is_ok());
}

#[test]
fn test_emulation_unknown() {
    let (_, registry) = setup_registry();
    let err = dispatch_cmd(&registry,"Emulation.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ---- CssHandler (stub) ----

#[test]
fn test_css_domain_name() {
    let (_, registry) = setup_registry();
    assert!(registry.has_domain("CSS"));
}

#[test]
fn test_css_enable_disable() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"CSS.enable", json!({})).is_ok());
    assert!(dispatch_cmd(&registry,"CSS.disable", json!({})).is_ok());
}

#[test]
fn test_css_get_computed_style() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"CSS.getComputedStyleForNode", json!({})).unwrap();
    assert!(result["computedStyle"].is_array());
}

#[test]
fn test_css_get_matched_styles() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"CSS.getMatchedStylesForNode", json!({})).unwrap();
    assert!(result["matchedCSSRules"].is_array());
    assert!(result["inlineStyle"].is_null());
    assert!(result["attributesStyle"].is_null());
}

#[test]
fn test_css_get_inline_styles() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"CSS.getInlineStylesForNode", json!({})).unwrap();
    assert!(result["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"CSS.setStyleTexts", json!({})).unwrap();
    assert!(result["styles"].is_array());
}

#[test]
fn test_css_unknown() {
    let (_, registry) = setup_registry();
    let err = dispatch_cmd(&registry,"CSS.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ---- OverlayHandler (stub) ----

#[test]
fn test_overlay_domain_name() {
    let (_, registry) = setup_registry();
    assert!(registry.has_domain("Overlay"));
}

#[test]
fn test_overlay_enable_disable() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Overlay.enable", json!({})).is_ok());
    assert!(dispatch_cmd(&registry,"Overlay.disable", json!({})).is_ok());
}

#[test]
fn test_overlay_highlight_node() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Overlay.highlightNode", json!({})).is_ok());
}

#[test]
fn test_overlay_hide_highlight() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Overlay.hideHighlight", json!({})).is_ok());
}

#[test]
fn test_overlay_set_inspect_mode() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Overlay.setInspectMode", json!({})).is_ok());
}

#[test]
fn test_overlay_set_paused_in_debugger() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Overlay.setPausedInDebuggerMessage", json!({})).is_ok());
}

#[test]
fn test_overlay_unknown() {
    let (_, registry) = setup_registry();
    let err = dispatch_cmd(&registry,"Overlay.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ---- LogHandler (stub) ----

#[test]
fn test_log_domain_name() {
    let (_, registry) = setup_registry();
    assert!(registry.has_domain("Log"));
}

#[test]
fn test_log_enable_disable() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Log.enable", json!({})).is_ok());
    assert!(dispatch_cmd(&registry,"Log.disable", json!({})).is_ok());
}

#[test]
fn test_log_clear() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Log.clear", json!({})).is_ok());
}

#[test]
fn test_log_start_violations_report() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Log.startViolationsReport", json!({})).is_ok());
}

#[test]
fn test_log_stop_violations_report() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Log.stopViolationsReport", json!({})).is_ok());
}

#[test]
fn test_log_unknown() {
    let (_, registry) = setup_registry();
    let err = dispatch_cmd(&registry,"Log.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ---- FetchHandler (stub) ----

#[test]
fn test_fetch_domain_name() {
    let (_, registry) = setup_registry();
    assert!(registry.has_domain("Fetch"));
}

#[test]
fn test_fetch_enable() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.enable", json!({})).unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 0);
}

#[test]
fn test_fetch_enable_with_patterns() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.enable", json!({
        "patterns": [{"urlPattern": "*"}, {"urlPattern": "/api/*"}]
    })).unwrap();
    assert_eq!(result["patternCount"], 2);
}

#[test]
fn test_fetch_disable() {
    let (_, registry) = setup_registry();
    assert!(dispatch_cmd(&registry,"Fetch.disable", json!({})).is_ok());
}

#[test]
fn test_fetch_continue_request() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.continueRequest", json!({"requestId": "r1"})).unwrap();
    assert_eq!(result["requestId"], "r1");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_continue_with_response() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.continueWithResponse", json!({"requestId": "r2"})).unwrap();
    assert_eq!(result["requestId"], "r2");
}

#[test]
fn test_fetch_fail_request() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.failRequest", json!({"requestId": "r3", "reason": "Aborted"})).unwrap();
    assert_eq!(result["requestId"], "r3");
    assert_eq!(result["failed"], true);
    assert_eq!(result["reason"], "Aborted");
}

#[test]
fn test_fetch_fulfill_request() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.fulfillRequest", json!({
        "requestId": "r4", "responseCode": 200, "body": "hello"
    })).unwrap();
    assert_eq!(result["requestId"], "r4");
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 5);
}

#[test]
fn test_fulfill_request_defaults() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.fulfillRequest", json!({"requestId": "r5"})).unwrap();
    assert_eq!(result["responseCode"], 200); // default
    assert_eq!(result["bodyLength"], 0); // empty body
}

#[test]
fn test_fetch_get_request_post_data() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.getRequestPostData", json!({"requestId": "r6"})).unwrap();
    assert_eq!(result["requestId"], "r6");
    assert_eq!(result["postData"], "");
}

#[test]
fn test_fetch_continue_with_auth() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.continueWithAuth", json!({"requestId": "r7"})).unwrap();
    assert_eq!(result["requestId"], "r7");
}

#[test]
fn test_fetch_take_response_body_as_stream() {
    let (_, registry) = setup_registry();
    let result = dispatch_cmd(&registry,"Fetch.takeResponseBodyAsStream", json!({"requestId": "r8"})).unwrap();
    assert_eq!(result["stream"], "stream-r8");
}

#[test]
fn test_fetch_unknown() {
    let (_, registry) = setup_registry();
    let err = dispatch_cmd(&registry,"Fetch.nonexistent", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}

// ---- ServoTargetProvider ----

#[test]
fn test_target_list_targets() {
    let (tx, rx) = bridge(500);
    let provider = bao_cdp::domains::ServoTargetProvider::new(tx, "127.0.0.1".into(), 9222);

    // list_targets sends 2 bridge.send() calls sequentially (GetTitle, GetUrl).
    // Each send() blocks waiting for response, so we need a thread that processes
    // them one at a time with retry loops.
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done2 = done.clone();
    std::thread::spawn(move || {
        let mut processed = 0;
        while processed < 2 && !done2.load(std::sync::atomic::Ordering::Relaxed) {
            let got = rx.try_process(|cmd| {
                match cmd {
                    BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Test Page")) },
                    BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!("https://example.com")) },
                    _ => BridgeResponse { result: Ok(json!("")) },
                }
            });
            if got { processed += 1; }
            if !got { std::thread::sleep(std::time::Duration::from_millis(1)); }
        }
    });

    let targets = provider.list_targets();
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(targets.len(), 1);
    let t = &targets[0];
    assert_eq!(t.target_type, "page");
    assert_eq!(t.title, "Test Page");
    assert_eq!(t.url, "https://example.com");
    assert!(t.web_socket_debugger_url.contains("ws://127.0.0.1:9222"));
    assert!(!t.id.is_empty());
}

#[test]
fn test_target_create_target_returns_existing() {
    let (tx, rx) = bridge(500);
    let provider = bao_cdp::domains::ServoTargetProvider::new(tx, "0.0.0.0".into(), 8080);

    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done2 = done.clone();
    std::thread::spawn(move || {
        let mut processed = 0;
        while processed < 2 && !done2.load(std::sync::atomic::Ordering::Relaxed) {
            let got = rx.try_process(|cmd| {
                match cmd {
                    BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Title")) },
                    BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!("https://test.com")) },
                    _ => BridgeResponse { result: Ok(json!("")) },
                }
            });
            if got { processed += 1; }
            if !got { std::thread::sleep(std::time::Duration::from_millis(1)); }
        }
    });

    let result = provider.create_target("https://new.com");
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(result.is_ok());
    let info = result.unwrap();
    assert_eq!(info.target_type, "page");
}

#[test]
fn test_target_close_target() {
    let (tx, _rx) = bridge(50);
    let provider = bao_cdp::domains::ServoTargetProvider::new(tx, "localhost".into(), 3000);
    // close_target sends fire-and-forget
    let result = provider.close_target("any-id");
    assert!(result.is_ok());
}

#[test]
fn test_target_activate_target() {
    let (tx, _rx) = bridge(50);
    let provider = bao_cdp::domains::ServoTargetProvider::new(tx, "localhost".into(), 3000);
    let result = provider.activate_target("any-id");
    assert!(result.is_ok());
}

// ---- All domains registered ----

#[test]
fn test_all_domains_registered() {
    let (_, registry) = setup_registry();
    let expected = ["Page", "Runtime", "DOM", "Network", "Debugger", "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch"];
    for domain in &expected {
        assert!(registry.has_domain(domain), "Missing domain: {}", domain);
    }
}

#[test]
fn test_unknown_domain_returns_error() {
    let (_, registry) = setup_registry();
    let err = dispatch_cmd(&registry,"Unknown.method", json!({})).unwrap_err();
    assert_eq!(err.code, -32601);
}
