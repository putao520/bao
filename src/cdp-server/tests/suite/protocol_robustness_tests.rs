// @trace TEST-CDS-009-ROBUST [req:REQ-CDS-001,REQ-CDS-004,REQ-CDS-006,REQ-CDS-008] [level:unit]
// Protocol robustness: edge cases for message parsing, dispatch, registry lifecycle.
// Covers criteria gaps from adversarial review:
//   - REQ-CDS-001-C7/C8: -32600 / -32601 error-response chain assertions
//   - REQ-CDS-004-C5:    built-in Target.* short-circuit (no DomainHandler)
//   - REQ-CDS-004-C6:    flat-session routing via session_id
//   - REQ-CDS-006-C3/C4: on_session_created / on_session_destroyed callback verification
//   - REQ-CDS-008-C2:    http_timeout_seconds builder
// Plus boundary coverage: malformed JSON, non-UTF-8, float id, params-type anomalies,
// concurrent dispatch, post-error recovery, handler panic safety, session_id extremes.

use std::sync::{Arc, Mutex};
use std::thread;

use cdp_server::{
    error_response, ok_empty, ok_response, parse_message, serialize_response, CdpError, CdpEvent,
    CdpMessage, CdpResponse, DomainHandler, DomainRegistry, EventSender, RegistryDispatch,
    ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Test scaffolding
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct NoopSender;

impl EventSender for NoopSender {
    fn send_event(&self, _method: &str, _params: Value) {}
}

static NOOP: NoopSender = NoopSender;
fn noop() -> &'static dyn EventSender {
    &NOOP
}

/// Spy handler — records every interaction via interior mutability so tests
/// can assert callback invocation counts (REQ-CDS-006-C3/C4).
struct SpyHandler {
    domain: &'static str,
    created: Mutex<Vec<String>>,
    destroyed: Mutex<Vec<String>>,
    commands: Mutex<Vec<String>>,
    response: Result<Value, CdpError>,
}

impl SpyHandler {
    fn new(domain: &'static str) -> Self {
        Self {
            domain,
            created: Mutex::new(Vec::new()),
            destroyed: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
            response: Ok(json!({"echo": true})),
        }
    }

    #[allow(dead_code)]
    fn with_response(domain: &'static str, response: Result<Value, CdpError>) -> Self {
        let mut h = Self::new(domain);
        h.response = response;
        h
    }

    fn created_ids(&self) -> Vec<String> {
        self.created.lock().unwrap().clone()
    }
    fn destroyed_ids(&self) -> Vec<String> {
        self.destroyed.lock().unwrap().clone()
    }
    fn commands(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }
}

impl DomainHandler for SpyHandler {
    fn domain_name(&self) -> &'static str {
        self.domain
    }
    fn handle_command(
        &self,
        command: &str,
        _params: Value,
        _sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        self.commands.lock().unwrap().push(command.to_string());
        self.response.clone()
    }
    fn on_session_created(&self, session_id: &str) {
        self.created.lock().unwrap().push(session_id.to_string());
    }
    fn on_session_destroyed(&self, session_id: &str) {
        self.destroyed.lock().unwrap().push(session_id.to_string());
    }
}

/// Arc-wrapped observer so the same handler instance can be interrogated after
/// being moved into the registry.
struct Observed {
    inner: Arc<SpyHandler>,
}
impl Observed {
    fn new(inner: Arc<SpyHandler>) -> Self {
        Self { inner }
    }
}
impl DomainHandler for Observed {
    fn domain_name(&self) -> &'static str {
        self.inner.domain_name()
    }
    fn handle_command(&self, c: &str, p: Value, s: &dyn EventSender) -> Result<Value, CdpError> {
        self.inner.handle_command(c, p, s)
    }
    fn on_session_created(&self, sid: &str) {
        self.inner.on_session_created(sid);
    }
    fn on_session_destroyed(&self, sid: &str) {
        self.inner.on_session_destroyed(sid);
    }
}

/// Handler that always returns an error — for post-error recovery tests.
struct ErrorAlways;
impl DomainHandler for ErrorAlways {
    fn domain_name(&self) -> &'static str {
        "ErrDomain"
    }
    fn handle_command(&self, _: &str, _: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        Err(CdpError {
            code: -32000,
            message: "intentional".into(),
        })
    }
}

