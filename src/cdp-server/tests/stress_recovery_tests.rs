// @trace TEST-CDS-010-STRESS [req:REQ-CDS-001,REQ-CDS-004,REQ-CDS-006,REQ-CDS-007] [level:stress]
// Stress tests + error recovery: high-frequency dispatch, concurrent registry, session state recovery

use cdp_server::{CdpMessage, CdpError, CdpResponse, DomainRegistry, EventSender, ServerConfig};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---- Test helpers ----

#[derive(Clone)]
struct CountingSender {
    count: Arc<AtomicUsize>,
}

impl CountingSender {
    fn new() -> Self {
        CountingSender { count: Arc::new(AtomicUsize::new(0)) }
    }
    fn event_count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

impl EventSender for CountingSender {
    fn send_event(&self, _method: &str, _params: Value) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

struct StressHandler {
    name: &'static str,
    call_count: Arc<AtomicUsize>,
}

impl cdp_server::DomainHandler for StressHandler {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, cmd: &str, params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        // Extract command after domain prefix (e.g. "Domain0.echo" → "echo")
        let command = cmd.split('.').nth(1).unwrap_or("");
        match command {
            "echo" => Ok(params),
            "compute" => {
                let input = params.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
                Ok(json!({"result": input * 2}))
            }
            "error" => Err(CdpError { code: -32000, message: "intentional stress error".into() }),
            "slow" => {
                std::thread::sleep(std::time::Duration::from_micros(100));
                Ok(json!({"done": true}))
            }
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", cmd) }),
        }
    }
}

struct StatefulHandler {
    name: &'static str,
    state: Arc<AtomicUsize>,
}

impl cdp_server::DomainHandler for StatefulHandler {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, cmd: &str, _params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        match cmd {
            "Stateful.increment" => {
                let v = self.state.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"value": v + 1}))
            }
            "Stateful.get" => {
                Ok(json!({"value": self.state.load(Ordering::SeqCst)}))
            }
            "Stateful.reset" => {
                self.state.store(0, Ordering::SeqCst);
                Ok(json!({}))
            }
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", cmd) }),
        }
    }
}

// ---- High-frequency dispatch ----

#[test]
fn test_stress_1000_echo_dispatches() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let reg = DomainRegistry::new();
    reg.register(Box::new(StressHandler {
        name: "Stress",
        call_count: call_count.clone(),
    })).unwrap();

    let sender = CountingSender::new();

    for i in 0..1000u32 {
        let result = reg.dispatch_command(
            "Stress.echo",
            json!({"iteration": i}),
            &sender,
        );
        assert!(result.is_some(), "Dispatch {} should succeed", i);
        let inner = result.unwrap();
        assert!(inner.is_ok(), "Echo {} should succeed", i);
        assert_eq!(inner.unwrap()["iteration"], i);
    }

    assert_eq!(call_count.load(Ordering::Relaxed), 1000);
}

#[test]
fn test_stress_mixed_commands() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let reg = DomainRegistry::new();
    reg.register(Box::new(StressHandler {
        name: "Stress",
        call_count: call_count.clone(),
    })).unwrap();

    let sender = CountingSender::new();
    let mut ok_count = 0usize;
    let mut err_count = 0usize;

    for i in 0..500 {
        let (method, params) = if i % 3 == 0 {
            ("Stress.echo", json!({"i": i}))
        } else if i % 3 == 1 {
            ("Stress.compute", json!({"n": i}))
        } else {
            ("Stress.error", json!({}))
        };

        let result = reg.dispatch_command(method, params, &sender);
        assert!(result.is_some());
        let inner = result.unwrap();
        if inner.is_ok() {
            ok_count += 1;
        } else {
            err_count += 1;
            assert_eq!(inner.unwrap_err().code, -32000);
        }
    }

    assert_eq!(ok_count + err_count, 500);
    assert!(ok_count > 0);
    assert!(err_count > 0);
    assert_eq!(call_count.load(Ordering::Relaxed), 500);
}

