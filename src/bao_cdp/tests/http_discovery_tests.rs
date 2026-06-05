// @trace REQ-CDP-001 [api:HTTP discovery]
// Tests for CDP HTTP discovery endpoints

use bao_cdp::CDPServer;

fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
    let request = format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", path, port);
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap_or(0);
    response
}

fn extract_body(response: &str) -> &str {
    if let Some(idx) = response.find("\r\n\r\n") {
        &response[idx + 4..]
    } else {
        ""
    }
}

fn start_server_on_port(port: u16) -> CDPServer {
    CDPServer::new(port)
}

#[test]
fn json_version_returns_browser_info() {
    let port = 19322u16;
    let mut server = start_server_on_port(port);
    let target_id = server.target_id().to_string();

    std::thread::spawn(move || {
        let _ = server.run();
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let response = http_get(port, "/json/version");
    assert!(response.contains("200 OK"));
    let body = extract_body(&response);
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(parsed["Browser"], "Bao/0.1.0");
    assert_eq!(parsed["Protocol-Version"], "1.3");
    assert!(parsed["webSocketDebuggerUrl"].as_str().unwrap().contains(&target_id));
}

#[test]
fn json_list_returns_target_array() {
    let port = 19323u16;
    let mut server = start_server_on_port(port);
    let target_id = server.target_id().to_string();

    std::thread::spawn(move || {
        let _ = server.run();
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let response = http_get(port, "/json/list");
    assert!(response.contains("200 OK"));
    let body = extract_body(&response);
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
    let targets = parsed.as_array().unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["id"], target_id);
    assert_eq!(targets[0]["type"], "page");
}

#[test]
fn json_new_creates_target_entry() {
    let port = 19324u16;
    let mut server = start_server_on_port(port);

    std::thread::spawn(move || {
        let _ = server.run();
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let response = http_get(port, "/json/new?https://example.com");
    assert!(response.contains("200 OK"));
    let body = extract_body(&response);
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(parsed["type"], "page");
    assert!(parsed["webSocketDebuggerUrl"].as_str().unwrap().starts_with("ws://"));
}

#[test]
fn json_close_returns_closing_message() {
    let port = 19325u16;
    let mut server = start_server_on_port(port);

    std::thread::spawn(move || {
        let _ = server.run();
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let response = http_get(port, "/json/close/abc123");
    assert!(response.contains("200 OK"));
    let body = extract_body(&response);
    assert!(body.contains("closing"));
}

#[test]
fn json_activate_returns_activated_message() {
    let port = 19326u16;
    let mut server = start_server_on_port(port);

    std::thread::spawn(move || {
        let _ = server.run();
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let response = http_get(port, "/json/activate/abc123");
    assert!(response.contains("200 OK"));
    let body = extract_body(&response);
    assert!(body.contains("activated"));
}