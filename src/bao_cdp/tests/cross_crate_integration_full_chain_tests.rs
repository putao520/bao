// @trace TEST-CDP-037 [req:REQ-CDP-001~008,REQ-CDS-001~008,REQ-LIB-001~004] [level:integration]
// Cross-crate integration: bao_cdp domain handlers → cdp-server registry → CdpRouter
// Full CDP command lifecycle exercising all 11 domains through both internal and cdp-server paths.

use bao_cdp::CdpRouter;
use bao_cdp::CdpSession;
use bao_cdp::BackendKind;
use bao_cdp::{CDPMessage, CDPResponse, CDPError, CDPEvent};
use bao_cdp::{parse_message, serialize_response, serialize_event};
use bao_cdp::{BridgeSender, BridgeReceiver, BridgeResponse, bridge_channel};
use bao_cdp::domains::ServoTargetProvider;
use cdp_server::{CdpServer, ServerConfig, DomainRegistry, EventBroadcaster, TargetInfo};
use serde_json::json;
use std::time::Duration;

// ============================================================================
// CdpRouter full lifecycle: create → enable domains → send commands → detach
// ============================================================================

#[test]
fn test_full_cdp_lifecycle_all_domains() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target-1");

    // Enable all domains
    assert!(session.send(&router, "Page.enable", None).is_ok());
    assert!(session.send(&router, "Runtime.enable", None).is_ok());
    assert!(session.send(&router, "DOM.enable", None).is_ok());
    assert!(session.send(&router, "Network.enable", None).is_ok());
    assert!(session.send(&router, "Debugger.enable", None).is_ok());
    assert!(session.send(&router, "CSS.enable", None).is_ok());
    assert!(session.send(&router, "Overlay.enable", None).is_ok());
    assert!(session.send(&router, "Log.enable", None).is_ok());
    assert!(session.send(&router, "Fetch.enable", None).is_ok());
    assert!(session.send(&router, "Input.dispatchTouchEvent", None).is_ok());
    assert!(session.send(&router, "Emulation.setTouchEmulationEnabled", None).is_ok());

    // Execute commands across all domains
    let page_result = session.send(&router, "Page.getLayoutMetrics", None).unwrap();
    assert!(page_result.get("contentSize").is_some());

    let runtime_result = session.send(&router, "Runtime.evaluate", Some(json!({"expression": "1+1"}))).is_ok();
    assert!(runtime_result);

    let dom_result = session.send(&router, "DOM.describeNode", None).unwrap();
    assert_eq!(dom_result["node"]["nodeName"], "HTML");

    let network_result = session.send(&router, "Network.getCookies", None).unwrap();
    assert!(network_result["cookies"].is_array());

    let debugger_result = session.send(&router, "Debugger.setBreakpointByUrl", Some(json!({"lineNumber": 0}))).unwrap();
    assert!(debugger_result.get("breakpointId").is_some());

    let css_result = session.send(&router, "CSS.getComputedStyleForNode", None).unwrap();
    assert!(css_result["computedStyle"].is_array());

    let fetch_result = session.send(&router, "Fetch.enable", Some(json!({"patterns": [{"urlPattern": "*"}]}))).unwrap();
    assert_eq!(fetch_result["patternCount"], 1);

    // Disable domains
    assert!(session.send(&router, "Page.disable", None).is_ok());
    assert!(session.send(&router, "Runtime.disable", None).is_ok());
    assert!(session.send(&router, "DOM.disable", None).is_ok());
    assert!(session.send(&router, "Network.disable", None).is_ok());
    assert!(session.send(&router, "Debugger.disable", None).is_ok());
    assert!(session.send(&router, "CSS.disable", None).is_ok());
    assert!(session.send(&router, "Overlay.disable", None).is_ok());
    assert!(session.send(&router, "Log.disable", None).is_ok());
    assert!(session.send(&router, "Fetch.disable", None).is_ok());

    // Detach
    let sid = session.session_id().to_string();
    assert!(session.detach(&router).is_ok());
    assert!(router.send_command(&sid, "Page.enable", None).is_err());
}

// ============================================================================
// Multiple sessions with independent domain state
// ============================================================================