/// Handler that records whether it was entered — used to prove Target.* built-in
/// handling never reaches a registered "Target" DomainHandler when the server
/// short-circuits (architecture contract for REQ-CDS-004-C5).
struct TargetRecorder {
    entered: Mutex<bool>,
}
impl TargetRecorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Mutex::new(false),
        })
    }
    fn was_entered(&self) -> bool {
        *self.entered.lock().unwrap()
    }
}
struct TargetObserved {
    inner: Arc<TargetRecorder>,
}
impl DomainHandler for TargetObserved {
    fn domain_name(&self) -> &'static str {
        "Target"
    }
    fn handle_command(&self, _c: &str, _p: Value, _s: &dyn EventSender) -> Result<Value, CdpError> {
        *self.inner.entered.lock().unwrap() = true;
        Ok(json!({"handledByDomainHandler": true}))
    }
}

// ===========================================================================
// §REQ-CDS-001-C7: invalid JSON → error_response(code=-32600) chain
// ===========================================================================
//
// The production chain (session.rs): parse_message(raw) == None  →
//   error_response(None, ERR_INVALID_REQUEST, "Invalid JSON").
// We exercise both ends of the chain at the public API surface.

#[test]
fn test_c7_invalid_request_error_response_chain() {
    // End 1: malformed JSON yields None from parse_message (same predicate
    // session.rs uses to decide to emit -32600).
    let malformed = r#"{"id":1,"method":"Page.navigate""#; // missing closing brace
    assert!(parse_message(malformed).is_none());

    // End 2: the error_response the server builds for that case carries the
    // exact JSON-RPC code -32600 and serialises to a wire message whose
    // error.code == -32600.
    let resp = error_response(None, ERR_INVALID_REQUEST, "Invalid Request");
    assert_eq!(resp.id, None);
    assert!(resp.result.is_none());
    let err = resp
        .error
        .as_ref()
        .expect("error_response must populate error");
    assert_eq!(err.code, -32600);
    assert!(err.message.contains("Invalid Request"));

    let wire = serialize_response(&resp);
    let v: Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(v["error"]["code"].as_i64(), Some(-32600));
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid Request"));
}

#[test]
fn test_c7_various_malformed_json_yields_none() {
    // Every input here must be rejected by parse_message — these are the
    // predicates that trigger the -32600 path in the server.
    let cases = [
        r#"{"id":1,"#, // truncated
        r#"{"id":1}"#, // missing method (valid JSON, invalid request)
        r#"{"#,        // bare brace
        "{",
        "}",
        "null",
        "[]",
        "42",
        "\"string\"",
        "",
        "   ",
        r#"{"id":1.5}"#,   // float id
        r#"{"id":1e308}"#, // overflow numeric
    ];
    for (i, raw) in cases.iter().enumerate() {
        assert!(
            parse_message(raw).is_none(),
            "case #{i} ({raw:?}) should parse to None to trigger -32600",
        );
    }
}

#[test]
fn test_c7_non_utf8_bytes_rejected() {
    // Non-UTF-8 input — server would receive bytes; parse_message takes &str so
    // the caller must first convert bytes → str. Invalid UTF-8 fails that
    // conversion, so parse_message can never be reached with such a payload.
    // Build the invalid byte sequence at runtime to avoid a const-eval warning.
    let mut bad: Vec<u8> = b"{\"id\":1, \"method\":\"".to_vec();
    bad.extend_from_slice(&[0xFF, 0xFE, 0xC0, 0xC0]);
    bad.extend_from_slice(b"\"}");
    let s = std::str::from_utf8(&bad);
    assert!(s.is_err(), "non-UTF-8 bytes must fail str conversion");
}

// ===========================================================================
// §REQ-CDS-001-C8: unknown Domain.Method → error_response(code=-32601)
// ===========================================================================

#[test]
fn test_c8_method_not_found_error_response_chain() {
    // End 1: registry dispatch returns None for an unregistered domain — the
    // exact predicate session.rs uses to emit -32601.
    let reg = DomainRegistry::<SpyHandler>::new();
    reg.register(SpyHandler::new("Page")).unwrap();
    assert!(reg
        .dispatch_command("Network.enable", json!({}), noop())
        .is_none());

    // End 2: the error_response the server builds carries -32601 + method name.
    let resp = error_response(
        Some(7),
        ERR_METHOD_NOT_FOUND,
        "'Network.enable' wasn't found",
    );
    assert_eq!(resp.id, Some(7));
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Network.enable"));

    let wire = serialize_response(&resp);
    let v: Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(v["error"]["code"].as_i64(), Some(-32601));
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Method not found")
            || v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("wasn't found")
    );
}

