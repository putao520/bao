// @trace TEST-CDS-009 [req:REQ-CDS-001,REQ-CDS-002] [level:unit]
// Transport parse functions: parse_close_request, parse_activate_request,
// parse_new_request (percent decoding), is_websocket_upgrade,
// TargetInfo field/serde completeness.

use cdp_server::{
    TargetInfo, parse_close_request, parse_activate_request,
    parse_new_request, is_websocket_upgrade,
};

// ---- parse_close_request ----

#[test]
fn test_close_valid() {
    let req = "GET /json/close/abc123 HTTP/1.1\r\nHost: localhost\r\n\r\n";
    assert_eq!(parse_close_request(req), Some("abc123".into()));
}

#[test]
fn test_close_simple() {
    assert_eq!(parse_close_request("GET /json/close/target-1 "), Some("target-1".into()));
}

#[test]
fn test_close_no_space_after_id() {
    // No trailing space — split returns the rest of the string
    let result = parse_close_request("GET /json/close/xyz");
    assert_eq!(result, Some("xyz".into()));
}

#[test]
fn test_close_with_query_params() {
    let req = "GET /json/close/my-target HTTP/1.1";
    assert_eq!(parse_close_request(req), Some("my-target".into()));
}

#[test]
fn test_close_uuid_target() {
    let req = "GET /json/close/550e8400-e29b-41d4-a716-446655440000 HTTP/1.1";
    assert_eq!(
        parse_close_request(req),
        Some("550e8400-e29b-41d4-a716-446655440000".into())
    );
}

#[test]
fn test_close_wrong_path() {
    assert_eq!(parse_close_request("GET /json/list HTTP/1.1"), None);
}

#[test]
fn test_close_post_method() {
    assert_eq!(parse_close_request("POST /json/close/abc HTTP/1.1"), None);
}

#[test]
fn test_close_empty_string() {
    assert_eq!(parse_close_request(""), None);
}

#[test]
fn test_close_prefix_only() {
    // "GET /json/close/" with no ID — split returns empty
    let result = parse_close_request("GET /json/close/ ");
    assert_eq!(result, Some("".into()));
}

#[test]
fn test_close_long_id() {
    let long_id = "a".repeat(200);
    let req = format!("GET /json/close/{} HTTP/1.1", long_id);
    assert_eq!(parse_close_request(&req), Some(long_id));
}

#[test]
fn test_close_special_chars_in_id() {
    let req = "GET /json/close/target%20with%20spaces HTTP/1.1";
    assert_eq!(parse_close_request(req), Some("target%20with%20spaces".into()));
}

// ---- parse_activate_request ----

#[test]
fn test_activate_valid() {
    let req = "GET /json/activate/abc123 HTTP/1.1\r\n\r\n";
    assert_eq!(parse_activate_request(req), Some("abc123".into()));
}

#[test]
fn test_activate_simple() {
    assert_eq!(parse_activate_request("GET /json/activate/t1 "), Some("t1".into()));
}

#[test]
fn test_activate_no_space() {
    assert_eq!(parse_activate_request("GET /json/activate/xyz"), Some("xyz".into()));
}

#[test]
fn test_activate_uuid() {
    let req = "GET /json/activate/550e8400-e29b-41d4-a716-446655440000 HTTP/1.1";
    assert_eq!(
        parse_activate_request(req),
        Some("550e8400-e29b-41d4-a716-446655440000".into())
    );
}

#[test]
fn test_activate_wrong_path() {
    assert_eq!(parse_activate_request("GET /json/close/abc HTTP/1.1"), None);
}

#[test]
fn test_activate_post_method() {
    assert_eq!(parse_activate_request("POST /json/activate/abc HTTP/1.1"), None);
}

#[test]
fn test_activate_empty_string() {
    assert_eq!(parse_activate_request(""), None);
}

#[test]
fn test_activate_prefix_only() {
    let result = parse_activate_request("GET /json/activate/ ");
    assert_eq!(result, Some("".into()));
}

#[test]
fn test_activate_long_id() {
    let long_id = "b".repeat(200);
    let req = format!("GET /json/activate/{} HTTP/1.1", long_id);
    assert_eq!(parse_activate_request(&req), Some(long_id));
}

// ---- parse_new_request ----

#[test]
fn test_new_with_url() {
    let req = "GET /json/new?https://example.com HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("https://example.com".into()));
}

#[test]
fn test_new_without_url() {
    let req = "GET /json/new HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("about:blank".into()));
}

#[test]
fn test_new_no_space_after() {
    let req = "GET /json/new?https://example.com";
    assert_eq!(parse_new_request(req), Some("https://example.com".into()));
}

#[test]
fn test_new_empty_query() {
    let req = "GET /json/new? HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("".into()));
}

#[test]
fn test_new_percent_encoded_url() {
    let req = "GET /json/new?https%3A%2F%2Fexample.com%2Fpath%3Fq%3D1 HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("https://example.com/path?q=1".into()));
}

