// REQ-CDP-005: CDP router with session management  @trace REQ-CDP-001 [entity:CdpRouter] @trace REQ-LIB-002
// REQ-LIB-002: CDP dual-layer API (internal/external routing)
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;

use crate::backend::{CdpBackend, ExternalBackend, InternalBackend};
use crate::protocol::CDPError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Internal,
    External,
}

type EventHandler = Box<dyn Fn(Value)>;

struct SessionInner {
    target_id: String,
    backend: BackendKind,
    enabled_domains: RefCell<std::collections::HashSet<String>>,
    event_handlers: RefCell<HashMap<String, EventHandler>>,
}

pub struct CdpRouter {
    internal: InternalBackend,
    external: RefCell<Option<ExternalBackend>>,
    sessions: RefCell<HashMap<String, Rc<SessionInner>>>,
}

impl Default for CdpRouter {
    fn default() -> Self {
        CdpRouter {
            internal: InternalBackend::new(),
            external: RefCell::new(None),
            sessions: RefCell::new(HashMap::new()),
        }
    }
}

impl CdpRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_internal_session(&self, target_id: &str) -> CdpSession {
        let session_id = format!("{:016x}", crate::rand_id());
        let inner = Rc::new(SessionInner {
            target_id: target_id.to_string(),
            backend: BackendKind::Internal,
            enabled_domains: RefCell::new(std::collections::HashSet::new()),
            event_handlers: RefCell::new(HashMap::new()),
        });
        self.sessions
            .borrow_mut()
            .insert(session_id.clone(), Rc::clone(&inner));
        CdpSession {
            session_id,
            inner,
        }
    }

    pub fn connect_external(&self, endpoint: &str) -> Result<ExternalBrowser, CDPError> {
        let backend = ExternalBackend::new(endpoint)?;
        *self.external.borrow_mut() = Some(backend);

        let session_id = format!("{:016x}", crate::rand_id());
        let inner = Rc::new(SessionInner {
            target_id: endpoint.to_string(),
            backend: BackendKind::External,
            enabled_domains: RefCell::new(std::collections::HashSet::new()),
            event_handlers: RefCell::new(HashMap::new()),
        });
        self.sessions
            .borrow_mut()
            .insert(session_id.clone(), Rc::clone(&inner));

        Ok(ExternalBrowser {
            endpoint: endpoint.to_string(),
            session_id,
        })
    }

    pub fn send_command(
        &self,
        session_id: &str,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, CDPError> {
        let sessions = self.sessions.borrow();
        let session = sessions.get(session_id).ok_or_else(|| CDPError {
            code: -32602,
            message: format!("session not found: {session_id}"),
        })?;

        match session.backend {
            BackendKind::Internal => {
                self.internal
                    .send_command(method, &params, &session.target_id)
            }
            BackendKind::External => {
                let external = self.external.borrow();
                let backend = external.as_ref().ok_or_else(|| CDPError {
                    code: -32603,
                    message: "external backend not connected".into(),
                })?;
                backend.send_command(method, &params, &session.target_id)
            }
        }
    }

    pub fn detach_session(&self, session_id: &str) -> Result<(), CDPError> {
        self.sessions
            .borrow_mut()
            .remove(session_id)
            .ok_or_else(|| CDPError {
                code: -32602,
                message: format!("session not found: {session_id}"),
            })?;
        Ok(())
    }
}

pub struct ExternalBrowser {
    pub endpoint: String,
    pub session_id: String,
}

pub struct CdpSession {
    session_id: String,
    inner: Rc<SessionInner>,
}

impl CdpSession {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn target_id(&self) -> &str {
        &self.inner.target_id
    }

    pub fn backend_kind(&self) -> BackendKind {
        self.inner.backend
    }

    pub fn send(
        &self,
        router: &CdpRouter,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, CDPError> {
        let domain = method.split('.').next().unwrap_or("");
        self.inner
            .enabled_domains
            .borrow_mut()
            .insert(domain.to_string());
        router.send_command(&self.session_id, method, params)
    }

    pub fn on<F: Fn(Value) + 'static>(&self, event: &str, handler: F) {
        self.inner
            .event_handlers
            .borrow_mut()
            .insert(event.to_string(), Box::new(handler));
    }

    pub fn detach(&self, router: &CdpRouter) -> Result<(), CDPError> {
        router.detach_session(&self.session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. CdpRouter::new creates empty router
    #[test]
    fn new_creates_empty_router() {
        let router = CdpRouter::new();
        assert!(router.sessions.borrow().is_empty());
    }

    // 2. CdpRouter::default same as new
    #[test]
    fn default_same_as_new() {
        let router_new = CdpRouter::new();
        let router_default = CdpRouter::default();
        assert!(router_new.sessions.borrow().is_empty());
        assert!(router_default.sessions.borrow().is_empty());
    }

    // 3. create_internal_session returns CdpSession with session_id
    #[test]
    fn create_internal_session_returns_session_with_id() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        assert!(!session.session_id().is_empty());
        assert_eq!(session.session_id().len(), 16);
    }

    // 4. create_internal_session session has BackendKind::Internal
    #[test]
    fn create_internal_session_has_internal_backend() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        assert_eq!(session.backend_kind(), BackendKind::Internal);
    }

