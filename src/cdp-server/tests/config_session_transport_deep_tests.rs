// @trace TEST-CDS-012 [req:REQ-CDS-001,REQ-CDS-002,REQ-CDS-003] [level:unit]
// ServerConfig builder chain, SessionState transitions,
// TargetInfo serde edge cases, transport parse boundary values.

use cdp_server::{ServerConfig, TargetInfo, SessionState};

// ---- ServerConfig defaults ----

#[test]
fn test_config_default_host() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.host, "127.0.0.1");
}

#[test]
fn test_config_default_port() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.port, 9222);
}

#[test]
fn test_config_default_timeout() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.http_timeout_seconds, 30);
}

#[test]
fn test_config_default_max_sessions() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.max_sessions, 100);
}

#[test]
fn test_config_default_browser_name() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.browser_name, "Bao/0.1.0");
}

#[test]
fn test_config_default_protocol_version() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.protocol_version, "1.3");
}

#[test]
fn test_config_default_optional_fields_none() {
    let cfg = ServerConfig::default();
    assert!(cfg.user_agent.is_none());
    assert!(cfg.v8_version.is_none());
    assert!(cfg.webkit_version.is_none());
}

// ---- ServerConfigBuilder full chain ----

#[test]
fn test_builder_default_equals_config_default() {
    let built = ServerConfig::builder().build();
    let default = ServerConfig::default();
    assert_eq!(built.host, default.host);
    assert_eq!(built.port, default.port);
    assert_eq!(built.http_timeout_seconds, default.http_timeout_seconds);
    assert_eq!(built.max_sessions, default.max_sessions);
}

#[test]
fn test_builder_custom_host() {
    let cfg = ServerConfig::builder().host("0.0.0.0").build();
    assert_eq!(cfg.host, "0.0.0.0");
}

#[test]
fn test_builder_custom_port() {
    let cfg = ServerConfig::builder().port(9333).build();
    assert_eq!(cfg.port, 9333);
}

#[test]
fn test_builder_port_zero() {
    let cfg = ServerConfig::builder().port(0).build();
    assert_eq!(cfg.port, 0);
}

#[test]
fn test_builder_port_max() {
    let cfg = ServerConfig::builder().port(65535).build();
    assert_eq!(cfg.port, 65535);
}

#[test]
fn test_builder_custom_timeout() {
    let cfg = ServerConfig::builder().http_timeout_seconds(5).build();
    assert_eq!(cfg.http_timeout_seconds, 5);
}

#[test]
fn test_builder_timeout_zero() {
    let cfg = ServerConfig::builder().http_timeout_seconds(0).build();
    assert_eq!(cfg.http_timeout_seconds, 0);
}

#[test]
fn test_builder_max_sessions_one() {
    let cfg = ServerConfig::builder().max_sessions(1).build();
    assert_eq!(cfg.max_sessions, 1);
}

#[test]
fn test_builder_custom_browser_name() {
    let cfg = ServerConfig::builder().browser_name("TestBrowser/1.0").build();
    assert_eq!(cfg.browser_name, "TestBrowser/1.0");
}

#[test]
fn test_builder_empty_browser_name() {
    let cfg = ServerConfig::builder().browser_name("").build();
    assert_eq!(cfg.browser_name, "");
}

#[test]
fn test_builder_user_agent() {
    let cfg = ServerConfig::builder().user_agent("Bao/1.0").build();
    assert_eq!(cfg.user_agent.as_deref(), Some("Bao/1.0"));
}

#[test]
fn test_builder_v8_version() {
    let cfg = ServerConfig::builder().v8_version("12.3.45").build();
    assert_eq!(cfg.v8_version.as_deref(), Some("12.3.45"));
}

#[test]
fn test_builder_webkit_version() {
    let cfg = ServerConfig::builder().webkit_version("537.36").build();
    assert_eq!(cfg.webkit_version.as_deref(), Some("537.36"));
}

#[test]
fn test_builder_all_fields() {
    let cfg = ServerConfig::builder()
        .host("192.168.1.1")
        .port(8080)
        .http_timeout_seconds(60)
        .max_sessions(50)
        .browser_name("MyBrowser")
        .user_agent("MyUA")
        .v8_version("13.0")
        .webkit_version("600.0")
        .build();
    assert_eq!(cfg.host, "192.168.1.1");
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.http_timeout_seconds, 60);
    assert_eq!(cfg.max_sessions, 50);
    assert_eq!(cfg.browser_name, "MyBrowser");
    assert_eq!(cfg.user_agent.as_deref(), Some("MyUA"));
    assert_eq!(cfg.v8_version.as_deref(), Some("13.0"));
    assert_eq!(cfg.webkit_version.as_deref(), Some("600.0"));
}

#[test]
fn test_builder_overwrite() {
    let cfg = ServerConfig::builder()
        .host("first")
        .host("second")
        .build();
    assert_eq!(cfg.host, "second");
}

