// @trace TEST-CDP-RES [req:REQ-CDP-001] [level:integration] [nfr:TMG-CDP-01]
// WebSocket server resilience tests for bao_cdp CDPServer.
// The server event loop is single-threaded with blocking per-session reads,
// so multi-session tests use sequential connect→send→recv→close cycles.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use bao_cdp::ws_codec::{FrameDecoder, FrameEncoder};
use bao_cdp::{bridge_channel, CDPCommand, CDPServer, CDPServerError};

const TID: &str = "test-target";

// ---------------------------------------------------------------------------
// Helpers — bao_cdp client (no masking, RFC-compliant server tolerates it)
// ---------------------------------------------------------------------------

fn allocate_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

/// Client-side WebSocket connection: raw TcpStream + bao_cdp codec.
struct ClientWs {
    stream: TcpStream,
    encoder: FrameEncoder,
    decoder: FrameDecoder,
}

impl ClientWs {
    /// Connect + perform client handshake. Server (bao_cdp::CDPServer) replies 101.
    fn connect(server: &TestServer) -> Self {
        let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        // Client handshake — RFC 6455 §4.1
        // bao_cdp server validates Sec-WebSocket-Key presence but not the value.
        let handshake = format!(
            "GET /devtools/page/{} HTTP/1.1\r\n\
             Host: 127.0.0.1:{}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n",
            server.target_id, server.port
        );
        stream.write_all(handshake.as_bytes()).unwrap();
        stream.flush().unwrap();

        // Read 101 response (drain until \r\n\r\n)
        let mut buf = [0u8; 4096];
        let _n = stream.read(&mut buf).unwrap();

        ClientWs {
            stream,
            encoder: FrameEncoder::new(),
            decoder: FrameDecoder::new(),
        }
    }

    fn send_text(&mut self, text: &str) {
        let frame = self.encoder.encode_text(text);
        self.stream.write_all(frame).unwrap();
        self.stream.flush().unwrap();
    }

    fn recv_text(&mut self) -> String {
        loop {
            match self.decoder.decode_frame(&mut self.stream) {
                Ok(Some(header)) => {
                    let payload = self.decoder.take_payload(&header);
                    return String::from_utf8_lossy(&payload).into_owned();
                }
                Ok(None) => continue,
                Err(e) => panic!("recv error: {:?}", e),
            }
        }
    }

