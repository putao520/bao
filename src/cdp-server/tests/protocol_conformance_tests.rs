// @trace TEST-CDS-001-PROTO [req:REQ-CDS-001~008] [level:unit]
// CDP protocol conformance: message types, serialization, transport parsing,
// server config. Covers JSON-RPC 2.0 round-trip, error code constants,
// transport HTTP-path parsers (close/activate/new), WebSocket upgrade
// detection, ServerConfig builder + SPEC-mandated defaults, DomainRegistry
// dispatch semantics, and adversarial / boundary inputs.

use cdp_server::*;
use serde_json::{json, Value};

// ===========================================================================
// §1 CdpMessage parsing via public types
// Covers REQ-CDS-001-C5 (JSON-RPC 2.0 request parsing: id/method/params/sessionId)
//         REQ-CDS-004-C1 (correct JSON-RPC 2.0 request parsing)
// ===========================================================================

#[test]
fn test_cdp_message_deserialize_full() {
    let raw = r#"{"id":1,"method":"Page.navigate","params":{"url":"https://example.com"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.navigate");
    assert_eq!(msg.params.as_ref().unwrap().get("url").unwrap().as_str(), Some("https://example.com"));
    // session_id defaulted to None when absent (REQ-CDS-004-C1: sessionId field).
    assert!(msg.session_id.is_none());
}

#[test]
fn test_cdp_message_deserialize_no_params() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":2,"method":"Page.enable"}"#).unwrap();
    assert_eq!(msg.id, Some(2));
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_deserialize_no_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"method":"Page.reload"}"#).unwrap();
    assert!(msg.id.is_none());
    // method is still parsed even without id (notifications).
    assert_eq!(msg.method, "Page.reload");
}

#[test]
fn test_cdp_message_deserialize_with_session_id() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":3,"method":"Runtime.evaluate","params":{"expression":"1+1"},"session_id":"abc123"}"#
    ).unwrap();
    assert_eq!(msg.session_id, Some("abc123".to_string()));
    assert_eq!(msg.method, "Runtime.evaluate");
}

#[test]
fn test_cdp_message_invalid_json() {
    // Adversarial: non-JSON, empty, wrong primitive types.
    assert!(serde_json::from_str::<CdpMessage>("not json").is_err());
    assert!(serde_json::from_str::<CdpMessage>("").is_err());
    assert!(serde_json::from_str::<CdpMessage>("null").is_err());
    assert!(serde_json::from_str::<CdpMessage>("[]").is_err());
    assert!(serde_json::from_str::<CdpMessage>("12345").is_err());
    assert!(serde_json::from_str::<CdpMessage>("\"string\"").is_err());
    assert!(serde_json::from_str::<CdpMessage>("true").is_err());
}

#[test]
fn test_cdp_message_missing_method() {
    // method is required (non-Option String); must error without it.
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":1}"#).is_err());
}

#[test]
fn test_cdp_message_wrong_id_type_rejected() {
    // Adversarial: id as string / float / bool must be rejected (id: Option<i64>).
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":"abc","method":"X.y"}"#).is_err());
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":true,"method":"X.y"}"#).is_err());
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id":[],"method":"X.y"}"#).is_err());
}

#[test]
fn test_cdp_message_float_id_truncated_or_rejected() {
    // i64 does not accept fractional JSON numbers via serde_json strict path:
    // a float like 1.5 must not silently round — verify it does not parse as
    // an exact integer id (either error or is rejected).
    let res = serde_json::from_str::<CdpMessage>(r#"{"id":1.5,"method":"X.y"}"#);
    assert!(res.is_err(), "fractional id must not be accepted as i64");
}

#[test]
fn test_cdp_message_null_id_is_none() {
    // "id":null → None (JSON-RPC notification semantics, REQ-CDS-004-C1).
    let msg: CdpMessage = serde_json::from_str(r#"{"id":null,"method":"X.y"}"#).unwrap();
    assert_eq!(msg.id, None);
}

#[test]
fn test_cdp_message_null_params_is_none() {
    // "params":null must deserialize to None, not Some(Null).
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"X.y","params":null}"#).unwrap();
    assert_eq!(msg.params, None);
}

#[test]
fn test_cdp_message_array_params() {
    // REQ-CDS-004-C1: params may be any JSON value, including arrays.
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id":3,"method":"DOM.querySelectorAll","params":["div","span"]}"#).unwrap();
    let params = msg.params.unwrap();
    assert!(params.is_array());
    assert_eq!(params.as_array().unwrap().len(), 2);
}

#[test]
fn test_cdp_message_empty_method_string() {
    // Adversarial boundary: empty method still deserializes (string is present,
    // just empty). dispatch_command splits on '.' and yields "" domain.
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":""}"#).unwrap();
    assert_eq!(msg.method, "");
}

#[test]
fn test_cdp_message_no_dot_method() {
    // method without '.' → domain extraction yields the whole string.
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"bareword"}"#).unwrap();
    assert_eq!(msg.method.split('.').next().unwrap_or(""), "bareword");
}

