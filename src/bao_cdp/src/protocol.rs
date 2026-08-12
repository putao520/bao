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
        "Target" => handle_target(command, target_id, bridge),
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
        "attached": true
    })
}

fn handle_target(command: &str, target_id: &str, bridge: Option<&BridgeSender>) -> HandlerResult {
    match command {
        "getTargets" | "getTargetTargets" => {
            // REQ-BRW-004: Include Worker sub-targets in Target.getTargets response
            // @trace REQ-BRW-004 [criterion:19] Worker targets are CDP-observable
            let mut target_infos = vec![live_target_info(target_id, bridge)];
            // Append Worker sub-targets if bridge is available
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
        "createTarget" => Ok(serde_json::json!({ "targetId": target_id })),
        "closeTarget" => {
            if let Some(b) = bridge {
                b.send_fire_and_forget(BridgeCommand::ClosePage {
                    target_id: target_id.to_string(),
                });
            }
            Ok(serde_json::json!({ "success": true }))
        }
        "setAutoAttach" | "setDiscoverTargets" => ok_empty(),
        "getTargetInfo" => {
            Ok(serde_json::json!({ "targetInfo": live_target_info(target_id, bridge) }))
        }
        "attachToTarget" => Ok(serde_json::json!({
            "sessionId": format!("{:016x}", target_id.chars().map(|c| c as u64).sum::<u64>())
        })),
        "detachFromTarget" | "sendMessageToTarget" => ok_empty(),
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
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::Navigate {
                        target_id: tid.clone(),
                        url: url.to_string(),
                    },
                )?;
            }
            let loader_id = format!("{:016x}", url.len() as u64);
            let resp = cdp_protocol::page::NavigateReturnObjectBuilder::default()
                .frame_id("0".into())
                .loader_id(Some(loader_id))
                .build()
                .expect("NavigateReturnObject build: frame_id is always set");
            Ok(serde_json::to_value(resp).unwrap_or_default())
        }
        "reload" => {
            let ignore_cache = params
                .as_ref()
                .and_then(|p| p.get("ignoreCache"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::Reload {
                        target_id: tid.clone(),
                        ignore_cache,
                    },
                )?;
            }
            Ok(serde_json::json!({ "frameId": "0", "loaderId": "0" }))
        }
        "getFrameTree" => {
            let url = bridge
                .and_then(|b| {
                    b.send(BridgeCommand::GetUrl {
                        target_id: tid.clone(),
                    })
                    .result
                    .ok()
                })
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "about:blank".into());
            Ok(serde_json::json!({
                "frameTree": {
                    "frame": { "id": "0", "url": url, "loaderId": "0", "mimeType": "text/html" }
                }
            }))
        }
        "getNavigationHistory" => {
            let url = bridge
                .and_then(|b| {
                    b.send(BridgeCommand::GetUrl {
                        target_id: tid.clone(),
                    })
                    .result
                    .ok()
                })
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "about:blank".into());
            Ok(serde_json::json!({
                "currentIndex": 0,
                "entries": [{ "id": 0, "url": url, "title": "" }]
            }))
        }
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
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::TakeScreenshot {
                        target_id: tid.clone(),
                        format,
                        quality,
                    },
                )
            } else {
                Ok(serde_json::json!({ "data": "" }))
            }
        }
        "setContent" | "close" | "bringToFront" => ok_empty(),
        "getLayoutMetrics" => Ok(serde_json::json!({
            "contentSize": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
            "cssContentSize": { "x": 0, "y": 0, "width": 1920, "height": 1080 }
        })),
        "addScriptToEvaluateOnNewDocument" => {
            let source = params_str(params, "source");
            if bridge.is_some() && !source.is_empty() {
                bridge_send(
                    bridge,
                    BridgeCommand::AddScriptToEvaluateOnNewDocument {
                        target_id: tid.clone(),
                        source,
                    },
                )?;
            }
            Ok(serde_json::json!({ "identifier": "1" }))
        }
        "removeScriptToEvaluateOnNewDocument" => ok_empty(),
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
        "enable" => Ok(serde_json::json!({ "executionContextId": 1 })),
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
                .unwrap_or(true);
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
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::RuntimeCallFunctionOn {
                        target_id: tid,
                        object_id,
                        function_declaration,
                        arguments,
                        return_by_value,
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
        "releaseObject" | "releaseObjectGroup" | "compileScript" | "callArgument" => ok_empty(),
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
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::GetOuterHtml {
                        target_id: tid.clone(),
                        node_id,
                    },
                )
            } else {
                Ok(serde_json::json!({ "outerHTML": "<html><body></body></html>" }))
            }
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
                ok_empty()
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
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::GetResponseBody {
                        target_id: tid,
                        request_id,
                    },
                )
            } else {
                Ok(serde_json::json!({ "body": "", "base64Encoded": false }))
            }
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
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::NetworkSetExtraHTTPHeaders {
                        target_id: tid,
                        headers,
                    },
                )?;
            }
            ok_empty()
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