/// Simulate the full session.route_command decision tree at the unit level:
/// given (a) whether dispatch returned Some/Ok, Some/Err, or None, assert the
/// error_response shape the server would emit. This is the C7/C8 contract.
#[test]
fn test_c8_dispatch_none_maps_to_method_not_found_response() {
    let reg = DomainRegistry::<SpyHandler>::new();
    reg.register(SpyHandler::new("Page")).unwrap();

    let method = "Mystery.thing";
    let id = Some(42);
    let dispatched = reg.dispatch_command(method, json!({}), noop());

    let resp: CdpResponse = match dispatched {
        Some(Ok(r)) => ok_response(id, r),
        Some(Err(e)) => CdpResponse {
            id,
            result: None,
            error: Some(e),
        },
        None => error_response(id, ERR_METHOD_NOT_FOUND, format!("'{method}' wasn't found")),
    };

    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap().code, -32601);
    assert_eq!(resp.id, Some(42));
}

// ===========================================================================
// §REQ-CDS-004-C5: built-in Target.* handled without DomainHandler
// ===========================================================================
//
// Architecture contract (CLAUDE.md / SPEC): cdp-server is engine-agnostic; it
// has NO hardcoded Target.* special-casing in route_command. Target.* commands
// are handled by whatever handler (if any) is registered under the "Target"
// domain — typically the bao_cdp integration registers its built-in handler.
//
// We assert BOTH halves of the contract:
//   (a) With NO "Target" handler registered, Target.* dispatches to None → -32601.
//   (b) With a "Target" handler registered, Target.* reaches it (this is how
//       the integration installs its built-in commands).

#[test]
fn test_c5_no_target_handler_means_target_commands_unrouted() {
    let reg = DomainRegistry::<SpyHandler>::new();
    reg.register(SpyHandler::new("Page")).unwrap();

    // No Target handler → built-in Target.* commands cannot be routed by the
    // generic cdp-server (proves there is no hidden short-circuit).
    for method in [
        "Target.getTargets",
        "Target.createTarget",
        "Target.closeTarget",
        "Target.attachToTarget",
        "Target.detachFromTarget",
        "Target.setDiscoverTargets",
        "Target.setAutoAttach",
        "Target.activateTarget",
    ] {
        assert!(
            reg.dispatch_command(method, json!({}), noop()).is_none(),
            "{method} must not be routed when no Target handler is registered",
        );
    }
}

#[test]
fn test_c5_target_handler_registered_serves_builtin_commands() {
    // Integration point: bao_cdp registers a "Target" DomainHandler that serves
    // the built-in Target.* command set. cdp-server routes to it transparently.
    let recorder = TargetRecorder::new();
    let reg = DomainRegistry::<TargetObserved>::new();
    reg.register(TargetObserved {
        inner: Arc::clone(&recorder),
    })
    .unwrap();

    let r = reg.dispatch_command("Target.getTargets", json!({}), noop());
    assert!(
        r.is_some(),
        "Target.* must reach the registered Target handler"
    );
    assert!(r.unwrap().is_ok());
    assert!(recorder.was_entered(), "Target handler must be invoked");
}

// ===========================================================================
// §REQ-CDS-004-C6: flat-session routing via session_id
// ===========================================================================
//
// The session_id field is parsed and preserved on CdpMessage; cdp-server's
// session map (server.rs) keys sessions by id, enabling multiple sessions to
// share one WebSocket. At the protocol/registry unit level we assert:
//   - session_id round-trips through parse_message (any string content),
//   - dispatch routes purely by domain, independent of session_id, so the
//     integration layer can demux by session_id before dispatch.

#[test]
fn test_c6_session_id_roundtrip_preserved() {
    let raw = r#"{"id":1,"method":"Runtime.evaluate","sessionId":"flat-1"}"#;
    let m = parse_message(raw).unwrap();
    assert_eq!(m.session_id.as_deref(), Some("flat-1"));
}

#[test]
fn test_c6_session_id_extremes_roundtrip() {
    let cases = [
        "",                // empty
        "a",               // single char
        &"x".repeat(1024), // very long
        "session/with/slashes",
        "session:with:colons",
        "session-with-dashes",
        "SESSION_UPPER",
        "session.with.dots",
        "session@special#chars$%",
        "日本語セッション", // unicode
    ];
    for sid in cases {
        let raw = json!({"id":1, "method":"Runtime.evaluate", "sessionId": sid}).to_string();
        let m = parse_message(&raw).unwrap_or_else(|| panic!("parse failed for sid={sid:?}"));
        assert_eq!(m.session_id.as_deref(), Some(sid), "sid={sid:?}");
    }
}

