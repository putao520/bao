// REQ-CDP-005: WS command-face registry — routes CdpServer WebSocket
// commands to the real bao_cdp command dispatch (servo-bridge backed).
// @trace REQ-CDP-001 [entity:CdpServer] [entity:DomainRegistry]
// @trace REQ-CDP-003 [entity:CdpSessionGeneric]
//
// This is the wiring point that兑现 Playwright 直连 (REQ-CDP): the WS
// session's commands are dispatched through `bao_cdp::protocol::handle_command`
// with the servo bridge, so Page.navigate / Runtime.evaluate / Target.* reach
// the real PagePool-backed handlers. It also owns the flattened-session
// routing table (CDP sessionId → target id) that Target.attachToTarget mints,
// and the auto-attach event stream Playwright's connect_over_cdp requires
// (Target.attachedToTarget + session-scoped Runtime/Page lifecycle events).

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use bao_cdp::servo_bridge::{BridgeCommand, BridgeSender};
use cdp_server::{CdpError, CdpMessage, EventSender, RegistryDispatch};
use serde_json::{json, Value};

/// JSON-RPC error code for an unknown/flattened CDP session id
/// (Chrome: "Session with given id not found").
const ERR_SESSION_NOT_FOUND: i64 = -32001;

/// The browser-endpoint pseudo target — target id of WS connections to
/// `/devtools/browser` (see cdp-server `handle_connection`).
const BROWSER_TARGET: &str = "__browser__";

/// Domains served by `bao_cdp::protocol::handle_command`.
const SERVED_DOMAINS: [&str; 21] = [
    "Target",
    "Page",
    "Runtime",
    "DOM",
    "Network",
    "CSS",
    "Emulation",
    "Input",
    "Overlay",
    "Debugger",
    "Log",
    "Fetch",
    "Storage",
    "Security",
    "Profiler",
    "HeapProfiler",
    "Memory",
    "Performance",
    "SystemInfo",
    "ServiceWorker",
    "Browser",
];

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);
static CONTEXT_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_session_id() -> String {
    let n = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("bao-session-{n:016x}")
}

/// WS command-face registry: bridges `CdpServer` sessions to the real
/// `bao_cdp` command dispatch.
///
/// - Commands on a page session (`/devtools/page/<id>`) route to that page.
/// - Commands on the browser session route pool-level (Target.*) or fail
///   per-target lookups with the browser pseudo-target.
/// - Commands carrying a `sessionId` (flattened mode, what Playwright uses)
///   route to the target `Target.attachToTarget` bound that session to.
pub struct BaoWsRegistry {
    bridge: BridgeSender,
    /// Flattened-session routing table: CDP sessionId → target id.
    attached_sessions: Mutex<HashMap<String, String>>,
    /// Whether the browser session asked for auto-attach (Target.setAutoAttach
    /// with autoAttach=true) — new targets emit Target.attachedToTarget.
    auto_attach: Mutex<bool>,
}

impl BaoWsRegistry {
    pub fn new(bridge: BridgeSender) -> Self {
        BaoWsRegistry {
            bridge,
            attached_sessions: Mutex::new(HashMap::new()),
            auto_attach: Mutex::new(false),
        }
    }

    /// Handle the session-table commands that only this registry can serve
    /// (it owns the sessionId→target table). Returns None when `method` is
    /// not a session-table command.
    fn dispatch_session_command(
        &self,
        method: &str,
        params: &Option<Value>,
        msg: &CdpMessage,
        event_sender: &dyn EventSender,
    ) -> Option<Result<Value, CdpError>> {
        match method {
            "Target.attachToTarget" => Some(self.attach_to_target(params)),
            "Target.detachFromTarget" => Some(self.detach_from_target(params)),
            // Page.createIsolatedWorld needs the event face (the new context
            // is announced via a session-scoped Runtime.executionContextCreated),
            // so it is served here rather than in the stateless dispatch.
            "Page.createIsolatedWorld" => {
                Some(self.create_isolated_world(params, msg, event_sender))
            }
            "Target.setAutoAttach" => {
                // Only the browser session's setAutoAttach enumerates existing
                // page targets — Playwright also sets auto-attach on each page
                // session (for worker sub-targets); re-emitting pages there
                // would duplicate targets client-side.
                Some(self.set_auto_attach(params, msg.session_id.is_none(), event_sender))
            }
            _ => None,
        }
    }