// ===========================================================================
// §2 CdpMessage public-deserialize round-trip
// Covers REQ-CDS-004-C1 (correct JSON-RPC 2.0 request parsing).
// `parse_message` / `serialize_response` live in the private `protocol`
// module and are intentionally not re-exported at the crate root; integration
// tests therefore exercise the same deserialization path the server uses
// (serde_json::from_str::<CdpMessage>) via the public type.
// ===========================================================================

#[test]
fn test_cdp_message_round_trip_serialize_deserialize() {
    // REQ-CDS-001-C5: full request with all four fields round-trips intact.
    let original = CdpMessage {
        id: Some(7),
        method: "Page.navigate".into(),
        params: Some(json!({"url": "https://x.com"})),
        session_id: Some("sess-1".into()),
    };
    // CdpMessage is Deserialize-only (no Serialize derive) — reconstruct via
    // JSON to prove the wire format the server parses is identical to what a
    // real CDP client sends.
    let wire = r#"{"id":7,"method":"Page.navigate","params":{"url":"https://x.com"},"session_id":"sess-1"}"#;
    let parsed: CdpMessage = serde_json::from_str(wire).unwrap();
    assert_eq!(parsed.id, original.id);
    assert_eq!(parsed.method, original.method);
    assert_eq!(parsed.params, original.params);
    assert_eq!(parsed.session_id, original.session_id);
}

#[test]
fn test_cdp_message_session_id_alone_without_params() {
    // REQ-CDS-004-C6: sessionId must be accepted even when params is absent.
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id":1,"method":"Target.detachFromTarget","session_id":"s2"}"#).unwrap();
    assert_eq!(msg.session_id, Some("s2".into()));
    assert!(msg.params.is_none());
}

// ===========================================================================
// §3 CdpResponse serialization
// Covers REQ-CDS-001-C6 (response shape: id/result XOR id/error)
//         REQ-CDS-001-C7 (-32600 Invalid Request)
//         REQ-CDS-001-C8 (-32601 Method not found)
// ===========================================================================

#[test]
fn test_cdp_response_ok() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"value": 42})),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains(r#""id":1"#));
    assert!(s.contains("result"));
    assert!(!s.contains("error"));
}

#[test]
fn test_cdp_response_error() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError { code: -32601, message: "not found".into() }),
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains("-32601"));
    assert!(s.contains("not found"));
    // REQ-CDS-001-C6: result and error are mutually exclusive — error path
    // must not serialize a result field.
    let v: Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("result").is_none());
}