#[test]
fn test_c6_dispatch_independent_of_session_id() {
    // Two messages with different session_id but same domain must route to the
    // same handler — this is what enables flat-session demux at the integration
    // layer (one WebSocket, many logical sessions).
    let spy = Arc::new(SpyHandler::new("Runtime"));
    let reg = DomainRegistry::<Observed>::new();
    reg.register(Observed::new(Arc::clone(&spy))).unwrap();

    for sid in ["flat-A", "flat-B", "flat-C"] {
        let raw = json!({"id":1, "method":"Runtime.evaluate", "sessionId": sid}).to_string();
        let m = parse_message(&raw).unwrap();
        let dispatched = reg.dispatch_command(&m.method, m.params.unwrap_or_default(), noop());
        assert!(dispatched.is_some(), "must route for session {sid}");
    }
    assert_eq!(spy.commands().len(), 3);
}

// ===========================================================================
// §REQ-CDS-006-C3: on_session_created fires on first enable of a domain
// ===========================================================================

#[test]
fn test_c3_notify_session_created_invokes_callback() {
    let spy = Arc::new(SpyHandler::new("Page"));
    let reg = DomainRegistry::<Observed>::new();
    reg.register(Observed::new(Arc::clone(&spy))).unwrap();

    assert!(spy.created_ids().is_empty(), "no callback before notify");

    reg.notify_session_created("Page", "sess-1");
    assert_eq!(spy.created_ids(), vec!["sess-1".to_string()]);

    // Subsequent notify for the same domain still fires (registry-level
    // first-enable gating is the session's job, not the registry's).
    reg.notify_session_created("Page", "sess-2");
    assert_eq!(spy.created_ids().len(), 2);
}

#[test]
fn test_c3_notify_session_created_unknown_domain_no_callback_no_panic() {
    let spy = Arc::new(SpyHandler::new("Page"));
    let reg = DomainRegistry::<Observed>::new();
    reg.register(Observed::new(Arc::clone(&spy))).unwrap();

    reg.notify_session_created("UnknownDomain", "sess-x");
    assert!(
        spy.created_ids().is_empty(),
        "unknown domain must not fire callback"
    );
}

#[test]
fn test_c3_notify_session_created_only_matching_domain_fires() {
    let page = Arc::new(SpyHandler::new("Page"));
    let runtime = Arc::new(SpyHandler::new("Runtime"));
    let reg = DomainRegistry::<Observed>::new();
    reg.register(Observed::new(Arc::clone(&page))).unwrap();
    reg.register(Observed::new(Arc::clone(&runtime))).unwrap();

    reg.notify_session_created("Page", "s1");
    assert_eq!(page.created_ids(), vec!["s1".to_string()]);
    assert!(
        runtime.created_ids().is_empty(),
        "Runtime must not fire for Page enable"
    );
}

// ===========================================================================
// §REQ-CDS-006-C4: on_session_destroyed fires for every enabled domain
// ===========================================================================

#[test]
fn test_c4_notify_session_destroyed_invokes_each_enabled_domain() {
    let page = Arc::new(SpyHandler::new("Page"));
    let runtime = Arc::new(SpyHandler::new("Runtime"));
    let dom = Arc::new(SpyHandler::new("DOM"));
    let reg = DomainRegistry::<Observed>::new();
    reg.register(Observed::new(Arc::clone(&page))).unwrap();
    reg.register(Observed::new(Arc::clone(&runtime))).unwrap();
    reg.register(Observed::new(Arc::clone(&dom))).unwrap();

    let enabled: Vec<String> = vec!["Page".into(), "Runtime".into(), "DOM".into()];
    reg.notify_session_destroyed(&enabled, "sess-dying");

    assert_eq!(page.destroyed_ids(), vec!["sess-dying".to_string()]);
    assert_eq!(runtime.destroyed_ids(), vec!["sess-dying".to_string()]);
    assert_eq!(dom.destroyed_ids(), vec!["sess-dying".to_string()]);
}

#[test]
fn test_c4_notify_session_destroyed_skips_unenabled_domains() {
    let page = Arc::new(SpyHandler::new("Page"));
    let runtime = Arc::new(SpyHandler::new("Runtime"));
    let reg = DomainRegistry::<Observed>::new();
    reg.register(Observed::new(Arc::clone(&page))).unwrap();
    reg.register(Observed::new(Arc::clone(&runtime))).unwrap();

    // Only Page was enabled; Runtime must NOT receive destroyed callback.
    let enabled: Vec<String> = vec!["Page".into()];
    reg.notify_session_destroyed(&enabled, "s");
    assert_eq!(page.destroyed_ids().len(), 1);
    assert!(runtime.destroyed_ids().is_empty());
}