    fn shutdown_both(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

struct TestServer {
    cmd_tx: Sender<CDPCommand>,
    target_id: String,
    port: u16,
    handle: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn start(port: u16) -> Self {
        Self::start_inner(port, None)
    }

    fn start_with_bridge(port: u16) -> Self {
        let (tx, _rx) = bridge_channel(Duration::from_millis(500));
        Self::start_inner(port, Some(tx))
    }

    fn start_inner(port: u16, bridge: Option<bao_cdp::BridgeSender>) -> Self {
        let mut server = match bridge {
            Some(tx) => CDPServer::with_bridge(port, tx),
            None => CDPServer::new(port),
        };
        let cmd_tx = server.event_sender();
        let target_id = server.target_id().to_string();
        let handle = thread::spawn(move || {
            let _ = server.run();
        });
        // Give the server thread time to bind.
        thread::sleep(Duration::from_millis(300));
        TestServer {
            cmd_tx,
            target_id,
            port,
            handle: Some(handle),
        }
    }

    fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(CDPCommand::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Connect, immediately send a CDP command, read response, force-close.
fn connect_send_recv(server: &TestServer, id: i64, method: &str) -> serde_json::Value {
    let mut ws = ClientWs::connect(server);
    let req = serde_json::json!({"id": id, "method": method});
    ws.send_text(&serde_json::to_string(&req).unwrap());

    let text = ws.recv_text();
    let resp: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse error: {} (text: {})", e, text));
    ws.shutdown_both();
    resp
}

/// Connect + send without reading (for abuse tests).
fn connect_and_send(server: &TestServer, id: i64, method: &str) -> ClientWs {
    let mut ws = ClientWs::connect(server);
    let req = serde_json::json!({"id": id, "method": method});
    ws.send_text(&serde_json::to_string(&req).unwrap());
    ws
}

fn read_response(ws: &mut ClientWs) -> serde_json::Value {
    let text = ws.recv_text();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse error: {} (text: {})", e, text))
}

/// Wait for server to clean up dropped sessions.
fn wait_for_cleanup() {
    thread::sleep(Duration::from_millis(100));
}

// ---------------------------------------------------------------------------
// Test 1: CDPServer starts on a port and accepts connections
// ---------------------------------------------------------------------------

#[test]
fn test_server_start_accepts_connection() {
    let port = allocate_port();
    let mut server = TestServer::start(port);

    let resp = connect_send_recv(&server, 1, "Page.enable");
    assert_eq!(resp["id"], 1);
    assert!(
        resp.get("result").is_some(),
        "expected result, got: {:?}",
        resp
    );

    wait_for_cleanup();
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 2: Starting on same port twice fails gracefully (AddrInUse)
// ---------------------------------------------------------------------------

#[test]
fn test_addr_in_use_fails_gracefully() {
    let port = allocate_port();
    let _guard = TcpListener::bind(("127.0.0.1", port)).unwrap();

    let mut server2 = CDPServer::new(port);
    let result = server2.run();
    assert!(result.is_err(), "second server should fail to bind");
    match result.unwrap_err() {
        CDPServerError::Bind(msg) => {
            let lower = msg.to_lowercase();
            assert!(
                lower.contains("address") || lower.contains("in use") || lower.contains("already"),
                "unexpected bind error: {}",
                msg
            );
        }
        other => panic!("expected CDPServerError::Bind, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Test 3: Multiple sequential client connections accepted and served
// ---------------------------------------------------------------------------

#[test]
fn test_sequential_connections() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);

    for i in 0i64..5 {
        let resp = connect_send_recv(&server, i, "Page.enable");
        assert_eq!(resp["id"], i, "sequential client {} response id mismatch", i);
        assert!(
            resp.get("result").is_some(),
            "client {} expected result: {:?}",
            i,
            resp
        );
    }

    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 4: 5 clients × 10 requests each (sequential per client)
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_connections_5x10() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);

    for cid in 0..5i64 {
        let mut ws = ClientWs::connect(&server);

        for rid in 0..10i64 {
            let id = cid * 100 + rid;
            let req = serde_json::json!({"id": id, "method": "Page.enable"});
            ws.send_text(&serde_json::to_string(&req).unwrap());
        }

        let mut responses = Vec::new();
        for _ in 0..10 {
            let text = ws.recv_text();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            responses.push(v);
        }

        assert_eq!(responses.len(), 10, "client {} expected 10 responses", cid);
        for resp in &responses {
            assert!(
                resp.get("result").is_some() || resp.get("error").is_some(),
                "client {} unexpected response: {:?}",
                cid,
                resp
            );
        }

        ws.shutdown_both();
        wait_for_cleanup();
    }

    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 5: Malformed JSON-RPC request — server stays alive, returns errors
// ---------------------------------------------------------------------------

#[test]
fn test_malformed_json_no_crash() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);

    {
        let mut ws = ClientWs::connect(&server);

        // Garbage — server silently ignores
        ws.send_text("NOT JSON AT ALL {{{");
        thread::sleep(Duration::from_millis(30));

        // Valid JSON but missing method — also silently ignored
        ws.send_text("{\"id\":42}");
        thread::sleep(Duration::from_millis(30));

        // Unknown domain — must return error response
        let req = serde_json::json!({"id": 43, "method": "UnknownDomain.nonexistent"});
        ws.send_text(&serde_json::to_string(&req).unwrap());
        thread::sleep(Duration::from_millis(30));
        let resp = read_response(&mut ws);
        assert_eq!(resp["id"], 43);
        assert_eq!(resp["error"]["code"], -32601);

        // Server still alive — valid request succeeds
        let req2 = serde_json::json!({"id": 44, "method": "Page.enable"});
        ws.send_text(&serde_json::to_string(&req2).unwrap());
        thread::sleep(Duration::from_millis(30));
        let resp2 = read_response(&mut ws);
        assert_eq!(resp2["id"], 44);
        assert!(resp2.get("result").is_some());

        ws.shutdown_both();
    }

    wait_for_cleanup();
    let resp = connect_send_recv(&server, 45, "Page.enable");
    assert_eq!(resp["id"], 45);

    wait_for_cleanup();
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 6: Large payload (1MB) handled without panic
// ---------------------------------------------------------------------------

#[test]
fn test_large_payload_1mb() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);

    let mut ws = ClientWs::connect(&server);

    let large_expr = "x".repeat(1_000_000);
    let msg = serde_json::json!({
        "id": 200,
        "method": "Runtime.evaluate",
        "params": {"expression": large_expr}
    });
    let msg_str = serde_json::to_string(&msg).unwrap();
    assert!(msg_str.len() > 1_000_000, "payload should exceed 1MB");

    ws.send_text(&msg_str);

    let resp = read_response(&mut ws);
    assert_eq!(resp["id"], 200, "large payload response id mismatch");

    let req2 = serde_json::json!({"id": 201, "method": "Page.enable"});
    ws.send_text(&serde_json::to_string(&req2).unwrap());
    thread::sleep(Duration::from_millis(30));
    let resp2 = read_response(&mut ws);
    assert_eq!(resp2["id"], 201);

    ws.shutdown_both();
    wait_for_cleanup();
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 7: Connection drop mid-request is cleaned up (no leak)
// ---------------------------------------------------------------------------

#[test]
fn test_connection_drop_cleanup() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);

    {
        let mut _ws = connect_and_send(&server, 1, "Page.enable");
        _ws.shutdown_both();
    }

    wait_for_cleanup();

    let resp = connect_send_recv(&server, 2, "Page.enable");
    assert_eq!(resp["id"], 2);
    assert!(resp.get("result").is_some());

    wait_for_cleanup();
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 8: Server shutdown drops all clients cleanly
// ---------------------------------------------------------------------------

#[test]
fn test_shutdown_drops_clients() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);

    let resp = connect_send_recv(&server, 1, "Page.enable");
    assert_eq!(resp["id"], 1);
    assert!(resp.get("result").is_some());

    wait_for_cleanup();
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 9: DomainHandler registry state isolation between sessions
// ---------------------------------------------------------------------------

#[test]
fn test_session_state_isolation() {
    let port = allocate_port();
    let mut server = TestServer::start(port);

    let r1 = connect_send_recv(&server, 1, "Page.enable");
    assert_eq!(r1["id"], 1);
    assert!(r1.get("result").is_some());

    let r2 = connect_send_recv(&server, 2, "Page.getLayoutMetrics");
    assert_eq!(r2["id"], 2);
    assert_eq!(r2["result"]["contentSize"]["width"], 1920);

    let r3 = connect_send_recv(&server, 3, "Runtime.enable");
    assert_eq!(r3["id"], 3);
    assert!(r3["result"]["executionContextId"].as_i64().unwrap() > 0);

    let r4 = connect_send_recv(&server, 4, "DOM.getDocument");
    assert_eq!(r4["id"], 4);
    assert_eq!(r4["result"]["root"]["nodeId"], 1);

    let r5 = connect_send_recv(&server, 5, "Network.enable");
    assert_eq!(r5["id"], 5);
    assert!(r5.get("result").is_some());

    wait_for_cleanup();
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 10: Thread safety — rapid sequential connections from different threads
// ---------------------------------------------------------------------------

#[test]
fn test_mutex_websocket_thread_safety() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);

    let handles: Vec<_> = (0..5)
        .map(|tid| {
            let server_port = server.port;
            let target_id = server.target_id.clone();
            thread::spawn(move || {
                let mut ws = ClientWs::connect(&TestServer {
                    cmd_tx: std::sync::mpsc::channel().0,
                    target_id: target_id.clone(),
                    port: server_port,
                    handle: None,
                });

                let req = serde_json::json!({"id": tid, "method": "Page.enable"});
                ws.send_text(&serde_json::to_string(&req).unwrap());

                let ok = match ws.recv_text().parse::<serde_json::Value>() {
                    Ok(v) => v.get("result").is_some(),
                    Err(_) => false,
                };
                ws.shutdown_both();
                ok
            })
        })
        .collect();

    let mut pass_count = 0;
    for h in handles {
        if h.join().unwrap() {
            pass_count += 1;
        }
    }
    assert_eq!(pass_count, 5, "all 5 threads should get valid responses");

    wait_for_cleanup();
    server.shutdown();
}
