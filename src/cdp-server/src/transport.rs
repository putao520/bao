// @trace REQ-CDS-001 [entity:CdpServer] [api:GET /json/version]
// @trace REQ-CDS-002 [entity:CdpTarget]
// Transport layer: HTTP discovery endpoints + WebSocket upgrade.

use std::io::Write;
use std::net::TcpStream;

use serde_json::Value;

/// Browser Target information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TargetInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub target_type: String,
    pub title: String,
    pub url: String,
    pub web_socket_debugger_url: String,
}

/// Extract the request path (the token between the method and the version).
/// "GET /json/list HTTP/1.1" → "/json/list". Query strings stay attached;
/// callers that need the bare path compare against exact path literals.
pub fn request_path(request: &str) -> &str {
    request.split(' ').nth(1).unwrap_or("")
}

/// Handle an HTTP request. Returns Some((target_id, is_browser)) if this
/// is a WebSocket upgrade request, None for plain HTTP.
pub fn handle_http_request(
    stream: &mut TcpStream,
    request: &str,
    config: &crate::ServerConfig,
    targets: &[TargetInfo],
) -> Option<(String, bool)> {
    let path = request_path(request);

    // GET /json/version
    if path == "/json/version" {
        let ws_url = format!("ws://{}:{}/devtools/browser", config.host, config.port);
        let mut body = serde_json::json!({
            "Browser": config.browser_name,
            "Protocol-Version": config.protocol_version,
            "webSocketDebuggerUrl": ws_url,
        });
        if let Some(ref ua) = config.user_agent {
            body["User-Agent"] = Value::String(ua.clone());
        }
        if let Some(ref v8) = config.v8_version {
            body["V8-Version"] = Value::String(v8.clone());
        }
        if let Some(ref wk) = config.webkit_version {
            body["WebKit-Version"] = Value::String(wk.clone());
        }
        respond_json(stream, &body);
        return None;
    }

    // GET /json or GET /json/list — both are the target-listing endpoint
    // (Chrome serves both; close/activate/new are parsed before this point).
    if path == "/json" || path == "/json/list" {
        respond_json(stream, &serde_json::json!(targets));
        return None;
    }

    // WebSocket upgrade: /devtools/page/{targetId}
    if let Some(rest) = request.strip_prefix("GET /devtools/page/") {
        let target_id = rest.split(' ').next().unwrap_or("").to_string();
        return Some((target_id, false));
    }

    // WebSocket upgrade: /devtools/browser
    if request.starts_with("GET /devtools/browser") {
        return Some(("__browser__".into(), true));
    }

    respond_text(stream, "404 Not Found", "Not Found");
    None
}

/// Parse the request to determine if it's a WebSocket upgrade.
/// Returns the pre-read bytes for replay.
#[allow(dead_code)]
pub fn is_websocket_upgrade(request: &str) -> bool {
    request.contains("Upgrade: websocket") || request.contains("upgrade: websocket")
}

pub fn respond_json(stream: &mut TcpStream, value: &Value) {
    let body = value.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    respond_raw(stream, &response);
}

/// Plain-text HTTP response with a proper status line. Every discovery
/// endpoint reply — success AND error — must go through a serializer that
/// emits the status line; bare bodies are unparseable by real HTTP clients.
pub fn respond_text(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    );
    respond_raw(stream, &response);
}

pub fn respond_raw(stream: &mut TcpStream, text: &str) {
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}

/// Parse HTTP path for target actions. Returns (action, target_id).
pub fn parse_close_request(request: &str) -> Option<String> {
    let prefix = "GET /json/close/";
    if let Some(rest) = request.strip_prefix(prefix) {
        let id = rest.split(' ').next()?;
        Some(id.to_string())
    } else {
        None
    }
}

pub fn parse_activate_request(request: &str) -> Option<String> {
    let prefix = "GET /json/activate/";
    if let Some(rest) = request.strip_prefix(prefix) {
        let id = rest.split(' ').next()?;
        Some(id.to_string())
    } else {
        None
    }
}