// ---- TargetInfo serde ----

#[test]
fn test_target_info_serialize() {
    let info = TargetInfo {
        id: "abc123".into(),
        target_type: "page".into(),
        title: "Test Page".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/abc123".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains(r#""type":"page""#));
    assert!(json.contains("abc123"));
}

#[test]
fn test_target_info_deserialize() {
    let json = r#"{"id":"x","type":"page","title":"T","url":"U","web_socket_debugger_url":"W"}"#;
    let info: TargetInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.id, "x");
    assert_eq!(info.target_type, "page");
    assert_eq!(info.title, "T");
    assert_eq!(info.url, "U");
    assert_eq!(info.web_socket_debugger_url, "W");
}

#[test]
fn test_target_info_roundtrip() {
    let info = TargetInfo {
        id: "round-trip".into(),
        target_type: "iframe".into(),
        title: "Title".into(),
        url: "http://test".into(),
        web_socket_debugger_url: "ws://test/ws".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, info.id);
    assert_eq!(parsed.target_type, info.target_type);
    assert_eq!(parsed.title, info.title);
    assert_eq!(parsed.url, info.url);
}

#[test]
fn test_target_info_empty_fields() {
    let info = TargetInfo {
        id: String::new(),
        target_type: String::new(),
        title: String::new(),
        url: String::new(),
        web_socket_debugger_url: String::new(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, "");
}

#[test]
fn test_target_info_unicode() {
    let info = TargetInfo {
        id: "id-日本語".into(),
        target_type: "page".into(),
        title: "中文标题".into(),
        url: "https://例え.jp".into(),
        web_socket_debugger_url: "ws://host/id".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("中文标题"));
    let parsed: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.title, "中文标题");
}

#[test]
fn test_target_info_clone() {
    let info = TargetInfo {
        id: "clone-test".into(),
        target_type: "page".into(),
        title: "T".into(),
        url: "U".into(),
        web_socket_debugger_url: "W".into(),
    };
    let cloned = info.clone();
    assert_eq!(cloned.id, info.id);
    assert_eq!(cloned.target_type, info.target_type);
}

#[test]
fn test_target_info_debug() {
    let info = TargetInfo {
        id: "debug-id".into(),
        target_type: "page".into(),
        title: "T".into(),
        url: "U".into(),
        web_socket_debugger_url: "W".into(),
    };
    let debug = format!("{:?}", info);
    assert!(debug.contains("debug-id"));
}

#[test]
fn test_target_info_multiple_serialize_order() {
    let info = TargetInfo {
        id: "1".into(),
        target_type: "page".into(),
        title: "T".into(),
        url: "U".into(),
        web_socket_debugger_url: "W".into(),
    };
    let j1 = serde_json::to_string(&info).unwrap();
    let j2 = serde_json::to_string(&info).unwrap();
    assert_eq!(j1, j2);
}

// ---- SessionState ----

#[test]
fn test_session_state_created() {
    assert_eq!(format!("{:?}", SessionState::Created), "Created");
}

#[test]
fn test_session_state_active() {
    assert_eq!(format!("{:?}", SessionState::Active), "Active");
}

#[test]
fn test_session_state_closing() {
    assert_eq!(format!("{:?}", SessionState::Closing), "Closing");
}

#[test]
fn test_session_state_closed() {
    assert_eq!(format!("{:?}", SessionState::Closed), "Closed");
}

#[test]
fn test_session_state_equality() {
    assert_eq!(SessionState::Created, SessionState::Created);
    assert_ne!(SessionState::Created, SessionState::Active);
}

#[test]
fn test_session_state_clone() {
    let state = SessionState::Active;
    let cloned = state.clone();
    assert_eq!(state, cloned);
}

#[test]
fn test_session_state_copy() {
    let state = SessionState::Closing;
    let copied = state;
    assert_eq!(state, copied);
}

#[test]
fn test_session_state_all_distinct() {
    let states = [SessionState::Created, SessionState::Active, SessionState::Closing, SessionState::Closed];
    for i in 0..states.len() {
        for j in (i+1)..states.len() {
            assert_ne!(states[i], states[j], "{:?} == {:?}", states[i], states[j]);
        }
    }
}

// ---- ServerConfig field access ----

#[test]
fn test_config_field_access() {
    let cfg = ServerConfig::builder().host("field-host").port(9999).build();
    assert_eq!(cfg.host, "field-host");
    assert_eq!(cfg.port, 9999);
    // Verify all fields are pub-accessible
    let _ = cfg.http_timeout_seconds;
    let _ = cfg.max_sessions;
    let _ = cfg.browser_name;
    let _ = cfg.protocol_version;
    let _ = cfg.user_agent;
    let _ = cfg.v8_version;
    let _ = cfg.webkit_version;
}
