// @trace TEST-CDP-030 [req:REQ-CDP-001,REQ-CDP-003] [level:unit]
// ServerConfig builder, DomainRegistry register/has_domain, CdpRouter accessors,
// BackendKind exhaustive, BridgeCommand all variants, BridgeResponse,
// bridge channel send/drain/fire-and-forget/is_alive.

use bao_cdp::{CdpRouter, BackendKind, BridgeCommand, BridgeResponse, bridge_channel};
use std::time::Duration;

// ---- ServerConfig builder ----

#[test]
fn test_server_config_default_values() {
    let cfg = cdp_server::ServerConfig::default();
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.port, 9222);
    assert_eq!(cfg.http_timeout_seconds, 30);
    assert_eq!(cfg.max_sessions, 100);
    assert_eq!(cfg.browser_name, "Bao/0.1.0");
    assert_eq!(cfg.protocol_version, "1.3");
    assert!(cfg.user_agent.is_none());
    assert!(cfg.v8_version.is_none());
    assert!(cfg.webkit_version.is_none());
}

#[test]
fn test_server_config_builder_custom() {
    let cfg = cdp_server::ServerConfig::builder()
        .host("0.0.0.0")
        .port(8080)
        .http_timeout_seconds(60)
        .max_sessions(50)
        .browser_name("CustomBrowser/1.0")
        .user_agent("CustomUA")
        .v8_version("12.0")
        .webkit_version("600.0")
        .build();
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.http_timeout_seconds, 60);
    assert_eq!(cfg.max_sessions, 50);
    assert_eq!(cfg.browser_name, "CustomBrowser/1.0");
    assert_eq!(cfg.user_agent.as_deref(), Some("CustomUA"));
    assert_eq!(cfg.v8_version.as_deref(), Some("12.0"));
    assert_eq!(cfg.webkit_version.as_deref(), Some("600.0"));
}

#[test]
fn test_server_config_builder_minimal() {
    let cfg = cdp_server::ServerConfig::builder().build();
    assert_eq!(cfg.port, 9222);
    assert_eq!(cfg.host, "127.0.0.1");
}

#[test]
fn test_server_config_builder_port_only() {
    let cfg = cdp_server::ServerConfig::builder().port(3000).build();
    assert_eq!(cfg.port, 3000);
    assert_eq!(cfg.host, "127.0.0.1");
}

#[test]
fn test_server_config_builder_chained() {
    let cfg = cdp_server::ServerConfig::builder()
        .host("192.168.1.1")
        .port(9999)
        .http_timeout_seconds(10)
        .max_sessions(5)
        .browser_name("TestBot")
        .build();
    assert_eq!(cfg.host, "192.168.1.1");
    assert_eq!(cfg.port, 9999);
    assert_eq!(cfg.http_timeout_seconds, 10);
    assert_eq!(cfg.max_sessions, 5);
}

// ---- DomainRegistry ----

#[test]
fn test_domain_registry_new_empty() {
    let reg = cdp_server::DomainRegistry::new();
    assert!(!reg.has_domain("Page"));
    assert!(!reg.has_domain("Runtime"));
    assert!(!reg.has_domain("DOM"));
}

#[test]
fn test_domain_registry_register_and_has_domain() {
    use cdp_server::DomainHandler;
    use serde_json::Value;

    struct TestHandler;
    impl DomainHandler for TestHandler {
        fn domain_name(&self) -> &'static str { "TestDomain" }
        fn handle_command(&self, _: &str, _: Value, _: &dyn cdp_server::EventSender) -> Result<Value, cdp_server::CdpError> {
            Ok(Value::Null)
        }
    }

    let reg = cdp_server::DomainRegistry::new();
    assert!(!reg.has_domain("TestDomain"));
    reg.register(Box::new(TestHandler));
    assert!(reg.has_domain("TestDomain"));
    assert!(!reg.has_domain("OtherDomain"));
}

#[test]
fn test_domain_registry_multiple_domains() {
    use cdp_server::DomainHandler;
    use serde_json::Value;

    struct DomainA;
    struct DomainB;
    impl DomainHandler for DomainA {
        fn domain_name(&self) -> &'static str { "A" }
        fn handle_command(&self, _: &str, _: Value, _: &dyn cdp_server::EventSender) -> Result<Value, cdp_server::CdpError> { Ok(Value::Null) }
    }
    impl DomainHandler for DomainB {
        fn domain_name(&self) -> &'static str { "B" }
        fn handle_command(&self, _: &str, _: Value, _: &dyn cdp_server::EventSender) -> Result<Value, cdp_server::CdpError> { Ok(Value::Null) }
    }

    let reg = cdp_server::DomainRegistry::new();
    reg.register(Box::new(DomainA));
    reg.register(Box::new(DomainB));
    assert!(reg.has_domain("A"));
    assert!(reg.has_domain("B"));
    assert!(!reg.has_domain("C"));
}

