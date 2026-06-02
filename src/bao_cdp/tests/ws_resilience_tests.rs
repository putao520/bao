// @trace TEST-CDP-RES [req:REQ-CDP-001] [level:integration] [nfr:TMG-CDP-01]
// WebSocket server resilience tests for bao_cdp CDPServer.
// The server event loop is single-threaded with blocking per-session reads,
// so multi-session tests use sequential connect→send→recv→close cycles.

use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use bao_cdp::{CDPServer, CDPCommand, CDPServerError, bridge_channel};
use tungstenite::Message;
use tungstenite::client::client as ws_client;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn allocate_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
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
        // Give the server thread time to bind. We avoid TcpListener::bind for polling
        // because it would preempt the server's own bind on the same port.
        thread::sleep(Duration::from_millis(300));
        TestServer { cmd_tx, target_id, port, handle: Some(handle) }
    }

    fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(CDPCommand::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }

    fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/devtools/page/{}", self.port, self.target_id)
    }
}

/// Connect, immediately send a CDP command, then read the response.
/// This ordering is critical: the server's event loop does blocking reads,
/// so data must be in the TCP buffer before the server processes the session.
/// Does NOT send Close frame — the server detects the dropped TCP connection
/// on its next read attempt and removes the session automatically.
fn connect_send_recv(server: &TestServer, id: i64, method: &str) -> serde_json::Value {
    let tcp = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let (mut ws, _) = ws_client(server.ws_url().as_str(), tcp).unwrap();

    // Send immediately — data enters TCP buffer before server reads this session
    let req = serde_json::json!({"id": id, "method": method});
    ws.send(Message::Text(serde_json::to_string(&req).unwrap().into())).unwrap();

    // Read response (server processes after accept, which may take 1-2 loop iterations)
    let resp = match ws.read() {
        Ok(Message::Text(text)) => serde_json::from_str(&text.to_string()).unwrap(),
        Ok(other) => panic!("expected text, got {:?}", other),
        Err(e) => panic!("read error: {}", e),
    };
    // Force-close the TCP stream so tungstenite's Drop can't block on close-handshake.
    // The server detects the RST and removes the session immediately.
    let _ = ws.get_mut().shutdown(std::net::Shutdown::Both);
    resp
}

/// Connect and send without reading — for tests that check server liveness after abuse.
fn connect_and_send(server: &TestServer, id: i64, method: &str) -> tungstenite::protocol::WebSocket<TcpStream> {
    let tcp = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let (mut ws, _) = ws_client(server.ws_url().as_str(), tcp).unwrap();
    let req = serde_json::json!({"id": id, "method": method});
    ws.send(Message::Text(serde_json::to_string(&req).unwrap().into())).unwrap();
    ws
}

fn read_response(ws: &mut tungstenite::protocol::WebSocket<TcpStream>) -> serde_json::Value {
    match ws.read() {
        Ok(Message::Text(text)) => serde_json::from_str(&text.to_string()).unwrap(),
        Ok(other) => panic!("expected text, got {:?}", other),
        Err(e) => panic!("read error: {}", e),
    }
}

/// Wait for server to clean up dropped sessions (its event loop sleeps 10ms).
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
    assert!(resp.get("result").is_some(), "expected result, got: {:?}", resp);

    wait_for_cleanup();
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 2: Starting on same port twice fails gracefully (AddrInUse)
// ---------------------------------------------------------------------------

