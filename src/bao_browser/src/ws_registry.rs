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

use bao_cdp::servo_bridge::{
    BridgeCommand, BridgeSender, DEFAULT_BROWSER_CONTEXT_ID, main_frame_id_for_target,
};
use cdp_server::{CdpError, CdpMessage, EventSender, RegistryDispatch};
use serde_json::{json, Value};

/// JSON-RPC error code for an unknown/flattened CDP session id
/// (Chrome: "Session with given id not found").
const ERR_SESSION_NOT_FOUND: i64 = -32001;

/// JSON-RPC error code for a target that does not resolve to a live page
/// (Chrome: "No target with given id found" / "Target closed").
const ERR_TARGET_NOT_FOUND: i64 = -32000;

/// The browser-endpoint pseudo target — target id of WS connections to
/// `/devtools/browser` (see cdp-server `handle_connection`).
const BROWSER_TARGET: &str = "__browser__";

/// Domains this registry serves — the method-reachability metadata table.
/// Faces (M1 wiring, REQ-CDP-001):
/// - 21 production domains handled by `bao_cdp::protocol::handle_command`;
/// - `ElementHandle` / `JSHandle` (M1 P1): the B-class Playwright surface
///   served by the `-32601` RDP fallback arm (`self.rdp`) — same production
///   `BridgeCommand` channel, listed here so the domain metadata covers the
///   full reachable method face (has_domain is advisory today — no call site
///   gates on it; keeping the table complete is the honest metadata).
const SERVED_DOMAINS: [&str; 23] = [
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
    "ElementHandle",
    "JSHandle",
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
    /// M1 wiring (REQ-CDP-001, user ruling "留并接线"): the parallel-universe
    /// dispatcher mounted as the -32601 fallback arm on the WS face too —
    /// its backend is the SAME production channel (BridgeSenderBackend over
    /// `bridge`), so B-class methods (Page.title / ElementHandle.* /
    /// JSHandle.*) served here terminate in the single servo truth.
    rdp: bao_cdp_client::bridge::CDPRdpBridge,
    /// Flattened-session routing table: CDP sessionId → target id.
    attached_sessions: Mutex<HashMap<String, String>>,
    /// Created isolated-world names per session — re-announced per document
    /// after navigation. Chrome re-announces every existing world's context
    /// when a navigation destroys the old document's contexts; clients bind
    /// their isolated realms (Puppeteer's utility world) to the new context
    /// and hang on the next evaluate without it.
    session_worlds: Mutex<HashMap<String, Vec<String>>>,
    /// Whether the browser session asked for auto-attach (Target.setAutoAttach
    /// with autoAttach=true) — new targets emit Target.attachedToTarget.
    auto_attach: Mutex<bool>,
}

impl BaoWsRegistry {
    pub fn new(bridge: BridgeSender) -> Self {
        BaoWsRegistry {
            rdp: bao_cdp_client::bridge::CDPRdpBridge::new(std::sync::Arc::new(
                bao_cdp_client::bridge::BridgeSenderBackend::new(bridge.clone()),
            )),
            bridge,
            attached_sessions: Mutex::new(HashMap::new()),
            session_worlds: Mutex::new(HashMap::new()),
            auto_attach: Mutex::new(false),
        }
    }

    /// M1 P1 (REQ-CDP-001): the fallback universe's event tap. The runtime
    /// pump (`run_with_bridge`) feeds every translated CDP event into it —
    /// the same event stream WS sessions broadcast — and the `waitFor*`
    /// family dispatched through `rdp` waits on it. `None` only when the
    /// backend has no tap (not the shape `new` builds; kept for the trait's
    /// honest default).
    pub fn event_tap(&self) -> Option<std::sync::Arc<bao_cdp_client::bridge::CdpEventTap>> {
        self.rdp.backend().event_tap().cloned()
    }

