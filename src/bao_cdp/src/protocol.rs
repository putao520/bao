// REQ-CDP-001: CDP protocol message types and 11-domain dispatch  @trace REQ-CDP-001 [entity:CdpServer]
// Uses cdp-protocol crate for typed parameter parsing and response construction.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::servo_bridge::{BridgeCommand, BridgeSender};

#[derive(Debug, Clone, Deserialize)]
pub struct CDPMessage {
    pub id: i64,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CDPResponse {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CDPError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CDPError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CDPEvent {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

pub fn parse_message(raw: &str) -> Option<CDPMessage> {
    serde_json::from_str(raw).ok()
}

pub fn handle_command(
    msg: CDPMessage,
    target_id: &str,
    params: &Option<Value>,
    bridge: Option<&BridgeSender>,
) -> CDPResponse {
    let parts: Vec<&str> = msg.method.splitn(2, '.').collect();
    let domain = parts.first().copied().unwrap_or("");
    let command = parts.get(1).copied().unwrap_or("");

    let result = match domain {
        "Target" => handle_target(command, target_id, bridge),
        "Page" => handle_page(command, params, bridge),
        "Runtime" => handle_runtime(command, params, bridge),
        "DOM" => handle_dom(command, params, bridge),
        "Network" => handle_network(command),
        "CSS" => handle_css(command),
        "Emulation" => handle_emulation(command, params, bridge),
        "Input" => handle_input(command, params, bridge),
        "Overlay" => handle_overlay(command),
        "Debugger" => handle_debugger(command),
        "Log" => handle_log(command),
        "Fetch" => handle_fetch(command, params),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'{}' wasn't found", msg.method),
        }),
    };

    match result {
        Ok(r) => CDPResponse { id: msg.id, result: Some(r), error: None },
        Err(e) => CDPResponse { id: msg.id, result: None, error: Some(e) },
    }
}

pub fn serialize_response(resp: &CDPResponse) -> String {
    serde_json::to_string(resp)
        .unwrap_or_else(|_| r#"{"id":0,"error":{"code":-32700,"message":"serialize error"}}"#.into())
}

pub fn serialize_event(ev: &CDPEvent) -> String {
    serde_json::to_string(ev).unwrap_or_else(|_| "{}".into())
}

type HandlerResult = Result<Value, CDPError>;

fn params_str(params: &Option<Value>, key: &str) -> String {
    params.as_ref()
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn bridge_send(bridge: Option<&BridgeSender>, cmd: BridgeCommand) -> HandlerResult {
    match bridge {
        Some(b) => {
            let resp = b.send(cmd);
            resp.result.map_err(|e| CDPError { code: -32603, message: e })
        }
        None => Err(CDPError { code: -32603, message: "no servo bridge connected".into() }),
    }
}

fn ok_empty() -> HandlerResult {
    Ok(serde_json::json!({}))
}

fn live_target_info(target_id: &str, bridge: Option<&BridgeSender>) -> Value {
    let title = bridge.and_then(|b| b.send(BridgeCommand::GetTitle).result.ok())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "Bao".into());
    let url = bridge.and_then(|b| b.send(BridgeCommand::GetUrl).result.ok())
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
            Ok(serde_json::json!({ "targetInfos": [live_target_info(target_id, bridge)] }))
        }
        "createTarget" => Ok(serde_json::json!({ "targetId": target_id })),
        "closeTarget" => {
            if let Some(b) = bridge { b.send_fire_and_forget(BridgeCommand::ClosePage); }
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
        _ => Err(CDPError { code: -32601, message: format!("'Target.{}' wasn't found", command) }),
    }
}

fn handle_page(command: &str, params: &Option<Value>, bridge: Option<&BridgeSender>) -> HandlerResult {
    match command {
        "enable" | "disable" => ok_empty(),
        "navigate" => {
            let url = params.as_ref()
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("about:blank");
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::Navigate { url: url.to_string() })?;
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
            let ignore_cache = params.as_ref()
                .and_then(|p| p.get("ignoreCache"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::Reload { ignore_cache })?;
            }
            Ok(serde_json::json!({ "frameId": "0", "loaderId": "0" }))
        }
        "getFrameTree" => {
            let url = bridge.and_then(|b| b.send(BridgeCommand::GetUrl).result.ok())
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "about:blank".into());
            Ok(serde_json::json!({
                "frameTree": {
                    "frame": { "id": "0", "url": url, "loaderId": "0", "mimeType": "text/html" }
                }
            }))
        }
        "getNavigationHistory" => {
            let url = bridge.and_then(|b| b.send(BridgeCommand::GetUrl).result.ok())
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "about:blank".into());
            Ok(serde_json::json!({
                "currentIndex": 0,
                "entries": [{ "id": 0, "url": url, "title": "" }]
            }))
        }
        "captureScreenshot" => {
            let format = params.as_ref()
                .and_then(|p| p.get("format"))
                .and_then(|v| v.as_str())
                .unwrap_or("png").to_string();
            let quality = params.as_ref()
                .and_then(|p| p.get("quality"))
                .and_then(|v| v.as_u64()).map(|q| q as u8);
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::TakeScreenshot { format, quality })
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
                bridge_send(bridge, BridgeCommand::AddScriptToEvaluateOnNewDocument { source })?;
            }
            Ok(serde_json::json!({ "identifier": "1" }))
        }
        "removeScriptToEvaluateOnNewDocument" => ok_empty(),
        _ => Err(CDPError { code: -32601, message: format!("'Page.{}' wasn't found", command) }),
    }
}

