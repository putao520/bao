//! TASK-2 integration tests for WebSocketTransport + client_handshake.
//!
//! Validates REQ-BAO-API-002: WebSocketTransport reuses bao_cdp::ws_codec +
//! bao_cdp::ws_handshake for CDP client mode (connecting to external Chrome).
//!
//! Tests spin up a minimal WebSocket server in-process using bao_cdp's own
//! server_handshake, then drive the client side.
//!
//! @trace REQ-BAO-API-002 [interface:Transport]

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use bao_cdp_client::transport::{Transport, TransportKind, WebSocketTransport};
use bao_cdp_client::CdpError;
use serde_json::{json, Value};

/// Apply RFC 6455 §5.3 mask: payload[i] ^= mask[i % 4].
fn apply_mask(payload: &mut [u8], mask: &[u8; 4]) {
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
}

/// Server-side text frame encoder (unmasked, RFC 6455 §5.1 for server→client).
fn encode_text_unmasked(payload: &str) -> Vec<u8> {
    let opcode = 0x1u8; // Text
    let fin = 0x80u8;
    let payload_bytes = payload.as_bytes();
    let len = payload_bytes.len();
    let mut buf = Vec::with_capacity(payload_bytes.len() + 14);
    buf.push(fin | opcode);
    if len < 126 {
        buf.push(len as u8);
    } else if len <= u16::MAX as usize {
        buf.push(126u8);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(127u8);
        buf.extend_from_slice(&(len as u64).to_be_bytes());
    }
    buf.extend_from_slice(payload_bytes);
    buf
}

/// Minimal WebSocket server that:
/// 1. Completes server_handshake
/// 2. Reads N text frames, returns JSON response for each (with id echoed)
/// 3. Optionally pushes events before reading
struct MiniCdpServer {
    addr: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl MiniCdpServer {
    fn new<F>(handler: F) -> Self
    where
        F: FnOnce(TcpStream) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handler(stream);
            }
        });
        Self {
            addr,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("ws://{}/devtools/page/test", self.addr)
    }
}

impl Drop for MiniCdpServer {
    fn drop(&mut self) {
        // Don't block test teardown; just detach.
        if let Some(h) = self.handle.take() {
            h.join().ok();
        }
    }
}

/// Echo server: reads one frame, echoes back JSON response with id.
fn echo_handler(mut stream: TcpStream) {
    if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
        return;
    }
    let mut decoder = bao_cdp::ws_codec::FrameDecoder::new();
    let header = match decoder.decode_frame(&mut stream) {
        Ok(Some(h)) => h,
        _ => return,
    };
    let payload = if header.mask {
        let mask = decoder.take_mask();
        let mut p = decoder.take_payload(&header);
        apply_mask(&mut p, &mask);
        p
    } else {
        decoder.take_payload(&header)
    };
    let v: Value = match serde_json::from_slice(&payload) {
        Ok(v) => v,
        Err(_) => return,
    };
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let method = v.get("method").cloned().unwrap_or(Value::Null);
    let response = json!({
        "id": id,
        "result": {"echoedMethod": method},
    });
    let resp_json = serde_json::to_string(&response).unwrap();
    let frame = encode_text_unmasked(&resp_json);
    let _ = stream.write_all(&frame);
    let _ = stream.flush();
    // Hold socket briefly to allow client to close gracefully.
    thread::sleep(Duration::from_millis(50));
}

#[test]
fn ws_transport_kind() {
    let server = MiniCdpServer::new(echo_handler);
    let t = WebSocketTransport::connect(&server.url()).unwrap();
    assert_eq!(t.kind(), TransportKind::WebSocket);
}

#[test]
fn ws_transport_connect_handshake_succeeds() {
    let server = MiniCdpServer::new(echo_handler);
    let mut t = WebSocketTransport::connect(&server.url()).expect("handshake");
    let result = t.send_command("Page.navigate", json!({}), None).expect("send");
    assert_eq!(result["echoedMethod"], "Page.navigate");
}