#[test]
fn test_two_sessions_independent_domain_state() {
    let router = CdpRouter::new();
    std::thread::sleep(std::time::Duration::from_millis(1));
    let s1 = router.create_internal_session("target-a");
    std::thread::sleep(std::time::Duration::from_millis(1));
    let s2 = router.create_internal_session("target-b");

    // Enable different domains on each session
    assert!(s1.send(&router, "Page.enable", None).is_ok());
    assert!(s2.send(&router, "Runtime.enable", None).is_ok());

    // Each session can use its enabled domain
    assert!(s1.send(&router, "Page.getLayoutMetrics", None).is_ok());
    assert!(s2.send(&router, "Runtime.evaluate", Some(json!({"expression": "test"}))).is_ok());

    // Cross-domain usage works
    assert!(s1.send(&router, "DOM.enable", None).is_ok());
    assert!(s2.send(&router, "Page.enable", None).is_ok());

    // Both sessions remain active
    assert!(s1.send(&router, "Page.enable", None).is_ok());
    assert!(s2.send(&router, "Page.enable", None).is_ok());
}

// ============================================================================
// CdpMessage parsing + CdpRouter dispatch roundtrip
// ============================================================================

#[test]
fn test_cdp_message_parse_and_dispatch() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");

    let raw = r#"{"id":1,"method":"Page.enable","params":{}}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.id, 1);
    assert_eq!(msg.method, "Page.enable");

    let result = router.send_command(session.session_id(), &msg.method, msg.params);
    assert!(result.is_ok());
}

#[test]
fn test_cdp_message_parse_unknown_method() {
    let raw = r#"{"id":2,"method":"Unknown.method","params":{}}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.method, "Unknown.method");
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let result = router.send_command(session.session_id(), &msg.method, msg.params);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, -32601);
}

#[test]
fn test_cdp_message_parse_with_session_id() {
    let raw = r#"{"id":3,"method":"Runtime.evaluate","params":{"expression":"1+1"},"session_id":"sess-abc"}"#;
    let msg = parse_message(raw).unwrap();
    assert_eq!(msg.session_id.as_deref(), Some("sess-abc"));
}

#[test]
fn test_cdp_response_serialization() {
    let response = CDPResponse {
        id: 42,
        result: Some(json!({"frameId": "0"})),
        error: None,
    };
    let serialized = serialize_response(&response);
    let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["id"], 42);
    assert_eq!(parsed["result"]["frameId"], "0");
}

#[test]
fn test_cdp_error_response_serialization() {
    let response = CDPResponse {
        id: 1,
        result: None,
        error: Some(CDPError { code: -32601, message: "'Foo.bar' wasn't found".into() }),
    };
    let serialized = serialize_response(&response);
    let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["error"]["code"], -32601);
    assert!(parsed["error"]["message"].as_str().unwrap().contains("wasn't found"));
}

#[test]
fn test_cdp_event_serialization() {
    let event = CDPEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345})),
    };
    let serialized = serialize_event(&event);
    let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["method"], "Page.loadEventFired");
    assert_eq!(parsed["params"]["timestamp"], 12345);
}

// ============================================================================
// cdp-server CdpServer construction with bao_cdp domain registration
// ============================================================================

#[test]
fn test_cdp_server_with_domain_registration() {
    let config = ServerConfig::builder()
        .port(0)
        .host("127.0.0.1")
        .build();
    let server = CdpServer::new(config);

    assert_eq!(server.port(), 0);

    let reg = server.registry();
    assert!(!reg.has_domain("Page"));
    assert!(!reg.has_domain("Runtime"));

    // Register all bao_cdp domain handlers
    let (bridge_tx, _bridge_rx) = bridge_channel(Duration::from_millis(500));
    bao_cdp::domains::register_all_domains_into(bridge_tx, reg);

    assert!(reg.has_domain("Page"));
    assert!(reg.has_domain("Runtime"));
    assert!(reg.has_domain("DOM"));
    assert!(reg.has_domain("Network"));
    assert!(reg.has_domain("Debugger"));
    assert!(reg.has_domain("Input"));
    assert!(reg.has_domain("Emulation"));
    assert!(reg.has_domain("CSS"));
    assert!(reg.has_domain("Overlay"));
    assert!(reg.has_domain("Log"));
    assert!(reg.has_domain("Fetch"));
}