// ---- CdpRouter ----

#[test]
fn test_router_new_default_equivalence() {
    let r1 = CdpRouter::new();
    let r2 = CdpRouter::default();
    let s1 = r1.create_internal_session("t1");
    let s2 = r2.create_internal_session("t2");
    assert_ne!(s1.session_id(), s2.session_id());
}

#[test]
fn test_router_default_creates_sessions() {
    let router = CdpRouter::default();
    let session = router.create_internal_session("test");
    assert_eq!(session.target_id(), "test");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

#[test]
fn test_router_sessions_unique_ids() {
    let router = CdpRouter::new();
    let mut ids = std::collections::HashSet::new();
    for i in 0..30 {
        let s = router.create_internal_session(&format!("t{}", i));
        assert!(ids.insert(s.session_id().to_string()), "Duplicate session ID");
    }
    assert_eq!(ids.len(), 30);
}

// ---- BackendKind ----

#[test]
fn test_backend_kind_equality() {
    assert_eq!(BackendKind::Internal, BackendKind::Internal);
    assert_eq!(BackendKind::External, BackendKind::External);
    assert_ne!(BackendKind::Internal, BackendKind::External);
}

#[test]
fn test_backend_kind_copy() {
    let k = BackendKind::Internal;
    let k2 = k;
    assert_eq!(k, k2);
}

#[test]
fn test_backend_kind_debug() {
    assert!(format!("{:?}", BackendKind::Internal).contains("Internal"));
    assert!(format!("{:?}", BackendKind::External).contains("External"));
}

// ---- BridgeCommand all variants ----

#[test]
fn test_bridge_command_navigate() {
    let cmd = BridgeCommand::Navigate { url: "https://example.com".into() };
    if let BridgeCommand::Navigate { url } = cmd {
        assert_eq!(url, "https://example.com");
    } else { panic!("Expected Navigate"); }
}

#[test]
fn test_bridge_command_evaluate_js() {
    let cmd = BridgeCommand::EvaluateJs { expression: "1+1".into(), return_by_value: true };
    if let BridgeCommand::EvaluateJs { expression, return_by_value } = cmd {
        assert_eq!(expression, "1+1");
        assert!(return_by_value);
    } else { panic!("Expected EvaluateJs"); }
}

#[test]
fn test_bridge_command_take_screenshot() {
    let cmd = BridgeCommand::TakeScreenshot { format: "png".into(), quality: Some(80) };
    if let BridgeCommand::TakeScreenshot { format, quality } = cmd {
        assert_eq!(format, "png");
        assert_eq!(quality, Some(80));
    } else { panic!("Expected TakeScreenshot"); }
}

#[test]
fn test_bridge_command_take_screenshot_no_quality() {
    let cmd = BridgeCommand::TakeScreenshot { format: "jpeg".into(), quality: None };
    if let BridgeCommand::TakeScreenshot { quality, .. } = cmd {
        assert!(quality.is_none());
    } else { panic!("Expected TakeScreenshot"); }
}

#[test]
fn test_bridge_command_get_title() {
    assert!(matches!(BridgeCommand::GetTitle, BridgeCommand::GetTitle));
}

#[test]
fn test_bridge_command_get_url() {
    assert!(matches!(BridgeCommand::GetUrl, BridgeCommand::GetUrl));
}

#[test]
fn test_bridge_command_get_document() {
    assert!(matches!(BridgeCommand::GetDocument, BridgeCommand::GetDocument));
}

#[test]
fn test_bridge_command_query_selector() {
    let cmd = BridgeCommand::QuerySelector { selector: "div".into() };
    if let BridgeCommand::QuerySelector { selector } = cmd {
        assert_eq!(selector, "div");
    } else { panic!("Expected QuerySelector"); }
}

#[test]
fn test_bridge_command_query_selector_all() {
    let cmd = BridgeCommand::QuerySelectorAll { selector: "a".into() };
    if let BridgeCommand::QuerySelectorAll { selector } = cmd {
        assert_eq!(selector, "a");
    } else { panic!("Expected QuerySelectorAll"); }
}

#[test]
fn test_bridge_command_get_outer_html() {
    let cmd = BridgeCommand::GetOuterHtml { node_id: Some(42) };
    if let BridgeCommand::GetOuterHtml { node_id } = cmd {
        assert_eq!(node_id, Some(42));
    } else { panic!("Expected GetOuterHtml"); }
}

#[test]
fn test_bridge_command_get_outer_html_no_node() {
    let cmd = BridgeCommand::GetOuterHtml { node_id: None };
    if let BridgeCommand::GetOuterHtml { node_id } = cmd {
        assert!(node_id.is_none());
    } else { panic!("Expected GetOuterHtml"); }
}

#[test]
fn test_bridge_command_set_attribute_value() {
    let cmd = BridgeCommand::SetAttributeValue { node_id: 5, name: "class".into(), value: "active".into() };
    if let BridgeCommand::SetAttributeValue { node_id, name, value } = cmd {
        assert_eq!(node_id, 5);
        assert_eq!(name, "class");
        assert_eq!(value, "active");
    } else { panic!("Expected SetAttributeValue"); }
}

#[test]
fn test_bridge_command_dispatch_mouse_event() {
    let cmd = BridgeCommand::DispatchMouseEvent {
        event_type: "mousePressed".into(), x: 100.0, y: 200.0,
        button: Some(0), click_count: Some(1),
    };
    if let BridgeCommand::DispatchMouseEvent { event_type, x, y, button, click_count } = cmd {
        assert_eq!(event_type, "mousePressed");
        assert!((x - 100.0).abs() < f64::EPSILON);
        assert!((y - 200.0).abs() < f64::EPSILON);
        assert_eq!(button, Some(0));
        assert_eq!(click_count, Some(1));
    } else { panic!("Expected DispatchMouseEvent"); }
}

#[test]
fn test_bridge_command_dispatch_key_event() {
    let cmd = BridgeCommand::DispatchKeyEvent {
        event_type: "keyDown".into(), key: "Enter".into(), code: "Enter".into(), text: None,
    };
    if let BridgeCommand::DispatchKeyEvent { event_type, key, code, text } = cmd {
        assert_eq!(event_type, "keyDown");
        assert_eq!(key, "Enter");
        assert_eq!(code, "Enter");
        assert!(text.is_none());
    } else { panic!("Expected DispatchKeyEvent"); }
}

#[test]
fn test_bridge_command_insert_text() {
    let cmd = BridgeCommand::InsertText { text: "hello".into() };
    if let BridgeCommand::InsertText { text } = cmd {
        assert_eq!(text, "hello");
    } else { panic!("Expected InsertText"); }
}

#[test]
fn test_bridge_command_set_viewport() {
    let cmd = BridgeCommand::SetViewport { width: 1920, height: 1080, device_scale_factor: Some(2.0) };
    if let BridgeCommand::SetViewport { width, height, device_scale_factor } = cmd {
        assert_eq!(width, 1920);
        assert_eq!(height, 1080);
        assert_eq!(device_scale_factor, Some(2.0));
    } else { panic!("Expected SetViewport"); }
}

#[test]
fn test_bridge_command_set_viewport_no_dpr() {
    let cmd = BridgeCommand::SetViewport { width: 800, height: 600, device_scale_factor: None };
    if let BridgeCommand::SetViewport { device_scale_factor, .. } = cmd {
        assert!(device_scale_factor.is_none());
    } else { panic!("Expected SetViewport"); }
}

#[test]
fn test_bridge_command_set_user_agent() {
    let cmd = BridgeCommand::SetUserAgent { user_agent: "TestBot/1.0".into() };
    if let BridgeCommand::SetUserAgent { user_agent } = cmd {
        assert_eq!(user_agent, "TestBot/1.0");
    } else { panic!("Expected SetUserAgent"); }
}

#[test]
fn test_bridge_command_get_cookies() {
    let cmd = BridgeCommand::GetCookies { urls: vec!["https://a.com".into()] };
    if let BridgeCommand::GetCookies { urls } = cmd {
        assert_eq!(urls.len(), 1);
    } else { panic!("Expected GetCookies"); }
}

#[test]
fn test_bridge_command_get_all_cookies() {
    assert!(matches!(BridgeCommand::GetAllCookies, BridgeCommand::GetAllCookies));
}

#[test]
fn test_bridge_command_delete_cookie() {
    let cmd = BridgeCommand::DeleteCookie { name: "session".into(), url: Some("https://x.com".into()) };
    if let BridgeCommand::DeleteCookie { name, url } = cmd {
        assert_eq!(name, "session");
        assert_eq!(url, Some("https://x.com".into()));
    } else { panic!("Expected DeleteCookie"); }
}

#[test]
fn test_bridge_command_set_cookie() {
    let cmd = BridgeCommand::SetCookie {
        name: "sid".into(), value: "abc".into(),
        url: Some("https://example.com".into()), domain: Some("example.com".into()),
    };
    if let BridgeCommand::SetCookie { name, value, url, domain } = cmd {
        assert_eq!(name, "sid");
        assert_eq!(value, "abc");
        assert_eq!(url, Some("https://example.com".into()));
        assert_eq!(domain, Some("example.com".into()));
    } else { panic!("Expected SetCookie"); }
}

#[test]
fn test_bridge_command_get_response_body() {
    let cmd = BridgeCommand::GetResponseBody { request_id: "req-123".into() };
    if let BridgeCommand::GetResponseBody { request_id } = cmd {
        assert_eq!(request_id, "req-123");
    } else { panic!("Expected GetResponseBody"); }
}

#[test]
fn test_bridge_command_add_script() {
    let cmd = BridgeCommand::AddScriptToEvaluateOnNewDocument { source: "console.log(1)".into() };
    if let BridgeCommand::AddScriptToEvaluateOnNewDocument { source } = cmd {
        assert_eq!(source, "console.log(1)");
    } else { panic!("Expected AddScriptToEvaluateOnNewDocument"); }
}

#[test]
fn test_bridge_command_reload() {
    let cmd = BridgeCommand::Reload { ignore_cache: true };
    if let BridgeCommand::Reload { ignore_cache } = cmd {
        assert!(ignore_cache);
    } else { panic!("Expected Reload"); }
}

#[test]
fn test_bridge_command_go_back() {
    assert!(matches!(BridgeCommand::GoBack, BridgeCommand::GoBack));
}

#[test]
fn test_bridge_command_go_forward() {
    assert!(matches!(BridgeCommand::GoForward, BridgeCommand::GoForward));
}

#[test]
fn test_bridge_command_stop_loading() {
    assert!(matches!(BridgeCommand::StopLoading, BridgeCommand::StopLoading));
}

#[test]
fn test_bridge_command_close_page() {
    assert!(matches!(BridgeCommand::ClosePage, BridgeCommand::ClosePage));
}

// ---- BridgeCommand Debug ----

#[test]
fn test_bridge_command_debug_navigate() {
    let cmd = BridgeCommand::Navigate { url: "https://test.com".into() };
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("Navigate"));
    assert!(debug.contains("https://test.com"));
}

