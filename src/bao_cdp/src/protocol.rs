// REQ-CDP-001: BAO CDP 11-domain command dispatch (servo-bridge backed).
// @trace REQ-CDP-001 [entity:CdpServer]
//
// REQ-PERF-002: Domain handlers run on the single servo script thread — all
// scalar mutable state is Cell/RefCell, never std::sync::Mutex<scalar>.
// @trace REQ-PERF-002 [entity:DomainHandler] [level:integration]
//
// REQ-PERF-004: 11 domains dispatch via a static match arm (enum dispatch),
// not Box<dyn DomainHandler> vtable. Unknown domains hit the wildcard arm
// returning ERR_METHOD_NOT_FOUND.
// @trace REQ-PERF-004 [entity:DomainDispatch] [level:integration]
//
// REQ-PERF-005: Empty params flow as Option::None (no Box::new([])); the
// dispatch loop uses iterator-collected arrays, never C-style index loops.
// @trace REQ-PERF-005 [entity:CodePattern] [level:integration]
//
// TASK-4-CDP: Removed dead protocol types (CDPMessage/CDPResponse/CDPError/
// CDPEvent) + dead codec helpers (parse_message/serialize_response/
// serialize_event). These were byte-for-byte duplicates of
// `cdp_server::protocol`. The wire types are now reused directly from the
// `cdp-server` crate (re-exported via `bao_cdp`).
//
// What remains here is the *BAO-specific* domain dispatch — `handle_command`
// routes the 11 CDP domains to servo-bridge-backed handlers (Page.navigate,
// Runtime.evaluate, DOM.getDocument, ...). The codec (parse_message/
// serialize_response/serialize_event), the ok/error response constructors
// and the shared JSON-RPC 2.0 error codes are single-sourced from
// `cdp_server::protocol` (M4, e152 audit DUP-CODEC-WRAPPERS): that module is
// `pub`, so nothing codec-shaped is re-implemented here anymore. Only
// ERR_NOT_SUPPORTED (-32000, Chrome "server error") stays local — it is a
// BAO-specific code cdp-server does not define.
//
// Re-export the JSON-RPC 2.0 wire types so internal modules (`backend.rs`,
// `router.rs`) and the crate root can refer to them as `protocol::CdpError`
// etc. These ARE `cdp_server` types — no duplicate definitions.
pub use cdp_server::{
    parse_message, serialize_event, serialize_response, CdpError, CdpMessage, CdpResponse,
};
// Private before M4, private after: same items, now imported instead of
// re-implemented. ERR_INVALID_PARAMS keeps pub(crate) reach for devtools_dom.
use cdp_server::{error_response, ok_response, ERR_METHOD_NOT_FOUND};
pub(crate) use cdp_server::ERR_INVALID_PARAMS;
use serde_json::Value;

use crate::devtools_dom;
// FetchResolveAction: REQ-CDP-006 C6 — the Fetch domain and the legacy
// Network interception face both resolve paused requests through it.
use crate::servo_bridge::{
    BridgeCommand, BridgeErrorKind, BridgeSender, FetchResolveAction, DEFAULT_BROWSER_CONTEXT_ID,
};

// Chrome DevTools "server error" code used for not-supported commands.
pub(crate) const ERR_NOT_SUPPORTED: i64 = -32000;

/// Build a not-supported error for a command whose backing facility does not
/// exist (servo/SM face absent). Explicit failure — never a canned success.
fn not_supported(method: &str, reason: &str) -> CdpError {
    CdpError {
        code: ERR_NOT_SUPPORTED,
        message: format!("'{method}' not supported: {reason}"),
    }
}

/// BAO 11-domain CDP command dispatch.
///
/// Routes `msg.method` ("Domain.command") to the appropriate domain handler,
/// forwarding servo-state-dependent commands (Page.navigate, Runtime.evaluate,
/// DOM.getDocument, ...) to the optional servo [`BridgeSender`]. Commands that
/// don't need servo state return stub/default responses.
///
/// Wire types come from `cdp_server` (id is `Option<i64>` per JSON-RPC 2.0,
/// which permits responses to notifications that carry no id).
pub fn handle_command(
    msg: CdpMessage,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> CdpResponse {
    let parts: Vec<&str> = msg.method.splitn(2, '.').collect();
    let domain = parts.first().copied().unwrap_or("");
    let command = parts.get(1).copied().unwrap_or("");

    let result = match domain {
        "Target" => handle_target(command, target_id, params, bridge),
        "Page" => handle_page(command, target_id, params, bridge),
        "Runtime" => handle_runtime(command, target_id, params, bridge),
        "DOM" => handle_dom(command, target_id, params, bridge),
        "Network" => handle_network(command, target_id, params, bridge),
        "CSS" => handle_css(command, target_id, params, bridge),
        "Emulation" => handle_emulation(command, target_id, params, bridge),
        "Input" => handle_input(command, target_id, params, bridge),
        "Overlay" => handle_overlay(command, target_id, params, bridge),
        "Debugger" => handle_debugger(command, target_id, params, bridge),
        "Log" => handle_log(command),
        // REQ-CDP-006 C6: the Fetch interception commands are bridge-backed
        // (the interception slot lives browser-side, design §5).
        "Fetch" => handle_fetch(command, target_id, params, bridge),
        "Storage" => handle_storage(command, target_id, params, bridge),
        "Security" => handle_security(command, target_id, params, bridge),
        "Profiler" => handle_profiler(command, target_id, params, bridge),
        "HeapProfiler" => handle_heap_profiler(command, target_id, bridge),
        "Memory" => handle_memory(command, target_id, bridge),
        "Performance" => handle_performance(command, target_id, bridge),
        "SystemInfo" => handle_system_info(command),
        // REQ-BRW-004: ServiceWorker CDP observability domain  @trace REQ-BRW-004 [criterion:19]
        "ServiceWorker" => handle_service_worker(command, target_id, params, bridge),
        // Browser domain — Playwright's connect_over_cdp handshake sends
        // Browser.getVersion as the first command after the WS opens.
        "Browser" => handle_browser(command, params),
        _ => Err(CdpError {
            code: ERR_METHOD_NOT_FOUND,
            message: format!("'{}' wasn't found", msg.method),
        }),
    };

    match result {
        Ok(r) => ok_response(msg.id, r),
        Err(e) => error_response(msg.id, e.code, e.message),
    }
}

pub(crate) type HandlerResult = Result<Value, CdpError>;

fn params_str(params: &Option<Value>, key: &str) -> String {
    params
        .as_ref()
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn bridge_send(bridge: Option<&BridgeSender>, cmd: BridgeCommand) -> HandlerResult {
    match bridge {
        Some(b) => {
            let resp = b.send(cmd);
            // e234/e236: the Err payload carries its own class — transport
            // faces (no bridge / closed / timeout) stay -32603, while
            // domain-owned outcomes (Fetch.resolve UnknownId/Rejected,
            // Overlay unknown node/object id) surface as -32000, the code
            // Chrome's DevTools uses for the same class (the in-repo
            // not-supported face shares the value).
            resp.result.map_err(|e| CdpError {
                code: match e.kind {
                    BridgeErrorKind::NoBridge => -32603,
                    BridgeErrorKind::DomainError => ERR_NOT_SUPPORTED,
                },
                message: e.message,
            })
        }
        None => Err(CdpError {
            code: -32603,
            message: "no servo bridge connected".into(),
        }),
    }
}

fn ok_empty() -> HandlerResult {
    Ok(serde_json::json!({}))
}

/// Evaluate a JS expression on the target and extract the JSON document it
/// returns. The expression must produce `JSON.stringify(...)` output; the
/// EvaluateJs bridge path parses it into `result.value` (an object).
pub(crate) fn eval_json(
    bridge: Option<&BridgeSender>,
    tid: &str,
    expression: &str,
) -> HandlerResult {
    let resp = bridge_send(
        bridge,
        BridgeCommand::EvaluateJs {
            target_id: tid.to_string(),
            expression: expression.to_string(),
            return_by_value: true,
        },
    )?;
    let extracted = resp.get("result").and_then(|r| r.get("value")).cloned();
    // W19a envelope adapter: since 3b74a655 the rbv bridge returns the raw
    // completion value under `result.value` — stringify-style page queries
    // arrive as a JSON-encoded STRING; peel it back to the document object
    // (Chrome returnByValue semantic: value is the parsed structure).
    let extracted = match extracted {
        Some(Value::String(ref text)) => match serde_json::from_str::<Value>(text) {
            Ok(v) => Some(v),
            Err(_) => Some(Value::String(text.clone())),
        },
        other => other,
    };
    extracted
        .filter(|v| v.is_object())
        .ok_or_else(|| CdpError {
            code: ERR_NOT_SUPPORTED,
            message: "page query failed: target did not return a JSON document".into(),
        })
}

fn live_target_info(
    target_id: &str,
    bridge: Option<&BridgeSender>,
    context_hint: Option<&str>,
) -> Value {
    let title = bridge
        .and_then(|b| {
            b.send(BridgeCommand::GetTitle {
                target_id: target_id.to_string(),
            })
            .result
            .ok()
        })
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "Bao".into());
    let url = bridge
        .and_then(|b| {
            b.send(BridgeCommand::GetUrl {
                target_id: target_id.to_string(),
            })
            .result
            .ok()
        })
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "about:blank".into());
    serde_json::json!({
        "targetId": target_id,
        "type": "page",
        "title": title,
        "url": url,
        "attached": true,
        // Real attribution (REQ-CDS-009): the context id the target was
        // created in — passed through from the listing entry, queried from
        // the bridge registry, or the implicit default when the target was
        // created outside any explicit BrowserContext. Clients like
        // Playwright require the field present.
        "browserContextId": context_hint
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_BROWSER_CONTEXT_ID),
    })
}

/// Per-target attribution query for the single-target faces (`getTargetInfo`):
/// the browser-side context registry is the truth source. A dead/unresolvable
/// target degrades to the implicit default — the same fallback every
/// bridge-less face uses — rather than failing the info lookup.
fn target_context_id(target_id: &str, bridge: Option<&BridgeSender>) -> String {
    bridge
        .and_then(|b| {
            b.send(BridgeCommand::GetTargetContextId {
                target_id: target_id.to_string(),
            })
            .result
            .ok()
        })
        .and_then(|v| {
            v.get("browserContextId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| DEFAULT_BROWSER_CONTEXT_ID.to_string())
}

fn handle_target(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    match command {
        "getTargets" | "getTargetTargets" => {
            // Real page enumeration via the bridge (PagePool). Falls back to
            // the session's own target only when no bridge is connected (the
            // bridge-less unit-test face).
            let mut target_infos: Vec<Value> = Vec::new();
            let listed = bridge
                .and_then(|b| b.send(BridgeCommand::ListTargets).result.ok())
                .and_then(|v| v.as_array().cloned());
            match listed {
                Some(entries) => {
                    for entry in entries {
                        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        // REQ-CDS-009 attribution: the listing entry carries
                        // the context the browser-side registry recorded at
                        // creation — passed through verbatim, never re-minted.
                        let context = entry
                            .get("browserContextId")
                            .and_then(|v| v.as_str())
                            .unwrap_or(DEFAULT_BROWSER_CONTEXT_ID);
                        let mut info = live_target_info(id, bridge, Some(context));
                        // REQ-CDP-010: pass the real opener relationship the
                        // listing entry already carried through (never
                        // re-derived, never a default).
                        if let Some(opener) = entry.get("canAccessOpener") {
                            if let Some(obj) = info.as_object_mut() {
                                obj.insert("canAccessOpener".into(), opener.clone());
                            }
                        }
                        target_infos.push(info);
                    }
                }
                None => target_infos.push(live_target_info(target_id, bridge, None)),
            }
            push_worker_target_infos(&mut target_infos, target_id, bridge);
            Ok(serde_json::json!({ "targetInfos": target_infos }))
        }
        "createTarget" => {
            // Real page creation via the bridge (PagePool::create_page). The
            // bridge handler returns the genuinely new page id — its response
            // is the truth; without a bridge there is no page pool, explicit
            // error (never an echo of the current target).
            // BCE-20260621-EMPTY-STR: empty url "" = "not provided" → about:blank.
            let url = params
                .as_ref()
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("about:blank");
            // REQ-CDS-009 criterion 4: a browserContextId param routes the
            // creation into that context (unknown ids fail closed at the
            // browser-side registry, Chrome's "Failed to find browser
            // context" server error). No param → the implicit default.
            let context_id = params
                .as_ref()
                .and_then(|v| v.get("browserContextId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            match context_id {
                Some(ctx) => bridge_send(
                    bridge,
                    BridgeCommand::CreateTargetInContext {
                        url: url.to_string(),
                        browser_context_id: ctx.to_string(),
                    },
                ),
                None => bridge_send(
                    bridge,
                    BridgeCommand::CreateTarget {
                        url: url.to_string(),
                    },
                ),
            }
        }
        // REQ-CDS-009 criteria 1-3: browser context lifecycle. All three are
        // bridge-backed — the registry lives browser-side with the PagePool
        // (single servo truth); the protocol face is thin dispatch.
        "createBrowserContext" => bridge_send(bridge, BridgeCommand::CreateBrowserContext),
        "disposeBrowserContext" => {
            let context_id = params
                .as_ref()
                .and_then(|v| v.get("browserContextId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "'Target.disposeBrowserContext' requires a non-empty browserContextId param"
                        .into(),
                })?;
            bridge_send(
                bridge,
                BridgeCommand::DisposeBrowserContext {
                    browser_context_id: context_id.to_string(),
                },
            )
        }
        // REQ-CDP-010 golden-oracle parity: Chrome's result carries
        // `defaultBrowserContextId` alongside `browserContextIds` (Playwright
        // locates the default context from it). The bridge owns the context
        // registry; add the default-context id (the canonical constant) to
        // whatever the registry returned — a real value, never a placeholder.
        "getBrowserContexts" => {
            let mut resp = bridge_send(bridge, BridgeCommand::ListBrowserContexts)?;
            if let Some(obj) = resp.as_object_mut() {
                obj.entry("defaultBrowserContextId")
                    .or_insert_with(|| serde_json::json!(DEFAULT_BROWSER_CONTEXT_ID));
            }
            Ok(resp)
        }
        "closeTarget" => {
            // CDP semantics: closes the target named by params.targetId (the
            // session's own target when omitted). Blocking bridge round-trip —
            // success means the page was really closed.
            let tid = params
                .as_ref()
                .and_then(|v| v.get("targetId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(target_id);
            bridge_send(
                bridge,
                BridgeCommand::ClosePage {
                    target_id: tid.to_string(),
                },
            )?;
            Ok(serde_json::json!({ "success": true }))
        }
        // Subscription acks: events are delivered through the EventBroadcaster
        // (Path B); these commands carry no per-command result payload.
        "setAutoAttach" | "setDiscoverTargets" => ok_empty(),
        "getTargetInfo" => {
            let tid = params
                .as_ref()
                .and_then(|v| v.get("targetId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(target_id);
            // REQ-CDS-009 attribution: the single-target face queries the
            // browser-side registry (criterion 5).
            let context = target_context_id(tid, bridge);
            let mut info = live_target_info(tid, bridge, Some(&context));
            // REQ-CDP-010: the opener relationship — the projection of the
            // real BaoWebViewState.opener_page_id (window.open popups only).
            let has_opener = bridge
                .and_then(|b| {
                    b.send(BridgeCommand::GetTargetOpenerAccess {
                        target_id: tid.to_string(),
                    })
                    .result
                    .ok()
                })
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Some(obj) = info.as_object_mut() {
                obj.insert("canAccessOpener".into(), serde_json::json!(has_opener));
            }
            Ok(serde_json::json!({ "targetInfo": info }))
        }
        // Session-table commands: minting/removing CDP sessions requires the
        // WS session registry (bao_browser::ws_registry::BaoWsRegistry), the
        // only component that owns the sessionId→target table. This stateless
        // dispatch cannot serve them — explicit error, never a fabricated
        // sessionId.
        "attachToTarget" => Err(not_supported(
            "Target.attachToTarget",
            "session minting requires the WS session registry (bao_browser); the stateless internal backend has no session table",
        )),
        "detachFromTarget" | "sendMessageToTarget" => Err(not_supported(
            &format!("Target.{command}"),
            "session routing requires the WS session registry (bao_browser); the stateless internal backend has no session table",
        )),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Target.{}' wasn't found", command),
        }),
    }
}

/// REQ-BRW-004: append Worker sub-targets to the Target.getTargets response
/// (list miss / bridge-less → nothing appended).
// @trace REQ-BRW-004 [criterion:19] Worker targets are CDP-observable
fn push_worker_target_infos(
    target_infos: &mut Vec<Value>,
    target_id: &str,
    bridge: Option<&BridgeSender>,
) {
    let Some(b) = bridge else {
        return;
    };
    let Ok(worker_resp) = b
        .send(BridgeCommand::ListWorkerTargets {
            target_id: target_id.to_string(),
        })
        .result
    else {
        return;
    };
    let Some(workers) = worker_resp.get("workerTargets").and_then(|v| v.as_array()) else {
        return;
    };
    for w in workers {
        target_infos.push(w.clone());
    }
}

fn handle_page(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        // Chrome semantics: enable returns {} and the page domain becomes an
        // event source. bao additionally arms the DOMContentLoaded
        // observability here (REQ-CDP-010: `Page.domContentEventFired` needs
        // a page-realm listener; `Page.enable` is the natural subscription
        // point — without it no DCL events are emitted, matching Chrome's
        // enable-gated delivery). The install is best-effort with a warn: a
        // page that cannot host the listener keeps a truthful enable ack,
        // exactly like the Runtime registry prelude's install convention.
        "enable" => { // @evidence .spec/10-REQUIREMENTS.html:2324
            if bridge.is_some() {
                let _ = bridge_send(
                    bridge,
                    BridgeCommand::InstallDomContentHook { target_id: tid },
                );
            }
            ok_empty()
        }
        "disable" => ok_empty(), // @evidence .spec/10-REQUIREMENTS.html:2324
        // Subscription ack: frame lifecycle events (frameStartedLoading /
        // frameNavigated) flow through the WS registry's event face.
        "setLifecycleEventsEnabled" => ok_empty(),
        "navigate" => {
            // BCE-20260621-EMPTY-STR: empty url "" must fall back to "about:blank"
            // (CDP/Chrome semantics: empty url = "not provided"). `Option::as_str()`
            // returns Some("") for empty strings, bypassing `unwrap_or`, so we add
            // `.filter(|s| !s.is_empty())` to treat "" as "not provided".
            let url = params
                .as_ref()
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("about:blank");
            // The bridge handler's response is the truth: frameId is the servo
            // event stream's main-frame id (same value every frame event
            // carries, REQ-CDP-004) plus the per-load loaderId from the
            // browser-side registry row (REQ-CDP-010 — the same identity
            // Page.getFrameTree reports for this load).
            let mut response = bridge_send(
                bridge,
                BridgeCommand::Navigate {
                    target_id: tid,
                    url: url.to_string(),
                },
            )?;
            // REQ-CDP-010 golden-oracle parity (Chrome 155, measured): the
            // navigate response carries `isDownload` — and on this embedder
            // surface the answer is always false (the embedder never treats
            // a load as a download; there is no download pipeline to
            // reclassify a navigation).
            if let Some(obj) = response.as_object_mut() {
                obj.entry("isDownload").or_insert(serde_json::json!(false));
            }
            Ok(response)
        }
        "reload" => {
            let ignore_cache = params
                .as_ref()
                .and_then(|p| p.get("ignoreCache"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Real servo reload. Chrome 155 oracle: the response is the
            // empty object; the reload's new per-load loaderId (REQ-CDP-010)
            // is observable through Page.getFrameTree.
            bridge_send(
                bridge,
                BridgeCommand::Reload {
                    target_id: tid,
                    ignore_cache,
                },
            )
        }
        "getFrameTree" => {
            // Real main-frame data: url/mimeType/name/securityOrigin read from
            // the live document via evaluate; frame id = the per-target main
            // frame id (REQ-CDP-004) — the same identifier navigate/reload
            // report and the frame events carry. Child frames are not
            // enumerable from the embedder — none are fabricated.
            // REQ-CDP-010 golden-oracle parity (Chrome 155 oracle, measured):
            // the frame face also carries
            // domainAndRegistry/securityOriginDetails/secureContextType/
            // crossOriginIsolatedContextType/gatedAPIFeatures/adFrameStatus.
            // All are derived from the live location/document, none invented:
            // secureContextType follows the potentially-trustworthy rule
            // (https or localhost/loopback), domainAndRegistry is "" for
            // empty/IP/loopback hosts (Chrome's rule; for named hosts the
            // hostname is the honest best value — no PSL in the embedding
            // surface, eTLD+1 reduction not attempted). loaderId is read
            // from the browser-side per-load registry row below (REQ-CDP-010
            // per-load identity — the same value Page.navigate reports).
            let mut frame = eval_json(
                bridge,
                &tid,
                r#"(function(){
                    var h = location.hostname;
                    var isLocal = (h === 'localhost' || h === '::1' || h.indexOf('::1') === 0 || h.indexOf('127.') === 0);
                    var ipLike = isLocal || /^\d+\.\d+\.\d+\.\d+$/.test(h) || (h === '' && location.protocol !== 'file:');
                    return JSON.stringify({
                        url: location.href,
                        mimeType: document.contentType,
                        name: window.name,
                        securityOrigin: location.origin,
                        domainAndRegistry: (h === '' || ipLike) ? '' : h,
                        secureContextType: ((location.protocol === 'https:') || isLocal) ? 'SecureContext' : 'InsecureScheme',
                        securityOriginDetails: { isLocalhost: isLocal },
                        crossOriginIsolatedContextType: window.crossOriginIsolated ? 'Isolated' : 'NotIsolated',
                        gatedAPIFeatures: [],
                        adFrameStatus: { adFrameType: 'none' }
                    }); })()"#,
            )?;
            // REQ-CDP-010: the load identity comes from the browser-side
            // per-load registry row — the SAME value Page.navigate reports
            // for this load — never a value minted on this command face
            // (Chrome 155 oracle: navigate's loaderId and the frame's
            // loaderId agree, and the initial about:blank load has one too).
            // The row is browser state, so it is read through the bridge
            // rather than the page realm.
            let loader_id = bridge_send(
                bridge,
                BridgeCommand::GetLoaderId {
                    target_id: tid.clone(),
                },
            )?;
            let loader_id = loader_id.as_str().ok_or_else(|| CdpError {
                code: ERR_NOT_SUPPORTED,
                message: "page query failed: loader registry did not return a loader id".into(),
            })?;
            if let Some(obj) = frame.as_object_mut() {
                obj.insert(
                    "id".into(),
                    serde_json::json!(crate::servo_bridge::main_frame_id_for_target(&tid)),
                );
                obj.insert("loaderId".into(), serde_json::json!(loader_id));
                // REQ-CDP-010 golden-oracle parity (Chrome 155, measured):
                // the frame omits `name` entirely when empty (the measured
                // 11-key face has no name key) — never a `name: ""` member.
                if obj.get("name").and_then(|v| v.as_str()) == Some("") {
                    obj.remove("name");
                }
            }
            Ok(serde_json::json!({ "frameTree": { "frame": frame } }))
        }
        "getNavigationHistory" => Err(not_supported(
            "Page.getNavigationHistory",
            "servo WebView does not expose session-history entry enumeration (only can_go_back/go_forward traversal)",
        )),
        "captureScreenshot" => {
            let format = params
                .as_ref()
                .and_then(|p| p.get("format"))
                .and_then(|v| v.as_str())
                .unwrap_or("png")
                .to_string();
            let quality = params
                .as_ref()
                .and_then(|p| p.get("quality"))
                .and_then(|v| v.as_u64())
                .map(|q| q as u8);
            // Real webrender screenshot via the bridge; without a bridge there
            // is no renderer — explicit error, never empty image data.
            bridge_send(
                bridge,
                BridgeCommand::TakeScreenshot {
                    target_id: tid,
                    format,
                    quality,
                },
            )
        }
        "setContent" => {
            // Real path — mirrors PageHandle::set_content (document.open/
            // write/close through the page's script thread).
            let html = params_str(params, "html");
            if html.is_empty() {
                return Err(CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "'Page.setContent' requires a non-empty html param".into(),
                });
            }
            let js = format!(
                r#"(function() {{ document.open(); document.write({}); document.close(); }})()"#,
                serde_json::to_string(&html).unwrap_or_default(),
            );
            bridge_send(
                bridge,
                BridgeCommand::EvaluateJs {
                    target_id: tid,
                    expression: js,
                    return_by_value: true,
                },
            )?;
            ok_empty()
        }
        "close" => {
            // Real close — same path as Target.closeTarget (PagePool::close_page).
            bridge_send(bridge, BridgeCommand::ClosePage { target_id: tid })?;
            ok_empty()
        }
        "bringToFront" => {
            // Real focus through the page's window (headless servo has no
            // window manager; window.focus() is the DOM activation path).
            bridge_send(
                bridge,
                BridgeCommand::EvaluateJs {
                    target_id: tid,
                    expression: "window.focus()".into(),
                    return_by_value: true,
                },
            )?;
            ok_empty()
        }
        "getLayoutMetrics" => {
            // Real layout data read from the live document: viewport size from
            // window.innerWidth/innerHeight, content extent from
            // documentElement scroll size. No hardcoded 1920×1080.
            // REQ-CDP-010 golden-oracle parity (Chrome 155 oracle, measured):
            // layoutViewport is {pageX,pageY,clientWidth,clientHeight} (the
            // former {x,y,width,height} keys were the pre-2020 DIP face), and
            // the visualViewport/cssVisualViewport pair is
            // {offsetX,offsetY,pageX,pageY,clientWidth,clientHeight,scale,zoom},
            // read off window.visualViewport when present. bao has no
            // device-pixel-ratio scaling — device and CSS values are identical
            // (same convention as the memory-bridge face in bao_cdp_client
            // layout_metrics_to_json).
            let m = eval_json(
                bridge,
                &tid,
                r#"(function(){ var d = document.documentElement;
                    var vv = window.visualViewport;
                    return JSON.stringify({
                        iw: window.innerWidth, ih: window.innerHeight,
                        cw: d.scrollWidth, ch: d.scrollHeight,
                        vvx: vv ? vv.offsetLeft : 0,
                        vvy: vv ? vv.offsetTop : 0,
                        vvpagex: vv ? vv.pageLeft : 0,
                        vvpagey: vv ? vv.pageTop : 0,
                        vvW: vv ? vv.width : window.innerWidth,
                        vvH: vv ? vv.height : window.innerHeight
                    }); })()"#,
            )?;
            let vv = serde_json::json!({
                "offsetX": m["vvx"], "offsetY": m["vvy"],
                "pageX": m["vvpagex"], "pageY": m["vvpagey"],
                "clientWidth": m["vvW"], "clientHeight": m["vvH"],
                "scale": 1, "zoom": 1
            });
            Ok(serde_json::json!({
                "layoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": m["iw"], "clientHeight": m["ih"] },
                "visualViewport": vv,
                "contentSize": { "x": 0, "y": 0, "width": m["cw"], "height": m["ch"] },
                "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": m["iw"], "clientHeight": m["ih"] },
                "cssVisualViewport": vv,
                "cssContentSize": { "x": 0, "y": 0, "width": m["cw"], "height": m["ch"] }
            }))
        }
        "addScriptToEvaluateOnNewDocument" => {
            // Every source — the empty placeholder Playwright registers
            // included — is registered in the vendor realm-entry registry via
            // the bridge, and the bridge response's identifier IS the
            // vendor-minted id (the single id source, REQ-CDP-004): the same
            // value removes the script later. A face-local mint would
            // fabricate a handle that maps to nothing.
            let source = params_str(params, "source");
            bridge_send(
                bridge,
                BridgeCommand::AddScriptToEvaluateOnNewDocument {
                    target_id: tid,
                    source,
                },
            )
        }
        "removeScriptToEvaluateOnNewDocument" => {
            let identifier = params_str(params, "identifier");
            if identifier.is_empty() {
                return Err(CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "'Page.removeScriptToEvaluateOnNewDocument' requires an identifier param"
                        .into(),
                });
            }
            // Real removal from the vendor registry (REQ-CDP-004): later
            // documents of the page replay only the remaining entries. The
            // bridge answers an unknown identifier with "Script not found"
            // (Chrome page_handler.cc behavior — an error, never a silent
            // ok).
            bridge_send(
                bridge,
                BridgeCommand::RemoveScriptToEvaluateOnNewDocument {
                    target_id: tid,
                    identifier,
                },
            )
        }
        // REQ-CDP-010 golden-oracle parity: servo has no print-to-PDF
        // pipeline — an explicit capability-absent -32000 (Chrome's
        // ServerError class), never the unknown-command -32601.
        "printToPDF" => Err(not_supported(
            "Page.printToPDF",
            "servo WebView has no print-to-PDF pipeline (no PDF backend in the embedding surface)",
        )),
        // REQ-CDP-010 golden-oracle parity (Chrome 155, measured): the real
        // session-history journal — currentIndex/entries with the measured
        // entry shape {id, title, transitionType, url, userTypedURL}; the
        // journal lives browser-side (BaoWebViewState), so the data rides
        // the bridge instead of a page query.
        "getNavigationHistory" => bridge_send( // @evidence .spec/10-REQUIREMENTS.html:2324
            bridge,
            BridgeCommand::GetNavigationHistory { target_id: tid },
        ),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Page.{}' wasn't found", command),
        }),
    }
}