/// Parse `GET /json/new[?...]` into the URL for the new target.
///
/// Wire forms (Chrome discovery contract): `?url=<encoded url>` key-value
/// form (the `url=` key is stripped, never forwarded to the target), the
/// legacy bare `?<url>` form, and no query → `about:blank`.
pub fn parse_new_request(request: &str) -> Option<String> {
    let prefix = "GET /json/new";
    if let Some(rest) = request.strip_prefix(prefix) {
        if rest.starts_with('?') {
            let end = rest.find(' ').unwrap_or(rest.len());
            let qs = &rest[1..end];
            let url = qs.strip_prefix("url=").unwrap_or(qs);
            Some(percent_decode(url))
        } else {
            Some("about:blank".to_string())
        }
    } else {
        None
    }
}

fn percent_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().unwrap_or(b'0');
            let lo = chars.next().unwrap_or(b'0');
            let val = hex_val(hi) << 4 | hex_val(lo);
            result.push(val as char);
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

// @trace REQ-CDS-004 [req:REQ-CDS-004] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_upgrade_mixed_case() {
        let req = "GET /devtools/page/abc HTTP/1.1\r\nUpgrade: websocket";
        assert!(is_websocket_upgrade(req));
    }

    #[test]
    fn websocket_upgrade_lowercase() {
        let req = "GET /devtools/page/abc HTTP/1.1\r\nupgrade: websocket";
        assert!(is_websocket_upgrade(req));
    }

    #[test]
    fn websocket_upgrade_missing_header() {
        let req = "GET /json/version HTTP/1.1\r\nHost: localhost";
        assert!(!is_websocket_upgrade(req));
    }

    #[test]
    fn close_request_extracts_target_id() {
        let req = "GET /json/close/target-123 HTTP/1.1";
        assert_eq!(parse_close_request(req), Some("target-123".to_string()));
    }

    #[test]
    fn close_request_wrong_path() {
        let req = "GET /json/list HTTP/1.1";
        assert_eq!(parse_close_request(req), None);
    }

    #[test]
    fn activate_request_extracts_target_id() {
        let req = "GET /json/activate/tid HTTP/1.1";
        assert_eq!(parse_activate_request(req), Some("tid".to_string()));
    }

    #[test]
    fn new_request_key_value_form_strips_url_key() {
        let req = "GET /json/new?url=https://example.com HTTP/1.1";
        assert_eq!(
            parse_new_request(req),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn new_request_bare_url_form() {
        let req = "GET /json/new?https://example.com/x HTTP/1.1";
        assert_eq!(
            parse_new_request(req),
            Some("https://example.com/x".to_string())
        );
    }

    #[test]
    fn new_request_percent_encoded_url() {
        let req = "GET /json/new?url=https%3A%2F%2Fexample.com%2Fa%20b HTTP/1.1";
        assert_eq!(
            parse_new_request(req),
            Some("https://example.com/a b".to_string())
        );
    }

    #[test]
    fn new_request_no_query_defaults_to_about_blank() {
        let req = "GET /json/new HTTP/1.1";
        assert_eq!(parse_new_request(req), Some("about:blank".to_string()));
    }

    #[test]
    fn new_request_wrong_path() {
        let req = "GET /json/version HTTP/1.1";
        assert_eq!(parse_new_request(req), None);
    }

    #[test]
    fn request_path_extracts_path_token() {
        assert_eq!(request_path("GET /json/list HTTP/1.1"), "/json/list");
        assert_eq!(request_path("GET /json HTTP/1.1"), "/json");
        assert_eq!(
            request_path("GET /json/new?url=x HTTP/1.1"),
            "/json/new?url=x"
        );
        assert_eq!(request_path("garbage"), "");
    }

    #[test]
    fn percent_decode_space() {
        assert_eq!(percent_decode("%20"), " ");
    }

    #[test]
    fn percent_decode_hello() {
        assert_eq!(percent_decode("%48%65%6C%6C%6F"), "Hello");
    }

    #[test]
    fn percent_decode_no_encoding() {
        assert_eq!(percent_decode("abc"), "abc");
    }

    #[test]
    fn hex_val_digits() {
        assert_eq!(hex_val(b'0'), 0);
        assert_eq!(hex_val(b'a'), 10);
        assert_eq!(hex_val(b'F'), 15);
    }
}