fn handle_runtime(command: &str, params: &Option<Value>, bridge: Option<&BridgeSender>) -> HandlerResult {
    match command {
        "enable" => Ok(serde_json::json!({ "executionContextId": 1 })),
        "disable" => ok_empty(),
        "evaluate" => {
            let expression = params.as_ref()
                .and_then(|p| p.get("expression"))
                .and_then(|v| v.as_str())
                .unwrap_or("").to_string();
            let return_by_value = params.as_ref()
                .and_then(|p| p.get("returnByValue"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if bridge.is_some() && !expression.is_empty() {
                bridge_send(bridge, BridgeCommand::EvaluateJs { expression, return_by_value })
            } else {
                Ok(serde_json::json!({ "result": { "type": "undefined" }, "exceptionDetails": null }))
            }
        }
        "callFunctionOn" => Ok(serde_json::json!({ "result": { "type": "undefined" } })),
        "getProperties" => Ok(serde_json::json!({ "result": [] })),
        "evaluateAsync" | "runScript" => Ok(serde_json::json!({ "result": { "type": "undefined" } })),
        "releaseObject" | "releaseObjectGroup" | "compileScript" | "callArgument" => ok_empty(),
        _ => Err(CDPError { code: -32601, message: format!("'Runtime.{}' wasn't found", command) }),
    }
}

fn handle_dom(command: &str, params: &Option<Value>, bridge: Option<&BridgeSender>) -> HandlerResult {
    match command {
        "enable" | "disable" => ok_empty(),
        "getDocument" => {
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::GetDocument)
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
        "describeNode" => Ok(serde_json::json!({ "node": { "nodeId": 1, "nodeType": 1, "nodeName": "HTML" } })),
        "querySelector" => {
            let selector = params_str(params, "selector");
            if bridge.is_some() && !selector.is_empty() {
                bridge_send(bridge, BridgeCommand::QuerySelector { selector })
            } else {
                Ok(serde_json::json!({ "nodeId": 0 }))
            }
        }
        "querySelectorAll" => {
            let selector = params_str(params, "selector");
            if bridge.is_some() && !selector.is_empty() {
                bridge_send(bridge, BridgeCommand::QuerySelectorAll { selector })
            } else {
                Ok(serde_json::json!({ "nodeIds": [] }))
            }
        }
        "getBoxModel" => Ok(serde_json::json!({
            "model": { "width": 1920, "height": 1080, "content": [0, 0, 1920, 0, 1920, 1080, 0, 1080] }
        })),
        "setAttributeValue" => {
            let node_id = params.as_ref().and_then(|p| p.get("nodeId")).and_then(|v| v.as_i64()).unwrap_or(0);
            let name = params_str(params, "name");
            let value = params_str(params, "value");
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::SetAttributeValue { node_id, name, value })
            } else {
                ok_empty()
            }
        }
        "removeAttribute" | "setOuterHTML" | "insertBefore" | "removeNode" => ok_empty(),
        "getOuterHTML" => {
            let node_id = params.as_ref().and_then(|p| p.get("nodeId")).and_then(|v| v.as_i64());
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::GetOuterHtml { node_id })
            } else {
                Ok(serde_json::json!({ "outerHTML": "<html><body></body></html>" }))
            }
        }
        "resolveNode" => Ok(serde_json::json!({ "object": { "type": "node" } })),
        "pushNodesByBackendIdsToFrontend" => Ok(serde_json::json!({ "nodeIds": [] })),
        _ => Err(CDPError { code: -32601, message: format!("'DOM.{}' wasn't found", command) }),
    }
}

fn handle_network(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => ok_empty(),
        "getResponseBody" => Ok(serde_json::json!({ "body": "", "base64Encoded": false })),
        "setCacheDisabled" | "setExtraHTTPHeaders" => ok_empty(),
        "emulateNetworkConditions" | "setRequestInterception" => ok_empty(),
        "continueInterceptedRequest" => ok_empty(),
        "getCookies" | "getAllCookies" => Ok(serde_json::json!({ "cookies": [] })),
        "deleteCookies" | "setCookie" => ok_empty(),
        _ => Err(CDPError { code: -32601, message: format!("'Network.{}' wasn't found", command) }),
    }
}

fn handle_css(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => ok_empty(),
        "getComputedStyleForNode" => Ok(serde_json::json!({ "computedStyle": [] })),
        "getMatchedStylesForNode" => Ok(serde_json::json!({
            "matchedCSSRules": [], "inlineStyle": null, "attributesStyle": null
        })),
        "getInlineStylesForNode" => Ok(serde_json::json!({ "inlineStyle": null })),
        "setStyleTexts" => Ok(serde_json::json!({ "styles": [] })),
        _ => Err(CDPError { code: -32601, message: format!("'CSS.{}' wasn't found", command) }),
    }
}

fn handle_emulation(command: &str, params: &Option<Value>, bridge: Option<&BridgeSender>) -> HandlerResult {
    match command {
        "setDeviceMetricsOverride" => {
            let width = params.as_ref()
                .and_then(|p| p.get("width")).and_then(|v| v.as_u64()).unwrap_or(1920) as u32;
            let height = params.as_ref()
                .and_then(|p| p.get("height")).and_then(|v| v.as_u64()).unwrap_or(1080) as u32;
            let dsf = params.as_ref()
                .and_then(|p| p.get("deviceScaleFactor")).and_then(|v| v.as_f64());
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::SetViewport { width, height, device_scale_factor: dsf })
            } else {
                ok_empty()
            }
        }
        "clearDeviceMetricsOverride" => ok_empty(),
        "setUserAgentOverride" => {
            let ua = params_str(params, "userAgent");
            if bridge.is_some() && !ua.is_empty() {
                bridge_send(bridge, BridgeCommand::SetUserAgent { user_agent: ua })
            } else {
                ok_empty()
            }
        }
        "setTouchEmulationEnabled" | "setScriptExecutionDisabled" => ok_empty(),
        "setFocusEmulationEnabled" | "setCPUThrottlingRate" => ok_empty(),
        "setDefaultBackgroundColorOverride" => ok_empty(),
        _ => Err(CDPError { code: -32601, message: format!("'Emulation.{}' wasn't found", command) }),
    }
}

fn handle_input(command: &str, params: &Option<Value>, bridge: Option<&BridgeSender>) -> HandlerResult {
    match command {
        "dispatchMouseEvent" => {
            let event_type = params_str(params, "type");
            let x = params.as_ref().and_then(|p| p.get("x")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = params.as_ref().and_then(|p| p.get("y")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let button = params.as_ref().and_then(|p| p.get("button")).and_then(|v| v.as_i64());
            let click_count = params.as_ref().and_then(|p| p.get("clickCount")).and_then(|v| v.as_i64());
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::DispatchMouseEvent { event_type, x, y, button, click_count })
            } else {
                ok_empty()
            }
        }
        "dispatchKeyEvent" => {
            let event_type = params_str(params, "type");
            let key = params_str(params, "key");
            let code = params_str(params, "code");
            let text = params.as_ref().and_then(|p| p.get("text")).and_then(|v| v.as_str()).map(|s| s.to_string());
            if bridge.is_some() {
                bridge_send(bridge, BridgeCommand::DispatchKeyEvent { event_type, key, code, text })
            } else {
                ok_empty()
            }
        }
        "dispatchTouchEvent" => ok_empty(),
        "insertText" => {
            let text = params_str(params, "text");
            if bridge.is_some() && !text.is_empty() {
                bridge_send(bridge, BridgeCommand::InsertText { text })
            } else {
                ok_empty()
            }
        }
        "setIgnoreInputEvents" | "setInterceptDrags" => ok_empty(),
        _ => Err(CDPError { code: -32601, message: format!("'Input.{}' wasn't found", command) }),
    }
}

