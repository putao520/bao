// REQ-CDP-003: Bridge trait for CDP ↔ servo communication  @trace REQ-CDP-003
// REQ-CDP-006: Channel-based async bridge (CDP thread ↔ main thread)
// REQ-BRW-004: Worker/ServiceWorker CDP observability (Target sub-targets + Network SW requests)  @trace REQ-BRW-004 [criterion:19]
//
// Architecture:
//   CDP WebSocket thread ──[BridgeCommand]──> main thread (servo)
//   CDP WebSocket thread <──[BridgeResponse]── main thread (servo)
//
// Why channels: servo's WebView is !Send (Rc<RefCell<>>),
// so all WebView operations must happen on the main thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use serde_json::Value;

/// CDP `frameId` of a target's main frame — the single truth source for
/// every face that reports a frame id (REQ-CDP-004).
///
/// CDP semantics: one frame carries exactly one stable id, so the
/// `Page.navigate` / `Page.reload` / history-traversal responses, the frame
/// event stream (`frameNavigated` / `frameStartedLoading` /
/// `frameStoppedLoading` / `lifecycleEvent` / `loadEventFired` /
/// `Network.requestWillBeSent`) and `Page.getFrameTree` must all report the
/// same value for the same target. The value is derived from the target
/// identity (v7 path B): `main-<targetId>` — per-target unique, so frames of
/// concurrently-attached pages can never collide inside a client-side
/// FrameManager (the phantom-frame class the v7 wave proved: a shared
/// cross-target frame id lets one page's `frameNavigated` be misread as the
/// other page's main frame). It is deliberately NOT the bare PageId: PageId
/// lives in the targetId namespace (Target domain) and the two namespaces
/// must never mix — hence the `main-` prefix.
pub fn main_frame_id_for_target(target_id: &str) -> String {
    format!("main-{target_id}")
}

/// The implicit default browser context id (REQ-CDS-009). Pages created
/// outside any explicit `Target.createBrowserContext` belong here; Chrome
/// likewise never lists the implicit default in `Target.getBrowserContexts`.
/// Single source for every wire face that reports attribution
/// (protocol `live_target_info`, the WS event stream) — the explicit-context
/// registry mints `bao-context-*` ids that can never collide with it.
pub const DEFAULT_BROWSER_CONTEXT_ID: &str = "bao-default-context";

// ─── REQ-CDP-006 C6: request interception command face ──────────────────
// The Fetch domain and the legacy `Network.setRequestInterception` /
// `Network.continueInterceptedRequest` face share one interception facility
// (design §5): the FetchEnable / FetchDisable / FetchResolve verbs below are
// its only bridge channel. The per-target interception slot and the
// pending-pause registry live browser-side (bao_browser), which consumes
// these types directly — bao_browser already depends on bao_cdp for
// BridgeCommand, so the normal dependency direction carries them.

/// One validated interception pattern (CDP `Fetch.pattern`); the field shape
/// mirrors the per-target slot in bao_browser's webview state (design §3.1).
/// @trace REQ-CDP-006 [criterion:6] [entity:FetchPattern]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchPattern {
    /// `urlPattern` glob (`*` wildcard, Chromium Fetch.urlPattern semantics);
    /// `None` matches every URL.
    pub url_pattern: Option<String>,
    /// CDP `resourceType` filter; `None` matches every type. bao's wire set
    /// is the closed 9-value set emitted by vendor/servo
    /// `net/http_loader.rs::bao_destination_to_resource_type` (Document /
    /// Script / Stylesheet / Image / Font / Media / Manifest / Fetch /
    /// Other) — a filter outside that set simply never matches.
    pub resource_type: Option<String>,
    /// Stage at which a matching request is paused.
    pub request_stage: FetchRequestStage,
}

/// @trace REQ-CDP-006 [criterion:6] [entity:FetchPattern]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchRequestStage {
    Request,
    Response,
}

/// Resolution verdict for one paused request (`Fetch.continueRequest` /
/// `failRequest` / `fulfillRequest` — design §3.2). The browser side maps
/// each verdict onto the held `WebResourceLoad`: Continue = drop the load
/// (servo's AutomaticResponder answers DoNotIntercept and the request
/// proceeds), Fail = intercept + cancel (net-side network error), Fulfill =
/// intercept + body + finish (synthesized response).
/// @trace REQ-CDP-006 [criterion:6] [entity:FetchResolve]
#[derive(Debug, Clone)]
pub enum FetchResolveAction {
    Continue,
    /// `error_reason` is advisory only — the servo pause face has no
    /// error-code channel, every fail surfaces as a plain network error
    /// (design §7).
    Fail {
        error_reason: Option<String>,
    },
    /// `response_headers` is the raw CDP `{name: value}` JSON object;
    /// `body` is the base64 text exactly as the client sent it — the browser
    /// layer owns base64 decoding (bun_base64) and the decoded-size cap.
    Fulfill {
        response_code: i64,
        status_message: Option<String>,
        response_headers: Option<Value>,
        body: Option<String>,
    },
}

