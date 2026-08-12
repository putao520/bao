// @trace REQ-CDS-007 [entity:BaoEvent] [entity:ConsoleMessage]
// Typed Bao event model: replaces raw __BAO_* string-prefix parsing in server.rs
// with a structured enum, standardized JS→Rust transport, and typed broadcast.

use serde_json::{json, Value};

use crate::EventSender;

// ---------------------------------------------------------------------------
// §1 BaoEvent enum — 8 typed CDP event variants
// ---------------------------------------------------------------------------

/// Typed representation of CDP events that originate from the browser engine
/// (servo/bao_browser) and are forwarded to cdp-server via console messages.
///
/// The old protocol used 8 different `__BAO_*__` string prefixes. This enum
/// replaces that with a single standardized transport format:
///
/// ```text
/// __BAO_EVT__CDP.MethodName\n{json}
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum BaoEvent {
    FetchRequestPaused {
        request_id: String,
        url: String,
        method: String,
        headers: Value,
        post_data: Option<String>,
        resource_type: String,
    },
    NetworkRequestWillBeSent {
        request_id: String,
        url: String,
        method: String,
        headers: Value,
        request: Value,
        timestamp: f64,
        resource_type: String,
    },
    NetworkResponseReceived {
        request_id: String,
        url: String,
        status: i32,
        status_text: String,
        headers: Value,
        timestamp: f64,
        resource_type: String,
    },
    NetworkLoadingFailed {
        request_id: String,
        resource_type: String,
        error_text: String,
        timestamp: f64,
    },
    DebuggerScriptParsed {
        script_id: String,
        url: String,
        start_line: i32,
        end_line: i32,
    },
    DebuggerPaused {
        call_frames: Value,
        reason: String,
        hit_breakpoints: Value,
    },
    RuntimeExceptionThrown {
        timestamp: f64,
        text: String,
        url: String,
        line: i32,
        column: i32,
        stack_trace: Value,
    },
    PageLoadEventFired {
        timestamp: f64,
    },
    PageFrameNavigated {
        frame_id: String,
        url: String,
        loader_id: String,
    },
    SecurityCertificateError {
        event_id: i32,
        error_type: String,
        url: String,
    },
}

// ---------------------------------------------------------------------------
// §2 ConsoleMessage — discriminates logs from CDP events
// ---------------------------------------------------------------------------

/// A console message from the browser engine. Either a plain log
/// (forwarded as Runtime.consoleAPICalled + Log.entryAdded) or a structured
/// CDP event that maps to a specific BaoEvent variant.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleMessage {
    Log { level: String, text: String },
    Event(BaoEvent),
}

// ---------------------------------------------------------------------------
// §3 from_console_text — JS→Rust transport parser
// ---------------------------------------------------------------------------

