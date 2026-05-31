// @trace TEST-CDS-020 [req:REQ-CDS-001,REQ-CDS-006] [level:unit]
// CdpServer construction, port(), ws_url_for_target(), registry/broadcaster accessors,
// ServerConfig + ServerConfigBuilder exhaustive field verification.

use cdp_server::{CdpServer, ServerConfig, DomainRegistry, EventBroadcaster};
use cdp_server::{CdpMessage, CdpEvent, SessionError, SessionState};
use cdp_server::{TargetInfo, is_websocket_upgrade};

use serde_json::json;

// ============================================================================
// CdpServer construction
// ============================================================================

#[test]
fn test_cdp_server_new_default_config() {
    let server = CdpServer::new(ServerConfig::default());
    assert_eq!(server.port(), 9222);
}

#[test]
fn test_cdp_server_new_custom_port() {
    let config = ServerConfig::builder().port(8421).build();
    let server = CdpServer::new(config);
    assert_eq!(server.port(), 8421);
}

#[test]
fn test_cdp_server_new_port_zero() {
    let config = ServerConfig::builder().port(0).build();
    let server = CdpServer::new(config);
    assert_eq!(server.port(), 0);
}

#[test]
fn test_cdp_server_new_port_max() {
    let config = ServerConfig::builder().port(65535).build();
    let server = CdpServer::new(config);
    assert_eq!(server.port(), 65535);
}

// ============================================================================
// ws_url_for_target
// ============================================================================

#[test]
fn test_ws_url_default_host_port() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("abc123");
    assert_eq!(url, "ws://127.0.0.1:9222/devtools/page/abc123");
}

#[test]
fn test_ws_url_custom_host() {
    let config = ServerConfig::builder().host("0.0.0.0").build();
    let server = CdpServer::new(config);
    let url = server.ws_url_for_target("target-1");
    assert_eq!(url, "ws://0.0.0.0:9222/devtools/page/target-1");
}

#[test]
fn test_ws_url_custom_port() {
    let config = ServerConfig::builder().port(3000).build();
    let server = CdpServer::new(config);
    let url = server.ws_url_for_target("t1");
    assert_eq!(url, "ws://127.0.0.1:3000/devtools/page/t1");
}

#[test]
fn test_ws_url_empty_target_id() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("");
    assert_eq!(url, "ws://127.0.0.1:9222/devtools/page/");
}

#[test]
fn test_ws_url_special_chars_target() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("abc-def_123");
    assert!(url.contains("abc-def_123"));
}

#[test]
fn test_ws_url_unicode_target() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("target\u{4e2d}\u{6587}");
    assert!(url.contains("target\u{4e2d}\u{6587}"));
}

#[test]
fn test_ws_url_different_targets_differ() {
    let server = CdpServer::new(ServerConfig::default());
    let url1 = server.ws_url_for_target("a");
    let url2 = server.ws_url_for_target("b");
    assert_ne!(url1, url2);
}

#[test]
fn test_ws_url_ipv6_host() {
    let config = ServerConfig::builder().host("::1").build();
    let server = CdpServer::new(config);
    let url = server.ws_url_for_target("t");
    assert!(url.contains("::1"));
}

// ============================================================================
// registry accessor
// ============================================================================

#[test]
fn test_cdp_server_registry_not_none() {
    let server = CdpServer::new(ServerConfig::default());
    // Registry should exist but be empty
    assert!(!server.registry().has_domain("Page"));
}

#[test]
fn test_cdp_server_registry_empty_initially() {
    let server = CdpServer::new(ServerConfig::default());
    let reg = server.registry();
    assert!(!reg.has_domain("Runtime"));
    assert!(!reg.has_domain("DOM"));
    assert!(!reg.has_domain("Network"));
    assert!(!reg.has_domain("Page"));
}

// ============================================================================
// broadcaster accessor
// ============================================================================

#[test]
fn test_cdp_server_broadcaster_exists() {
    let server = CdpServer::new(ServerConfig::default());
    let b = server.broadcaster();
    // Can create a sender without panic
    let _sender = b.sender();
}

#[test]
fn test_broadcaster_sender_is_boxed() {
    let server = CdpServer::new(ServerConfig::default());
    let b = server.broadcaster();
    let sender = b.sender();
    // Should be able to send event without panic (no sessions)
    sender.send_event("Page.loadEventFired", json!({}));
}