    /// Handle the session-table commands that only this registry can serve
    /// (it owns the sessionId→target table). Returns None when `method` is
    /// not a session-table command.
    fn dispatch_session_command(
        &self,
        method: &str,
        params: &Option<Value>,
        msg: &CdpMessage,
        ws_target_id: &str,
        event_sender: &dyn EventSender,
    ) -> Option<Result<Value, CdpError>> {
        if method == "Page.addScriptToEvaluateOnNewDocument" {
            let source = params
                .as_ref()
                .and_then(|p| p.get("source"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Resolve the command's target: flattened sessionId wins, else
            // the WS session's own target (/devtools/page/<id>). The browser
            // pseudo-target is not a page — Chrome answers such a
            // registration from a browser session with an error, and so do
            // we (fail-closed: an unresolvable target must not silently
            // widen the script to every page).
            let target_id = match &msg.session_id {
                Some(sid) => self
                    .attached_sessions
                    .lock()
                    .ok()
                    .and_then(|t| t.get(sid).cloned()),
                None => Some(ws_target_id.to_string()),
            };
            let Some(target_id) = target_id else {
                let sid = msg.session_id.as_deref().unwrap_or_default();
                return Some(Err(CdpError {
                    code: ERR_SESSION_NOT_FOUND,
                    message: format!("Session with given id not found: {sid}"),
                }));
            };
            let Some(page_id) = target_id.parse::<usize>().ok() else {
                return Some(Err(CdpError {
                    code: ERR_TARGET_NOT_FOUND,
                    message: format!("No target with given id found: {target_id}"),
                }));
            };
            let Some(webview_id) = crate::page::webview_id_for_page(page_id) else {
                return Some(Err(CdpError {
                    code: ERR_TARGET_NOT_FOUND,
                    message: format!("Target closed: {target_id}"),
                }));
            };
            // Vendor realm entry injection (REQ-CDP-004): servo evaluates
            // the script on every new document of this webview BEFORE any
            // page script — replacing the former pump-timed dispatch that
            // fired after parsing began (0/40 NO-HARVEST). The identifier is
            // minted by the vendor registry itself (the single id source for
            // both CDP faces — self-minting here would desynchronize the
            // remove face) and returned verbatim to the client; the same
            // value removes the script via the memory-bridge face.
            let script_id = servo::register_embedder_new_document_script(webview_id, source);
            return Some(Ok(json!({ "identifier": script_id.to_string() })));
        }
        self.dispatch_session_command_inner(method, params, msg, ws_target_id, event_sender)
    }

    fn dispatch_session_command_inner(
        &self,
        method: &str,
        params: &Option<Value>,
        msg: &CdpMessage,
        ws_target_id: &str,
        event_sender: &dyn EventSender,
    ) -> Option<Result<Value, CdpError>> {
        match method {
            "Target.attachToTarget" => Some(self.attach_to_target(params)),
            "Target.detachFromTarget" => Some(self.detach_from_target(params)),
            // W40 (#11-D): Puppeteer's page-session setup enables a sweep of
            // domains. These are ACCEPTED-BUT-INERT here: enable/disable is
            // a truthful subscription acknowledgment (the session tracks the
            // domain), and no domain data or events are ever fabricated —
            // any other method in these domains still answers not-found.
            // (The W11 matrix marks them out-of-matrix; this is the honest
            // compat face for clients that gate on the enable round-trip.)
            "Audits.enable" | "Audits.disable"
            | "Tracing.enable" | "Tracing.disable" | "Tracing.start" | "Tracing.end"
            | "Performance.enable" | "Performance.disable"
            | "Accessibility.enable" | "Accessibility.disable"
            | "Console.enable" | "Console.disable"
            | "HeapProfiler.enable" | "HeapProfiler.disable"
            | "Profiler.enable" | "Profiler.disable"
            | "Overlay.enable" | "Overlay.disable"
            | "SystemInfo.enable" | "SystemInfo.disable" => {
                Some(Ok(serde_json::json!({})))
            }
            // W40 (#11-D) retirement via REQ-CDS-009: Target.getBrowserContexts
            // / createBrowserContext / disposeBrowserContext are no longer
            // served here — they fall through to the bridge-backed
            // bao_cdp dispatch (the context registry lives browser-side with
            // the PagePool, the single servo truth).
            // W40 (#11-D) + REQ-CDS-009 C5: the static browser pseudo-target
            // answer serves only the browser-level handshake (no session, no
            // targetId). Session-bound or targeted queries fall through to
            // the bridge-backed attribution arm (protocol.rs handle_target →
            // GetTargetContextId), which carries the real browserContextId.
            "Target.getTargetInfo"
                if msg.session_id.is_none()
                    && params
                        .as_ref()
                        .and_then(|p| p.get("targetId"))
                        .is_none() =>
            {
                Some(Ok(serde_json::json!({
                    "targetInfo": {
                        "targetId": "bao-browser",
                        "type": "browser",
                        "title": "Bao",
                        "url": "",
                        "attached": true,
                    }
                })))
            }
            // Page.createIsolatedWorld needs the event face (the new context
            // is announced via a session-scoped Runtime.executionContextCreated),
            // so it is served here rather than in the stateless dispatch.
            "Page.createIsolatedWorld" => {
                Some(self.create_isolated_world(params, msg, ws_target_id, event_sender))
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
                // REQ-CDS-009 criterion 5: attribution is the listing entry's
                // context id (browser-side registry truth) — the default id
                // is the fallback for entries predating the field.
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
                            "browserContextId": entry
                                .get("browserContextId")
                                .cloned()
                                .unwrap_or(json!(DEFAULT_BROWSER_CONTEXT_ID)),
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
        ws_target_id: &str,
        event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        let world_name = params
            .as_ref()
            .and_then(|p| p.get("worldName"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Chrome shape: the created context's auxData carries the frame the
        // world was created in (clients bind the world to that frame through
        // it — Puppeteer's isolated realm resolution requires it). The
        // requesting frameId when the client named one, else the owning
        // target's main frame (REQ-CDP-004 per-target derivation; the
        // command's target resolves exactly like
        // Page.addScriptToEvaluateOnNewDocument: flattened session wins, else
        // the WS session's own page target).
        let command_target = match &msg.session_id {
            Some(sid) => self
                .attached_sessions
                .lock()
                .ok()
                .and_then(|t| t.get(sid).cloned())
                .unwrap_or_else(|| ws_target_id.to_string()),
            None => ws_target_id.to_string(),
        };
        let frame_id = params
            .as_ref()
            .and_then(|p| p.get("frameId"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| main_frame_id_for_target(&command_target));
        let context_id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
        if let Some(sid) = msg.session_id.as_deref() {
            // Remember the world so navigations re-announce its context
            // (Chrome keeps isolated worlds alive across documents).
            if !world_name.is_empty() {
                if let Some(mut m) = self.session_worlds.lock().ok() {
                    let worlds = m.entry(sid.to_string()).or_default();
                    if !worlds.contains(&world_name) {
                        worlds.push(world_name.clone());
                    }
                }
            }
            event_sender.send_session_event(
                sid,
                "Runtime.executionContextCreated",
                json!({
                    "context": {
                        "id": context_id,
                        "origin": "-",
                        "name": world_name,
                        "auxData": {
                            "isDefault": false,
                            "frameId": frame_id,
                        },
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

    /// Chrome-shape `Runtime.executionContextCreated` payload, shared by
    /// the default-context announces and the isolated-world re-announces
    /// (REQ-CDP-004 frame identity, never the PageId namespace).
    fn emit_execution_context(
        &self,
        event_sender: &dyn EventSender,
        sid: Option<&str>,
        context_id: u64,
        name: &str,
        is_default: bool,
        frame_id: &str,
    ) {
        self.emit(
            event_sender,
            sid,
            "Runtime.executionContextCreated",
            json!({
                "context": {
                    "id": context_id,
                    "origin": "-",
                    "name": name,
                    "auxData": {
                        "isDefault": is_default,
                        "type": "default",
                        "frameId": frame_id,
                    },
                }
            }),
        );
    }

    /// Chrome close semantics: Target.closeTarget must be followed by
    /// Target.targetDestroyed (to everyone who saw the target) and
    /// Target.detachedFromTarget (per attached session) — clients resolve
    /// page.close() on the session-detach signal and hang forever without
    /// it (puppeteer W40 S11 stall).
    fn emit_target_closed_events(
        &self,
        event_sender: &dyn EventSender,
        msg: &CdpMessage,
        target_id: &str,
    ) {
        let closed_tid = msg
            .params
            .as_ref()
            .and_then(|p| p.get("targetId"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(target_id)
            .to_string();
        self.emit_target_destroyed(event_sender, &closed_tid);
    }

    /// The per-target half of the Chrome close-event face — shared by
    /// Target.closeTarget and Target.disposeBrowserContext (REQ-CDS-009
    /// criterion 2: a disposed context's pages must die observably, or
    /// client-side page objects for them strand).
    fn emit_target_destroyed(&self, event_sender: &dyn EventSender, closed_tid: &str) {
        self.emit(
            event_sender,
            None,
            "Target.targetDestroyed",
            json!({ "targetId": closed_tid }),
        );
        // Detach every CDP session bound to the closed target (Chrome
        // closes them) — tagged per-session events plus table purge.
        if let Ok(mut table) = self.attached_sessions.lock() {
            let bound: Vec<String> = table
                .iter()
                .filter(|(_, t)| t.as_str() == closed_tid)
                .map(|(s, _)| s.clone())
                .collect();
            for s in bound {
                table.remove(&s);
                // Chrome shape: detachedFromTarget arrives untagged on the
                // parent connection (mirroring how the matching
                // attachedToTarget was delivered) — puppeteer's Connection
                // routes by the message sessionId tag to find the parent
                // session, and a dying-session tag points at the wrong
                // object. params.sessionId names the closed session.
                self.emit(
                    event_sender,
                    None,
                    "Target.detachedFromTarget",
                    json!({
                        "sessionId": s,
                        "targetId": closed_tid,
                    }),
                );
            }
        }
    }

    /// Playwright's page-session init: Runtime.enable must be followed by
    /// executionContextCreated or evaluate() has no context to bind to.
    /// Chrome shape: auxData carries the owning frameId — clients
    /// (Playwright/Puppeteer) bind the default context to the frame
    /// through it. Same per-target frame identity the navigate response
    /// and every frame event carry (REQ-CDP-004) — never the PageId
    /// (targetId namespace).
    fn emit_runtime_enabled_context(
        &self,
        event_sender: &dyn EventSender,
        sid: Option<&str>,
        target_id: &str,
    ) {
        let context_id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.emit_execution_context(
            event_sender,
            sid,
            context_id,
            "",
            true,
            &main_frame_id_for_target(target_id),
        );
    }

    /// Page.navigate command face: Chrome emits NO frame lifecycle events
    /// from the command path — the browser process event stream is the
    /// sole source (REQ-CDP-004). The real face (servo delegate → event
    /// queue → pump) delivers frameStartedLoading / frameNavigated /
    /// frameStoppedLoading with real load timing; the former command-face
    /// synth pair was retired once real-path delivery was probe-proven
    /// lossless. What stays here is the execution-context semantics the
    /// real path has no equivalent for.
    fn emit_page_navigate_contexts(
        &self,
        event_sender: &dyn EventSender,
        sid: Option<&str>,
        result: &Result<Value, CdpError>,
        target_id: &str,
    ) {
        let Ok(r) = result else {
            return;
        };
        // The response frameId is authoritative; the tolerance fallback
        // derives the same per-target main frame id the real face reports
        // (never the PageId — that is the targetId namespace).
        let fid = r
            .get("frameId")
            .and_then(|v| v.as_str())
            .unwrap_or(&main_frame_id_for_target(target_id))
            .to_string();
        // Cross-document navigation replaces the document's execution
        // contexts (Chrome semantics): clear the old ones and announce a
        // fresh default context bound to the frame, or clients wait for a
        // context that never comes after navigation.
        self.emit(event_sender, sid, "Runtime.executionContextsCleared", json!({}));
        let context_id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.emit_execution_context(event_sender, sid, context_id, "", true, &fid);
        // Chrome keeps isolated worlds alive across documents: re-announce
        // each created world's context for the new document (clients bound
        // their realms to contexts the navigation just destroyed and would
        // hang without it).
        let worlds = sid
            .and_then(|sid| {
                self.session_worlds
                    .lock()
                    .ok()
                    .and_then(|m| m.get(sid).cloned())
            })
            .unwrap_or_default();
        for world_name in worlds {
            let context_id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
            self.emit_execution_context(event_sender, sid, context_id, &world_name, false, &fid);
        }
    }

    /// Auto-attach for programmatically created targets:
    /// Target.createTarget → Target.attachedToTarget event so Playwright's
    /// context.new_page() completes.
    fn emit_target_created_attached(
        &self,
        event_sender: &dyn EventSender,
        result: &Result<Value, CdpError>,
    ) {
        let auto = self.auto_attach.lock().map(|f| *f).unwrap_or(false);
        if !auto {
            return;
        }
        let Ok(r) = result else {
            return;
        };
        if let Some(new_id) = r.get("targetId").and_then(|v| v.as_str()) {
            let session_id = self.mint_session(new_id);
            // REQ-CDS-009 criterion 4/5: the create response carries the
            // context the page landed in (read back from the pool registry
            // by the bridge face) — the event attributes the same truth.
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
                        "browserContextId": r
                            .get("browserContextId")
                            .cloned()
                            .unwrap_or(json!(DEFAULT_BROWSER_CONTEXT_ID)),
                    },
                }),
            );
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

impl BaoWsRegistry {
    /// W43 flat-session event demux + Target 路由波 tightening: route a
    /// servo-target-scoped CDP event to every CDP session attached to
    /// `target_id` (each tagged with its sessionId via `send_session_event`).
    ///
    /// No attached session for this target → the event is DROPPED with a
    /// debug log (Chrome semantics: no subscriber, no delivery —
    /// REQ-CDP-004). This replaces the W49 miss fallback (untagged broadcast
    /// + tagged copy to every attached session), which existed only because
    /// the delegate tagged events with the placeholder target "0" so real
    /// lookups always missed; the same wave switched every emitter to the
    /// page's real CDP target, making the fallback a cross-page pollution
    /// channel (one page's `frameNavigated` landing in every other attached
    /// page's FrameManager — the phantom-frame class) instead of a delivery
    /// mechanism. Removed in the same batch as the placeholder emission so
    /// no intermediate state drops all placeholder-target events.
    pub fn broadcast_for_target(
        &self,
        event_sender: &dyn EventSender,
        target_id: &str,
        method: &str,
        params: Value,
    ) {
        let sessions: Vec<String> = self
            .attached_sessions
            .lock()
            .ok()
            .map(|table| {
                table
                    .iter()
                    .filter(|(_, tid)| tid.as_str() == target_id)
                    .map(|(sid, _)| sid.clone())
                    .collect()
            })
            .unwrap_or_default();

        if sessions.is_empty() {
            // No flattened session attached to this target — but a
            // `/devtools/page/<id>` connection IS a subscription to that
            // page (Chrome delivers the page's events on its endpoint
            // untagged), so the event goes to the page-endpoint subscribers
            // of exactly this target. The broadcaster keeps the tightened
            // semantics on its side: browser-endpoint sessions receive
            // nothing and zero page subscribers → the event is dropped
            // (no-subscriber-no-deliver — the placeholder-target
            // broadcast-everywhere fallback is NOT resurrected).
            log::debug!(
                "[cdp-route] {method}: no flattened session attached to target {target_id} — routing to page-endpoint subscribers"
            );
            event_sender.send_page_event(target_id, method, params);
            return;
        }
        for sid in sessions {
            event_sender.send_session_event(&sid, method, params.clone());
        }
    }
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
            self.dispatch_session_command(&msg.method, &msg.params, msg, ws_target_id, event_sender)
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

        // REQ-CDP-009: the screencast trio is served by bao_browser's own
        // frame-production domain BEFORE the bao_cdp dispatch (which has no
        // screencast handler). Target validation mirrors the
        // session-command face: unresolvable targets fail closed.
        // @trace REQ-CDP-009 [level:library]
        let null_params = Value::Null;
        let sc_params = msg.params.as_ref().unwrap_or(&null_params);
        if let Some(result) =
            crate::screencast::dispatch_ws_command(&msg.method, sc_params, &target_id)
        {
            return Some(result);
        }

        // Real command face: bao_cdp's servo-bridge-backed domain dispatch.
        let response =
            bao_cdp::handle_command(msg.clone(), &target_id, &msg.params, Some(&self.bridge));
        let mut result = match (response.result, response.error) {
            (Some(result), _) => Ok(result),
            (None, Some(err)) => Err(err),
            (None, None) => Ok(json!({})),
        };
        // M1 wiring (REQ-CDP-001): -32601 is the ONLY fall-through — the
        // parallel universe serves what the production core doesn't know
        // (B-class Playwright surface); explicit not-supported verdicts stay
        // authoritative. A universe miss keeps the production error.
        if let Err(err) = &result {
            if err.code == -32601 {
                let fallback_params = msg.params.clone().unwrap_or(Value::Null);
                match self
                    .rdp
                    .dispatch(&target_id, &msg.method, fallback_params)
                {
                    Ok(v) => result = Ok(v),
                    // Universe miss — the production "'X.y' wasn't found"
                    // verdict in `result` stays as-is.
                    Err(
                        bao_cdp_client::bridge::BridgeError::MethodNotFound(_)
                        | bao_cdp_client::bridge::BridgeError::InvalidMethod(_),
                    ) => {}
                    Err(e) => {
                        result = Err(CdpError {
                            code: e.cdp_error_code() as i64,
                            message: e.message(),
                        })
                    }
                }
            }
        }

        // Post-command lifecycle events (the "events 按需" face Playwright's
        // init/navigation sequences are driven by).
        if result.is_ok() {
            let sid = msg.session_id.as_deref();
            match msg.method.as_str() {
                // Chrome close semantics — see `emit_target_closed_events`.
                "Target.closeTarget" => {
                    self.emit_target_closed_events(event_sender, msg, &target_id);
                }
                // REQ-CDS-009 criterion 2: disposing a context closes its
                // pages — every closed target gets the same Chrome
                // close-event stream Target.closeTarget delivers.
                "Target.disposeBrowserContext" => {
                    if let Ok(closed) = &result {
                        for tid in closed
                            .get("closedTargetIds")
                            .and_then(|v| v.as_array())
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str())
                        {
                            self.emit_target_destroyed(event_sender, tid);
                        }
                    }
                }
                // Playwright page-session init — see
                // `emit_runtime_enabled_context`.
                "Runtime.enable" => {
                    self.emit_runtime_enabled_context(event_sender, sid, &target_id);
                }
                // Command-face execution-context semantics — see
                // `emit_page_navigate_contexts`.
                "Page.navigate" => {
                    self.emit_page_navigate_contexts(event_sender, sid, &result, &target_id);
                }
                // Auto-attach — see `emit_target_created_attached`.
                "Target.createTarget" => {
                    self.emit_target_created_attached(event_sender, &result);
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
            self.session_events.lock().unwrap().push((
                session_id.to_string(),
                method.to_string(),
                params,
            ));
        }
    }

    fn page_responder(rx: bao_cdp::servo_bridge::BridgeReceiver) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || loop {
            let handled = rx.try_process(|cmd| match cmd {
                BridgeCommand::ListTargets => BridgeResponse {
                    result: Ok(json!([
                        { "id": "1", "title": "Page 1", "url": "about:blank" }
                    ])),
                },
                BridgeCommand::Navigate { .. } => BridgeResponse {
                    result: Ok(json!({
                        "frameId": main_frame_id_for_target("1"),
                        "loaderId": "loader-1"
                    })),
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
                &msg(
                    "Page.navigate",
                    json!({"url": "about:blank"}),
                    Some(sid.clone()),
                ),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        assert_eq!(nav["frameId"], main_frame_id_for_target("1"));
    }

    #[test]
    fn unknown_session_id_is_explicit_error() {
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;
        let err = reg
            .dispatch_message(
                &msg(
                    "Page.navigate",
                    json!({"url": "about:blank"}),
                    Some("nope".into()),
                ),
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
            .dispatch_message(
                &msg("Page.enable", json!({}), Some(sid)),
                BROWSER_TARGET,
                &sender,
            )
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
        // REQ-CDP-006 C6: Fetch.enable is a real bridge command now (the
        // interception slot lives browser-side). With a live-but-unresponsive
        // bridge channel the face is the bridge timeout error, never a
        // canned "enabled".
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
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("bridge response timeout"));
    }

    #[test]
    fn has_domain_served_domains() {
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        assert!(reg.has_domain("Page"));
        assert!(reg.has_domain("Runtime"));
        assert!(reg.has_domain("Target"));
        assert!(reg.has_domain("Browser"));
        // M1 P1 (REQ-CDP-001): the -32601 fallback universe's B-class
        // domains are part of the reachable method face — the metadata
        // table must cover them.
        assert!(reg.has_domain("ElementHandle"));
        assert!(reg.has_domain("JSHandle"));
        assert!(!reg.has_domain("NotADomain"));
    }

    #[test]
    fn set_auto_attach_emits_attached_to_target_for_existing_pages() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = page_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = CapturingSender::new();

        reg.dispatch_message(
            &msg(
                "Target.setAutoAttach",
                json!({"autoAttach": true, "flatten": true}),
                None,
            ),
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
        // REQ-CDP-004 (v7 path B): auxData.frameId is the per-target main
        // frame id — the same value the navigate response and every frame
        // event for this target carry, never the bare PageId.
        assert_eq!(
            ctx_events[0].2["context"]["auxData"]["frameId"],
            main_frame_id_for_target("1")
        );
    }

    #[test]
    fn navigate_command_face_emits_no_synth_frame_events() {
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
            &msg(
                "Page.navigate",
                json!({"url": "https://example.com"}),
                Some(sid.clone()),
            ),
            BROWSER_TARGET,
            &*sender,
        )
        .unwrap()
        .unwrap();

        let session_events = sender.session_events.lock().unwrap();
        // REQ-CDP-004 synth retirement: the command face emits NO frame
        // lifecycle events — the real event path (servo delegate → event
        // queue → pump → broadcast) is the sole frame-event source.
        assert!(
            !session_events
                .iter()
                .any(|(_, m, _)| m == "Page.frameStartedLoading"),
            "command face must not synth frameStartedLoading"
        );
        assert!(
            !session_events
                .iter()
                .any(|(_, m, _)| m == "Page.frameNavigated"),
            "command face must not synth frameNavigated"
        );
        // The execution-context semantics the real path has no equivalent
        // for stay on the command face.
        assert!(
            session_events
                .iter()
                .any(|(_, m, _)| m == "Runtime.executionContextsCleared"),
            "executionContextsCleared synth must survive retirement"
        );
        assert!(
            session_events
                .iter()
                .any(|(_, m, _)| m == "Runtime.executionContextCreated"),
            "executionContextCreated synth must survive retirement"
        );
    }

    // ── Target 路由波 pins (REQ-CDP-004): targeted delivery ────────────

    // @trace TEST-CDP-004 [req:REQ-CDP-004] [level:unit]
    #[test]
    fn broadcast_for_target_delivers_only_to_sessions_attached_to_the_target() {
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        let sender = CapturingSender::new();

        // Two flattened sessions: s1 attached to target "1", s2 to "2".
        let s1 = reg
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
            .unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .to_string();
        let s2 = reg
            .dispatch_message(
                &msg(
                    "Target.attachToTarget",
                    json!({"targetId": "2", "flatten": true}),
                    None,
                ),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .to_string();

        // A target-"1" event must reach ONLY s1 (tagged), never s2 and never
        // the untagged broadcast face.
        reg.broadcast_for_target(
            &*sender,
            "1",
            "Page.frameNavigated",
            json!({"frame": {"id": "main-1"}}),
        );
        let session_events = sender.session_events.lock().unwrap();
        assert_eq!(session_events.len(), 1, "exactly one tagged delivery");
        assert_eq!(session_events[0].0, s1);
        assert_eq!(session_events[0].1, "Page.frameNavigated");
        assert!(sender.events.lock().unwrap().is_empty(), "no untagged broadcast");
        drop(session_events);

        // Same for target "2".
        reg.broadcast_for_target(
            &*sender,
            "2",
            "Page.frameStartedLoading",
            json!({"frameId": "main-2"}),
        );
        let session_events = sender.session_events.lock().unwrap();
        assert_eq!(session_events.len(), 2);
        assert_eq!(session_events[1].0, s2);
        assert_eq!(session_events[1].1, "Page.frameStartedLoading");
    }

    // @trace TEST-CDP-004 [req:REQ-CDP-004] [level:unit]
    #[test]
    fn broadcast_for_target_miss_is_dropped_not_broadcast() {
        // Target 路由波: the W49 fallback (untagged broadcast + tagged copy
        // to every attached session) is replaced by Chrome semantics — no
        // subscriber, no delivery. An event for a target nobody attached to
        // must reach NOTHING (this is the phantom-frame containment pin: a
        // page-B session never receives page-A frame events).
        let (tx, _rx) = bridge_channel(Duration::from_millis(100));
        let reg = BaoWsRegistry::new(tx);
        let sender = CapturingSender::new();

        // s1 attached to "1"; the event is for unrelated target "9".
        reg.dispatch_message(
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

        reg.broadcast_for_target(
            &*sender,
            "9",
            "Page.frameNavigated",
            json!({"frame": {"id": "main-9"}}),
        );

        assert!(
            sender.session_events.lock().unwrap().is_empty(),
            "unsubscribed target events must not reach attached sessions"
        );
        assert!(
            sender.events.lock().unwrap().is_empty(),
            "unsubscribed target events must not degrade to the untagged broadcast"
        );
    }

    // ── REQ-CDS-009: browser context lifecycle over the WS face ─────────

    /// Browser-side context registry emulation (the same arms
    /// `cdp_handler::handle_bridge_command` serves) — scripted so the WS-face
    /// tests stay hermetic.
    fn context_responder(rx: bao_cdp::servo_bridge::BridgeReceiver) -> std::thread::JoinHandle<()> {
        use bao_cdp::servo_bridge::BridgeResponse;
        let created: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let counter = Arc::new(AtomicU64::new(1));
        std::thread::spawn(move || loop {
            let handled = rx.try_process(|cmd| {
                let result = match cmd {
                    BridgeCommand::CreateBrowserContext => {
                        let n = counter.fetch_add(1, Ordering::Relaxed);
                        let id = format!("bao-context-{n:016x}");
                        created.lock().unwrap().push(id.clone());
                        Ok(json!({ "browserContextId": id }))
                    }
                    BridgeCommand::ListBrowserContexts => Ok(json!({
                        "browserContextIds": created.lock().unwrap().clone()
                    })),
                    BridgeCommand::DisposeBrowserContext { browser_context_id } => {
                        let mut ids = created.lock().unwrap();
                        if let Some(pos) = ids.iter().position(|c| *c == browser_context_id) {
                            ids.remove(pos);
                            // Emulates the pool response: the context's member
                            // pages were closed (this hermetic fixture pins
                            // target 9 as the context's member —
                            // createTargetInContext mints it below).
                            Ok(json!({ "closedTargetIds": ["9"] }))
                        } else {
                            Err(bao_cdp::servo_bridge::BridgeError::domain(format!(
                                "Failed to find browser context: {browser_context_id}"
                            )))
                        }
                    }
                    BridgeCommand::CreateTargetInContext {
                        url,
                        browser_context_id,
                    } => {
                        if created.lock().unwrap().contains(&browser_context_id) {
                            Ok(json!({
                                "targetId": "9",
                                "browserContextId": browser_context_id,
                                "_url": url,
                            }))
                        } else {
                            Err(bao_cdp::servo_bridge::BridgeError::domain(format!(
                                "Failed to find browser context: {browser_context_id}"
                            )))
                        }
                    }
                    BridgeCommand::ListTargets => Ok(json!([
                        {
                            "id": "1",
                            "title": "Page 1",
                            "url": "about:blank",
                            "browserContextId": "bao-context-1",
                        }
                    ])),
                    _ => Ok(json!({})),
                };
                BridgeResponse { result }
            });
            if !handled {
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:1,3] [level:unit]
    #[test]
    fn ws_browser_session_context_create_and_list_roundtrip() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = context_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = NopSender;

        // Chrome's getBrowserContexts before any explicit creation: empty —
        // the implicit default is never listed.
        let empty = reg
            .dispatch_message(
                &msg("Target.getBrowserContexts", json!({}), None),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        assert_eq!(empty["browserContextIds"], json!([]));

        let created = reg
            .dispatch_message(
                &msg("Target.createBrowserContext", json!({}), None),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        let id = created["browserContextId"].as_str().unwrap().to_string();
        assert!(id.starts_with("bao-context-"), "minted id: {id}");
        assert_ne!(id, "bao-default-context");

        let listed = reg
            .dispatch_message(
                &msg("Target.getBrowserContexts", json!({}), None),
                BROWSER_TARGET,
                &sender,
            )
            .unwrap()
            .unwrap();
        assert_eq!(listed["browserContextIds"], json!([id]));
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:2] [level:unit]
    #[test]
    fn ws_dispose_browser_context_removes_emits_close_events_unknown_fails() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = context_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = CapturingSender::new();

        let id = reg
            .dispatch_message(
                &msg("Target.createBrowserContext", json!({}), None),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap()["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();

        reg.dispatch_message(
            &msg(
                "Target.disposeBrowserContext",
                json!({"browserContextId": id}),
                None,
            ),
            BROWSER_TARGET,
            &*sender,
        )
        .unwrap()
        .unwrap();

        // Chrome close semantics: every target the disposal closed dies
        // observably — targetDestroyed on the browser stream.
        let events = sender.events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|(m, p)| m == "Target.targetDestroyed" && p["targetId"] == "9"),
            "disposed context's pages must emit targetDestroyed: {events:?}"
        );
        drop(events);

        let listed = reg
            .dispatch_message(
                &msg("Target.getBrowserContexts", json!({}), None),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap();
        assert_eq!(listed["browserContextIds"], json!([]));

        // Unknown id → the domain-classified server error, never a fake ok.
        let err = reg
            .dispatch_message(
                &msg(
                    "Target.disposeBrowserContext",
                    json!({"browserContextId": "bao-context-404"}),
                    None,
                ),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap_err();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("Failed to find browser context"));
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:4,5] [level:unit]
    #[test]
    fn ws_create_target_in_context_and_auto_attach_attribution() {
        let (tx, rx) = bridge_channel(Duration::from_secs(2));
        let _keeper = context_responder(rx);
        let reg = BaoWsRegistry::new(tx);
        let sender = CapturingSender::new();

        let id = reg
            .dispatch_message(
                &msg("Target.createBrowserContext", json!({}), None),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap()["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();

        // createTarget carrying the context id → created inside it, and the
        // auto-attach event attributes the same context.
        reg.dispatch_message(
            &msg(
                "Target.setAutoAttach",
                json!({"autoAttach": true, "flatten": true}),
                None,
            ),
            BROWSER_TARGET,
            &*sender,
        )
        .unwrap()
        .unwrap();
        {
            let events = sender.events.lock().unwrap();
            let attach = events
                .iter()
                .find(|(m, _)| m == "Target.attachedToTarget")
                .expect("auto-attach enumerates existing pages");
            assert_eq!(attach.1["targetInfo"]["browserContextId"], "bao-context-1");
        }

        let created = reg
            .dispatch_message(
                &msg(
                    "Target.createTarget",
                    json!({"url": "about:blank", "browserContextId": id}),
                    None,
                ),
                BROWSER_TARGET,
                &*sender,
            )
            .unwrap()
            .unwrap();
        assert_eq!(created["browserContextId"], id);

        // Auto-attach for the programmatically created target reuses the
        // response's attribution.
        let events = sender.events.lock().unwrap();
        let attach_for_new = events
            .iter()
            .filter(|(m, _)| m == "Target.attachedToTarget")
            .find(|(_, p)| p["targetInfo"]["targetId"] == "9")
            .expect("created target auto-attached");
        assert_eq!(attach_for_new.1["targetInfo"]["browserContextId"], id);
    }
}
