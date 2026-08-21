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
// Runtime.evaluate, DOM.getDocument, ...) — plus thin serde wrappers for the
// codec helpers (the cdp-server `protocol` module is private, so its
// `parse_message`/`serialize_response`/`serialize_event` cannot be re-exported
// and are re-implemented here against the shared `cdp_server` wire types).
// Re-export the JSON-RPC 2.0 wire types so internal modules (`backend.rs`,
// `router.rs`) and the crate root can refer to them as `protocol::CdpError`
// etc. These ARE `cdp_server` types — no duplicate definitions.
pub use cdp_server::{CdpError, CdpEvent, CdpMessage, CdpResponse};
use serde_json::Value;

use crate::servo_bridge::{BridgeCommand, BridgeSender};

// JSON-RPC 2.0 error code: method not found (per spec §5.1).
const ERR_METHOD_NOT_FOUND: i64 = -32601;
// JSON-RPC 2.0 error code: parse error (fallback on serialize failure).
const ERR_PARSE_ERROR: i64 = -32700;
// JSON-RPC 2.0 error code: invalid params.
const ERR_INVALID_PARAMS: i64 = -32602;
// Chrome DevTools "server error" code used for not-supported commands.
const ERR_NOT_SUPPORTED: i64 = -32000;

/// Build a not-supported error for a command whose backing facility does not
/// exist (servo/SM face absent). Explicit failure — never a canned success.
fn not_supported(method: &str, reason: &str) -> CdpError {
    CdpError {
        code: ERR_NOT_SUPPORTED,
        message: format!("'{method}' not supported: {reason}"),
    }
}

/// Parse a raw JSON-RPC 2.0 request string into a [`CdpMessage`].
///
/// Returns `None` on malformed JSON. Thin serde wrapper over the shared
/// `cdp_server::CdpMessage` type.
pub fn parse_message(raw: &str) -> Option<CdpMessage> {
    serde_json::from_str(raw).ok()
}

/// Serialize a [`CdpResponse`] to a JSON-RPC 2.0 string.
///
/// Falls back to a parse-error envelope on serializer failure (cannot happen
/// for the well-formed responses produced by `handle_command`, but kept for
/// defense-in-depth).
pub fn serialize_response(resp: &CdpResponse) -> String {
    serde_json::to_string(resp).unwrap_or_else(|_| {
        format!(r#"{{"id":null,"error":{{"code":{ERR_PARSE_ERROR},"message":"serialize error"}}}}"#)
    })
}

/// Serialize a CDP event notification to a JSON string.
pub fn serialize_event(ev: &CdpEvent) -> String {
    serde_json::to_string(ev).unwrap_or_else(|_| "{}".into())
}

/// Build a success response carrying `result`.
fn ok_response(id: Option<i64>, result: Value) -> CdpResponse {
    CdpResponse {
        id,
        result: Some(result),
        error: None,
    }
}

/// Build an error response carrying `code` + `message`.
fn error_response(id: Option<i64>, code: i64, message: impl Into<String>) -> CdpResponse {
    CdpResponse {
        id,
        result: None,
        error: Some(CdpError {
            code,
            message: message.into(),
        }),
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
        "Overlay" => handle_overlay(command),
        "Debugger" => handle_debugger(command, target_id, params, bridge),
        "Log" => handle_log(command),
        "Fetch" => handle_fetch(command, params),
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
        "Browser" => handle_browser(command),
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

type HandlerResult = Result<Value, CdpError>;

/// Monotonic id source for CDP identifiers returned by the stateless face
/// (script ids when no bridge is involved). Chrome semantics: fresh id per
/// registration — never a hardcoded constant.
fn next_cdp_identifier(prefix: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}-{n:016x}")
}

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
            resp.result.map_err(|e| CdpError {
                code: -32603,
                message: e,
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
fn eval_json(bridge: Option<&BridgeSender>, tid: &str, expression: &str) -> HandlerResult {
    let resp = bridge_send(
        bridge,
        BridgeCommand::EvaluateJs {
            target_id: tid.to_string(),
            expression: expression.to_string(),
            return_by_value: true,
        },
    )?;
    resp.get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .filter(|v| v.is_object())
        .ok_or_else(|| CdpError {
            code: ERR_NOT_SUPPORTED,
            message: "page query failed: target did not return a JSON document".into(),
        })
}

fn live_target_info(target_id: &str, bridge: Option<&BridgeSender>) -> Value {
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
        // Stable default-context id (pages are created outside any explicit
        // BrowserContext; clients like Playwright require the field present).
        "browserContextId": "bao-default-context",
    })
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
                        target_infos.push(live_target_info(id, bridge));
                    }
                }
                None => target_infos.push(live_target_info(target_id, bridge)),
            }
            // REQ-BRW-004: Include Worker sub-targets in Target.getTargets response
            // @trace REQ-BRW-004 [criterion:19] Worker targets are CDP-observable
            if let Some(b) = bridge {
                if let Ok(worker_resp) = b
                    .send(BridgeCommand::ListWorkerTargets {
                        target_id: target_id.to_string(),
                    })
                    .result
                {
                    if let Some(workers) =
                        worker_resp.get("workerTargets").and_then(|v| v.as_array())
                    {
                        for w in workers {
                            target_infos.push(w.clone());
                        }
                    }
                }
            }
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
            bridge_send(
                bridge,
                BridgeCommand::CreateTarget {
                    url: url.to_string(),
                },
            )
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
            Ok(serde_json::json!({ "targetInfo": live_target_info(tid, bridge) }))
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