#[test]
fn test_broadcaster_clone() {
    let server = CdpServer::new(ServerConfig::default());
    let b1 = server.broadcaster();
    let b2 = b1.clone();
    let s1 = b1.sender();
    let s2 = b2.sender();
    // Both should work
    s1.send_event("Page.frameNavigated", json!({"url": "a"}));
    s2.send_event("Page.frameNavigated", json!({"url": "b"}));
}

// ============================================================================
// CdpMessage parsing via serde (parse_message is internal, test via serde_json)
// ============================================================================

#[test]
fn test_cdp_message_deserialize_all_fields() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":42,"method":"Page.navigate","params":{"url":"http://x"},"session_id":"s1"}"#).unwrap();
    assert_eq!(msg.id, Some(42));
    assert_eq!(msg.method, "Page.navigate");
    assert_eq!(msg.params.as_ref().unwrap()["url"], "http://x");
    assert_eq!(msg.session_id.as_deref(), Some("s1"));
}

#[test]
fn test_cdp_message_deserialize_minimal() {
    let msg: CdpMessage = serde_json::from_str(r#"{"method":"Page.enable"}"#).unwrap();
    assert_eq!(msg.id, None);
    assert_eq!(msg.method, "Page.enable");
    assert!(msg.params.is_none());
    assert!(msg.session_id.is_none());
}

#[test]
fn test_cdp_message_null_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":null,"method":"X"}"#).unwrap();
    assert_eq!(msg.id, None);
}

#[test]
fn test_cdp_message_negative_large_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":-999999,"method":"X"}"#).unwrap();
    assert_eq!(msg.id, Some(-999999));
}

// ============================================================================
// CdpEvent serialization via serde
// ============================================================================

#[test]
fn test_cdp_event_serialize_with_params() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345})),
    };
    let s = serde_json::to_string(&ev).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["method"], "Page.loadEventFired");
    assert_eq!(parsed["params"]["timestamp"], 12345);
    assert!(parsed.get("id").is_none());
}

#[test]
fn test_cdp_event_serialize_no_params_skipped() {
    let ev = CdpEvent {
        method: "Runtime.consoleAPICalled".into(),
        params: None,
    };
    let s = serde_json::to_string(&ev).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["method"], "Runtime.consoleAPICalled");
    assert!(parsed.get("params").is_none());
}

// ============================================================================
// SessionState exhaustive
// ============================================================================

#[test]
fn test_session_state_all_variants() {
    let states = [SessionState::Created, SessionState::Active, SessionState::Closing, SessionState::Closed];
    // All distinct
    for i in 0..states.len() {
        for j in (i+1)..states.len() {
            assert_ne!(states[i], states[j]);
        }
    }
}

#[test]
fn test_session_state_ordering() {
    // SessionState derives Copy + Clone + PartialEq + Eq + Debug, not PartialOrd
    // Verify distinctness instead
    assert_ne!(SessionState::Created, SessionState::Active);
    assert_ne!(SessionState::Active, SessionState::Closing);
    assert_ne!(SessionState::Closing, SessionState::Closed);
}

#[test]
fn test_session_state_copy() {
    let s = SessionState::Active;
    let s2 = s;
    assert_eq!(s, s2);
}

#[test]
fn test_session_state_clone() {
    let s = SessionState::Closing;
    let s2 = s.clone();
    assert_eq!(s, s2);
}

// ============================================================================
// SessionError exhaustive
// ============================================================================

#[test]
fn test_session_error_variants_differ() {
    let e1 = format!("{:?}", SessionError::Closed);
    let e2 = format!("{:?}", SessionError::Io);
    assert_ne!(e1, e2);
}

#[test]
fn test_session_error_debug_closed() {
    assert!(format!("{:?}", SessionError::Closed).contains("Closed"));
}

#[test]
fn test_session_error_debug_io() {
    assert!(format!("{:?}", SessionError::Io).contains("Io"));
}

// ============================================================================
// TargetInfo exhaustive
// ============================================================================

#[test]
fn test_target_info_all_fields() {
    let info = TargetInfo {
        id: "tid-1".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/tid-1".into(),
    };
    assert_eq!(info.id, "tid-1");
    assert_eq!(info.target_type, "page");
    assert_eq!(info.title, "Test");
    assert_eq!(info.url, "https://example.com");
    assert!(info.web_socket_debugger_url.contains("tid-1"));
}

