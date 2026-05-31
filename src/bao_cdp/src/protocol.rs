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
                .unwrap();
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
}