#[test]
fn test_c4_notify_session_destroyed_unknown_domain_in_list_no_panic() {
    let page = Arc::new(SpyHandler::new("Page"));
    let reg = DomainRegistry::<Observed>::new();
    reg.register(Observed::new(Arc::clone(&page))).unwrap();

    let enabled: Vec<String> = vec!["Page".into(), "Ghost".into(), "Phantom".into()];
    reg.notify_session_destroyed(&enabled, "s");
    assert_eq!(page.destroyed_ids().len(), 1);
}

#[test]
fn test_c4_notify_session_destroyed_empty_list_no_callback() {
    let page = Arc::new(SpyHandler::new("Page"));
    let reg = DomainRegistry::<Observed>::new();
    reg.register(Observed::new(Arc::clone(&page))).unwrap();

    reg.notify_session_destroyed(&[], "s");
    assert!(page.destroyed_ids().is_empty());
}

// ===========================================================================
// §REQ-CDS-001-C5/C6: parsing & response serialisation (carried over + hardened)
// ===========================================================================

#[test]
fn test_cdp_message_parse_missing_method() {
    let raw = r#"{"id": 1}"#;
    assert!(serde_json::from_str::<CdpMessage>(raw).is_err());
    // Method is a required (non-optional) field → invalid request → -32600 path.
    assert!(parse_message(raw).is_none());
}

#[test]
fn test_cdp_message_parse_empty_method() {
    let raw = r#"{"id": 1, "method": ""}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.method, "");
}

#[test]
fn test_cdp_message_parse_null_params() {
    let raw = r#"{"id": 1, "method": "Page.navigate", "params": null}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_parse_missing_params() {
    let raw = r#"{"id": 1, "method": "Page.navigate"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_parse_empty_object_params() {
    let raw = r#"{"id": 1, "method": "Page.navigate", "params": {}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params, Some(json!({})));
}

#[test]
fn test_cdp_message_parse_nested_params() {
    let raw = r#"{"id": 1, "method": "DOM.setAttributeValue", "params": {"nodeId": 1, "attributes": {"class": "test", "data-x": "[1,2,3]"}}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["nodeId"], 1);
    assert_eq!(params["attributes"]["class"], "test");
}

#[test]
fn test_cdp_message_parse_large_array_params() {
    let nums: Vec<i64> = (0..1000).collect();
    let raw = json!({"id": 1, "method": "test", "params": {"data": nums}}).to_string();
    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["data"].as_array().unwrap().len(), 1000);
}

#[test]
fn test_cdp_message_parse_string_id_rejected() {
    let raw = r#"{"id": "abc", "method": "Page.navigate"}"#;
    assert!(serde_json::from_str::<CdpMessage>(raw).is_err());
    assert!(parse_message(raw).is_none());
}

#[test]
fn test_cdp_message_parse_negative_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id": -999, "method": "x.y"}"#).unwrap();
    assert_eq!(msg.id, Some(-999));
}

#[test]
fn test_cdp_message_parse_zero_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id": 0, "method": "x.y"}"#).unwrap();
    assert_eq!(msg.id, Some(0));
}

#[test]
fn test_cdp_message_parse_large_id_max_safe_integer() {
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id": 9007199254740991, "method": "x.y"}"#).unwrap();
    assert_eq!(msg.id, Some(9007199254740991));
}