fn handle_overlay(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => ok_empty(),
        "highlightNode" | "hideHighlight" | "setInspectMode" => ok_empty(),
        "setPausedInDebuggerMessage" => ok_empty(),
        _ => Err(CDPError { code: -32601, message: format!("'Overlay.{}' wasn't found", command) }),
    }
}

fn handle_debugger(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => ok_empty(),
        "setBreakpointByUrl" => Ok(serde_json::json!({ "breakpointId": "1", "locations": [] })),
        "removeBreakpoint" | "pause" | "resume" => ok_empty(),
        "stepOver" | "stepInto" | "stepOut" => ok_empty(),
        "setSkipAllPauses" | "setBreakpointsActive" => ok_empty(),
        "evaluateOnCallFrame" => Ok(serde_json::json!({ "result": { "type": "undefined" } })),
        "getPossibleBreakpoints" => Ok(serde_json::json!({ "locations": [] })),
        "getScriptSource" => Ok(serde_json::json!({ "scriptSource": "" })),
        "setPauseOnExceptions" => ok_empty(),
        _ => Err(CDPError { code: -32601, message: format!("'Debugger.{}' wasn't found", command) }),
    }
}

fn handle_log(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" | "clear" => ok_empty(),
        "startViolationsReport" | "stopViolationsReport" => ok_empty(),
        _ => Err(CDPError { code: -32601, message: format!("'Log.{}' wasn't found", command) }),
    }
}