fn handle_runtime(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        // Chrome semantics: Runtime.enable returns {} and fires
        // executionContextCreated events — there is no executionContextId in
        // the response (that was a fabricated context id).
        "enable" => ok_empty(),
        "disable" => ok_empty(),
        "evaluate" => {
            let expression = params
                .as_ref()
                .and_then(|p| p.get("expression"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let return_by_value = params
                .as_ref()
                .and_then(|p| p.get("returnByValue"))
                .and_then(|v| v.as_bool())
                // Chrome's default is false (RemoteObject by reference); the
                // utility-script evaluateHandle that boots Playwright's
                // evaluate pipeline omits the flag and needs the objectId.
                .unwrap_or(false);
            if bridge.is_some() && !expression.is_empty() {
                // Chrome semantics: exceptionDetails is a TOP-LEVEL member of
                // the evaluate response, not nested under `result`. The bridge
                // handler returns the envelope { result, exceptionDetails };
                // lift the members to the response top level (the pre-envelope
                // callers read `result.result` — the lift preserves that path
                // exactly).
                let envelope = bridge_send(
                    bridge,
                    BridgeCommand::EvaluateJs {
                        target_id: tid,
                        expression,
                        return_by_value,
                    },
                )?;
                let mut out = serde_json::json!({ "result": envelope["result"] });
                if let Some(details) = envelope.get("exceptionDetails") {
                    out["exceptionDetails"] = details.clone();
                }
                Ok(out)
            } else {
                Ok(
                    serde_json::json!({ "result": { "type": "undefined" }, "exceptionDetails": null }),
                )
            }
        }
        "callFunctionOn" => {
            let object_id = params
                .as_ref()
                .and_then(|p| p.get("objectId"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let function_declaration = params
                .as_ref()
                .and_then(|p| p.get("functionDeclaration"))
                .and_then(|v| v.as_str())
                .unwrap_or("function(){}")
                .to_string();
            let arguments = params.as_ref().and_then(|p| p.get("arguments")).cloned();
            let return_by_value = params
                .as_ref()
                .and_then(|p| p.get("returnByValue"))
                .and_then(|v| v.as_bool());
            let execution_context_id = params
                .as_ref()
                .and_then(|p| p.get("executionContextId"))
                .and_then(|v| v.as_i64());
            let await_promise = params
                .as_ref()
                .and_then(|p| p.get("awaitPromise"))
                .and_then(|v| v.as_bool());
            let object_group = params
                .as_ref()
                .and_then(|p| p.get("objectGroup"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::RuntimeCallFunctionOn {
                        target_id: tid,
                        object_id,
                        execution_context_id,
                        function_declaration,
                        arguments,
                        return_by_value,
                        await_promise,
                        object_group,
                    },
                )
            } else {
                Ok(serde_json::json!({ "result": { "type": "undefined" } }))
            }
        }
        "getProperties" => {
            let object_id = params
                .as_ref()
                .and_then(|p| p.get("objectId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let own_properties = params
                .as_ref()
                .and_then(|p| p.get("ownProperties"))
                .and_then(|v| v.as_bool());
            if bridge.is_some() && !object_id.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::RuntimeGetProperties {
                        target_id: tid,
                        object_id,
                        own_properties,
                    },
                )
            } else {
                Ok(serde_json::json!({ "result": [] }))
            }
        }
        "evaluateAsync" | "runScript" => {
            Ok(serde_json::json!({ "result": { "type": "undefined" } }))
        }
        "releaseObject" => {
            let object_id = params
                .as_ref()
                .and_then(|p| p.get("objectId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if bridge.is_some() && !object_id.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::RuntimeReleaseObject {
                        target_id: tid,
                        object_id,
                    },
                )
            } else {
                ok_empty()
            }
        }
        "releaseObjectGroup" => {
            let object_group = params
                .as_ref()
                .and_then(|p| p.get("objectGroup"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if bridge.is_some() && !object_group.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::RuntimeReleaseObjectGroup {
                        target_id: tid,
                        object_group,
                    },
                )
            } else {
                ok_empty()
            }
        }
        // REQ-CDP-010 golden-oracle parity (Chrome 155, measured): real
        // page-global bindings — the binding function is installed on the
        // live document AND on every future document (an internal
        // new-document script keeps the install list durable), and each call
        // reports `Runtime.bindingCalled`. Chrome rejects an empty name with
        // -32602; anything else (including `:`-scoped names) is accepted.
        // Without a bridge there is no page global — -32603, never a silent
        // no-op ack.
        "addBinding" => { // @evidence .spec/10-REQUIREMENTS.html:2324
            let name = params_str(params, "name");
            if name.is_empty() {
                return Err(CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "'Runtime.addBinding' requires a non-empty name param".into(),
                });
            }
            bridge_send(
                bridge,
                BridgeCommand::RuntimeAddBinding {
                    target_id: tid,
                    name,
                },
            )
        }
        "removeBinding" => { // @evidence .spec/10-REQUIREMENTS.html:2324
            let name = params_str(params, "name");
            if name.is_empty() {
                return Err(CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "'Runtime.removeBinding' requires a non-empty name param".into(),
                });
            }
            bridge_send(
                bridge,
                BridgeCommand::RuntimeRemoveBinding {
                    target_id: tid,
                    name,
                },
            )
        }
        // REQ-CDP-010 golden-oracle parity (Chrome 155, measured): the probe
        // measured -32602 for a parameter-less compileScript — the mandatory
        // {expression, sourceURL, persistScript} validation is Chrome-faithful.
        // For VALID parameters Chrome compiles the script (never executes) and
        // returns {scriptId}; the compiled-script table that id addresses does
        // not exist on this embedding surface (no persistScript consumer —
        // Runtime.runScript is not wired), so the honest face is the explicit
        // capability-absent -32000, the same class as Page.printToPDF. Never a
        // fabricated scriptId that maps to nothing.
        "compileScript" => { // @evidence .spec/10-REQUIREMENTS.html:2324
            let expression = params_str(params, "expression");
            let source_url = params_str(params, "sourceURL");
            let persist_script = params
                .as_ref()
                .and_then(|p| p.get("persistScript"))
                .and_then(|v| v.as_bool());
            if expression.is_empty() || source_url.is_empty() || persist_script.is_none() {
                return Err(CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "Invalid parameters".into(),
                });
            }
            Err(not_supported(
                "Runtime.compileScript",
                "compile-only script persistence is not wired on this face (no compiled-script table; Runtime.runScript is not implemented)",
            ))
        }
        "callArgument" => ok_empty(), // @evidence .spec/10-REQUIREMENTS.html:2324
        // Ack for the waitForDebuggerOnStart auto-attach flow: bao does not
        // actually pause new targets (no debugger gating exists), so this
        // ack simply unblocks the client's init sequence.
        "runIfWaitingForDebugger" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Runtime.{}' wasn't found", command),
        }),
    }
}

fn handle_dom(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" | "disable" => ok_empty(),
        // REQ-BRW-048 follow-up: the document tree is read from the live
        // page (the bridge walker emits the same canonical identity as
        // devtools_dom). Without a bridge there is no document — explicit
        // error, never a canned tree.
        "getDocument" => bridge_send(
            bridge,
            BridgeCommand::GetDocument {
                target_id: tid.clone(),
            },
        ),
        // Real node data through the identity channel: resolved from the
        // canonical nodeId or a registry objectId, children expanded to
        // `depth` (CDP default 1; negative = full subtree).
        "describeNode" => {
            let node_ref = devtools_dom::require_node_ref(params)?;
            let depth = params
                .as_ref()
                .and_then(|p| p.get("depth"))
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::describe_node_body(&node_ref, depth),
            )
        }
        // The first match's real canonical nodeId (0 when nothing matches —
        // the CDP convention). The former found?1:0 response collided with
        // the document nodeId and was unusable by the style faces.
        "querySelector" => {
            let selector = devtools_dom::require_selector(params)?;
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::query_selector_body(&selector),
            )
        }
        // Every match's real canonical nodeId in document order — the former
        // 1..=count sequence fabricated ids that addressed wrong nodes.
        "querySelectorAll" => {
            let selector = devtools_dom::require_selector(params)?;
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::query_selector_all_body(&selector),
            )
        }
        // Real geometry: border box from getBoundingClientRect, the other
        // boxes from the computed border/padding/margin widths.
        "getBoxModel" => {
            let node_ref = devtools_dom::require_node_ref(params)?;
            devtools_dom::eval_dom(bridge, &tid, &devtools_dom::box_model_body(&node_ref))
        }
        // REQ-BRW-048: flattened tree query for the DevTools element panel —
        // the live tree as a document-order node list with parentId links.
        // depth follows the CDP contract (default 1 = root plus one level of
        // children; negative = the entire subtree). pierce is accepted with no
        // observable effect: the exposed DOM face has no shadow-root
        // boundary, so the walk below IS the whole exposed tree. Without a
        // bridge there is no document — explicit error, never a canned tree.
        "getFlattenedDocument" => {
            let depth = params
                .as_ref()
                .and_then(|p| p.get("depth"))
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::flattened_document_body(depth),
            )
        }
        // REQ-BRW-048: RemoteObject → nodeId mapping (the half that closes
        // the Runtime object protocol with the DOM face): an objectId minted
        // by Runtime.evaluate(returnByValue:false) resolves to the canonical
        // nodeId of the Node it references. Dead handles / non-Node values /
        // detached nodes are explicit errors — never a fabricated id.
        "getNodeForOwner" => {
            let object_id = devtools_dom::require_object_id(params)?;
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::node_for_owner_body(&object_id),
            )
        }
        // REQ-CDP-010 golden-oracle parity (Chrome 155, measured): the real
        // attribute list of the addressed node in CDP's flat pair form; a
        // non-element node fails with Chrome's exact "Node is not an
        // Element" (-32000 class). The former face was absent (-32601).
        "getAttributes" => { // @evidence .spec/10-REQUIREMENTS.html:2324
            let node_ref = devtools_dom::require_node_ref(params)?;
            devtools_dom::eval_dom(bridge, &tid, &devtools_dom::attributes_body(&node_ref))
        }
        // REQ-CDP-010 golden-oracle parity (Chrome 155, measured): a real
        // scroll of the addressed element into the viewport (nearest, only
        // when out of view). The former face was absent (-32601).
        "scrollIntoViewIfNeeded" => { // @evidence .spec/10-REQUIREMENTS.html:2324
            let node_ref = devtools_dom::require_node_ref(params)?;
            devtools_dom::eval_dom(bridge, &tid, &devtools_dom::scroll_into_view_body(&node_ref))
        }
        "setAttributeValue" => {
            let node_id = params
                .as_ref()
                .and_then(|p| p.get("nodeId"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let name = params_str(params, "name");
            let value = params_str(params, "value");
            // REQ-BRW-048 task #10: a node write with no bridge answers
            // -32603 — an ok here was a silent no-op masquerading as a
            // delivered mutation.
            bridge_send(
                bridge,
                BridgeCommand::SetAttributeValue {
                    target_id: tid.clone(),
                    node_id,
                    name,
                    value,
                },
            )
        }
        // REQ-BRW-048 task #10: these mutations have no delivery path on
        // this face (no bridge command carries them) — an unconditional ok
        // was a fake success on BOTH faces. Explicit unsupported, per the
        // StopLoading/Profiler precedent in this file.
        "removeAttribute" | "setOuterHTML" | "insertBefore" | "removeNode" => Err(not_supported(
            &format!("DOM.{}", command),
            "no mutation delivery path exists on this face (the servo embedder bridge carries no such write command)",
        )),
        "getOuterHTML" => {
            let node_id = params
                .as_ref()
                .and_then(|p| p.get("nodeId"))
                .and_then(|v| v.as_i64());
            // Real outerHTML via the page's document; without a bridge there
            // is no document — explicit error, never canned html.
            bridge_send(
                bridge,
                BridgeCommand::GetOuterHtml {
                    target_id: tid,
                    node_id,
                },
            )
        }
        // REQ-BRW-048 follow-up: a real RemoteObject handle minted through
        // the page-realm registry (the same table Runtime.evaluate uses —
        // this face never mints a parallel registry; absent registry is an
        // explicit error).
        "resolveNode" => {
            // CDP spec: DOM.resolveNode accepts nodeId / objectId /
            // backendNodeId (+ executionContextId). Clients resolve elements
            // through the last shape (Puppeteer's isolated-realm adoption
            // sends exactly {backendNodeId, executionContextId});
            // backendNodeId ≡ canonical nodeId on this face (same documented
            // identity as pushNodesByBackendIdsToFrontend below).
            let node_ref = devtools_dom::require_node_ref(params).or_else(|_| {
                let backend = params
                    .as_ref()
                    .and_then(|p| p.get("backendNodeId"))
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| CdpError {
                        code: ERR_INVALID_PARAMS,
                        message: "missing required parameter: nodeId, objectId or backendNodeId"
                            .into(),
                    })?;
                Ok(devtools_dom::NodeRef::NodeId(backend))
            })?;
            let object_group = params
                .as_ref()
                .and_then(|p| p.get("objectGroup"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::resolve_node_body(&node_ref, object_group.as_deref()),
            )
        }
        // backendNodeId ≡ canonical nodeId on this face (documented
        // identity), so the push is a liveness check over real ids: resolved
        // ids are returned, unknown ids are omitted — never fabricated.
        "pushNodesByBackendIdsToFrontend" => {
            let backend_ids = params
                .as_ref()
                .and_then(|p| p.get("backendNodeIds"))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_i64())
                        .collect::<Vec<i64>>()
                })
                .ok_or_else(|| CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "missing required parameter: backendNodeIds".into(),
                })?;
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::push_nodes_body(&backend_ids),
            )
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'DOM.{}' wasn't found", command),
        }),
    }
}