/// Commands that the CDP server sends to the main thread for servo execution.
/// Each command carries a `target_id` identifying which page/target to route to.
#[derive(Debug, Clone)]
pub enum BridgeCommand {
    Navigate {
        target_id: String,
        url: String,
    },
    EvaluateJs {
        target_id: String,
        expression: String,
        return_by_value: bool,
    },
    TakeScreenshot {
        target_id: String,
        format: String,
        quality: Option<u8>,
    },
    GetTitle {
        target_id: String,
    },
    GetUrl {
        target_id: String,
    },
    GetDocument {
        target_id: String,
    },
    QuerySelector {
        target_id: String,
        selector: String,
    },
    QuerySelectorAll {
        target_id: String,
        selector: String,
    },
    GetOuterHtml {
        target_id: String,
        node_id: Option<i64>,
    },
    SetAttributeValue {
        target_id: String,
        node_id: i64,
        name: String,
        value: String,
    },
    DispatchMouseEvent {
        target_id: String,
        event_type: String,
        x: f64,
        y: f64,
        /// CDP `button`: "none" | "left" | "middle" | "right" | "back" | "forward".
        /// Transcribed verbatim — the servo enum mapping lives in bao_browser
        /// (the servo-aware layer), not here.
        button: Option<String>,
        click_count: Option<i64>,
    },
    DispatchKeyEvent {
        target_id: String,
        event_type: String,
        key: String,
        code: String,
        text: Option<String>,
        /// CDP `modifiers` bitmask: Alt=1, Ctrl=2, Meta/Command=4, Shift=8.
        modifiers: u32,
        /// CDP `location`: 0=default, 1=left, 2=right, 3=numpad.
        location: i64,
        /// CDP `autoRepeat`.
        repeat: bool,
    },
    InsertText {
        target_id: String,
        text: String,
    },
    SetViewport {
        target_id: String,
        width: u32,
        height: u32,
        device_scale_factor: Option<f64>,
    },
    SetUserAgent {
        target_id: String,
        user_agent: String,
    },
    GetCookies {
        target_id: String,
        urls: Vec<String>,
    },
    GetAllCookies {
        target_id: String,
    },
    DeleteCookie {
        target_id: String,
        name: String,
        url: Option<String>,
    },
    SetCookie {
        target_id: String,
        name: String,
        value: String,
        url: Option<String>,
        domain: Option<String>,
    },
    GetResponseBody {
        target_id: String,
        request_id: String,
    },
    AddScriptToEvaluateOnNewDocument {
        target_id: String,
        source: String,
    },
    /// Remove one new-document script by the identifier the vendor registry
    /// minted at add time (REQ-CDP-004; Chrome answers an unknown identifier
    /// with a "Script not found" server error).
    RemoveScriptToEvaluateOnNewDocument {
        target_id: String,
        identifier: String,
    },
    Reload {
        target_id: String,
        ignore_cache: bool,
    },
    GoBack {
        target_id: String,
    },
    GoForward {
        target_id: String,
    },
    StopLoading {
        target_id: String,
    },
    ClosePage {
        target_id: String,
    },
    // Multi-target management commands
    CreateTarget {
        url: String,
    },
    ListTargets,
    // Browser context management (REQ-CDS-009). The context-aware target
    // creation is a distinct variant rather than a new field on CreateTarget:
    // the parallel-universe client (bao_cdp_client BridgeSenderBackend)
    // constructs/matches the base variant outside this wave's ownership
    // boundary, and the context-free shape is the only one it can mint.
    CreateBrowserContext,
    DisposeBrowserContext {
        browser_context_id: String,
    },
    ListBrowserContexts,
    CreateTargetInContext {
        url: String,
        browser_context_id: String,
    },
    /// Per-target attribution query for the single-target `getTargetInfo`
    /// face (`getTargets` reads the context id straight off the
    /// `ListTargets` entries).
    GetTargetContextId {
        target_id: String,
    },
    // Debugger domain — mapped to servo DevtoolScriptControlMsg (BUG-CDP-006)
    DebuggerEnable {
        target_id: String,
    },
    DebuggerDisable {
        target_id: String,
    },
    DebuggerSetBreakpoint {
        target_id: String,
        url: Option<String>,
        url_regex: Option<String>,
        line: u32,
        column: Option<u32>,
    },
    DebuggerRemoveBreakpoint {
        target_id: String,
        breakpoint_id: String,
    },
    DebuggerInterrupt {
        target_id: String,
    },
    DebuggerResume {
        target_id: String,
        step_type: Option<String>,
    },
    DebuggerListFrames {
        target_id: String,
    },
    DebuggerGetEnvironment {
        target_id: String,
        frame_actor_id: String,
    },
    DebuggerEval {
        target_id: String,
        expression: String,
        frame_actor_id: Option<String>,
    },
    DebuggerGetPossibleBreakpoints {
        target_id: String,
        start_script_id: String,
    },
    DebuggerGetScriptSource {
        target_id: String,
        script_id: u32,
    },
    DebuggerBlackbox {
        target_id: String,
        script_id: u32,
    },
    DebuggerUnblackbox {
        target_id: String,
        script_id: u32,
    },
    // Network domain — cache control + browser cache/cookies clearing
    NetworkEnable {
        target_id: String,
    },
    NetworkDisable {
        target_id: String,
    },
    NetworkSetCacheDisabled {
        target_id: String,
        cache_disabled: bool,
    },
    NetworkSetExtraHTTPHeaders {
        target_id: String,
        headers: Value,
    },
    NetworkClearBrowserCache {
        target_id: String,
    },
    NetworkClearBrowserCookies {
        target_id: String,
    },
    // Storage domain — origin-scoped storage queries and clearing
    StorageGetStorageItemsForOrigin {
        target_id: String,
        origin: String,
        storage_type: String,
    },
    StorageClearDataForOrigin {
        target_id: String,
        origin: String,
        storage_type: String,
    },
    // Security domain — certificate error handling
    SecurityEnable {
        target_id: String,
    },
    SecurityDisable {
        target_id: String,
    },
    SecuritySetOverrideCertificateErrors {
        target_id: String,
        override_errors: bool,
    },
    // Profiler domain — JS CPU profiling (stub, awaits SM profiler API bridge)
    ProfilerStart {
        target_id: String,
    },
    ProfilerStop {
        target_id: String,
    },
    ProfilerSetSamplingInterval {
        target_id: String,
        interval: u32,
    },
    // HeapProfiler domain — heap snapshots (stub, awaits SM heap profiler bridge)
    HeapProfilerTakeSnapshot {
        target_id: String,
    },
    HeapProfilerStartTracking {
        target_id: String,
    },
    HeapProfilerStopTracking {
        target_id: String,
    },
    HeapProfilerCollectGarbage {
        target_id: String,
    },
    // Memory domain — memory metrics
    MemoryGetDOMCounters {
        target_id: String,
    },
    MemoryPurgeJS {
        target_id: String,
    },
    // Performance domain — performance metrics
    PerformanceGetMetrics {
        target_id: String,
    },
    // CSS domain — computed/matched/inline style retrieval via JS evaluate.
    // REQ-BRW-048: superseded on the CDP dispatch face — the CSS.* query
    // methods now resolve nodeIds inside the page (canonical positional
    // identity, devtools_dom) through EvaluateJs, because these
    // id-only commands could not address real-document nodes. The variants
    // stay (their browser-side handlers remain functional for direct
    // callers; removing them would be a cross-crate break).
    CssGetComputedStyleForNode {
        target_id: String,
        node_id: i64,
    },
    CssGetMatchedStylesForNode {
        target_id: String,
        node_id: i64,
    },
    CssGetInlineStylesForNode {
        target_id: String,
        node_id: i64,
    },
    // Runtime domain — object property inspection and function invocation via JS evaluate
    RuntimeGetProperties {
        target_id: String,
        object_id: String,
        own_properties: Option<bool>,
    },
    RuntimeCallFunctionOn {
        target_id: String,
        object_id: Option<String>,
        execution_context_id: Option<i64>,
        function_declaration: String,
        arguments: Option<Value>,
        return_by_value: Option<bool>,
        await_promise: Option<bool>,
        object_group: Option<String>,
    },
    // Runtime.releaseObject / releaseObjectGroup — drop object-registry
    // entries so repeated evaluateHandle roundtrips cannot leak (each
    // RemoteObject objectId pins a strong ref in the page registry).
    RuntimeReleaseObject {
        target_id: String,
        object_id: String,
    },
    RuntimeReleaseObjectGroup {
        target_id: String,
        object_group: String,
    },
    // Worker target observability — CDP Target domain exposes Worker as sub-targets  @trace REQ-BRW-004 [criterion:19]
    ListWorkerTargets {
        target_id: String,
    },
    GetWorkerTargetInfo {
        target_id: String,
        worker_id: String,
    },
    // ServiceWorker CDP observability — Network domain can observe SW-initiated requests  @trace REQ-BRW-004 [criterion:19]
    ListServiceWorkerRegistrations {
        target_id: String,
    },
    GetServiceWorkerRegistrationInfo {
        target_id: String,
        registration_id: String,
    },
    TerminateServiceWorker {
        target_id: String,
        registration_id: String,
    },
    StopServiceWorker {
        target_id: String,
        registration_id: String,
    },
    // Overlay domain — REQ-CDP-007-C6: screenshot/screencast annotation
    // highlight. The highlight spec travels as raw serde JSON so this crate
    // stays decoupled from bao_browser's `HighlightSpec` (design §7 E1/E2
    // weak serde coupling); the browser layer deserializes it against its
    // own paint-face type.
    OverlayHighlightNode {
        target_id: String,
        /// Canonical positional node id (the shared devtools_dom identity;
        /// `backendNodeId` folds into it — the DOM.resolveNode documented
        /// identity). `None` when the request addressed an objectId.
        node_id: Option<i64>,
        /// Runtime object-registry handle. `None` when addressed by nodeId.
        object_id: Option<String>,
        /// Raw `Overlay.HighlightSpec` JSON as the client sent it (`None` =
        /// the default DevTools palette). Not validated here — geometry and
        /// color semantics live on the browser paint face (the E3 consumer).
        highlight_spec: Option<Value>,
    },
    /// Clear the single highlight slot (Overlay.hideHighlight).
    OverlayHideHighlight {
        target_id: String,
    },
    // REQ-CDP-006 C6: request interception (the Fetch domain and the legacy
    // Network.setRequestInterception face share this facility; design §5).
    // `patterns` semantics: an empty vec intercepts nothing (an explicit
    // empty pattern list matches no request); `Fetch.enable` without a
    // `patterns` param is normalized at the protocol layer to one catch-all
    // Request-stage pattern (Chromium omitted-patterns semantics).
    // `legacy_network_events`: a hit emits `Network.requestIntercepted`
    // instead of `Fetch.requestPaused` (POP6-C6 criterion wording).
    FetchEnable {
        target_id: String,
        patterns: Vec<FetchPattern>,
        legacy_network_events: bool,
    },
    /// Clear the interception slot and drain every pending pause — each held
    /// load drops, which servo's AutomaticResponder answers as
    /// DoNotIntercept (design §3.1; the disable-is-the-exit invariant).
    FetchDisable {
        target_id: String,
    },
    /// Resolve one paused request by the interception id minted at pause
    /// time and carried on Fetch.requestPaused / Network.requestIntercepted.
    FetchResolve {
        target_id: String,
        interception_id: String,
        action: FetchResolveAction,
    },
}