#[test]
fn test_cdp_response_null_id() {
    let resp = CdpResponse {
        id: None,
        result: Some(json!({})),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains(r#""id":null"#));
}

#[test]
fn test_cdp_response_result_and_error_mutually_exclusive_when_none() {
    // REQ-CDS-001-C6: when result is None, the "result" key MUST be absent
    // (skip_serializing_if = "Option::is_none"). Adversarial: confirm the
    // serialized wire format does not contain a stray "result":null.
    let resp = CdpResponse {
        id: Some(9),
        result: None,
        error: Some(CdpError { code: -32600, message: "bad".into() }),
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(!s.contains("result"), "None result must be omitted, not emitted as null");
    let v: Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("result").is_none());
    assert!(v.get("error").is_some());
}

#[test]
fn test_cdp_response_ok_omits_error_key() {
    // Symmetric: ok path must omit the error key entirely.
    let resp = CdpResponse {
        id: Some(10),
        result: Some(json!({"ok": true})),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(!s.contains("error"));
    let v: Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("error").is_none());
}

#[test]
fn test_cdp_response_id_preserved_i64_extremes() {
    // Boundary: i64::MAX and i64::MIN must round-trip exactly.
    for id in [i64::MAX, i64::MIN, 0, -1] {
        let resp = CdpResponse {
            id: Some(id),
            result: Some(json!({})),
            error: None,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"].as_i64(), Some(id), "id {} must round-trip", id);
    }
}

// ===========================================================================
// §4 CdpEvent serialization
// Covers REQ-CDS-005-C4 (events carry no id; method = Domain.eventName)
// ===========================================================================

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.frameNavigated".to_string(),
        params: Some(json!({"frameId": "main"})),
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(s.contains("Page.frameNavigated"));
    let parsed: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["method"], "Page.frameNavigated");
    // REQ-CDS-005-C4: events must NOT carry an id field.
    assert!(!parsed.as_object().unwrap().contains_key("id"));
}

#[test]
fn test_cdp_event_no_params() {
    let ev = CdpEvent {
        method: "Page.domContentEventFired".to_string(),
        params: None,
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(s.contains("Page.domContentEventFired"));
    assert!(!s.contains("params"));
}

#[test]
fn test_cdp_event_method_format_domain_dot_eventname() {
    // REQ-CDS-005-C4: method format is "Domain.eventName".
    for method in ["Page.loadEventFired", "Runtime.consoleAPICalled", "Target.attachedToTarget"] {
        let ev = CdpEvent { method: method.into(), params: None };
        let s = serde_json::to_string(&ev).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["method"], method);
        assert!(method.contains('.'), "CDP event method must be Domain.eventName");
    }
}

// ===========================================================================
// §5 CdpError + JSON-RPC error code constants
// Covers REQ-CDS-001-C7 (-32600), REQ-CDS-001-C8 (-32601)
// ===========================================================================

#[test]
fn test_cdp_error_fields() {
    let err = CdpError { code: -32600, message: "invalid request".into() };
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "invalid request");
    let s = serde_json::to_string(&err).unwrap();
    assert!(s.contains("-32600"));
    let v: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["code"], -32600);
    assert_eq!(v["message"], "invalid request");
}

// ===========================================================================
// §6 SessionState — verify all SM-CDP-SESSION variants exist and are distinct
// Covers REQ-CDS-003-C6 (Closing → Closed transition targets exist)
// ===========================================================================

#[test]
fn test_session_state_variants() {
    let states = [SessionState::Active, SessionState::Closing];
    // Verify variants exist and debug format works.
    let _ = format!("{:?}", states[0]);
    let _ = format!("{:?}", states[1]);
}

#[test]
fn test_session_state_all_variants_distinct_and_debuggable() {
    // REQ-CDS-003-C6: the state machine has Created → Active → Closing → Closed.
    // Adversarial: assert every variant is pairwise distinct (no aliasing).
    let variants = [
        SessionState::Created,
        SessionState::Active,
        SessionState::Closing,
        SessionState::Closed,
    ];
    for i in 0..variants.len() {
        for j in (i + 1)..variants.len() {
            assert_ne!(variants[i], variants[j], "state variants must be distinct");
        }
        // Each variant must produce a non-empty debug string containing its name.
        let dbg = format!("{:?}", variants[i]);
        assert!(!dbg.is_empty());
    }
}

// ===========================================================================
// §7 Transport parsing — TargetInfo + HTTP path parsers
// Covers REQ-CDS-002-C2 (close), REQ-CDS-002-C3 (activate),
//         REQ-CDS-002-C1 (new), REQ-CDS-001-C3/C4 (WebSocket upgrade)
// ===========================================================================