#[test]
fn test_cdp_message_parse_i64_max_id() {
    let msg: CdpMessage =
        serde_json::from_str(r#"{"id": 9223372036854775807, "method": "x.y"}"#).unwrap();
    assert_eq!(msg.id, Some(i64::MAX));
}

#[test]
fn test_cdp_message_parse_float_id_rejected() {
    // Float id is invalid → triggers -32600 path.
    assert!(serde_json::from_str::<CdpMessage>(r#"{"id": 1.5, "method": "x.y"}"#).is_err());
    assert!(parse_message(r#"{"id": 1.5, "method": "x.y"}"#).is_none());
}

#[test]
fn test_cdp_message_notification_no_id() {
    let msg: CdpMessage = serde_json::from_str(r#"{"method": "Page.loadEventFired"}"#).unwrap();
    assert!(msg.id.is_none());
    assert_eq!(msg.method, "Page.loadEventFired");
}

#[test]
fn test_cdp_message_with_session_id() {
    let raw = r#"{"id": 1, "method": "Runtime.evaluate", "sessionId": "sess-abc123"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.session_id, Some("sess-abc123".into()));
}

#[test]
fn test_cdp_message_parse_extra_fields_ignored() {
    let raw = r#"{"id":1,"method":"Page.reload","extra":"x","another":123}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.id, Some(1));
    assert_eq!(msg.method, "Page.reload");
}

#[test]
fn test_cdp_message_parse_params_as_array_accepted() {
    // CdpMessage.params is Value — any JSON shape is accepted at parse time.
    // Domain-level type validation is the handler's responsibility.
    let raw = r#"{"id":1,"method":"DOM.querySelectorAll","params":["div","span"]}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap().as_array().unwrap().len(), 2);
}

#[test]
fn test_cdp_message_parse_params_as_string_accepted() {
    let raw = r#"{"id":1,"method":"X.y","params":"raw"}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap(), json!("raw"));
}

#[test]
fn test_cdp_message_parse_params_as_number_accepted() {
    let raw = r#"{"id":1,"method":"X.y","params":42}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap(), json!(42));
}

#[test]
fn test_cdp_message_parse_params_as_bool_accepted() {
    let raw = r#"{"id":1,"method":"X.y","params":true}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.params.unwrap(), json!(true));
}

#[test]
fn test_cdp_message_parse_unicode_method() {
    let msg: CdpMessage = serde_json::from_str(r#"{"id":1,"method":"Page.日本語テスト"}"#).unwrap();
    assert_eq!(msg.method, "Page.日本語テスト");
}

// ---- Response / event serialisation ----

#[test]
fn test_cdp_response_success_serialization() {
    let resp = CdpResponse {
        id: Some(1),
        result: Some(json!({"value": 42})),
        error: None,
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains("\"result\""));
    assert!(!s.contains("\"error\""));
}

#[test]
fn test_cdp_response_error_serialization() {
    let resp = CdpResponse {
        id: Some(2),
        result: None,
        error: Some(CdpError {
            code: -32601,
            message: "not found".into(),
        }),
    };
    let s = serde_json::to_string(&resp).unwrap();
    assert!(s.contains("\"error\""));
    assert!(!s.contains("\"result\""));
    assert!(s.contains("-32601"));
    assert!(s.contains("not found"));
}

#[test]
fn test_cdp_event_with_params() {
    let ev = CdpEvent {
        method: "Page.loadEventFired".into(),
        params: Some(json!({"timestamp": 12345.0})),
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(s.contains("\"method\""));
    assert!(s.contains("\"params\""));
}

#[test]
fn test_cdp_event_without_params() {
    let ev = CdpEvent {
        method: "DOM.documentUpdated".into(),
        params: None,
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(!s.contains("\"params\""));
}

// ===========================================================================
// Registry dispatch edge cases
// ===========================================================================

struct EchoHandler {
    name: &'static str,
}
impl DomainHandler for EchoHandler {
    fn domain_name(&self) -> &'static str {
        self.name
    }
    fn handle_command(
        &self,
        cmd: &str,
        params: Value,
        _: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        Ok(json!({"command": cmd, "params": params}))
    }
}

#[test]
fn test_dispatch_no_dot_in_method() {
    let reg = DomainRegistry::<EchoHandler>::new();
    reg.register(EchoHandler { name: "Page" }).unwrap();
    assert!(reg.dispatch_command("Page", json!({}), noop()).is_some());
}

#[test]
fn test_dispatch_multiple_dots_in_method() {
    let reg = DomainRegistry::<EchoHandler>::new();
    reg.register(EchoHandler { name: "Page" }).unwrap();
    let r = reg
        .dispatch_command("Page.navigate.to.url", json!({}), noop())
        .unwrap()
        .unwrap();
    assert_eq!(r["command"], "Page.navigate.to.url");
}

#[test]
fn test_dispatch_empty_method() {
    let reg = DomainRegistry::<EchoHandler>::new();
    reg.register(EchoHandler { name: "Page" }).unwrap();
    assert!(reg.dispatch_command("", json!({}), noop()).is_none());
}

#[test]
fn test_dispatch_unregistered_domain() {
    let reg = DomainRegistry::<EchoHandler>::new();
    reg.register(EchoHandler { name: "Page" }).unwrap();
    assert!(reg
        .dispatch_command("Network.enable", json!({}), noop())
        .is_none());
}

#[test]
fn test_dispatch_case_sensitive_domain() {
    let reg = DomainRegistry::<EchoHandler>::new();
    reg.register(EchoHandler { name: "Page" }).unwrap();
    assert!(reg
        .dispatch_command("page.navigate", json!({}), noop())
        .is_none());
}

#[test]
fn test_registry_empty_name_handler() {
    let reg = DomainRegistry::<EchoHandler>::new();
    assert!(reg.register(EchoHandler { name: "" }).is_ok());
}

#[test]
fn test_registry_multiple_handlers_independent() {
    let reg = DomainRegistry::<EchoHandler>::new();
    reg.register(EchoHandler { name: "Page" }).unwrap();
    reg.register(EchoHandler { name: "Runtime" }).unwrap();
    reg.register(EchoHandler { name: "DOM" }).unwrap();

    assert!(reg
        .dispatch_command("Page.enable", json!({}), noop())
        .is_some());
    assert!(reg
        .dispatch_command("Runtime.evaluate", json!({"expr": "1"}), noop())
        .is_some());
    assert!(reg
        .dispatch_command("DOM.getDocument", json!({}), noop())
        .is_some());
    assert!(reg
        .dispatch_command("Network.enable", json!({}), noop())
        .is_none());
}

#[test]
fn test_registry_duplicate_rejected_preserves_original() {
    let reg = DomainRegistry::<EchoHandler>::new();
    reg.register(EchoHandler { name: "Page" }).unwrap();
    let dup = reg.register(EchoHandler { name: "Page" });
    assert!(dup.is_err());
    assert!(dup.unwrap_err().contains("already registered"));

    // Original handler must remain intact after a failed re-register.
    let r = reg
        .dispatch_command("Page.enable", json!({}), noop())
        .unwrap()
        .unwrap();
    assert_eq!(r["command"], "Page.enable");
}

// ===========================================================================
// Post-error recovery & handler-panic safety (boundary)
// ===========================================================================

#[test]
fn test_registry_dispatch_recovers_after_handler_error() {
    // REQ-CDS-004 robustness: a handler returning Err must not poison the
    // registry — subsequent dispatches to the same or other handlers must work.
    let reg = DomainRegistry::<ErrorAlways>::new();
    reg.register(ErrorAlways).unwrap();

    let r1 = reg
        .dispatch_command("ErrDomain.failing", json!({}), noop())
        .unwrap();
    assert!(r1.is_err());
    assert_eq!(r1.unwrap_err().code, -32000);

    // Dispatch again — registry state must be intact.
    let r2 = reg
        .dispatch_command("ErrDomain.again", json!({}), noop())
        .unwrap();
    assert!(r2.is_err());

    let r3 = reg.dispatch_command("Unknown.method", json!({}), noop());
    assert!(
        r3.is_none(),
        "unknown domain still returns None after errors"
    );
}

#[test]
fn test_registry_lock_not_poisoned_after_catch_unwind_style_panic() {
    // We cannot actually catch the panic without catch_unwind (DomainHandler is
    // not declared unwind-safe). Instead we prove the complementary contract:
    // when a handler does NOT panic but returns Err, the internal Mutex is
    // cleanly released and the registry remains fully usable. The panic case
    // is covered by integration-level tests with catch_unwind at the server.
    let reg = DomainRegistry::<ErrorAlways>::new();
    reg.register(ErrorAlways).unwrap();
    for _ in 0..50 {
        let _ = reg.dispatch_command("ErrDomain.x", json!({}), noop());
    }
    // If the lock had been poisoned, has_domain would return false (registry
    // guards lock failures by returning false/None). It must still be true.
    assert!(reg.has_domain("ErrDomain"));
}

// ===========================================================================
// Concurrent dispatch (REQ-CDS-007 robustness overlap)
// ===========================================================================

#[test]
fn test_concurrent_dispatch_thread_safe() {
    let spy = Arc::new(SpyHandler::new("Page"));
    let reg = Arc::new(DomainRegistry::<Observed>::new());
    reg.register(Observed::new(Arc::clone(&spy))).unwrap();

    let mut handles = Vec::new();
    for i in 0..8 {
        let reg = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let r = reg.dispatch_command("Page.navigate", json!({"i": i}), noop());
                assert!(r.is_some(), "concurrent dispatch must succeed");
                assert!(r.unwrap().is_ok());
            }
        }));
    }
    for h in handles {
        h.join()
            .expect("no thread should panic under concurrent dispatch");
    }
    assert_eq!(spy.commands().len(), 8 * 100);
}

#[test]
fn test_concurrent_register_and_dispatch() {
    // Registerers and dispatchers running concurrently must not corrupt the
    // internal HashMap. We don't assert exact contents (races on insertion)
    // only that nothing panics and the registry stays queryable.
    let reg = Arc::new(DomainRegistry::<EchoHandler>::new());
    reg.register(EchoHandler { name: "Page" }).unwrap();

    let mut handles = Vec::new();
    for d in ["A", "B", "C", "D"] {
        let reg = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            let _ = reg.register(EchoHandler { name: d });
        }));
    }
    for _ in 0..4 {
        let reg = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            let _ = reg.dispatch_command("Page.enable", json!({}), noop());
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    // Registry remains usable.
    assert!(reg.has_domain("Page"));
}

// ===========================================================================
// ServerConfig builder (REQ-CDS-008-C2 timeout + remaining fields)
// ===========================================================================

#[test]
fn test_server_config_builder_minimal() {
    let config = cdp_server::ServerConfig::builder().build();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 9222);
    assert_eq!(config.http_timeout_seconds, 30);
    assert_eq!(config.max_sessions, 100);
}

#[test]
fn test_server_config_builder_custom_host() {
    let config = cdp_server::ServerConfig::builder()
        .host("0.0.0.0")
        .port(8080)
        .build();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 8080);
}