#[test]
fn test_cdp_server_ws_url_format() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(9222)
        .build();
    let server = CdpServer::new(config);
    let url = server.ws_url_for_target("abc-123");
    assert_eq!(url, "ws://0.0.0.0:9222/devtools/page/abc-123");
}

#[test]
fn test_cdp_server_broadcaster_with_domains() {
    let config = ServerConfig::default();
    let server = CdpServer::new(config);
    let b = server.broadcaster();

    let sender = b.sender();
    sender.send_event("Page.loadEventFired", json!({"timestamp": 0}));
    sender.send_event("Runtime.consoleAPICalled", json!({"type": "log", "args": []}));
    sender.send_event("DOM.documentUpdated", json!({}));
}

// ============================================================================
// TargetInfo serde roundtrip through full CDP flow
// ============================================================================

#[test]
fn test_target_info_roundtrip_through_cdp() {
    let info = TargetInfo {
        id: "tid-001".into(),
        target_type: "page".into(),
        title: "Integration Test".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/tid-001".into(),
    };

    let json_str = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed.id, info.id);
    assert_eq!(parsed.target_type, info.target_type);
    assert_eq!(parsed.title, info.title);
    assert_eq!(parsed.url, info.url);

    // Verify JSON field "type" not "target_type"
    let raw: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(raw.get("type").is_some());
    assert!(raw.get("target_type").is_none());
}

// ============================================================================
// Session error paths through router
// ============================================================================

#[test]
fn test_router_session_not_found_error() {
    let router = CdpRouter::new();
    let result = router.send_command("nonexistent-session-id", "Page.enable", None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("session not found"));
}

#[test]
fn test_router_detach_and_reaccess() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("t1");
    let sid = session.session_id().to_string();

    assert!(router.send_command(&sid, "Page.enable", None).is_ok());
    assert!(router.detach_session(&sid).is_ok());

    let result = router.send_command(&sid, "Page.getLayoutMetrics", None);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("session not found"));
}

#[test]
fn test_router_connect_external_stores_endpoint() {
    let router = CdpRouter::new();
    let result = router.connect_external("ws://localhost:9222");
    assert!(result.is_ok());
    let eb = result.unwrap();
    assert_eq!(eb.endpoint, "ws://localhost:9222");
}

// ============================================================================
// Bridge channel integration with domain handlers
// ============================================================================