/// Parse a console text string into a `ConsoleMessage`.
///
/// Transport format for events:
/// ```text
/// __BAO_EVT__CDP.MethodName\n{json}
/// ```
///
/// - Lines without the `__BAO_EVT__` prefix → `ConsoleMessage::Log`
/// - Lines with unknown CDP method → `None`
/// - Malformed JSON → best-effort parse with defaults
/// - Missing newline → treat everything after the prefix as JSON
impl BaoEvent {
    pub fn from_console_text(text: &str) -> Option<ConsoleMessage> {
        let rest = match text.strip_prefix("__BAO_EVT__") {
            Some(r) => r,
            None => return None,
        };

        let (method, json_str): (_, String) = match rest.split_once('\n') {
            Some((m, j)) => (m, j.to_string()),
            None => {
                // No newline — try to split at the first '{' to separate
                // method name from JSON body.
                match rest.split_once('{') {
                    Some((m, j_rest)) => (m, format!("{{{j_rest}")),
                    None => return None,
                }
            }
        };

        let v: Value = serde_json::from_str(&json_str).unwrap_or_default();

        match method {
            "Fetch.requestPaused" => Some(ConsoleMessage::Event(BaoEvent::FetchRequestPaused {
                request_id: v["id"].as_str().unwrap_or_default().to_string(),
                url: v["url"].as_str().unwrap_or_default().to_string(),
                method: v["method"].as_str().unwrap_or_default().to_string(),
                headers: v.get("headers").cloned().unwrap_or(json!({})),
                post_data: v.get("postData").and_then(|p| p.as_str()).map(String::from),
                resource_type: v
                    .get("resourceType")
                    .and_then(|r| r.as_str())
                    .unwrap_or("Other")
                    .to_string(),
            })),
            "Network.requestWillBeSent" => {
                Some(ConsoleMessage::Event(BaoEvent::NetworkRequestWillBeSent {
                    request_id: v["id"].as_str().unwrap_or_default().to_string(),
                    url: v["url"].as_str().unwrap_or_default().to_string(),
                    method: v["method"].as_str().unwrap_or_default().to_string(),
                    headers: v.get("headers").cloned().unwrap_or(json!({})),
                    request: v.get("request").cloned().unwrap_or_else(|| {
                        json!({
                            "url": v["url"],
                            "method": v["method"],
                        })
                    }),
                    timestamp: v.get("timestamp").and_then(|t| t.as_f64()).unwrap_or(0.0),
                    resource_type: v
                        .get("type")
                        .and_then(|r| r.as_str())
                        .unwrap_or("Other")
                        .to_string(),
                }))
            }
            "Network.responseReceived" => {
                Some(ConsoleMessage::Event(BaoEvent::NetworkResponseReceived {
                    request_id: v["id"].as_str().unwrap_or_default().to_string(),
                    url: v["url"].as_str().unwrap_or_default().to_string(),
                    status: v["status"].as_i64().unwrap_or(0) as i32,
                    status_text: v
                        .get("statusText")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    headers: v.get("headers").cloned().unwrap_or(json!({})),
                    timestamp: v.get("timestamp").and_then(|t| t.as_f64()).unwrap_or(0.0),
                    resource_type: v
                        .get("type")
                        .and_then(|r| r.as_str())
                        .unwrap_or("Other")
                        .to_string(),
                }))
            }
            "Network.loadingFailed" => {
                Some(ConsoleMessage::Event(BaoEvent::NetworkLoadingFailed {
                    request_id: v["id"].as_str().unwrap_or_default().to_string(),
                    resource_type: v
                        .get("type")
                        .and_then(|r| r.as_str())
                        .unwrap_or("Other")
                        .to_string(),
                    error_text: v
                        .get("errorText")
                        .and_then(|e| e.as_str())
                        .unwrap_or("Network error")
                        .to_string(),
                    timestamp: v.get("timestamp").and_then(|t| t.as_f64()).unwrap_or(0.0),
                }))
            }
            "Debugger.scriptParsed" => {
                Some(ConsoleMessage::Event(BaoEvent::DebuggerScriptParsed {
                    script_id: v["id"].as_str().unwrap_or_default().to_string(),
                    url: v["url"].as_str().unwrap_or_default().to_string(),
                    start_line: v.get("startLine").and_then(|l| l.as_i64()).unwrap_or(0) as i32,
                    end_line: v.get("endLine").and_then(|l| l.as_i64()).unwrap_or(0) as i32,
                }))
            }
            "Debugger.paused" => Some(ConsoleMessage::Event(BaoEvent::DebuggerPaused {
                call_frames: v.get("callFrames").cloned().unwrap_or(json!([])),
                reason: v
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("other")
                    .to_string(),
                hit_breakpoints: v.get("hitBreakpoints").cloned().unwrap_or(json!([])),
            })),
            "Runtime.exceptionThrown" => {
                Some(ConsoleMessage::Event(BaoEvent::RuntimeExceptionThrown {
                    timestamp: v.get("timestamp").and_then(|t| t.as_f64()).unwrap_or(0.0),
                    text: v
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    url: v
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    line: v.get("line").and_then(|l| l.as_i64()).unwrap_or(0) as i32,
                    column: v.get("column").and_then(|c| c.as_i64()).unwrap_or(0) as i32,
                    stack_trace: v.get("stackTrace").cloned().unwrap_or(Value::Null),
                }))
            }
            "Page.loadEventFired" => Some(ConsoleMessage::Event(BaoEvent::PageLoadEventFired {
                timestamp: v.get("timestamp").and_then(|t| t.as_f64()).unwrap_or(0.0),
            })),
            "Page.frameNavigated" => Some(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id: v["frameId"].as_str().unwrap_or("0").to_string(),
                url: v["url"].as_str().unwrap_or_default().to_string(),
                loader_id: v["loaderId"].as_str().unwrap_or_default().to_string(),
            })),
            "Security.certificateError" => {
                Some(ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                    event_id: v["eventId"].as_i64().unwrap_or(0) as i32,
                    error_type: v["errorType"].as_str().unwrap_or_default().to_string(),
                    url: v["url"].as_str().unwrap_or_default().to_string(),
                }))
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// §4 broadcast — emit CDP events via EventSender
// ---------------------------------------------------------------------------

