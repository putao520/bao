// @trace TEST-CDS-CONCURRENCY [req:REQ-CDS-007] [level:unit]
// Concurrency safety tests: Arc<Mutex> DomainRegistry dispatch, EventSender thread safety

use cdp_server::{DomainRegistry, DomainHandler, EventSender, CdpError};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::thread;

struct EchoHandler;
impl DomainHandler for EchoHandler {
    fn domain_name(&self) -> &'static str { "Echo" }
    fn handle_command(&self, cmd: &str, params: Value, _sender: &dyn EventSender) -> Result<Value, CdpError> {
        match cmd {
            "Echo.ping" => Ok(json!({"pong": true, "echo": params})),
            "Echo.add" => {
                let a = params.get("a").and_then(|v| v.as_i64()).unwrap_or(0);
                let b = params.get("b").and_then(|v| v.as_i64()).unwrap_or(0);
                Ok(json!({"result": a + b}))
            }
            _ => Err(CdpError { code: -32601, message: format!("'{}' not found", cmd) }),
        }
    }
}

struct CollectingSender {
    events: Arc<Mutex<Vec<(String, Value)>>>,
}
impl EventSender for CollectingSender {
    fn send_event(&self, method: &str, params: Value) {
        self.events.lock().unwrap().push((method.to_string(), params));
    }
}

fn make_registry() -> Arc<DomainRegistry> {
    let reg = DomainRegistry::new();
    reg.register(Box::new(EchoHandler));
    Arc::new(reg)
}

#[test]
fn test_concurrent_dispatch_ping() {
    let reg = make_registry();
    let mut handles = vec![];
    for _ in 0..8 {
        let r = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let sender = CollectingSender { events: Arc::new(Mutex::new(Vec::new())) };
                let result = r.dispatch_command("Echo.ping", json!({}), &sender);
                assert!(matches!(result, Some(Ok(_))), "concurrent ping should succeed");
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_concurrent_dispatch_compute() {
    let reg = make_registry();
    let mut handles = vec![];
    for t in 0..4 {
        let r = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                let sender = CollectingSender { events: Arc::new(Mutex::new(Vec::new())) };
                let result = r.dispatch_command("Echo.add", json!({"a": t, "b": i}), &sender);
                if let Some(Ok(val)) = result {
                    let sum = val.get("result").and_then(|v| v.as_i64()).unwrap();
                    assert_eq!(sum, t as i64 + i as i64);
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_sequential_dispatch_thousand() {
    let reg = make_registry();
    let sender = CollectingSender { events: Arc::new(Mutex::new(Vec::new())) };
    for i in 0..1000 {
        let result = reg.dispatch_command("Echo.ping", json!({"iter": i}), &sender);
        assert!(matches!(result, Some(Ok(_))));
    }
}

#[test]
fn test_concurrent_unknown_commands() {
    let reg = make_registry();
    let mut handles = vec![];
    for _ in 0..4 {
        let r = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                let sender = CollectingSender { events: Arc::new(Mutex::new(Vec::new())) };
                let result = r.dispatch_command("Echo.nonexistent", json!({}), &sender);
                assert!(matches!(result, Some(Err(_))));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_concurrent_has_domain() {
    let reg = make_registry();
    let mut handles = vec![];
    for _ in 0..8 {
        let r = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                assert!(r.has_domain("Echo"));
                assert!(!r.has_domain("NonExistent"));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_collecting_sender_concurrent_events() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut handles = vec![];
    for i in 0..4 {
        let evts = Arc::clone(&events);
        handles.push(thread::spawn(move || {
            let sender = CollectingSender { events: evts };
            for j in 0..100 {
                sender.send_event("test", json!({"i": i, "j": j}));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(events.lock().unwrap().len(), 400);
}

#[test]
fn test_mixed_commands_concurrent() {
    let reg = make_registry();
    let mut handles = vec![];
    for _ in 0..4 {
        let r = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            for i in 0..200 {
                let sender = CollectingSender { events: Arc::new(Mutex::new(Vec::new())) };
                if i % 3 == 0 {
                    let _ = r.dispatch_command("Echo.ping", json!({}), &sender);
                } else if i % 3 == 1 {
                    let _ = r.dispatch_command("Echo.add", json!({"a": 1, "b": 2}), &sender);
                } else {
                    let _ = r.dispatch_command("Echo.unknown", json!({}), &sender);
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_error_code_and_message() {
    let err = CdpError { code: -32601, message: "not found".into() };
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "not found");
}

#[test]
fn test_noop_event_sender_trait() {
    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }
    let sender = NoopSender;
    sender.send_event("any", json!({}));
    sender.send_event("", json!(null));
    sender.send_event("long.event.name.with.dots", json!({"nested": {"deep": true}}));
}