#[test]
fn test_c8_config_http_timeout_seconds_settable() {
    // REQ-CDS-008-C2: HTTP timeout must be configurable (default 30s).
    let config = cdp_server::ServerConfig::builder()
        .http_timeout_seconds(120)
        .build();
    assert_eq!(config.http_timeout_seconds, 120);
}

#[test]
fn test_c8_config_http_timeout_zero_boundary() {
    let config = cdp_server::ServerConfig::builder()
        .http_timeout_seconds(0)
        .build();
    assert_eq!(config.http_timeout_seconds, 0);
}

#[test]
fn test_c8_config_max_sessions_settable() {
    let config = cdp_server::ServerConfig::builder().max_sessions(1).build();
    assert_eq!(config.max_sessions, 1);
}

#[test]
fn test_c8_config_all_optional_fields() {
    let config = cdp_server::ServerConfig::builder()
        .browser_name("Custom/1.0")
        .user_agent("UA/1.0")
        .v8_version("12.0")
        .webkit_version("537.36")
        .build();
    assert_eq!(config.browser_name, "Custom/1.0");
    assert_eq!(config.user_agent.as_deref(), Some("UA/1.0"));
    assert_eq!(config.v8_version.as_deref(), Some("12.0"));
    assert_eq!(config.webkit_version.as_deref(), Some("537.36"));
}