fn handle_fetch(command: &str, params: &Option<Value>) -> HandlerResult {
    match command {
        "enable" => {
            let pattern_count = params.as_ref()
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
            let status_code = params.as_ref()
                .and_then(|p| p.get("responseCode")).and_then(|v| v.as_u64()).unwrap_or(200);
            let body = params_str(params, "body");
            Ok(serde_json::json!({ "requestId": request_id, "fulfilled": true, "responseCode": status_code, "bodyLength": body.len() }))
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
        _ => Err(CDPError { code: -32601, message: format!("'Fetch.{}' wasn't found", command) }),
    }
}

// @trace TEST-CDP-001 [req:REQ-CDP-001] [level:unit] [nfr:TMG-CDP-01]
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 1. parse_message valid JSON → Some(CDPMessage) with correct id/method/params
    #[test]
    fn parse_message_valid_json() {
        let msg = parse_message(r#"{"id":1,"method":"Page.enable","params":{"url":"http://x"}}"#).unwrap();
        assert_eq!(msg.id, 1);
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
        assert_eq!(msg.id, 5);
        assert_eq!(msg.method, "Runtime.evaluate");
        assert_eq!(msg.session_id, Some("abc123".to_string()));
    }

    // 5. parse_message with null params
    #[test]
    fn parse_message_null_params() {
        let msg = parse_message(r#"{"id":2,"method":"Page.enable","params":null}"#).unwrap();
        assert_eq!(msg.id, 2);
        assert_eq!(msg.params, None);
    }

    // 6. serialize_response with result
    #[test]
    fn serialize_response_with_result() {
        let resp = CDPResponse {
            id: 1,
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
        let resp = CDPResponse {
            id: 2,
            result: None,
            error: Some(CDPError { code: -32601, message: "not found".into() }),
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
        let msg = CDPMessage { id: 1, method: "Foo.bar".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    // 9. handle_command Target.getTargets (no bridge) → ok with targetInfos
    #[test]
    fn handle_command_target_get_targets() {
        let msg = CDPMessage { id: 2, method: "Target.getTargets".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 3, method: "Target.createTarget".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["targetId"], "t1");
    }

    // 11. handle_command Target.closeTarget (no bridge) → ok with success:true
    #[test]
    fn handle_command_target_close_target() {
        let msg = CDPMessage { id: 4, method: "Target.closeTarget".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["success"], true);
    }

    // 12. handle_command Target.setAutoAttach → ok empty
    #[test]
    fn handle_command_target_set_auto_attach() {
        let msg = CDPMessage { id: 5, method: "Target.setAutoAttach".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 13. handle_command Page.enable → ok empty
    #[test]
    fn handle_command_page_enable() {
        let msg = CDPMessage { id: 6, method: "Page.enable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 14. handle_command Page.getLayoutMetrics → ok with contentSize
    #[test]
    fn handle_command_page_get_layout_metrics() {
        let msg = CDPMessage { id: 7, method: "Page.getLayoutMetrics".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 8, method: "Runtime.enable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["executionContextId"], 1);
    }

    // 16. handle_command Runtime.evaluate (no bridge, empty expr) → undefined result
    #[test]
    fn handle_command_runtime_evaluate_no_bridge() {
        let msg = CDPMessage {
            id: 9,
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
        let msg = CDPMessage { id: 10, method: "DOM.getDocument".into(), params: None, session_id: None };
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
        let msg = CDPMessage {
            id: 11,
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
        let msg = CDPMessage { id: 12, method: "Network.enable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 20. handle_command Network.getCookies → ok with empty cookies
    #[test]
    fn handle_command_network_get_cookies() {
        let msg = CDPMessage { id: 13, method: "Network.getCookies".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["cookies"], json!([]));
    }

    // 21. handle_command CSS.enable → ok empty
    #[test]
    fn handle_command_css_enable() {
        let msg = CDPMessage { id: 14, method: "CSS.enable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 22. handle_command CSS.getComputedStyleForNode → ok empty computedStyle
    #[test]
    fn handle_command_css_get_computed_style() {
        let msg = CDPMessage { id: 15, method: "CSS.getComputedStyleForNode".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["computedStyle"], json!([]));
    }

    // 23. handle_command Emulation.setDeviceMetricsOverride (no bridge) → ok empty
    #[test]
    fn handle_command_emulation_set_device_metrics() {
        let msg = CDPMessage {
            id: 16,
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
        let msg = CDPMessage {
            id: 17,
            method: "Input.dispatchMouseEvent".into(),
            params: Some(json!({"type": "mousePressed", "x": 100, "y": 200, "button": 0, "clickCount": 1})),
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
        let msg = CDPMessage { id: 18, method: "Overlay.enable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 26. handle_command Debugger.enable → ok empty
    #[test]
    fn handle_command_debugger_enable() {
        let msg = CDPMessage { id: 19, method: "Debugger.enable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 27. handle_command Debugger.setBreakpointByUrl → ok with breakpointId
    #[test]
    fn handle_command_debugger_set_breakpoint_by_url() {
        let msg = CDPMessage { id: 20, method: "Debugger.setBreakpointByUrl".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["breakpointId"], "1");
    }

    // 28. handle_command Log.enable → ok empty
    #[test]
    fn handle_command_log_enable() {
        let msg = CDPMessage { id: 21, method: "Log.enable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 29. handle_command Fetch.enable with patterns → ok with patternCount
    #[test]
    fn handle_command_fetch_enable_with_patterns() {
        let msg = CDPMessage {
            id: 22,
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
        let msg = CDPMessage {
            id: 23,
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

    // 31. CDPError clone + debug format
    #[test]
    fn cdp_error_clone_and_debug() {
        let err = CDPError { code: -32601, message: "not found".into() };
        let cloned = err.clone();
        assert_eq!(cloned.code, err.code);
        assert_eq!(cloned.message, err.message);
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("-32601"));
        assert!(debug_str.contains("not found"));
    }

    // 32. CDPEvent serialize
    #[test]
    fn cdp_event_serialize() {
        let ev = CDPEvent {
            method: "Page.loadEventFired".into(),
            params: Some(json!({"timestamp": 12345})),
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["method"], "Page.loadEventFired");
        assert_eq!(parsed["params"]["timestamp"], 12345);
    }

    // 33. CDPMessage deserialize with unicode method name
    #[test]
    fn parse_message_unicode_method() {
        let msg = parse_message(r#"{"id":99,"method":"Page.你好世界"}"#).unwrap();
        assert_eq!(msg.id, 99);
        assert_eq!(msg.method, "Page.你好世界");
    }

    // ─── CdpMessage parsing edge cases ─────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 34. CDPMessage with id = 0
    #[test]
    fn parse_message_id_zero() {
        let msg = parse_message(r#"{"id":0,"method":"Page.enable"}"#).unwrap();
        assert_eq!(msg.id, 0);
        assert_eq!(msg.method, "Page.enable");
    }

    // 35. CDPMessage with id = i64::MAX
    #[test]
    fn parse_message_id_max() {
        let msg = parse_message(r#"{"id":9223372036854775807,"method":"Page.enable"}"#).unwrap();
        assert_eq!(msg.id, i64::MAX);
    }

    // 36. CDPMessage with negative id
    #[test]
    fn parse_message_negative_id() {
        let msg = parse_message(r#"{"id":-1,"method":"Page.enable"}"#).unwrap();
        assert_eq!(msg.id, -1);
    }

    // 37. CDPMessage with id = i64::MIN
    #[test]
    fn parse_message_id_min() {
        let msg = parse_message(r#"{"id":-9223372036854775808,"method":"Page.enable"}"#).unwrap();
        assert_eq!(msg.id, i64::MIN);
    }

    // 38. CDPMessage with empty method string
    #[test]
    fn parse_message_empty_method() {
        let msg = parse_message(r#"{"id":1,"method":""}"#).unwrap();
        assert_eq!(msg.method, "");
    }

    // 39. CDPMessage with method containing no dot
    #[test]
    fn parse_message_method_no_dot() {
        let msg = parse_message(r#"{"id":1,"method":"NoDomain"}"#).unwrap();
        assert_eq!(msg.method, "NoDomain");
    }

    // 40. CDPMessage with method containing multiple dots
    #[test]
    fn parse_message_method_multiple_dots() {
        let msg = parse_message(r#"{"id":1,"method":"Page.navigate.to"}"#).unwrap();
        assert_eq!(msg.method, "Page.navigate.to");
        // splitn(2, '.') only splits on first dot
        let parts: Vec<&str> = msg.method.splitn(2, '.').collect();
        assert_eq!(parts[0], "Page");
        assert_eq!(parts[1], "navigate.to");
    }

    // 41. CDPMessage with empty string input
    #[test]
    fn parse_message_empty_string() {
        assert!(parse_message("").is_none());
    }

    // 42. CDPMessage with whitespace-only input
    #[test]
    fn parse_message_whitespace_only() {
        assert!(parse_message("   ").is_none());
    }

    // 43. CDPMessage with extra JSON fields (should succeed, ignores unknown)
    #[test]
    fn parse_message_extra_fields() {
        let msg = parse_message(r#"{"id":1,"method":"Page.enable","extra":"ignored"}"#);
        assert!(msg.is_some());
        assert_eq!(msg.unwrap().method, "Page.enable");
    }

    // 44. CDPMessage params as object
    #[test]
    fn parse_message_params_object() {
        let msg = parse_message(r#"{"id":1,"method":"Page.navigate","params":{"url":"http://x.com"}}"#).unwrap();
        assert!(msg.params.is_some());
        assert_eq!(msg.params.unwrap()["url"], "http://x.com");
    }

    // 45. CDPMessage params as array (unusual but valid JSON)
    #[test]
    fn parse_message_params_array() {
        let msg = parse_message(r#"{"id":1,"method":"Test.cmd","params":[1,2,3]}"#).unwrap();
        assert!(msg.params.is_some());
        assert!(msg.params.unwrap().is_array());
    }

    // 46. CDPMessage params as string (unusual but valid JSON)
    #[test]
    fn parse_message_params_string() {
        let msg = parse_message(r#"{"id":1,"method":"Test.cmd","params":"hello"}"#).unwrap();
        assert!(msg.params.is_some());
        assert!(msg.params.unwrap().is_string());
    }

    // 47. CDPMessage params as number (unusual but valid JSON)
    #[test]
    fn parse_message_params_number() {
        let msg = parse_message(r#"{"id":1,"method":"Test.cmd","params":42}"#).unwrap();
        assert!(msg.params.is_some());
        assert!(msg.params.unwrap().is_number());
    }

    // 48. CDPMessage params as boolean (unusual but valid JSON)
    #[test]
    fn parse_message_params_boolean() {
        let msg = parse_message(r#"{"id":1,"method":"Test.cmd","params":true}"#).unwrap();
        assert!(msg.params.is_some());
        assert!(msg.params.unwrap().is_boolean());
    }

    // 49. CDPMessage with very long session_id
    #[test]
    fn parse_message_long_session_id() {
        let long_session = "A".repeat(10000);
        let raw = format!(r#"{{"id":1,"method":"Page.enable","session_id":"{}"}}"#, long_session);
        let msg = parse_message(&raw).unwrap();
        assert_eq!(msg.session_id.unwrap().len(), 10000);
    }

    // 50. CDPMessage with empty session_id
    #[test]
    fn parse_message_empty_session_id() {
        let msg = parse_message(r#"{"id":1,"method":"Page.enable","session_id":""}"#).unwrap();
        assert_eq!(msg.session_id, Some("".to_string()));
    }

    // ─── CDPResponse serialization edge cases ──────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 51. CDPResponse with null result
    #[test]
    fn serialize_response_null_result() {
        let resp = CDPResponse {
            id: 1,
            result: Some(Value::Null),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"], Value::Null);
    }

    // 52. CDPResponse with empty object result
    #[test]
    fn serialize_response_empty_object_result() {
        let resp = CDPResponse {
            id: 2,
            result: Some(json!({})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"], json!({}));
    }

    // 53. CDPResponse with nested result
    #[test]
    fn serialize_response_nested_result() {
        let resp = CDPResponse {
            id: 3,
            result: Some(json!({"root": {"nodeId": 1, "children": [{"nodeId": 2}]}})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"]["root"]["nodeId"], 1);
        assert_eq!(parsed["result"]["root"]["children"][0]["nodeId"], 2);
    }

    // 54. CDPResponse with id = 0
    #[test]
    fn serialize_response_id_zero() {
        let resp = CDPResponse {
            id: 0,
            result: Some(json!({"ok": true})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], 0);
    }

    // 55. CDPResponse with negative id
    #[test]
    fn serialize_response_negative_id() {
        let resp = CDPResponse {
            id: -42,
            result: Some(json!({})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], -42);
    }

    // 56. CDPResponse with i64::MAX id
    #[test]
    fn serialize_response_max_id() {
        let resp = CDPResponse {
            id: i64::MAX,
            result: Some(json!({})),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["id"], i64::MAX);
    }

    // 57. CDPResponse with array result
    #[test]
    fn serialize_response_array_result() {
        let resp = CDPResponse {
            id: 5,
            result: Some(json!([1, 2, 3])),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"], json!([1, 2, 3]));
    }

    // 58. CDPResponse with string result
    #[test]
    fn serialize_response_string_result() {
        let resp = CDPResponse {
            id: 6,
            result: Some(json!("hello world")),
            error: None,
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"], "hello world");
    }

    // ─── CDPError code boundaries ──────────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 59. CDPError code -32700 (Parse error)
    #[test]
    fn cdp_error_code_parse_error() {
        let resp = CDPResponse {
            id: 1,
            result: None,
            error: Some(CDPError { code: -32700, message: "Parse error".into() }),
        };
        let s = serialize_response(&resp);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["error"]["code"], -32700);
        assert_eq!(parsed["error"]["message"], "Parse error");
    }

    // 60. CDPError code -32600 (Invalid Request)
    #[test]
    fn cdp_error_code_invalid_request() {
        let err = CDPError { code: -32600, message: "Invalid Request".into() };
        assert_eq!(err.code, -32600);
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("-32600"));
    }

    // 61. CDPError code -32601 (Method not found)
    #[test]
    fn cdp_error_code_method_not_found() {
        let err = CDPError { code: -32601, message: "Method not found".into() };
        assert_eq!(err.code, -32601);
    }

    // 62. CDPError code -32602 (Invalid params)
    #[test]
    fn cdp_error_code_invalid_params() {
        let err = CDPError { code: -32602, message: "Invalid params".into() };
        assert_eq!(err.code, -32602);
    }

    // 63. CDPError code -32603 (Internal error)
    #[test]
    fn cdp_error_code_internal_error() {
        let err = CDPError { code: -32603, message: "Internal error".into() };
        assert_eq!(err.code, -32603);
    }

    // 64. CDPError with custom error code
    #[test]
    fn cdp_error_custom_code() {
        let err = CDPError { code: -32000, message: "Server error".into() };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("-32000"));
        assert!(s.contains("Server error"));
    }

    // 65. CDPError with empty message
    #[test]
    fn cdp_error_empty_message() {
        let err = CDPError { code: -32601, message: String::new() };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("-32601"));
    }

    // 66. CDPError with unicode message
    #[test]
    fn cdp_error_unicode_message() {
        let err = CDPError { code: -32601, message: "方法未找到".into() };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("方法未找到"));
    }

    // ─── CDPEvent edge cases ───────────────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 67. CDPEvent with no params
    #[test]
    fn cdp_event_no_params() {
        let ev = CDPEvent {
            method: "Page.domContentEventFired".into(),
            params: None,
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["method"], "Page.domContentEventFired");
        assert!(parsed.get("params").is_none(), "params should be skipped when None");
    }

    // 68. CDPEvent with large data
    #[test]
    fn cdp_event_large_data() {
        let large_string = "X".repeat(100_000);
        let ev = CDPEvent {
            method: "Network.dataReceived".into(),
            params: Some(json!({ "dataLength": large_string.len(), "encodedDataLength": large_string.len() })),
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["params"]["dataLength"], 100_000);
    }

    // 69. CDPEvent with nested params
    #[test]
    fn cdp_event_nested_params() {
        let ev = CDPEvent {
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

    // 70. CDPEvent with null params
    #[test]
    fn cdp_event_null_params() {
        let ev = CDPEvent {
            method: "Page.frameResized".into(),
            params: Some(Value::Null),
        };
        let s = serialize_event(&ev);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["params"], Value::Null);
    }

    // 71. CDPEvent with empty method
    #[test]
    fn cdp_event_empty_method() {
        let ev = CDPEvent {
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
        let msg = CDPMessage { id: 1, method: "NoDomain".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 2, method: String::new(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    // 74. handle_command with known domain but unknown command
    #[test]
    fn handle_command_known_domain_unknown_command() {
        let msg = CDPMessage { id: 3, method: "Page.nonExistentCommand".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 4, method: "Target.getTargetInfo".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 5, method: "Target.attachToTarget".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 6, method: "Target.detachFromTarget".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 78. handle_command Target.setDiscoverTargets → ok empty
    #[test]
    fn handle_command_target_set_discover_targets() {
        let msg = CDPMessage { id: 7, method: "Target.setDiscoverTargets".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 79. handle_command Target.getTargetTargets → ok (alias for getTargets)
    #[test]
    fn handle_command_target_get_target_targets() {
        let msg = CDPMessage { id: 8, method: "Target.getTargetTargets".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("targetInfos").unwrap().as_array().unwrap().len() > 0);
    }

    // 80. handle_command Page.navigate (no bridge) → ok with default url
    #[test]
    fn handle_command_page_navigate_no_bridge_default_url() {
        let msg = CDPMessage { id: 9, method: "Page.navigate".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("frameId").is_some());
    }

    // 81. handle_command Page.navigate (no bridge) with url param
    #[test]
    fn handle_command_page_navigate_with_url() {
        let msg = CDPMessage {
            id: 10,
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
        let msg = CDPMessage { id: 11, method: "Page.reload".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 12, method: "Page.getFrameTree".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 13, method: "Page.getNavigationHistory".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 14, method: "Page.captureScreenshot".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["data"], "");
    }

    // 86. handle_command Page.addScriptToEvaluateOnNewDocument (no bridge, empty source)
    #[test]
    fn handle_command_page_add_script_empty_source() {
        let msg = CDPMessage { id: 15, method: "Page.addScriptToEvaluateOnNewDocument".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["identifier"], "1");
    }

    // 87. handle_command Page.removeScriptToEvaluateOnNewDocument → ok empty
    #[test]
    fn handle_command_page_remove_script() {
        let msg = CDPMessage { id: 16, method: "Page.removeScriptToEvaluateOnNewDocument".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 88. handle_command Page.setContent → ok empty
    #[test]
    fn handle_command_page_set_content() {
        let msg = CDPMessage { id: 17, method: "Page.setContent".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 89. handle_command Page.close → ok empty
    #[test]
    fn handle_command_page_close() {
        let msg = CDPMessage { id: 18, method: "Page.close".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    // 90. handle_command Page.bringToFront → ok empty
    #[test]
    fn handle_command_page_bring_to_front() {
        let msg = CDPMessage { id: 19, method: "Page.bringToFront".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 91. handle_command Page.disable → ok empty
    #[test]
    fn handle_command_page_disable() {
        let msg = CDPMessage { id: 20, method: "Page.disable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 92. handle_command Runtime.disable → ok empty
    #[test]
    fn handle_command_runtime_disable() {
        let msg = CDPMessage { id: 21, method: "Runtime.disable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 93. handle_command Runtime.callFunctionOn → ok
    #[test]
    fn handle_command_runtime_call_function_on() {
        let msg = CDPMessage { id: 22, method: "Runtime.callFunctionOn".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 94. handle_command Runtime.getProperties → ok with empty array
    #[test]
    fn handle_command_runtime_get_properties() {
        let msg = CDPMessage { id: 23, method: "Runtime.getProperties".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"], json!([]));
    }

    // 95. handle_command Runtime.evaluateAsync → ok
    #[test]
    fn handle_command_runtime_evaluate_async() {
        let msg = CDPMessage { id: 24, method: "Runtime.evaluateAsync".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 96. handle_command Runtime.runScript → ok
    #[test]
    fn handle_command_runtime_run_script() {
        let msg = CDPMessage { id: 25, method: "Runtime.runScript".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 97. handle_command Runtime.releaseObject → ok empty
    #[test]
    fn handle_command_runtime_release_object() {
        let msg = CDPMessage { id: 26, method: "Runtime.releaseObject".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 98. handle_command Runtime.releaseObjectGroup → ok empty
    #[test]
    fn handle_command_runtime_release_object_group() {
        let msg = CDPMessage { id: 27, method: "Runtime.releaseObjectGroup".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 99. handle_command Runtime.compileScript → ok empty
    #[test]
    fn handle_command_runtime_compile_script() {
        let msg = CDPMessage { id: 28, method: "Runtime.compileScript".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 100. handle_command Runtime.unknown → error -32601
    #[test]
    fn handle_command_runtime_unknown_command() {
        let msg = CDPMessage { id: 29, method: "Runtime.unknownMethod".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 30, method: "DOM.enable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 102. handle_command DOM.disable → ok empty
    #[test]
    fn handle_command_dom_disable() {
        let msg = CDPMessage { id: 31, method: "DOM.disable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 103. handle_command DOM.describeNode → ok
    #[test]
    fn handle_command_dom_describe_node() {
        let msg = CDPMessage { id: 32, method: "DOM.describeNode".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("node").is_some());
    }

    // 104. handle_command DOM.getBoxModel → ok with model
    #[test]
    fn handle_command_dom_get_box_model() {
        let msg = CDPMessage { id: 33, method: "DOM.getBoxModel".into(), params: None, session_id: None };
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
        let msg = CDPMessage {
            id: 34,
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
        let msg = CDPMessage { id: 35, method: "DOM.removeAttribute".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 107. handle_command DOM.setOuterHTML → ok empty
    #[test]
    fn handle_command_dom_set_outer_html() {
        let msg = CDPMessage { id: 36, method: "DOM.setOuterHTML".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 108. handle_command DOM.insertBefore → ok empty
    #[test]
    fn handle_command_dom_insert_before() {
        let msg = CDPMessage { id: 37, method: "DOM.insertBefore".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 109. handle_command DOM.removeNode → ok empty
    #[test]
    fn handle_command_dom_remove_node() {
        let msg = CDPMessage { id: 38, method: "DOM.removeNode".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 110. handle_command DOM.getOuterHTML (no bridge) → ok with default html
    #[test]
    fn handle_command_dom_get_outer_html_no_bridge() {
        let msg = CDPMessage { id: 39, method: "DOM.getOuterHTML".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result.get("outerHTML").is_some());
    }

    // 111. handle_command DOM.resolveNode → ok
    #[test]
    fn handle_command_dom_resolve_node() {
        let msg = CDPMessage { id: 40, method: "DOM.resolveNode".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["object"]["type"], "node");
    }

    // 112. handle_command DOM.pushNodesByBackendIdsToFrontend → ok
    #[test]
    fn handle_command_dom_push_nodes_by_backend_ids() {
        let msg = CDPMessage { id: 41, method: "DOM.pushNodesByBackendIdsToFrontend".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["nodeIds"], json!([]));
    }

    // 113. handle_command DOM.unknown → error -32601
    #[test]
    fn handle_command_dom_unknown_command() {
        let msg = CDPMessage { id: 42, method: "DOM.nonExistent".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 114. handle_command Network.disable → ok empty
    #[test]
    fn handle_command_network_disable() {
        let msg = CDPMessage { id: 43, method: "Network.disable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 115. handle_command Network.getResponseBody → ok
    #[test]
    fn handle_command_network_get_response_body() {
        let msg = CDPMessage { id: 44, method: "Network.getResponseBody".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 45, method: "Network.setCacheDisabled".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 117. handle_command Network.setExtraHTTPHeaders → ok empty
    #[test]
    fn handle_command_network_set_extra_http_headers() {
        let msg = CDPMessage { id: 46, method: "Network.setExtraHTTPHeaders".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 118. handle_command Network.emulateNetworkConditions → ok empty
    #[test]
    fn handle_command_network_emulate_conditions() {
        let msg = CDPMessage { id: 47, method: "Network.emulateNetworkConditions".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 119. handle_command Network.getAllCookies → ok with empty cookies
    #[test]
    fn handle_command_network_get_all_cookies() {
        let msg = CDPMessage { id: 48, method: "Network.getAllCookies".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["cookies"], json!([]));
    }

    // 120. handle_command Network.deleteCookies → ok empty
    #[test]
    fn handle_command_network_delete_cookies() {
        let msg = CDPMessage { id: 49, method: "Network.deleteCookies".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 121. handle_command Network.setCookie → ok empty
    #[test]
    fn handle_command_network_set_cookie() {
        let msg = CDPMessage { id: 50, method: "Network.setCookie".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 122. handle_command Network.setRequestInterception → ok empty
    #[test]
    fn handle_command_network_set_request_interception() {
        let msg = CDPMessage { id: 51, method: "Network.setRequestInterception".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 123. handle_command Network.continueInterceptedRequest → ok empty
    #[test]
    fn handle_command_network_continue_intercepted_request() {
        let msg = CDPMessage { id: 52, method: "Network.continueInterceptedRequest".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 124. handle_command Network.unknown → error -32601
    #[test]
    fn handle_command_network_unknown() {
        let msg = CDPMessage { id: 53, method: "Network.bogus".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 125. handle_command CSS.disable → ok empty
    #[test]
    fn handle_command_css_disable() {
        let msg = CDPMessage { id: 54, method: "CSS.disable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 126. handle_command CSS.getMatchedStylesForNode → ok
    #[test]
    fn handle_command_css_get_matched_styles() {
        let msg = CDPMessage { id: 55, method: "CSS.getMatchedStylesForNode".into(), params: None, session_id: None };
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
        let msg = CDPMessage { id: 56, method: "CSS.getInlineStylesForNode".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["inlineStyle"], Value::Null);
    }

    // 128. handle_command CSS.setStyleTexts → ok
    #[test]
    fn handle_command_css_set_style_texts() {
        let msg = CDPMessage { id: 57, method: "CSS.setStyleTexts".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["styles"], json!([]));
    }

    // 129. handle_command CSS.unknown → error -32601
    #[test]
    fn handle_command_css_unknown() {
        let msg = CDPMessage { id: 58, method: "CSS.bogus".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 130. handle_command Emulation.clearDeviceMetricsOverride → ok empty
    #[test]
    fn handle_command_emulation_clear_device_metrics() {
        let msg = CDPMessage { id: 59, method: "Emulation.clearDeviceMetricsOverride".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 131. handle_command Emulation.setUserAgentOverride (no bridge, empty ua) → ok empty
    #[test]
    fn handle_command_emulation_set_user_agent_no_bridge() {
        let msg = CDPMessage { id: 60, method: "Emulation.setUserAgentOverride".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 132. handle_command Emulation.setTouchEmulationEnabled → ok empty
    #[test]
    fn handle_command_emulation_set_touch_emulation() {
        let msg = CDPMessage { id: 61, method: "Emulation.setTouchEmulationEnabled".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 133. handle_command Emulation.setScriptExecutionDisabled → ok empty
    #[test]
    fn handle_command_emulation_set_script_execution_disabled() {
        let msg = CDPMessage { id: 62, method: "Emulation.setScriptExecutionDisabled".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 134. handle_command Emulation.setFocusEmulationEnabled → ok empty
    #[test]
    fn handle_command_emulation_set_focus_emulation() {
        let msg = CDPMessage { id: 63, method: "Emulation.setFocusEmulationEnabled".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 135. handle_command Emulation.setCPUThrottlingRate → ok empty
    #[test]
    fn handle_command_emulation_set_cpu_throttling() {
        let msg = CDPMessage { id: 64, method: "Emulation.setCPUThrottlingRate".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 136. handle_command Emulation.setDefaultBackgroundColorOverride → ok empty
    #[test]
    fn handle_command_emulation_set_default_bg_color() {
        let msg = CDPMessage { id: 65, method: "Emulation.setDefaultBackgroundColorOverride".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 137. handle_command Emulation.unknown → error -32601
    #[test]
    fn handle_command_emulation_unknown() {
        let msg = CDPMessage { id: 66, method: "Emulation.bogus".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 138. handle_command Input.dispatchKeyEvent (no bridge) → ok empty
    #[test]
    fn handle_command_input_dispatch_key_no_bridge() {
        let msg = CDPMessage { id: 67, method: "Input.dispatchKeyEvent".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 139. handle_command Input.dispatchTouchEvent → ok empty
    #[test]
    fn handle_command_input_dispatch_touch() {
        let msg = CDPMessage { id: 68, method: "Input.dispatchTouchEvent".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 140. handle_command Input.insertText (no bridge, empty text) → ok empty
    #[test]
    fn handle_command_input_insert_text_no_bridge() {
        let msg = CDPMessage { id: 69, method: "Input.insertText".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 141. handle_command Input.setIgnoreInputEvents → ok empty
    #[test]
    fn handle_command_input_set_ignore_input_events() {
        let msg = CDPMessage { id: 70, method: "Input.setIgnoreInputEvents".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 142. handle_command Input.setInterceptDrags → ok empty
    #[test]
    fn handle_command_input_set_intercept_drags() {
        let msg = CDPMessage { id: 71, method: "Input.setInterceptDrags".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 143. handle_command Input.unknown → error -32601
    #[test]
    fn handle_command_input_unknown() {
        let msg = CDPMessage { id: 72, method: "Input.bogus".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 144. handle_command Overlay.highlightNode → ok empty
    #[test]
    fn handle_command_overlay_highlight_node() {
        let msg = CDPMessage { id: 73, method: "Overlay.highlightNode".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 145. handle_command Overlay.hideHighlight → ok empty
    #[test]
    fn handle_command_overlay_hide_highlight() {
        let msg = CDPMessage { id: 74, method: "Overlay.hideHighlight".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 146. handle_command Overlay.setInspectMode → ok empty
    #[test]
    fn handle_command_overlay_set_inspect_mode() {
        let msg = CDPMessage { id: 75, method: "Overlay.setInspectMode".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 147. handle_command Overlay.setPausedInDebuggerMessage → ok empty
    #[test]
    fn handle_command_overlay_set_paused_in_debugger() {
        let msg = CDPMessage { id: 76, method: "Overlay.setPausedInDebuggerMessage".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 148. handle_command Overlay.unknown → error -32601
    #[test]
    fn handle_command_overlay_unknown() {
        let msg = CDPMessage { id: 77, method: "Overlay.bogus".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 149. handle_command Debugger.disable → ok empty
    #[test]
    fn handle_command_debugger_disable() {
        let msg = CDPMessage { id: 78, method: "Debugger.disable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 150. handle_command Debugger.removeBreakpoint → ok empty
    #[test]
    fn handle_command_debugger_remove_breakpoint() {
        let msg = CDPMessage { id: 79, method: "Debugger.removeBreakpoint".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 151. handle_command Debugger.pause → ok empty
    #[test]
    fn handle_command_debugger_pause() {
        let msg = CDPMessage { id: 80, method: "Debugger.pause".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 152. handle_command Debugger.resume → ok empty
    #[test]
    fn handle_command_debugger_resume() {
        let msg = CDPMessage { id: 81, method: "Debugger.resume".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 153. handle_command Debugger.stepOver → ok empty
    #[test]
    fn handle_command_debugger_step_over() {
        let msg = CDPMessage { id: 82, method: "Debugger.stepOver".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 154. handle_command Debugger.stepInto → ok empty
    #[test]
    fn handle_command_debugger_step_into() {
        let msg = CDPMessage { id: 83, method: "Debugger.stepInto".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 155. handle_command Debugger.stepOut → ok empty
    #[test]
    fn handle_command_debugger_step_out() {
        let msg = CDPMessage { id: 84, method: "Debugger.stepOut".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 156. handle_command Debugger.setSkipAllPauses → ok empty
    #[test]
    fn handle_command_debugger_set_skip_all_pauses() {
        let msg = CDPMessage { id: 85, method: "Debugger.setSkipAllPauses".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 157. handle_command Debugger.setBreakpointsActive → ok empty
    #[test]
    fn handle_command_debugger_set_breakpoints_active() {
        let msg = CDPMessage { id: 86, method: "Debugger.setBreakpointsActive".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 158. handle_command Debugger.evaluateOnCallFrame → ok
    #[test]
    fn handle_command_debugger_evaluate_on_call_frame() {
        let msg = CDPMessage { id: 87, method: "Debugger.evaluateOnCallFrame".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    // 159. handle_command Debugger.getPossibleBreakpoints → ok
    #[test]
    fn handle_command_debugger_get_possible_breakpoints() {
        let msg = CDPMessage { id: 88, method: "Debugger.getPossibleBreakpoints".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["locations"], json!([]));
    }

    // 160. handle_command Debugger.getScriptSource → ok
    #[test]
    fn handle_command_debugger_get_script_source() {
        let msg = CDPMessage { id: 89, method: "Debugger.getScriptSource".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["scriptSource"], "");
    }

    // 161. handle_command Debugger.setPauseOnExceptions → ok empty
    #[test]
    fn handle_command_debugger_set_pause_on_exceptions() {
        let msg = CDPMessage { id: 90, method: "Debugger.setPauseOnExceptions".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 162. handle_command Debugger.unknown → error -32601
    #[test]
    fn handle_command_debugger_unknown() {
        let msg = CDPMessage { id: 91, method: "Debugger.bogus".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 163. handle_command Log.disable → ok empty
    #[test]
    fn handle_command_log_disable() {
        let msg = CDPMessage { id: 92, method: "Log.disable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 164. handle_command Log.clear → ok empty
    #[test]
    fn handle_command_log_clear() {
        let msg = CDPMessage { id: 93, method: "Log.clear".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 165. handle_command Log.startViolationsReport → ok empty
    #[test]
    fn handle_command_log_start_violations_report() {
        let msg = CDPMessage { id: 94, method: "Log.startViolationsReport".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 166. handle_command Log.stopViolationsReport → ok empty
    #[test]
    fn handle_command_log_stop_violations_report() {
        let msg = CDPMessage { id: 95, method: "Log.stopViolationsReport".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 167. handle_command Log.unknown → error -32601
    #[test]
    fn handle_command_log_unknown() {
        let msg = CDPMessage { id: 96, method: "Log.bogus".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // 168. handle_command Fetch.disable → ok empty
    #[test]
    fn handle_command_fetch_disable() {
        let msg = CDPMessage { id: 97, method: "Fetch.disable".into(), params: None, session_id: None };
        let params = msg.params.clone();
        let resp = handle_command(msg, "t1", &params, None);
        assert!(resp.error.is_none());
    }

    // 169. handle_command Fetch.continueWithResponse → ok
    #[test]
    fn handle_command_fetch_continue_with_response() {
        let msg = CDPMessage {
            id: 98,
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
        let msg = CDPMessage {
            id: 99,
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
        let msg = CDPMessage {
            id: 100,
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
        let msg = CDPMessage {
            id: 101,
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
        let msg = CDPMessage {
            id: 102,
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
        let msg = CDPMessage {
            id: 103,
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
        let msg = CDPMessage { id: 104, method: "Fetch.enable".into(), params: None, session_id: None };
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
        let msg = CDPMessage {
            id: 105,
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
        let msg = CDPMessage { id: 106, method: "Fetch.bogus".into(), params: None, session_id: None };
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
        let result = bridge_send(None, BridgeCommand::GetTitle);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("no servo bridge connected"));
    }

    // ─── CDPMessage Debug trait ────────────────────────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    // 186. CDPMessage debug format
    #[test]
    fn cdp_message_debug_format() {
        let msg = CDPMessage { id: 1, method: "Page.enable".into(), params: None, session_id: None };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("CDPMessage"));
        assert!(debug.contains("Page.enable"));
    }

    // 187. CDPMessage clone
    #[test]
    fn cdp_message_clone() {
        let msg = CDPMessage { id: 1, method: "Page.enable".into(), params: Some(json!({"k": "v"})), session_id: Some("s1".into()) };
        let cloned = msg.clone();
        assert_eq!(cloned.id, msg.id);
        assert_eq!(cloned.method, msg.method);
        assert_eq!(cloned.params, msg.params);
        assert_eq!(cloned.session_id, msg.session_id);
    }
}