    fn attach_to_target(&self, params: &Option<Value>) -> Result<Value, CdpError> {
        let target_id = require_param(params, "targetId")?;
        // Chrome's non-flattened mode (session nesting via
        // Target.sendMessageToTarget) is not implemented — flattened mode is
        // the only routing model Playwright/Puppeteer use.
        let flatten = params
            .as_ref()
            .and_then(|p| p.get("flatten"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !flatten {
            return Err(CdpError {
                code: -32000,
                message: "'Target.attachToTarget' not supported: only flatten=true (Playwright/Puppeteer mode) is implemented".into(),
            });
        }
        Ok(json!({ "sessionId": self.mint_session(&target_id) }))
    }

    fn detach_from_target(&self, params: &Option<Value>) -> Result<Value, CdpError> {
        let session_id = require_param(params, "sessionId")?;
        if let Ok(mut table) = self.attached_sessions.lock() {
            table.remove(session_id.as_str());
        }
        Ok(json!({}))
    }

    /// Target.setAutoAttach — Playwright's connect_over_cdp discovery path.
    /// With autoAttach=true every existing page target gets a minted session
    /// and a Target.attachedToTarget event (browser-level, routed client-side
    /// by the embedded sessionId).
    fn set_auto_attach(
        &self,
        params: &Option<Value>,
        from_browser_session: bool,
        event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        let auto_attach = params
            .as_ref()
            .and_then(|p| p.get("autoAttach"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if let Ok(mut flag) = self.auto_attach.lock() {
            *flag = auto_attach;
        }
        if !auto_attach || !from_browser_session {
            return Ok(json!({}));
        }
        // Emit attachedToTarget for every existing page (real enumeration via
        // the bridge — the same ListTargets face Target.getTargets uses).
        let listed = self
            .bridge
            .send(BridgeCommand::ListTargets)
            .result
            .ok()
            .and_then(|v| v.as_array().cloned());
        if let Some(entries) = listed {
            for entry in entries {
                let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let session_id = self.mint_session(id);
                event_sender.send_event(
                    "Target.attachedToTarget",
                    json!({
                        "sessionId": session_id,
                        "targetInfo": {
                            "targetId": id,
                            "type": "page",
                            "title": entry.get("title").cloned().unwrap_or(json!("")),
                            "url": entry.get("url").cloned().unwrap_or(json!("about:blank")),
                            "attached": true,
                            "browserContextId": "bao-default-context",
                        },
                    }),
                );
            }
        }
        Ok(json!({}))
    }

    /// Page.createIsolatedWorld — mints a Runtime context for the named
    /// world and announces it with a session-scoped
    /// Runtime.executionContextCreated event.
    ///
    /// DEVIATION (documented): the servo embedder exposes no isolated-world
    /// (separate compartment) API — evaluates against the returned contextId
    /// run in the page realm. The handle is real (evaluation works and is
    /// observable); only world isolation is absent.
    fn create_isolated_world(
        &self,
        params: &Option<Value>,
        msg: &CdpMessage,
        event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        let world_name = params
            .as_ref()
            .and_then(|p| p.get("worldName"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let context_id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
        if let Some(sid) = msg.session_id.as_deref() {
            event_sender.send_session_event(
                sid,
                "Runtime.executionContextCreated",
                json!({
                    "context": {
                        "id": context_id,
                        "origin": "-",
                        "name": world_name,
                        "auxData": { "isDefault": false },
                    }
                }),
            );
        }
        Ok(json!({ "executionContextId": context_id }))
    }

    /// Mint a fresh flattened session bound to `target_id`.
    fn mint_session(&self, target_id: &str) -> String {
        let session_id = next_session_id();
        if let Ok(mut table) = self.attached_sessions.lock() {
            table.insert(session_id.clone(), target_id.to_string());
        }
        session_id
    }

    /// Session-scoped lifecycle events the Playwright page-session init
    /// sequence requires. `session_id` is None for page-endpoint connections
    /// (plain broadcast, no routing tag).
    fn emit(
        &self,
        event_sender: &dyn EventSender,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) {
        match session_id {
            Some(sid) => event_sender.send_session_event(sid, method, params),
            None => event_sender.send_event(method, params),
        }
    }
}

fn require_param(params: &Option<Value>, key: &str) -> Result<String, CdpError> {
    params
        .as_ref()
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| CdpError {
            code: -32602,
            message: format!("requires a non-empty {key} param"),
        })
}

impl RegistryDispatch for BaoWsRegistry {
    fn dispatch_command(
        &self,
        method: &str,
        params: Value,
        event_sender: &dyn EventSender,
    ) -> Option<Result<Value, CdpError>> {
        // Legacy signature carries no routing context — treat as a browser
        // endpoint message (pool-level Target.* still resolves correctly).
        let msg = CdpMessage {
            id: None,
            method: method.to_string(),
            params: Some(params),
            session_id: None,
        };
        self.dispatch_message(&msg, BROWSER_TARGET, event_sender)
    }

    fn dispatch_message(
        &self,
        msg: &CdpMessage,
        ws_target_id: &str,
        event_sender: &dyn EventSender,
    ) -> Option<Result<Value, CdpError>> {
        // Session-table commands first (they mint/remove routing entries).
        if let Some(result) =
            self.dispatch_session_command(&msg.method, &msg.params, msg, event_sender)
        {
            return Some(result);
        }

        // Resolve the routing target: flattened sessionId wins, else the WS
        // session's own target (page id for /devtools/page/<id>, the browser
        // pseudo-target for /devtools/browser).
        let target_id = match &msg.session_id {
            Some(sid) => match self
                .attached_sessions
                .lock()
                .ok()
                .and_then(|t| t.get(sid).cloned())
            {
                Some(t) => t,
                None => {
                    return Some(Err(CdpError {
                        code: ERR_SESSION_NOT_FOUND,
                        message: format!("Session with given id not found: {sid}"),
                    }))
                }
            },
            None => ws_target_id.to_string(),
        };

        // Real command face: bao_cdp's servo-bridge-backed domain dispatch.
        let response = bao_cdp::handle_command(
            msg.clone(),
            &target_id,
            &msg.params,
            Some(&self.bridge),
        );
        let result = match (response.result, response.error) {
            (Some(result), _) => Ok(result),
            (None, Some(err)) => Err(err),
            (None, None) => Ok(json!({})),
        };

        // Post-command lifecycle events (the "events 按需" face Playwright's
        // init/navigation sequences are driven by).
        if result.is_ok() {
            let sid = msg.session_id.as_deref();
            match msg.method.as_str() {
                // Playwright's page-session init: Runtime.enable must be
                // followed by executionContextCreated or evaluate() has no
                // context to bind to.
                "Runtime.enable" => {
                    // Chrome shape: auxData carries the owning frameId —
                    // clients (Playwright) bind the default context to the
                    // frame through it. Our frame id IS the page target id.
                    let context_id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
                    self.emit(
                        event_sender,
                        sid,
                        "Runtime.executionContextCreated",
                        json!({
                            "context": {
                                "id": context_id,
                                "origin": "-",
                                "name": "",
                                "auxData": {
                                    "isDefault": true,
                                    "type": "default",
                                    "frameId": target_id,
                                },
                            }
                        }),
                    );
                }
                // Frame lifecycle for page.goto: Playwright resolves the
                // navigation promise from frameStartedLoading/frameNavigated.
                "Page.navigate" => {
                    if let Ok(ref r) = result {
                        let fid = r
                            .get("frameId")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&target_id)
                            .to_string();
                        let loader = r
                            .get("loaderId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let url = msg
                            .params
                            .as_ref()
                            .and_then(|p| p.get("url"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("about:blank")
                            .to_string();
                        self.emit(
                            event_sender,
                            sid,
                            "Page.frameStartedLoading",
                            json!({ "frameId": fid }),
                        );
                        // Cross-document navigation replaces the document's
                        // execution contexts (Chrome semantics): clear the old
                        // ones and announce a fresh default context bound to
                        // the frame, or clients wait for a context that never
                        // comes after navigation.
                        self.emit(
                            event_sender,
                            sid,
                            "Runtime.executionContextsCleared",
                            json!({}),
                        );
                        self.emit(
                            event_sender,
                            sid,
                            "Page.frameNavigated",
                            json!({
                                "frame": {
                                    "id": fid,
                                    "loaderId": loader,
                                    "url": url,
                                    "mimeType": "text/html",
                                    "securityOrigin": "",
                                },
                            }),
                        );
                        let context_id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
                        self.emit(
                            event_sender,
                            sid,
                            "Runtime.executionContextCreated",
                            json!({
                                "context": {
                                    "id": context_id,
                                    "origin": "-",
                                    "name": "",
                                    "auxData": {
                                        "isDefault": true,
                                        "type": "default",
                                        "frameId": fid,
                                    },
                                }
                            }),
                        );
                    }
                }
                // Auto-attach for programmatically created targets:
                // Target.createTarget → Target.attachedToTarget event so
                // Playwright's context.new_page() completes.
                "Target.createTarget" => {
                    let auto = self.auto_attach.lock().map(|f| *f).unwrap_or(false);
                    if auto {
                        if let Ok(ref r) = result {
                            if let Some(new_id) = r.get("targetId").and_then(|v| v.as_str()) {
                                let session_id = self.mint_session(new_id);
                                event_sender.send_event(
                                    "Target.attachedToTarget",
                                    json!({
                                        "sessionId": session_id,
                                        "targetInfo": {
                                            "targetId": new_id,
                                            "type": "page",
                                            "title": "",
                                            "url": "about:blank",
                                            "attached": true,
                                            "browserContextId": "bao-default-context",
                                        },
                                    }),
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        Some(result)
    }

    fn notify_session_created(&self, _domain: &str, _session_id: &str) {
        // bao domains keep no per-WS-session handler state — nothing to do.
    }

    fn notify_session_destroyed(&self, _domains: &[String], _session_id: &str) {
        // Flattened CDP sessions outlive the WS connection that minted them
        // only in Chrome; here entries are removed by Target.detachFromTarget
        // and dropped with the registry.
    }

    fn has_domain(&self, domain: &str) -> bool {
        SERVED_DOMAINS.contains(&domain)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bao_cdp::servo_bridge::{bridge_channel, BridgeResponse};
    use std::sync::Arc;
    use std::time::Duration;

    struct NopSender;
    impl EventSender for NopSender {
        fn send_event(&self, _: &str, _: Value) {}
    }

    struct CapturingSender {
        events: Mutex<Vec<(String, Value)>>,
        session_events: Mutex<Vec<(String, String, Value)>>,
    }
    impl CapturingSender {
        fn new() -> Arc<Self> {
            Arc::new(CapturingSender {
                events: Mutex::new(Vec::new()),
                session_events: Mutex::new(Vec::new()),
            })
        }
    }
    impl EventSender for CapturingSender {
        fn send_event(&self, method: &str, params: Value) {
            self.events
                .lock()
                .unwrap()
                .push((method.to_string(), params));
        }
        fn send_session_event(&self, session_id: &str, method: &str, params: Value) {
            self.session_events
                .lock()
                .unwrap()
                .push((session_id.to_string(), method.to_string(), params));
        }
    }

    fn page_responder(rx: bao_cdp::servo_bridge::BridgeReceiver) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            loop {
                let handled = rx.try_process(|cmd| match cmd {
                    BridgeCommand::ListTargets => BridgeResponse {
                        result: Ok(json!([
                            { "id": "1", "title": "Page 1", "url": "about:blank" }
                        ])),
                    },
                    BridgeCommand::Navigate { .. } => BridgeResponse {
                        result: Ok(json!({ "frameId": "1", "loaderId": "loader-1" })),
                    },
                    BridgeCommand::EvaluateJs { expression, .. } => BridgeResponse {
                        result: Ok(json!({ "result": { "type": "string", "value": expression } })),
                    },
                    _ => BridgeResponse {
                        result: Ok(json!({})),
                    },
                });
                if !handled {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        })
    }

    fn msg(method: &str, params: Value, session_id: Option<String>) -> CdpMessage {
        CdpMessage {
            id: Some(1),
            method: method.to_string(),
            params: Some(params),
            session_id,
        }
    }

    // @trace TEST-CDP-005 [req:REQ-CDP-005] [level:unit]
    #[test]
    fn attach_to_target_mints_real_unique_sessions() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = page_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;

        let r1 = reg
            .dispatch_message(
                &msg(
                    "Target.attachToTarget",
                    json!({"targetId": "1", "flatten": true}),
                    None,
                ),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        let r2 = reg
            .dispatch_message(
                &msg(
                    "Target.attachToTarget",
                    json!({"targetId": "1", "flatten": true}),
                    None,
                ),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        let s1 = r1["sessionId"].as_str().unwrap().to_string();
        let s2 = r2["sessionId"].as_str().unwrap().to_string();
        assert_ne!(s1, s2, "each attach mints a fresh session id");
    }

    #[test]
    fn attach_to_target_requires_flatten() {
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;
        let err = reg
            .dispatch_message(
                &msg("Target.attachToTarget", json!({"targetId": "1"}), None),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap_err();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("flatten"));
    }

    #[test]
    fn flattened_session_routes_to_attached_target() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = page_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;

        let r = reg
            .dispatch_message(
                &msg(
                    "Target.attachToTarget",
                    json!({"targetId": "1", "flatten": true}),
                    None,
                ),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        let sid = r["sessionId"].as_str().unwrap().to_string();

        // Page.navigate carrying the sessionId must route to target "1" —
        // the responder answers every Navigate with frameId "1".
        let nav = reg
            .dispatch_message(
                &msg("Page.navigate", json!({"url": "about:blank"}), Some(sid.clone())),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        assert_eq!(nav["frameId"], "1");
    }

    #[test]
    fn unknown_session_id_is_explicit_error() {
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;
        let err = reg
            .dispatch_message(
                &msg("Page.navigate", json!({"url": "about:blank"}), Some("nope".into())),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap_err();
        assert_eq!(err.code, -32001);
        assert!(err.message.contains("not found"));
    }

    #[test]
    fn detach_removes_routing_entry() {
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;
        // attach without responder still mints (no bridge round-trip needed)
        let r = reg
            .dispatch_message(
                &msg(
                    "Target.attachToTarget",
                    json!({"targetId": "7", "flatten": true}),
                    None,
                ),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        let sid = r["sessionId"].as_str().unwrap().to_string();
        reg.dispatch_message(
            &msg(
                "Target.detachFromTarget",
                json!({"sessionId": sid.clone()}),
                None,
            ),
            BROWSER_TARGET,
            &sender,
        )
        .unwrap()
        .unwrap();
        let err = reg
            .dispatch_message(&msg("Page.enable", json!({}), Some(sid)), BROWSER_TARGET, &sender)
            .unwrap()
            .unwrap_err();
        assert_eq!(err.code, -32001);
    }

    #[test]
    fn page_session_target_used_without_session_id() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = page_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;
        // Runtime.evaluate on a /devtools/page/1 session routes to target "1".
        let r = reg
            .dispatch_message(
                &msg("Runtime.evaluate", json!({"expression": "1+1"}), None),
                "1",
                &sender,
            )
            .unwrap()
            .unwrap();
        assert_eq!(r["result"]["value"], "1+1");
    }

    #[test]
    fn fetch_domain_is_explicit_error() {
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;
        let err = reg
            .dispatch_message(
                &msg(
                    "Fetch.enable",
                    json!({"patterns": [{"urlPattern": "*"}]}),
                    None,
                ),
                "1",
                &sender,
            )
            .unwrap()
            .unwrap_err();
        assert!(err.message.contains("no request interception facility"));
    }

    #[test]
    fn has_domain_served_domains() {
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        assert!(reg.has_domain("Page"));
        assert!(reg.has_domain("Runtime"));
        assert!(reg.has_domain("Target"));
        assert!(reg.has_domain("Browser"));
        assert!(!reg.has_domain("NotADomain"));
    }

    #[test]
    fn set_auto_attach_emits_attached_to_target_for_existing_pages() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = page_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = CapturingSender::new();

        reg.dispatch_message(
            &msg("Target.setAutoAttach", json!({"autoAttach": true, "flatten": true}), None),
            BROWSER_TARGET,
            &*sender,
        )
        .unwrap()
        .unwrap();

        let events = sender.events.lock().unwrap();
        let attach_events: Vec<_> = events
            .iter()
            .filter(|(m, _)| m == "Target.attachedToTarget")
            .collect();
        assert_eq!(attach_events.len(), 1, "one event per listed page");
        let (_, params) = &attach_events[0];
        assert_eq!(params["targetInfo"]["targetId"], "1");
        assert!(params["sessionId"].as_str().is_some());

        // The minted session is really routable.
        let sid = params["sessionId"].as_str().unwrap().to_string();
        drop(events);
        let r = reg
            .dispatch_message(
                &msg("Runtime.evaluate", json!({"expression": "x"}), Some(sid)),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn runtime_enable_emits_execution_context_created_on_session() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = page_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = CapturingSender::new();

        let attach = reg
            .dispatch_message(
                &msg(
                    "Target.attachToTarget",
                    json!({"targetId": "1", "flatten": true}),
                    None,
                ),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap();
        let sid = attach["sessionId"].as_str().unwrap().to_string();

        reg.dispatch_message(
            &msg("Runtime.enable", json!({}), Some(sid.clone())),
            BROWSER_TARGET,
            &*sender,
        )
        .unwrap()
        .unwrap();

        let session_events = sender.session_events.lock().unwrap();
        let ctx_events: Vec<_> = session_events
            .iter()
            .filter(|(s, m, _)| s == &sid && m == "Runtime.executionContextCreated")
            .collect();
        assert_eq!(ctx_events.len(), 1);
        assert!(ctx_events[0].2["context"]["id"].as_u64().is_some());
    }

    #[test]
    fn navigate_emits_frame_lifecycle_events_on_session() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = page_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = CapturingSender::new();

        let attach = reg
            .dispatch_message(
                &msg(
                    "Target.attachToTarget",
                    json!({"targetId": "1", "flatten": true}),
                    None,
                ),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap();
        let sid = attach["sessionId"].as_str().unwrap().to_string();

        reg.dispatch_message(
            &msg("Page.navigate", json!({"url": "https://example.com"}), Some(sid.clone())),
            BROWSER_TARGET,
            &*sender,
        )
        .unwrap()
        .unwrap();

        let session_events = sender.session_events.lock().unwrap();
        let methods: Vec<&str> = session_events
            .iter()
            .filter(|(s, _, _)| s == &sid)
            .map(|(_, m, _)| m.as_str())
            .collect();
        assert!(methods.contains(&"Page.frameStartedLoading"));
        assert!(methods.contains(&"Page.frameNavigated"));
        let nav = session_events
            .iter()
            .find(|(_, m, _)| m == "Page.frameNavigated")
            .unwrap();
        assert_eq!(nav.2["frame"]["url"], "https://example.com");
        assert_eq!(nav.2["frame"]["id"], "1");
    }
}
