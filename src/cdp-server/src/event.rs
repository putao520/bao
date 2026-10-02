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

    /// Target-scoped page-endpoint delivery (REQ-CDP-004): deliver an
    /// UNTAGGED event to every page-endpoint session bound to `target_id`.
    /// A `/devtools/page/<id>` connection IS a subscription to that page —
    /// Chrome delivers the page's events on its endpoint untagged — while
    /// browser-endpoint sessions are not page subscribers and receive
    /// nothing here (that asymmetry is what keeps the tightened targeted
    /// routing from re-broadcasting one page's frame stream into unrelated
    /// endpoints). Routes by the handle's `page_target` snapshot, so the
    /// session mutex is never taken on the enqueue path. Zero matching
    /// page subscribers → nothing enqueued (no-subscriber-no-deliver).
    fn send_page_event(&self, target_id: &str, method: &str, params: Value) {
        let domain = method.split('.').next().unwrap_or("").to_string();
        let event = CdpEvent {
            method: method.to_string(),
            params: Some(params),
        };
        let json = serialize_event(&event);
        let sessions = match self.sessions.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut delivered = 0usize;
        for handle in sessions.values() {
            if handle.page_target.as_deref() != Some(target_id) {
                continue;
            }
            if let Ok(mut outbox) = handle.outbox.lock() {
                outbox.push_back(OutboxEvent {
                    json: json.clone(),
                    domain: domain.clone(),
                    browser_only: false,
                });
                delivered += 1;
            }
        }
        if delivered == 0 {
            log::debug!(
                "[cdp-route] {method}: no page-endpoint subscriber for target {target_id} — dropped"
            );
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
    use crate::session::CdpSession;
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

    /// A CdpSession over a loopback TCP pair (never read/written — the tests
    /// only exercise the outbox routing).
    fn loopback_session(session_id: &str, target_id: &str, is_browser: bool) -> CdpSession {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(addr).unwrap();
        let (stream, _) = listener.accept().unwrap();
        drop(client);
        let ws = tungstenite::protocol::WebSocket::from_raw_socket(
            crate::session::ReplayStream::new(stream, Vec::new()),
            tungstenite::protocol::Role::Server,
            None,
        );
        CdpSession::new(session_id.to_string(), target_id.to_string(), ws, is_browser)
    }

    fn outbox_len(handle: &SessionHandle) -> usize {
        handle.outbox.lock().unwrap().len()
    }

    // @trace TEST-CDS-005 [req:REQ-CDS-005] [level:unit]
    // @trace REQ-CDP-004 [req:REQ-CDP-004] [level:unit]
    #[test]
    fn send_page_event_delivers_only_to_matching_page_endpoints() {
        // REQ-CDP-004 定向 page-endpoint 送达钉:page 订阅者按 endpoint target
        // 精确匹配;browser endpoint 不是 page 订阅者,零收;零匹配目标 =
        // drop(no-subscriber-no-deliver)。
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        sessions
            .lock()
            .unwrap()
            .insert("conn-7".to_string(), SessionHandle::new(loopback_session("conn-7", "7", false)));
        sessions
            .lock()
            .unwrap()
            .insert("conn-8".to_string(), SessionHandle::new(loopback_session("conn-8", "8", false)));
        sessions
            .lock()
            .unwrap()
            .insert(
                "conn-browser".to_string(),
                SessionHandle::new(loopback_session("conn-browser", "__browser__", true)),
            );
        let broadcaster = EventBroadcaster::new(Arc::clone(&sessions));

        broadcaster.send_page_event("7", "Page.frameNavigated", serde_json::json!({}));

        // Inspect the handles through the same map Arc the broadcaster holds.
        // The guard MUST be scoped out before the next send_page_event — the
        // broadcaster takes the same non-reentrant sessions lock.
        {
            let map = sessions.lock().unwrap();
            assert_eq!(outbox_len(map.get("conn-7").unwrap()), 1, "page-7 subscriber received");
            assert_eq!(outbox_len(map.get("conn-8").unwrap()), 0, "other page received nothing");
            assert_eq!(
                outbox_len(map.get("conn-browser").unwrap()),
                0,
                "browser endpoint is not a page subscriber"
            );
        }

        // Zero matching page subscribers → dropped everywhere.
        broadcaster.send_page_event("99", "Page.frameStoppedLoading", serde_json::json!({}));
        let map = sessions.lock().unwrap();
        assert_eq!(outbox_len(map.get("conn-7").unwrap()), 1);
        assert_eq!(outbox_len(map.get("conn-8").unwrap()), 0);
        assert_eq!(outbox_len(map.get("conn-browser").unwrap()), 0);
    }
}