fn handle_network(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::NetworkEnable { target_id: tid })?;
            }
            ok_empty()
        }
        "disable" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::NetworkDisable { target_id: tid })?;
            }
            ok_empty()
        }
        "getCookies" => {
            let urls: Vec<String> = params
                .as_ref()
                .and_then(|p| p.get("urls"))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::GetCookies {
                        target_id: tid,
                        urls,
                    },
                )
            } else {
                Ok(serde_json::json!({ "cookies": [] }))
            }
        }
        "getAllCookies" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::GetAllCookies { target_id: tid })
            } else {
                Ok(serde_json::json!({ "cookies": [] }))
            }
        }
        "setCookie" => {
            let name = params_str(params, "name");
            let value = params_str(params, "value");
            let url = params
                .as_ref()
                .and_then(|p| p.get("url"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let domain = params
                .as_ref()
                .and_then(|p| p.get("domain"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::SetCookie {
                        target_id: tid,
                        name,
                        value,
                        url,
                        domain,
                    },
                )
            } else {
                // CDP spec Network.setCookie returns {"success":true}
                // (SetCookieReturnObject). The no-bridge stub face keeps the
                // spec shape — same as the browser-side bridge path
                // (cmd_set_cookie in bao_browser returns {"success":true}).
                Ok(serde_json::json!({ "success": true }))
            }
        }
        "deleteCookies" => {
            let name = params_str(params, "name");
            let url = params
                .as_ref()
                .and_then(|p| p.get("url"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DeleteCookie {
                        target_id: tid,
                        name,
                        url,
                    },
                )
            } else {
                ok_empty()
            }
        }
        "getResponseBody" => {
            let request_id = params_str(params, "requestId");
            // The bridge handler reports the real availability — servo does
            // not expose stored response bodies, so this resolves to an
            // explicit error rather than an empty-body fake success.
            bridge_send(
                bridge,
                BridgeCommand::GetResponseBody {
                    target_id: tid,
                    request_id,
                },
            )
        }
        "setCacheDisabled" => {
            let cache_disabled = params
                .as_ref()
                .and_then(|p| p.get("cacheDisabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::NetworkSetCacheDisabled {
                        target_id: tid,
                        cache_disabled,
                    },
                )?;
            }
            ok_empty()
        }
        "setExtraHTTPHeaders" => {
            let headers = params
                .as_ref()
                .and_then(|p| p.get("headers"))
                .cloned()
                .unwrap_or(serde_json::json!({}));
            // The bridge handler reports the real support status — servo has
            // no per-target extra-headers API, so this resolves to an explicit
            // error rather than silently dropping the headers.
            bridge_send(
                bridge,
                BridgeCommand::NetworkSetExtraHTTPHeaders {
                    target_id: tid,
                    headers,
                },
            )
        }
        "clearBrowserCache" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::NetworkClearBrowserCache { target_id: tid },
                )?;
            }
            ok_empty()
        }
        "clearBrowserCookies" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::NetworkClearBrowserCookies { target_id: tid },
                )?;
            }
            ok_empty()
        }
        // ── Full-surface completion (cdp-protocol 0.3.1 oracle, 40/40) ──
        // The remaining methods land on three honest tracks; per-method
        // fidelity is documented on each arm:
        //   real bridge  — existing BridgeCommand, real browser state change
        //   truthful     — deterministic answer matching actual capability
        //   fail-closed  — the backing servo/embedder facility does not
        //                  exist: explicit -32000, never a shape-only stub
        //                  that would fake the capability
        "setCookies" => {
            // Real bridge track: batch form of setCookie — one
            // BridgeCommand::SetCookie per params.cookies entry. Field
            // fidelity is identical to the single-setCookie path above
            // (name/value/url/domain carried; path fixed "/", and secure/
            // httpOnly/expires/sameSite are not representable in the bridge
            // payload — the documented single-cookie limitation, inherited).
            let cookies = params
                .as_ref()
                .and_then(|p| p.get("cookies"))
                .and_then(|v| v.as_array())
                .cloned()
                .ok_or_else(|| CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "missing required parameter: cookies".into(),
                })?;
            for c in &cookies {
                let name = c
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let value = c
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let url = c.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());
                let domain = c
                    .get("domain")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if bridge.is_some() {
                    bridge_send(
                        bridge,
                        BridgeCommand::SetCookie {
                            target_id: tid.clone(),
                            name,
                            value,
                            url,
                            domain,
                        },
                    )?;
                }
            }
            // SetCookiesReturnObject is the empty object.
            ok_empty()
        }
        "setUserAgentOverride" => {
            // Real bridge track — same face as Emulation.setUserAgentOverride:
            // navigator.userAgent via the SetUserAgent bridge (real observable
            // override), acceptLanguage/platform via the same defineProperty
            // technique on the page. userAgentMetadata is accepted but not
            // applied: servo has no User-Agent Client Hints surface.
            let ua = params_str(params, "userAgent");
            let accept_language = params
                .as_ref()
                .and_then(|p| p.get("acceptLanguage"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let platform = params
                .as_ref()
                .and_then(|p| p.get("platform"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // REQ-BRW-048 task #10: the UA/language/platform overrides are
            // real navigator writes through the bridge — without a bridge
            // the command is a -32603, never a silent no-op ok.
            if bridge.is_none() {
                return Err(CdpError {
                    code: -32603,
                    message: "no servo bridge connected".into(),
                });
            }
            if !ua.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::SetUserAgent {
                        target_id: tid.clone(),
                        user_agent: ua,
                    },
                )?;
            }
            if accept_language.is_some() || platform.is_some() {
                let mut js = String::from("(function(){var d=Object.defineProperty;");
                if let Some(lang) = &accept_language {
                    js.push_str(&format!(
                        "d(navigator,'language',{{get:function(){{return {};}}}});",
                        serde_json::to_string(lang).unwrap_or_default()
                    ));
                }
                if let Some(p) = &platform {
                    js.push_str(&format!(
                        "d(navigator,'platform',{{get:function(){{return {};}}}});",
                        serde_json::to_string(p).unwrap_or_default()
                    ));
                }
                js.push_str("})();");
                bridge_send(
                    bridge,
                    BridgeCommand::EvaluateJs {
                        target_id: tid.clone(),
                        expression: js,
                        return_by_value: false,
                    },
                )?;
            }
            ok_empty()
        }
        "overrideNetworkState" => {
            // Real bridge track for the observable surface: navigator.onLine
            // override (the Playwright setOffline surface). latency /
            // downloadThroughput / uploadThroughput / connectionType are
            // accepted with no effect — servo has no NetworkInformation API
            // (navigator.connection is undefined) and no embedder-visible
            // throttling hook.
            let offline = params
                .as_ref()
                .and_then(|p| p.get("offline"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if bridge.is_some() {
                // offline:true → navigator.onLine reads false (spec semantics).
                let js = format!(
                    "(function(){{Object.defineProperty(navigator,'onLine',\
                     {{get:function(){{return {};}}}});}})();",
                    !offline
                );
                bridge_send(
                    bridge,
                    BridgeCommand::EvaluateJs {
                        target_id: tid.clone(),
                        expression: js,
                        return_by_value: false,
                    },
                )?;
            }
            ok_empty()
        }
        // Truthful capability answers — these report what the Bao face can
        // really do: clearBrowserCache/Cookies are backed by real servo
        // NetworkManager/SiteDataManager operations behind the bridge arms
        // above; network-condition emulation does not exist anywhere in the
        // stack, so canEmulateNetworkConditions answers false.
        "canClearBrowserCache" => Ok(serde_json::json!({ "result": true })),
        "canClearBrowserCookies" => Ok(serde_json::json!({ "result": true })),
        "canEmulateNetworkConditions" => Ok(serde_json::json!({ "result": false })),
        // Fail-closed track: the backing facility does not exist. Each arm
        // answers an explicit -32000 with the real reason — never a spec-shape
        // stub that would fake the capability (query methods fabricating
        // state, or config methods silently not delivering their promised
        // effect). Params are tolerated: any spec-shaped params reach the
        // capability error, not a parse rejection.
        "getRequestPostData" => Err(not_supported(
            "Network.getRequestPostData",
            "servo does not store request post data for embedder access (same class as Network.getResponseBody)",
        )),
        "getCertificate" => Err(not_supported(
            "Network.getCertificate",
            "servo does not expose TLS certificates to the embedder",
        )),
        "getResponseBodyForInterception" => Err(not_supported(
            "Network.getResponseBodyForInterception",
            "request interception is not implemented — no interceptionId can exist",
        )),
        "takeResponseBodyForInterceptionAsStream" => Err(not_supported(
            "Network.takeResponseBodyForInterceptionAsStream",
            "request interception is not implemented — no stream handle can exist",
        )),
        "searchInResponseBody" => Err(not_supported(
            "Network.searchInResponseBody",
            "servo does not store response bodies for embedder access",
        )),
        "streamResourceContent" => Err(not_supported(
            "Network.streamResourceContent",
            "per-request response streaming state is not tracked by the bridge",
        )),
        "replayXHR" => Err(not_supported(
            "Network.replayXHR",
            "servo does not expose XHR replay to the embedder",
        )),
        "setBlockedURLs" => Err(not_supported(
            "Network.setBlockedURLs",
            "the servo net stack has no embedder-visible request blocklist",
        )),
        "setBypassServiceWorker" => Err(not_supported(
            "Network.setBypassServiceWorker",
            "no runtime service-worker bypass toggle on the servo embedding face",
        )),
        "emulateNetworkConditionsByRule" => Err(not_supported(
            "Network.emulateNetworkConditionsByRule",
            "no network-condition emulation facility exists (Network.canEmulateNetworkConditions answers false)",
        )),
        "configureDurableMessages" => Err(not_supported(
            "Network.configureDurableMessages",
            "response bodies are not buffered outside the renderer (Network.getResponseBody reports the same limitation)",
        )),
        "setAttachDebugStack" => Err(not_supported(
            "Network.setAttachDebugStack",
            "script stack ids are not attached to network requests",
        )),
        "getSecurityIsolationStatus" => Err(not_supported(
            "Network.getSecurityIsolationStatus",
            "COEP/COOP/CSP isolation state is not exposed by the servo embedding face",
        )),
        "enableReportingApi" => Err(not_supported(
            "Network.enableReportingApi",
            "Reporting API tracking is not implemented — no reports can be delivered",
        )),
        "enableDeviceBoundSessions" => Err(not_supported(
            "Network.enableDeviceBoundSessions",
            "device-bound session tracking is not implemented",
        )),
        "fetchSchemefulSite" => Err(not_supported(
            "Network.fetchSchemefulSite",
            "schemeful-site computation requires the public-suffix list, which is not linked into bao_cdp",
        )),
        "loadNetworkResource" => Err(not_supported(
            "Network.loadNetworkResource",
            "embedder-side network fetch is not wired to the CDP face",
        )),
        "setCookieControls" => Err(not_supported(
            "Network.setCookieControls",
            "third-party-cookie restriction is not runtime-controllable on the servo embedding face",
        )),
        // Spec-deprecated with no servo override face — accepted no-op (the
        // legal deprecated implementation; Accept-Encoding is owned by the
        // servo net stack).
        "setAcceptedEncodings" | "clearAcceptedEncodingsOverride" => ok_empty(),
        // REQ-CDP-006 C6 (design §5): the legacy Network interception face
        // shares the Fetch facility through the same bridge path —
        // setRequestInterception arms the same per-target slot (hits emit
        // Network.requestIntercepted, the POP6-C6 criterion wording), and
        // continueInterceptedRequest resolves through the same dispatch.
        "setRequestInterception" => {
            let patterns = parse_interception_patterns(
                params,
                PatternFace::LegacyNetwork,
                "Network.setRequestInterception",
            )?;
            bridge_send(
                bridge,
                BridgeCommand::FetchEnable {
                    target_id: tid,
                    patterns,
                    legacy_network_events: true,
                },
            )
        }
        "continueInterceptedRequest" => {
            let interception_id = interception_id(params, "Network.continueInterceptedRequest")?;
            reject_request_modification("Network.continueInterceptedRequest", params)?;
            if params.as_ref().is_some_and(|p| p.get("rawResponse").is_some()) {
                return Err(not_supported(
                    "Network.continueInterceptedRequest",
                    "raw HTTP wire-format response reassembly is phase-2 — the servo pause face answers with Fetch.fulfillRequest-shaped synthesized responses, not raw response bytes (design §7)",
                ));
            }
            let action = match params.as_ref().and_then(|p| p.get("errorReason")) {
                Some(Value::String(s)) => {
                    FetchResolveAction::Fail { error_reason: Some(s.clone()) }
                }
                Some(_) => {
                    return Err(invalid_params(
                        "Network.continueInterceptedRequest: 'errorReason' must be a CDP ErrorReason string",
                    ))
                }
                None => FetchResolveAction::Continue,
            };
            bridge_send(
                bridge,
                BridgeCommand::FetchResolve {
                    target_id: tid,
                    interception_id,
                    action,
                },
            )
        }
        // Unrelated to interception: cache/offline emulation has no servo
        // embedding face — accepted no-op, unchanged (design §5).
        "emulateNetworkConditions" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Network.{}' wasn't found", command),
        }),
    }
}

fn handle_storage(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "getStorageItemsForOrigin" => {
            let origin = params_str(params, "origin");
            let storage_type = params_str(params, "storageType");
            bridge_send(
                bridge,
                BridgeCommand::StorageGetStorageItemsForOrigin {
                    target_id: tid,
                    origin,
                    storage_type,
                },
            )
        }
        "clearDataForOrigin" => {
            let origin = params_str(params, "origin");
            let storage_type = params_str(params, "storageType");
            bridge_send(
                bridge,
                BridgeCommand::StorageClearDataForOrigin {
                    target_id: tid,
                    origin,
                    storage_type,
                },
            )
        }
        "getCookies" => {
            // Storage.getCookies is an alias for Network.getCookies
            let urls: Vec<String> = params
                .as_ref()
                .and_then(|p| p.get("urls"))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            bridge_send(
                bridge,
                BridgeCommand::GetCookies {
                    target_id: tid,
                    urls,
                },
            )
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Storage.{}' wasn't found", command),
        }),
    }
}

fn handle_security(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" => bridge_send(bridge, BridgeCommand::SecurityEnable { target_id: tid }),
        "disable" => bridge_send(bridge, BridgeCommand::SecurityDisable { target_id: tid }),
        "setOverrideCertificateErrors" => {
            let override_errors = params
                .as_ref()
                .and_then(|p| p.get("override"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            bridge_send(
                bridge,
                BridgeCommand::SecuritySetOverrideCertificateErrors {
                    target_id: tid,
                    override_errors,
                },
            )
        }
        "handleCertificateError" | "certificateError" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Security.{}' wasn't found", command),
        }),
    }
}

fn handle_css(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" | "disable" => ok_empty(),
        // REQ-BRW-048: the CSS query methods read the live page through the
        // EvaluateJs channel. The node id is resolved INSIDE the page with
        // the canonical positional identity (see devtools_dom) — the same
        // encoding DOM.getDocument/getFlattenedDocument emit — so the
        // nodeIds DevTools obtained from the DOM face resolve to the right
        // element. (The dedicated CssGet* bridge commands carried only a raw
        // node id whose browser-side fallback resolution could not address
        // real-document nodes; they are superseded on this dispatch face.)
        // Missing nodeId → -32602; no bridge (no page) → -32603; unknown id /
        // non-element → explicit -32000 from the page. Never an empty style
        // list standing in for data.
        "getComputedStyleForNode" => {
            let node_id = devtools_dom::require_node_id(params)?;
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::computed_style_body(node_id),
            )
        }
        "getMatchedStylesForNode" => {
            let node_id = devtools_dom::require_node_id(params)?;
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::matched_styles_body(node_id),
            )
        }
        "getInlineStylesForNode" => {
            let node_id = devtools_dom::require_node_id(params)?;
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::inline_styles_body(node_id),
            )
        }
        // REQ-BRW-048 follow-up: the real write path. Addressing on this
        // face: styleSheetId = document.styleSheets ordinal (the id
        // getMatchedStylesForNode reports), range.startLine = the rule index
        // within that sheet — Chrome's span model needs stylesheet-text
        // bookkeeping this face never minted. The edit replaces the rule's
        // declaration block via style.cssText (a real CSSOM write that
        // restyles the page) and returns the resulting declarations for
        // read-back. The former {"styles": []} shape-shell is eradicated.
        "setStyleTexts" => {
            let edits = devtools_dom::require_edits(params)?;
            devtools_dom::eval_dom(
                bridge,
                &tid,
                &devtools_dom::set_style_texts_body(&edits),
            )
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'CSS.{}' wasn't found", command),
        }),
    }
}

fn handle_emulation(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        // Preference ack for the DEFAULT media state clients set at init
        // (prefers-color-scheme: light etc.) — bao's pages are in that
        // default state, so the ack is truthful. Non-default emulation is
        // not implemented and surfaces as an explicit error at evaluate time.
        "setEmulatedMedia" => ok_empty(),
        "setDeviceMetricsOverride" => {
            let width = params
                .as_ref()
                .and_then(|p| p.get("width"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1920) as u32;
            let height = params
                .as_ref()
                .and_then(|p| p.get("height"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1080) as u32;
            let dsf = params
                .as_ref()
                .and_then(|p| p.get("deviceScaleFactor"))
                .and_then(|v| v.as_f64());
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::SetViewport {
                        target_id: tid,
                        width,
                        height,
                        device_scale_factor: dsf,
                    },
                )
            } else {
                ok_empty()
            }
        }
        "clearDeviceMetricsOverride" => ok_empty(),
        "setUserAgentOverride" => {
            let ua = params_str(params, "userAgent");
            // REQ-BRW-048 task #10: the override is a real navigator write
            // through the bridge — without a bridge it is a -32603, never a
            // silent no-op ok. Empty ua stays a tolerated no-op ack.
            if bridge.is_none() {
                return Err(CdpError {
                    code: -32603,
                    message: "no servo bridge connected".into(),
                });
            }
            if !ua.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::SetUserAgent {
                        target_id: tid,
                        user_agent: ua,
                    },
                )?;
            }
            ok_empty()
        }
        "setTouchEmulationEnabled" | "setScriptExecutionDisabled" => ok_empty(),
        "setFocusEmulationEnabled" | "setCPUThrottlingRate" => ok_empty(),
        "setDefaultBackgroundColorOverride" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Emulation.{}' wasn't found", command),
        }),
    }
}

