// @trace TEST-FINAL-REM-CDP [req:REQ-CDP-001] [level:unit]
// CDP-side integration tests for final-remaining plan:
// T7: log crate (verified via grep)
// T8: DomainDispatch enum dispatch
// BaoEvent parsing and broadcast

use cdp_server::{
    BaoEvent, CdpError, ConsoleMessage, DomainHandler, DomainRegistry, EmptyHandler, EventSender,
};
use serde_json::{json, Value};

struct NoopSender;
impl EventSender for NoopSender {
    fn send_event(&self, _: &str, _: Value) {}
}

/// Helper: parse text to BaoEvent, asserting it's an Event variant.
fn parse_event(text: &str) -> BaoEvent {
    match BaoEvent::from_console_text(text) {
        Some(ConsoleMessage::Event(evt)) => evt,
        other => panic!("expected ConsoleMessage::Event, got {:?}", other),
    }
}

// ─── BaoEvent parsing ────────────────────────────────────────────────────

#[test]
fn bao_event_parse_fetch_request_paused() {
    let evt = parse_event(
        "__BAO_EVT__Fetch.requestPaused\n{\"requestId\":\"req-1\",\"url\":\"https://example.com\",\"method\":\"GET\"}"
    );
    evt.broadcast(&NoopSender);
}

#[test]
fn bao_event_parse_network_request() {
    let evt = parse_event(
        "__BAO_EVT__Network.requestWillBeSent\n{\"requestId\":\"n-1\",\"url\":\"https://test.com\",\"method\":\"POST\"}"
    );
    evt.broadcast(&NoopSender);
}

#[test]
fn bao_event_parse_debugger_script_parsed() {
    let evt = parse_event(
        "__BAO_EVT__Debugger.scriptParsed\n{\"scriptId\":\"42\",\"url\":\"app.js\",\"startLine\":0,\"endLine\":100}"
    );
    evt.broadcast(&NoopSender);
}

#[test]
fn bao_event_parse_debugger_paused() {
    let evt = parse_event(
        "__BAO_EVT__Debugger.paused\n{\"callFrames\":[],\"reason\":\"breakpoint\",\"hitBreakpoints\":[\"1:0:0\"]}"
    );
    evt.broadcast(&NoopSender);
}

#[test]
fn bao_event_parse_runtime_exception() {
    let evt = parse_event(
        "__BAO_EVT__Runtime.exceptionThrown\n{\"timestamp\":1000,\"text\":\"TypeError\",\"url\":\"script.js\",\"line\":10,\"column\":5}"
    );
    evt.broadcast(&NoopSender);
}

#[test]
fn bao_event_parse_page_load() {
    let evt = parse_event("__BAO_EVT__Page.loadEventFired\n{\"timestamp\":2000}");
    evt.broadcast(&NoopSender);
}

#[test]
fn bao_event_reject_unknown_method() {
    assert!(BaoEvent::from_console_text("__BAO_EVT__Foo.bar\n{}").is_none());
}

#[test]
fn bao_event_reject_no_prefix() {
    assert!(BaoEvent::from_console_text("regular log message").is_none());
}

#[test]
fn bao_event_malformed_json_uses_defaults() {
    let result = BaoEvent::from_console_text("__BAO_EVT__Debugger.paused\nnot-json");
    assert!(
        result.is_some(),
        "malformed JSON should still parse with defaults"
    );
}

#[test]
fn bao_event_reject_empty_after_prefix() {
    assert!(BaoEvent::from_console_text("__BAO_EVT__").is_none());
}

// ─── ConsoleMessage ─────────────────────────────────────────────────────

#[test]
fn console_message_event_variant() {
    let msg =
        BaoEvent::from_console_text("__BAO_EVT__Page.loadEventFired\n{\"timestamp\":0}").unwrap();
    match &msg {
        ConsoleMessage::Event(_) => {}
        ConsoleMessage::Log { .. } => panic!("expected Event variant"),
    }
}

#[test]
fn console_message_log_variant() {
    let msg = ConsoleMessage::Log {
        level: "info".into(),
        text: "hello".into(),
    };
    match &msg {
        ConsoleMessage::Log { level, text } => {
            assert_eq!(level, "info");
            assert_eq!(text, "hello");
        }
        ConsoleMessage::Event(_) => panic!("expected Log variant"),
    }
}

// ─── DomainRegistry with concrete handler type ───────────────────────────

struct MockH {
    name: &'static str,
}
impl DomainHandler for MockH {
    fn domain_name(&self) -> &'static str {
        self.name
    }
    fn handle_command(&self, cmd: &str, _: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Ok(json!({"domain": self.name, "cmd": cmd}))
    }
}

#[test]
fn registry_concrete_handler_works() {
    let reg = DomainRegistry::<MockH>::new();
    reg.register(MockH { name: "TestDomain" }).unwrap();
    assert!(reg.has_domain("TestDomain"));
}

// ─── DomainRegistry with enum dispatch ───────────────────────────────────

enum TestDispatch {
    Alpha(MockH),
    Beta(MockH),
}

impl DomainHandler for TestDispatch {
    fn domain_name(&self) -> &'static str {
        match self {
            Self::Alpha(h) => h.domain_name(),
            Self::Beta(h) => h.domain_name(),
        }
    }
    fn handle_command(&self, cmd: &str, p: Value, s: &dyn EventSender) -> Result<Value, CdpError> {
        match self {
            Self::Alpha(h) => h.handle_command(cmd, p, s),
            Self::Beta(h) => h.handle_command(cmd, p, s),
        }
    }
}

#[test]
fn registry_enum_dispatch_works() {
    let reg = DomainRegistry::<TestDispatch>::new();
    reg.register(TestDispatch::Alpha(MockH { name: "Alpha" }))
        .unwrap();
    reg.register(TestDispatch::Beta(MockH { name: "Beta" }))
        .unwrap();
    assert!(reg.has_domain("Alpha"));
    assert!(reg.has_domain("Beta"));

    let result = reg.dispatch_command("Alpha.doSomething", json!({}), &NoopSender);
    assert!(result.is_some());
    let val = result.unwrap().unwrap();
    assert_eq!(val["domain"], "Alpha");
    assert_eq!(val["cmd"], "Alpha.doSomething");
}

#[test]
fn registry_enum_dispatch_duplicate_rejected() {
    let reg = DomainRegistry::<TestDispatch>::new();
    reg.register(TestDispatch::Alpha(MockH { name: "X" }))
        .unwrap();
    let err = reg
        .register(TestDispatch::Alpha(MockH { name: "X" }))
        .unwrap_err();
    assert!(err.contains("already registered"));
}