#[test]
fn test_bridge_command_debug_get_title() {
    let debug = format!("{:?}", BridgeCommand::GetTitle);
    assert!(debug.contains("GetTitle"));
}

// ---- BridgeResponse ----

#[test]
fn test_bridge_response_ok() {
    let resp = BridgeResponse { result: Ok(serde_json::json!({"status": "ok"})) };
    assert!(resp.result.is_ok());
    assert_eq!(resp.result.unwrap()["status"], "ok");
}

#[test]
fn test_bridge_response_err() {
    let resp = BridgeResponse { result: Err("something failed".into()) };
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "something failed");
}

#[test]
fn test_bridge_response_null_ok() {
    let resp = BridgeResponse { result: Ok(serde_json::Value::Null) };
    assert!(resp.result.is_ok());
}

#[test]
fn test_bridge_response_debug() {
    let resp = BridgeResponse { result: Ok(serde_json::json!(42)) };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("BridgeResponse"));
}

// ---- Bridge channel ----

#[test]
fn test_bridge_channel_send_and_drain() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx.send_fire_and_forget(BridgeCommand::GetUrl);
    let count = std::sync::atomic::AtomicUsize::new(0);
    rx.drain(|_cmd| {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        BridgeResponse { result: Ok(serde_json::Value::Null) }
    });
    assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 2);
}