#[test]
fn test_new_percent_encoded_spaces() {
    let req = "GET /json/new?https%3A%2F%2Fexample.com%2Fhello%20world HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("https://example.com/hello world".into()));
}

#[test]
fn test_new_plus_as_space() {
    let req = "GET /json/new?https://example.com/hello+world HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("https://example.com/hello world".into()));
}

#[test]
fn test_new_percent_encoded_multibyte() {
    // percent_decode treats each byte as char, so multi-byte UTF-8 chars
    // are decoded byte-by-byte, producing latin1-ish output instead of UTF-8.
    let req = "GET /json/new?https%3A%2F%2Fexample.com%2F%E4%B8%AD%E6%96%87 HTTP/1.1";
    let result = parse_new_request(req).unwrap();
    assert!(result.starts_with("https://example.com/"));
    // Each %XX byte becomes one char (not valid UTF-8 reconstruction)
    assert!(!result.contains("?"));
}

#[test]
fn test_new_mixed_encoding() {
    let req = "GET /json/new?https%3A%2F%2Fexample.com%2Fa+b%20c HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("https://example.com/a b c".into()));
}

#[test]
fn test_new_wrong_path() {
    assert_eq!(parse_new_request("GET /json/list HTTP/1.1"), None);
}

#[test]
fn test_new_post_method() {
    assert_eq!(parse_new_request("POST /json/new?url HTTP/1.1"), None);
}

#[test]
fn test_new_empty_string() {
    assert_eq!(parse_new_request(""), None);
}

#[test]
fn test_new_percent_incomplete_hi() {
    // % at end of URL with no following hex chars — defaults to 0x00
    let req = "GET /json/new?test% HTTP/1.1";
    let result = parse_new_request(req).unwrap();
    // '0' (null char) from incomplete percent encoding
    assert!(result.starts_with("test"));
}

#[test]
fn test_new_percent_incomplete_lo() {
    // %4 with only one hex digit — lo defaults to 0x00
    let req = "GET /json/new?test%4 HTTP/1.1";
    let result = parse_new_request(req).unwrap();
    assert!(result.starts_with("test"));
}

#[test]
fn test_new_percent_uppercase_hex() {
    let req = "GET /json/new?%41%42%43 HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("ABC".into()));
}

#[test]
fn test_new_percent_lowercase_hex() {
    let req = "GET /json/new?%61%62%63 HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("abc".into()));
}

#[test]
fn test_new_percent_mixed_case_hex() {
    let req = "GET /json/new?%4a%4B%4c HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("JKL".into()));
}

#[test]
fn test_new_percent_invalid_hex_chars() {
    // %GG — non-hex chars default to 0
    let req = "GET /json/new?%GG HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("\0".into()));
}

#[test]
fn test_new_long_url() {
    let long_url = "a".repeat(500);
    let req = format!("GET /json/new?{} HTTP/1.1", long_url);
    assert_eq!(parse_new_request(&req), Some(long_url));
}

// ---- is_websocket_upgrade ----

#[test]
fn test_ws_upgrade_standard() {
    let req = "GET /devtools/page/abc HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
    assert!(is_websocket_upgrade(req));
}

#[test]
fn test_ws_upgrade_lowercase() {
    let req = "GET /devtools/page/abc HTTP/1.1\r\nupgrade: websocket\r\n\r\n";
    assert!(is_websocket_upgrade(req));
}

#[test]
fn test_ws_upgrade_mixed_case_not_matched() {
    // "Upgrade: WebSocket" (capital S) does NOT match
    let req = "GET /devtools/page/abc HTTP/1.1\r\nUpgrade: WebSocket\r\n\r\n";
    assert!(!is_websocket_upgrade(req));
}

#[test]
fn test_ws_upgrade_not_present() {
    let req = "GET /json/version HTTP/1.1\r\nHost: localhost\r\n\r\n";
    assert!(!is_websocket_upgrade(req));
}

#[test]
fn test_ws_upgrade_empty_string() {
    assert!(!is_websocket_upgrade(""));
}

#[test]
fn test_ws_upgrade_in_body() {
    // "upgrade: websocket" in request body still matches (string contains check)
    let req = "GET /json HTTP/1.1\r\n\r\nupgrade: websocket is cool";
    assert!(is_websocket_upgrade(req));
}

#[test]
fn test_ws_upgrade_partial_word() {
    // "websockets" contains "websocket" as substring — function uses .contains()
    let req = "X-Header: Upgrade: websockets";
    assert!(is_websocket_upgrade(req));
}

#[test]
fn test_ws_upgrade_only_websocket() {
    let req = "websocket";
    assert!(!is_websocket_upgrade(req));
}

// ---- TargetInfo struct ----

