//! TASK-2 integration tests for InMemoryTransport.
//!
//! Validates DEC-CDP-002: crossbeam-channel-like / std::sync::mpsc bridge
//! between CDP client and servo ScriptThread (!Send). Uses mock InMemoryBridge
//! implementations (TASK-3 will swap in real CDPRdpBridge).
//!
//! @trace REQ-BAO-API-002 [interface:Transport]

use std::sync::Arc;
use std::time::Duration;

use bao_cdp_client::transport::{
    CdpEvent, InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport, Transport, TransportKind,
};
use serde_json::{json, Value};

/// Mock bridge that returns Ok({"result": "<method>"}) for any command.
struct EchoMethodBridge;

impl InMemoryBridge for EchoMethodBridge {
    fn dispatch_command(
        &self,
        method: &str,
        _params: Value,
        _session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        InMemoryBridgeResponse::Ok(json!({"result": method}))
    }
}

/// Mock bridge that returns Err for any command.
struct FailingBridge {
    msg: String,
}

impl InMemoryBridge for FailingBridge {
    fn dispatch_command(
        &self,
        _method: &str,
        _params: Value,
        _session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        InMemoryBridgeResponse::Err(self.msg.clone())
    }
}

#[test]
fn in_memory_transport_kind() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let t = InMemoryTransport::new(bridge);
    // Act
    // Assert
    assert_eq!(t.kind(), TransportKind::InMemory);
}

#[test]
fn in_memory_transport_send_command_echo() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    // Act
    let r = t
        .send_command("Page.navigate", json!({"url": "about:blank"}), None)
        .unwrap();
    // Assert
    assert_eq!(r["result"], "Page.navigate");
}

#[test]
fn in_memory_transport_send_command_session_id() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    // Act
    let r = t
        .send_command("Page.navigate", json!({}), Some("TARGET-1"))
        .unwrap();
    // Assert
    assert_eq!(r["result"], "Page.navigate");
}

#[test]
fn in_memory_transport_command_error_propagates() {
    // Arrange
    let bridge = Arc::new(FailingBridge {
        // Act
        msg: "method not implemented".into(),
    });
    let mut t = InMemoryTransport::new(bridge);
    let err = t
        .send_command("Unknown.method", json!({}), None)
        .unwrap_err();
    let s = err.to_string();
    // Assert
    assert!(s.contains("CDP protocol error"), "got: {}", s);
    assert!(s.contains("method not implemented"), "got: {}", s);
}

#[test]
fn in_memory_transport_close_then_send_returns_connection_closed() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    // Act
    t.close().unwrap();
    let err = t.send_command("X", json!({}), None).unwrap_err();
    // Assert
    assert!(matches!(err, bao_cdp_client::CdpError::ConnectionClosed));
}

#[test]
fn in_memory_transport_recv_event_returns_pushed_event() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    let sender = t.event_sender();
    sender
        .send(CdpEvent::new(
            "Page.frameNavigated",
            // Act
            json!({"url": "https://example.com"}),
        ))
        .unwrap();
    let ev = t.recv_event().unwrap().expect("expected event");
    // Assert
    assert_eq!(ev.method, "Page.frameNavigated");
    assert_eq!(ev.params["url"], "https://example.com");
    assert!(ev.session_id.is_none());
}

#[test]
fn in_memory_transport_recv_event_with_session_id() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    let sender = t.event_sender();
    sender
        .send(
            // Act
            CdpEvent::new("Network.requestWillBeSent", json!({})).with_session("TARGET-7"),
        )
        .unwrap();
    let ev = t.recv_event().unwrap().expect("expected event");
    // Assert
    assert_eq!(ev.method, "Network.requestWillBeSent");
    assert_eq!(ev.session_id.as_deref(), Some("TARGET-7"));
}

#[test]
fn in_memory_transport_recv_event_timeout_returns_none() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    // Default event_timeout = 100ms; no event pushed → Ok(None) after 100ms.
    // Act
    let start = std::time::Instant::now();
    let ev = t.recv_event().unwrap();
    let elapsed = start.elapsed();
    // Assert
    assert!(ev.is_none());
    assert!(elapsed.as_millis() >= 50, "elapsed: {:?}", elapsed);
}

#[test]
fn in_memory_transport_recv_event_after_close() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    // Act
    t.close().unwrap();
    let err = t.recv_event().unwrap_err();
    // Assert
    assert!(matches!(err, bao_cdp_client::CdpError::ConnectionClosed));
}

#[test]
fn in_memory_transport_close_is_idempotent() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    // Act
    t.close().unwrap();
    t.close().unwrap();
    // Assert
    t.close().unwrap();
}

#[test]
fn in_memory_transport_set_command_timeout_documented() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    // Act
    t.set_command_timeout(Duration::from_secs(10));
    // Assert — 无运行时 assert,编译通过即验证方法签名(set_command_timeout 接受 Duration)
}

#[test]
fn in_memory_transport_set_event_timeout_affects_recv() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    // Act
    t.set_event_timeout(Duration::from_millis(5));
    let start = std::time::Instant::now();
    let ev = t.recv_event().unwrap();
    let elapsed = start.elapsed();
    // Assert
    assert!(ev.is_none());
    assert!(elapsed.as_millis() < 200, "elapsed: {:?}", elapsed);
}

/// Bridge that records all dispatches into a shared Vec (for history inspection).
use std::sync::Mutex;

struct RecordingBridge {
    history: Mutex<Vec<(String, Value, Option<String>)>>,
}

impl RecordingBridge {
    fn new() -> Self {
        Self {
            history: Mutex::new(Vec::new()),
        }
    }
}

impl InMemoryBridge for RecordingBridge {
    fn dispatch_command(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        self.history.lock().unwrap().push((
            method.to_string(),
            params.clone(),
            session_id.map(|s| s.to_string()),
        ));
        InMemoryBridgeResponse::Ok(json!({"ok": true}))
    }
}

#[test]
fn in_memory_transport_records_command_history() {
    // Arrange
    let bridge = Arc::new(RecordingBridge::new());
    // Act
    let weak = Arc::downgrade(&bridge);
    let mut t = InMemoryTransport::new(bridge);
    t.send_command("A", json!({"x": 1}), None).unwrap();
    t.send_command("B", json!({"y": 2}), Some("SID")).unwrap();
    let history = weak.upgrade().unwrap();
    let h = history.history.lock().unwrap();
    // Assert
    assert_eq!(h.len(), 2);
    assert_eq!(h[0].0, "A");
    assert_eq!(h[0].1["x"], 1);
    assert!(h[0].2.is_none());
    assert_eq!(h[1].0, "B");
    assert_eq!(h[1].2.as_deref(), Some("SID"));
}

#[test]
fn in_memory_transport_event_order_preserved_fifo() {
    // Arrange
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    let sender = t.event_sender();
    for i in 0..5 {
        sender
            .send(CdpEvent::new(
                "X.y",
                // Act
                json!({"index": i}),
            ))
            .unwrap();
    }
    for expected in 0..5 {
        let ev = t.recv_event().unwrap().expect("expected event");
        // Assert
        assert_eq!(ev.params["index"], expected);
    }
}