fn handle_input(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "dispatchMouseEvent" => {
            let event_type = params_str(params, "type");
            let x = params
                .as_ref()
                .and_then(|p| p.get("x"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let y = params
                .as_ref()
                .and_then(|p| p.get("y"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            // CDP `button` is a string enum ("none"|"left"|"middle"|"right"|
            // "back"|"forward") — not a number.
            let button = params
                .as_ref()
                .and_then(|p| p.get("button"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let click_count = params
                .as_ref()
                .and_then(|p| p.get("clickCount"))
                .and_then(|v| v.as_i64());
            // REQ-BRW-048 task #10: input delivery without a bridge is a
            // -32603, never a silent no-op ok.
            bridge_send(
                bridge,
                BridgeCommand::DispatchMouseEvent {
                    target_id: tid.clone(),
                    event_type,
                    x,
                    y,
                    button,
                    click_count,
                },
            )
        }
        "dispatchKeyEvent" => {
            let event_type = params_str(params, "type");
            let key = params_str(params, "key");
            let code = params_str(params, "code");
            let text = params
                .as_ref()
                .and_then(|p| p.get("text"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // CDP `modifiers` bitmask: Alt=1, Ctrl=2, Meta/Command=4, Shift=8.
            let modifiers = params
                .as_ref()
                .and_then(|p| p.get("modifiers"))
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
                .unwrap_or(0);
            // CDP `location`: 0=default, 1=left, 2=right, 3=numpad.
            let location = params
                .as_ref()
                .and_then(|p| p.get("location"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            // CDP `autoRepeat`.
            let repeat = params
                .as_ref()
                .and_then(|p| p.get("autoRepeat"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // REQ-BRW-048 task #10: input delivery without a bridge is a
            // -32603, never a silent no-op ok.
            bridge_send(
                bridge,
                BridgeCommand::DispatchKeyEvent {
                    target_id: tid.clone(),
                    event_type,
                    key,
                    code,
                    text,
                    modifiers,
                    location,
                    repeat,
                },
            )
        }
        // REQ-BRW-048 task #10: no touch delivery path exists on this face —
        // explicit unsupported instead of a shape-only ok.
        "dispatchTouchEvent" => Err(not_supported(
            "Input.dispatchTouchEvent",
            "no touch delivery path exists on this face (the servo embedder bridge carries no touch command)",
        )),
        "insertText" => {
            let text = params_str(params, "text");
            // REQ-BRW-048 task #10: empty text is a Chrome-tolerated no-op
            // ack (nothing to deliver); anything else without a bridge is a
            // -32603 — never a silent no-op ok.
            if text.is_empty() {
                return ok_empty();
            }
            bridge_send(
                bridge,
                BridgeCommand::InsertText {
                    target_id: tid,
                    text,
                },
            )
        }
        "setIgnoreInputEvents" | "setInterceptDrags" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Input.{}' wasn't found", command),
        }),
    }
}

// REQ-CDP-007-C6: the Overlay domain's two state commands are real —
// highlightNode routes the node ref + highlight spec to the browser layer
// (screenshot/screencast annotation); hideHighlight clears that slot.
// setInspectMode is an explicit unsupported: the hover→
// Overlay.nodeHighlightRequested event chain does not exist on this face, so
// a canned ok would be a fake success (design §6 boundary matrix).
// @trace REQ-CDP-007 [criterion:C6]
fn handle_overlay(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        // Domain lifecycle — no side-effect face on this server.
        "enable" | "disable" => ok_empty(),
        // Node identity resolves through the same shared channel as
        // DOM.getBoxModel (nodeId / objectId / backendNodeId ≡ canonical
        // id); the highlight spec rides as raw JSON — its color semantics
        // live on the browser paint face.
        "highlightNode" => {
            let (node_id, object_id) = match require_overlay_node_ref(params)? {
                devtools_dom::NodeRef::NodeId(id) => (Some(id), None),
                devtools_dom::NodeRef::ObjectId(oid) => (None, Some(oid)),
            };
            let highlight_spec = params
                .as_ref()
                .and_then(|p| p.get("highlightSpec"))
                .cloned();
            bridge_send(
                bridge,
                BridgeCommand::OverlayHighlightNode {
                    target_id: tid,
                    node_id,
                    object_id,
                    highlight_spec,
                },
            )
        }
        "hideHighlight" => bridge_send(
            bridge,
            BridgeCommand::OverlayHideHighlight { target_id: tid },
        ),
        "setInspectMode" => Err(not_supported(
            "Overlay.setInspectMode",
            "inspect mode needs the hover→Overlay.nodeHighlightRequested event chain, which does not exist on this face",
        )),
        // Known relaxation (design §6): acknowledged without effect; a
        // phase-2 candidate for the annotation face (paused banner).
        "setPausedInDebuggerMessage" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Overlay.{}' wasn't found", command),
        }),
    }
}

/// Node-ref resolution for `Overlay.highlightNode`: the shared
/// `require_node_ref` shapes with `backendNodeId` folded in (backendNodeId ≡
/// canonical nodeId on this face — the DOM.resolveNode documented identity).
/// None of the three accepted shapes present → -32602.
fn require_overlay_node_ref(
    params: &Option<Value>,
) -> Result<devtools_dom::NodeRef, CdpError> {
    devtools_dom::require_node_ref(params).or_else(|_| {
        let backend = params
            .as_ref()
            .and_then(|p| p.get("backendNodeId"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CdpError {
                code: ERR_INVALID_PARAMS,
                message: "missing required parameter: nodeId, objectId or backendNodeId".into(),
            })?;
        Ok(devtools_dom::NodeRef::NodeId(backend))
    })
}

fn handle_debugger(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::DebuggerEnable { target_id: tid })
            } else {
                ok_empty()
            }
        }
        "disable" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::DebuggerDisable { target_id: tid })
            } else {
                ok_empty()
            }
        }
        "setBreakpointByUrl" => {
            let url = params
                .as_ref()
                .and_then(|p| p.get("url"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let url_regex = params
                .as_ref()
                .and_then(|p| p.get("urlRegex"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let line = params
                .as_ref()
                .and_then(|p| p.get("lineNumber"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let column = params
                .as_ref()
                .and_then(|p| p.get("columnNumber"))
                .and_then(|v| v.as_u64())
                .map(|c| c as u32);
            // REQ-BRW-048 task #10: a breakpoint write without a bridge is a
            // -32603 — the canned breakpointId "1" fabricated a success for
            // a breakpoint that can never be hit.
            bridge_send(
                bridge,
                BridgeCommand::DebuggerSetBreakpoint {
                    target_id: tid,
                    url,
                    url_regex,
                    line,
                    column,
                },
            )
        }
        "removeBreakpoint" => {
            let breakpoint_id = params
                .as_ref()
                .and_then(|p| p.get("breakpointId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DebuggerRemoveBreakpoint {
                        target_id: tid,
                        breakpoint_id,
                    },
                )
            } else {
                ok_empty()
            }
        }
        "pause" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::DebuggerInterrupt { target_id: tid })
            } else {
                ok_empty()
            }
        }
        "resume" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DebuggerResume {
                        target_id: tid,
                        step_type: None,
                    },
                )
            } else {
                ok_empty()
            }
        }
        "stepOver" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DebuggerResume {
                        target_id: tid,
                        step_type: Some("next".into()),
                    },
                )
            } else {
                ok_empty()
            }
        }
        "stepInto" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DebuggerResume {
                        target_id: tid,
                        step_type: Some("step".into()),
                    },
                )
            } else {
                ok_empty()
            }
        }
        "stepOut" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DebuggerResume {
                        target_id: tid,
                        step_type: Some("finish".into()),
                    },
                )
            } else {
                ok_empty()
            }
        }
        "setSkipAllPauses" | "setBreakpointsActive" => ok_empty(),
        "evaluateOnCallFrame" => {
            let expression = params
                .as_ref()
                .and_then(|p| p.get("expression"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let frame_actor_id = params
                .as_ref()
                .and_then(|p| p.get("callFrameId"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if bridge.is_some() && !expression.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::DebuggerEval {
                        target_id: tid,
                        expression,
                        frame_actor_id,
                    },
                )
            } else {
                Ok(serde_json::json!({ "result": { "type": "undefined" } }))
            }
        }
        "getPossibleBreakpoints" => {
            let start_script_id = params
                .as_ref()
                .and_then(|p| p.get("start"))
                .and_then(|v| v.get("scriptId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DebuggerGetPossibleBreakpoints {
                        target_id: tid,
                        start_script_id,
                    },
                )
            } else {
                Ok(serde_json::json!({ "locations": [] }))
            }
        }
        "getScriptSource" => {
            let script_id = params
                .as_ref()
                .and_then(|p| p.get("scriptId"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DebuggerGetScriptSource {
                        target_id: tid,
                        script_id,
                    },
                )
            } else {
                Ok(serde_json::json!({ "scriptSource": "" }))
            }
        }
        "setPauseOnExceptions" => ok_empty(),
        // SM-EVOLUTION #27 裁决 3: SpiderMonkey has no native blackbox face
        // (Debug.h exposes nothing) — explicit unsupported error, never a
        // canned ok that pretends sources were blackboxed.
        "blackbox" | "unblackbox" => Err(not_supported(
            &format!("Debugger.{command}"),
            "SpiderMonkey's Debugger API has no native blackbox; bao does not emulate one",
        )),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Debugger.{}' wasn't found", command),
        }),
    }
}

fn handle_log(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" | "clear" => ok_empty(),
        "startViolationsReport" | "stopViolationsReport" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Log.{}' wasn't found", command),
        }),
    }
}

// ─── REQ-CDP-006 C6: request interception command face (design §7) ──────
// enable / disable / continueRequest / failRequest / fulfillRequest are
// real: they carry the decision through the servo bridge to the per-target
// interception slot in bao_browser. Four commands stay explicit errors —
// the servo embedder request face genuinely lacks the backing facility
// (no auth challenge channel, no request body on WebResourceRequest, no
// response-body stream handles), and pretending otherwise would fabricate
// success. Error-code split: a command whose backing facility does not
// exist at all → -32000 not_supported; a real command whose params fall
// outside the supported envelope → -32602.

/// Upper bound for a `Fetch.fulfillRequest` body (design §7: explicit error
/// past the cap, never a silent truncation). Checked here on the base64
/// length (decoded ≈ len·3/4); the browser layer re-checks the exact
/// decoded size after bun_base64 decoding.
const FULFILL_BODY_MAX_DECODED_BYTES: usize = 8 * 1024 * 1024;
/// 4·⌈N/3⌉ encoded chars for [`FULFILL_BODY_MAX_DECODED_BYTES`].
const FULFILL_BODY_MAX_BASE64_CHARS: usize = (FULFILL_BODY_MAX_DECODED_BYTES / 3 + 1) * 4;

fn invalid_params(message: impl std::fmt::Display) -> CdpError {
    CdpError {
        code: ERR_INVALID_PARAMS,
        message: message.to_string(),
    }
}

/// Which command face a `patterns` array is parsed for — the two faces
/// share the pattern-object shape but differ in the stage key (design §5).
#[derive(Clone, Copy)]
enum PatternFace {
    /// `Fetch.enable`: reads `requestStage` ("Request" | "Response").
    Fetch,
    /// Legacy `Network.setRequestInterception`: the stage is pinned to
    /// Request (事件阶段恒 Request); a legacy `interceptionStage:
    /// "HeadersReceived"` is an explicit error — the servo pause point is
    /// request-time only.
    LegacyNetwork,
}

/// Parse the `patterns` array shared by `Fetch.enable` and the legacy
/// `Network.setRequestInterception` face into normalized
/// [`BridgeCommand`]-carrier patterns. Absent `patterns` = one catch-all
/// Request-stage pattern (Chromium omitted-patterns semantics); an explicit
/// empty array stays empty (matches nothing). Every malformed entry is an
/// explicit -32602 — never a silently dropped filter.
/// @trace REQ-CDP-006 [criterion:6]
fn parse_interception_patterns(
    params: &Option<Value>,
    face: PatternFace,
    label: &str,
) -> Result<Vec<crate::servo_bridge::FetchPattern>, CdpError> {
    use crate::servo_bridge::{FetchPattern, FetchRequestStage};
    // Absent or null `patterns` = one catch-all Request-stage pattern
    // (Chromium omitted-patterns semantics; null ≡ absent for optional
    // fields at this layer).
    let patterns_val = params
        .as_ref()
        .and_then(|p| p.get("patterns"))
        .filter(|v| !v.is_null());
    let Some(patterns_val) = patterns_val else {
        return Ok(vec![FetchPattern {
            url_pattern: None,
            resource_type: None,
            request_stage: FetchRequestStage::Request,
        }]);
    };
    let arr = patterns_val
        .as_array()
        .ok_or_else(|| invalid_params(format!("{label}: 'patterns' must be an array of pattern objects")))?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let obj = entry.as_object().ok_or_else(|| {
            invalid_params(format!(
                "{label}: each pattern must be an object with optional urlPattern / resourceType / requestStage"
            ))
        })?;
        let url_pattern = match obj.get("urlPattern") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(invalid_params(format!(
                    "{label}: 'urlPattern' must be a string glob"
                )))
            }
        };
        let resource_type = match obj.get("resourceType") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(_) => {
                return Err(invalid_params(format!(
                    "{label}: 'resourceType' must be a non-empty CDP ResourceType string"
                )))
            }
        };
        let request_stage = match face {
            PatternFace::Fetch => match obj.get("requestStage") {
                None | Some(Value::Null) => FetchRequestStage::Request,
                Some(Value::String(s)) if s == "Request" => FetchRequestStage::Request,
                Some(Value::String(s)) if s == "Response" => FetchRequestStage::Response,
                Some(_) => {
                    return Err(invalid_params(format!(
                        "{label}: 'requestStage' must be \"Request\" or \"Response\""
                    )))
                }
            },
            PatternFace::LegacyNetwork => match obj.get("interceptionStage") {
                None | Some(Value::Null) => FetchRequestStage::Request,
                Some(Value::String(s)) if s == "Request" => FetchRequestStage::Request,
                Some(_) => {
                    return Err(invalid_params(format!(
                        "{label}: interception stage must be \"Request\" — the servo pause point is request-time only, there is no headers-received pause"
                    )))
                }
            },
        };
        out.push(FetchPattern {
            url_pattern,
            resource_type,
            request_stage,
        });
    }
    Ok(out)
}

/// `interceptionId` is the mandatory resolution key for every
/// pause-resolution command — minted by the interception facility at pause
/// time and carried on `Fetch.requestPaused` / `Network.requestIntercepted`.
fn interception_id(params: &Option<Value>, label: &str) -> Result<String, CdpError> {
    match params.as_ref().and_then(|p| p.get("interceptionId")) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(_) => Err(invalid_params(format!(
            "{label}: 'interceptionId' must be a non-empty string"
        ))),
        None => Err(invalid_params(format!(
            "{label}: missing required 'interceptionId' (the resolution key carried on Fetch.requestPaused / Network.requestIntercepted)"
        ))),
    }
}

/// Explicit -32602 for `Fetch.continueRequest` /
/// `Network.continueInterceptedRequest` modification fields: the servo
/// embedder request face can only let a paused request through or answer it
/// with a synthesized response — it cannot re-issue a reconstructed request
/// (design §7 capability boundary; never silently ignored params).
fn reject_request_modification(command: &str, params: &Option<Value>) -> Result<(), CdpError> {
    for field in ["url", "method", "postData", "headers"] {
        if params.as_ref().is_some_and(|p| p.get(field).is_some()) {
            return Err(invalid_params(format!(
                "{command}: '{field}' request modification is not supported — the servo embedder request face can only let the paused request through or answer it with a synthesized response; it cannot re-issue a reconstructed request (design §7 boundary)"
            )));
        }
    }
    Ok(())
}

/// REQ-CDP-006 C6: the Fetch interception command face. Real commands route
/// through the servo bridge to the browser-side interception slot (design
/// §5); capability-absent commands are explicit -32000 errors with the
/// design §7 boundary declaration in the message.
/// @trace REQ-CDP-006 [criterion:6]
fn handle_fetch(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    use crate::servo_bridge::FetchResolveAction;
    let tid = target_id.to_string();
    match command {
        // Real: arms the per-target interception slot (browser-side). Without
        // a bridge there is no slot owner — -32603, never a canned "enabled".
        "enable" => {
            if params.as_ref().and_then(|p| p.get("handleAuthRequests"))
                == Some(&Value::Bool(true))
            {
                return Err(invalid_params(
                    "Fetch.enable: handleAuthRequests=true is not supported — the auth interception facet is not wired (no Fetch.authRequired event is ever emitted and Fetch.continueWithAuth is an explicit error; design §7)",
                ));
            }
            let patterns = parse_interception_patterns(params, PatternFace::Fetch, "Fetch.enable")?;
            bridge_send(
                bridge,
                BridgeCommand::FetchEnable {
                    target_id: tid,
                    patterns,
                    legacy_network_events: false,
                },
            )
        }
        // Real: clears the slot and drains every pending pause (each held
        // load drops = the request proceeds; design §3.1 disable-as-exit).
        "disable" => bridge_send(bridge, BridgeCommand::FetchDisable { target_id: tid }),
        // Real: drop the held load → servo answers DoNotIntercept → the
        // original request proceeds into the net stack (design §3.2).
        "continueRequest" => {
            let interception_id = interception_id(params, "Fetch.continueRequest")?;
            reject_request_modification("Fetch.continueRequest", params)?;
            bridge_send(
                bridge,
                BridgeCommand::FetchResolve {
                    target_id: tid,
                    interception_id,
                    action: FetchResolveAction::Continue,
                },
            )
        }
        // Real: intercept + cancel → net-side network error. errorReason is
        // carried advisory-only (the servo face has no error-code channel;
        // every fail is a plain network error — design §7).
        "failRequest" => {
            let interception_id = interception_id(params, "Fetch.failRequest")?;
            let error_reason = match params.as_ref().and_then(|p| p.get("errorReason")) {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => {
                    return Err(invalid_params(
                        "Fetch.failRequest: 'errorReason' must be a CDP ErrorReason string",
                    ))
                }
            };
            bridge_send(
                bridge,
                BridgeCommand::FetchResolve {
                    target_id: tid,
                    interception_id,
                    action: FetchResolveAction::Fail { error_reason },
                },
            )
        }
        // Real: synthesize the response (status / headers / base64 body).
        "fulfillRequest" => {
            let interception_id = interception_id(params, "Fetch.fulfillRequest")?;
            let response_code = match params.as_ref().and_then(|p| p.get("responseCode")) {
                Some(Value::Number(n)) => match n.as_i64() {
                    Some(c) => c,
                    // Out-of-i64-range numbers are rejected, never
                    // silently clamped into a fabricated status.
                    None => {
                        return Err(invalid_params(
                            "Fetch.fulfillRequest: 'responseCode' must be an integer HTTP status",
                        ))
                    }
                },
                Some(_) => {
                    return Err(invalid_params(
                        "Fetch.fulfillRequest: 'responseCode' must be an integer HTTP status",
                    ))
                }
                None => {
                    return Err(invalid_params(
                        "Fetch.fulfillRequest: missing required 'responseCode'",
                    ))
                }
            };
            let status_message = match params.as_ref().and_then(|p| p.get("responsePhrase")) {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => {
                    return Err(invalid_params(
                        "Fetch.fulfillRequest: 'responsePhrase' must be a string",
                    ))
                }
            };
            let response_headers = match params.as_ref().and_then(|p| p.get("responseHeaders")) {
                None | Some(Value::Null) => None,
                Some(h @ Value::Object(_)) => {
                    if h.as_object().is_some_and(|o| o.values().all(Value::is_string)) {
                        Some(h.clone())
                    } else {
                        return Err(invalid_params(
                            "Fetch.fulfillRequest: 'responseHeaders' values must be strings",
                        ));
                    }
                }
                Some(_) => {
                    return Err(invalid_params(
                        "Fetch.fulfillRequest: 'responseHeaders' must be an object of {name: value}",
                    ))
                }
            };
            let body = match params.as_ref().and_then(|p| p.get("body")) {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => {
                    if s.len() > FULFILL_BODY_MAX_BASE64_CHARS {
                        return Err(invalid_params(format!(
                            "Fetch.fulfillRequest: body exceeds the 8 MiB cap (base64 length {} > {}); the browser layer re-checks the decoded size (design §7)",
                            s.len(),
                            FULFILL_BODY_MAX_BASE64_CHARS
                        )));
                    }
                    Some(s.clone())
                }
                Some(_) => {
                    return Err(invalid_params(
                        "Fetch.fulfillRequest: 'body' must be a base64 string",
                    ))
                }
            };
            bridge_send(
                bridge,
                BridgeCommand::FetchResolve {
                    target_id: tid,
                    interception_id,
                    action: FetchResolveAction::Fulfill {
                        response_code,
                        status_message,
                        response_headers,
                        body,
                    },
                },
            )
        }
        // ─── Capability-absent faces: explicit errors, never fake success ───
        "getRequestPostData" => Err(not_supported(
            "Fetch.getRequestPostData",
            "the servo embedder request face carries no request body (WebResourceRequest has method/url/headers only) — a vendor body carrier is phase-2 (design §7)",
        )),
        "continueWithAuth" => Err(not_supported(
            "Fetch.continueWithAuth",
            "the auth interception facet is not wired: Fetch.enable with handleAuthRequests=true is rejected the same way and no auth challenge is ever paused (design §7)",
        )),
        "takeResponseBodyAsStream" => Err(not_supported(
            "Fetch.takeResponseBodyAsStream",
            "stream handles are not part of the P1 interception face — a fulfillRequest client already holds the body it sent; capturing the network response body of a paused request is phase-2 (design §7)",
        )),
        "continueWithResponse" => Err(not_supported(
            "Fetch.continueWithResponse",
            "phase-2 (a renamed fulfillRequest) — use Fetch.fulfillRequest, which is the real synthesized-response path (design §7)",
        )),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Fetch.{}' wasn't found", command),
        }),
    }
}

fn handle_profiler(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" | "disable" => ok_empty(),
        // The bridge handler reports the real support status — the mozjs FFI
        // surface exposes no SpiderMonkey sampling profiler, so these resolve
        // to explicit errors rather than an empty-profile fake success.
        "start" => bridge_send(bridge, BridgeCommand::ProfilerStart { target_id: tid }),
        "stop" => bridge_send(bridge, BridgeCommand::ProfilerStop { target_id: tid }),
        "setSamplingInterval" => {
            let interval = params
                .as_ref()
                .and_then(|p| p.get("interval"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1000) as u32;
            bridge_send(
                bridge,
                BridgeCommand::ProfilerSetSamplingInterval {
                    target_id: tid,
                    interval,
                },
            )
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Profiler.{}' wasn't found", command),
        }),
    }
}

fn handle_heap_profiler(
    command: &str,
    target_id: &str,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" | "disable" => ok_empty(),
        // Snapshot/tracking resolve to explicit errors at the bridge handler
        // (no heap-snapshot API in the mozjs FFI surface); collectGarbage is
        // REAL — it drives servo's GarbageCollectAllContexts → JS_GC.
        "takeHeapSnapshot" => {
            bridge_send(
                bridge,
                BridgeCommand::HeapProfilerTakeSnapshot { target_id: tid },
            )
        }
        "startTrackingHeapObjects" => {
            bridge_send(
                bridge,
                BridgeCommand::HeapProfilerStartTracking { target_id: tid },
            )
        }
        "stopTrackingHeapObjects" => {
            bridge_send(
                bridge,
                BridgeCommand::HeapProfilerStopTracking { target_id: tid },
            )
        }
        "collectGarbage" => {
            bridge_send(
                bridge,
                BridgeCommand::HeapProfilerCollectGarbage { target_id: tid },
            )
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'HeapProfiler.{}' wasn't found", command),
        }),
    }
}