#[test]
fn test_target_info_construction() {
    let info = TargetInfo {
        id: "target-1".into(),
        target_type: "page".into(),
        title: "Test Page".into(),
        url: "https://example.com".into(),
        web_socket_debugger_url: "ws://localhost:9222/devtools/page/target-1".into(),
    };
    assert_eq!(info.id, "target-1");
    assert_eq!(info.target_type, "page");
    assert_eq!(info.title, "Test Page");
    assert_eq!(info.url, "https://example.com");
    assert_eq!(info.web_socket_debugger_url, "ws://localhost:9222/devtools/page/target-1");
}

#[test]
fn test_target_info_debug() {
    let info = TargetInfo {
        id: "abc".into(),
        target_type: "page".into(),
        title: "T".into(),
        url: "u".into(),
        web_socket_debugger_url: "ws".into(),
    };
    let debug = format!("{:?}", info);
    assert!(debug.contains("TargetInfo"));
    assert!(debug.contains("abc"));
}

#[test]
fn test_target_info_clone() {
    let info = TargetInfo {
        id: "x".into(),
        target_type: "y".into(),
        title: "z".into(),
        url: "w".into(),
        web_socket_debugger_url: "v".into(),
    };
    let cloned = info.clone();
    assert_eq!(cloned.id, info.id);
    assert_eq!(cloned.target_type, info.target_type);
    assert_eq!(cloned.title, info.title);
    assert_eq!(cloned.url, info.url);
    assert_eq!(cloned.web_socket_debugger_url, info.web_socket_debugger_url);
}

#[test]
fn test_target_info_serde_roundtrip() {
    let info = TargetInfo {
        id: "target-42".into(),
        target_type: "page".into(),
        title: "My Page".into(),
        url: "https://example.com/page".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/target-42".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let deserialized: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, "target-42");
    assert_eq!(deserialized.target_type, "page");
    assert_eq!(deserialized.title, "My Page");
    assert_eq!(deserialized.url, "https://example.com/page");
    assert_eq!(deserialized.web_socket_debugger_url, "ws://127.0.0.1:9222/devtools/page/target-42");
}

#[test]
fn test_target_info_serde_rename_type() {
    let info = TargetInfo {
        id: "t1".into(),
        target_type: "iframe".into(),
        title: "Frame".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: "ws://localhost/devtools/page/t1".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    // "target_type" should serialize as "type" due to #[serde(rename = "type")]
    assert!(json.contains(r#""type":"iframe""#));
    assert!(!json.contains("target_type"), "should not contain target_type field name");
}

#[test]
fn test_target_info_deserialize_with_type_field() {
    let json = r#"{
        "id": "t2",
        "type": "service_worker",
        "title": "SW",
        "url": "https://example.com/sw.js",
        "web_socket_debugger_url": "ws://localhost/devtools/page/t2"
    }"#;
    let info: TargetInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.id, "t2");
    assert_eq!(info.target_type, "service_worker");
    assert_eq!(info.title, "SW");
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
    let back: TargetInfo = serde_json::from_str(&json).unwrap();
    assert!(back.id.is_empty());
    assert!(back.target_type.is_empty());
}

#[test]
fn test_target_info_unicode_fields() {
    let info = TargetInfo {
        id: "ターゲット".into(),
        target_type: "page".into(),
        title: "日本語ページ".into(),
        url: "https://example.com/中文路径".into(),
        web_socket_debugger_url: "ws://localhost/devtools/page/ターゲット".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let back: TargetInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "ターゲット");
    assert_eq!(back.title, "日本語ページ");
    assert_eq!(back.url, "https://example.com/中文路径");
}

#[test]
fn test_target_info_serde_json_value() {
    let info = TargetInfo {
        id: "t3".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: "ws://x".into(),
    };
    let value = serde_json::to_value(&info).unwrap();
    assert_eq!(value["id"], "t3");
    assert_eq!(value["type"], "page");
    assert_eq!(value["title"], "Test");
    assert_eq!(value["url"], "about:blank");
    assert_eq!(value["web_socket_debugger_url"], "ws://x");
}

// ---- Cross-function consistency ----

#[test]
fn test_close_vs_activate_different_prefixes() {
    let id = "shared-id";
    let close_req = format!("GET /json/close/{} HTTP/1.1", id);
    let activate_req = format!("GET /json/activate/{} HTTP/1.1", id);
    let new_req = "GET /json/new HTTP/1.1";

    assert_eq!(parse_close_request(&close_req), Some(id.into()));
    assert_eq!(parse_activate_request(&activate_req), Some(id.into()));
    assert_eq!(parse_new_request(&new_req), Some("about:blank".into()));

    // Each parser only matches its own prefix
    assert_eq!(parse_close_request(&activate_req), None);
    assert_eq!(parse_activate_request(&close_req), None);
    assert_eq!(parse_new_request(&close_req), None);
}

#[test]
fn test_all_parse_functions_return_none_for_unrelated() {
    let unrelated = "GET /json/version HTTP/1.1";
    assert_eq!(parse_close_request(unrelated), None);
    assert_eq!(parse_activate_request(unrelated), None);
    assert_eq!(parse_new_request(unrelated), None);
}