    // 5. create_internal_session session has target_id
    #[test]
    fn create_internal_session_has_target_id() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("my-target-123");
        assert_eq!(session.target_id(), "my-target-123");
    }

    // 6. send_command on valid session → ok (internal backend handles it)
    #[test]
    fn send_command_on_valid_session_returns_ok() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        let result = router.send_command(
            session.session_id(),
            "Target.getTargetInfo",
            Some(serde_json::json!({})),
        );
        assert!(result.is_ok());
    }

    // 7. send_command on invalid session_id → Err 'session not found'
    #[test]
    fn send_command_on_invalid_session_returns_error() {
        let router = CdpRouter::new();
        let result = router.send_command("nonexistent-id", "Target.getTargetInfo", None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("session not found"));
    }

    // 8. detach_session removes session
    #[test]
    fn detach_session_removes_session() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        let session_id = session.session_id().to_string();
        assert_eq!(router.sessions.borrow().len(), 1);
        let result = router.detach_session(&session_id);
        assert!(result.is_ok());
        assert!(router.sessions.borrow().is_empty());
    }

    // 9. detach_session twice → Err 'session not found' on second
    #[test]
    fn detach_session_twice_returns_error_on_second() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        let session_id = session.session_id().to_string();
        let first = router.detach_session(&session_id);
        assert!(first.is_ok());
        let second = router.detach_session(&session_id);
        assert!(second.is_err());
        let err = second.unwrap_err();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("session not found"));
    }

    // 10. BackendKind variants equality
    #[test]
    fn backend_kind_variants_equality() {
        assert_eq!(BackendKind::Internal, BackendKind::Internal);
        assert_eq!(BackendKind::External, BackendKind::External);
        assert_ne!(BackendKind::Internal, BackendKind::External);
    }

    // 11. BackendKind debug format
    #[test]
    fn backend_kind_debug_format() {
        assert_eq!(format!("{:?}", BackendKind::Internal), "Internal");
        assert_eq!(format!("{:?}", BackendKind::External), "External");
    }

    // 12. CdpSession::session_id returns correct id
    #[test]
    fn cdp_session_session_id_returns_correct_id() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        let session_id = session.session_id();
        assert!(!session_id.is_empty());
        assert_eq!(session_id.len(), 16);
    }

    // 13. CdpSession::target_id returns correct target
    #[test]
    fn cdp_session_target_id_returns_correct_target() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("test-target-xyz");
        assert_eq!(session.target_id(), "test-target-xyz");
    }

    // 14. CdpSession::backend_kind returns Internal
    #[test]
    fn cdp_session_backend_kind_returns_internal() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        assert_eq!(session.backend_kind(), BackendKind::Internal);
    }

    // 15. CdpSession::send registers domain in enabled_domains
    #[test]
    fn cdp_session_send_registers_domain() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        let _ = session.send(&router, "Page.enable", Some(serde_json::json!({})));
        let sessions = router.sessions.borrow();
        let inner = sessions.get(session.session_id()).unwrap();
        assert!(inner.enabled_domains.borrow().contains("Page"));
    }

    // 16. Two sessions on same router work independently
    #[test]
    fn two_sessions_work_independently() {
        let router = CdpRouter::new();
        let session1 = router.create_internal_session("target-1");
        let session2 = router.create_internal_session("target-2");
        assert_ne!(session1.session_id(), session2.session_id());
        assert_eq!(session1.target_id(), "target-1");
        assert_eq!(session2.target_id(), "target-2");
        let _ = session1.send(&router, "Page.enable", None);
        let _ = session2.send(&router, "Network.enable", None);
        let sessions = router.sessions.borrow();
        let inner1 = sessions.get(session1.session_id()).unwrap();
        let inner2 = sessions.get(session2.session_id()).unwrap();
        assert!(inner1.enabled_domains.borrow().contains("Page"));
        assert!(!inner1.enabled_domains.borrow().contains("Network"));
        assert!(inner2.enabled_domains.borrow().contains("Network"));
        assert!(!inner2.enabled_domains.borrow().contains("Page"));
    }

    // 17. CdpSession::on registers event handler
    #[test]
    fn cdp_session_on_registers_event_handler() {
        use std::cell::Cell;
        use std::rc::Rc;
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        let called = Rc::new(Cell::new(false));
        let called_clone = Rc::clone(&called);
        session.on("Page.loadEventFired", move |_event| {
            called_clone.set(true);
        });
        let sessions = router.sessions.borrow();
        let inner = sessions.get(session.session_id()).unwrap();
        assert!(inner.event_handlers.borrow().contains_key("Page.loadEventFired"));
        let binding = inner.event_handlers.borrow();
        let handler = binding.get("Page.loadEventFired").unwrap();
        handler(serde_json::json!({}));
        assert!(called.get());
    }

    // 18. CdpSession::detach removes session from router
    #[test]
    fn cdp_session_detach_removes_session() {
        let router = CdpRouter::new();
        let session = router.create_internal_session("target-1");
        let _session_id = session.session_id().to_string();
        assert_eq!(router.sessions.borrow().len(), 1);
        let result = session.detach(&router);
        assert!(result.is_ok());
        assert!(router.sessions.borrow().is_empty());
    }

    // 19. BackendKind Copy + Clone
    #[test]
    fn backend_kind_copy_clone() {
        let kind = BackendKind::Internal;
        let copied = kind;
        let cloned = kind.clone();
        assert_eq!(kind, copied);
        assert_eq!(kind, cloned);
        fn takes_copy<T: Copy>(_: T) {}
        takes_copy(BackendKind::Internal);
        fn takes_clone<T: Clone>(_: T) {}
        takes_clone(BackendKind::Internal);
    }

    // 20. ExternalBrowser fields accessible
    #[test]
    fn external_browser_fields_accessible() {
        let browser = ExternalBrowser {
            endpoint: "ws://localhost:9222".to_string(),
            session_id: "abcd1234efgh5678".to_string(),
        };
        assert_eq!(browser.endpoint, "ws://localhost:9222");
        assert_eq!(browser.session_id, "abcd1234efgh5678");
    }
}
