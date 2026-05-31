// @trace REQ-CDS-005 [entity:EventSubscription]
// Event broadcaster: domain-based subscription filtering.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::protocol::{CdpEvent, serialize_event};
use crate::session::CdpSession;
use crate::EventSender;

type SessionMap = Arc<Mutex<HashMap<String, Arc<Mutex<CdpSession>>>>>;

/// EventBroadcaster implements EventSender. It holds a reference to the
/// session map and broadcasts events only to sessions that have enabled
/// the relevant domain.
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
}

impl EventSender for EventBroadcaster {
    fn send_event(&self, method: &str, params: Value) {
        let domain = method.split('.').next().unwrap_or("");
        let event = CdpEvent {
            method: method.to_string(),
            params: Some(params),
        };
        let json = serialize_event(&event);

        let sessions = match self.sessions.lock() {
            Ok(s) => s,
            Err(_) => return,
        };

        for session in sessions.values() {
            let mut session = match session.lock() {
                Ok(s) => s,
                Err(_) => continue,
            };
            if session.is_browser_session() || session.has_domain_enabled(domain) {
                let _ = session.send_text(&json);
            }
        }
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

    // @trace TEST-CDS-005 [req:REQ-CDS-005] [level:unit]
    #[test]
    fn sender_returns_boxed_event_sender() {
        let broadcaster = EventBroadcaster::new(empty_session_map());
        let _sender: Box<dyn EventSender> = broadcaster.sender();
    }

    // @trace TEST-CDS-005 [req:REQ-CDS-005] [level:unit]
    #[test]
    fn send_event_empty_sessions_no_panic() {
        let broadcaster = EventBroadcaster::new(empty_session_map());
        broadcaster.send_event("Page.loadEventFired", serde_json::json!({}));
    }

    // @trace TEST-CDS-005 [req:REQ-CDS-005] [level:unit]
    #[test]
    fn clone_shares_sessions_arc() {
        let sessions = empty_session_map();
        let a = EventBroadcaster::new(Arc::clone(&sessions));
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.sessions, &b.sessions));
    }

    // @trace TEST-CDS-005 [req:REQ-CDS-005] [level:unit]
    #[test]
    fn send_event_method_domain_extraction_unit_test() {
        assert_eq!("Page".split('.').next().unwrap_or(""), "Page");
        assert_eq!("Runtime.consoleAPICalled".split('.').next().unwrap_or(""), "Runtime");
        assert_eq!("no_dot_method".split('.').next().unwrap_or(""), "no_dot_method");
        assert_eq!("".split('.').next().unwrap_or(""), "");
    }
}