#[test]
fn test_target_info_serialize_roundtrip() {
    let info = TargetInfo {
        id: "tid-2".into(),
        target_type: "page".into(),
        title: "My Page".into(),
        url: "https://test.com".into(),
        web_socket_debugger_url: "ws://localhost:9222/devtools/page/tid-2".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, info.id);
    assert_eq!(parsed.target_type, info.target_type);
    assert_eq!(parsed.title, info.title);
    assert_eq!(parsed.url, info.url);
}

#[test]
fn test_target_info_type_field_rename() {
    let info = TargetInfo {
        id: "x".into(),
        target_type: "iframe".into(),
        title: "".into(),
        url: "".into(),
        web_socket_debugger_url: "".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("\"type\":\"iframe\""));
    assert!(!json.contains("\"target_type\""));
}

#[test]
fn test_target_info_clone() {
    let info = TargetInfo {
        id: "c1".into(),
        target_type: "page".into(),
        title: "Clone Test".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/c1".into(),
    };
    let cloned = info.clone();
    assert_eq!(cloned.id, info.id);
    assert_eq!(cloned.target_type, info.target_type);
}

// ============================================================================
// ServerConfig builder exhaustive
// ============================================================================

#[test]
fn test_config_all_fields_custom() {
    let config = ServerConfig::builder()
        .host("192.168.1.1")
        .port(8080)
        .http_timeout_seconds(60)
        .max_sessions(200)
        .browser_name("Bao/2.0")
        .user_agent("Bao/2.0 (compatible)")
        .v8_version("12.0")
        .webkit_version("537.36")
        .build();
    assert_eq!(config.host, "192.168.1.1");
    assert_eq!(config.port, 8080);
    assert_eq!(config.http_timeout_seconds, 60);
    assert_eq!(config.max_sessions, 200);
    assert_eq!(config.browser_name, "Bao/2.0");
    assert_eq!(config.user_agent.as_deref(), Some("Bao/2.0 (compatible)"));
    assert_eq!(config.v8_version.as_deref(), Some("12.0"));
    assert_eq!(config.webkit_version.as_deref(), Some("537.36"));
}

#[test]
fn test_config_default_protocol_version() {
    let config = ServerConfig::default();
    assert_eq!(config.protocol_version, "1.3");
}

#[test]
fn test_config_default_optional_fields() {
    let config = ServerConfig::default();
    assert!(config.user_agent.is_none());
    assert!(config.v8_version.is_none());
    assert!(config.webkit_version.is_none());
}

#[test]
fn test_builder_chaining_order() {
    // Builder should accept any order
    let config = ServerConfig::builder()
        .v8_version("11")
        .port(9999)
        .host("0.0.0.0")
        .user_agent("UA")
        .build();
    assert_eq!(config.v8_version.as_deref(), Some("11"));
    assert_eq!(config.port, 9999);
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.user_agent.as_deref(), Some("UA"));
}

#[test]
fn test_builder_overwrite() {
    let config = ServerConfig::builder()
        .port(1000)
        .port(2000)
        .build();
    assert_eq!(config.port, 2000);
}

#[test]
fn test_builder_empty_strings() {
    let config = ServerConfig::builder()
        .host("")
        .browser_name("")
        .user_agent("")
        .build();
    assert_eq!(config.host, "");
    assert_eq!(config.browser_name, "");
    assert_eq!(config.user_agent.as_deref(), Some(""));
}

// ============================================================================
// is_websocket_upgrade
// ============================================================================

#[test]
fn test_is_ws_upgrade_uppercase() {
    assert!(is_websocket_upgrade("GET / HTTP/1.1\r\nUpgrade: websocket\r\n"));
}

#[test]
fn test_is_ws_upgrade_lowercase() {
    assert!(is_websocket_upgrade("GET / HTTP/1.1\r\nupgrade: websocket\r\n"));
}

#[test]
fn test_is_ws_upgrade_no_upgrade() {
    assert!(!is_websocket_upgrade("GET / HTTP/1.1\r\nHost: localhost\r\n"));
}

#[test]
fn test_is_ws_upgrade_empty() {
    assert!(!is_websocket_upgrade(""));
}

#[test]
fn test_is_ws_upgrade_partial() {
    assert!(!is_websocket_upgrade("Upgrade: "));
}

// ============================================================================
// DomainRegistry thread safety
// ============================================================================

#[test]
fn test_registry_arc_thread_safety() {
    use std::sync::Arc;
    use std::thread;
    let reg = Arc::new(DomainRegistry::new());
    let mut handles = vec![];
    for _ in 0..4 {
        let r = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            assert!(!r.has_domain("Test"));
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}