fn handle_fetch(command: &str, params: &Option<Value>) -> HandlerResult {
    match command {
        "enable" => {
            let pattern_count = params
                .as_ref()
                .and_then(|p| p.get("patterns"))
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(serde_json::json!({ "enabled": true, "patternCount": pattern_count }))
        }
        "disable" => ok_empty(),
        "continueRequest" | "continueWithResponse" => {
            let request_id = params_str(params, "requestId");
            Ok(serde_json::json!({ "requestId": request_id, "continued": true }))
        }
        "failRequest" => {
            let request_id = params_str(params, "requestId");
            let reason = params_str(params, "reason");
            Ok(serde_json::json!({ "requestId": request_id, "failed": true, "reason": reason }))
        }
        "fulfillRequest" => {
            let request_id = params_str(params, "requestId");
            let status_code = params
                .as_ref()
                .and_then(|p| p.get("responseCode"))
                .and_then(|v| v.as_u64())
                .unwrap_or(200);
            let body = params_str(params, "body");
            Ok(
                serde_json::json!({ "requestId": request_id, "fulfilled": true, "responseCode": status_code, "bodyLength": body.len() }),
            )
        }
        "getRequestPostData" => {
            let request_id = params_str(params, "requestId");
            Ok(serde_json::json!({ "requestId": request_id, "postData": "" }))
        }
        "continueWithAuth" => {
            let request_id = params_str(params, "requestId");
            Ok(serde_json::json!({ "requestId": request_id }))
        }
        "takeResponseBodyAsStream" => {
            let request_id = params_str(params, "requestId");
            Ok(serde_json::json!({ "stream": format!("stream-{}", request_id) }))
        }
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
        "start" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::ProfilerStart { target_id: tid })
            } else {
                ok_empty()
            }
        }
        "stop" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::ProfilerStop { target_id: tid })
            } else {
                // Stub: return empty profile when no bridge
                Ok(serde_json::json!({ "profile": {} }))
            }
        }
        "setSamplingInterval" => {
            let interval = params
                .as_ref()
                .and_then(|p| p.get("interval"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1000) as u32;
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::ProfilerSetSamplingInterval {
                        target_id: tid,
                        interval,
                    },
                )
            } else {
                ok_empty()
            }
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
        "takeHeapSnapshot" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::HeapProfilerTakeSnapshot { target_id: tid },
                )
            } else {
                // Stub: return empty snapshot when no bridge
                Ok(serde_json::json!({}))
            }
        }
        "startTrackingHeapObjects" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::HeapProfilerStartTracking { target_id: tid },
                )
            } else {
                ok_empty()
            }
        }
        "stopTrackingHeapObjects" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::HeapProfilerStopTracking { target_id: tid },
                )
            } else {
                ok_empty()
            }
        }
        "collectGarbage" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::HeapProfilerCollectGarbage { target_id: tid },
                )
            } else {
                ok_empty()
            }
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
        "getDOMCounters" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::MemoryGetDOMCounters { target_id: tid },
                )
            } else {
                // Default zero values when no bridge
                Ok(serde_json::json!({
                    "documents": 0,
                    "nodes": 0,
                    "jsEventListeners": 0
                }))
            }
        }
        "prepareForLeakDetection" => ok_empty(),
        "forciblyPurgeJavaScriptMemory" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::MemoryPurgeJS { target_id: tid })
            } else {
                ok_empty()
            }
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
        "getMetrics" => {
            if bridge.is_some() {
                bridge_send(
                    bridge,
                    BridgeCommand::PerformanceGetMetrics { target_id: tid },
                )
            } else {
                // Return empty metrics list when no bridge
                Ok(serde_json::json!({ "metrics": [] }))
            }
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
        let raw = r#"{"id":5,"method":"Runtime.evaluate","session_id":"abc123"}"#;
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
        let msg = CdpMessage {
            id: Some(1),
            method: "Foo.bar".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    // 9. handle_command Target.getTargets (no bridge) → ok with targetInfos
    #[test]
    fn handle_command_target_get_targets() {
        let msg = CdpMessage {
            id: Some(2),
            method: "Target.getTargets".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("targetInfos").unwrap().as_array().unwrap().len() > 0);
        assert_eq!(result["targetInfos"][0]["targetId"], "t1");
    }

    // 10. handle_command Target.createTarget (no bridge) → ok with targetId
    #[test]
    fn handle_command_target_create_target() {
        let msg = CdpMessage {
            id: Some(3),
            method: "Target.createTarget".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["targetId"], "t1");
    }

    // 11. handle_command Target.closeTarget (no bridge) → ok with success:true
    #[test]
    fn handle_command_target_close_target() {
        let msg = CdpMessage {
            id: Some(4),
            method: "Target.closeTarget".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["success"], true);
    }

    // 12. handle_command Target.setAutoAttach → ok empty
    #[test]
    fn handle_command_target_set_auto_attach() {
        let msg = CdpMessage {
            id: Some(5),
            method: "Target.setAutoAttach".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 13. handle_command Page.enable → ok empty
    #[test]
    fn handle_command_page_enable() {
        let msg = CdpMessage {
            id: Some(6),
            method: "Page.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 14. handle_command Page.getLayoutMetrics → ok with contentSize
    #[test]
    fn handle_command_page_get_layout_metrics() {
        let msg = CdpMessage {
            id: Some(7),
            method: "Page.getLayoutMetrics".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("contentSize").is_some());
        assert_eq!(result["contentSize"]["width"], 1920);
        assert_eq!(result["contentSize"]["height"], 1080);
    }

    // 15. handle_command Runtime.enable → ok with executionContextId
    #[test]
    fn handle_command_runtime_enable() {
        let msg = CdpMessage {
            id: Some(8),
            method: "Runtime.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["executionContextId"], 1);
    }

    // 16. handle_command Runtime.evaluate (no bridge, empty expr) → undefined result
    #[test]
    fn handle_command_runtime_evaluate_no_bridge() {
        let msg = CdpMessage {
            id: Some(9),
            method: "Runtime.evaluate".into(),
            params: Some(json!({"expression": ""})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 17. handle_command DOM.getDocument (no bridge) → ok with root node
    #[test]
    fn handle_command_dom_get_document() {
        let msg = CdpMessage {
            id: Some(10),
            method: "DOM.getDocument".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
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
        let msg = CdpMessage {
            id: Some(11),
            method: "DOM.querySelector".into(),
            params: Some(json!({"selector": "div"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["nodeId"], 0);
    }

    // 19. handle_command Network.enable → ok empty
    #[test]
    fn handle_command_network_enable() {
        let msg = CdpMessage {
            id: Some(12),
            method: "Network.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 20. handle_command Network.getCookies → ok with empty cookies
    #[test]
    fn handle_command_network_get_cookies() {
        let msg = CdpMessage {
            id: Some(13),
            method: "Network.getCookies".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["cookies"], json!([]));
    }

    // 21. handle_command CSS.enable → ok empty
    #[test]
    fn handle_command_css_enable() {
        let msg = CdpMessage {
            id: Some(14),
            method: "CSS.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 22. handle_command CSS.getComputedStyleForNode → ok empty computedStyle
    #[test]
    fn handle_command_css_get_computed_style() {
        let msg = CdpMessage {
            id: Some(15),
            method: "CSS.getComputedStyleForNode".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["computedStyle"], json!([]));
    }

    // 23. handle_command Emulation.setDeviceMetricsOverride (no bridge) → ok empty
    #[test]
    fn handle_command_emulation_set_device_metrics() {
        let msg = CdpMessage {
            id: Some(16),
            method: "Emulation.setDeviceMetricsOverride".into(),
            params: Some(json!({"width": 800, "height": 600, "deviceScaleFactor": 2})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 24. handle_command Input.dispatchMouseEvent (no bridge) → ok empty
    #[test]
    fn handle_command_input_dispatch_mouse() {
        let msg = CdpMessage {
            id: Some(17),
            method: "Input.dispatchMouseEvent".into(),
            params: Some(
                json!({"type": "mousePressed", "x": 100, "y": 200, "button": 0, "clickCount": 1}),
            ),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 25. handle_command Overlay.enable → ok empty
    #[test]
    fn handle_command_overlay_enable() {
        let msg = CdpMessage {
            id: Some(18),
            method: "Overlay.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 26. handle_command Debugger.enable → ok empty
    #[test]
    fn handle_command_debugger_enable() {
        let msg = CdpMessage {
            id: Some(19),
            method: "Debugger.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 27. handle_command Debugger.setBreakpointByUrl → ok with breakpointId
    #[test]
    fn handle_command_debugger_set_breakpoint_by_url() {
        let msg = CdpMessage {
            id: Some(20),
            method: "Debugger.setBreakpointByUrl".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["breakpointId"], "1");
    }

    // 28. handle_command Log.enable → ok empty
    #[test]
    fn handle_command_log_enable() {
        let msg = CdpMessage {
            id: Some(21),
            method: "Log.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 29. handle_command Fetch.enable with patterns → ok with patternCount
    #[test]
    fn handle_command_fetch_enable_with_patterns() {
        let msg = CdpMessage {
            id: Some(22),
            method: "Fetch.enable".into(),
            params: Some(json!({"patterns": [{"urlPattern": "*"}]})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["patternCount"], 1);
    }

    // 30. handle_command Fetch.continueRequest → ok with requestId
    #[test]
    fn handle_command_fetch_continue_request() {
        let msg = CdpMessage {
            id: Some(23),
            method: "Fetch.continueRequest".into(),
            params: Some(json!({"requestId": "req-001"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["requestId"], "req-001");
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
            r#"{{"id":1,"method":"Page.enable","session_id":"{}"}}"#,
            long_session
        );
        let msg = parse_message(&raw).unwrap();
        assert_eq!(msg.session_id.unwrap().len(), 10000);
    }

    // 50. CdpMessage with empty session_id
    #[test]
    fn parse_message_empty_session_id() {
        let msg = parse_message(r#"{"id":1,"method":"Page.enable","session_id":""}"#).unwrap();
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
        let msg = CdpMessage {
            id: Some(1),
            method: "NoDomain".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("NoDomain"));
    }

    // 73. handle_command with empty method → empty domain, error
    #[test]
    fn handle_command_empty_method() {
        let msg = CdpMessage {
            id: Some(2),
            method: String::new(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    // 74. handle_command with known domain but unknown command
    #[test]
    fn handle_command_known_domain_unknown_command() {
        let msg = CdpMessage {
            id: Some(3),
            method: "Page.nonExistentCommand".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Page.nonExistentCommand"));
    }

    // 75. handle_command Target.getTargetInfo (no bridge) → ok with targetInfo
    #[test]
    fn handle_command_target_get_target_info() {
        let msg = CdpMessage {
            id: Some(4),
            method: "Target.getTargetInfo".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let info = result.get("targetInfo").unwrap();
        assert_eq!(info["targetId"], "t1");
        assert_eq!(info["type"], "page");
        assert_eq!(info["attached"], true);
    }

    // 76. handle_command Target.attachToTarget → ok with sessionId
    #[test]
    fn handle_command_target_attach_to_target() {
        let msg = CdpMessage {
            id: Some(5),
            method: "Target.attachToTarget".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("sessionId").is_some());
        assert!(result["sessionId"].as_str().unwrap().len() > 0);
    }

    // 77. handle_command Target.detachFromTarget → ok empty
    #[test]
    fn handle_command_target_detach_from_target() {
        let msg = CdpMessage {
            id: Some(6),
            method: "Target.detachFromTarget".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 78. handle_command Target.setDiscoverTargets → ok empty
    #[test]
    fn handle_command_target_set_discover_targets() {
        let msg = CdpMessage {
            id: Some(7),
            method: "Target.setDiscoverTargets".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 79. handle_command Target.getTargetTargets → ok (alias for getTargets)
    #[test]
    fn handle_command_target_get_target_targets() {
        let msg = CdpMessage {
            id: Some(8),
            method: "Target.getTargetTargets".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("targetInfos").unwrap().as_array().unwrap().len() > 0);
    }

    // 80. handle_command Page.navigate (no bridge) → ok with default url
    #[test]
    fn handle_command_page_navigate_no_bridge_default_url() {
        let msg = CdpMessage {
            id: Some(9),
            method: "Page.navigate".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("frameId").is_some());
    }

    // 81. handle_command Page.navigate (no bridge) with url param
    #[test]
    fn handle_command_page_navigate_with_url() {
        let msg = CdpMessage {
            id: Some(10),
            method: "Page.navigate".into(),
            params: Some(json!({"url": "https://example.com"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("frameId").is_some());
    }

    // 82. handle_command Page.reload (no bridge) → ok
    #[test]
    fn handle_command_page_reload_no_bridge() {
        let msg = CdpMessage {
            id: Some(11),
            method: "Page.reload".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["frameId"], "0");
        assert_eq!(result["loaderId"], "0");
    }

    // 83. handle_command Page.getFrameTree (no bridge) → ok
    #[test]
    fn handle_command_page_get_frame_tree() {
        let msg = CdpMessage {
            id: Some(12),
            method: "Page.getFrameTree".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let frame = result["frameTree"]["frame"].as_object().unwrap();
        assert!(frame.contains_key("id"));
        assert!(frame.contains_key("url"));
        assert!(frame.contains_key("mimeType"));
    }

    // 84. handle_command Page.getNavigationHistory (no bridge) → ok
    #[test]
    fn handle_command_page_get_navigation_history() {
        let msg = CdpMessage {
            id: Some(13),
            method: "Page.getNavigationHistory".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["currentIndex"], 0);
        assert!(result["entries"].is_array());
    }

    // 85. handle_command Page.captureScreenshot (no bridge) → ok with empty data
    #[test]
    fn handle_command_page_capture_screenshot_no_bridge() {
        let msg = CdpMessage {
            id: Some(14),
            method: "Page.captureScreenshot".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["data"], "");
    }

    // 86. handle_command Page.addScriptToEvaluateOnNewDocument (no bridge, empty source)
    #[test]
    fn handle_command_page_add_script_empty_source() {
        let msg = CdpMessage {
            id: Some(15),
            method: "Page.addScriptToEvaluateOnNewDocument".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["identifier"], "1");
    }

    // 87. handle_command Page.removeScriptToEvaluateOnNewDocument → ok empty
    #[test]
    fn handle_command_page_remove_script() {
        let msg = CdpMessage {
            id: Some(16),
            method: "Page.removeScriptToEvaluateOnNewDocument".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 88. handle_command Page.setContent → ok empty
    #[test]
    fn handle_command_page_set_content() {
        let msg = CdpMessage {
            id: Some(17),
            method: "Page.setContent".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 89. handle_command Page.close → ok empty
    #[test]
    fn handle_command_page_close() {
        let msg = CdpMessage {
            id: Some(18),
            method: "Page.close".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 90. handle_command Page.bringToFront → ok empty
    #[test]
    fn handle_command_page_bring_to_front() {
        let msg = CdpMessage {
            id: Some(19),
            method: "Page.bringToFront".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 91. handle_command Page.disable → ok empty
    #[test]
    fn handle_command_page_disable() {
        let msg = CdpMessage {
            id: Some(20),
            method: "Page.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 92. handle_command Runtime.disable → ok empty
    #[test]
    fn handle_command_runtime_disable() {
        let msg = CdpMessage {
            id: Some(21),
            method: "Runtime.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 93. handle_command Runtime.callFunctionOn → ok
    #[test]
    fn handle_command_runtime_call_function_on() {
        let msg = CdpMessage {
            id: Some(22),
            method: "Runtime.callFunctionOn".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 94. handle_command Runtime.getProperties → ok with empty array
    #[test]
    fn handle_command_runtime_get_properties() {
        let msg = CdpMessage {
            id: Some(23),
            method: "Runtime.getProperties".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"], json!([]));
    }

    // 95. handle_command Runtime.evaluateAsync → ok
    #[test]
    fn handle_command_runtime_evaluate_async() {
        let msg = CdpMessage {
            id: Some(24),
            method: "Runtime.evaluateAsync".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 96. handle_command Runtime.runScript → ok
    #[test]
    fn handle_command_runtime_run_script() {
        let msg = CdpMessage {
            id: Some(25),
            method: "Runtime.runScript".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 97. handle_command Runtime.releaseObject → ok empty
    #[test]
    fn handle_command_runtime_release_object() {
        let msg = CdpMessage {
            id: Some(26),
            method: "Runtime.releaseObject".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 98. handle_command Runtime.releaseObjectGroup → ok empty
    #[test]
    fn handle_command_runtime_release_object_group() {
        let msg = CdpMessage {
            id: Some(27),
            method: "Runtime.releaseObjectGroup".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 99. handle_command Runtime.compileScript → ok empty
    #[test]
    fn handle_command_runtime_compile_script() {
        let msg = CdpMessage {
            id: Some(28),
            method: "Runtime.compileScript".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 100. handle_command Runtime.unknown → error -32601
    #[test]
    fn handle_command_runtime_unknown_command() {
        let msg = CdpMessage {
            id: Some(29),
            method: "Runtime.unknownMethod".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Runtime.unknownMethod"));
    }

    // 101. handle_command DOM.enable → ok empty
    #[test]
    fn handle_command_dom_enable() {
        let msg = CdpMessage {
            id: Some(30),
            method: "DOM.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 102. handle_command DOM.disable → ok empty
    #[test]
    fn handle_command_dom_disable() {
        let msg = CdpMessage {
            id: Some(31),
            method: "DOM.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 103. handle_command DOM.describeNode → ok
    #[test]
    fn handle_command_dom_describe_node() {
        let msg = CdpMessage {
            id: Some(32),
            method: "DOM.describeNode".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("node").is_some());
    }

    // 104. handle_command DOM.getBoxModel → ok with model
    #[test]
    fn handle_command_dom_get_box_model() {
        let msg = CdpMessage {
            id: Some(33),
            method: "DOM.getBoxModel".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("model").is_some());
        assert_eq!(result["model"]["width"], 1920);
        assert_eq!(result["model"]["height"], 1080);
    }

    // 105. handle_command DOM.setAttributeValue (no bridge) → ok empty
    #[test]
    fn handle_command_dom_set_attribute_value_no_bridge() {
        let msg = CdpMessage {
            id: Some(34),
            method: "DOM.setAttributeValue".into(),
            params: Some(json!({"nodeId": 1, "name": "class", "value": "active"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 106. handle_command DOM.removeAttribute → ok empty
    #[test]
    fn handle_command_dom_remove_attribute() {
        let msg = CdpMessage {
            id: Some(35),
            method: "DOM.removeAttribute".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 107. handle_command DOM.setOuterHTML → ok empty
    #[test]
    fn handle_command_dom_set_outer_html() {
        let msg = CdpMessage {
            id: Some(36),
            method: "DOM.setOuterHTML".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 108. handle_command DOM.insertBefore → ok empty
    #[test]
    fn handle_command_dom_insert_before() {
        let msg = CdpMessage {
            id: Some(37),
            method: "DOM.insertBefore".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 109. handle_command DOM.removeNode → ok empty
    #[test]
    fn handle_command_dom_remove_node() {
        let msg = CdpMessage {
            id: Some(38),
            method: "DOM.removeNode".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 110. handle_command DOM.getOuterHTML (no bridge) → ok with default html
    #[test]
    fn handle_command_dom_get_outer_html_no_bridge() {
        let msg = CdpMessage {
            id: Some(39),
            method: "DOM.getOuterHTML".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("outerHTML").is_some());
    }

    // 111. handle_command DOM.resolveNode → ok
    #[test]
    fn handle_command_dom_resolve_node() {
        let msg = CdpMessage {
            id: Some(40),
            method: "DOM.resolveNode".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["object"]["type"], "node");
    }

    // 112. handle_command DOM.pushNodesByBackendIdsToFrontend → ok
    #[test]
    fn handle_command_dom_push_nodes_by_backend_ids() {
        let msg = CdpMessage {
            id: Some(41),
            method: "DOM.pushNodesByBackendIdsToFrontend".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["nodeIds"], json!([]));
    }

    // 113. handle_command DOM.unknown → error -32601
    #[test]
    fn handle_command_dom_unknown_command() {
        let msg = CdpMessage {
            id: Some(42),
            method: "DOM.nonExistent".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 114. handle_command Network.disable → ok empty
    #[test]
    fn handle_command_network_disable() {
        let msg = CdpMessage {
            id: Some(43),
            method: "Network.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 115. handle_command Network.getResponseBody → ok
    #[test]
    fn handle_command_network_get_response_body() {
        let msg = CdpMessage {
            id: Some(44),
            method: "Network.getResponseBody".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["body"], "");
        assert_eq!(result["base64Encoded"], false);
    }

    // 116. handle_command Network.setCacheDisabled → ok empty
    #[test]
    fn handle_command_network_set_cache_disabled() {
        let msg = CdpMessage {
            id: Some(45),
            method: "Network.setCacheDisabled".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 117. handle_command Network.setExtraHTTPHeaders → ok empty
    #[test]
    fn handle_command_network_set_extra_http_headers() {
        let msg = CdpMessage {
            id: Some(46),
            method: "Network.setExtraHTTPHeaders".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 118. handle_command Network.emulateNetworkConditions → ok empty
    #[test]
    fn handle_command_network_emulate_conditions() {
        let msg = CdpMessage {
            id: Some(47),
            method: "Network.emulateNetworkConditions".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 119. handle_command Network.getAllCookies → ok with empty cookies
    #[test]
    fn handle_command_network_get_all_cookies() {
        let msg = CdpMessage {
            id: Some(48),
            method: "Network.getAllCookies".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["cookies"], json!([]));
    }

    // 120. handle_command Network.deleteCookies → ok empty
    #[test]
    fn handle_command_network_delete_cookies() {
        let msg = CdpMessage {
            id: Some(49),
            method: "Network.deleteCookies".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 121. handle_command Network.setCookie → ok empty
    #[test]
    fn handle_command_network_set_cookie() {
        let msg = CdpMessage {
            id: Some(50),
            method: "Network.setCookie".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 122. handle_command Network.setRequestInterception → ok empty
    #[test]
    fn handle_command_network_set_request_interception() {
        let msg = CdpMessage {
            id: Some(51),
            method: "Network.setRequestInterception".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 123. handle_command Network.continueInterceptedRequest → ok empty
    #[test]
    fn handle_command_network_continue_intercepted_request() {
        let msg = CdpMessage {
            id: Some(52),
            method: "Network.continueInterceptedRequest".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 124. handle_command Network.unknown → error -32601
    #[test]
    fn handle_command_network_unknown() {
        let msg = CdpMessage {
            id: Some(53),
            method: "Network.bogus".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 125. handle_command CSS.disable → ok empty
    #[test]
    fn handle_command_css_disable() {
        let msg = CdpMessage {
            id: Some(54),
            method: "CSS.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 126. handle_command CSS.getMatchedStylesForNode → ok
    #[test]
    fn handle_command_css_get_matched_styles() {
        let msg = CdpMessage {
            id: Some(55),
            method: "CSS.getMatchedStylesForNode".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["matchedCSSRules"], json!([]));
        assert_eq!(result["inlineStyle"], Value::Null);
    }

    // 127. handle_command CSS.getInlineStylesForNode → ok
    #[test]
    fn handle_command_css_get_inline_styles() {
        let msg = CdpMessage {
            id: Some(56),
            method: "CSS.getInlineStylesForNode".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["inlineStyle"], Value::Null);
    }

    // 128. handle_command CSS.setStyleTexts → ok
    #[test]
    fn handle_command_css_set_style_texts() {
        let msg = CdpMessage {
            id: Some(57),
            method: "CSS.setStyleTexts".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["styles"], json!([]));
    }

    // 129. handle_command CSS.unknown → error -32601
    #[test]
    fn handle_command_css_unknown() {
        let msg = CdpMessage {
            id: Some(58),
            method: "CSS.bogus".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 130. handle_command Emulation.clearDeviceMetricsOverride → ok empty
    #[test]
    fn handle_command_emulation_clear_device_metrics() {
        let msg = CdpMessage {
            id: Some(59),
            method: "Emulation.clearDeviceMetricsOverride".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 131. handle_command Emulation.setUserAgentOverride (no bridge, empty ua) → ok empty
    #[test]
    fn handle_command_emulation_set_user_agent_no_bridge() {
        let msg = CdpMessage {
            id: Some(60),
            method: "Emulation.setUserAgentOverride".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 132. handle_command Emulation.setTouchEmulationEnabled → ok empty
    #[test]
    fn handle_command_emulation_set_touch_emulation() {
        let msg = CdpMessage {
            id: Some(61),
            method: "Emulation.setTouchEmulationEnabled".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 133. handle_command Emulation.setScriptExecutionDisabled → ok empty
    #[test]
    fn handle_command_emulation_set_script_execution_disabled() {
        let msg = CdpMessage {
            id: Some(62),
            method: "Emulation.setScriptExecutionDisabled".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 134. handle_command Emulation.setFocusEmulationEnabled → ok empty
    #[test]
    fn handle_command_emulation_set_focus_emulation() {
        let msg = CdpMessage {
            id: Some(63),
            method: "Emulation.setFocusEmulationEnabled".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 135. handle_command Emulation.setCPUThrottlingRate → ok empty
    #[test]
    fn handle_command_emulation_set_cpu_throttling() {
        let msg = CdpMessage {
            id: Some(64),
            method: "Emulation.setCPUThrottlingRate".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 136. handle_command Emulation.setDefaultBackgroundColorOverride → ok empty
    #[test]
    fn handle_command_emulation_set_default_bg_color() {
        let msg = CdpMessage {
            id: Some(65),
            method: "Emulation.setDefaultBackgroundColorOverride".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 137. handle_command Emulation.unknown → error -32601
    #[test]
    fn handle_command_emulation_unknown() {
        let msg = CdpMessage {
            id: Some(66),
            method: "Emulation.bogus".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 138. handle_command Input.dispatchKeyEvent (no bridge) → ok empty
    #[test]
    fn handle_command_input_dispatch_key_no_bridge() {
        let msg = CdpMessage {
            id: Some(67),
            method: "Input.dispatchKeyEvent".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 139. handle_command Input.dispatchTouchEvent → ok empty
    #[test]
    fn handle_command_input_dispatch_touch() {
        let msg = CdpMessage {
            id: Some(68),
            method: "Input.dispatchTouchEvent".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 140. handle_command Input.insertText (no bridge, empty text) → ok empty
    #[test]
    fn handle_command_input_insert_text_no_bridge() {
        let msg = CdpMessage {
            id: Some(69),
            method: "Input.insertText".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 141. handle_command Input.setIgnoreInputEvents → ok empty
    #[test]
    fn handle_command_input_set_ignore_input_events() {
        let msg = CdpMessage {
            id: Some(70),
            method: "Input.setIgnoreInputEvents".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 142. handle_command Input.setInterceptDrags → ok empty
    #[test]
    fn handle_command_input_set_intercept_drags() {
        let msg = CdpMessage {
            id: Some(71),
            method: "Input.setInterceptDrags".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 143. handle_command Input.unknown → error -32601
    #[test]
    fn handle_command_input_unknown() {
        let msg = CdpMessage {
            id: Some(72),
            method: "Input.bogus".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 144. handle_command Overlay.highlightNode → ok empty
    #[test]
    fn handle_command_overlay_highlight_node() {
        let msg = CdpMessage {
            id: Some(73),
            method: "Overlay.highlightNode".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 145. handle_command Overlay.hideHighlight → ok empty
    #[test]
    fn handle_command_overlay_hide_highlight() {
        let msg = CdpMessage {
            id: Some(74),
            method: "Overlay.hideHighlight".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 146. handle_command Overlay.setInspectMode → ok empty
    #[test]
    fn handle_command_overlay_set_inspect_mode() {
        let msg = CdpMessage {
            id: Some(75),
            method: "Overlay.setInspectMode".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 147. handle_command Overlay.setPausedInDebuggerMessage → ok empty
    #[test]
    fn handle_command_overlay_set_paused_in_debugger() {
        let msg = CdpMessage {
            id: Some(76),
            method: "Overlay.setPausedInDebuggerMessage".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 148. handle_command Overlay.unknown → error -32601
    #[test]
    fn handle_command_overlay_unknown() {
        let msg = CdpMessage {
            id: Some(77),
            method: "Overlay.bogus".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 149. handle_command Debugger.disable → ok empty
    #[test]
    fn handle_command_debugger_disable() {
        let msg = CdpMessage {
            id: Some(78),
            method: "Debugger.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 150. handle_command Debugger.removeBreakpoint → ok empty
    #[test]
    fn handle_command_debugger_remove_breakpoint() {
        let msg = CdpMessage {
            id: Some(79),
            method: "Debugger.removeBreakpoint".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 151. handle_command Debugger.pause → ok empty
    #[test]
    fn handle_command_debugger_pause() {
        let msg = CdpMessage {
            id: Some(80),
            method: "Debugger.pause".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 152. handle_command Debugger.resume → ok empty
    #[test]
    fn handle_command_debugger_resume() {
        let msg = CdpMessage {
            id: Some(81),
            method: "Debugger.resume".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 153. handle_command Debugger.stepOver → ok empty
    #[test]
    fn handle_command_debugger_step_over() {
        let msg = CdpMessage {
            id: Some(82),
            method: "Debugger.stepOver".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 154. handle_command Debugger.stepInto → ok empty
    #[test]
    fn handle_command_debugger_step_into() {
        let msg = CdpMessage {
            id: Some(83),
            method: "Debugger.stepInto".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 155. handle_command Debugger.stepOut → ok empty
    #[test]
    fn handle_command_debugger_step_out() {
        let msg = CdpMessage {
            id: Some(84),
            method: "Debugger.stepOut".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 156. handle_command Debugger.setSkipAllPauses → ok empty
    #[test]
    fn handle_command_debugger_set_skip_all_pauses() {
        let msg = CdpMessage {
            id: Some(85),
            method: "Debugger.setSkipAllPauses".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 157. handle_command Debugger.setBreakpointsActive → ok empty
    #[test]
    fn handle_command_debugger_set_breakpoints_active() {
        let msg = CdpMessage {
            id: Some(86),
            method: "Debugger.setBreakpointsActive".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 158. handle_command Debugger.evaluateOnCallFrame → ok
    #[test]
    fn handle_command_debugger_evaluate_on_call_frame() {
        let msg = CdpMessage {
            id: Some(87),
            method: "Debugger.evaluateOnCallFrame".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 159. handle_command Debugger.getPossibleBreakpoints → ok
    #[test]
    fn handle_command_debugger_get_possible_breakpoints() {
        let msg = CdpMessage {
            id: Some(88),
            method: "Debugger.getPossibleBreakpoints".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["locations"], json!([]));
    }

    // 160. handle_command Debugger.getScriptSource → ok
    #[test]
    fn handle_command_debugger_get_script_source() {
        let msg = CdpMessage {
            id: Some(89),
            method: "Debugger.getScriptSource".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["scriptSource"], "");
    }

    // 161. handle_command Debugger.setPauseOnExceptions → ok empty
    #[test]
    fn handle_command_debugger_set_pause_on_exceptions() {
        let msg = CdpMessage {
            id: Some(90),
            method: "Debugger.setPauseOnExceptions".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 162. handle_command Debugger.unknown → error -32601
    #[test]
    fn handle_command_debugger_unknown() {
        let msg = CdpMessage {
            id: Some(91),
            method: "Debugger.bogus".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 163. handle_command Log.disable → ok empty
    #[test]
    fn handle_command_log_disable() {
        let msg = CdpMessage {
            id: Some(92),
            method: "Log.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 164. handle_command Log.clear → ok empty
    #[test]
    fn handle_command_log_clear() {
        let msg = CdpMessage {
            id: Some(93),
            method: "Log.clear".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 165. handle_command Log.startViolationsReport → ok empty
    #[test]
    fn handle_command_log_start_violations_report() {
        let msg = CdpMessage {
            id: Some(94),
            method: "Log.startViolationsReport".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 166. handle_command Log.stopViolationsReport → ok empty
    #[test]
    fn handle_command_log_stop_violations_report() {
        let msg = CdpMessage {
            id: Some(95),
            method: "Log.stopViolationsReport".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 167. handle_command Log.unknown → error -32601
    #[test]
    fn handle_command_log_unknown() {
        let msg = CdpMessage {
            id: Some(96),
            method: "Log.bogus".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 168. handle_command Fetch.disable → ok empty
    #[test]
    fn handle_command_fetch_disable() {
        let msg = CdpMessage {
            id: Some(97),
            method: "Fetch.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 169. handle_command Fetch.continueWithResponse → ok
    #[test]
    fn handle_command_fetch_continue_with_response() {
        let msg = CdpMessage {
            id: Some(98),
            method: "Fetch.continueWithResponse".into(),
            params: Some(json!({"requestId": "r1"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["requestId"], "r1");
        assert_eq!(result["continued"], true);
    }

    // 170. handle_command Fetch.failRequest → ok
    #[test]
    fn handle_command_fetch_fail_request() {
        let msg = CdpMessage {
            id: Some(99),
            method: "Fetch.failRequest".into(),
            params: Some(json!({"requestId": "r2", "reason": "Aborted"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["requestId"], "r2");
        assert_eq!(result["failed"], true);
        assert_eq!(result["reason"], "Aborted");
    }

    // 171. handle_command Fetch.fulfillRequest → ok
    #[test]
    fn handle_command_fetch_fulfill_request() {
        let msg = CdpMessage {
            id: Some(100),
            method: "Fetch.fulfillRequest".into(),
            params: Some(json!({"requestId": "r3", "responseCode": 404, "body": "dGVzdA=="})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["requestId"], "r3");
        assert_eq!(result["fulfilled"], true);
        assert_eq!(result["responseCode"], 404);
    }

    // 172. handle_command Fetch.getRequestPostData → ok
    #[test]
    fn handle_command_fetch_get_request_post_data() {
        let msg = CdpMessage {
            id: Some(101),
            method: "Fetch.getRequestPostData".into(),
            params: Some(json!({"requestId": "r4"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["requestId"], "r4");
        assert_eq!(result["postData"], "");
    }

    // 173. handle_command Fetch.continueWithAuth → ok
    #[test]
    fn handle_command_fetch_continue_with_auth() {
        let msg = CdpMessage {
            id: Some(102),
            method: "Fetch.continueWithAuth".into(),
            params: Some(json!({"requestId": "r5"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["requestId"], "r5");
    }

    // 174. handle_command Fetch.takeResponseBodyAsStream → ok
    #[test]
    fn handle_command_fetch_take_response_body_as_stream() {
        let msg = CdpMessage {
            id: Some(103),
            method: "Fetch.takeResponseBodyAsStream".into(),
            params: Some(json!({"requestId": "r6"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["stream"], "stream-r6");
    }

    // 175. handle_command Fetch.enable without patterns → patternCount 0
    #[test]
    fn handle_command_fetch_enable_without_patterns() {
        let msg = CdpMessage {
            id: Some(104),
            method: "Fetch.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["patternCount"], 0);
        assert_eq!(result["enabled"], true);
    }

    // 176. handle_command Fetch.enable with multiple patterns
    #[test]
    fn handle_command_fetch_enable_with_multiple_patterns() {
        let msg = CdpMessage {
            id: Some(105),
            method: "Fetch.enable".into(),
            params: Some(json!({"patterns": [{"urlPattern": "*"}, {"urlPattern": "https://*"}]})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["patternCount"], 2);
    }

    // 177. handle_command Fetch.unknown → error -32601
    #[test]
    fn handle_command_fetch_unknown() {
        let msg = CdpMessage {
            id: Some(106),
            method: "Fetch.bogus".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
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
        let msg = CdpMessage {
            id: Some(1),
            method: "Page.enable".into(),
            params: None,
            session_id: None,
        };
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
        let msg = CdpMessage {
            id: Some(1),
            method: "ServiceWorker.enable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 189. ServiceWorker.disable → ok empty
    #[test]
    fn service_worker_disable() {
        let msg = CdpMessage {
            id: Some(2),
            method: "ServiceWorker.disable".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 190. ServiceWorker.getAllRegistrations (no bridge) → empty registrations
    #[test]
    fn service_worker_get_all_registrations_no_bridge() {
        let msg = CdpMessage {
            id: Some(3),
            method: "ServiceWorker.getAllRegistrations".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["registrations"], json!([]));
    }

    // 191. ServiceWorker.getRegistration (no bridge) → null registration
    #[test]
    fn service_worker_get_registration_no_bridge() {
        let msg = CdpMessage {
            id: Some(4),
            method: "ServiceWorker.getRegistration".into(),
            params: Some(json!({"registrationId": "sw-reg-1"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["registration"], Value::Null);
    }

    // 192. ServiceWorker.stopWorker (no bridge) → ok empty
    #[test]
    fn service_worker_stop_worker_no_bridge() {
        let msg = CdpMessage {
            id: Some(5),
            method: "ServiceWorker.stopWorker".into(),
            params: Some(json!({"registrationId": "sw-reg-1"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 193. ServiceWorker.unregister (no bridge) → ok empty
    #[test]
    fn service_worker_unregister_no_bridge() {
        let msg = CdpMessage {
            id: Some(6),
            method: "ServiceWorker.unregister".into(),
            params: Some(json!({"registrationId": "sw-reg-1"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 194. ServiceWorker.deliverPushMessage → ok with delivered flag
    #[test]
    fn service_worker_deliver_push_message() {
        let msg = CdpMessage {
            id: Some(7),
            method: "ServiceWorker.deliverPushMessage".into(),
            params: Some(json!({"origin": "https://example.com", "registrationId": "sw-reg-1"})),
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["origin"], "https://example.com");
        assert_eq!(result["delivered"], true);
    }

    // 195. ServiceWorker.dispatchPeriodicSyncEvent → ok empty
    #[test]
    fn service_worker_dispatch_periodic_sync_event() {
        let msg = CdpMessage {
            id: Some(8),
            method: "ServiceWorker.dispatchPeriodicSyncEvent".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 196. ServiceWorker unknown command → error -32601
    #[test]
    fn service_worker_unknown_command() {
        let msg = CdpMessage {
            id: Some(9),
            method: "ServiceWorker.nonExistent".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    // 197. Target.getTargets (no bridge) still returns page target (Worker sub-targets are empty)
    #[test]
    fn target_get_targets_no_bridge_includes_page() {
        let msg = CdpMessage {
            id: Some(10),
            method: "Target.getTargets".into(),
            params: None,
            session_id: None,
        };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let infos = result["targetInfos"].as_array().unwrap();
        assert!(infos.len() >= 1, "should at least have the page target");
        assert_eq!(infos[0]["targetId"], "t1");
    }
}