#[test]
fn ws_transport_send_command_increments_id() {
    let server = MiniCdpServer::new(|mut stream| {
        // Server responds to each frame; we serve 3 frames.
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        let mut decoder = bao_cdp::ws_codec::FrameDecoder::new();
        for _ in 0..3 {
            let header = match decoder.decode_frame(&mut stream) {
                Ok(Some(h)) => h,
                _ => return,
            };
            let payload = if header.mask {
                let mask = decoder.take_mask();
                let mut p = decoder.take_payload(&header);
                apply_mask(&mut p, &mask);
                p
            } else {
                decoder.take_payload(&header)
            };
            let v: Value = match serde_json::from_slice(&payload) {
                Ok(v) => v,
                Err(_) => return,
            };
            let id = v.get("id").cloned().unwrap_or(Value::Null);
            let response = json!({"id": id, "result": {"ok": true}});
            let frame = encode_text_unmasked(&serde_json::to_string(&response).unwrap());
            let _ = stream.write_all(&frame);
            let _ = stream.flush();
        }
        thread::sleep(Duration::from_millis(50));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    assert_eq!(t.current_id(), 1);
    t.send_command("A", json!({}), None).unwrap();
    assert_eq!(t.current_id(), 2);
    t.send_command("B", json!({}), None).unwrap();
    assert_eq!(t.current_id(), 3);
    t.send_command("C", json!({}), None).unwrap();
    assert_eq!(t.current_id(), 4);
}

#[test]
fn ws_transport_send_command_with_session_id() {
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        let mut decoder = bao_cdp::ws_codec::FrameDecoder::new();
        let header = match decoder.decode_frame(&mut stream) {
            Ok(Some(h)) => h,
            _ => return,
        };
        let payload = if header.mask {
            let mask = decoder.take_mask();
            let mut p = decoder.take_payload(&header);
            apply_mask(&mut p, &mask);
            p
        } else {
            decoder.take_payload(&header)
        };
        let v: Value = serde_json::from_slice(&payload).unwrap();
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        // Echo session_id back in result for assertion.
        let session = v.get("sessionId").cloned().unwrap_or(Value::Null);
        let response = json!({"id": id, "result": {"echoedSession": session}});
        let frame = encode_text_unmasked(&serde_json::to_string(&response).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(50));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    let r = t.send_command("X.y", json!({}), Some("TARGET-42")).unwrap();
    assert_eq!(r["echoedSession"], "TARGET-42");
}

#[test]
fn ws_transport_recv_event_gets_pushed() {
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // Push one event.
        let event = json!({
            "method": "Page.frameNavigated",
            "params": {"url": "https://example.com"},
        });
        let frame = encode_text_unmasked(&serde_json::to_string(&event).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.set_event_timeout(Duration::from_secs(2));
    let ev = t.recv_event().unwrap().expect("expected event");
    assert_eq!(ev.method, "Page.frameNavigated");
    assert_eq!(ev.params["url"], "https://example.com");
}

#[test]
fn ws_transport_recv_event_with_session_id() {
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        let event = json!({
            "method": "Network.requestWillBeSent",
            "params": {},
            "sessionId": "TARGET-7",
        });
        let frame = encode_text_unmasked(&serde_json::to_string(&event).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.set_event_timeout(Duration::from_secs(2));
    let ev = t.recv_event().unwrap().expect("expected event");
    assert_eq!(ev.method, "Network.requestWillBeSent");
    assert_eq!(ev.session_id.as_deref(), Some("TARGET-7"));
}

#[test]
fn ws_transport_recv_event_returns_none_on_timeout() {
    let server = MiniCdpServer::new(|mut stream| {
        let _ = bao_cdp::ws_handshake::server_handshake(&mut stream);
        thread::sleep(Duration::from_millis(500));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.set_event_timeout(Duration::from_millis(20));
    let start = std::time::Instant::now();
    let ev = t.recv_event().unwrap();
    let elapsed = start.elapsed();
    assert!(ev.is_none());
    assert!(elapsed.as_millis() < 200, "elapsed: {:?}", elapsed);
}

#[test]
fn ws_transport_close_returns_connection_closed_after() {
    let server = MiniCdpServer::new(echo_handler);
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.close().unwrap();
    let err = t.send_command("X", json!({}), None).unwrap_err();
    assert!(matches!(err, CdpError::ConnectionClosed));
}

#[test]
fn ws_transport_close_idempotent() {
    let server = MiniCdpServer::new(echo_handler);
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.close().unwrap();
    t.close().unwrap();
    t.close().unwrap();
}

#[test]
fn ws_transport_connect_invalid_url_returns_handshake_error() {
    let err = WebSocketTransport::connect("not a url").unwrap_err();
    assert!(matches!(err, CdpError::HandshakeError(_)));
}

#[test]
fn ws_transport_connect_refused_returns_error() {
    // Use a port that's almost certainly closed.
    let err = WebSocketTransport::connect("ws://127.0.0.1:1/x").unwrap_err();
    // Could be IoError (refused) or HandshakeError — both acceptable.
    match err {
        CdpError::IoError(_) | CdpError::HandshakeError(_) => {}
        other => panic!("expected io/handshake err, got {:?}", other),
    }
}

#[test]
fn ws_transport_recv_event_after_close_returns_connection_closed() {
    let server = MiniCdpServer::new(echo_handler);
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.close().unwrap();
    let err = t.recv_event().unwrap_err();
    assert!(matches!(err, CdpError::ConnectionClosed));
}

/// Round-trip test: send command, server pushes event, then second command.
#[test]
fn ws_transport_interleaved_command_and_event() {
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // Push one unsolicited event first.
        let event = json!({"method": "Log.entryAdded", "params": {"text": "hi"}});
        let frame = encode_text_unmasked(&serde_json::to_string(&event).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();

        // Then handle one command.
        let mut decoder = bao_cdp::ws_codec::FrameDecoder::new();
        let header = match decoder.decode_frame(&mut stream) {
            Ok(Some(h)) => h,
            _ => return,
        };
        let payload = if header.mask {
            let mask = decoder.take_mask();
            let mut p = decoder.take_payload(&header);
            apply_mask(&mut p, &mask);
            p
        } else {
            decoder.take_payload(&header)
        };
        let v: Value = serde_json::from_slice(&payload).unwrap();
        let response = json!({"id": v["id"], "result": {"ok": true}});
        let frame = encode_text_unmasked(&serde_json::to_string(&response).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    // send_command should succeed — but it should also queue the unsolicited event.
    let r = t.send_command("X", json!({}), None).unwrap();
    assert_eq!(r["ok"], true);
    // The event pushed before our command should be queued for recv_event.
    let ev = t.recv_event().unwrap().expect("expected queued event");
    assert_eq!(ev.method, "Log.entryAdded");
    assert_eq!(ev.params["text"], "hi");
}

#[test]
fn ws_transport_json_rpc_error_response() {
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        let mut decoder = bao_cdp::ws_codec::FrameDecoder::new();
        let header = match decoder.decode_frame(&mut stream) {
            Ok(Some(h)) => h,
            _ => return,
        };
        let payload = if header.mask {
            let mask = decoder.take_mask();
            let mut p = decoder.take_payload(&header);
            apply_mask(&mut p, &mask);
            p
        } else {
            decoder.take_payload(&header)
        };
        let v: Value = serde_json::from_slice(&payload).unwrap();
        // Return JSON-RPC error -32601 (method not found).
        let response = json!({
            "id": v["id"],
            "error": {"code": -32601, "message": "method not found"},
        });
        let frame = encode_text_unmasked(&serde_json::to_string(&response).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(50));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    let err = t.send_command("Unknown", json!({}), None).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("CDP protocol error"), "got: {}", s);
}

#[test]
fn ws_transport_set_command_timeout_zero_means_immediate() {
    let server = MiniCdpServer::new(|mut stream| {
        let _ = bao_cdp::ws_handshake::server_handshake(&mut stream);
        // Don't respond — client should timeout.
        thread::sleep(Duration::from_millis(500));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.set_command_timeout(Duration::from_millis(1));
    let err = t.send_command("X", json!({}), None).unwrap_err();
    // Either Timeout or IoError (reset) acceptable.
    match err {
        CdpError::Timeout(_) | CdpError::IoError(_) | CdpError::ConnectionClosed => {}
        other => panic!("expected timeout/io/closed, got: {:?}", other),
    }
}