/// Error-category class carried by a [`BridgeResponse`] failure (e234/e236).
///
/// The protocol layer used to map every bridge Err onto a uniform -32603 with
/// only the message distinguishing faces. Chrome classifies domain-owned
/// errors (Fetch.resolve UnknownId/Rejected, Overlay unknown node/object id,
/// ...) as -32000 ServerError while transport faces (no bridge / channel
/// closed / response timeout) stay -32603. The class rides inside the Err
/// payload so `bridge_send` can pick the JSON-RPC code without re-parsing
/// error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BridgeErrorKind {
    /// Transport-class face — no bridge connected, channel closed, response
    /// timeout. Maps to JSON-RPC -32603 at the protocol layer. The default
    /// for every error not explicitly classified as domain-owned.
    #[default]
    NoBridge,
    /// Domain-owned outcome (unknown interceptionId, rejected fulfillment,
    /// unknown node/object id, ...) — maps to -32000 ServerError, the code
    /// Chrome's DevTools uses for the same class.
    DomainError,
}

/// A bridge-channel failure: the message plus its [`BridgeErrorKind`] class.
///
/// `From<&str>`/`From<String>` default to [`BridgeErrorKind::NoBridge`] so
/// existing transport-class constructions keep their classification; the
/// domain-classified faces construct via [`BridgeError::domain`].
#[derive(Debug, Clone)]
pub struct BridgeError {
    /// The failure message, verbatim (the protocol layer forwards it as the
    /// JSON-RPC error message).
    pub message: String,
    /// Which JSON-RPC error class this failure belongs to.
    pub kind: BridgeErrorKind,
}

