// @trace REQ-CDP-007
use serde_json::{json, Value};

use std::sync::atomic::Ordering;

use cdp_server::{CdpError, DomainHandler, EventSender};

/// Log domain handler — routes console messages to CDP events.
pub struct LogHandler {
    enabled: std::sync::atomic::AtomicBool,
}

impl LogHandler {
    pub fn new() -> Self {
        LogHandler {
            enabled: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl DomainHandler for LogHandler {
    fn domain_name(&self) -> &'static str { "Log" }

    fn handle_command(&self, command: &str, params: Value, event_sender: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Log.enable" => {
                self.enabled.store(true, Ordering::SeqCst);
                event_sender.send_event("Log.entryAdded", json!({
                    "entry": {
                        "source": "other",
                        "level": "info",
                        "text": "Log domain enabled",
                        "timestamp": chrono_now_ms(),
                    }
                }));
                Ok(json!({}))
            }
            "Log.disable" => {
                self.enabled.store(false, Ordering::SeqCst);
                Ok(json!({}))
            }
            "Log.clear" => {
                if self.enabled.load(Ordering::SeqCst) {
                    event_sender.send_event("Log.entryAdded", json!({
                        "entry": {
                            "source": "other",
                            "level": "info",
                            "text": "Log cleared",
                            "timestamp": chrono_now_ms(),
                        }
                    }));
                }
                Ok(json!({}))
            }
            "Log.startViolationsReport" => {
                let config = params.get("config").and_then(|v| v.as_array());
                let count = config.map(|a| a.len()).unwrap_or(0);
                if self.enabled.load(Ordering::SeqCst) {
                    event_sender.send_event("Log.violationReportChanged", json!({
                        "activeViolations": count,
                    }));
                }
                Ok(json!({}))
            }
            "Log.stopViolationsReport" => {
                if self.enabled.load(Ordering::SeqCst) {
                    event_sender.send_event("Log.violationReportChanged", json!({
                        "activeViolations": 0,
                    }));
                }
                Ok(json!({}))
            }
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

fn chrono_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }
    static NOOP: NoopSender = NoopSender;

    #[test]
    fn log_domain_name() {
        let h = LogHandler::new();
        assert_eq!(h.domain_name(), "Log");
    }

    #[test]
    fn log_enable_sends_entry_added_event() {
        let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        struct CapturingSender { events: Arc<Mutex<Vec<(String, Value)>>> }
        impl EventSender for CapturingSender {
            fn send_event(&self, method: &str, params: Value) {
                self.events.lock().unwrap().push((method.to_string(), params));
            }
        }
        let sender = CapturingSender { events: events_clone };

        let h = LogHandler::new();
        let result = h.handle_command("Log.enable", json!({}), &sender);
        assert!(result.is_ok());

        let evts = events.lock().unwrap();
        assert_eq!(evts.len(), 1);
        assert_eq!(evts[0].0, "Log.entryAdded");
        assert!(evts[0].1["entry"]["text"].as_str().unwrap().contains("enabled"));
    }

    #[test]
    fn log_disable_resets_state() {
        let h = LogHandler::new();
        h.handle_command("Log.enable", json!({}), &NOOP).unwrap();
        h.handle_command("Log.disable", json!({}), &NOOP).unwrap();
        assert!(!h.enabled.load(Ordering::SeqCst));
    }

    #[test]
    fn log_clear_when_enabled_sends_event() {
        let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        struct CapturingSender { events: Arc<Mutex<Vec<(String, Value)>>> }
        impl EventSender for CapturingSender {
            fn send_event(&self, method: &str, params: Value) {
                self.events.lock().unwrap().push((method.to_string(), params));
            }
        }
        let sender = CapturingSender { events: events_clone };

        let h = LogHandler::new();
        h.handle_command("Log.enable", json!({}), &sender).unwrap();
        h.handle_command("Log.clear", json!({}), &sender).unwrap();

        let evts = events.lock().unwrap();
        assert_eq!(evts.len(), 2);
        assert!(evts[1].0 == "Log.entryAdded");
    }

    #[test]
    fn log_clear_when_disabled_no_event() {
        let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        struct CapturingSender { events: Arc<Mutex<Vec<(String, Value)>>> }
        impl EventSender for CapturingSender {
            fn send_event(&self, method: &str, params: Value) {
                self.events.lock().unwrap().push((method.to_string(), params));
            }
        }
        let sender = CapturingSender { events: events_clone };

        let h = LogHandler::new();
        h.handle_command("Log.clear", json!({}), &sender).unwrap();

        let evts = events.lock().unwrap();
        assert!(evts.is_empty());
    }

    #[test]
    fn log_violations_report_sends_event_when_enabled() {
        let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        struct CapturingSender { events: Arc<Mutex<Vec<(String, Value)>>> }
        impl EventSender for CapturingSender {
            fn send_event(&self, method: &str, params: Value) {
                self.events.lock().unwrap().push((method.to_string(), params));
            }
        }
        let sender = CapturingSender { events: events_clone };

        let h = LogHandler::new();
        h.handle_command("Log.enable", json!({}), &sender).unwrap();
        h.handle_command("Log.startViolationsReport", json!({"config": [{"name": "longTask"}]}), &sender).unwrap();
        h.handle_command("Log.stopViolationsReport", json!({}), &sender).unwrap();

        let evts = events.lock().unwrap();
        assert!(evts.iter().any(|(m, _)| m == "Log.violationReportChanged"));
    }

    #[test]
    fn log_unknown_returns_error() {
        let h = LogHandler::new();
        let err = h.handle_command("Log.nonexistent", json!({}), &NOOP).unwrap_err();
        assert_eq!(err.code, -32601);
    }
}