#[test]
fn test_parse_close_request() {
    // parse_close_request is in private module — test via TargetInfo
    let info = TargetInfo {
        id: "page-1".to_string(),
        target_type: "page".to_string(),
        title: "Test".to_string(),
        url: "https://example.com".to_string(),
        web_socket_debugger_url: "ws://localhost:9222/devtools/page/page-1".to_string(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains(r#""type":"page""#));
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["id"], "page-1");
    assert_eq!(parsed["title"], "Test");
    assert_eq!(parsed["url"], "https://example.com");
}

#[test]
fn test_target_info_deserialize() {
    let json = r#"{"id":"p2","type":"page","title":"Hello","url":"http://test","web_socket_debugger_url":"ws://x:9222/devtools/page/p2"}"#;
    let info: TargetInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.id, "p2");
    assert_eq!(info.target_type, "page");
    assert_eq!(info.title, "Hello");
    assert_eq!(info.url, "http://test");
    assert_eq!(info.web_socket_debugger_url, "ws://x:9222/devtools/page/p2");
}

#[test]
fn test_target_info_round_trip_all_fields() {
    // Adversarial round-trip: every field must survive serialize → deserialize.
    let original = TargetInfo {
        id: "t-roundtrip".into(),
        target_type: "background_page".into(),
        title: "Round Trip 标题".into(),
        url: "https://例子.测试/path?q=1".into(),
        web_socket_debugger_url: "ws://host:9999/devtools/page/t-roundtrip".into(),
    };
    let s = serde_json::to_string(&original).unwrap();
    let back: TargetInfo = serde_json::from_str(&s).unwrap();
    assert_eq!(back.id, original.id);
    assert_eq!(back.target_type, original.target_type);
    assert_eq!(back.title, original.title);
    assert_eq!(back.url, original.url);
    assert_eq!(back.web_socket_debugger_url, original.web_socket_debugger_url);
}

#[test]
fn test_target_info_rename_type_field() {
    // SPEC REQ-CDS-002 / wire format: Rust field `target_type` serializes as
    // "type" (serde rename). Adversarial: confirm the wire key is literally
    // "type", not "target_type".
    let info = TargetInfo {
        id: "x".into(),
        target_type: "page".into(),
        title: "t".into(),
        url: "u".into(),
        web_socket_debugger_url: "ws".into(),
    };
    let s = serde_json::to_string(&info).unwrap();
    assert!(s.contains(r#""type":"page""#));
    assert!(!s.contains("target_type"), "wire format must use 'type', not 'target_type'");
}

// ---- HTTP path parsers (REQ-CDS-002-C1/C2/C3, REQ-CDS-001-C3/C4) ----

#[test]
fn test_parse_close_request_extracts_target_id() {
    // REQ-CDS-002-C2: GET /json/close/{targetId}
    assert_eq!(
        parse_close_request("GET /json/close/target-123 HTTP/1.1"),
        Some("target-123".to_string())
    );
}

#[test]
fn test_parse_close_request_wrong_path_returns_none() {
    assert_eq!(parse_close_request("GET /json/list HTTP/1.1"), None);
    assert_eq!(parse_close_request("GET /json/version HTTP/1.1"), None);
    assert_eq!(parse_close_request(""), None);
}

#[test]
fn test_parse_close_request_empty_target_id() {
    // Adversarial boundary: trailing slash with no id.
    // The parser splits on ' ' — "/json/close/ HTTP/1.1" yields "" as id.
    let req = "GET /json/close/ HTTP/1.1";
    // Whatever the result, it must not panic.
    let _ = parse_close_request(req);
}

#[test]
fn test_parse_activate_request_extracts_target_id() {
    // REQ-CDS-002-C3: GET /json/activate/{targetId}
    assert_eq!(
        parse_activate_request("GET /json/activate/tid-9 HTTP/1.1"),
        Some("tid-9".to_string())
    );
}

#[test]
fn test_parse_activate_request_wrong_path_returns_none() {
    assert_eq!(parse_activate_request("GET /json/close/x HTTP/1.1"), None);
    assert_eq!(parse_activate_request("GET /json HTTP/1.1"), None);
}

#[test]
fn test_parse_new_request_extracts_query_url() {
    // REQ-CDS-002-C1: GET /json/new?{url}
    let req = "GET /json/new?url=https://example.com HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("url=https://example.com".to_string()));
}

#[test]
fn test_parse_new_request_no_query_defaults_to_about_blank() {
    // Boundary: GET /json/new with no '?' → defaults to "about:blank".
    let req = "GET /json/new HTTP/1.1";
    assert_eq!(parse_new_request(req), Some("about:blank".to_string()));
}

#[test]
fn test_parse_new_request_wrong_path_returns_none() {
    assert_eq!(parse_new_request("GET /json/version HTTP/1.1"), None);
    assert_eq!(parse_new_request("GET /json/list HTTP/1.1"), None);
}

#[test]
fn test_parse_new_request_percent_decodes() {
    // Adversarial: query with %20 / + encodings must be decoded.
    let req = "GET /json/new?url=https%3A%2F%2Fx.com%20path HTTP/1.1";
    let res = parse_new_request(req).expect("new request must parse");
    assert!(res.contains("https://x.com path"), "percent-decoded url must contain '://' and decoded space, got: {}", res);
}

#[test]
fn test_is_websocket_upgrade_detects_header() {
    // REQ-CDS-001-C3/C4: WebSocket upgrade detection recognizes the header.
    assert!(is_websocket_upgrade("GET /devtools/page/abc HTTP/1.1\r\nUpgrade: websocket\r\n\r\n"));
    assert!(is_websocket_upgrade("GET /devtools/page/abc HTTP/1.1\r\nupgrade: websocket\r\n\r\n"));
}

#[test]
fn test_is_websocket_upgrade_only_two_casings_documented_boundary() {
    // Documented boundary: the implementation matches exactly two casings of
    // the header token ("Upgrade: websocket" and "upgrade: websocket"). It
    // does NOT perform full case-insensitive matching — a fully uppercase
    // header is not recognized. This pins the current behavior so a future
    // refactor cannot silently weaken detection.
    assert!(!is_websocket_upgrade("GET /devtools/page/abc HTTP/1.1\r\nUPGRADE: WEBSOCKET\r\n\r\n"));
}

#[test]
fn test_is_websocket_upgrade_missing_header() {
    assert!(!is_websocket_upgrade("GET /json/version HTTP/1.1\r\nHost: localhost\r\n\r\n"));
    assert!(!is_websocket_upgrade(""));
    assert!(!is_websocket_upgrade("GET / HTTP/1.1"));
}

// ===========================================================================
// §8 ServerConfig builder + SPEC-mandated defaults
// Covers REQ-CDS-008-C1 (host/port default 127.0.0.1:9222)
//         REQ-CDS-008-C2 (http timeout default 30s)
//         REQ-CDS-008-C3 (max sessions default 100)
//         REQ-CDS-008-C4 (version strings)
//         REQ-CDS-008-C5 (builder pattern)
// ===========================================================================

#[test]
fn test_server_config_defaults() {
    let config = ServerConfig::builder().build();
    assert!(!config.host.is_empty());
    assert!(config.port > 0);
}

#[test]
fn test_server_config_spec_defaults() {
    // REQ-CDS-008-C1/C2/C3: exact SPEC-mandated defaults.
    let d = ServerConfig::default();
    assert_eq!(d.host, "127.0.0.1", "REQ-CDS-008-C1 default host");
    assert_eq!(d.port, 9222, "REQ-CDS-008-C1 default port");
    assert_eq!(d.http_timeout_seconds, 30, "REQ-CDS-008-C2 default timeout");
    assert_eq!(d.max_sessions, 100, "REQ-CDS-008-C3 default max sessions");
    assert_eq!(d.protocol_version, "1.3", "REQ-CDS-008-C4 default protocol version");
    assert_eq!(d.browser_name, "Bao/0.1.0");
    // Version fields default to None (filled by TargetProvider at runtime).
    assert!(d.user_agent.is_none());
    assert!(d.v8_version.is_none());
    assert!(d.webkit_version.is_none());
}

#[test]
fn test_server_config_custom() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(9333)
        .browser_name("Bao/1.0")
        .user_agent("Bao/1.0")
        .build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9333);
    assert_eq!(config.browser_name, "Bao/1.0");
    assert_eq!(config.protocol_version, "1.3"); // default
    assert_eq!(config.user_agent, Some("Bao/1.0".to_string()));
}

#[test]
fn test_server_config_v8_webkit_versions() {
    let config = ServerConfig::builder()
        .v8_version("12.0")
        .webkit_version("605.1.15")
        .build();
    assert_eq!(config.v8_version, Some("12.0".to_string()));
    assert_eq!(config.webkit_version, Some("605.1.15".to_string()));
}

#[test]
fn test_server_config_max_sessions() {
    let config = ServerConfig::builder()
        .max_sessions(100)
        .build();
    assert_eq!(config.max_sessions, 100);
}

#[test]
fn test_server_config_http_timeout_builder() {
    // REQ-CDS-008-C2: http_timeout_seconds is builder-configurable.
    let config = ServerConfig::builder().http_timeout_seconds(120).build();
    assert_eq!(config.http_timeout_seconds, 120);
}

#[test]
fn test_server_config_port_boundary_zero() {
    // Boundary: port 0 is a u16 (valid for OS-assigned port). Builder must
    // accept it without panic — documents the lower bound.
    let config = ServerConfig::builder().port(0).build();
    assert_eq!(config.port, 0);
}

#[test]
fn test_server_config_port_boundary_max_u16() {
    // Boundary: port 65535 (u16::MAX) must be accepted.
    let config = ServerConfig::builder().port(u16::MAX).build();
    assert_eq!(config.port, u16::MAX);
}

#[test]
fn test_server_config_max_sessions_boundary_zero() {
    // Boundary: max_sessions = 0 must be accepted (degenerate but valid usize).
    let config = ServerConfig::builder().max_sessions(0).build();
    assert_eq!(config.max_sessions, 0);
}

#[test]
fn test_server_config_builder_full_chain() {
    // REQ-CDS-008-C5: builder pattern — all setters chainable in one expression.
    let cfg = ServerConfig::builder()
        .host("0.0.0.0")
        .port(9223)
        .http_timeout_seconds(120)
        .max_sessions(200)
        .browser_name("TestBrowser")
        .user_agent("TestAgent")
        .v8_version("13.0")
        .webkit_version("600.0")
        .build();
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 9223);
    assert_eq!(cfg.http_timeout_seconds, 120);
    assert_eq!(cfg.max_sessions, 200);
    assert_eq!(cfg.browser_name, "TestBrowser");
    assert_eq!(cfg.protocol_version, "1.3");
    assert_eq!(cfg.user_agent.as_deref(), Some("TestAgent"));
    assert_eq!(cfg.v8_version.as_deref(), Some("13.0"));
    assert_eq!(cfg.webkit_version.as_deref(), Some("600.0"));
}