impl BridgeError {
    /// Transport-class failure (no bridge / closed channel / timeout).
    pub fn no_bridge(message: impl Into<String>) -> Self {
        BridgeError {
            message: message.into(),
            kind: BridgeErrorKind::NoBridge,
        }
    }

    /// Domain-owned outcome (Chrome's -32000 ServerError class).
    pub fn domain(message: impl Into<String>) -> Self {
        BridgeError {
            message: message.into(),
            kind: BridgeErrorKind::DomainError,
        }
    }
}

impl From<&str> for BridgeError {
    fn from(message: &str) -> Self {
        BridgeError::no_bridge(message)
    }
}

impl From<String> for BridgeError {
    fn from(message: String) -> Self {
        BridgeError::no_bridge(message)
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::ops::Deref for BridgeError {
    type Target = String;

    fn deref(&self) -> &String {
        &self.message
    }
}

impl PartialEq<str> for BridgeError {
    fn eq(&self, other: &str) -> bool {
        self.message == other
    }
}

impl PartialEq<&str> for BridgeError {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

impl PartialEq<String> for BridgeError {
    fn eq(&self, other: &String) -> bool {
        &self.message == other
    }
}

/// Response from the main thread back to the CDP server.
#[derive(Debug)]
pub struct BridgeResponse {
    pub result: Result<Value, BridgeError>,
}

/// Internal request structure used for channel communication.
struct BridgeRequest {
    command: BridgeCommand,
    responder: Sender<BridgeResponse>,
}

/// Handle for the CDP server to send commands to the main thread.
pub struct BridgeSender {
    tx: Sender<BridgeRequest>,
    timeout: Duration,
}

/// Handle for the main thread to receive and process commands from CDP.
pub struct BridgeReceiver {
    rx: Receiver<BridgeRequest>,
}

/// Create a new bridge channel pair.
pub fn bridge_channel(timeout: Duration) -> (BridgeSender, BridgeReceiver) {
    let (tx, rx) = mpsc::channel();
    (BridgeSender { tx, timeout }, BridgeReceiver { rx })
}

impl BridgeSender {
    /// Send a command to the main thread and wait for the response.
    pub fn send(&self, command: BridgeCommand) -> BridgeResponse {
        let (resp_tx, resp_rx) = mpsc::channel();
        if self
            .tx
            .send(BridgeRequest {
                command,
                responder: resp_tx,
            })
            .is_err()
        {
            return BridgeResponse {
                result: Err(BridgeError::no_bridge("bridge channel closed")),
            };
        }
        match resp_rx.recv_timeout(self.timeout) {
            Ok(resp) => resp,
            Err(_) => BridgeResponse {
                result: Err(BridgeError::no_bridge("bridge response timeout")),
            },
        }
    }

    /// Send a command without waiting for response (fire-and-forget).
    pub fn send_fire_and_forget(&self, command: BridgeCommand) {
        let (resp_tx, _) = mpsc::channel();
        let _ = self.tx.send(BridgeRequest {
            command,
            responder: resp_tx,
        });
    }

    /// Check if the channel is still open.
    pub fn is_alive(&self) -> bool {
        !self
            .tx
            .send(BridgeRequest {
                command: BridgeCommand::ListTargets,
                responder: mpsc::channel().0,
            })
            .is_err()
    }
}

impl BridgeReceiver {
    /// Try to receive and process a pending command. Returns true if a command was processed.
    /// Block until a command arrives or timeout, then process it.
    pub fn recv_and_process<F>(&self, timeout: Duration, handler: F) -> bool
    where
        F: FnOnce(BridgeCommand) -> BridgeResponse,
    {
        match self.rx.recv_timeout(timeout) {
            Ok(request) => {
                let response = handler(request.command);
                let _ = request.responder.send(response);
                true
            }
            Err(_) => false,
        }
    }

    pub fn try_process<F>(&self, handler: F) -> bool
    where
        F: FnOnce(BridgeCommand) -> BridgeResponse,
    {
        match self.rx.try_recv() {
            Ok(request) => {
                let response = handler(request.command);
                let _ = request.responder.send(response);
                true
            }
            Err(_) => false,
        }
    }

    /// Process all pending commands.
    pub fn drain<F>(&self, handler: F) -> usize
    where
        F: Fn(BridgeCommand) -> BridgeResponse,
    {
        let mut count = 0;
        while let Ok(request) = self.rx.try_recv() {
            let response = handler(request.command);
            let _ = request.responder.send(response);
            count += 1;
        }
        count
    }
}

impl std::clone::Clone for BridgeSender {
    fn clone(&self) -> Self {
        BridgeSender {
            tx: self.tx.clone(),
            timeout: self.timeout,
        }
    }
}

// @trace TEST-CDP-003 [req:REQ-CDP-003] [level:unit] [nfr:TMG-CDP-01]
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_millis(100);

    fn ok_response(val: Value) -> BridgeResponse {
        BridgeResponse { result: Ok(val) }
    }

    fn err_response(msg: &str) -> BridgeResponse {
        BridgeResponse {
            result: Err(msg.into()),
        }
    }

    // 3b. BridgeError carrier: the explicit domain class survives the channel
    //     round-trip and the From-defaults stay transport-class (e234/e236).
    #[test]
    fn bridge_error_kind_roundtrips_through_channel() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        std::thread::spawn(move || {
            receiver.recv_and_process(TIMEOUT, |cmd| match cmd {
                BridgeCommand::FetchResolve { .. } => BridgeResponse {
                    result: Err(BridgeError::domain("unknown interceptionId: i-1")),
                },
                _ => BridgeResponse {
                    result: Err(BridgeError::no_bridge("bridge channel closed")),
                },
            })
        });
        let resp = sender.send(BridgeCommand::FetchResolve {
            target_id: TID.into(),
            interception_id: "i-1".into(),
            action: FetchResolveAction::Continue,
        });
        let err = resp.result.unwrap_err();
        assert_eq!(err.kind, BridgeErrorKind::DomainError);
        assert_eq!(err, "unknown interceptionId: i-1");
        assert!(err.contains("interceptionId"));
        // From-defaults keep the transport classification.
        assert_eq!(BridgeError::from("boom").kind, BridgeErrorKind::NoBridge);
        assert_eq!(
            BridgeError::from(String::from("boom")).kind,
            BridgeErrorKind::NoBridge
        );
        assert_eq!(BridgeError::no_bridge("x").to_string(), "x");
    }

    fn noop_handler(_: BridgeCommand) -> BridgeResponse {
        ok_response(Value::Null)
    }

    const TID: &str = "test-target";

    // 1. bridge_channel creates sender and receiver
    #[test]
    fn bridge_channel_creates_sender_and_receiver() {
        let (sender, _receiver) = bridge_channel(TIMEOUT);
        assert!(
            sender.is_alive(),
            "sender should report alive when receiver exists"
        );
    }

    // 2. BridgeSender::send with responding receiver → gets BridgeResponse with Ok
    #[test]
    fn send_with_responding_receiver_returns_ok() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        sender.send_fire_and_forget(BridgeCommand::GetTitle {
            target_id: TID.into(),
        });
        let mut captured_response: Option<BridgeResponse> = None;
        let processed = receiver.try_process(|cmd| {
            let resp = match cmd {
                BridgeCommand::GetTitle { .. } => ok_response(Value::String("Test Page".into())),
                _ => err_response("unexpected"),
            };
            captured_response = Some(BridgeResponse {
                result: resp.result.clone(),
            });
            resp
        });
        assert!(processed);
        let resp = captured_response.unwrap();
        assert!(resp.result.is_ok());
        assert_eq!(resp.result.unwrap(), Value::String("Test Page".into()));
    }