fn handle_memory(command: &str, target_id: &str, bridge: Option<&BridgeSender>) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        // getDOMCounters resolves to an explicit error at the bridge handler
        // (jsEventListeners is not introspectable in SpiderMonkey) — never a
        // zeroed counters object. forciblyPurgeJavaScriptMemory is REAL — it
        // drives servo's GarbageCollectAllContexts → JS_GC.
        "getDOMCounters" => {
            bridge_send(
                bridge,
                BridgeCommand::MemoryGetDOMCounters { target_id: tid },
            )
        }
        "prepareForLeakDetection" => ok_empty(),
        "forciblyPurgeJavaScriptMemory" => {
            bridge_send(bridge, BridgeCommand::MemoryPurgeJS { target_id: tid })
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Memory.{}' wasn't found", command),
        }),
    }
}

fn handle_performance(
    command: &str,
    target_id: &str,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" | "disable" => ok_empty(),
        // Real metrics (Timestamp/Documents/Frames/Nodes) computed from the
        // live document at the bridge handler — never an empty canned list.
        "getMetrics" => {
            bridge_send(
                bridge,
                BridgeCommand::PerformanceGetMetrics { target_id: tid },
            )
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Performance.{}' wasn't found", command),
        }),
    }
}

fn handle_system_info(command: &str) -> HandlerResult {
    match command {
        "getInfo" => {
            let os_name = std::env::consts::OS;
            let arch = std::env::consts::ARCH;
            let pid = std::process::id();
            Ok(serde_json::json!({
                "gpu": {
                    "vendorString": "Bao",
                    "deviceString": "Software Renderer"
                },
                "modelName": "Bao",
                "modelVersion": env!("CARGO_PKG_VERSION"),
                "commandLine": "",
                "platform": os_name,
                "product": "Bao",
                "cpu": {
                    "arch": arch,
                    "processors": num_cpus()
                },
                "osName": os_name,
                "osVersion": "",
                "pid": pid
            }))
        }
        "getProcessInfo" => {
            let pid = std::process::id();
            Ok(serde_json::json!({
                "processInfo": [{
                    "id": pid,
                    "type": "browser",
                    "name": "Bao"
                }]
            }))
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'SystemInfo.{}' wasn't found", command),
        }),
    }
}

/// Return the number of logical CPUs available.
/// Uses `std::thread::available_parallelism` (stable since Rust 1.59).
fn num_cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

// REQ-BRW-004: ServiceWorker CDP observability domain.
// @trace REQ-BRW-004 [criterion:19]
//
// SPEC criterion #19: "CDP Network 域可观测 SW 发起的请求/响应; SW 持久生命周期
//   (跨页存活)下 profile 继承注册页且 terminate 后正确注销"
//
// This handler routes ServiceWorker lifecycle + fetch-interception queries to the
// servo bridge. The actual ServiceWorker DOM binding lives in servo; bao tracks
// per-delegate registration state (ServiceWorkerRegistrationTracking /
// ServiceWorkerHandle in bao_browser/src/delegate.rs) and exposes it to CDP.
//
// CDP Network domain observability of SW-initiated requests is provided by the
// existing Network.* handlers — SW-intercepted fetches flow through the same
// Network.requestWillBeSent / responseReceived event stream as page fetches
// (per SPEC criterion #19: "SW 拦截并转发的 fetch 仍走主页同一 stealth ... profile").
/// Browser domain — metadata commands on the browser endpoint.
///
/// `Browser.getVersion` is the first command Playwright's connect_over_cdp
/// sends after the WebSocket opens; the values are the real runtime's.
fn handle_browser(command: &str, params: &Option<Value>) -> HandlerResult {
    match command {
        "getVersion" => Ok(serde_json::json!({
            "protocolVersion": "1.3",
            "product": "Bao/0.1.0",
            "revision": env!("CARGO_PKG_VERSION"),
            "userAgent": format!("Bao/{}", env!("CARGO_PKG_VERSION")),
            "jsVersion": "SpiderMonkey",
        })),
        // REQ-CDP-010 golden-oracle parity (Chrome 155 oracle, measured): the
        // binding rejects a MISSING `behavior` (-32602 mandatory field), and
        // behavior allow/allowAndName require `downloadPath` (-32602
        // "downloadPath not provided"); any other behavior value — unknown
        // enums, empty string included — is accepted as a no-op success.
        // bao has no download manager yet, so every accepted call records
        // nothing: the shape mirrors Chrome exactly, malformed clients still
        // fail with -32602.
        "setDownloadBehavior" => {
            let behavior = params
                .as_ref()
                .and_then(|p| p.get("behavior"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| CdpError {
                    code: ERR_INVALID_PARAMS,
                    message: "'Browser.setDownloadBehavior' requires a behavior param".into(),
                })?;
            if matches!(behavior, "allow" | "allowAndName") {
                let has_path = params
                    .as_ref()
                    .and_then(|p| p.get("downloadPath"))
                    .and_then(|v| v.as_str())
                    .is_some();
                if !has_path {
                    return Err(CdpError {
                        code: ERR_INVALID_PARAMS,
                        message: "'Browser.setDownloadBehavior' with behavior \"allow\" requires a downloadPath param".into(),
                    });
                }
            }
            ok_empty()
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Browser.{}' wasn't found", command),
        }),
    }
}

fn handle_service_worker(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" | "disable" => ok_empty(),
        "deliverPushMessage" => {
            let origin = params_str(params, "origin");
            let registration_id = params_str(params, "registrationId");
            Ok(serde_json::json!({
                "origin": origin,
                "registrationId": registration_id,
                "delivered": true
            }))
        }
        "dispatchPeriodicSyncEvent" | "dispatchSyncEvent" => ok_empty(),
        // List all ServiceWorker registrations tracked by bao_browser.
        // Maps to BridgeCommand::ListServiceWorkerRegistrations which queries
        // ServiceWorkerRegistrationTracking entries per-delegate.
        "getAllRegistrations" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::ListServiceWorkerRegistrations { target_id: tid },
                )
            } else {
                Ok(serde_json::json!({ "registrations": [] }))
            }
        }
        // Get detailed info for a specific registration.
        "getRegistration" => {
            let registration_id = params_str(params, "registrationId");
            if bridge.is_some() && !registration_id.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::GetServiceWorkerRegistrationInfo {
                        target_id: tid,
                        registration_id,
                    },
                )
            } else {
                Ok(serde_json::json!({ "registration": null }))
            }
        }
        // Terminate a ServiceWorker (terminate flag + disable fetch interception).
        // Per SPEC criterion #19: "terminate 后正确注销"
        "stopWorker" => {
            let registration_id = params_str(params, "registrationId");
            if bridge.is_some() && !registration_id.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::StopServiceWorker {
                        target_id: tid,
                        registration_id,
                    },
                )?;
            }
            ok_empty()
        }
        "unregister" => {
            let registration_id = params_str(params, "registrationId");
            if bridge.is_some() && !registration_id.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::TerminateServiceWorker {
                        target_id: tid,
                        registration_id,
                    },
                )?;
            }
            ok_empty()
        }
        "updateRegistration" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'ServiceWorker.{}' wasn't found", command),
        }),
    }
}

// @trace TEST-CDP-001 [req:REQ-CDP-001] [level:unit] [nfr:TMG-CDP-01]
#[cfg(test)]
mod tests {
    use super::*;
    // CdpEvent lives in the crate-root re-export (lib.rs) since the protocol
    // module's re-export was deduplicated; the tests below construct it.
    use crate::CdpEvent;
    use serde_json::json;

    /// REQ-BRW-048 fail-closed contract shared by the CSS style-query faces:
    /// no bridge + missing required nodeId → explicit -32602 naming the param.
    fn assert_css_style_query_requires_nodeid(id: i64, method: &str) {
        let resp = dispatch_no_bridge(id, method, None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("nodeId"));
    }

    /// Sessionless CdpMessage fixture — the construction shape every
    /// handle_command test below shares (params passthrough, no session tag).
    fn cdp_msg(id: i64, method: &str, params: Option<Value>) -> CdpMessage {
        CdpMessage {
            id: Some(id),
            method: method.into(),
            params,
            session_id: None,
        }
    }

    /// Standard handle_command test shape: build a sessionless message and
    /// dispatch it against fixture target "t1" with no servo bridge.
    fn dispatch_no_bridge(id: i64, method: &str, params: Option<Value>) -> CdpResponse {
        let msg = cdp_msg(id, method, params);
        let params = msg.params.clone();
        handle_command(msg, "t1", &params, None)
}