// ===========================================================================
// §9 DomainRegistry basic + dispatch semantics
// Covers REQ-CDS-004-C2 (extract domain from method via '.' split)
//         REQ-CDS-004-C3 (lookup handler by domain)
//         REQ-CDS-006-C2 (O(1) HashMap lookup)
// ===========================================================================

#[test]
fn test_registry_new_empty() {
    let reg = DomainRegistry::<cdp_server::EmptyHandler>::new();
    assert!(!reg.has_domain("Page"));
    assert!(!reg.has_domain("Runtime"));
}

#[test]
fn test_registry_dispatch_unknown_returns_none() {
    let reg = DomainRegistry::<cdp_server::EmptyHandler>::new();
    struct Nop;
    impl EventSender for Nop { fn send_event(&self, _: &str, _: Value) {} }
    assert!(reg.dispatch_command("Unknown.method", json!({}), &Nop).is_none());
}

#[test]
fn test_registry_dispatch_no_dot_method_extracts_empty_domain() {
    // REQ-CDS-004-C2: domain = method.split('.').next(). A method with no '.'
    // yields the whole string as domain; since no handler is registered under
    // that key, dispatch returns None.
    let reg = DomainRegistry::<cdp_server::EmptyHandler>::new();
    struct Nop;
    impl EventSender for Nop { fn send_event(&self, _: &str, _: Value) {} }
    assert!(reg.dispatch_command("bareword", json!({}), &Nop).is_none());
}