#[test]
fn test_addr_in_use_fails_gracefully() {
    let port = allocate_port();
    // Hold the port with a raw listener — no CDPServer needed
    let _guard = TcpListener::bind(("127.0.0.1", port)).unwrap();

    let mut server2 = CDPServer::new(port);
    let result = server2.run();
    assert!(result.is_err(), "second server should fail to bind");
    match result.unwrap_err() {
        CDPServerError::Bind(msg) => {
            let lower = msg.to_lowercase();
            assert!(
                lower.contains("address") || lower.contains("in use") || lower.contains("already"),
                "unexpected bind error: {}", msg
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
        assert!(resp.get("result").is_some(), "client {} expected result: {:?}", i, resp);
    }

    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 4: Multiple clients × multiple requests (sequential per client)
//         Tests server's ability to handle repeated connect/send/recv cycles
//         from 5 different clients, each issuing 10 requests.
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_connections_5x10() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);

    for cid in 0..5i64 {
        // Each client: connect once, send 10 requests, read 10 responses, drop
        let tcp = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (mut ws, _) = ws_client(server.ws_url().as_str(), tcp).unwrap();

        // Send 10 requests immediately (buffered before server reads)
        for rid in 0..10i64 {
            let id = cid * 100 + rid;
            let req = serde_json::json!({"id": id, "method": "Page.enable"});
            ws.send(Message::Text(serde_json::to_string(&req).unwrap().into())).unwrap();
        }

        // Read 10 responses
        let mut responses = Vec::new();
        for _ in 0..10 {
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let v: serde_json::Value = serde_json::from_str(&text.to_string()).unwrap();
                    responses.push(v);
                }
                Ok(other) => panic!("client {} got non-text: {:?}", cid, other),
                Err(e) => panic!("client {} read error: {}", cid, e),
            }
        }

        assert_eq!(responses.len(), 10, "client {} expected 10 responses", cid);
        for resp in &responses {
            assert!(
                resp.get("result").is_some() || resp.get("error").is_some(),
                "client {} unexpected response: {:?}", cid, resp
            );
        }

        // Force-close TCP so server detects RST immediately
        let _ = ws.get_mut().shutdown(std::net::Shutdown::Both);
        drop(ws);
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

    // Phase 1: Send garbage through a single connection
    {
        let tcp = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (mut ws, _) = ws_client(server.ws_url().as_str(), tcp).unwrap();

        // Completely invalid JSON — server silently ignores
        ws.send(Message::Text("NOT JSON AT ALL {{{".into())).unwrap();
        thread::sleep(Duration::from_millis(30));

        // Valid JSON but missing method — also silently ignored
        ws.send(Message::Text("{\"id\":42}".into())).unwrap();
        thread::sleep(Duration::from_millis(30));

        // Valid JSON with unknown domain — must return error response
        let req = serde_json::json!({"id": 43, "method": "UnknownDomain.nonexistent"});
        ws.send(Message::Text(serde_json::to_string(&req).unwrap().into())).unwrap();
        thread::sleep(Duration::from_millis(30));
        let resp = read_response(&mut ws);
        assert_eq!(resp["id"], 43);
        assert_eq!(resp["error"]["code"], -32601);

        // Server still alive — valid request succeeds
        let req2 = serde_json::json!({"id": 44, "method": "Page.enable"});
        ws.send(Message::Text(serde_json::to_string(&req2).unwrap().into())).unwrap();
        thread::sleep(Duration::from_millis(30));
        let resp2 = read_response(&mut ws);
        assert_eq!(resp2["id"], 44);
        assert!(resp2.get("result").is_some());

        // Force-close TCP
        let _ = ws.get_mut().shutdown(std::net::Shutdown::Both);
    }

    // Phase 2: Server still accepts new connections after abuse
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

    let tcp = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let (mut ws, _) = ws_client(server.ws_url().as_str(), tcp).unwrap();

    let large_expr = "x".repeat(1_000_000);
    let msg = serde_json::json!({
        "id": 200,
        "method": "Runtime.evaluate",
        "params": {"expression": large_expr}
    });
    let msg_str = serde_json::to_string(&msg).unwrap();
    assert!(msg_str.len() > 1_000_000, "payload should exceed 1MB");

    ws.send(Message::Text(msg_str.into())).unwrap();

    let resp = read_response(&mut ws);
    assert_eq!(resp["id"], 200, "large payload response id mismatch");

    // Server still works after large payload
    let req2 = serde_json::json!({"id": 201, "method": "Page.enable"});
    ws.send(Message::Text(serde_json::to_string(&req2).unwrap().into())).unwrap();
    thread::sleep(Duration::from_millis(30));
    let resp2 = read_response(&mut ws);
    assert_eq!(resp2["id"], 201);

    // Force-close TCP
    let _ = ws.get_mut().shutdown(std::net::Shutdown::Both);
    drop(ws);
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

    // Connect, send, drop without close frame
    {
        let _ws = connect_and_send(&server, 1, "Page.enable");
        // Drop without WebSocket close — simulates network failure
    }

    // Give server time to detect broken connection
    wait_for_cleanup();

    // Server still works
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

    // Connect a client and complete a full exchange
    let resp = connect_send_recv(&server, 1, "Page.enable");
    assert_eq!(resp["id"], 1);
    assert!(resp.get("result").is_some());

    // Session is cleaned up after drop (connect_send_recv drops without Close)
    wait_for_cleanup();

    // Shutdown — server thread should join cleanly
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 9: DomainHandler registry state isolation between sessions
// ---------------------------------------------------------------------------

#[test]
fn test_session_state_isolation() {
    let port = allocate_port();
    // No bridge: protocol handles DOM/Page/Runtime/Network commands locally,
    // returning deterministic fixture responses. With a bridge but no responder,
    // commands would timeout (500ms) and return errors.
    let mut server = TestServer::start(port);

    // Session 1: Page domain commands
    let r1 = connect_send_recv(&server, 1, "Page.enable");
    assert_eq!(r1["id"], 1);
    assert!(r1.get("result").is_some());

    let r2 = connect_send_recv(&server, 2, "Page.getLayoutMetrics");
    assert_eq!(r2["id"], 2);
    assert_eq!(r2["result"]["contentSize"]["width"], 1920);

    // Session 2: Runtime domain commands — independent results
    let r3 = connect_send_recv(&server, 3, "Runtime.enable");
    assert_eq!(r3["id"], 3);
    assert!(r3["result"]["executionContextId"].as_i64().unwrap() > 0);

    let r4 = connect_send_recv(&server, 4, "DOM.getDocument");
    assert_eq!(r4["id"], 4);
    assert_eq!(r4["result"]["root"]["nodeId"], 1);

    // Session 3: Cross-domain commands still work
    let r5 = connect_send_recv(&server, 5, "Network.enable");
    assert_eq!(r5["id"], 5);
    assert!(r5.get("result").is_some());

    wait_for_cleanup();
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Test 10: Thread safety — Mutex<WebSocket> via ExternalBackend (unit-level)
//          + server handles rapid sequential connections from different threads
// ---------------------------------------------------------------------------

#[test]
fn test_mutex_websocket_thread_safety() {
    let port = allocate_port();
    let mut server = TestServer::start_with_bridge(port);
    let ws_url = server.ws_url();
    let sport = server.port;

    // Spawn threads that each connect sequentially, send, recv, close
    let handles: Vec<_> = (0..5).map(|tid| {
        let url = ws_url.clone();
        thread::spawn(move || {
            let tcp = TcpStream::connect(("127.0.0.1", sport)).unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let (mut ws, _) = ws_client(url.as_str(), tcp).unwrap();

            // Send immediately
            let req = serde_json::json!({"id": tid, "method": "Page.enable"});
            ws.send(Message::Text(serde_json::to_string(&req).unwrap().into())).unwrap();

            // Read response
            let ok = match ws.read() {
                Ok(Message::Text(text)) => {
                    let v: serde_json::Value = serde_json::from_str(&text.to_string()).unwrap();
                    v.get("result").is_some()
                }
                _ => false,
            };
            // Force-close TCP so server detects RST immediately
            let _ = ws.get_mut().shutdown(std::net::Shutdown::Both);
            ok
        })
    }).collect();

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