#[test]
fn test_bridge_channel_sender_clone() {
    let (tx, _rx) = bridge_channel(Duration::from_secs(5));
    let tx2 = tx.clone();
    // Both senders can fire-and-forget
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    tx2.send_fire_and_forget(BridgeCommand::GetUrl);
}

#[test]
fn test_bridge_channel_not_alive_after_drop() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    drop(rx);
    // is_alive sends a test message — it returns false when channel closed
    assert!(!tx.is_alive());
}

#[test]
fn test_bridge_channel_drain_empty() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let count = rx.drain(|_| BridgeResponse { result: Ok(serde_json::Value::Null) });
    assert_eq!(count, 0);
}

#[test]
fn test_bridge_channel_try_process_none() {
    let (_tx, rx) = bridge_channel(Duration::from_secs(5));
    let processed = rx.try_process(|_| BridgeResponse { result: Ok(serde_json::Value::Null) });
    assert!(!processed);
}

#[test]
fn test_bridge_channel_try_process_one() {
    let (tx, rx) = bridge_channel(Duration::from_secs(5));
    tx.send_fire_and_forget(BridgeCommand::GetTitle);
    let processed = rx.try_process(|cmd| {
        assert!(matches!(cmd, BridgeCommand::GetTitle));
        BridgeResponse { result: Ok(serde_json::Value::Null) }
    });
    assert!(processed);
    // Second try should be empty
    let processed2 = rx.try_process(|_| unreachable!());
    assert!(!processed2);
}

#[test]
fn test_bridge_send_returns_timeout_on_no_processor() {
    let (tx, rx) = bridge_channel(Duration::from_millis(50));
    // Drop receiver without processing — send should timeout
    drop(rx);
    let resp = tx.send(BridgeCommand::GetTitle);
    assert!(resp.result.is_err());
}