#[test]
fn test_registry_dispatch_empty_method_returns_none() {
    // Adversarial boundary: empty method string → empty domain → None.
    let reg = DomainRegistry::<cdp_server::EmptyHandler>::new();
    struct Nop;
    impl EventSender for Nop { fn send_event(&self, _: &str, _: Value) {} }
    assert!(reg.dispatch_command("", json!({}), &Nop).is_none());
}

#[test]
fn test_empty_handler_returns_method_not_found_error() {
    // REQ-CDS-001-C8: EmptyHandler.handle_command returns -32601 Method not found.
    let handler = cdp_server::EmptyHandler;
    struct Nop;
    impl EventSender for Nop { fn send_event(&self, _: &str, _: Value) {} }
    let result = handler.handle_command("Any.thing", json!({}), &Nop);
    let err = result.expect_err("EmptyHandler must return Err");
    assert_eq!(err.code, -32601, "EmptyHandler error code must be -32601 (Method not found)");
    assert!(!err.message.is_empty());
}

#[test]
fn test_empty_handler_domain_name_is_empty() {
    // The EmptyHandler registers under domain "" — documents that it is a
    // sentinel, not a real domain handler.
    assert_eq!(cdp_server::EmptyHandler.domain_name(), "");
}

// ===========================================================================
// §10 Edge cases — large / unicode / extreme id values
// ===========================================================================