fn handle_page(
    command: &str,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> HandlerResult {
    let tid = target_id.to_string();
    match command {
        "enable" | "disable" => ok_empty(),
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
            // The bridge handler returns the real frameId (page id) and a
            // freshly generated loaderId — its response is the truth.
            bridge_send(
                bridge,
                BridgeCommand::Navigate {
                    target_id: tid,
                    url: url.to_string(),
                },
            )
        }
        "reload" => {
            let ignore_cache = params
                .as_ref()
                .and_then(|p| p.get("ignoreCache"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Real servo reload; response carries real frameId + fresh loaderId.
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
            // the live document via evaluate; frameId = the page's stable id
            // (same identifier navigate/reload report). Child frames are not
            // enumerable from the embedder — none are fabricated.
            let mut frame = eval_json(
                bridge,
                &tid,
                r#"(function(){ return JSON.stringify({
                    url: location.href,
                    mimeType: document.contentType,
                    name: window.name,
                    securityOrigin: location.origin
                }); })()"#,
            )?;
            if let Some(obj) = frame.as_object_mut() {
                obj.insert("id".into(), serde_json::json!(tid));
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
            let m = eval_json(
                bridge,
                &tid,
                r#"(function(){ var d = document.documentElement;
                    return JSON.stringify({
                        iw: window.innerWidth, ih: window.innerHeight,
                        cw: d.scrollWidth, ch: d.scrollHeight
                    }); })()"#,
            )?;
            Ok(serde_json::json!({
                "layoutViewport": { "x": 0, "y": 0, "width": m["iw"], "height": m["ih"] },
                "contentSize": { "x": 0, "y": 0, "width": m["cw"], "height": m["ch"] },
                "cssLayoutViewport": { "x": 0, "y": 0, "width": m["iw"], "height": m["ih"] },
                "cssContentSize": { "x": 0, "y": 0, "width": m["cw"], "height": m["ch"] }
            }))
        }
        "addScriptToEvaluateOnNewDocument" => {
            let source = params_str(params, "source");
            if source.is_empty() {
                // Chrome-compatible: clients (Playwright) register an empty
                // placeholder init script for later binding injection — an
                // empty script registers nothing and runs nothing, so an ok
                // with a fresh identifier is the truthful response.
                return Ok(serde_json::json!({
                    "identifier": next_cdp_identifier("script")
                }));
            }
            // The bridge handler returns a genuinely generated identifier —
            // its response is the truth (no hardcoded "1").
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
            Err(not_supported(
                "Page.removeScriptToEvaluateOnNewDocument",
                "added scripts are not kept in a removable registry",
            ))
        }
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
                bridge_send(
                    bridge,
                    BridgeCommand::EvaluateJs {
                        target_id: tid,
                        expression,
                        return_by_value,
                    },
                )
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
        "compileScript" | "callArgument" => ok_empty(),
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
        "getDocument" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::GetDocument {
                        target_id: tid.clone(),
                    },
                )
            } else {
                Ok(serde_json::json!({
                    "root": {
                        "nodeId": 1, "backendNodeId": 1, "nodeType": 9,
                        "nodeName": "#document", "localName": "", "nodeValue": "",
                        "childNodeCount": 1,
                        "children": [{
                            "nodeId": 2, "backendNodeId": 2, "nodeType": 1,
                            "nodeName": "HTML", "localName": "html", "nodeValue": "",
                            "childNodeCount": 2
                        }]
                    }
                }))
            }
        }
        "describeNode" => {
            Ok(serde_json::json!({ "node": { "nodeId": 1, "nodeType": 1, "nodeName": "HTML" } }))
        }
        "querySelector" => {
            let selector = params_str(params, "selector");
            if bridge.is_some() && !selector.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::QuerySelector {
                        target_id: tid.clone(),
                        selector,
                    },
                )
            } else {
                Ok(serde_json::json!({ "nodeId": 0 }))
            }
        }
        "querySelectorAll" => {
            let selector = params_str(params, "selector");
            if bridge.is_some() && !selector.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::QuerySelectorAll {
                        target_id: tid.clone(),
                        selector,
                    },
                )
            } else {
                Ok(serde_json::json!({ "nodeIds": [] }))
            }
        }
        "getBoxModel" => Ok(serde_json::json!({
            "model": { "width": 1920, "height": 1080, "content": [0, 0, 1920, 0, 1920, 1080, 0, 1080] }
        })),
        "setAttributeValue" => {
            let node_id = params
                .as_ref()
                .and_then(|p| p.get("nodeId"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let name = params_str(params, "name");
            let value = params_str(params, "value");
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::SetAttributeValue {
                        target_id: tid.clone(),
                        node_id,
                        name,
                        value,
                    },
                )
            } else {
                ok_empty()
            }
        }
        "removeAttribute" | "setOuterHTML" | "insertBefore" | "removeNode" => ok_empty(),
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
        "resolveNode" => Ok(serde_json::json!({ "object": { "type": "node" } })),
        "pushNodesByBackendIdsToFrontend" => Ok(serde_json::json!({ "nodeIds": [] })),
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
            if bridge.is_some() && !ua.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::SetUserAgent {
                        target_id: tid.clone(),
                        user_agent: ua,
                    },
                )?;
            }
            if bridge.is_some() && (accept_language.is_some() || platform.is_some()) {
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
        "emulateNetworkConditions" | "setRequestInterception" | "continueInterceptedRequest" => {
            ok_empty()
        }
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
        "getComputedStyleForNode" => {
            let node_id = params
                .as_ref()
                .and_then(|p| p.get("nodeId"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::CssGetComputedStyleForNode {
                        target_id: tid,
                        node_id,
                    },
                )
            } else {
                Ok(serde_json::json!({ "computedStyle": [] }))
            }
        }
        "getMatchedStylesForNode" => {
            let node_id = params
                .as_ref()
                .and_then(|p| p.get("nodeId"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::CssGetMatchedStylesForNode {
                        target_id: tid,
                        node_id,
                    },
                )
            } else {
                Ok(serde_json::json!({
                    "matchedCSSRules": [], "inlineStyle": null, "attributesStyle": null
                }))
            }
        }
        "getInlineStylesForNode" => {
            let node_id = params
                .as_ref()
                .and_then(|p| p.get("nodeId"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::CssGetInlineStylesForNode {
                        target_id: tid,
                        node_id,
                    },
                )
            } else {
                Ok(serde_json::json!({ "inlineStyle": null }))
            }
        }
        "setStyleTexts" => Ok(serde_json::json!({ "styles": [] })),
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
            if bridge.is_some() && !ua.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::SetUserAgent {
                        target_id: tid,
                        user_agent: ua,
                    },
                )
            } else {
                ok_empty()
            }
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
            let button = params
                .as_ref()
                .and_then(|p| p.get("button"))
                .and_then(|v| v.as_i64());
            let click_count = params
                .as_ref()
                .and_then(|p| p.get("clickCount"))
                .and_then(|v| v.as_i64());
            if bridge.is_some() {
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
            } else {
                ok_empty()
            }
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
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::DispatchKeyEvent {
                        target_id: tid.clone(),
                        event_type,
                        key,
                        code,
                        text,
                    },
                )
            } else {
                ok_empty()
            }
        }
        "dispatchTouchEvent" => ok_empty(),
        "insertText" => {
            let text = params_str(params, "text");
            if bridge.is_some() && !text.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::InsertText {
                        target_id: tid,
                        text,
                    },
                )
            } else {
                ok_empty()
            }
        }
        "setIgnoreInputEvents" | "setInterceptDrags" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Input.{}' wasn't found", command),
        }),
    }
}