/// Broadcast a `BaoEvent` as the correct CDP event(s) through the
/// `EventSender`. The JSON shapes exactly match the existing server.rs
/// implementation (lines 140–254).
impl BaoEvent {
    pub fn broadcast(&self, sender: &dyn EventSender) {
        match self {
            BaoEvent::FetchRequestPaused {
                request_id,
                url,
                method,
                headers,
                post_data: _,
                resource_type,
            } => {
                sender.send_event(
                    "Fetch.requestPaused",
                    serde_json::json!({
                        "requestId": request_id,
                        "request": {
                            "url": url,
                            "method": method,
                            "headers": headers,
                        },
                        "resourceType": resource_type,
                        "networkStage": "Request",
                    }),
                );
            }
            BaoEvent::NetworkRequestWillBeSent {
                request_id,
                request,
                timestamp,
                resource_type,
                ..
            } => {
                sender.send_event(
                    "Network.requestWillBeSent",
                    serde_json::json!({
                        "requestId": request_id,
                        "request": request,
                        "timestamp": *timestamp,
                        "type": resource_type,
                    }),
                );
            }
            BaoEvent::NetworkResponseReceived {
                request_id,
                url,
                status,
                status_text,
                headers,
                timestamp,
                resource_type,
            } => {
                sender.send_event(
                    "Network.responseReceived",
                    serde_json::json!({
                        "requestId": request_id,
                        "response": {
                            "url": url,
                            "status": *status,
                            "statusText": status_text,
                            "headers": headers,
                        },
                        "timestamp": *timestamp,
                        "type": resource_type,
                    }),
                );
                sender.send_event(
                    "Network.loadingFinished",
                    serde_json::json!({
                        "requestId": request_id,
                        "timestamp": *timestamp,
                    }),
                );
            }
            BaoEvent::NetworkLoadingFailed {
                request_id,
                resource_type,
                error_text,
                timestamp,
            } => {
                sender.send_event(
                    "Network.loadingFailed",
                    serde_json::json!({
                        "requestId": request_id,
                        "type": resource_type,
                        "errorText": error_text,
                        "timestamp": *timestamp,
                    }),
                );
            }
            BaoEvent::DebuggerScriptParsed {
                script_id,
                url,
                start_line,
                end_line,
            } => {
                sender.send_event(
                    "Debugger.scriptParsed",
                    serde_json::json!({
                        "scriptId": script_id,
                        "url": url,
                        "startLine": *start_line,
                        "endLine": *end_line,
                    }),
                );
            }
            BaoEvent::DebuggerPaused {
                call_frames,
                reason,
                hit_breakpoints,
            } => {
                sender.send_event(
                    "Debugger.paused",
                    serde_json::json!({
                        "callFrames": call_frames,
                        "reason": reason,
                        "hitBreakpoints": hit_breakpoints,
                    }),
                );
            }
            BaoEvent::RuntimeExceptionThrown {
                timestamp,
                text,
                url,
                line,
                column,
                stack_trace,
            } => {
                sender.send_event(
                    "Runtime.exceptionThrown",
                    serde_json::json!({
                        "timestamp": *timestamp,
                        "exceptionDetails": {
                            "text": text,
                            "url": url,
                            "lineNumber": *line,
                            "columnNumber": *column,
                            "stackTrace": stack_trace,
                        },
                    }),
                );
            }
            BaoEvent::PageLoadEventFired { timestamp } => {
                sender.send_event(
                    "Page.loadEventFired",
                    serde_json::json!({
                        "timestamp": *timestamp,
                    }),
                );
            }
            BaoEvent::PageFrameNavigated {
                frame_id,
                url,
                loader_id,
            } => {
                sender.send_event(
                    "Page.frameNavigated",
                    serde_json::json!({
                        "frame": {
                            "id": frame_id,
                            "url": url,
                            "loaderId": loader_id,
                            "mimeType": "text/html",
                            "securityOrigin": "",
                            "secureContextType": "Secure",
                        },
                    }),
                );
            }
            BaoEvent::SecurityCertificateError {
                event_id,
                error_type,
                url,
            } => {
                sender.send_event(
                    "Security.certificateError",
                    serde_json::json!({
                        "eventId": event_id,
                        "errorType": error_type,
                        "resourceUrl": url,
                    }),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// §5 Tests — TDD comprehensive coverage
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Recording EventSender for broadcast verification.
    struct RecordingSender {
        events: RefCell<Vec<(String, Value)>>,
    }

    impl RecordingSender {
        fn new() -> Self {
            RecordingSender {
                events: RefCell::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<(String, Value)> {
            self.events.borrow().clone()
        }
    }

    impl EventSender for RecordingSender {
        fn send_event(&self, method: &str, params: Value) {
            self.events.borrow_mut().push((method.to_string(), params));
        }
    }

    // Make RecordingSender Send + Sync (RefCell is single-threaded, tests are
    // single-threaded).
    unsafe impl Send for RecordingSender {}
    unsafe impl Sync for RecordingSender {}

    // ---- from_console_text tests ----

    #[test]
    fn from_console_text_fetch_request_paused() {
        let input = "__BAO_EVT__Fetch.requestPaused\n{\"id\":\"r1\",\"url\":\"http://test.com\",\"method\":\"GET\",\"headers\":{\"X-Custom\":\"val\"},\"resourceType\":\"Document\"}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::FetchRequestPaused {
                request_id,
                url,
                method,
                headers,
                post_data,
                resource_type,
            }) => {
                assert_eq!(request_id, "r1");
                assert_eq!(url, "http://test.com");
                assert_eq!(method, "GET");
                assert_eq!(headers["X-Custom"], "val");
                assert!(post_data.is_none());
                assert_eq!(resource_type, "Document");
            }
            other => panic!("expected FetchRequestPaused, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_fetch_request_paused_with_post_data() {
        let input = "__BAO_EVT__Fetch.requestPaused\n{\"id\":\"r2\",\"url\":\"http://test.com\",\"method\":\"POST\",\"headers\":{},\"postData\":\"body\",\"resourceType\":\"XHR\"}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::FetchRequestPaused { post_data, .. }) => {
                assert_eq!(post_data, Some("body".to_string()));
            }
            other => panic!("expected FetchRequestPaused, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_network_request() {
        let input = "__BAO_EVT__Network.requestWillBeSent\n{\"id\":\"req1\",\"url\":\"http://example.com\",\"method\":\"GET\",\"headers\":{},\"request\":{\"url\":\"http://example.com\",\"method\":\"GET\"},\"timestamp\":12345.0,\"type\":\"Document\"}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::NetworkRequestWillBeSent {
                request_id,
                url,
                method,
                timestamp,
                resource_type,
                ..
            }) => {
                assert_eq!(request_id, "req1");
                assert_eq!(url, "http://example.com");
                assert_eq!(method, "GET");
                assert_eq!(timestamp, 12345.0);
                assert_eq!(resource_type, "Document");
            }
            other => panic!("expected NetworkRequestWillBeSent, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_network_request_defaults_when_request_missing() {
        let input = "__BAO_EVT__Network.requestWillBeSent\n{\"id\":\"req1\",\"url\":\"http://example.com\",\"method\":\"GET\",\"headers\":{},\"timestamp\":100.0,\"type\":\"Script\"}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::NetworkRequestWillBeSent { request, .. }) => {
                assert_eq!(request["url"], "http://example.com");
                assert_eq!(request["method"], "GET");
            }
            other => panic!("expected NetworkRequestWillBeSent, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_network_response() {
        let input = "__BAO_EVT__Network.responseReceived\n{\"id\":\"req2\",\"url\":\"http://example.com\",\"status\":200,\"statusText\":\"OK\",\"headers\":{\"Content-Type\":\"text/html\"},\"timestamp\":12346.0,\"type\":\"Document\"}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::NetworkResponseReceived {
                request_id,
                url,
                status,
                status_text,
                headers,
                timestamp,
                resource_type,
            }) => {
                assert_eq!(request_id, "req2");
                assert_eq!(url, "http://example.com");
                assert_eq!(status, 200);
                assert_eq!(status_text, "OK");
                assert_eq!(headers["Content-Type"], "text/html");
                assert_eq!(timestamp, 12346.0);
                assert_eq!(resource_type, "Document");
            }
            other => panic!("expected NetworkResponseReceived, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_network_loading_failed() {
        let input = "__BAO_EVT__Network.loadingFailed\n{\"id\":\"req3\",\"type\":\"XHR\",\"timestamp\":12347.0}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::NetworkLoadingFailed {
                request_id,
                resource_type,
                error_text,
                timestamp,
            }) => {
                assert_eq!(request_id, "req3");
                assert_eq!(resource_type, "XHR");
                assert_eq!(error_text, "Network error");
                assert_eq!(timestamp, 12347.0);
            }
            other => panic!("expected NetworkLoadingFailed, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_network_loading_failed_with_custom_error() {
        let input = "__BAO_EVT__Network.loadingFailed\n{\"id\":\"req3\",\"type\":\"XHR\",\"errorText\":\"Connection refused\",\"timestamp\":100.0}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::NetworkLoadingFailed { error_text, .. }) => {
                assert_eq!(error_text, "Connection refused");
            }
            other => panic!("expected NetworkLoadingFailed, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_debugger_script_parsed() {
        let input = "__BAO_EVT__Debugger.scriptParsed\n{\"id\":\"1\",\"url\":\"test.js\",\"startLine\":0,\"endLine\":10}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::DebuggerScriptParsed {
                script_id,
                url,
                start_line,
                end_line,
            }) => {
                assert_eq!(script_id, "1");
                assert_eq!(url, "test.js");
                assert_eq!(start_line, 0);
                assert_eq!(end_line, 10);
            }
            other => panic!("expected DebuggerScriptParsed, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_debugger_paused() {
        let input = "__BAO_EVT__Debugger.paused\n{\"callFrames\":[],\"reason\":\"breakpoint\",\"hitBreakpoints\":[]}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::DebuggerPaused {
                call_frames,
                reason,
                hit_breakpoints,
            }) => {
                assert_eq!(call_frames, json!([]));
                assert_eq!(reason, "breakpoint");
                assert_eq!(hit_breakpoints, json!([]));
            }
            other => panic!("expected DebuggerPaused, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_runtime_exception() {
        let input = "__BAO_EVT__Runtime.exceptionThrown\n{\"timestamp\":12348.0,\"text\":\"TypeError: x is not a function\",\"url\":\"test.js\",\"line\":10,\"column\":5,\"stackTrace\":null}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::RuntimeExceptionThrown {
                timestamp,
                text,
                url,
                line,
                column,
                stack_trace,
            }) => {
                assert_eq!(timestamp, 12348.0);
                assert_eq!(text, "TypeError: x is not a function");
                assert_eq!(url, "test.js");
                assert_eq!(line, 10);
                assert_eq!(column, 5);
                assert!(stack_trace.is_null());
            }
            other => panic!("expected RuntimeExceptionThrown, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_page_load() {
        let input = "__BAO_EVT__Page.loadEventFired\n{\"timestamp\":12345.0}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp }) => {
                assert_eq!(timestamp, 12345.0);
            }
            other => panic!("expected PageLoadEventFired, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_unknown_method_returns_none() {
        let input = "__BAO_EVT__Unknown.method\n{}";
        assert!(BaoEvent::from_console_text(input).is_none());
    }

    #[test]
    fn from_console_text_no_prefix_returns_none() {
        let input = "plain text log message";
        assert!(BaoEvent::from_console_text(input).is_none());
    }

    #[test]
    fn from_console_text_malformed_json_still_parses() {
        // Malformed JSON → serde returns default Value::Null → defaults used
        let input = "__BAO_EVT__Page.loadEventFired\n{bad json}";
        let msg = BaoEvent::from_console_text(input).expect("should parse with defaults");
        match msg {
            ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp }) => {
                assert_eq!(timestamp, 0.0);
            }
            other => panic!("expected PageLoadEventFired, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_missing_newline() {
        // No newline — method + JSON in one string after prefix
        let input = "__BAO_EVT__Page.loadEventFired{\"timestamp\":1.0}";
        let msg = BaoEvent::from_console_text(input).expect("should parse without newline");
        match msg {
            ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp }) => {
                assert_eq!(timestamp, 1.0);
            }
            other => panic!("expected PageLoadEventFired, got {:?}", other),
        }
    }

    // ---- broadcast tests ----

    #[test]
    fn broadcast_fetch_request_paused() {
        let sender = RecordingSender::new();
        let event = BaoEvent::FetchRequestPaused {
            request_id: "r1".into(),
            url: "http://test.com".into(),
            method: "GET".into(),
            headers: json!({"X-Custom": "val"}),
            post_data: None,
            resource_type: "Document".into(),
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Fetch.requestPaused");
        let params = &events[0].1;
        assert_eq!(params["requestId"], "r1");
        assert_eq!(params["request"]["url"], "http://test.com");
        assert_eq!(params["request"]["method"], "GET");
        assert_eq!(params["request"]["headers"]["X-Custom"], "val");
        assert_eq!(params["resourceType"], "Document");
        assert_eq!(params["networkStage"], "Request");
    }

    #[test]
    fn broadcast_network_request_will_be_sent() {
        let sender = RecordingSender::new();
        let event = BaoEvent::NetworkRequestWillBeSent {
            request_id: "req1".into(),
            url: "http://example.com".into(),
            method: "GET".into(),
            headers: json!({}),
            request: json!({"url": "http://example.com", "method": "GET"}),
            timestamp: 12345.0,
            resource_type: "Document".into(),
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Network.requestWillBeSent");
        let params = &events[0].1;
        assert_eq!(params["requestId"], "req1");
        assert_eq!(params["request"]["url"], "http://example.com");
        assert_eq!(params["timestamp"], 12345.0);
        assert_eq!(params["type"], "Document");
    }

    #[test]
    fn broadcast_network_response_received_also_emits_loading_finished() {
        let sender = RecordingSender::new();
        let event = BaoEvent::NetworkResponseReceived {
            request_id: "req2".into(),
            url: "http://example.com".into(),
            status: 200,
            status_text: "OK".into(),
            headers: json!({"Content-Type": "text/html"}),
            timestamp: 12346.0,
            resource_type: "Document".into(),
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 2);

        assert_eq!(events[0].0, "Network.responseReceived");
        let p0 = &events[0].1;
        assert_eq!(p0["requestId"], "req2");
        assert_eq!(p0["response"]["url"], "http://example.com");
        assert_eq!(p0["response"]["status"], 200);
        assert_eq!(p0["response"]["statusText"], "OK");
        assert_eq!(p0["response"]["headers"]["Content-Type"], "text/html");
        assert_eq!(p0["timestamp"], 12346.0);
        assert_eq!(p0["type"], "Document");

        assert_eq!(events[1].0, "Network.loadingFinished");
        let p1 = &events[1].1;
        assert_eq!(p1["requestId"], "req2");
        assert_eq!(p1["timestamp"], 12346.0);
    }

    #[test]
    fn broadcast_network_loading_failed() {
        let sender = RecordingSender::new();
        let event = BaoEvent::NetworkLoadingFailed {
            request_id: "req3".into(),
            resource_type: "XHR".into(),
            error_text: "Network error".into(),
            timestamp: 12347.0,
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Network.loadingFailed");
        let params = &events[0].1;
        assert_eq!(params["requestId"], "req3");
        assert_eq!(params["type"], "XHR");
        assert_eq!(params["errorText"], "Network error");
        assert_eq!(params["timestamp"], 12347.0);
    }

    #[test]
    fn broadcast_debugger_script_parsed() {
        let sender = RecordingSender::new();
        let event = BaoEvent::DebuggerScriptParsed {
            script_id: "1".into(),
            url: "test.js".into(),
            start_line: 0,
            end_line: 10,
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Debugger.scriptParsed");
        let params = &events[0].1;
        assert_eq!(params["scriptId"], "1");
        assert_eq!(params["url"], "test.js");
        assert_eq!(params["startLine"], 0);
        assert_eq!(params["endLine"], 10);
    }

    #[test]
    fn broadcast_debugger_paused() {
        let sender = RecordingSender::new();
        let event = BaoEvent::DebuggerPaused {
            call_frames: json!([]),
            reason: "breakpoint".into(),
            hit_breakpoints: json!([]),
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Debugger.paused");
        let params = &events[0].1;
        assert_eq!(params["callFrames"], json!([]));
        assert_eq!(params["reason"], "breakpoint");
        assert_eq!(params["hitBreakpoints"], json!([]));
    }

    #[test]
    fn broadcast_runtime_exception_thrown() {
        let sender = RecordingSender::new();
        let event = BaoEvent::RuntimeExceptionThrown {
            timestamp: 12348.0,
            text: "TypeError: x is not a function".into(),
            url: "test.js".into(),
            line: 10,
            column: 5,
            stack_trace: Value::Null,
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Runtime.exceptionThrown");
        let params = &events[0].1;
        assert_eq!(params["timestamp"], 12348.0);
        let details = &params["exceptionDetails"];
        assert_eq!(details["text"], "TypeError: x is not a function");
        assert_eq!(details["url"], "test.js");
        assert_eq!(details["lineNumber"], 10);
        assert_eq!(details["columnNumber"], 5);
        assert!(details["stackTrace"].is_null());
    }

    #[test]
    fn broadcast_page_load_event_fired() {
        let sender = RecordingSender::new();
        let event = BaoEvent::PageLoadEventFired { timestamp: 12345.0 };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Page.loadEventFired");
        let params = &events[0].1;
        assert_eq!(params["timestamp"], 12345.0);
    }

    #[test]
    fn broadcast_produces_correct_cdp_events() {
        // End-to-end: from_console_text → broadcast → verify CDP output
        let sender = RecordingSender::new();

        let input = "__BAO_EVT__Fetch.requestPaused\n{\"id\":\"r1\",\"url\":\"http://test.com\",\"method\":\"GET\",\"headers\":{},\"resourceType\":\"Document\"}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        if let ConsoleMessage::Event(evt) = msg {
            evt.broadcast(&sender);
        }

        let input = "__BAO_EVT__Page.loadEventFired\n{\"timestamp\":999.0}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        if let ConsoleMessage::Event(evt) = msg {
            evt.broadcast(&sender);
        }

        let events = sender.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "Fetch.requestPaused");
        assert_eq!(events[0].1["requestId"], "r1");
        assert_eq!(events[1].0, "Page.loadEventFired");
        assert_eq!(events[1].1["timestamp"], 999.0);
    }

    #[test]
    fn console_message_log_variant() {
        let log = ConsoleMessage::Log {
            level: "info".into(),
            text: "hello".into(),
        };
        match log {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "info");
                assert_eq!(text, "hello");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    #[test]
    fn console_message_event_variant() {
        let evt = ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp: 1.0 });
        match evt {
            ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp }) => {
                assert_eq!(timestamp, 1.0);
            }
            _ => panic!("expected Event(PageLoadEventFired)"),
        }
    }

    // ---- PageFrameNavigated tests ----

    #[test]
    fn from_console_text_page_frame_navigated() {
        let input = "__BAO_EVT__Page.frameNavigated\n{\"frameId\":\"0\",\"url\":\"https://example.com\",\"loaderId\":\"abc\"}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id,
                url,
                loader_id,
            }) => {
                assert_eq!(frame_id, "0");
                assert_eq!(url, "https://example.com");
                assert_eq!(loader_id, "abc");
            }
            other => panic!("expected PageFrameNavigated, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_page_frame_navigated_defaults() {
        let input = "__BAO_EVT__Page.frameNavigated\n{}";
        let msg = BaoEvent::from_console_text(input).expect("should parse with defaults");
        match msg {
            ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id,
                url,
                loader_id,
            }) => {
                assert_eq!(frame_id, "0");
                assert_eq!(url, "");
                assert_eq!(loader_id, "");
            }
            other => panic!("expected PageFrameNavigated, got {:?}", other),
        }
    }

    #[test]
    fn broadcast_page_frame_navigated() {
        let sender = RecordingSender::new();
        let event = BaoEvent::PageFrameNavigated {
            frame_id: "0".into(),
            url: "https://example.com".into(),
            loader_id: "abc".into(),
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Page.frameNavigated");
        let params = &events[0].1;
        assert_eq!(params["frame"]["id"], "0");
        assert_eq!(params["frame"]["url"], "https://example.com");
        assert_eq!(params["frame"]["loaderId"], "abc");
        assert_eq!(params["frame"]["mimeType"], "text/html");
        assert_eq!(params["frame"]["securityOrigin"], "");
        assert_eq!(params["frame"]["secureContextType"], "Secure");
    }

    // ---- SecurityCertificateError tests ----

    #[test]
    fn from_console_text_security_certificate_error() {
        let input = "__BAO_EVT__Security.certificateError\n{\"eventId\":1,\"errorType\":\"net::ERR_CERT_AUTHORITY_INVALID\",\"url\":\"https://bad.example.com\"}";
        let msg = BaoEvent::from_console_text(input).expect("should parse");
        match msg {
            ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                event_id,
                error_type,
                url,
            }) => {
                assert_eq!(event_id, 1);
                assert_eq!(error_type, "net::ERR_CERT_AUTHORITY_INVALID");
                assert_eq!(url, "https://bad.example.com");
            }
            other => panic!("expected SecurityCertificateError, got {:?}", other),
        }
    }

    #[test]
    fn from_console_text_security_certificate_error_defaults() {
        let input = "__BAO_EVT__Security.certificateError\n{}";
        let msg = BaoEvent::from_console_text(input).expect("should parse with defaults");
        match msg {
            ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                event_id,
                error_type,
                url,
            }) => {
                assert_eq!(event_id, 0);
                assert_eq!(error_type, "");
                assert_eq!(url, "");
            }
            other => panic!("expected SecurityCertificateError, got {:?}", other),
        }
    }

    #[test]
    fn broadcast_security_certificate_error() {
        let sender = RecordingSender::new();
        let event = BaoEvent::SecurityCertificateError {
            event_id: 1,
            error_type: "net::ERR_CERT_AUTHORITY_INVALID".into(),
            url: "https://bad.example.com".into(),
        };
        event.broadcast(&sender);
        let events = sender.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "Security.certificateError");
        let params = &events[0].1;
        assert_eq!(params["eventId"], 1);
        assert_eq!(params["errorType"], "net::ERR_CERT_AUTHORITY_INVALID");
        assert_eq!(params["resourceUrl"], "https://bad.example.com");
    }
}