    // 3. BridgeSender::send when receiver dropped → Err 'bridge channel closed'
    #[test]
    fn send_when_receiver_dropped_returns_channel_closed() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        drop(receiver);
        let resp = sender.send(BridgeCommand::GetTitle {
            target_id: TID.into(),
        });
        assert!(resp.result.is_err());
        assert_eq!(resp.result.unwrap_err(), "bridge channel closed");
    }

    // 4. BridgeSender::send_fire_and_forget works without panic
    #[test]
    fn send_fire_and_forget_does_not_panic() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        sender.send_fire_and_forget(BridgeCommand::GetTitle {
            target_id: TID.into(),
        });
        let processed = receiver.try_process(noop_handler);
        assert!(processed, "fire-and-forget command should be receivable");
    }

    // 5. BridgeSender::is_alive when channel open → returns true
    #[test]
    fn is_alive_when_channel_open_returns_true() {
        let (sender, _receiver) = bridge_channel(TIMEOUT);
        assert!(sender.is_alive());
    }

    // 6. BridgeSender clone preserves connection
    #[test]
    fn clone_preserves_connection() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        let cloned = sender.clone();
        cloned.send_fire_and_forget(BridgeCommand::GetTitle {
            target_id: TID.into(),
        });
        let processed = receiver.try_process(noop_handler);
        assert!(
            processed,
            "cloned sender should deliver command to same receiver"
        );
    }

    // 7. BridgeReceiver::try_process processes one command
    #[test]
    fn try_process_processes_one_command() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        sender.send_fire_and_forget(BridgeCommand::GetTitle {
            target_id: TID.into(),
        });
        let processed = receiver.try_process(|_| ok_response(Value::Bool(true)));
        assert!(processed);
        let again = receiver.try_process(noop_handler);
        assert!(!again, "no second command should be pending");
    }

    // 8. BridgeReceiver::drain processes multiple commands
    #[test]
    fn drain_processes_multiple_commands() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        for _ in 0..5 {
            sender.send_fire_and_forget(BridgeCommand::GetTitle {
                target_id: TID.into(),
            });
        }
        let count = receiver.drain(noop_handler);
        assert_eq!(count, 5);
    }

    // 9. BridgeReceiver::try_process returns false when empty
    #[test]
    fn try_process_returns_false_when_empty() {
        let (_sender, receiver) = bridge_channel(TIMEOUT);
        let processed = receiver.try_process(noop_handler);
        assert!(!processed);
    }

    // 10. BridgeReceiver::recv_and_process blocks until command arrives
    #[test]
    fn recv_and_process_receives_command() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        let sender_thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            sender.send_fire_and_forget(BridgeCommand::GetTitle {
                target_id: TID.into(),
            });
        });
        let processed = receiver.recv_and_process(TIMEOUT, |_| ok_response(Value::Bool(true)));
        assert!(processed, "recv_and_process should receive the command");
        sender_thread.join().unwrap();
    }

    // 11. BridgeReceiver::recv_and_process returns false on timeout
    #[test]
    fn recv_and_process_returns_false_on_timeout() {
        let (_sender, receiver) = bridge_channel(Duration::from_millis(50));
        let processed = receiver.recv_and_process(Duration::from_millis(50), noop_handler);
        assert!(
            !processed,
            "recv_and_process should return false on timeout"
        );
    }

    // 12. BridgeResponse result Ok with Value
    #[test]
    fn bridge_response_ok_with_value() {
        let resp = ok_response(Value::Number(42.into()));
        assert!(resp.result.is_ok());
        assert_eq!(resp.result.unwrap(), Value::Number(42.into()));
    }

    // 11. BridgeResponse result Err with string
    #[test]
    fn bridge_response_err_with_string() {
        let resp = err_response("something failed");
        assert!(resp.result.is_err());
        assert_eq!(resp.result.unwrap_err(), "something failed");
    }

    // 12. BridgeCommand::Navigate debug format contains 'Navigate'
    #[test]
    fn navigate_debug_format_contains_navigate() {
        let cmd = BridgeCommand::Navigate {
            target_id: TID.into(),
            url: "https://example.com".into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(
            debug_str.contains("Navigate"),
            "debug output should contain 'Navigate': {}",
            debug_str
        );
    }

    // 13. BridgeCommand::EvaluateJs debug format
    #[test]
    fn evaluate_js_debug_format() {
        let cmd = BridgeCommand::EvaluateJs {
            target_id: TID.into(),
            expression: "1+1".into(),
            return_by_value: true,
        };
        let debug_str = format!("{:?}", cmd);
        assert!(
            debug_str.contains("EvaluateJs"),
            "debug output should contain 'EvaluateJs': {}",
            debug_str
        );
    }

    // 14. BridgeCommand::TakeScreenshot debug format
    #[test]
    fn take_screenshot_debug_format() {
        let cmd = BridgeCommand::TakeScreenshot {
            target_id: TID.into(),
            format: "png".into(),
            quality: Some(80),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(
            debug_str.contains("TakeScreenshot"),
            "debug output should contain 'TakeScreenshot': {}",
            debug_str
        );
    }

    // 15. BridgeCommand variants construction
    #[test]
    fn dispatch_mouse_event_construction() {
        let cmd = BridgeCommand::DispatchMouseEvent {
            target_id: TID.into(),
            event_type: "mouseMoved".into(),
            x: 100.0,
            y: 200.0,
            button: Some("left".into()),
            click_count: Some(2),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("DispatchMouseEvent"));
        assert!(debug_str.contains("mouseMoved"));
    }

    #[test]
    fn dispatch_key_event_construction() {
        let cmd = BridgeCommand::DispatchKeyEvent {
            target_id: TID.into(),
            event_type: "keyDown".into(),
            key: "Enter".into(),
            code: "Enter".into(),
            text: Some("\r".into()),
            modifiers: 0,
            location: 0,
            repeat: false,
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("DispatchKeyEvent"));
    }

    #[test]
    fn set_viewport_construction() {
        let cmd = BridgeCommand::SetViewport {
            target_id: TID.into(),
            width: 1920,
            height: 1080,
            device_scale_factor: Some(2.0),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("SetViewport"));
    }

    #[test]
    fn set_cookie_construction() {
        let cmd = BridgeCommand::SetCookie {
            target_id: TID.into(),
            name: "session".into(),
            value: "abc123".into(),
            url: Some("https://example.com".into()),
            domain: Some(".example.com".into()),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("SetCookie"));
    }

    #[test]
    fn get_response_body_construction() {
        let cmd = BridgeCommand::GetResponseBody {
            target_id: TID.into(),
            request_id: "req-001".into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GetResponseBody"));
    }

    #[test]
    fn add_script_to_evaluate_on_new_document_construction() {
        let cmd = BridgeCommand::AddScriptToEvaluateOnNewDocument {
            target_id: TID.into(),
            source: "console.log('hi')".into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("AddScriptToEvaluateOnNewDocument"));
    }

    #[test]
    fn reload_construction() {
        let cmd = BridgeCommand::Reload {
            target_id: TID.into(),
            ignore_cache: true,
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Reload"));
    }

    #[test]
    fn go_back_construction() {
        let cmd = BridgeCommand::GoBack {
            target_id: TID.into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GoBack"));
    }

    #[test]
    fn go_forward_construction() {
        let cmd = BridgeCommand::GoForward {
            target_id: TID.into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GoForward"));
    }

    #[test]
    fn stop_loading_construction() {
        let cmd = BridgeCommand::StopLoading {
            target_id: TID.into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("StopLoading"));
    }

    #[test]
    fn close_page_construction() {
        let cmd = BridgeCommand::ClosePage {
            target_id: TID.into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("ClosePage"));
    }

    // 16. BridgeSender timeout works (short timeout + slow responder)
    #[test]
    fn send_timeout_returns_err() {
        let (sender, receiver) = bridge_channel(Duration::from_millis(10));
        sender.send_fire_and_forget(BridgeCommand::GetTitle {
            target_id: TID.into(),
        });
        let resp = sender.send(BridgeCommand::GetUrl {
            target_id: TID.into(),
        });
        assert!(resp.result.is_err());
        assert_eq!(resp.result.unwrap_err(), "bridge response timeout");
        receiver.drain(noop_handler);
    }

    // 17. Multiple sequential send+process
    #[test]
    fn multiple_sequential_send_process() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        let commands: Vec<BridgeCommand> = vec![
            BridgeCommand::Navigate {
                target_id: TID.into(),
                url: "https://a.com".into(),
            },
            BridgeCommand::EvaluateJs {
                target_id: TID.into(),
                expression: "1+1".into(),
                return_by_value: true,
            },
            BridgeCommand::GetTitle {
                target_id: TID.into(),
            },
        ];

        for cmd in commands {
            sender.send_fire_and_forget(cmd);
        }

        let mut results: Vec<String> = Vec::new();
        loop {
            let processed = receiver.try_process(|c| {
                let label = match c {
                    BridgeCommand::Navigate { url, .. } => format!("nav:{}", url),
                    BridgeCommand::EvaluateJs { expression, .. } => format!("eval:{}", expression),
                    BridgeCommand::GetTitle { .. } => "title".into(),
                    _ => "other".into(),
                };
                results.push(label);
                ok_response(Value::Null)
            });
            if !processed {
                break;
            }
        }

        assert_eq!(results.len(), 3);
        assert!(results[0].starts_with("nav:"));
        assert!(results[1].starts_with("eval:"));
        assert_eq!(results[2], "title");
    }

    // ─── Worker/ServiceWorker CDP observability commands ────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [level:unit] [criterion:19]

    #[test]
    fn list_worker_targets_construction() {
        let cmd = BridgeCommand::ListWorkerTargets {
            target_id: TID.into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("ListWorkerTargets"));
    }

    #[test]
    fn get_worker_target_info_construction() {
        let cmd = BridgeCommand::GetWorkerTargetInfo {
            target_id: TID.into(),
            worker_id: "worker-1".into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GetWorkerTargetInfo"));
        assert!(debug_str.contains("worker-1"));
    }

    #[test]
    fn list_service_worker_registrations_construction() {
        let cmd = BridgeCommand::ListServiceWorkerRegistrations {
            target_id: TID.into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("ListServiceWorkerRegistrations"));
    }

    #[test]
    fn get_service_worker_registration_info_construction() {
        let cmd = BridgeCommand::GetServiceWorkerRegistrationInfo {
            target_id: TID.into(),
            registration_id: "sw-reg-1".into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GetServiceWorkerRegistrationInfo"));
        assert!(debug_str.contains("sw-reg-1"));
    }

    #[test]
    fn terminate_service_worker_construction() {
        let cmd = BridgeCommand::TerminateServiceWorker {
            target_id: TID.into(),
            registration_id: "sw-reg-1".into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("TerminateServiceWorker"));
    }

    #[test]
    fn stop_service_worker_construction() {
        let cmd = BridgeCommand::StopServiceWorker {
            target_id: TID.into(),
            registration_id: "sw-reg-1".into(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("StopServiceWorker"));
    }

    #[test]
    fn list_worker_targets_bridge_send_receive() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        sender.send_fire_and_forget(BridgeCommand::ListWorkerTargets {
            target_id: TID.into(),
        });
        let processed = receiver.try_process(|cmd| {
            match cmd {
                BridgeCommand::ListWorkerTargets { .. } => ok_response(serde_json::json!({
                    "workerTargets": [
                        { "targetId": "worker-1", "type": "worker", "url": "https://example.com/worker.js", "title": "" }
                    ]
                })),
                _ => err_response("unexpected"),
            }
        });
        assert!(processed, "ListWorkerTargets should be receivable");
    }

    #[test]
    fn service_worker_registrations_bridge_roundtrip() {
        let (sender, receiver) = bridge_channel(Duration::from_secs(2));
        let keeper = sender.clone();
        std::thread::spawn(move || {
            let _keeper = keeper;
            loop {
                let handled = receiver.try_process(|cmd| match cmd {
                    BridgeCommand::ListServiceWorkerRegistrations { .. } => BridgeResponse {
                        result: Ok(serde_json::json!({
                            "registrations": [{
                                "registrationId": "/sw.js:https://example.com/",
                                "scriptURL": "https://example.com/sw.js",
                                "scope": "https://example.com/",
                                "state": "activated",
                                "fetchInterceptActive": true
                            }]
                        })),
                    },
                    _ => BridgeResponse {
                        result: Ok(serde_json::json!({})),
                    },
                });
                if !handled {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });
        let resp = sender.send(BridgeCommand::ListServiceWorkerRegistrations {
            target_id: TID.into(),
        });
        assert!(resp.result.is_ok());
        let result = resp.result.unwrap();
        let regs = result["registrations"].as_array().unwrap();
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0]["state"], "activated");
        assert_eq!(regs[0]["fetchInterceptActive"], true);
    }
}
