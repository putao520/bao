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

// ────────────────────────────────────────────────────────────────────────────
// command_timeout honest contract — slow commands return bounded Timeout
// errors instead of hanging on the synchronous bridge dispatch.
// ────────────────────────────────────────────────────────────────────────────

/// Bridge whose dispatch blocks longer than any test timeout.
struct SlowBridge {
    delay: Duration,
}

impl InMemoryBridge for SlowBridge {
    fn dispatch_command(
        &self,
        _method: &str,
        _params: Value,
        _session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        std::thread::sleep(self.delay);
        InMemoryBridgeResponse::Ok(json!({"slow": true}))
    }
}

/// Bridge whose first dispatch is slow, subsequent ones instant — lets a test
/// observe the timed-out command's late completion AND keep using the
/// transport afterwards.
struct FirstSlowBridge {
    first_delay: Duration,
    calls: std::sync::atomic::AtomicUsize,
}

impl InMemoryBridge for FirstSlowBridge {
    fn dispatch_command(
        &self,
        _method: &str,
        _params: Value,
        _session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            std::thread::sleep(self.first_delay);
        }
        InMemoryBridgeResponse::Ok(json!({"call": n + 1}))
    }
}

/// Bridge that panics inside dispatch — the transport must re-raise the panic
/// on the caller thread (same observable behavior as an in-place dispatch).
struct PanickingBridge;

impl InMemoryBridge for PanickingBridge {
    fn dispatch_command(
        &self,
        _method: &str,
        _params: Value,
        _session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        panic!("bridge exploded");
    }
}

#[test]
fn in_memory_transport_slow_command_times_out_with_timeout_error() {
    // Arrange — bridge needs 30s, timeout is 50ms.
    let bridge = Arc::new(SlowBridge {
        delay: Duration::from_secs(30),
    });
    let mut t = InMemoryTransport::new(bridge);
    t.set_command_timeout(Duration::from_millis(50));
    // Act
    let start = std::time::Instant::now();
    let err = t.send_command("Page.navigate", json!({}), None).unwrap_err();
    let elapsed = start.elapsed();
    // Assert — bounded Timeout error, not a hang and not a bridge response.
    assert!(
        matches!(err, bao_cdp_client::CdpError::Timeout(_)),
        "got: {:?}",
        err
    );
    assert!(elapsed < Duration::from_secs(5), "elapsed: {:?}", elapsed);
    assert!(elapsed >= Duration::from_millis(40), "elapsed: {:?}", elapsed);
}

#[test]
fn in_memory_transport_timeout_error_names_method_and_duration() {
    // Arrange
    let bridge = Arc::new(SlowBridge {
        delay: Duration::from_secs(30),
    });
    let mut t = InMemoryTransport::new(bridge);
    t.set_command_timeout(Duration::from_millis(50));
    // Act
    let err = t
        .send_command("Runtime.evaluate", json!({}), None)
        .unwrap_err();
    // Assert
    let s = err.to_string();
    assert!(s.contains("Runtime.evaluate"), "got: {}", s);
    assert!(s.contains("timeout"), "got: {}", s);
    assert!(s.contains("50ms"), "got: {}", s);
}

#[test]
fn in_memory_transport_fast_command_unaffected_by_timeout_wiring() {
    // Arrange — generous timeout, instant bridge: the bounded path must stay
    // a synchronous request/response for fast commands.
    let bridge = Arc::new(EchoMethodBridge);
    let mut t = InMemoryTransport::new(bridge);
    t.set_command_timeout(Duration::from_secs(5));
    // Act
    let r = t
        .send_command("Page.navigate", json!({"url": "about:blank"}), Some("S1"))
        .unwrap();
    // Assert
    assert_eq!(r["result"], "Page.navigate");
}

#[test]
fn in_memory_transport_usable_after_timeout_late_response_not_misrouted() {
    // Arrange — first dispatch outlives the timeout, the next one is instant.
    let bridge = Arc::new(FirstSlowBridge {
        first_delay: Duration::from_millis(250),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut t = InMemoryTransport::new(bridge);
    t.set_command_timeout(Duration::from_millis(50));
    // Act — first command times out...
    let err = t.send_command("A", json!({}), None).unwrap_err();
    assert!(
        matches!(err, bao_cdp_client::CdpError::Timeout(_)),
        "got: {:?}",
        err
    );
    // ...wait out the detached worker so its late response has landed (and
    // been dropped), then prove the transport still dispatches normally.
    std::thread::sleep(Duration::from_millis(300));
    let r = t.send_command("B", json!({}), None).unwrap();
    // Assert — second command got ITS OWN response (call #2), not the late
    // response of the timed-out first command (call #1).
    assert_eq!(r["call"], 2);
}

#[test]
#[should_panic(expected = "bridge exploded")]
fn in_memory_transport_bridge_panic_propagates_to_caller() {
    let bridge = Arc::new(PanickingBridge);
    let mut t = InMemoryTransport::new(bridge);
    let _ = t.send_command("X", json!({}), None);
}