    // 1. parse_message valid JSON → Some(CdpMessage) with correct id/method/params
    #[test]
    fn parse_message_valid_json() {
        let msg = parse_message(r#"{"id":1,"method":"Page.enable","params":{"url":"http://x"}}"#)
            .unwrap();
        assert_eq!(msg.id, Some(1));
        assert_eq!(msg.method, "Page.enable");
        assert_eq!(msg.params, Some(json!({"url": "http://x"})));
        assert_eq!(msg.session_id, None);
    }

    // 2. parse_message invalid JSON → None
    #[test]
    fn parse_message_invalid_json() {
        assert!(parse_message("{not json}").is_none());
    }

    // 3. parse_message missing method → None
    #[test]
    fn parse_message_missing_method() {
        assert!(parse_message(r#"{"id":1}"#).is_none());
    }

    // 4. parse_message with session_id (serde snake_case default)
    #[test]
    fn parse_message_with_session_id() {
        let raw = r#"{"id":5,"method":"Runtime.evaluate","sessionId":"abc123"}"#;
        let msg = parse_message(raw).expect("should parse valid JSON with session_id");
        assert_eq!(msg.id, Some(5));
        assert_eq!(msg.method, "Runtime.evaluate");
        assert_eq!(msg.session_id, Some("abc123".to_string()));
    }

    // 5. parse_message with null params
    #[test]
    fn parse_message_null_params() {
        let msg = parse_message(r#"{"id":2,"method":"Page.enable","params":null}"#).unwrap();
        assert_eq!(msg.id, Some(2));
        assert_eq!(msg.params, None);
    }

    // 6. serialize_response with result
    #[test]
    fn serialize_response_with_result() {
        let resp = CdpResponse {
            id: Some(1),
            result: Some(json!({"key": "val"})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"]["key"], "val");
        assert!(parsed.get("error").is_none());
    }

    // 7. serialize_response with error
    #[test]
    fn serialize_response_with_error() {
        let resp = CdpResponse {
            id: Some(2),
            result: None,
            error: Some(CdpError {
                code: -32601,
                message: "not found".into(),
            }),
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], 2);
        assert!(parsed.get("result").is_none());
        assert_eq!(parsed["error"]["code"], -32601);
        assert_eq!(parsed["error"]["message"], "not found");
    }

    // 8. handle_command with unknown domain → error code -32601
    #[test]
    fn handle_command_unknown_domain() {
        let resp = dispatch_no_bridge(1, "Foo.bar", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    // 9. handle_command Target.getTargets (no bridge) → ok with targetInfos
    #[test]
    fn handle_command_target_get_targets() {
        let resp = dispatch_no_bridge(2, "Target.getTargets", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("targetInfos").unwrap().as_array().unwrap().len() > 0);
        assert_eq!(result["targetInfos"][0]["targetId"], "t1");
    }

    // ─── REQ-CDS-009: browser context command face ─────────────────────

    /// Scripted bridge responder serving the context-management commands and
    /// the context-aware create routing — the same live-channel shape the
    /// ws_registry tests use.
    fn context_responder(
        rx: crate::servo_bridge::BridgeReceiver,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || loop {
            let handled = rx.try_process(|cmd| {
                crate::servo_bridge::BridgeResponse {
                    result: match cmd {
                        BridgeCommand::CreateBrowserContext => {
                            Ok(json!({"browserContextId": "bao-context-1"}))
                        }
                        BridgeCommand::ListBrowserContexts => {
                            Ok(json!({"browserContextIds": ["bao-context-1"]}))
                        }
                        BridgeCommand::DisposeBrowserContext { browser_context_id } => {
                            if browser_context_id == "bao-context-1" {
                                Ok(json!({}))
                            } else {
                                Err(crate::servo_bridge::BridgeError::domain(format!(
                                    "Failed to find browser context: {browser_context_id}"
                                )))
                            }
                        }
                        BridgeCommand::CreateTargetInContext { browser_context_id, .. } => {
                            if browser_context_id == "bao-context-1" {
                                Ok(json!({
                                    "targetId": "7",
                                    "browserContextId": browser_context_id
                                }))
                            } else {
                                Err(crate::servo_bridge::BridgeError::domain(format!(
                                    "Failed to find browser context: {browser_context_id}"
                                )))
                            }
                        }
                        BridgeCommand::GetTargetContextId { target_id } => {
                            Ok(json!({"browserContextId": format!("ctx-of-{target_id}")}))
                        }
                        BridgeCommand::GetTitle { .. } => Ok(json!("T")),
                        BridgeCommand::GetUrl { .. } => Ok(json!("https://example.com")),
                        BridgeCommand::ListTargets => Ok(json!([
                            {"id": "1", "title": "P", "url": "about:blank",
                             "browserContextId": "bao-context-1"}
                        ])),
                        _ => Ok(json!({})),
                    },
                }
            });
            if !handled {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    }

    fn dispatch_with_bridge(
        id: i64,
        method: &str,
        params: Option<Value>,
        bridge: &crate::servo_bridge::BridgeSender,
    ) -> CdpResponse {
        let msg = cdp_msg(id, method, params);
        let params = msg.params.clone();
        handle_command(msg, "t1", &params, Some(bridge))
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:1] [level:unit]
    #[test]
    fn create_browser_context_returns_bridge_minted_id() {
        let (tx, rx) = crate::servo_bridge::bridge_channel(std::time::Duration::from_secs(2));
        let _keeper = context_responder(rx);
        let resp = dispatch_with_bridge(1, "Target.createBrowserContext", None, &tx);
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let id = resp.result.unwrap()["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(id, "bao-context-1");
        assert_ne!(id, DEFAULT_BROWSER_CONTEXT_ID);
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:2] [level:unit]
    #[test]
    fn dispose_browser_context_unknown_id_surfaces_domain_error() {
        let (tx, rx) = crate::servo_bridge::bridge_channel(std::time::Duration::from_secs(2));
        let _keeper = context_responder(rx);
        let resp = dispatch_with_bridge(
            1,
            "Target.disposeBrowserContext",
            Some(json!({"browserContextId": "nope"})),
            &tx,
        );
        let err = resp.error.expect("unknown context must fail");
        assert_eq!(err.code, ERR_NOT_SUPPORTED);
        assert!(err.message.contains("Failed to find browser context"));
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:3] [level:unit]
    #[test]
    fn get_browser_contexts_returns_registry_listing() {
        let (tx, rx) = crate::servo_bridge::bridge_channel(std::time::Duration::from_secs(2));
        let _keeper = context_responder(rx);
        let resp = dispatch_with_bridge(1, "Target.getBrowserContexts", None, &tx);
        assert!(resp.error.is_none());
        let ids = resp.result.unwrap()["browserContextIds"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(ids, vec![json!("bao-context-1")]);
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:4] [level:unit]
    #[test]
    fn create_target_with_context_routes_context_aware_creation() {
        let (tx, rx) = crate::servo_bridge::bridge_channel(std::time::Duration::from_secs(2));
        let _keeper = context_responder(rx);
        let resp = dispatch_with_bridge(
            1,
            "Target.createTarget",
            Some(json!({"url": "about:blank", "browserContextId": "bao-context-1"})),
            &tx,
        );
        assert!(resp.error.is_none());
        let r = resp.result.unwrap();
        assert_eq!(r["targetId"], "7");
        assert_eq!(r["browserContextId"], "bao-context-1");
        // Unknown context fails closed with Chrome's server-error class.
        let err = dispatch_with_bridge(
            2,
            "Target.createTarget",
            Some(json!({"url": "about:blank", "browserContextId": "nope"})),
            &tx,
        )
        .error
        .expect("unknown context must fail");
        assert_eq!(err.code, ERR_NOT_SUPPORTED);
        assert!(err.message.contains("Failed to find browser context"));
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:5] [level:unit]
    #[test]
    fn get_targets_attribution_comes_from_listing_entries() {
        let (tx, rx) = crate::servo_bridge::bridge_channel(std::time::Duration::from_secs(2));
        let _keeper = context_responder(rx);
        let resp = dispatch_with_bridge(1, "Target.getTargets", None, &tx);
        assert!(resp.error.is_none());
        let infos = resp.result.unwrap()["targetInfos"].as_array().unwrap().clone();
        let page = infos
            .iter()
            .find(|i| i["type"] == "page")
            .expect("page entry present");
        assert_eq!(page["browserContextId"], "bao-context-1");
    }

    // ─── handle_command table-driven protocol matrix ──────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]
    //
    // Convergence of the per-command protocol-coverage suites whose
    // members differ only in (dispatch id, method, params) — one table
    // per assertion shape (α ok+empty-result / β ok-only / γ
    // unknown-command→-32601 / δ explicit-error). Row comments preserve
    // the original inventory numbering and dispatch ids of the pre-table
    // tests; assertion parity per row is identical to the replaced tests
    // (α: 2 asserts, β: 1, γ: 2, δ1: 3, δ2: 2, δ3: 1).

    /// α form: command succeeds and the result is exactly `{}`.
    #[test]
    fn handle_command_ok_empty_result_matrix() {
        let cases: Vec<(i64, &str, Option<Value>)> = vec![
            // 12. Target.setAutoAttach → ok empty
            (5, "Target.setAutoAttach", None),
            // 13. Page.enable → ok empty
            (6, "Page.enable", None),
            // 15. Runtime.enable → ok empty (Chrome semantics: no
            //     executionContextId in the response)
            (8, "Runtime.enable", None),
            // 19. Network.enable → ok empty
            (12, "Network.enable", None),
            // 21. CSS.enable → ok empty
            (14, "CSS.enable", None),
            // 23. Emulation.setDeviceMetricsOverride (no bridge) → ok empty
            (
                16,
                "Emulation.setDeviceMetricsOverride",
                Some(json!({"width": 800, "height": 600, "deviceScaleFactor": 2})),
            ),
            // 24. Input.dispatchMouseEvent left this matrix (task #10):
            //     input delivery without a bridge is -32603.
            // 25. Overlay.enable → ok empty
            (18, "Overlay.enable", None),
            // 26. Debugger.enable → ok empty
            (19, "Debugger.enable", None),
            // 28. Log.enable → ok empty
            (21, "Log.enable", None),
            // 78. Target.setDiscoverTargets → ok empty
            (7, "Target.setDiscoverTargets", None),
            // 188. ServiceWorker.enable → ok empty  @trace REQ-BRW-004 [criterion:19]
            (1, "ServiceWorker.enable", None),
            // 189. ServiceWorker.disable → ok empty  @trace REQ-BRW-004 [criterion:19]
            (2, "ServiceWorker.disable", None),
        ];
        for (id, method, params) in cases {
            let resp = dispatch_no_bridge(id, method, params);
            assert!(resp.error.is_none(), "{method} (id {id}): unexpected error");
            assert_eq!(
                resp.result.unwrap(),
                json!({}),
                "{method} (id {id}): expected empty result"
            );
        }
    }

    /// β form: command succeeds (the replaced suites asserted
    /// error-none only, without pinning the result payload).
    #[test]
    fn handle_command_ok_only_matrix() {
        let cases: Vec<(i64, &str, Option<Value>)> = vec![
            // 91. Page.disable → ok empty
            (20, "Page.disable", None),
            // 92. Runtime.disable → ok empty
            (21, "Runtime.disable", None),
            // 95. Runtime.evaluateAsync → ok
            (24, "Runtime.evaluateAsync", None),
            // 96. Runtime.runScript → ok
            (25, "Runtime.runScript", None),
            // 97. Runtime.releaseObject → ok empty
            (26, "Runtime.releaseObject", None),
            // 98. Runtime.releaseObjectGroup → ok empty
            (27, "Runtime.releaseObjectGroup", None),
            // 99. Runtime.compileScript → ok empty
            (28, "Runtime.compileScript", None),
            // 101. DOM.enable → ok empty
            (30, "DOM.enable", None),
            // 102. DOM.disable → ok empty
            (31, "DOM.disable", None),
            // 105-109. DOM write faces left this matrix (task #10):
            //     setAttributeValue without a bridge is -32603; the four
            //     undeliverable mutations are -32000 on both faces.
            // 114. Network.disable → ok empty
            (43, "Network.disable", None),
            // 116. Network.setCacheDisabled → ok empty
            (45, "Network.setCacheDisabled", None),
            // 118. Network.emulateNetworkConditions → ok empty
            (47, "Network.emulateNetworkConditions", None),
            // 120. Network.deleteCookies → ok empty
            (49, "Network.deleteCookies", None),
            // 122/123. Network.setRequestInterception /
            //     continueInterceptedRequest left this matrix (REQ-CDP-006
            //     C6): both are real bridge commands now — setRequestInterception
            //     without a bridge is -32603, continueInterceptedRequest
            //     without an interceptionId is -32602 (see the fetch_
            //     interception_ tests below).
            // 125. CSS.disable → ok empty
            (54, "CSS.disable", None),
            // 130. Emulation.clearDeviceMetricsOverride → ok empty
            (59, "Emulation.clearDeviceMetricsOverride", None),
            // 131. Emulation.setUserAgentOverride left this matrix (task
            //     #10): the override is a real bridge write — -32603.
            // 132. Emulation.setTouchEmulationEnabled → ok empty
            (61, "Emulation.setTouchEmulationEnabled", None),
            // 133. Emulation.setScriptExecutionDisabled → ok empty
            (62, "Emulation.setScriptExecutionDisabled", None),
            // 134. Emulation.setFocusEmulationEnabled → ok empty
            (63, "Emulation.setFocusEmulationEnabled", None),
            // 135. Emulation.setCPUThrottlingRate → ok empty
            (64, "Emulation.setCPUThrottlingRate", None),
            // 136. Emulation.setDefaultBackgroundColorOverride → ok empty
            (65, "Emulation.setDefaultBackgroundColorOverride", None),
            // 138. Input.dispatchKeyEvent left this matrix (task #10):
            //     input delivery without a bridge is -32603.
            // 139. Input.dispatchTouchEvent left this matrix (task #10):
            //     no touch delivery path exists → -32000 on both faces.
            // 140. Input.insertText with EMPTY text stays an ack (nothing
            //     to deliver); non-empty without a bridge is -32603.
            (69, "Input.insertText", Some(json!({"text": ""}))),
            // 141. Input.setIgnoreInputEvents → ok empty
            (70, "Input.setIgnoreInputEvents", None),
            // 142. Input.setInterceptDrags → ok empty
            (71, "Input.setInterceptDrags", None),
            // 144-146. Overlay.highlightNode / hideHighlight / setInspectMode
            //     left this matrix (REQ-CDP-007-C6): the first two are real
            //     bridge commands (-32603 without a bridge / -32602 without a
            //     node ref); setInspectMode is -32000 (no hover→event chain).
            // 147. Overlay.setPausedInDebuggerMessage → ok empty
            (76, "Overlay.setPausedInDebuggerMessage", None),
            // 149. Debugger.disable → ok empty
            (78, "Debugger.disable", None),
            // 150. Debugger.removeBreakpoint → ok empty
            (79, "Debugger.removeBreakpoint", None),
            // 151. Debugger.pause → ok empty
            (80, "Debugger.pause", None),
            // 152. Debugger.resume → ok empty
            (81, "Debugger.resume", None),
            // 153. Debugger.stepOver → ok empty
            (82, "Debugger.stepOver", None),
            // 154. Debugger.stepInto → ok empty
            (83, "Debugger.stepInto", None),
            // 155. Debugger.stepOut → ok empty
            (84, "Debugger.stepOut", None),
            // 156. Debugger.setSkipAllPauses → ok empty
            (85, "Debugger.setSkipAllPauses", None),
            // 157. Debugger.setBreakpointsActive → ok empty
            (86, "Debugger.setBreakpointsActive", None),
            // 161. Debugger.setPauseOnExceptions → ok empty
            (90, "Debugger.setPauseOnExceptions", None),
            // 163. Log.disable → ok empty
            (92, "Log.disable", None),
            // 164. Log.clear → ok empty
            (93, "Log.clear", None),
            // 165. Log.startViolationsReport → ok empty
            (94, "Log.startViolationsReport", None),
            // 166. Log.stopViolationsReport → ok empty
            (95, "Log.stopViolationsReport", None),
            // 168. Fetch.disable left this matrix (REQ-CDP-006 C6): a real
            //     bridge command now — without a bridge it is -32603 (the
            //     slot + pending drains live browser-side).
            // 192. ServiceWorker.stopWorker (no bridge) → ok empty  @trace REQ-BRW-004 [criterion:19]
            (
                5,
                "ServiceWorker.stopWorker",
                Some(json!({"registrationId": "sw-reg-1"})),
            ),
            // 193. ServiceWorker.unregister (no bridge) → ok empty  @trace REQ-BRW-004 [criterion:19]
            (
                6,
                "ServiceWorker.unregister",
                Some(json!({"registrationId": "sw-reg-1"})),
            ),
            // 195. ServiceWorker.dispatchPeriodicSyncEvent → ok empty  @trace REQ-BRW-004 [criterion:19]
            (8, "ServiceWorker.dispatchPeriodicSyncEvent", None),
        ];
        for (id, method, params) in cases {
            let resp = dispatch_no_bridge(id, method, params);
            assert!(resp.error.is_none(), "{method} (id {id}): unexpected error");
        }
    }

    /// γ form: known domain, unknown command → error -32601.
    #[test]
    fn handle_command_unknown_command_matrix() {
        let cases: &[(i64, &str)] = &[
            // 113. DOM.unknown → error -32601
            (42, "DOM.nonExistent"),
            // 124. Network.unknown → error -32601
            (53, "Network.bogus"),
            // 129. CSS.unknown → error -32601
            (58, "CSS.bogus"),
            // 137. Emulation.unknown → error -32601
            (66, "Emulation.bogus"),
            // 143. Input.unknown → error -32601
            (72, "Input.bogus"),
            // 148. Overlay.unknown → error -32601
            (77, "Overlay.bogus"),
            // 162. Debugger.unknown → error -32601
            (91, "Debugger.bogus"),
            // 167. Log.unknown → error -32601
            (96, "Log.bogus"),
            // 177. Fetch.unknown → error -32601
            (106, "Fetch.bogus"),
            // 196. ServiceWorker unknown command → error -32601  @trace REQ-BRW-004 [criterion:19]
            (9, "ServiceWorker.nonExistent"),
        ];
        for (id, method) in cases {
            let resp = dispatch_no_bridge(*id, method, None);
            assert!(
                resp.result.is_none(),
                "{method} (id {id}): expected no result"
            );
            assert_eq!(
                resp.error.unwrap().code,
                -32601,
                "{method} (id {id}): expected method-not-found"
            );
        }
    }

    /// δ1 form: explicit error — no result, error code, and message
    /// fragment all pinned (the "fail loudly, never fake success" face;
    /// the reason fragment is per-row data because each failure carries
    /// its own reason).
    #[test]
    fn handle_command_explicit_error_full_matrix() {
        let cases: Vec<(i64, &str, Option<Value>, i64, &str)> = vec![
            // 10. Target.createTarget (no bridge) → -32603 (page creation
            //     requires the servo bridge; never an echo of the target id)
            (
                3,
                "Target.createTarget",
                Some(json!({"url": "https://example.com"})),
                -32603,
                "no servo bridge connected",
            ),
            // 11. Target.closeTarget (no bridge) → -32603 (closing a page
            //     requires the bridge — no fire-and-forget ok)
            (
                4,
                "Target.closeTarget",
                None,
                -32603,
                "no servo bridge connected",
            ),
            // 29. Fetch.enable (one pattern) → -32603 (REQ-CDP-006 C6: a
            //     real bridge command — the interception slot lives
            //     browser-side, so arming it without a bridge is -32603,
            //     never a canned "enabled")
            (
                22,
                "Fetch.enable",
                Some(json!({"patterns": [{"urlPattern": "*"}]})),
                -32603,
                "no servo bridge connected",
            ),
            // 30. Fetch.continueRequest (no bridge) → -32603 (real
            //     resolution now: the held load lives browser-side)
            (
                23,
                "Fetch.continueRequest",
                Some(json!({"interceptionId": "interception-1"})),
                -32603,
                "no servo bridge connected",
            ),
            // 76. Target.attachToTarget (stateless face) → -32000 (session
            //     minting lives in bao_browser::ws_registry::BaoWsRegistry;
            //     the stateless backend has no session table — never a
            //     fabricated sessionId)
            (
                5,
                "Target.attachToTarget",
                Some(json!({"targetId": "t1", "flatten": true})),
                -32000,
                "WS session registry",
            ),
            // 77. Target.detachFromTarget (stateless face) → -32000 (same
            //     session-table reasoning as attachToTarget)
            (
                6,
                "Target.detachFromTarget",
                None,
                -32000,
                "WS session registry",
            ),
            // 169. Fetch.continueWithResponse → -32000 (capability-absent
            //      face: a renamed fulfillRequest, phase-2; use
            //      Fetch.fulfillRequest)
            (
                98,
                "Fetch.continueWithResponse",
                Some(json!({"requestId": "r1"})),
                -32000,
                "phase-2",
            ),
            // 170. Fetch.failRequest (no bridge) → -32603 (real resolution
            //      now; errorReason rides along advisory-only)
            (
                99,
                "Fetch.failRequest",
                Some(json!({"interceptionId": "i2", "errorReason": "Aborted"})),
                -32603,
                "no servo bridge connected",
            ),
            // 171. Fetch.fulfillRequest (no bridge) → -32603 (real
            //      synthesized-response path now)
            (
                100,
                "Fetch.fulfillRequest",
                Some(
                    json!({"interceptionId": "i3", "responseCode": 404, "body": "dGVzdA=="}),
                ),
                -32603,
                "no servo bridge connected",
            ),
            // 172. Fetch.getRequestPostData → -32000 (capability-absent
            //      face: the servo embedder request face carries no request
            //      body — never empty-body ok)
            (
                101,
                "Fetch.getRequestPostData",
                Some(json!({"requestId": "r4"})),
                -32000,
                "no request body",
            ),
            // 173. Fetch.continueWithAuth → -32000 (capability-absent face:
            //      the auth interception facet is not wired)
            (
                102,
                "Fetch.continueWithAuth",
                Some(json!({"requestId": "r5"})),
                -32000,
                "auth interception facet is not wired",
            ),
            // 174. Fetch.takeResponseBodyAsStream → -32000
            //      (capability-absent face: no stream handles in P1)
            (
                103,
                "Fetch.takeResponseBodyAsStream",
                Some(json!({"requestId": "r6"})),
                -32000,
                "stream handles",
            ),
            // 175. Fetch.enable without patterns (no bridge) → -32603 (an
            //      omitted patterns param is a catch-all pattern — still a
            //      real bridge arming)
            (104, "Fetch.enable", None, -32603, "no servo bridge connected"),
            // 176. Fetch.enable with multiple patterns (no bridge) → -32603
            (
                105,
                "Fetch.enable",
                Some(json!({"patterns": [{"urlPattern": "*"}, {"urlPattern": "https://*"}]})),
                -32603,
                "no servo bridge connected",
            ),
        ];
        for (id, method, params, code, msg) in cases {
            let resp = dispatch_no_bridge(id, method, params);
            assert!(
                resp.result.is_none(),
                "{method} (id {id}): expected no result"
            );
            let err = resp.error.unwrap();
            assert_eq!(err.code, code, "{method} (id {id}): wrong error code");
            assert!(
                err.message.contains(msg),
                "{method} (id {id}): message lacks {msg:?}: {err:?}"
            );
        }
    }

    /// REQ-CDP-010: Page.getNavigationHistory reads the browser-side session-
    /// history journal through the bridge (the measured Chrome entry shape is
    /// reported verbatim) and never fabricates a 1-entry history on a bridge
    /// miss (the no-bridge transport-class -32603 is pinned by the suite).
    #[test]
    fn page_get_navigation_history_reads_journal_through_bridge() {
        use crate::servo_bridge::{bridge_channel, BridgeError, BridgeResponse};
        let (tx, rx) = bridge_channel(std::time::Duration::from_secs(2));
        let _keeper = std::thread::spawn(move || loop {
            let handled = rx.try_process(|cmd| match cmd {
                BridgeCommand::GetNavigationHistory { .. } => BridgeResponse { // @evidence .spec/10-REQUIREMENTS.html:2324
                    result: Ok(json!({
                        "currentIndex": 1,
                        "entries": [
                            {
                                "id": 1,
                                "title": "",
                                "transitionType": "auto_toplevel",
                                "url": "about:blank",
                                "userTypedURL": "about:blank"
                            },
                            {
                                "id": 2,
                                "title": "t",
                                "transitionType": "typed",
                                "url": "https://example.com/",
                                "userTypedURL": "https://example.com/"
                            }
                        ]
                    })),
                },
                _ => BridgeResponse { // @evidence .spec/10-REQUIREMENTS.html:2324
                    result: Err(BridgeError::no_bridge("unexpected bridge command")),
                },
            });
            if !handled {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let msg = cdp_msg(1, "Page.getNavigationHistory", None);
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, Some(&tx));
        let result = resp.result.expect("history result");
        assert_eq!(result["currentIndex"], json!(1));
        assert_eq!(result["entries"].as_array().map(|a| a.len()), Some(2));
        let first = &result["entries"][0];
        assert_eq!(first["transitionType"], json!("auto_toplevel"));
        assert_eq!(first["userTypedURL"], json!("about:blank"));
    }

    /// REQ-CDP-010: Page.getFrameTree consumes the browser-side per-load
    /// loader registry row through the bridge — the frame carries the SAME
    /// loaderId Page.navigate reports (verified against the Chrome 155
    /// oracle), and the id is never minted on this command face.
    #[test]
    fn page_get_frame_tree_reads_loader_id_from_registry_row() {
        use crate::servo_bridge::{bridge_channel, BridgeError, BridgeResponse};
        let (tx, rx) = bridge_channel(std::time::Duration::from_secs(2));
        let _keeper = std::thread::spawn(move || loop {
            let handled = rx.try_process(|cmd| match cmd {
                BridgeCommand::EvaluateJs { .. } => BridgeResponse { // @evidence .spec/10-REQUIREMENTS.html:2324
                    result: Ok(json!({
                        "result": {
                            "type": "string",
                            "value": "{\"url\":\"https://example.com/\",\"mimeType\":\"text/html\",\"name\":\"\",\"securityOrigin\":\"https://example.com\",\"domainAndRegistry\":\"example.com\",\"secureContextType\":\"SecureContext\",\"securityOriginDetails\":{\"isLocalhost\":false},\"crossOriginIsolatedContextType\":\"NotIsolated\",\"gatedAPIFeatures\":[],\"adFrameStatus\":{\"adFrameType\":\"none\"}}"
                        }
                    })),
                },
                BridgeCommand::GetLoaderId { .. } => BridgeResponse { // @evidence .spec/10-REQUIREMENTS.html:2324
                    result: Ok(json!("1A2B3C4D5E6F708192A3B4C5D6E7F809")),
                },
                _ => BridgeResponse { // @evidence .spec/10-REQUIREMENTS.html:2324
                    result: Err(BridgeError::no_bridge("unexpected bridge command")),
                },
            });
            if !handled {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let msg = cdp_msg(1, "Page.getFrameTree", None);
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, Some(&tx));
        let frame = resp
            .result
            .expect("frame tree result")
            .get("frameTree")
            .and_then(|t| t.get("frame"))
            .cloned()
            .expect("frame object");
        assert_eq!(frame["id"], json!("main-t1"));
        assert_eq!(
            frame["loaderId"],
            json!("1A2B3C4D5E6F708192A3B4C5D6E7F809"),
            "getFrameTree must report the registry row verbatim"
        );
    }

    /// δ2 form: explicit error — code and message fragment pinned (the
    /// replaced suites expected the error without asserting result-none;
    /// parity keeps exactly that shape).
    #[test]
    fn handle_command_explicit_error_message_matrix() {
        let cases: Vec<(i64, &str, Option<Value>, i64, &str)> = vec![
            // 14. Page.getLayoutMetrics (no bridge) → -32603 (real layout
            //     data requires the bridge; never canned 1920×1080)
            (7, "Page.getLayoutMetrics", None, -32603, "no servo bridge"),
            // 80. Page.navigate (no bridge, default url) → -32603 (the
            //     bridge response carries the real frameId/loaderId —
            //     never fabricated here)
            (9, "Page.navigate", None, -32603, "no servo bridge"),
            // 81. Page.navigate (no bridge, explicit url) → -32603
            (
                10,
                "Page.navigate",
                Some(json!({"url": "https://example.com"})),
                -32603,
                "no servo bridge",
            ),
            // 82. Page.reload (no bridge) → -32603
            (11, "Page.reload", None, -32603, "no servo bridge"),
            // 83. Page.getFrameTree (no bridge) → -32603 (frame data is
            //     read from the live document via the bridge)
            (12, "Page.getFrameTree", None, -32603, "no servo bridge"),
            // 84. Page.getNavigationHistory → -32000 (servo WebView exposes
            //     no session-history enumeration)
            (13, "Page.getNavigationHistory", None, -32000, "not supported"),
            // 85. Page.captureScreenshot (no bridge) → -32603 (no renderer
            //     without the bridge — never empty image data)
            (14, "Page.captureScreenshot", None, -32603, "no servo bridge"),
            // 87. Page.removeScriptToEvaluateOnNewDocument (no bridge) →
            //     -32603 (real removal lives behind the servo bridge — the
            //     vendor registry is per-page state)
            (
                16,
                "Page.removeScriptToEvaluateOnNewDocument",
                Some(json!({"identifier": "3"})),
                -32603,
                "no servo bridge",
            ),
            // 88. Page.setContent (no html param) → -32602 invalid params
            (17, "Page.setContent", None, -32602, "html"),
            // REQ-CDS-009: browser-context lifecycle commands are
            // bridge-backed (the registry lives browser-side with the
            // PagePool) — bridge-less dispatch fails loudly, never a canned
            // context id or an empty list.
            (
                18,
                "Target.createBrowserContext",
                None,
                -32603,
                "no servo bridge connected",
            ),
            (
                19,
                "Target.disposeBrowserContext",
                Some(json!({"browserContextId": "bao-context-1"})),
                -32603,
                "no servo bridge connected",
            ),
            (
                20,
                "Target.getBrowserContexts",
                None,
                -32603,
                "no servo bridge connected",
            ),
            // Missing browserContextId param → -32602 invalid params (Chrome
            // requires it for disposeBrowserContext).
            (
                21,
                "Target.disposeBrowserContext",
                None,
                -32602,
                "requires a non-empty browserContextId param",
            ),
        ];
        for (id, method, params, code, msg) in cases {
            let resp = dispatch_no_bridge(id, method, params);
            let err = resp
                .error
                .unwrap_or_else(|| panic!("{method} (id {id}): explicit error required"));
            assert_eq!(err.code, code, "{method} (id {id}): wrong error code");
            assert!(
                err.message.contains(msg),
                "{method} (id {id}): message lacks {msg:?}: {err:?}"
            );
        }
    }

    /// δ3 form: explicit error — code only (the replaced suites pinned
    /// nothing beyond the -32603 no-bridge error code).
    #[test]
    fn handle_command_explicit_error_code_only_matrix() {
        let cases: Vec<(i64, &str, Option<Value>)> = vec![
            // 89. Page.close (no bridge) → -32603
            (18, "Page.close", None),
            // 90. Page.bringToFront (no bridge) → -32603
            (19, "Page.bringToFront", None),
            // 110. DOM.getOuterHTML (no bridge) → -32603 (outerHTML is read
            //      from the live document — never canned html)
            (39, "DOM.getOuterHTML", None),
            // 115. Network.getResponseBody (no bridge) → -32603 (servo does
            //      not expose stored response bodies — never empty-body ok)
            (44, "Network.getResponseBody", None),
            // 117. Network.setExtraHTTPHeaders (no bridge) → -32603 (servo
            //      has no extra-headers injection API — headers never
            //      silently dropped)
            (46, "Network.setExtraHTTPHeaders", None),
        ];
        for (id, method, params) in cases {
            let resp = dispatch_no_bridge(id, method, params);
            let err = resp
                .error
                .unwrap_or_else(|| panic!("{method} (id {id}): explicit error required"));
            assert_eq!(err.code, -32603, "{method} (id {id}): wrong error code");
        }
    }

    // 16. handle_command Runtime.evaluate (no bridge, empty expr) → undefined result
    #[test]
    fn handle_command_runtime_evaluate_no_bridge() {
        let resp = dispatch_no_bridge(9, "Runtime.evaluate", Some(json!({"expression": ""})));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 17. handle_command DOM.getDocument (no bridge) → -32603 — REQ-BRW-048
    //     follow-up: the canned root tree is eradicated; the document lives
    //     on the page, and there is no page without a bridge.
    #[test]
    fn handle_command_dom_get_document() {
        let resp = dispatch_no_bridge(10, "DOM.getDocument", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"));
    }

    // 18. handle_command DOM.querySelector (no bridge, selector given) →
    //     -32603 — REQ-BRW-048 follow-up: real canonical ids come from the
    //     live page (the found?1:0 fake is eradicated; nodeId 0 now only
    //     means "no match" on a live page).
    #[test]
    fn handle_command_dom_query_selector() {
        let resp = dispatch_no_bridge(11, "DOM.querySelector", Some(json!({"selector": "div"})));
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32603);
    }

    // 20. handle_command Network.getCookies → ok with empty cookies
    #[test]
    fn handle_command_network_get_cookies() {
        let resp = dispatch_no_bridge(13, "Network.getCookies", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["cookies"], json!([]));
    }

    // 22. handle_command CSS.getComputedStyleForNode (no bridge, no params)
    //     → -32602 (required nodeId) — REQ-BRW-048: the empty computedStyle
    //     list is eradicated; the style face reads the live page or fails
    //     explicitly.
    #[test]
    fn handle_command_css_get_computed_style() {
        assert_css_style_query_requires_nodeid(15, "CSS.getComputedStyleForNode");
    }

    // 27. handle_command Debugger.setBreakpointByUrl → -32603 (task #10:
    //     a breakpoint write without a bridge can never be hit).
    #[test]
    fn handle_command_debugger_set_breakpoint_by_url() {
        let resp = dispatch_no_bridge(20, "Debugger.setBreakpointByUrl", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32603);
    }

    // 31. CdpError clone + debug format
    #[test]
    fn cdp_error_clone_and_debug() {
        let err = CdpError {
            code: -32601,
            message: "not found".into(),
        };
        let cloned = err.clone();
        assert_eq!(cloned.code, err.code);
        assert_eq!(cloned.message, err.message);
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("-32601"));
        assert!(debug_str.contains("not found"));
    }

    // 32. CdpEvent serialize
    #[test]
    fn cdp_event_serialize() {
        let ev = CdpEvent {
            method: "Page.loadEventFired".into(),
            params: Some(json!({"timestamp": 12345})),
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["method"], "Page.loadEventFired");
        assert_eq!(parsed["params"]["timestamp"], 12345);
    }

    // 33. CdpMessage deserialize with unicode method name
    #[test]
    fn parse_message_unicode_method() {
        let msg = parse_message(r#"{"id":99,"method":"Page.你好世界"}"#).unwrap();
        assert_eq!(msg.id, Some(99));
        assert_eq!(msg.method, "Page.你好世界");
    }

    // ─── CdpMessage parsing edge cases ─────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 34. CdpMessage with id = 0
    #[test]
    fn parse_message_id_zero() {
        let msg = parse_message(r#"{"id":0,"method":"Page.enable"}"#).unwrap();
        assert_eq!(msg.id, Some(0));
        assert_eq!(msg.method, "Page.enable");
    }

    // 35. CdpMessage with id = i64::MAX
    #[test]
    fn parse_message_id_max() {
        let msg = parse_message(r#"{"id":9223372036854775807,"method":"Page.enable"}"#).unwrap();
        assert_eq!(msg.id, Some(i64::MAX));
    }

    // 36. CdpMessage with negative id
    #[test]
    fn parse_message_negative_id() {
        let msg = parse_message(r#"{"id":-1,"method":"Page.enable"}"#).unwrap();
        assert_eq!(msg.id, Some(-1));
    }

    // 37. CdpMessage with id = i64::MIN
    #[test]
    fn parse_message_id_min() {
        let msg = parse_message(r#"{"id":-9223372036854775808,"method":"Page.enable"}"#).unwrap();
        assert_eq!(msg.id, Some(i64::MIN));
    }

    // 38. CdpMessage with empty method string
    #[test]
    fn parse_message_empty_method() {
        let msg = parse_message(r#"{"id":1,"method":""}"#).unwrap();
        assert_eq!(msg.method, "");
    }

    // 39. CdpMessage with method containing no dot
    #[test]
    fn parse_message_method_no_dot() {
        let msg = parse_message(r#"{"id":1,"method":"NoDomain"}"#).unwrap();
        assert_eq!(msg.method, "NoDomain");
    }

    // 40. CdpMessage with method containing multiple dots
    #[test]
    fn parse_message_method_multiple_dots() {
        let msg = parse_message(r#"{"id":1,"method":"Page.navigate.to"}"#).unwrap();
        assert_eq!(msg.method, "Page.navigate.to");
        // splitn(2, '.') only splits on first dot
        let parts: Vec<&str> = msg.method.splitn(2, '.').collect();
        assert_eq!(parts[0], "Page");
        assert_eq!(parts[1], "navigate.to");
    }

    // 41. CdpMessage with empty string input
    #[test]
    fn parse_message_empty_string() {
        assert!(parse_message("").is_none());
    }

    // 42. CdpMessage with whitespace-only input
    #[test]
    fn parse_message_whitespace_only() {
        assert!(parse_message("   ").is_none());
    }

    // 43. CdpMessage with extra JSON fields (should succeed, ignores unknown)
    #[test]
    fn parse_message_extra_fields() {
        let msg = parse_message(r#"{"id":1,"method":"Page.enable","extra":"ignored"}"#);
        assert!(msg.is_some());
        assert_eq!(msg.unwrap().method, "Page.enable");
    }

    // 44. CdpMessage params as object
    #[test]
    fn parse_message_params_object() {
        let msg =
            parse_message(r#"{"id":1,"method":"Page.navigate","params":{"url":"http://x.com"}}"#)
                .unwrap();
        assert!(msg.params.is_some());
        assert_eq!(msg.params.unwrap()["url"], "http://x.com");
    }

    // 45. CdpMessage params as array (unusual but valid JSON)
    #[test]
    fn parse_message_params_array() {
        let msg = parse_message(r#"{"id":1,"method":"Test.cmd","params":[1,2,3]}"#).unwrap();
        assert!(msg.params.is_some());
        assert!(msg.params.unwrap().is_array());
    }

    // 46. CdpMessage params as string (unusual but valid JSON)
    #[test]
    fn parse_message_params_string() {
        let msg = parse_message(r#"{"id":1,"method":"Test.cmd","params":"hello"}"#).unwrap();
        assert!(msg.params.is_some());
        assert!(msg.params.unwrap().is_string());
    }

    // 47. CdpMessage params as number (unusual but valid JSON)
    #[test]
    fn parse_message_params_number() {
        let msg = parse_message(r#"{"id":1,"method":"Test.cmd","params":42}"#).unwrap();
        assert!(msg.params.is_some());
        assert!(msg.params.unwrap().is_number());
    }

    // 48. CdpMessage params as boolean (unusual but valid JSON)
    #[test]
    fn parse_message_params_boolean() {
        let msg = parse_message(r#"{"id":1,"method":"Test.cmd","params":true}"#).unwrap();
        assert!(msg.params.is_some());
        assert!(msg.params.unwrap().is_boolean());
    }

    // 49. CdpMessage with very long session_id
    #[test]
    fn parse_message_long_session_id() {
        let long_session = "A".repeat(10000);
        let raw = format!(
            r#"{{"id":1,"method":"Page.enable","sessionId":"{}"}}"#,
            long_session
        );
        let msg = parse_message(&raw).unwrap();
        assert_eq!(msg.session_id.unwrap().len(), 10000);
    }

    // 50. CdpMessage with empty session_id
    #[test]
    fn parse_message_empty_session_id() {
        let msg = parse_message(r#"{"id":1,"method":"Page.enable","sessionId":""}"#).unwrap();
        assert_eq!(msg.session_id, Some("".to_string()));
    }

    // ─── CdpResponse serialization edge cases ──────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 51. CdpResponse with null result
    #[test]
    fn serialize_response_null_result() {
        let resp = CdpResponse {
            id: Some(1),
            result: Some(Value::Null),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"], Value::Null);
    }

    // 52. CdpResponse with empty object result
    #[test]
    fn serialize_response_empty_object_result() {
        let resp = CdpResponse {
            id: Some(2),
            result: Some(json!({})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"], json!({}));
    }

    // 53. CdpResponse with nested result
    #[test]
    fn serialize_response_nested_result() {
        let resp = CdpResponse {
            id: Some(3),
            result: Some(json!({"root": {"nodeId": 1, "children": [{"nodeId": 2}]}})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"]["root"]["nodeId"], 1);
        assert_eq!(parsed["result"]["root"]["children"][0]["nodeId"], 2);
    }

    // 54. CdpResponse with id = 0
    #[test]
    fn serialize_response_id_zero() {
        let resp = CdpResponse {
            id: Some(0),
            result: Some(json!({"ok": true})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], 0);
    }

    // 55. CdpResponse with negative id
    #[test]
    fn serialize_response_negative_id() {
        let resp = CdpResponse {
            id: Some(-42),
            result: Some(json!({})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], -42);
    }

    // 56. CdpResponse with i64::MAX id
    #[test]
    fn serialize_response_max_id() {
        let resp = CdpResponse {
            id: Some(i64::MAX),
            result: Some(json!({})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], i64::MAX);
    }

    // 57. CdpResponse with array result
    #[test]
    fn serialize_response_array_result() {
        let resp = CdpResponse {
            id: Some(5),
            result: Some(json!([1, 2, 3])),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"], json!([1, 2, 3]));
    }

    // 58. CdpResponse with string result
    #[test]
    fn serialize_response_string_result() {
        let resp = CdpResponse {
            id: Some(6),
            result: Some(json!("hello world")),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"], "hello world");
    }

    // ─── CdpError code boundaries ──────────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 59. CdpError code -32700 (Parse error)
    #[test]
    fn cdp_error_code_parse_error() {
        let resp = CdpResponse {
            id: Some(1),
            result: None,
            error: Some(CdpError {
                code: -32700,
                message: "Parse error".into(),
            }),
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["error"]["code"], -32700);
        assert_eq!(parsed["error"]["message"], "Parse error");
    }

    // 60. CdpError code -32600 (Invalid Request)
    #[test]
    fn cdp_error_code_invalid_request() {
        let err = CdpError {
            code: -32600,
            message: "Invalid Request".into(),
        };
        assert_eq!(err.code, -32600);
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("-32600"));
    }

    // 61. CdpError code -32601 (Method not found)
    #[test]
    fn cdp_error_code_method_not_found() {
        let err = CdpError {
            code: -32601,
            message: "Method not found".into(),
        };
        assert_eq!(err.code, -32601);
    }

    // 62. CdpError code -32602 (Invalid params)
    #[test]
    fn cdp_error_code_invalid_params() {
        let err = CdpError {
            code: -32602,
            message: "Invalid params".into(),
        };
        assert_eq!(err.code, -32602);
    }

    // 63. CdpError code -32603 (Internal error)
    #[test]
    fn cdp_error_code_internal_error() {
        let err = CdpError {
            code: -32603,
            message: "Internal error".into(),
        };
        assert_eq!(err.code, -32603);
    }

    // 64. CdpError with custom error code
    #[test]
    fn cdp_error_custom_code() {
        let err = CdpError {
            code: -32000,
            message: "Server error".into(),
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("-32000"));
        assert!(s.contains("Server error"));
    }

    // 65. CdpError with empty message
    #[test]
    fn cdp_error_empty_message() {
        let err = CdpError {
            code: -32601,
            message: String::new(),
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("-32601"));
    }

    // 66. CdpError with unicode message
    #[test]
    fn cdp_error_unicode_message() {
        let err = CdpError {
            code: -32601,
            message: "方法未找到".into(),
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("方法未找到"));
    }

    // ─── CdpEvent edge cases ───────────────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 67. CdpEvent with no params
    #[test]
    fn cdp_event_no_params() {
        let ev = CdpEvent {
            method: "Page.domContentEventFired".into(),
            params: None,
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["method"], "Page.domContentEventFired");
        assert!(
            parsed.get("params").is_none(),
            "params should be skipped when None"
        );
    }

    // 68. CdpEvent with large data
    #[test]
    fn cdp_event_large_data() {
        let large_string = "X".repeat(100_000);
        let ev = CdpEvent {
            method: "Network.dataReceived".into(),
            params: Some(
                json!({ "dataLength": large_string.len(), "encodedDataLength": large_string.len() }),
            ),
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["params"]["dataLength"], 100_000);
    }

    // 69. CdpEvent with nested params
    #[test]
    fn cdp_event_nested_params() {
        let ev = CdpEvent {
            method: "DOM.attributeModified".into(),
            params: Some(json!({
                "nodeId": 1,
                "name": "class",
                "value": "container active",
                "metadata": {
                    "source": "user",
                    "timestamp": 1234567890
                }
            })),
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["params"]["nodeId"], 1);
        assert_eq!(parsed["params"]["metadata"]["source"], "user");
    }

    // 70. CdpEvent with null params
    #[test]
    fn cdp_event_null_params() {
        let ev = CdpEvent {
            method: "Page.frameResized".into(),
            params: Some(Value::Null),
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["params"], Value::Null);
    }

    // 71. CdpEvent with empty method
    #[test]
    fn cdp_event_empty_method() {
        let ev = CdpEvent {
            method: String::new(),
            params: None,
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["method"], "");
    }

    // ─── handle_command edge cases ─────────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 72. handle_command with method containing no dot → empty domain
    #[test]
    fn handle_command_no_dot_method() {
        let resp = dispatch_no_bridge(1, "NoDomain", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("NoDomain"));
    }

    // 73. handle_command with empty method → empty domain, error
    #[test]
    fn handle_command_empty_method() {
        let resp = dispatch_no_bridge(2, "", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    // 74. handle_command with known domain but unknown command
    #[test]
    fn handle_command_known_domain_unknown_command() {
        let resp = dispatch_no_bridge(3, "Page.nonExistentCommand", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Page.nonExistentCommand"));
    }

    // 75. handle_command Target.getTargetInfo (no bridge) → ok with targetInfo
    #[test]
    fn handle_command_target_get_target_info() {
        let resp = dispatch_no_bridge(4, "Target.getTargetInfo", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let info = result.get("targetInfo").unwrap();
        assert_eq!(info["targetId"], "t1");
        assert_eq!(info["type"], "page");
        assert_eq!(info["attached"], true);
    }

    // 79. handle_command Target.getTargetTargets → ok (alias for getTargets)
    #[test]
    fn handle_command_target_get_target_targets() {
        let resp = dispatch_no_bridge(8, "Target.getTargetTargets", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("targetInfos").unwrap().as_array().unwrap().len() > 0);
    }

    // 86. handle_command Page.addScriptToEvaluateOnNewDocument (no bridge)
    //     → -32603: registration (and the vendor-minted identifier it
    //     returns) lives behind the servo bridge — no bridge, no handle.
    #[test]
    fn handle_command_page_add_script_empty_source() {
        let resp = dispatch_no_bridge(15, "Page.addScriptToEvaluateOnNewDocument", None);
        let err = resp.error.expect("no bridge = no registration possible");
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"), "{err:?}");
    }

    // 93. handle_command Runtime.callFunctionOn → ok
    #[test]
    fn handle_command_runtime_call_function_on() {
        let resp = dispatch_no_bridge(22, "Runtime.callFunctionOn", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 94. handle_command Runtime.getProperties → ok with empty array
    #[test]
    fn handle_command_runtime_get_properties() {
        let resp = dispatch_no_bridge(23, "Runtime.getProperties", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"], json!([]));
    }

    // 100. handle_command Runtime.unknown → error -32601
    #[test]
    fn handle_command_runtime_unknown_command() {
        let resp = dispatch_no_bridge(29, "Runtime.unknownMethod", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Runtime.unknownMethod"));
    }

    // 103. handle_command DOM.describeNode → -32602 — REQ-BRW-048 follow-up:
    //     real node data needs a node ref (canned HTML node eradicated).
    #[test]
    fn handle_command_dom_describe_node() {
        let resp = dispatch_no_bridge(32, "DOM.describeNode", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("nodeId or objectId"));
    }

    // 104. handle_command DOM.getBoxModel → -32602 — REQ-BRW-048 follow-up:
    //     the canned 1920×1080 geometry is eradicated.
    #[test]
    fn handle_command_dom_get_box_model() {
        let resp = dispatch_no_bridge(33, "DOM.getBoxModel", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("nodeId or objectId"));
    }

    // 111. handle_command DOM.resolveNode → -32602 — REQ-BRW-048 follow-up:
    //     the shape-shell RemoteObject is eradicated.
    #[test]
    fn handle_command_dom_resolve_node() {
        let resp = dispatch_no_bridge(40, "DOM.resolveNode", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32602);
        // CDP spec: nodeId / objectId / backendNodeId — none present is
        // -32602 (the message pins all three accepted shapes).
        assert!(err.message.contains("nodeId, objectId or backendNodeId"));
    }

    // 112. handle_command DOM.pushNodesByBackendIdsToFrontend → -32602 —
    //     REQ-BRW-048 follow-up: backendNodeIds is required.
    #[test]
    fn handle_command_dom_push_nodes_by_backend_ids() {
        let resp = dispatch_no_bridge(41, "DOM.pushNodesByBackendIdsToFrontend", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("backendNodeIds"));
    }

    // 119. handle_command Network.getAllCookies → ok with empty cookies
    #[test]
    fn handle_command_network_get_all_cookies() {
        let resp = dispatch_no_bridge(48, "Network.getAllCookies", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["cookies"], json!([]));
    }

    // 121. handle_command Network.setCookie → spec shape {"success":true}
    #[test]
    fn handle_command_network_set_cookie() {
        let resp = dispatch_no_bridge(50, "Network.setCookie", None);
        assert!(resp.error.is_none());
        // CDP spec SetCookieReturnObject — no-bridge stub face keeps the
        // spec shape, same as the browser-side bridge path.
        assert_eq!(resp.result, Some(json!({ "success": true })));
    }

    // 126. handle_command CSS.getMatchedStylesForNode (no bridge, no params)
    //     → -32602 (required nodeId) — REQ-BRW-048 fail-closed contract.
    #[test]
    fn handle_command_css_get_matched_styles() {
        assert_css_style_query_requires_nodeid(55, "CSS.getMatchedStylesForNode");
    }

    // 127. handle_command CSS.getInlineStylesForNode (no bridge, no params)
    //     → -32602 (required nodeId) — REQ-BRW-048 fail-closed contract.
    #[test]
    fn handle_command_css_get_inline_styles() {
        assert_css_style_query_requires_nodeid(56, "CSS.getInlineStylesForNode");
    }

    // 127b. REQ-BRW-048: the CSS query methods with a valid nodeId still fail
    //       explicitly without a bridge (no page → no styles), and the two
    //       new DOM query methods carry the same contract (getFlattenedDocument
    //       has only optional params → -32603 directly).
    #[test]
    fn handle_command_css_dom_query_methods_no_bridge_explicit_error() {
        let node = Some(json!({ "nodeId": 1 }));
        for (method, params, code) in [
            ("CSS.getComputedStyleForNode", node.clone(), -32603),
            ("CSS.getMatchedStylesForNode", node.clone(), -32603),
            ("CSS.getInlineStylesForNode", node.clone(), -32603),
            ("DOM.getFlattenedDocument", None, -32603),
            ("DOM.getNodeForOwner", None, -32602),
        ] {
            let resp = dispatch_no_bridge(80, method, params);
            let err = resp
                .error
                .unwrap_or_else(|| panic!("{method}: explicit error required"));
            assert_eq!(err.code, code, "{method}: wrong error code");
            assert!(resp.result.is_none(), "{method}: error must not carry result");
        }
    }

    // 128. handle_command CSS.setStyleTexts → -32602 — REQ-BRW-048
    //     follow-up: the write path requires edits ({"styles":[]} shell
    //     eradicated).
    #[test]
    fn handle_command_css_set_style_texts() {
        let resp = dispatch_no_bridge(57, "CSS.setStyleTexts", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("edits"));
    }

    // 158. handle_command Debugger.evaluateOnCallFrame → ok
    #[test]
    fn handle_command_debugger_evaluate_on_call_frame() {
        let resp = dispatch_no_bridge(87, "Debugger.evaluateOnCallFrame", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 159. handle_command Debugger.getPossibleBreakpoints → ok
    #[test]
    fn handle_command_debugger_get_possible_breakpoints() {
        let resp = dispatch_no_bridge(88, "Debugger.getPossibleBreakpoints", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["locations"], json!([]));
    }

    // 160. handle_command Debugger.getScriptSource → ok
    #[test]
    fn handle_command_debugger_get_script_source() {
        let resp = dispatch_no_bridge(89, "Debugger.getScriptSource", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["scriptSource"], "");
    }

    // ─── params_str edge cases ─────────────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 178. params_str with nested key returns empty
    #[test]
    fn params_str_nested_key_returns_empty() {
        let params = Some(json!({"outer": {"inner": "value"}}));
        let result = params_str(&params, "outer.inner");
        assert_eq!(result, ""); // JSON pointer doesn't work with dot notation in params_str
    }

    // 179. params_str with numeric value returns empty (not a string)
    #[test]
    fn params_str_numeric_value_returns_empty() {
        let params = Some(json!({"count": 42}));
        let result = params_str(&params, "count");
        assert_eq!(result, ""); // as_str() returns None for numbers
    }

    // 180. params_str with boolean value returns empty
    #[test]
    fn params_str_boolean_value_returns_empty() {
        let params = Some(json!({"flag": true}));
        let result = params_str(&params, "flag");
        assert_eq!(result, ""); // as_str() returns None for booleans
    }

    // 181. params_str with null value returns empty
    #[test]
    fn params_str_null_value_returns_empty() {
        let params = Some(json!({"key": null}));
        let result = params_str(&params, "key");
        assert_eq!(result, ""); // as_str() returns None for null
    }

    // 182. params_str with missing key returns empty
    #[test]
    fn params_str_missing_key_returns_empty() {
        let params = Some(json!({"other": "value"}));
        let result = params_str(&params, "key");
        assert_eq!(result, "");
    }

    // 183. params_str with None params returns empty
    #[test]
    fn params_str_none_returns_empty() {
        let result = params_str(&None, "key");
        assert_eq!(result, "");
    }

    // 184. params_str with empty string value returns empty string
    #[test]
    fn params_str_empty_string_value() {
        let params = Some(json!({"key": ""}));
        let result = params_str(&params, "key");
        assert_eq!(result, "");
    }

    // ─── bridge_send edge case (no bridge → error -32603) ──────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 185. handle_command with bridge-dependent command and no bridge returns -32603
    #[test]
    fn handle_command_bridge_required_no_bridge_returns_internal_error() {
        // Page.navigate without bridge still succeeds (returns default)
        // but Runtime.evaluate with non-empty expression and no bridge returns undefined
        // DOM.querySelector with selector and no bridge returns nodeId:0
        // The key scenario is when bridge_send is called with None
        // This is tested indirectly through the domain handlers
        // Direct test: bridge_send(None, ...) → Err(-32603)
        let result = bridge_send(
            None,
            BridgeCommand::GetTitle {
                target_id: "test-target".into(),
            },
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge connected"));
    }

    // 185b. bridge_send with a live bridge maps the Err payload's class:
    //     domain-owned outcomes (Fetch.resolve UnknownId/Rejected, Overlay
    //     unknown node) surface as -32000 with the message verbatim, while
    //     transport-class faces stay -32603 (e234/e236; REQ-CDP-006/007).
    #[test]
    fn bridge_send_maps_error_kind_to_cdp_code() {
        use crate::servo_bridge::BridgeError;
        let (bridge, receiver) = bridge_channel(std::time::Duration::from_secs(2));
        let responder = std::thread::spawn(move || {
            receiver.recv_and_process(std::time::Duration::from_secs(2), |cmd| BridgeResponse {
                result: Err(match cmd {
                    BridgeCommand::FetchResolve { .. } => {
                        BridgeError::domain(
                            "unknown interceptionId: i-1 (never paused or already resolved)",
                        )
                    }
                    _ => BridgeError::no_bridge("bridge response timeout"),
                }),
            })
        });
        // Domain-class face: Fetch.continueRequest with an unknown id.
        let msg = cdp_msg(
            90,
            "Fetch.continueRequest",
            Some(json!({"interceptionId": "i-1"})),
        );
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, Some(&bridge));
        let err = resp.error.expect("unknown interceptionId must fail explicitly");
        assert_eq!(err.code, ERR_NOT_SUPPORTED, "domain class → -32000: {err:?}");
        assert!(
            err.message.contains("unknown interceptionId"),
            "message verbatim: {err:?}"
        );
        // Control: transport-class face keeps -32603.
        let msg = cdp_msg(91, "Page.navigate", Some(json!({"url": "https://example.com/"})));
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, Some(&bridge));
        let err = resp.error.expect("transport failure must fail explicitly");
        assert_eq!(err.code, -32603, "transport class → -32603: {err:?}");
        responder.join().unwrap();
    }

    // ─── CdpMessage Debug trait ────────────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 186. CdpMessage debug format
    #[test]
    fn cdp_message_debug_format() {
        let msg = cdp_msg(1, "Page.enable", None);
        let debug = format!("{:?}", msg);
        assert!(debug.contains("CdpMessage"));
        assert!(debug.contains("Page.enable"));
    }

    // 187. CdpMessage clone
    #[test]
    fn cdp_message_clone() {
        let msg = CdpMessage {
            id: Some(1),
            method: "Page.enable".into(),
            params: Some(json!({"k": "v"})),
            session_id: Some("s1".into()),
        };
        let cloned = msg.clone();
        assert_eq!(cloned.id, msg.id);
        assert_eq!(cloned.method, msg.method);
        assert_eq!(cloned.params, msg.params);
        assert_eq!(cloned.session_id, msg.session_id);
    }

    // ─── ServiceWorker domain (REQ-BRW-004 criterion #19) ──────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [level:unit] [criterion:19]

    // 190. ServiceWorker.getAllRegistrations (no bridge) → empty registrations
    #[test]
    fn service_worker_get_all_registrations_no_bridge() {
        let resp = dispatch_no_bridge(3, "ServiceWorker.getAllRegistrations", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["registrations"], json!([]));
    }

    // 191. ServiceWorker.getRegistration (no bridge) → null registration
    #[test]
    fn service_worker_get_registration_no_bridge() {
        let resp = dispatch_no_bridge(
            4,
            "ServiceWorker.getRegistration",
            Some(json!({"registrationId": "sw-reg-1"})),
        );
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["registration"], Value::Null);
    }

    // 194. ServiceWorker.deliverPushMessage → ok with delivered flag
    #[test]
    fn service_worker_deliver_push_message() {
        let resp = dispatch_no_bridge(
            7,
            "ServiceWorker.deliverPushMessage",
            Some(json!({"origin": "https://example.com", "registrationId": "sw-reg-1"})),
        );
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["origin"], "https://example.com");
        assert_eq!(result["delivered"], true);
    }

    // 197. Target.getTargets (no bridge) still returns page target (Worker sub-targets are empty)
    #[test]
    fn target_get_targets_no_bridge_includes_page() {
        let resp = dispatch_no_bridge(10, "Target.getTargets", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let infos = result["targetInfos"].as_array().unwrap();
        assert!(infos.len() >= 1, "should at least have the page target");
        assert_eq!(infos[0]["targetId"], "t1");
    }

    // ─── Overlay domain (REQ-CDP-007-C6) ────────────────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit] [criterion:C6]

    // 195a. Overlay.highlightNode with a node ref but no bridge → -32603:
    //     a real bridge command, never a canned ok.
    #[test]
    fn overlay_highlight_node_no_bridge() {
        let resp = dispatch_no_bridge(
            80,
            "Overlay.highlightNode",
            Some(json!({"nodeId": 5})),
        );
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge connected"));
    }

    // 195b. Overlay.highlightNode without any node shape → -32602
    //     (nodeId / objectId / backendNodeId all accepted, none present).
    #[test]
    fn overlay_highlight_node_missing_node_ref() {
        let resp = dispatch_no_bridge(81, "Overlay.highlightNode", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("nodeId, objectId or backendNodeId"));
    }

    // 195c. Overlay.setInspectMode → -32000: the hover→
    //     Overlay.nodeHighlightRequested event chain does not exist on this
    //     face — explicit unsupported instead of a canned ok.
    #[test]
    fn overlay_set_inspect_mode_not_supported() {
        let resp = dispatch_no_bridge(82, "Overlay.setInspectMode", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, ERR_NOT_SUPPORTED);
        assert!(err.message.contains("Overlay.setInspectMode"));
        assert!(err.message.contains("nodeHighlightRequested"));
    }

    // 195d. Overlay.highlightNode via backendNodeId folds to the canonical
    //     id (DOM.resolveNode identity); the missing-bridge -32603 proves
    //     param validation passed before the bridge check.
    #[test]
    fn overlay_highlight_node_backend_node_id_accepted() {
        let resp = dispatch_no_bridge(
            83,
            "Overlay.highlightNode",
            Some(json!({"backendNodeId": 7, "highlightSpec": {"borderColor": {"r": 255}} })),
        );
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge connected"));
    }

    // 195e. Overlay.hideHighlight without a bridge → -32603 (real state
    //     command on the browser layer, not a shape-only ack).
    #[test]
    fn overlay_hide_highlight_no_bridge() {
        let resp = dispatch_no_bridge(84, "Overlay.hideHighlight", None);
        let err = resp.error.unwrap_or_else(|| panic!("explicit error required"));
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge connected"));
    }

    // ─── REQ-CDP-006 C6: Fetch / legacy-Network interception matrix ────
    // Command face of the request interception facility (design §7
    // boundary matrix). Real commands route to the servo bridge — captured
    // here against the exact BridgeCommand shape the browser-side consumer
    // (bao_browser cdp_handler) matches. Capability-absent faces are
    // explicit -32000 errors with the design §7 boundary declaration in the
    // message, and every malformed param is a fail-fast -32602 that fires
    // before the bridge check.
    // @trace REQ-CDP-006 [criterion:6]
    use crate::servo_bridge::{bridge_channel, BridgeResponse, FetchRequestStage};

    /// Bridge-capture fixture: a helper thread answers `{}` while `drive`
    /// dispatches on this thread; returns the BridgeCommand the handler
    /// actually sent — the wire contract the browser-side consumer is
    /// written against.
    fn capture_bridge_command(drive: impl FnOnce(&BridgeSender)) -> BridgeCommand {
        let (bridge, receiver) = bridge_channel(std::time::Duration::from_secs(2));
        let captured: std::sync::Arc<std::sync::Mutex<Option<BridgeCommand>>> =
            std::sync::Arc::default();
        let sink = std::sync::Arc::clone(&captured);
        let responder = std::thread::spawn(move || {
            receiver.recv_and_process(std::time::Duration::from_secs(2), |cmd| {
                *sink.lock().unwrap() = Some(cmd);
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
        });
        drive(&bridge);
        let cmd = captured
            .lock()
            .unwrap()
            .clone()
            .expect("handler should have sent exactly one bridge command");
        responder.join().unwrap();
        cmd
    }

    /// One-off δ shape: dispatch and pin the error code + message fragment.
    fn expect_dispatch_error(id: i64, method: &str, params: Option<Value>, code: i64, fragment: &str) {
        let resp = dispatch_no_bridge(id, method, params);
        let err = resp
            .error
            .unwrap_or_else(|| panic!("{method} (id {id}): explicit error required"));
        assert_eq!(err.code, code, "{method} (id {id}): wrong error code");
        assert!(
            err.message.contains(fragment),
            "{method} (id {id}): message lacks {fragment:?}: {err:?}"
        );
    }

    #[test]
    fn fetch_enable_sends_normalized_patterns_to_bridge() {
        let cmd = capture_bridge_command(|bridge| {
            let msg = cdp_msg(
                1,
                "Fetch.enable",
                Some(json!({
                    "patterns": [
                        {"urlPattern": "https://example.com/*", "requestStage": "Response"},
                        {"resourceType": "Document"}
                    ]
                })),
            );
            let params = msg.params.clone();
            let resp = handle_command(msg, "t1", &params, Some(bridge));
            assert!(
                resp.error.is_none(),
                "valid patterns should arm the slot: {:?}",
                resp.error
            );
        });
        match cmd {
            BridgeCommand::FetchEnable {
                target_id,
                patterns,
                legacy_network_events,
            } => {
                assert_eq!(target_id, "t1");
                assert!(!legacy_network_events, "Fetch domain emits requestPaused");
                assert_eq!(patterns.len(), 2);
                assert_eq!(
                    patterns[0].url_pattern.as_deref(),
                    Some("https://example.com/*")
                );
                assert_eq!(patterns[0].resource_type, None);
                assert_eq!(patterns[0].request_stage, FetchRequestStage::Response);
                assert_eq!(patterns[1].url_pattern, None);
                assert_eq!(patterns[1].resource_type.as_deref(), Some("Document"));
                // CDP default: requestStage omitted → Request.
                assert_eq!(patterns[1].request_stage, FetchRequestStage::Request);
            }
            other => panic!("expected FetchEnable, got {other:?}"),
        }
    }

    #[test]
    fn fetch_enable_without_patterns_normalizes_catch_all() {
        // Chromium omitted-patterns semantics: intercept everything at the
        // Request stage → normalized to one unconstrained Request pattern.
        let cmd = capture_bridge_command(|bridge| {
            let msg = cdp_msg(2, "Fetch.enable", None);
            let params = msg.params.clone();
            let resp = handle_command(msg, "t1", &params, Some(bridge));
            assert!(resp.error.is_none(), "{:?}", resp.error);
        });
        match cmd {
            BridgeCommand::FetchEnable { patterns, .. } => {
                assert_eq!(patterns.len(), 1);
                assert_eq!(patterns[0].url_pattern, None);
                assert_eq!(patterns[0].resource_type, None);
                assert_eq!(patterns[0].request_stage, FetchRequestStage::Request);
            }
            other => panic!("expected FetchEnable, got {other:?}"),
        }
    }

    #[test]
    fn fetch_enable_handle_auth_requests_is_explicit_error() {
        // design §7: the auth facet is not wired — enabling it would
        // silently never emit authRequired, which is a lie, not a noop.
        expect_dispatch_error(
            3,
            "Fetch.enable",
            Some(json!({"handleAuthRequests": true})),
            -32602,
            "handleAuthRequests=true is not supported",
        );
    }

    #[test]
    fn fetch_enable_malformed_patterns_are_32602() {
        let cases: Vec<(&str, Value)> = vec![
            ("array", json!({"patterns": "*"})),
            ("object", json!({"patterns": ["*"]})),
            ("string", json!({"patterns": [{"urlPattern": 5}]})),
            ("non-empty", json!({"patterns": [{"resourceType": ""}]})),
            ("Request", json!({"patterns": [{"requestStage": "Headers"}]})),
        ];
        for (i, (fragment, params)) in cases.into_iter().enumerate() {
            expect_dispatch_error(
                10 + i as i64,
                "Fetch.enable",
                Some(params),
                -32602,
                fragment,
            );
        }
    }

    #[test]
    fn fetch_disable_without_bridge_is_32603() {
        // Real command: the slot + pending drains live browser-side.
        expect_dispatch_error(20, "Fetch.disable", None, -32603, "no servo bridge connected");
    }

    #[test]
    fn fetch_continue_request_modification_fields_are_32602() {
        // design §7: the servo pause face can only pass through or answer
        // with a synthesized response — modification params must not be
        // silently ignored.
        for (i, field) in ["url", "method", "postData", "headers"].into_iter().enumerate() {
            let params = json!({"interceptionId": "i1", (field): "x"});
            expect_dispatch_error(
                30 + i as i64,
                "Fetch.continueRequest",
                Some(params),
                -32602,
                "request modification is not supported",
            );
        }
    }

    #[test]
    fn fetch_resolution_missing_interception_id_is_32602() {
        for (i, method) in ["Fetch.continueRequest", "Fetch.failRequest", "Fetch.fulfillRequest"]
            .iter()
            .enumerate()
        {
            expect_dispatch_error(
                40 + i as i64,
                method,
                Some(json!({"requestId": "not-the-resolution-key"})),
                -32602,
                "missing required 'interceptionId'",
            );
        }
    }

    #[test]
    fn fetch_fulfill_request_param_validation_is_32602() {
        let cases: Vec<(&str, Value)> = vec![
            ("missing required 'responseCode'", json!({"interceptionId": "i"})),
            (
                "'responseCode' must be an integer",
                json!({"interceptionId": "i", "responseCode": "404"}),
            ),
            (
                "'responseHeaders' must be an object",
                json!({"interceptionId": "i", "responseCode": 200, "responseHeaders": []}),
            ),
            (
                "'responseHeaders' values must be strings",
                json!({"interceptionId": "i", "responseCode": 200, "responseHeaders": {"a": 1}}),
            ),
            (
                "'body' must be a base64 string",
                json!({"interceptionId": "i", "responseCode": 200, "body": 42}),
            ),
        ];
        for (i, (fragment, params)) in cases.into_iter().enumerate() {
            expect_dispatch_error(
                50 + i as i64,
                "Fetch.fulfillRequest",
                Some(params),
                -32602,
                fragment,
            );
        }
    }

    #[test]
    fn fetch_fulfill_request_oversized_body_is_explicit_error() {
        // design §7: past the 8 MiB cap the error is explicit — never a
        // silent truncation. The base64 length bound is the protocol-layer
        // pre-check (the browser layer re-checks decoded size).
        let oversized = "A".repeat(8 * 1024 * 1024 * 2);
        expect_dispatch_error(
            60,
            "Fetch.fulfillRequest",
            Some(json!({"interceptionId": "i", "responseCode": 200, "body": oversized})),
            -32602,
            "exceeds the 8 MiB cap",
        );
    }

    #[test]
    fn fetch_resolution_commands_carry_bridge_action_shapes() {
        // continue → the pass-through verdict.
        let cmd = capture_bridge_command(|bridge| {
            let msg =
                cdp_msg(70, "Fetch.continueRequest", Some(json!({"interceptionId": "i-7"})));
            let params = msg.params.clone();
            handle_command(msg, "t1", &params, Some(bridge));
        });
        match cmd {
            BridgeCommand::FetchResolve {
                target_id,
                interception_id,
                action: FetchResolveAction::Continue,
            } => {
                assert_eq!(target_id, "t1");
                assert_eq!(interception_id, "i-7");
            }
            other => panic!("expected FetchResolve(Continue), got {other:?}"),
        }
        // fail → advisory error reason rides along.
        let cmd = capture_bridge_command(|bridge| {
            let msg = cdp_msg(
                71,
                "Fetch.failRequest",
                Some(json!({"interceptionId": "i-8", "errorReason": "Failed"})),
            );
            let params = msg.params.clone();
            handle_command(msg, "t1", &params, Some(bridge));
        });
        match cmd {
            BridgeCommand::FetchResolve {
                action: FetchResolveAction::Fail { error_reason },
                ..
            } => assert_eq!(error_reason.as_deref(), Some("Failed")),
            other => panic!("expected FetchResolve(Fail), got {other:?}"),
        }
        // fulfill → status / phrase / headers / base64 body verbatim (the
        // browser layer owns base64 decoding and the decoded-size cap).
        let cmd = capture_bridge_command(|bridge| {
            let msg = cdp_msg(
                72,
                "Fetch.fulfillRequest",
                Some(json!({
                    "interceptionId": "i-9",
                    "responseCode": 200,
                    "responsePhrase": "OK",
                    "responseHeaders": {"content-type": "text/html"},
                    "body": "dGVzdA=="
                })),
            );
            let params = msg.params.clone();
            handle_command(msg, "t1", &params, Some(bridge));
        });
        match cmd {
            BridgeCommand::FetchResolve {
                target_id,
                interception_id,
                action: FetchResolveAction::Fulfill {
                    response_code,
                    status_message,
                    response_headers,
                    body,
                },
            } => {
                assert_eq!(target_id, "t1");
                assert_eq!(interception_id, "i-9");
                assert_eq!(response_code, 200);
                assert_eq!(status_message.as_deref(), Some("OK"));
                assert_eq!(
                    response_headers.unwrap()["content-type"],
                    json!("text/html")
                );
                assert_eq!(body.as_deref(), Some("dGVzdA=="));
            }
            other => panic!("expected FetchResolve(Fulfill), got {other:?}"),
        }
    }

    #[test]
    fn legacy_network_set_request_interception_maps_to_fetch_bridge() {
        // POP6-C6 criterion wording: the legacy Network face shares the
        // Fetch facility; hits emit Network.requestIntercepted, so the
        // bridge command carries the legacy-event flag (design §5).
        let cmd = capture_bridge_command(|bridge| {
            let msg = cdp_msg(
                80,
                "Network.setRequestInterception",
                Some(json!({"patterns": [{"urlPattern": "*", "interceptionStage": "Request"}]})),
            );
            let params = msg.params.clone();
            let resp = handle_command(msg, "t1", &params, Some(bridge));
            assert!(resp.error.is_none(), "{:?}", resp.error);
        });
        match cmd {
            BridgeCommand::FetchEnable {
                patterns,
                legacy_network_events,
                ..
            } => {
                assert!(legacy_network_events, "legacy face emits requestIntercepted");
                assert_eq!(patterns.len(), 1);
                assert_eq!(patterns[0].url_pattern.as_deref(), Some("*"));
                assert_eq!(patterns[0].request_stage, FetchRequestStage::Request);
            }
            other => panic!("expected FetchEnable, got {other:?}"),
        }
    }

    #[test]
    fn legacy_network_set_request_interception_rejects_headers_received_stage() {
        // The servo pause point is request-time only.
        expect_dispatch_error(
            81,
            "Network.setRequestInterception",
            Some(json!({"patterns": [{"urlPattern": "*", "interceptionStage": "HeadersReceived"}]})),
            -32602,
            "request-time only",
        );
    }

    #[test]
    fn legacy_network_continue_intercepted_request_maps_to_fetch_bridge() {
        // Plain continue → Continue verdict through the same dispatch.
        let cmd = capture_bridge_command(|bridge| {
            let msg = cdp_msg(
                82,
                "Network.continueInterceptedRequest",
                Some(json!({"interceptionId": "i-10"})),
            );
            let params = msg.params.clone();
            handle_command(msg, "t1", &params, Some(bridge));
        });
        match cmd {
            BridgeCommand::FetchResolve {
                action: FetchResolveAction::Continue,
                ..
            } => {}
            other => panic!("expected FetchResolve(Continue), got {other:?}"),
        }
        // errorReason → Fail verdict.
        let cmd = capture_bridge_command(|bridge| {
            let msg = cdp_msg(
                83,
                "Network.continueInterceptedRequest",
                Some(json!({"interceptionId": "i-11", "errorReason": "Aborted"})),
            );
            let params = msg.params.clone();
            handle_command(msg, "t1", &params, Some(bridge));
        });
        match cmd {
            BridgeCommand::FetchResolve {
                action: FetchResolveAction::Fail { error_reason },
                ..
            } => assert_eq!(error_reason.as_deref(), Some("Aborted")),
            other => panic!("expected FetchResolve(Fail), got {other:?}"),
        }
    }

    #[test]
    fn legacy_network_continue_intercepted_request_boundaries() {
        // rawResponse (raw HTTP wire-format reassembly) is phase-2.
        expect_dispatch_error(
            84,
            "Network.continueInterceptedRequest",
            Some(json!({"interceptionId": "i", "rawResponse": "SFRUUC8xLjEgMjAw"})),
            -32000,
            "raw HTTP wire-format response reassembly is phase-2",
        );
        // Request modification shares the Fetch.continueRequest boundary.
        expect_dispatch_error(
            85,
            "Network.continueInterceptedRequest",
            Some(json!({"interceptionId": "i", "url": "https://example.com/"})),
            -32602,
            "request modification is not supported",
        );
        // Missing resolution key.
        expect_dispatch_error(
            86,
            "Network.continueInterceptedRequest",
            None,
            -32602,
            "missing required 'interceptionId'",
        );
    }

    #[test]
    fn network_emulate_network_conditions_stays_accepted_noop() {
        // Unrelated to interception — unchanged ok_empty (design §5).
        let resp = dispatch_no_bridge(87, "Network.emulateNetworkConditions", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    #[test]
    fn fetch_fail_request_non_string_error_reason_is_32602() {
        expect_dispatch_error(
            88,
            "Fetch.failRequest",
            Some(json!({"interceptionId": "i", "errorReason": 7})),
            -32602,
            "'errorReason' must be a CDP ErrorReason string",
        );
    }
}
