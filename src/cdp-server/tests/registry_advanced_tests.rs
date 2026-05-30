// @trace TEST-CDS-013-REGISTRY-ADV [req:REQ-CDS-001,REQ-CDS-006] [level:unit]
// DomainRegistry advanced tests: thread-safety, session lifecycle callbacks,
// multi-domain dispatch, has_domain, notify_session lifecycle, error paths.

use cdp_server::{DomainRegistry, DomainHandler, CdpError, EventSender};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---- Test helpers ----

struct CountingDomain {
    name: &'static str,
    create_count: Arc<AtomicUsize>,
    destroy_count: Arc<AtomicUsize>,
}

impl DomainHandler for CountingDomain {
    fn domain_name(&self) -> &'static str { self.name }
    fn handle_command(&self, cmd: &str, _params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Ok(json!({"handled": cmd}))
    }
    fn on_session_created(&self, _session_id: &str) {
        self.create_count.fetch_add(1, Ordering::SeqCst);
    }
    fn on_session_destroyed(&self, _session_id: &str) {
        self.destroy_count.fetch_add(1, Ordering::SeqCst);
    }
}

struct ErrorDomain;
impl DomainHandler for ErrorDomain {
    fn domain_name(&self) -> &'static str { "Error" }
    fn handle_command(&self, cmd: &str, _params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Err(CdpError { code: -32000, message: format!("ErrorDomain cannot handle '{}'", cmd) })
    }
}

struct NopSender;
impl EventSender for NopSender {
    fn send_event(&self, _: &str, _: Value) {}
}

// ---- has_domain ----

#[test]
fn test_has_domain_empty_registry() {
    let reg = DomainRegistry::new();
    assert!(!reg.has_domain("Page"));
    assert!(!reg.has_domain(""));
}

#[test]
fn test_has_domain_after_register() {
    let reg = DomainRegistry::new();
    let create = Arc::new(AtomicUsize::new(0));
    let destroy = Arc::new(AtomicUsize::new(0));
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: create,
        destroy_count: destroy,
    })).unwrap();
    assert!(reg.has_domain("Page"));
    assert!(!reg.has_domain("Runtime"));
}

#[test]
fn test_has_domain_multiple() {
    let reg = DomainRegistry::new();
    let c = Arc::new(AtomicUsize::new(0));
    let d = Arc::new(AtomicUsize::new(0));
    reg.register(Box::new(CountingDomain { name: "Page", create_count: c.clone(), destroy_count: d.clone() })).unwrap();
    reg.register(Box::new(CountingDomain { name: "Runtime", create_count: c.clone(), destroy_count: d.clone() })).unwrap();
    reg.register(Box::new(CountingDomain { name: "DOM", create_count: c.clone(), destroy_count: d.clone() })).unwrap();
    assert!(reg.has_domain("Page"));
    assert!(reg.has_domain("Runtime"));
    assert!(reg.has_domain("DOM"));
    assert!(!reg.has_domain("Network"));
}

// ---- dispatch_command edge cases ----

#[test]
fn test_dispatch_no_dot_in_method() {
    let reg = DomainRegistry::new();
    // "noDotMethod" → domain = "noDotMethod" (full string before '.')
    let result = reg.dispatch_command("noDotMethod", json!({}), &NopSender);
    // No handler registered for "noDotMethod" domain
    assert!(result.is_none());
}

#[test]
fn test_dispatch_empty_method() {
    let reg = DomainRegistry::new();
    let result = reg.dispatch_command("", json!({}), &NopSender);
    assert!(result.is_none());
}

#[test]
fn test_dispatch_unregistered_domain() {
    let reg = DomainRegistry::new();
    let result = reg.dispatch_command("Unknown.method", json!({}), &NopSender);
    assert!(result.is_none());
}

#[test]
fn test_dispatch_error_domain() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(ErrorDomain)).unwrap();
    let result = reg.dispatch_command("Error.fail", json!({}), &NopSender);
    assert!(result.is_some());
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("ErrorDomain"));
}

#[test]
fn test_dispatch_multiple_commands_same_domain() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: Arc::new(AtomicUsize::new(0)),
        destroy_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();
    for i in 0..20 {
        let result = reg.dispatch_command("Page.navigate", json!({"url": format!("https://{}.com", i)}), &NopSender);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }
}

// ---- Session lifecycle callbacks ----

#[test]
fn test_session_created_callback() {
    let create_count = Arc::new(AtomicUsize::new(0));
    let destroy_count = Arc::new(AtomicUsize::new(0));
    let reg = DomainRegistry::new();
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: create_count.clone(),
        destroy_count: destroy_count.clone(),
    })).unwrap();

    reg.notify_session_created("Page", "session-1");
    assert_eq!(create_count.load(Ordering::SeqCst), 1);
    assert_eq!(destroy_count.load(Ordering::SeqCst), 0);
}