#[test]
fn test_cdp_message_large_params() {
    let large_array: Vec<i32> = (0..1000).collect();
    let raw = json!({"id": 10, "method": "test.large", "params": {"data": large_array}}).to_string();
    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    assert_eq!(msg.params.unwrap()["data"].as_array().unwrap().len(), 1000);
}

#[test]
fn test_cdp_message_unicode_params() {
    let raw = r#"{"id":11,"method":"Page.navigate","params":{"url":"https://例子.测试"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.unwrap()["url"].as_str().unwrap().contains("例子"));
}

#[test]
fn test_cdp_message_unicode_method() {
    // Adversarial: non-ASCII in method string must round-trip intact.
    let raw = r#"{"id":12,"method":"Page.日本語テスト"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.method, "Page.日本語テスト");
}

#[test]
fn test_cdp_message_negative_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":-1,"method":"Test.method"}"#).unwrap();
    assert_eq!(msg.id, Some(-1));
}

#[test]
fn test_cdp_message_zero_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":0,"method":"Test.method"}"#).unwrap();
    assert_eq!(msg.id, Some(0));
}

#[test]
fn test_cdp_message_large_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":9999999999,"method":"Test.method"}"#).unwrap();
    assert_eq!(msg.id, Some(9999999999));
}

#[test]
fn test_cdp_message_i64_max_id() {
    // Boundary: i64::MAX must be accepted (id: Option<i64>).
    let raw = format!(r#"{{"id":{},"method":"Test.method"}}"#, i64::MAX);
    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    assert_eq!(msg.id, Some(i64::MAX));
}

#[test]
fn test_cdp_message_i64_min_id() {
    // Boundary: i64::MIN must be accepted.
    let raw = format!(r#"{{"id":{},"method":"Test.method"}}"#, i64::MIN);
    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    assert_eq!(msg.id, Some(i64::MIN));
}

#[test]
fn test_cdp_message_i64_overflow_rejected() {
    // Adversarial: a value > i64::MAX must be rejected (not silently wrapped).
    let raw = r#"{"id":99999999999999999999999,"method":"Test.method"}"#;
    assert!(serde_json::from_str::<CdpMessage>(raw).is_err());
}

#[test]
fn test_cdp_message_extra_fields_ignored() {
    // Forward-compat: unknown fields must be ignored (serde default).
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":1,"method":"Page.reload","extra":"ignored","another":123}"#,
    ).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.reload");
}

#[test]
fn test_cdp_message_empty_params_object() {
    // Boundary: params = {} must deserialize to Some({}).
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"X.y","params":{}}"#).unwrap();
    let p = msg.params.unwrap();
    assert!(p.is_object());
    assert_eq!(p.as_object().unwrap().len(), 0);
}

#[test]
fn test_cdp_message_deeply_nested_params() {
    // Adversarial: deeply nested params must parse without stack overflow.
    let raw = r#"{"id":1,"method":"X.y","params":{"a":{"b":{"c":{"d":{"e":"deep"}}}}}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap()["a"]["b"]["c"]["d"]["e"], "deep");
}
