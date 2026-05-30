// REQ-CDP-005: CDP router with session management  @trace REQ-CDP-001 [entity:CdpRouter] REQ-LIB-002
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