#[test]
fn test_session_destroyed_callback() {
    let create_count = Arc::new(AtomicUsize::new(0));
    let destroy_count = Arc::new(AtomicUsize::new(0));
    let reg = DomainRegistry::new();
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: create_count.clone(),
        destroy_count: destroy_count.clone(),
    })).unwrap();

    reg.notify_session_destroyed(&["Page".to_string()], "session-1");
    assert_eq!(destroy_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_multiple_session_lifecycle() {
    let create_count = Arc::new(AtomicUsize::new(0));
    let destroy_count = Arc::new(AtomicUsize::new(0));
    let reg = DomainRegistry::new();
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: create_count.clone(),
        destroy_count: destroy_count.clone(),
    })).unwrap();

    for i in 0..10 {
        reg.notify_session_created("Page", &format!("s-{}", i));
    }
    assert_eq!(create_count.load(Ordering::SeqCst), 10);

    let _domains: Vec<String> = (0..10).map(|i| format!("s-{}", i)).collect();
    // These won't match "Page" domain since domains list contains session IDs not domain names
    // The method takes domain names to notify
    reg.notify_session_destroyed(&["Page".to_string()], "s-0");
    assert_eq!(destroy_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_notify_unregistered_domain_noop() {
    let reg = DomainRegistry::new();
    // Should not panic
    reg.notify_session_created("NonExistent", "s-1");
    reg.notify_session_destroyed(&["NonExistent".to_string()], "s-1");
}

// ---- Duplicate registration ----

#[test]
fn test_duplicate_registration_returns_error() {
    let reg = DomainRegistry::new();
    let c = Arc::new(AtomicUsize::new(0));
    let d = Arc::new(AtomicUsize::new(0));
    reg.register(Box::new(CountingDomain { name: "Page", create_count: c.clone(), destroy_count: d.clone() })).unwrap();
    let result = reg.register(Box::new(CountingDomain { name: "Page", create_count: c.clone(), destroy_count: d.clone() }));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already registered"));
}

// ---- Thread safety ----

#[test]
fn test_concurrent_dispatch() {
    let reg = Arc::new(DomainRegistry::new());
    let c = Arc::new(AtomicUsize::new(0));
    let d = Arc::new(AtomicUsize::new(0));
    reg.register(Box::new(CountingDomain {
        name: "Page",
        create_count: c,
        destroy_count: d,
    })).unwrap();

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let reg = reg.clone();
            thread::spawn(move || {
                let result = reg.dispatch_command(
                    "Page.navigate",
                    json!({"url": format!("https://{}.com", i)}),
                    &NopSender,
                );
                assert!(result.is_some());
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_concurrent_register_different_domains() {
    let reg = Arc::new(DomainRegistry::new());
    let handles: Vec<_> = (0..5)
        .map(|i| {
            let reg = reg.clone();
            thread::spawn(move || {
                let name = Box::leak(format!("Domain{}", i).into_boxed_str());
                reg.register(Box::new(CountingDomain {
                    name,
                    create_count: Arc::new(AtomicUsize::new(0)),
                    destroy_count: Arc::new(AtomicUsize::new(0)),
                })).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
    assert!(reg.has_domain("Domain0"));
    assert!(reg.has_domain("Domain4"));
}

use std::thread;

// ---- Default trait ----

#[test]
fn test_domain_registry_default() {
    let reg = DomainRegistry::default();
    assert!(!reg.has_domain("Page"));
}

// ---- SharedRegistry type alias ----

#[test]
fn test_shared_registry_arc() {
    let reg: Arc<DomainRegistry> = Arc::new(DomainRegistry::new());
    assert!(!reg.has_domain("Page"));
    reg.register(Box::new(CountingDomain {
        name: "Test",
        create_count: Arc::new(AtomicUsize::new(0)),
        destroy_count: Arc::new(AtomicUsize::new(0)),
    })).unwrap();
    assert!(reg.has_domain("Test"));
}

// ---- Multi-domain dispatch interleaved ----

#[test]
fn test_multi_domain_interleaved_dispatch() {
    let reg = DomainRegistry::new();
    let c = Arc::new(AtomicUsize::new(0));
    let d = Arc::new(AtomicUsize::new(0));
    reg.register(Box::new(CountingDomain { name: "Page", create_count: c.clone(), destroy_count: d.clone() })).unwrap();
    reg.register(Box::new(CountingDomain { name: "Runtime", create_count: c.clone(), destroy_count: d.clone() })).unwrap();
    reg.register(Box::new(CountingDomain { name: "DOM", create_count: c.clone(), destroy_count: d.clone() })).unwrap();

    let commands = vec![
        "Page.enable", "Runtime.enable", "DOM.enable",
        "Page.navigate", "Runtime.evaluate", "DOM.getDocument",
        "Page.disable", "Runtime.disable", "DOM.disable",
    ];
    for cmd in &commands {
        let result = reg.dispatch_command(cmd, json!({}), &NopSender);
        assert!(result.is_some(), "Command '{}' should dispatch", cmd);
        assert!(result.unwrap().is_ok(), "Command '{}' should succeed", cmd);
    }
}