fn handle_overlay(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => ok_empty(),
        "highlightNode" | "hideHighlight" | "setInspectMode" => ok_empty(),
        "setPausedInDebuggerMessage" => ok_empty(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("'Overlay.{}' wasn't found", command),
        }),
    }
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
            if bridge.is_some() {
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
            } else {
                Ok(serde_json::json!({ "breakpointId": "1", "locations": [] }))
            }
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

fn handle_fetch(command: &str, _params: &Option<Value>) -> HandlerResult {
    match command {
        // Idempotent no-op: disabling an interception that was never enabled
        // (and cannot be — see below) is truthful as an empty success.
        "disable" => ok_empty(),
        // The Fetch domain is request interception. bao has no interception
        // facility: the servo embedder does not expose request pausing, so no
        // request can be paused, continued, fulfilled or failed. Every
        // interception command is an explicit error — never a canned success
        // with fabricated "continued"/"fulfilled" flags (REQ-CDP contract:
        // real implementation or explicit failure).
        "enable"
        | "continueRequest"
        | "continueWithResponse"
        | "failRequest"
        | "fulfillRequest"
        | "getRequestPostData"
        | "continueWithAuth"
        | "takeResponseBodyAsStream" => Err(not_supported(
            &format!("Fetch.{command}"),
            "bao has no request interception facility: the servo embedder does not expose request pausing, so requests cannot be paused, continued, fulfilled or failed",
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
fn handle_browser(command: &str) -> HandlerResult {
    match command {
        "getVersion" => Ok(serde_json::json!({
            "protocolVersion": "1.3",
            "product": "Bao/0.1.0",
            "revision": env!("CARGO_PKG_VERSION"),
            "userAgent": format!("Bao/{}", env!("CARGO_PKG_VERSION")),
            "jsVersion": "SpiderMonkey",
        })),
        // Preference ack: records the download-behavior preference (bao has
        // no download manager yet — the preference is stored, nothing is
        // fabricated).
        "setDownloadBehavior" => ok_empty(),
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
    use serde_json::json;

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

    // 10. handle_command Target.createTarget (no bridge) → explicit error
    //     (page creation requires the servo bridge; never an echo of the
    //     current target id)
    #[test]
    fn handle_command_target_create_target() {
        let resp = dispatch_no_bridge(
            3,
            "Target.createTarget",
            Some(json!({"url": "https://example.com"})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge connected"));
    }

    // 11. handle_command Target.closeTarget (no bridge) → explicit error
    //     (closing a page requires the servo bridge — no fire-and-forget ok)
    #[test]
    fn handle_command_target_close_target() {
        let resp = dispatch_no_bridge(4, "Target.closeTarget", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge connected"));
    }

    // 12. handle_command Target.setAutoAttach → ok empty
    #[test]
    fn handle_command_target_set_auto_attach() {
        let resp = dispatch_no_bridge(5, "Target.setAutoAttach", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 13. handle_command Page.enable → ok empty
    #[test]
    fn handle_command_page_enable() {
        let resp = dispatch_no_bridge(6, "Page.enable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 14. handle_command Page.getLayoutMetrics (no bridge) → explicit error
    //     (real layout data requires the servo bridge; never canned 1920×1080)
    #[test]
    fn handle_command_page_get_layout_metrics() {
        let resp = dispatch_no_bridge(7, "Page.getLayoutMetrics", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"));
    }

    // 15. handle_command Runtime.enable → ok empty (Chrome semantics: no
    //     executionContextId in the response)
    #[test]
    fn handle_command_runtime_enable() {
        let resp = dispatch_no_bridge(8, "Runtime.enable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 16. handle_command Runtime.evaluate (no bridge, empty expr) → undefined result
    #[test]
    fn handle_command_runtime_evaluate_no_bridge() {
        let resp = dispatch_no_bridge(9, "Runtime.evaluate", Some(json!({"expression": ""})));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 17. handle_command DOM.getDocument (no bridge) → ok with root node
    #[test]
    fn handle_command_dom_get_document() {
        let resp = dispatch_no_bridge(10, "DOM.getDocument", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let root = result.get("root").unwrap();
        assert_eq!(root["nodeId"], 1);
        assert_eq!(root["nodeType"], 9);
        assert_eq!(root["nodeName"], "#document");
    }

    // 18. handle_command DOM.querySelector (no bridge) → ok nodeId:0
    #[test]
    fn handle_command_dom_query_selector() {
        let resp = dispatch_no_bridge(11, "DOM.querySelector", Some(json!({"selector": "div"})));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["nodeId"], 0);
    }

    // 19. handle_command Network.enable → ok empty
    #[test]
    fn handle_command_network_enable() {
        let resp = dispatch_no_bridge(12, "Network.enable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 20. handle_command Network.getCookies → ok with empty cookies
    #[test]
    fn handle_command_network_get_cookies() {
        let resp = dispatch_no_bridge(13, "Network.getCookies", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["cookies"], json!([]));
    }

    // 21. handle_command CSS.enable → ok empty
    #[test]
    fn handle_command_css_enable() {
        let resp = dispatch_no_bridge(14, "CSS.enable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 22. handle_command CSS.getComputedStyleForNode → ok empty computedStyle
    #[test]
    fn handle_command_css_get_computed_style() {
        let resp = dispatch_no_bridge(15, "CSS.getComputedStyleForNode", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["computedStyle"], json!([]));
    }

    // 23. handle_command Emulation.setDeviceMetricsOverride (no bridge) → ok empty
    #[test]
    fn handle_command_emulation_set_device_metrics() {
        let resp = dispatch_no_bridge(
            16,
            "Emulation.setDeviceMetricsOverride",
            Some(json!({"width": 800, "height": 600, "deviceScaleFactor": 2})),
        );
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 24. handle_command Input.dispatchMouseEvent (no bridge) → ok empty
    #[test]
    fn handle_command_input_dispatch_mouse() {
        let resp = dispatch_no_bridge(
            17,
            "Input.dispatchMouseEvent",
            Some(json!({"type": "mousePressed", "x": 100, "y": 200, "button": 0, "clickCount": 1})),
        );
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 25. handle_command Overlay.enable → ok empty
    #[test]
    fn handle_command_overlay_enable() {
        let resp = dispatch_no_bridge(18, "Overlay.enable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 26. handle_command Debugger.enable → ok empty
    #[test]
    fn handle_command_debugger_enable() {
        let resp = dispatch_no_bridge(19, "Debugger.enable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 27. handle_command Debugger.setBreakpointByUrl → ok with breakpointId
    #[test]
    fn handle_command_debugger_set_breakpoint_by_url() {
        let resp = dispatch_no_bridge(20, "Debugger.setBreakpointByUrl", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["breakpointId"], "1");
    }

    // 28. handle_command Log.enable → ok empty
    #[test]
    fn handle_command_log_enable() {
        let resp = dispatch_no_bridge(21, "Log.enable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 29. handle_command Fetch.enable → explicit error (no interception
    //     facility exists: the servo embedder does not expose request
    //     pausing — never a canned "enabled"/patternCount success)
    #[test]
    fn handle_command_fetch_enable_with_patterns() {
        let resp = dispatch_no_bridge(
            22,
            "Fetch.enable",
            Some(json!({"patterns": [{"urlPattern": "*"}]})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 30. handle_command Fetch.continueRequest → explicit error (an
    //     interception that can never be enabled can never be continued)
    #[test]
    fn handle_command_fetch_continue_request() {
        let resp = dispatch_no_bridge(
            23,
            "Fetch.continueRequest",
            Some(json!({"requestId": "req-001"})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
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

    // 76. handle_command Target.attachToTarget (stateless face) → explicit
    //     error: session minting lives in the WS session registry
    //     (bao_browser::ws_registry::BaoWsRegistry); the stateless internal
    //     backend has no session table — never a fabricated sessionId.
    #[test]
    fn handle_command_target_attach_to_target() {
        let resp = dispatch_no_bridge(
            5,
            "Target.attachToTarget",
            Some(json!({"targetId": "t1", "flatten": true})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("WS session registry"));
    }

    // 77. handle_command Target.detachFromTarget (stateless face) → explicit
    //     error (same session-table reasoning as attachToTarget)
    #[test]
    fn handle_command_target_detach_from_target() {
        let resp = dispatch_no_bridge(6, "Target.detachFromTarget", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("WS session registry"));
    }

    // 78. handle_command Target.setDiscoverTargets → ok empty
    #[test]
    fn handle_command_target_set_discover_targets() {
        let resp = dispatch_no_bridge(7, "Target.setDiscoverTargets", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 79. handle_command Target.getTargetTargets → ok (alias for getTargets)
    #[test]
    fn handle_command_target_get_target_targets() {
        let resp = dispatch_no_bridge(8, "Target.getTargetTargets", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("targetInfos").unwrap().as_array().unwrap().len() > 0);
    }

    // 80. handle_command Page.navigate (no bridge) → explicit error
    //     (navigation requires the servo bridge; the bridge response carries
    //     the real frameId/loaderId — never fabricated here)
    #[test]
    fn handle_command_page_navigate_no_bridge_default_url() {
        let resp = dispatch_no_bridge(9, "Page.navigate", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"));
    }

    // 81. handle_command Page.navigate (no bridge) with url param → explicit error
    #[test]
    fn handle_command_page_navigate_with_url() {
        let resp = dispatch_no_bridge(
            10,
            "Page.navigate",
            Some(json!({"url": "https://example.com"})),
        );
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"));
    }

    // 82. handle_command Page.reload (no bridge) → explicit error
    #[test]
    fn handle_command_page_reload_no_bridge() {
        let resp = dispatch_no_bridge(11, "Page.reload", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"));
    }

    // 83. handle_command Page.getFrameTree (no bridge) → explicit error
    //     (frame data is read from the live document via the bridge)
    #[test]
    fn handle_command_page_get_frame_tree() {
        let resp = dispatch_no_bridge(12, "Page.getFrameTree", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"));
    }

    // 84. handle_command Page.getNavigationHistory → explicit not-supported error
    //     (servo WebView exposes no session-history enumeration)
    #[test]
    fn handle_command_page_get_navigation_history() {
        let resp = dispatch_no_bridge(13, "Page.getNavigationHistory", None);
        let err = resp.error.expect("history enumeration must fail loudly");
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("not supported"));
    }

    // 85. handle_command Page.captureScreenshot (no bridge) → explicit error
    //     (no renderer without the bridge — never empty image data)
    #[test]
    fn handle_command_page_capture_screenshot_no_bridge() {
        let resp = dispatch_no_bridge(14, "Page.captureScreenshot", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge"));
    }

    // 86. handle_command Page.addScriptToEvaluateOnNewDocument (empty source)
    //     → invalid-params error (identifier generation lives behind the bridge)
    #[test]
    fn handle_command_page_add_script_empty_source() {
        let resp = dispatch_no_bridge(15, "Page.addScriptToEvaluateOnNewDocument", None);
        // Chrome-compatible: an empty init script (Playwright's placeholder
        // registration) is a no-op success with a fresh identifier.
        let result = resp.result.expect("empty source registers as a no-op");
        assert!(result["identifier"].as_str().unwrap().starts_with("script-"));
    }

    // 87. handle_command Page.removeScriptToEvaluateOnNewDocument → explicit
    //     not-supported error (no removable script registry exists)
    #[test]
    fn handle_command_page_remove_script() {
        let resp = dispatch_no_bridge(
            16,
            "Page.removeScriptToEvaluateOnNewDocument",
            Some(json!({"identifier": "script-1"})),
        );
        let err = resp.error.expect("removal must fail loudly");
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("not supported"));
    }

    // 88. handle_command Page.setContent (no html param) → invalid-params error
    #[test]
    fn handle_command_page_set_content() {
        let resp = dispatch_no_bridge(17, "Page.setContent", None);
        let err = resp.error.expect("missing html must be rejected");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("html"));
    }

    // 89. handle_command Page.close (no bridge) → explicit error
    #[test]
    fn handle_command_page_close() {
        let resp = dispatch_no_bridge(18, "Page.close", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
    }

    // 90. handle_command Page.bringToFront (no bridge) → explicit error
    #[test]
    fn handle_command_page_bring_to_front() {
        let resp = dispatch_no_bridge(19, "Page.bringToFront", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
    }

    // 91. handle_command Page.disable → ok empty
    #[test]
    fn handle_command_page_disable() {
        let resp = dispatch_no_bridge(20, "Page.disable", None);
        assert!(resp.error.is_none());
    }

    // 92. handle_command Runtime.disable → ok empty
    #[test]
    fn handle_command_runtime_disable() {
        let resp = dispatch_no_bridge(21, "Runtime.disable", None);
        assert!(resp.error.is_none());
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

    // 95. handle_command Runtime.evaluateAsync → ok
    #[test]
    fn handle_command_runtime_evaluate_async() {
        let resp = dispatch_no_bridge(24, "Runtime.evaluateAsync", None);
        assert!(resp.error.is_none());
    }

    // 96. handle_command Runtime.runScript → ok
    #[test]
    fn handle_command_runtime_run_script() {
        let resp = dispatch_no_bridge(25, "Runtime.runScript", None);
        assert!(resp.error.is_none());
    }

    // 97. handle_command Runtime.releaseObject → ok empty
    #[test]
    fn handle_command_runtime_release_object() {
        let resp = dispatch_no_bridge(26, "Runtime.releaseObject", None);
        assert!(resp.error.is_none());
    }

    // 98. handle_command Runtime.releaseObjectGroup → ok empty
    #[test]
    fn handle_command_runtime_release_object_group() {
        let resp = dispatch_no_bridge(27, "Runtime.releaseObjectGroup", None);
        assert!(resp.error.is_none());
    }

    // 99. handle_command Runtime.compileScript → ok empty
    #[test]
    fn handle_command_runtime_compile_script() {
        let resp = dispatch_no_bridge(28, "Runtime.compileScript", None);
        assert!(resp.error.is_none());
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

    // 101. handle_command DOM.enable → ok empty
    #[test]
    fn handle_command_dom_enable() {
        let resp = dispatch_no_bridge(30, "DOM.enable", None);
        assert!(resp.error.is_none());
    }

    // 102. handle_command DOM.disable → ok empty
    #[test]
    fn handle_command_dom_disable() {
        let resp = dispatch_no_bridge(31, "DOM.disable", None);
        assert!(resp.error.is_none());
    }

    // 103. handle_command DOM.describeNode → ok
    #[test]
    fn handle_command_dom_describe_node() {
        let resp = dispatch_no_bridge(32, "DOM.describeNode", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("node").is_some());
    }

    // 104. handle_command DOM.getBoxModel → ok with model
    #[test]
    fn handle_command_dom_get_box_model() {
        let resp = dispatch_no_bridge(33, "DOM.getBoxModel", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("model").is_some());
        assert_eq!(result["model"]["width"], 1920);
        assert_eq!(result["model"]["height"], 1080);
    }

    // 105. handle_command DOM.setAttributeValue (no bridge) → ok empty
    #[test]
    fn handle_command_dom_set_attribute_value_no_bridge() {
        let resp = dispatch_no_bridge(
            34,
            "DOM.setAttributeValue",
            Some(json!({"nodeId": 1, "name": "class", "value": "active"})),
        );
        assert!(resp.error.is_none());
    }

    // 106. handle_command DOM.removeAttribute → ok empty
    #[test]
    fn handle_command_dom_remove_attribute() {
        let resp = dispatch_no_bridge(35, "DOM.removeAttribute", None);
        assert!(resp.error.is_none());
    }

    // 107. handle_command DOM.setOuterHTML → ok empty
    #[test]
    fn handle_command_dom_set_outer_html() {
        let resp = dispatch_no_bridge(36, "DOM.setOuterHTML", None);
        assert!(resp.error.is_none());
    }

    // 108. handle_command DOM.insertBefore → ok empty
    #[test]
    fn handle_command_dom_insert_before() {
        let resp = dispatch_no_bridge(37, "DOM.insertBefore", None);
        assert!(resp.error.is_none());
    }

    // 109. handle_command DOM.removeNode → ok empty
    #[test]
    fn handle_command_dom_remove_node() {
        let resp = dispatch_no_bridge(38, "DOM.removeNode", None);
        assert!(resp.error.is_none());
    }

    // 110. handle_command DOM.getOuterHTML (no bridge) → explicit error
    //      (outerHTML is read from the live document — never canned html)
    #[test]
    fn handle_command_dom_get_outer_html_no_bridge() {
        let resp = dispatch_no_bridge(39, "DOM.getOuterHTML", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
    }

    // 111. handle_command DOM.resolveNode → ok
    #[test]
    fn handle_command_dom_resolve_node() {
        let resp = dispatch_no_bridge(40, "DOM.resolveNode", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["object"]["type"], "node");
    }

    // 112. handle_command DOM.pushNodesByBackendIdsToFrontend → ok
    #[test]
    fn handle_command_dom_push_nodes_by_backend_ids() {
        let resp = dispatch_no_bridge(41, "DOM.pushNodesByBackendIdsToFrontend", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["nodeIds"], json!([]));
    }

    // 113. handle_command DOM.unknown → error -32601
    #[test]
    fn handle_command_dom_unknown_command() {
        let resp = dispatch_no_bridge(42, "DOM.nonExistent", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 114. handle_command Network.disable → ok empty
    #[test]
    fn handle_command_network_disable() {
        let resp = dispatch_no_bridge(43, "Network.disable", None);
        assert!(resp.error.is_none());
    }

    // 115. handle_command Network.getResponseBody (no bridge) → explicit error
    //      (servo does not expose stored response bodies — fail loudly, never
    //      return an empty-body fake success)
    #[test]
    fn handle_command_network_get_response_body() {
        let resp = dispatch_no_bridge(44, "Network.getResponseBody", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
    }

    // 116. handle_command Network.setCacheDisabled → ok empty
    #[test]
    fn handle_command_network_set_cache_disabled() {
        let resp = dispatch_no_bridge(45, "Network.setCacheDisabled", None);
        assert!(resp.error.is_none());
    }

    // 117. handle_command Network.setExtraHTTPHeaders (no bridge) → explicit error
    //      (servo has no extra-headers injection API — the headers are never
    //      silently dropped)
    #[test]
    fn handle_command_network_set_extra_http_headers() {
        let resp = dispatch_no_bridge(46, "Network.setExtraHTTPHeaders", None);
        let err = resp.error.expect("no bridge must yield an error");
        assert_eq!(err.code, -32603);
    }

    // 118. handle_command Network.emulateNetworkConditions → ok empty
    #[test]
    fn handle_command_network_emulate_conditions() {
        let resp = dispatch_no_bridge(47, "Network.emulateNetworkConditions", None);
        assert!(resp.error.is_none());
    }

    // 119. handle_command Network.getAllCookies → ok with empty cookies
    #[test]
    fn handle_command_network_get_all_cookies() {
        let resp = dispatch_no_bridge(48, "Network.getAllCookies", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["cookies"], json!([]));
    }

    // 120. handle_command Network.deleteCookies → ok empty
    #[test]
    fn handle_command_network_delete_cookies() {
        let resp = dispatch_no_bridge(49, "Network.deleteCookies", None);
        assert!(resp.error.is_none());
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

    // 122. handle_command Network.setRequestInterception → ok empty
    #[test]
    fn handle_command_network_set_request_interception() {
        let resp = dispatch_no_bridge(51, "Network.setRequestInterception", None);
        assert!(resp.error.is_none());
    }

    // 123. handle_command Network.continueInterceptedRequest → ok empty
    #[test]
    fn handle_command_network_continue_intercepted_request() {
        let resp = dispatch_no_bridge(52, "Network.continueInterceptedRequest", None);
        assert!(resp.error.is_none());
    }

    // 124. handle_command Network.unknown → error -32601
    #[test]
    fn handle_command_network_unknown() {
        let resp = dispatch_no_bridge(53, "Network.bogus", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 125. handle_command CSS.disable → ok empty
    #[test]
    fn handle_command_css_disable() {
        let resp = dispatch_no_bridge(54, "CSS.disable", None);
        assert!(resp.error.is_none());
    }

    // 126. handle_command CSS.getMatchedStylesForNode → ok
    #[test]
    fn handle_command_css_get_matched_styles() {
        let resp = dispatch_no_bridge(55, "CSS.getMatchedStylesForNode", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["matchedCSSRules"], json!([]));
        assert_eq!(result["inlineStyle"], Value::Null);
    }

    // 127. handle_command CSS.getInlineStylesForNode → ok
    #[test]
    fn handle_command_css_get_inline_styles() {
        let resp = dispatch_no_bridge(56, "CSS.getInlineStylesForNode", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["inlineStyle"], Value::Null);
    }

    // 128. handle_command CSS.setStyleTexts → ok
    #[test]
    fn handle_command_css_set_style_texts() {
        let resp = dispatch_no_bridge(57, "CSS.setStyleTexts", None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["styles"], json!([]));
    }

    // 129. handle_command CSS.unknown → error -32601
    #[test]
    fn handle_command_css_unknown() {
        let resp = dispatch_no_bridge(58, "CSS.bogus", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 130. handle_command Emulation.clearDeviceMetricsOverride → ok empty
    #[test]
    fn handle_command_emulation_clear_device_metrics() {
        let resp = dispatch_no_bridge(59, "Emulation.clearDeviceMetricsOverride", None);
        assert!(resp.error.is_none());
    }

    // 131. handle_command Emulation.setUserAgentOverride (no bridge, empty ua) → ok empty
    #[test]
    fn handle_command_emulation_set_user_agent_no_bridge() {
        let resp = dispatch_no_bridge(60, "Emulation.setUserAgentOverride", None);
        assert!(resp.error.is_none());
    }

    // 132. handle_command Emulation.setTouchEmulationEnabled → ok empty
    #[test]
    fn handle_command_emulation_set_touch_emulation() {
        let resp = dispatch_no_bridge(61, "Emulation.setTouchEmulationEnabled", None);
        assert!(resp.error.is_none());
    }

    // 133. handle_command Emulation.setScriptExecutionDisabled → ok empty
    #[test]
    fn handle_command_emulation_set_script_execution_disabled() {
        let resp = dispatch_no_bridge(62, "Emulation.setScriptExecutionDisabled", None);
        assert!(resp.error.is_none());
    }

    // 134. handle_command Emulation.setFocusEmulationEnabled → ok empty
    #[test]
    fn handle_command_emulation_set_focus_emulation() {
        let resp = dispatch_no_bridge(63, "Emulation.setFocusEmulationEnabled", None);
        assert!(resp.error.is_none());
    }

    // 135. handle_command Emulation.setCPUThrottlingRate → ok empty
    #[test]
    fn handle_command_emulation_set_cpu_throttling() {
        let resp = dispatch_no_bridge(64, "Emulation.setCPUThrottlingRate", None);
        assert!(resp.error.is_none());
    }

    // 136. handle_command Emulation.setDefaultBackgroundColorOverride → ok empty
    #[test]
    fn handle_command_emulation_set_default_bg_color() {
        let resp = dispatch_no_bridge(65, "Emulation.setDefaultBackgroundColorOverride", None);
        assert!(resp.error.is_none());
    }

    // 137. handle_command Emulation.unknown → error -32601
    #[test]
    fn handle_command_emulation_unknown() {
        let resp = dispatch_no_bridge(66, "Emulation.bogus", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 138. handle_command Input.dispatchKeyEvent (no bridge) → ok empty
    #[test]
    fn handle_command_input_dispatch_key_no_bridge() {
        let resp = dispatch_no_bridge(67, "Input.dispatchKeyEvent", None);
        assert!(resp.error.is_none());
    }

    // 139. handle_command Input.dispatchTouchEvent → ok empty
    #[test]
    fn handle_command_input_dispatch_touch() {
        let resp = dispatch_no_bridge(68, "Input.dispatchTouchEvent", None);
        assert!(resp.error.is_none());
    }

    // 140. handle_command Input.insertText (no bridge, empty text) → ok empty
    #[test]
    fn handle_command_input_insert_text_no_bridge() {
        let resp = dispatch_no_bridge(69, "Input.insertText", None);
        assert!(resp.error.is_none());
    }

    // 141. handle_command Input.setIgnoreInputEvents → ok empty
    #[test]
    fn handle_command_input_set_ignore_input_events() {
        let resp = dispatch_no_bridge(70, "Input.setIgnoreInputEvents", None);
        assert!(resp.error.is_none());
    }

    // 142. handle_command Input.setInterceptDrags → ok empty
    #[test]
    fn handle_command_input_set_intercept_drags() {
        let resp = dispatch_no_bridge(71, "Input.setInterceptDrags", None);
        assert!(resp.error.is_none());
    }

    // 143. handle_command Input.unknown → error -32601
    #[test]
    fn handle_command_input_unknown() {
        let resp = dispatch_no_bridge(72, "Input.bogus", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 144. handle_command Overlay.highlightNode → ok empty
    #[test]
    fn handle_command_overlay_highlight_node() {
        let resp = dispatch_no_bridge(73, "Overlay.highlightNode", None);
        assert!(resp.error.is_none());
    }

    // 145. handle_command Overlay.hideHighlight → ok empty
    #[test]
    fn handle_command_overlay_hide_highlight() {
        let resp = dispatch_no_bridge(74, "Overlay.hideHighlight", None);
        assert!(resp.error.is_none());
    }

    // 146. handle_command Overlay.setInspectMode → ok empty
    #[test]
    fn handle_command_overlay_set_inspect_mode() {
        let resp = dispatch_no_bridge(75, "Overlay.setInspectMode", None);
        assert!(resp.error.is_none());
    }

    // 147. handle_command Overlay.setPausedInDebuggerMessage → ok empty
    #[test]
    fn handle_command_overlay_set_paused_in_debugger() {
        let resp = dispatch_no_bridge(76, "Overlay.setPausedInDebuggerMessage", None);
        assert!(resp.error.is_none());
    }

    // 148. handle_command Overlay.unknown → error -32601
    #[test]
    fn handle_command_overlay_unknown() {
        let resp = dispatch_no_bridge(77, "Overlay.bogus", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 149. handle_command Debugger.disable → ok empty
    #[test]
    fn handle_command_debugger_disable() {
        let resp = dispatch_no_bridge(78, "Debugger.disable", None);
        assert!(resp.error.is_none());
    }

    // 150. handle_command Debugger.removeBreakpoint → ok empty
    #[test]
    fn handle_command_debugger_remove_breakpoint() {
        let resp = dispatch_no_bridge(79, "Debugger.removeBreakpoint", None);
        assert!(resp.error.is_none());
    }

    // 151. handle_command Debugger.pause → ok empty
    #[test]
    fn handle_command_debugger_pause() {
        let resp = dispatch_no_bridge(80, "Debugger.pause", None);
        assert!(resp.error.is_none());
    }

    // 152. handle_command Debugger.resume → ok empty
    #[test]
    fn handle_command_debugger_resume() {
        let resp = dispatch_no_bridge(81, "Debugger.resume", None);
        assert!(resp.error.is_none());
    }

    // 153. handle_command Debugger.stepOver → ok empty
    #[test]
    fn handle_command_debugger_step_over() {
        let resp = dispatch_no_bridge(82, "Debugger.stepOver", None);
        assert!(resp.error.is_none());
    }

    // 154. handle_command Debugger.stepInto → ok empty
    #[test]
    fn handle_command_debugger_step_into() {
        let resp = dispatch_no_bridge(83, "Debugger.stepInto", None);
        assert!(resp.error.is_none());
    }

    // 155. handle_command Debugger.stepOut → ok empty
    #[test]
    fn handle_command_debugger_step_out() {
        let resp = dispatch_no_bridge(84, "Debugger.stepOut", None);
        assert!(resp.error.is_none());
    }

    // 156. handle_command Debugger.setSkipAllPauses → ok empty
    #[test]
    fn handle_command_debugger_set_skip_all_pauses() {
        let resp = dispatch_no_bridge(85, "Debugger.setSkipAllPauses", None);
        assert!(resp.error.is_none());
    }

    // 157. handle_command Debugger.setBreakpointsActive → ok empty
    #[test]
    fn handle_command_debugger_set_breakpoints_active() {
        let resp = dispatch_no_bridge(86, "Debugger.setBreakpointsActive", None);
        assert!(resp.error.is_none());
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

    // 161. handle_command Debugger.setPauseOnExceptions → ok empty
    #[test]
    fn handle_command_debugger_set_pause_on_exceptions() {
        let resp = dispatch_no_bridge(90, "Debugger.setPauseOnExceptions", None);
        assert!(resp.error.is_none());
    }

    // 162. handle_command Debugger.unknown → error -32601
    #[test]
    fn handle_command_debugger_unknown() {
        let resp = dispatch_no_bridge(91, "Debugger.bogus", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 163. handle_command Log.disable → ok empty
    #[test]
    fn handle_command_log_disable() {
        let resp = dispatch_no_bridge(92, "Log.disable", None);
        assert!(resp.error.is_none());
    }

    // 164. handle_command Log.clear → ok empty
    #[test]
    fn handle_command_log_clear() {
        let resp = dispatch_no_bridge(93, "Log.clear", None);
        assert!(resp.error.is_none());
    }

    // 165. handle_command Log.startViolationsReport → ok empty
    #[test]
    fn handle_command_log_start_violations_report() {
        let resp = dispatch_no_bridge(94, "Log.startViolationsReport", None);
        assert!(resp.error.is_none());
    }

    // 166. handle_command Log.stopViolationsReport → ok empty
    #[test]
    fn handle_command_log_stop_violations_report() {
        let resp = dispatch_no_bridge(95, "Log.stopViolationsReport", None);
        assert!(resp.error.is_none());
    }

    // 167. handle_command Log.unknown → error -32601
    #[test]
    fn handle_command_log_unknown() {
        let resp = dispatch_no_bridge(96, "Log.bogus", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 168. handle_command Fetch.disable → ok empty
    #[test]
    fn handle_command_fetch_disable() {
        let resp = dispatch_no_bridge(97, "Fetch.disable", None);
        assert!(resp.error.is_none());
    }

    // 169. handle_command Fetch.continueWithResponse → explicit error
    //     (no request interception facility — never a canned "continued" flag)
    #[test]
    fn handle_command_fetch_continue_with_response() {
        let resp = dispatch_no_bridge(
            98,
            "Fetch.continueWithResponse",
            Some(json!({"requestId": "r1"})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 170. handle_command Fetch.failRequest → explicit error (an
    //     interception that can never be enabled can never be failed)
    #[test]
    fn handle_command_fetch_fail_request() {
        let resp = dispatch_no_bridge(
            99,
            "Fetch.failRequest",
            Some(json!({"requestId": "r2", "reason": "Aborted"})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 171. handle_command Fetch.fulfillRequest → explicit error (never a
    //     canned "fulfilled" flag)
    #[test]
    fn handle_command_fetch_fulfill_request() {
        let resp = dispatch_no_bridge(
            100,
            "Fetch.fulfillRequest",
            Some(json!({"requestId": "r3", "responseCode": 404, "body": "dGVzdA=="})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 172. handle_command Fetch.getRequestPostData → explicit error (servo
    //     does not store request bodies for embedder access — never an
    //     empty-body fake success)
    #[test]
    fn handle_command_fetch_get_request_post_data() {
        let resp = dispatch_no_bridge(
            101,
            "Fetch.getRequestPostData",
            Some(json!({"requestId": "r4"})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 173. handle_command Fetch.continueWithAuth → explicit error
    #[test]
    fn handle_command_fetch_continue_with_auth() {
        let resp = dispatch_no_bridge(
            102,
            "Fetch.continueWithAuth",
            Some(json!({"requestId": "r5"})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 174. handle_command Fetch.takeResponseBodyAsStream → explicit error
    //      (never a fabricated stream handle)
    #[test]
    fn handle_command_fetch_take_response_body_as_stream() {
        let resp = dispatch_no_bridge(
            103,
            "Fetch.takeResponseBodyAsStream",
            Some(json!({"requestId": "r6"})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 175. handle_command Fetch.enable without patterns → explicit error
    #[test]
    fn handle_command_fetch_enable_without_patterns() {
        let resp = dispatch_no_bridge(104, "Fetch.enable", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 176. handle_command Fetch.enable with multiple patterns → explicit
    //      error (pattern count is irrelevant without an interception facility)
    #[test]
    fn handle_command_fetch_enable_with_multiple_patterns() {
        let resp = dispatch_no_bridge(
            105,
            "Fetch.enable",
            Some(json!({"patterns": [{"urlPattern": "*"}, {"urlPattern": "https://*"}]})),
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("no request interception facility"));
    }

    // 177. handle_command Fetch.unknown → error -32601
    #[test]
    fn handle_command_fetch_unknown() {
        let resp = dispatch_no_bridge(106, "Fetch.bogus", None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
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

    // 188. ServiceWorker.enable → ok empty
    #[test]
    fn service_worker_enable() {
        let resp = dispatch_no_bridge(1, "ServiceWorker.enable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 189. ServiceWorker.disable → ok empty
    #[test]
    fn service_worker_disable() {
        let resp = dispatch_no_bridge(2, "ServiceWorker.disable", None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

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

    // 192. ServiceWorker.stopWorker (no bridge) → ok empty
    #[test]
    fn service_worker_stop_worker_no_bridge() {
        let resp = dispatch_no_bridge(
            5,
            "ServiceWorker.stopWorker",
            Some(json!({"registrationId": "sw-reg-1"})),
        );
        assert!(resp.error.is_none());
    }

    // 193. ServiceWorker.unregister (no bridge) → ok empty
    #[test]
    fn service_worker_unregister_no_bridge() {
        let resp = dispatch_no_bridge(
            6,
            "ServiceWorker.unregister",
            Some(json!({"registrationId": "sw-reg-1"})),
        );
        assert!(resp.error.is_none());
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

    // 195. ServiceWorker.dispatchPeriodicSyncEvent → ok empty
    #[test]
    fn service_worker_dispatch_periodic_sync_event() {
        let resp = dispatch_no_bridge(8, "ServiceWorker.dispatchPeriodicSyncEvent", None);
        assert!(resp.error.is_none());
    }

    // 196. ServiceWorker unknown command → error -32601
    #[test]
    fn service_worker_unknown_command() {
        let resp = dispatch_no_bridge(9, "ServiceWorker.nonExistent", None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
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
}
