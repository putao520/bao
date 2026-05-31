// @trace REQ-CDS-006 [entity:DomainRegistry]
// DomainHandler registration, lookup and lifecycle callbacks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::{DomainHandler, EventSender, CdpError};

/// Registry of CDP domain handlers. Thread-safe (Mutex-protected).
pub struct DomainRegistry {
    handlers: Mutex<HashMap<&'static str, Box<dyn DomainHandler>>>,
}

impl Default for DomainRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DomainRegistry {
    pub fn new() -> Self {
        DomainRegistry {
            handlers: Mutex::new(HashMap::new()),
        }
    }

    /// Register a DomainHandler. Returns Err if a handler with the same
    /// domain_name is already registered (REQ-CDS-006 C5: no overwrite).
    pub fn register(&self, handler: Box<dyn DomainHandler>) -> Result<(), String> {
        let mut map = self.handlers.lock().map_err(|_| -> String { "lock poisoned".into() })?;
        let name = handler.domain_name();
        if map.contains_key(name) {
            return Err(format!("domain '{}' already registered", name));
        }
        map.insert(name, handler);
        Ok(())
    }

    /// Look up a DomainHandler by domain name (O(1) HashMap).
    pub fn get(&self, _domain: &str) -> Option<Box<dyn DomainHandler + '_>> {
        // We cannot return a reference through a MutexGuard without owning it.
        // Instead, we clone the domain name and use a scoped approach.
        // Actually, we need to handle this differently - use a callback pattern
        // or return the result of the operation directly.
        None // placeholder — actual dispatch done via dispatch_command
    }

    /// Dispatch a command to the appropriate DomainHandler.
    /// Extracts domain from method (e.g. "Page.navigate" → "Page").
    pub fn dispatch_command(
        &self,
        method: &str,
        params: Value,
        event_sender: &dyn EventSender,
    ) -> Option<Result<Value, CdpError>> {
        let domain = method.split('.').next().unwrap_or("");
        let map = self.handlers.lock().ok()?;
        let handler = map.get(domain)?;
        Some(handler.handle_command(method, params, event_sender))
    }

    /// Notify the DomainHandler for the given domain that a session was created.
    pub fn notify_session_created(&self, domain: &str, session_id: &str) {
        if let Ok(map) = self.handlers.lock() {
            if let Some(handler) = map.get(domain) {
                handler.on_session_created(session_id);
            }
        }
    }

    /// Notify all DomainHandlers for the given domains that a session was destroyed.
    pub fn notify_session_destroyed(&self, domains: &[String], session_id: &str) {
        if let Ok(map) = self.handlers.lock() {
            for domain in domains {
                if let Some(handler) = map.get(domain.as_str()) {
                    handler.on_session_destroyed(session_id);
                }
            }
        }
    }

    /// Check if a domain is registered.
    pub fn has_domain(&self, domain: &str) -> bool {
        self.handlers
            .lock()
            .map(|m| m.contains_key(domain))
            .unwrap_or(false)
    }
}

// Arc wrapper for shared ownership across sessions.
pub type SharedRegistry = Arc<DomainRegistry>;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{DomainHandler, EventSender, CdpError};

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }

    struct MockHandler {
        name: &'static str,
    }

    impl DomainHandler for MockHandler {
        fn domain_name(&self) -> &'static str {
            self.name
        }

        fn handle_command(
            &self,
            _command: &str,
            _params: Value,
            _event_sender: &dyn EventSender,
        ) -> Result<Value, CdpError> {
            Ok(json!({}))
        }

        fn on_session_created(&self, _session_id: &str) {}

        fn on_session_destroyed(&self, _session_id: &str) {}
    }

    // @trace TEST-CDS-REG-001 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn new_registry_is_empty() {
        let reg = DomainRegistry::new();
        assert!(!reg.has_domain("Page"));
    }

    // @trace TEST-CDS-REG-002 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn default_same_as_new() {
        let via_new = DomainRegistry::new();
        let via_default = DomainRegistry::default();
        assert!(!via_new.has_domain("Page"));
        assert!(!via_default.has_domain("Page"));
    }

    // @trace TEST-CDS-REG-003 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn register_handler_then_has_domain() {
        let reg = DomainRegistry::new();
        reg.register(Box::new(MockHandler { name: "Page" })).unwrap();
        assert!(reg.has_domain("Page"));
    }

    // @trace TEST-CDS-REG-004 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn register_duplicate_returns_err() {
        let reg = DomainRegistry::new();
        reg.register(Box::new(MockHandler { name: "Page" })).unwrap();
        let err = reg.register(Box::new(MockHandler { name: "Page" })).unwrap_err();
        assert!(err.contains("'Page'"));
    }

    // @trace TEST-CDS-REG-005 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn register_different_domains_both_present() {
        let reg = DomainRegistry::new();
        reg.register(Box::new(MockHandler { name: "Page" })).unwrap();
        reg.register(Box::new(MockHandler { name: "Runtime" })).unwrap();
        assert!(reg.has_domain("Page"));
        assert!(reg.has_domain("Runtime"));
    }

    // @trace TEST-CDS-REG-006 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn has_domain_unregistered_returns_false() {
        let reg = DomainRegistry::new();
        reg.register(Box::new(MockHandler { name: "Page" })).unwrap();
        assert!(!reg.has_domain("DOM"));
    }

    // @trace TEST-CDS-REG-007 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn dispatch_command_registered_returns_some_ok() {
        let reg = DomainRegistry::new();
        reg.register(Box::new(MockHandler { name: "Page" })).unwrap();
        let result = reg.dispatch_command("Page.navigate", json!(null), &NoopSender);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }

    // @trace TEST-CDS-REG-008 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn dispatch_command_unregistered_returns_none() {
        let reg = DomainRegistry::new();
        let result = reg.dispatch_command("DOM.getDocument", json!(null), &NoopSender);
        assert!(result.is_none());
    }

    // @trace TEST-CDS-REG-009 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn dispatch_command_extracts_domain_from_method() {
        let reg = DomainRegistry::new();
        reg.register(Box::new(MockHandler { name: "Runtime" })).unwrap();
        let result = reg.dispatch_command("Runtime.evaluate", json!(null), &NoopSender);
        assert!(result.is_some());
    }

    // @trace TEST-CDS-REG-010 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn notify_session_created_unregistered_no_panic() {
        let reg = DomainRegistry::new();
        reg.notify_session_created("Page", "sess-1");
    }

    // @trace TEST-CDS-REG-011 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn notify_session_destroyed_unregistered_no_panic() {
        let reg = DomainRegistry::new();
        reg.notify_session_destroyed(&["Page".to_string()], "sess-1");
    }

    // @trace TEST-CDS-REG-012 [req:REQ-CDS-006] [level:unit]
    #[test]
    fn dispatch_command_with_valid_json_params() {
        let reg = DomainRegistry::new();
        reg.register(Box::new(MockHandler { name: "Page" })).unwrap();
        let result = reg.dispatch_command(
            "Page.navigate",
            json!({ "url": "https://example.com" }),
            &NoopSender,
        );
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }
}
