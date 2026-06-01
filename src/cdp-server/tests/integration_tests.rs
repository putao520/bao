// @trace TEST-CDS-006 [req:REQ-CDS-001] [level:integration]
// @trace TEST-CDS-007 [req:REQ-CDS-002] [level:integration]
// @trace TEST-CDS-008 [req:REQ-CDS-003] [level:integration]

use cdp_server::{CdpServer, DomainHandler, DomainRegistry, EventSender, ServerConfig, CdpError};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Test handler
// ---------------------------------------------------------------------------

struct TestHandler;

impl DomainHandler for TestHandler {
    fn domain_name(&self) -> &'static str { "Test" }
    fn handle_command(&self, command: &str, params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Test.hello" => Ok(json!({ "message": "world" })),
            "Test.echo" => Ok(json!({ "params": params })),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

struct NoopEventSender;
impl EventSender for NoopEventSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn start_server(port: u16) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let config = ServerConfig::builder()
            .host("127.0.0.1")
            .port(port)
            .build();
        let mut server = CdpServer::new(config);
        server.registry().register(Box::new(TestHandler)).unwrap();
        // Run for a limited time then stop
        let _ = server.run();
    })
}

fn http_get(url: &str) -> String {
    let stripped = url.trim_start_matches("http://");
    let path_start = stripped.find('/').unwrap_or(stripped.len());
    let host = &stripped[..path_start];
    let path = &stripped[path_start..];
    if path.is_empty() { return String::new(); }
    let mut stream = TcpStream::connect(host).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(stream, "GET {} HTTP/1.1\r\nHost: {}\r\n\r\n", path, host).unwrap();
    stream.flush().unwrap();
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).unwrap();
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

fn ws_request(port: u16, target_id: &str, messages: &[&str]) -> Vec<String> {
    use tungstenite::client;

    let url = format!("ws://127.0.0.1:{}/devtools/page/{}", port, target_id);
    let stream = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
    let (mut ws, _) = client(&url, stream).unwrap();

    let mut responses = Vec::new();
    for msg in messages {
        use tungstenite::Message;
        ws.send(Message::Text(msg.to_string().into())).unwrap();
        // Small delay for server to process
        thread::sleep(Duration::from_millis(50));
        match ws.read() {
            Ok(Message::Text(text)) => responses.push(text.to_string()),
            Ok(Message::Binary(data)) => responses.push(String::from_utf8_lossy(&data).into_owned()),
            _ => {}
        }
    }
    responses
}

// ===========================================================================
// Integration tests
// ===========================================================================

#[test]
fn test_http_json_version() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let response = http_get(&format!("http://127.0.0.1:{}/json/version", port));
    assert!(response.contains("200 OK"));
    assert!(response.contains("Bao"));
    assert!(response.contains("Protocol-Version"));
    assert!(response.contains("1.3"));
}

#[test]
fn test_http_json_list() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let response = http_get(&format!("http://127.0.0.1:{}/json", port));
    assert!(response.contains("200 OK"));
    // Without TargetProvider, targets list is empty
    assert!(response.contains("[]"));
}

#[test]
fn test_http_404_unknown_path() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let response = http_get(&format!("http://127.0.0.1:{}/unknown/path", port));
    assert!(response.contains("404"));
}

#[test]
fn test_websocket_command_dispatch() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let responses = ws_request(port, "test-target", &[
        r#"{"id":1,"method":"Test.hello","params":{}}"#,
    ]);
    assert_eq!(responses.len(), 1);
    let resp: Value = serde_json::from_str(&responses[0]).unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["message"], "world");
}

#[test]
fn test_websocket_echo_params() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let responses = ws_request(port, "test-target", &[
        r#"{"id":2,"method":"Test.echo","params":{"key":"value","num":42}}"#,
    ]);
    assert_eq!(responses.len(), 1);
    let resp: Value = serde_json::from_str(&responses[0]).unwrap();
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["params"]["key"], "value");
    assert_eq!(resp["result"]["params"]["num"], 42);
}

#[test]
fn test_websocket_unknown_method() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let responses = ws_request(port, "test-target", &[
        r#"{"id":3,"method":"Test.nonexistent","params":{}}"#,
    ]);
    assert_eq!(responses.len(), 1);
    let resp: Value = serde_json::from_str(&responses[0]).unwrap();
    assert_eq!(resp["id"], 3);
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn test_websocket_unknown_domain() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let responses = ws_request(port, "test-target", &[
        r#"{"id":4,"method":"Unknown.method","params":{}}"#,
    ]);
    assert_eq!(responses.len(), 1);
    let resp: Value = serde_json::from_str(&responses[0]).unwrap();
    assert_eq!(resp["id"], 4);
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn test_websocket_invalid_json() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let responses = ws_request(port, "test-target", &[
        r#"not valid json"#,
    ]);
    assert_eq!(responses.len(), 1);
    let resp: Value = serde_json::from_str(&responses[0]).unwrap();
    assert_eq!(resp["error"]["code"], -32600);
}

#[test]
fn test_websocket_multiple_commands() {
    let port = find_free_port();
    let _handle = start_server(port);
    thread::sleep(Duration::from_millis(200));

    let responses = ws_request(port, "test-target", &[
        r#"{"id":10,"method":"Test.hello","params":{}}"#,
        r#"{"id":11,"method":"Test.echo","params":{"x":1}}"#,
        r#"{"id":12,"method":"Test.hello","params":{}}"#,
    ]);
    assert_eq!(responses.len(), 3);

    let r0: Value = serde_json::from_str(&responses[0]).unwrap();
    assert_eq!(r0["id"], 10);

    let r1: Value = serde_json::from_str(&responses[1]).unwrap();
    assert_eq!(r1["id"], 11);
    assert_eq!(r1["result"]["params"]["x"], 1);

    let r2: Value = serde_json::from_str(&responses[2]).unwrap();
    assert_eq!(r2["id"], 12);
}