#[test]
fn test_stress_unknown_domain_returns_none() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(StressHandler {
        name: "Stress",
        call_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();

    let sender = CountingSender::new();

    for _ in 0..100 {
        assert!(reg.dispatch_command("Unknown.method", json!({}), &sender).is_none());
    }
}

#[test]
fn test_stress_stateful_handler_consistency() {
    let state = Arc::new(AtomicUsize::new(0));
    let reg = DomainRegistry::new();
    reg.register(Box::new(StatefulHandler {
        name: "Stateful",
        state: state.clone(),
    })).unwrap();

    let sender = CountingSender::new();

    // Increment 100 times
    for _ in 0..100 {
        let result = reg.dispatch_command("Stateful.increment", json!({}), &sender);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }

    // Verify counter is 100
    let result = reg.dispatch_command("Stateful.get", json!({}), &sender);
    assert_eq!(result.unwrap().unwrap()["value"], 100);

    // Reset
    reg.dispatch_command("Stateful.reset", json!({}), &sender);

    // Verify reset
    let result = reg.dispatch_command("Stateful.get", json!({}), &sender);
    assert_eq!(result.unwrap().unwrap()["value"], 0);
}

// ---- Error recovery ----

#[test]
fn test_error_recovery_continues_after_error() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(StressHandler {
        name: "Stress",
        call_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();

    let sender = CountingSender::new();

    // Error
    let err_result = reg.dispatch_command("Stress.error", json!({}), &sender);
    assert!(err_result.unwrap().is_err());

    // Recovery: next command should succeed
    let ok_result = reg.dispatch_command("Stress.echo", json!({"after": "error"}), &sender);
    assert!(ok_result.unwrap().is_ok());
}

#[test]
fn test_error_recovery_unknown_command_then_known() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(StressHandler {
        name: "Stress",
        call_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();

    let sender = CountingSender::new();

    // Unknown command → -32601
    let result = reg.dispatch_command("Stress.nonexistent", json!({}), &sender);
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32601);

    // Known command works fine
    let result = reg.dispatch_command("Stress.echo", json!({"recovered": true}), &sender);
    assert!(result.unwrap().is_ok());
}

#[test]
fn test_error_recovery_sequential_errors() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(StressHandler {
        name: "Stress",
        call_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();

    let sender = CountingSender::new();

    // Multiple errors in sequence
    for _ in 0..10 {
        let result = reg.dispatch_command("Stress.error", json!({}), &sender);
        assert!(result.unwrap().is_err());
    }

    // Recovery after multiple errors
    let result = reg.dispatch_command("Stress.compute", json!({"n": 42}), &sender);
    let val = result.unwrap().unwrap();
    assert_eq!(val["result"], 84);
}

// ---- EventSender stress ----

#[test]
fn test_event_sender_count_accuracy() {
    let sender = CountingSender::new();
    let n = 500;
    for i in 0..n {
        sender.send_event("Page.loadEventFired", json!({"timestamp": i}));
    }
    assert_eq!(sender.event_count(), n);
}

#[test]
fn test_event_sender_cloned_shared_count() {
    let sender1 = CountingSender::new();
    let sender2 = sender1.clone();

    sender1.send_event("A.event", json!({}));
    sender2.send_event("B.event", json!({}));
    sender1.send_event("C.event", json!({}));

    assert_eq!(sender1.event_count(), 3);
    assert_eq!(sender2.event_count(), 3);
}

// ---- Protocol message edge cases ----

#[test]
fn test_parse_message_very_large_payload() {
    let large_data: Vec<Value> = (0..10000).map(|i| json!({"id": i, "data": "x".repeat(100)})).collect();
    let raw = json!({
        "id": 1,
        "method": "Stress.bulk",
        "params": {"items": large_data}
    }).to_string();

    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    assert_eq!(msg.method, "Stress.bulk");
    let params = msg.params.unwrap();
    let items = params["items"].as_array().unwrap();
    assert_eq!(items.len(), 10000);
}

#[test]
fn test_parse_message_unicode_method() {
    let raw = r#"{"id": 1, "method": "Page.日本語テスト"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.method, "Page.日本語テスト");
}

#[test]
fn test_parse_message_emoji_params() {
    let raw = r#"{"id": 1, "method": "Page.navigate", "params": {"url": "https://example.com/🎉"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap()["url"], "https://example.com/🎉");
}

#[test]
fn test_response_large_result() {
    let large_array: Vec<Value> = (0..5000).map(|i| json!({"index": i})).collect();
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"data": large_array})),
        error: None,
    };
    let serialized = serde_json::to_string(&resp).unwrap();
    assert!(serialized.len() > 50000);
    // Verify roundtrip by re-parsing as generic Value
    let back: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(back["result"]["data"].as_array().unwrap().len(), 5000);
}

