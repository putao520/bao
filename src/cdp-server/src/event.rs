// @trace REQ-CDS-005 [entity:EventSubscription]
// Event broadcaster: domain-based subscription filtering.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::protocol::{serialize_event, CdpEvent};
use crate::session::{OutboxEvent, SessionHandle};
use crate::EventSender;

type SessionMap = Arc<Mutex<HashMap<String, Arc<SessionHandle>>>>;

/// EventBroadcaster implements EventSender. It holds a reference to the
/// session map and queues events into per-session outboxes; the server loop
/// drains the outboxes into the WebSocket while holding the session lock.
///
/// Events are NEVER written to the socket directly here: a command dispatch
/// running inside `CdpSession::process` may emit events for that very
/// session — taking the session lock here would self-deadlock the server
/// loop. Outbox + drain-at-send-time preserves the domain gating (applied
/// when the drain holds the session).
pub struct EventBroadcaster {
    sessions: SessionMap,
}

impl EventBroadcaster {
    pub fn new(sessions: SessionMap) -> Self {
        EventBroadcaster { sessions }
    }

    /// Create a boxed clone-safe EventSender reference.
    pub fn sender(&self) -> Box<dyn EventSender> {
        Box::new(EventBroadcaster {
            sessions: Arc::clone(&self.sessions),
        })
    }

    fn enqueue(&self, entry: OutboxEvent) {
        let sessions = match self.sessions.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        for handle in sessions.values() {
            if let Ok(mut outbox) = handle.outbox.lock() {
                outbox.push_back(entry.clone());
            }
        }
    }
}

impl EventSender for EventBroadcaster {
    fn send_event(&self, method: &str, params: Value) {
        let domain = method.split('.').next().unwrap_or("").to_string();
        let event = CdpEvent {
            method: method.to_string(),
            params: Some(params),
        };
        self.enqueue(OutboxEvent {
            json: serialize_event(&event),
            domain,
            browser_only: false,
        });
    }

    /// Session-scoped event (flattened CDP sessions): the event JSON carries
    /// `sessionId`, so clients route it to the attached target session.
    /// Delivered to browser-endpoint sessions (they own the flat sessions).
    fn send_session_event(&self, session_id: &str, method: &str, params: Value) {
        let domain = method.split('.').next().unwrap_or("").to_string();
        let json = serde_json::json!({
            "method": method,
            "params": params,
            "sessionId": session_id,
        })
        .to_string();
        self.enqueue(OutboxEvent {
            json,
            domain,
            browser_only: true,
        });
    }
}

// Clone: Arc-based shallow copy.
impl Clone for EventBroadcaster {
    fn clone(&self) -> Self {
        EventBroadcaster {
            sessions: Arc::clone(&self.sessions),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn empty_session_map() -> SessionMap {
        Arc::new(Mutex::new(HashMap::new()))
    }

    // @trace TEST-CDS-005 [req:REQ-CDS-005] [level:unit]
    #[test]
    fn new_with_empty_sessions_no_panic() {
        let _broadcaster = EventBroadcaster::new(empty_session_map());
    }

    #[test]
    fn sender_returns_boxed_event_sender() {
        let broadcaster = EventBroadcaster::new(empty_session_map());
        let _sender: Box<dyn EventSender> = broadcaster.sender();
    }

    #[test]
    fn send_event_empty_sessions_no_panic() {
        let broadcaster = EventBroadcaster::new(empty_session_map());
        broadcaster.send_event("Page.loadEventFired", serde_json::json!({}));
    }

    #[test]
    fn send_session_event_empty_sessions_no_panic() {
        let broadcaster = EventBroadcaster::new(empty_session_map());
        broadcaster.send_session_event("sid", "Runtime.executionContextCreated", serde_json::json!({}));
    }

    #[test]
    fn clone_shares_sessions_arc() {
        let sessions = empty_session_map();
        let a = EventBroadcaster::new(Arc::clone(&sessions));
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.sessions, &b.sessions));
    }

    #[test]
    fn send_event_method_domain_extraction_unit_test() {
        assert_eq!("Page".split('.').next().unwrap_or(""), "Page");
        assert_eq!(
            "Runtime.consoleAPICalled".split('.').next().unwrap_or(""),
            "Runtime"
        );
        assert_eq!(
            "no_dot_method".split('.').next().unwrap_or(""),
            "no_dot_method"
        );
        assert_eq!("".split('.').next().unwrap_or(""), "");
    }
}