#[test]
fn test_bridge_responds_to_navigate_command() {
    let (tx, rx) = bridge_channel(Duration::from_millis(500));
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let rx2 = rx.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done2 = done.clone();

    std::thread::spawn(move || {
        for _ in 0..200 {
            let processed = {
                let guard = rx2.lock().unwrap();
                guard.try_process(|cmd| {
                    match cmd {
                        bao_cdp::BridgeCommand::Navigate { url } => {
                            BridgeResponse { result: Ok(json!({"navigated": url})) }
                        }
                        _ => BridgeResponse { result: Ok(json!({})) },
                    }
                })
            };
            if processed {
                done2.store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    std::thread::sleep(std::time::Duration::from_millis(10));
    let resp = tx.send(bao_cdp::BridgeCommand::Navigate { url: "http://test.com".into() });
    assert!(resp.result.is_ok());

    for _ in 0..200 {
        if done.load(std::sync::atomic::Ordering::SeqCst) { break; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

// ============================================================================
// ServerConfig builder: full configuration verification
// ============================================================================

#[test]
fn test_server_config_full_build() {
    let config = ServerConfig::builder()
        .host("192.168.1.1")
        .port(8080)
        .http_timeout_seconds(30)
        .max_sessions(100)
        .browser_name("Bao/1.0")
        .user_agent("Bao/1.0 (Test)")
        .v8_version("SM-115")
        .webkit_version("Servo/1.0")
        .build();

    assert_eq!(config.host, "192.168.1.1");
    assert_eq!(config.port, 8080);
    assert_eq!(config.http_timeout_seconds, 30);
    assert_eq!(config.max_sessions, 100);
    assert_eq!(config.browser_name, "Bao/1.0");
    assert_eq!(config.user_agent.as_deref(), Some("Bao/1.0 (Test)"));
    assert_eq!(config.v8_version.as_deref(), Some("SM-115"));
    assert_eq!(config.webkit_version.as_deref(), Some("Servo/1.0"));
    assert_eq!(config.protocol_version, "1.3");

    let server = CdpServer::new(config);
    assert_eq!(server.port(), 8080);
    let url = server.ws_url_for_target("test");
    assert_eq!(url, "ws://192.168.1.1:8080/devtools/page/test");
}

// ============================================================================
// DomainRegistry dispatch: all 11 domains via registry
// ============================================================================

#[test]
fn test_registry_dispatch_all_known_commands() {
    let registry = DomainRegistry::new();
    let (bridge_tx, _bridge_rx) = bridge_channel(Duration::from_millis(500));
    bao_cdp::domains::register_all_domains_into(bridge_tx, &registry);

    let broadcaster = EventBroadcaster::new(std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())));
    let sender = broadcaster.sender();

    let commands = vec![
        ("Page.enable", json!({})),
        ("Runtime.enable", json!({})),
        ("DOM.describeNode", json!({})),
        ("Network.getCookies", json!({})),
        ("Debugger.enable", json!({})),
        ("Input.dispatchTouchEvent", json!({})),
        ("Emulation.clearDeviceMetricsOverride", json!({})),
        ("CSS.enable", json!({})),
        ("Overlay.enable", json!({})),
        ("Log.enable", json!({})),
        ("Fetch.disable", json!({})),
    ];

    for (method, params) in commands {
        let result = registry.dispatch_command(method, params, &*sender);
        assert!(result.is_some(), "{} should dispatch, got None", method);
        let inner = result.unwrap();
        assert!(inner.is_ok(), "{} should succeed, got {:?}", method, inner);
    }
}

#[test]
fn test_registry_dispatch_unknown_domain() {
    let registry = DomainRegistry::new();
    let (bridge_tx, _bridge_rx) = bridge_channel(Duration::from_millis(500));
    bao_cdp::domains::register_all_domains_into(bridge_tx, &registry);

    let broadcaster = EventBroadcaster::new(std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())));
    let sender = broadcaster.sender();

    let result = registry.dispatch_command("Unknown.method", json!({}), &*sender);
    assert!(result.is_none(), "Unknown domain should return None");
}

// ============================================================================
// Cross-crate: CdpRouter + cdp-server registry integration
// ============================================================================

#[test]
fn test_router_and_registry_consistent_domains() {
    // Verify CdpRouter and cdp-server DomainRegistry cover same domains
    let router = CdpRouter::new();
    let session = router.create_internal_session("consistency-target");

    let registry = DomainRegistry::new();
    let (bridge_tx, _bridge_rx) = bridge_channel(Duration::from_millis(500));
    bao_cdp::domains::register_all_domains_into(bridge_tx, &registry);

    let broadcaster = EventBroadcaster::new(std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())));
    let sender = broadcaster.sender();

    let domains = ["Page", "Runtime", "DOM", "Network", "Debugger", "CSS", "Overlay", "Log", "Fetch"];
    for domain in &domains {
        assert!(registry.has_domain(domain), "Registry missing domain {}", domain);

        let method = format!("{}.enable", domain);
        let reg_result = registry.dispatch_command(&method, json!({}), &*sender);
        assert!(reg_result.is_some(), "Registry dispatch {} returned None", method);

        let router_result = session.send(&router, &method, None);
        assert!(router_result.is_ok(), "Router dispatch {} failed: {:?}", method, router_result);
    }

    // Input and Emulation don't have .enable — verify their specific commands work
    assert!(registry.has_domain("Input"));
    assert!(registry.has_domain("Emulation"));
    let input_result = registry.dispatch_command("Input.dispatchMouseEvent", json!({"type": "mousePressed", "x": 0, "y": 0}), &*sender);
    assert!(input_result.is_some());
    let emul_result = registry.dispatch_command("Emulation.setDeviceMetricsOverride", json!({"width": 1920, "height": 1080}), &*sender);
    assert!(emul_result.is_some());
}

#[test]
fn test_backend_kind_internal_on_new_session() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("bk-test");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

#[test]
fn test_session_target_id_preserved() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("my-target-123");
    assert_eq!(session.target_id(), "my-target-123");
    assert!(!session.session_id().is_empty());
    assert_ne!(session.session_id(), "my-target-123");
}