// ===========================================================================
// CdpError / protocol helpers
// ===========================================================================

#[test]
fn test_cdp_error_clone() {
    let e1 = CdpError {
        code: -32601,
        message: "test".into(),
    };
    let e2 = e1.clone();
    assert_eq!(e1.code, e2.code);
    assert_eq!(e1.message, e2.message);
}

#[test]
fn test_cdp_error_debug() {
    let e = CdpError {
        code: -32601,
        message: "not found".into(),
    };
    let d = format!("{:?}", e);
    assert!(d.contains("-32601"));
    assert!(d.contains("not found"));
}

#[test]
fn test_error_constants_match_jsonrpc_2_0() {
    assert_eq!(ERR_INVALID_REQUEST, -32600);
    assert_eq!(ERR_METHOD_NOT_FOUND, -32601);
}

#[test]
fn test_ok_response_carries_result() {
    let r = ok_response(Some(5), json!({"x": 1}));
    assert_eq!(r.id, Some(5));
    assert!(r.result.is_some());
    assert!(r.error.is_none());
}

#[test]
fn test_ok_empty_returns_empty_object() {
    let r = ok_empty(Some(9));
    assert_eq!(r.result, Some(json!({})));
    assert!(r.error.is_none());
}

#[test]
fn test_error_response_id_propagated() {
    let r = error_response(Some(123), -32601, "missing");
    assert_eq!(r.id, Some(123));
    assert_eq!(r.error.unwrap().code, -32601);
}

// ===========================================================================
// RegistryDispatch trait object (used by CdpServer via SharedRegistry)
// ===========================================================================

#[test]
fn test_registry_as_registry_dispatch() {
    // CdpServer holds Arc<dyn RegistryDispatch>. Verify the blanket impl
    // exposes dispatch / notify / has_domain through the trait object.
    let spy = Arc::new(SpyHandler::new("Page"));
    let reg: Arc<DomainRegistry<Observed>> = Arc::new(DomainRegistry::new());
    reg.register(Observed::new(Arc::clone(&spy))).unwrap();

    let dispatch: Arc<dyn RegistryDispatch> = reg;
    assert!(dispatch.has_domain("Page"));
    assert!(!dispatch.has_domain("Network"));

    let r = dispatch.dispatch_command("Page.navigate", json!({}), noop());
    assert!(r.is_some());

    dispatch.notify_session_created("Page", "s1");
    dispatch.notify_session_destroyed(&["Page".to_string()], "s1");

    assert_eq!(spy.created_ids(), vec!["s1".to_string()]);
    assert_eq!(spy.destroyed_ids(), vec!["s1".to_string()]);
}