// ---- ServerConfig stress ----

#[test]
fn test_server_config_builder_many_instances() {
    for port in 9200..9300u16 {
        let config = ServerConfig::builder()
            .host("127.0.0.1")
            .port(port)
            .build();
        assert_eq!(config.port, port);
    }
}

#[test]
fn test_server_config_builder_zero_port() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(0)
        .build();
    assert_eq!(config.port, 0);
    assert_eq!(config.host, "0.0.0.0");
}

// ---- Registry multi-domain stress ----

#[test]
fn test_registry_10_domains_dispatch() {
    let reg = DomainRegistry::new();
    let counters: Vec<Arc<AtomicUsize>> = (0..10)
        .map(|_| Arc::new(AtomicUsize::new(0)))
        .collect();

    for i in 0..10 {
        reg.register(Box::new(StressHandler {
            name: Box::leak(format!("Domain{}", i).into_boxed_str()),
            call_count: counters[i].clone(),
        })).unwrap();
    }

    let sender = CountingSender::new();

    // Dispatch to each domain 100 times
    for i in 0..10 {
        for _ in 0..100 {
            let result = reg.dispatch_command(
                &format!("Domain{}.echo", i),
                json!({"domain": i}),
                &sender,
            );
            assert!(result.is_some());
            assert!(result.unwrap().is_ok());
        }
    }

    // Verify each counter is exactly 100
    for (i, counter) in counters.iter().enumerate() {
        assert_eq!(counter.load(Ordering::Relaxed), 100, "Domain{} counter", i);
    }
}

#[test]
fn test_registry_has_domain_after_register() {
    let reg = DomainRegistry::new();
    assert!(!reg.has_domain("Test"));

    reg.register(Box::new(StressHandler {
        name: "Test",
        call_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();

    assert!(reg.has_domain("Test"));
    assert!(!reg.has_domain("test")); // case-sensitive
    assert!(!reg.has_domain("Other"));
}

// ---- SessionState transitions ----

#[test]
fn test_session_state_ordering() {
    use cdp_server::SessionState;
    // Verify the logical ordering via discriminant
    assert!((SessionState::Created as usize) < (SessionState::Active as usize));
    assert!((SessionState::Active as usize) < (SessionState::Closing as usize));
    assert!((SessionState::Closing as usize) < (SessionState::Closed as usize));
}

#[test]
fn test_session_state_equality() {
    use cdp_server::SessionState;
    assert_eq!(SessionState::Created, SessionState::Created);
    assert_ne!(SessionState::Created, SessionState::Active);
    assert_ne!(SessionState::Active, SessionState::Closed);
}

#[test]
fn test_session_state_debug() {
    use cdp_server::SessionState;
    assert!(format!("{:?}", SessionState::Created).contains("Created"));
    assert!(format!("{:?}", SessionState::Active).contains("Active"));
    assert!(format!("{:?}", SessionState::Closing).contains("Closing"));
    assert!(format!("{:?}", SessionState::Closed).contains("Closed"));
}

#[test]
fn test_session_state_copy() {
    use cdp_server::SessionState;
    let s1 = SessionState::Active;
    let s2 = s1; // Copy (SessionState derives Copy)
    assert_eq!(s1, s2);
}

// ---- CdpError stress ----

#[test]
fn test_cdp_error_various_codes() {
    let codes = [-32700, -32600, -32601, -32602, -32603, -32000, -32001];
    for code in codes {
        let err = CdpError { code, message: format!("error {}", code) };
        assert_eq!(err.code, code);
    }
}

#[test]
fn test_cdp_error_message_preservation() {
    let long_msg = "x".repeat(10000);
    let err = CdpError { code: -1, message: long_msg.clone() };
    assert_eq!(err.message.len(), 10000);

    let serialized = serde_json::to_string(&err).unwrap();
    // Verify via generic Value since CdpError may not impl Deserialize
    let back: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(back["message"].as_str().unwrap().len(), 10000);
    assert_eq!(back["code"], -1);
}
