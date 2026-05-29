// REQ-CDP-001: CDP protocol message types and 11-domain dispatch
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Serialize)]
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

pub fn handle_command(msg: CDPMessage, target_id: &str, params: &Option<Value>) -> CDPResponse {
    let parts: Vec<&str> = msg.method.splitn(2, '.').collect();
    let domain = parts.first().map(|s| *s).unwrap_or("");
    let command = parts.get(1).map(|s| *s).unwrap_or("");

    let result = match domain {
        "Target" => handle_target(command, target_id),
        "Page" => handle_page(command, params),
        "Runtime" => handle_runtime(command, params),
        "DOM" => handle_dom(command),
        "Network" => handle_network(command),
        "CSS" => handle_css(command),
        "Emulation" => handle_emulation(command, params),
        "Input" => handle_input(command),
        "Overlay" => handle_overlay(command),
        "Debugger" => handle_debugger(command),
        "Log" => handle_log(command),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'{}' wasn't found", msg.method),
        }),
    };

    match result {
        Ok(r) => CDPResponse {
            id: msg.id,
            result: Some(r),
            error: None,
        },
        Err(e) => CDPResponse {
            id: msg.id,
            result: None,
            error: Some(e),
        },
    }
}

pub fn serialize_response(resp: &CDPResponse) -> String {
    serde_json::to_string(resp).unwrap_or_else(|_| r#"{"id":0,"error":{"code":-32700,"message":"serialize error"}}"#.into())
}

pub fn serialize_event(ev: &CDPEvent) -> String {
    serde_json::to_string(ev).unwrap_or_else(|_| "{}".into())
}

type HandlerResult = Result<Value, CDPError>;

fn handle_target(command: &str, target_id: &str) -> HandlerResult {
    match command {
        "getTargets" | "getTargetTargets" => Ok(serde_json::json!({
            "targetInfos": [{
                "targetId": target_id,
                "type": "page",
                "title": "Bao",
                "url": "about:blank",
                "attached": true
            }]
        })),
        "createTarget" => Ok(serde_json::json!({ "targetId": target_id })),
        "closeTarget" => Ok(serde_json::json!({ "success": true })),
        "setAutoAttach" | "setDiscoverTargets" => Ok(serde_json::json!({})),
        "getTargetInfo" => Ok(serde_json::json!({
            "targetInfo": {
                "targetId": target_id,
                "type": "page",
                "title": "Bao",
                "url": "about:blank",
                "attached": true
            }
        })),
        "attachToTarget" => Ok(serde_json::json!({ "sessionId": format!("{:016x}", target_id.chars().map(|c| c as u64).sum::<u64>()) })),
        "detachFromTarget" => Ok(serde_json::json!({})),
        "sendMessageToTarget" => Ok(serde_json::json!({})),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Target.{}' wasn't found", command),
        }),
    }
}

fn handle_page(command: &str, params: &Option<Value>) -> HandlerResult {
    match command {
        "enable" | "disable" => Ok(serde_json::json!({})),
        "navigate" => {
            let url = params.as_ref()
                .and_then(|p| p.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("about:blank");
            Ok(serde_json::json!({
                "frameId": "0",
                "loaderId": format!("{:016x}", url.len() as u64)
            }))
        }
        "reload" => Ok(serde_json::json!({ "frameId": "0", "loaderId": "0" })),
        "getFrameTree" => Ok(serde_json::json!({
            "frameTree": {
                "frame": {
                    "id": "0",
                    "url": "about:blank",
                    "loaderId": "0",
                    "mimeType": "text/html"
                }
            }
        })),
        "getNavigationHistory" => Ok(serde_json::json!({
            "currentIndex": 0,
            "entries": [{"id": 0, "url": "about:blank", "title": ""}]
        })),
        "captureScreenshot" => Ok(serde_json::json!({ "data": "" })),
        "setContent" | "close" | "bringToFront" => Ok(serde_json::json!({})),
        "getLayoutMetrics" => Ok(serde_json::json!({
            "contentSize": {"x": 0, "y": 0, "width": 1920, "height": 1080},
            "cssContentSize": {"x": 0, "y": 0, "width": 1920, "height": 1080}
        })),
        "addScriptToEvaluateOnNewDocument" => Ok(serde_json::json!({ "identifier": "1" })),
        "removeScriptToEvaluateOnNewDocument" => Ok(serde_json::json!({})),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Page.{}' wasn't found", command),
        }),
    }
}

fn handle_runtime(command: &str, _params: &Option<Value>) -> HandlerResult {
    match command {
        "enable" => Ok(serde_json::json!({
            "executionContextId": 1
        })),
        "disable" => Ok(serde_json::json!({})),
        "evaluate" => Ok(serde_json::json!({
            "result": {"type": "undefined"},
            "exceptionDetails": null
        })),
        "callFunctionOn" => Ok(serde_json::json!({
            "result": {"type": "undefined"}
        })),
        "getProperties" => Ok(serde_json::json!({ "result": [] })),
        "evaluateAsync" => Ok(serde_json::json!({ "result": {"type": "undefined"} })),
        "releaseObject" | "releaseObjectGroup" => Ok(serde_json::json!({})),
        "callArgument" => Ok(serde_json::json!({})),
        "compileScript" => Ok(serde_json::json!({})),
        "runScript" => Ok(serde_json::json!({ "result": {"type": "undefined"} })),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Runtime.{}' wasn't found", command),
        }),
    }
}

fn handle_dom(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => Ok(serde_json::json!({})),
        "getDocument" => Ok(serde_json::json!({
            "root": {
                "nodeId": 1,
                "backendNodeId": 1,
                "nodeType": 9,
                "nodeName": "#document",
                "localName": "",
                "nodeValue": "",
                "childNodeCount": 1,
                "children": [{
                    "nodeId": 2,
                    "backendNodeId": 2,
                    "nodeType": 1,
                    "nodeName": "HTML",
                    "localName": "html",
                    "nodeValue": "",
                    "childNodeCount": 2
                }]
            }
        })),
        "describeNode" => Ok(serde_json::json!({
            "node": {"nodeId": 1, "nodeType": 1, "nodeName": "HTML"}
        })),
        "querySelector" => Ok(serde_json::json!({ "nodeId": 0 })),
        "querySelectorAll" => Ok(serde_json::json!({ "nodeIds": [] })),
        "getBoxModel" => Ok(serde_json::json!({
            "model": {
                "width": 1920,
                "height": 1080,
                "content": [0, 0, 1920, 0, 1920, 1080, 0, 1080]
            }
        })),
        "setAttributeValue" | "removeAttribute" => Ok(serde_json::json!({})),
        "getOuterHTML" => Ok(serde_json::json!({ "outerHTML": "<html><body></body></html>" })),
        "setOuterHTML" | "insertBefore" | "removeNode" => Ok(serde_json::json!({})),
        "resolveNode" => Ok(serde_json::json!({ "object": {"type": "node"} })),
        "pushNodesByBackendIdsToFrontend" => Ok(serde_json::json!({ "nodeIds": [] })),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'DOM.{}' wasn't found", command),
        }),
    }
}

fn handle_network(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => Ok(serde_json::json!({})),
        "getResponseBody" => Ok(serde_json::json!({ "body": "", "base64Encoded": false })),
        "setCacheDisabled" | "setExtraHTTPHeaders" => Ok(serde_json::json!({})),
        "emulateNetworkConditions" => Ok(serde_json::json!({})),
        "setRequestInterception" => Ok(serde_json::json!({})),
        "continueInterceptedRequest" => Ok(serde_json::json!({})),
        "getCookies" => Ok(serde_json::json!({ "cookies": [] })),
        "getAllCookies" => Ok(serde_json::json!({ "cookies": [] })),
        "deleteCookies" => Ok(serde_json::json!({})),
        "setCookie" => Ok(serde_json::json!({ "success": true })),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Network.{}' wasn't found", command),
        }),
    }
}

fn handle_css(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => Ok(serde_json::json!({})),
        "getComputedStyleForNode" => Ok(serde_json::json!({ "computedStyle": [] })),
        "getMatchedStylesForNode" => Ok(serde_json::json!({
            "matchedCSSRules": [],
            "inlineStyle": null,
            "attributesStyle": null
        })),
        "getInlineStylesForNode" => Ok(serde_json::json!({ "inlineStyle": null })),
        "setStyleTexts" => Ok(serde_json::json!({ "styles": [] })),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'CSS.{}' wasn't found", command),
        }),
    }
}

fn handle_emulation(command: &str, _params: &Option<Value>) -> HandlerResult {
    match command {
        "setDeviceMetricsOverride" => Ok(serde_json::json!({})),
        "clearDeviceMetricsOverride" => Ok(serde_json::json!({})),
        "setUserAgentOverride" => Ok(serde_json::json!({})),
        "setTouchEmulationEnabled" => Ok(serde_json::json!({})),
        "setScriptExecutionDisabled" => Ok(serde_json::json!({})),
        "setFocusEmulationEnabled" => Ok(serde_json::json!({})),
        "setCPUThrottlingRate" => Ok(serde_json::json!({})),
        "setDefaultBackgroundColorOverride" => Ok(serde_json::json!({})),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Emulation.{}' wasn't found", command),
        }),
    }
}

fn handle_input(command: &str) -> HandlerResult {
    match command {
        "dispatchMouseEvent" | "dispatchMouseEvent" => Ok(serde_json::json!({})),
        "dispatchKeyEvent" => Ok(serde_json::json!({})),
        "dispatchTouchEvent" => Ok(serde_json::json!({})),
        "insertText" => Ok(serde_json::json!({})),
        "setIgnoreInputEvents" => Ok(serde_json::json!({})),
        "setInterceptDrags" => Ok(serde_json::json!({})),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Input.{}' wasn't found", command),
        }),
    }
}

fn handle_overlay(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => Ok(serde_json::json!({})),
        "highlightNode" | "hideHighlight" => Ok(serde_json::json!({})),
        "setInspectMode" => Ok(serde_json::json!({})),
        "setPausedInDebuggerMessage" => Ok(serde_json::json!({})),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Overlay.{}' wasn't found", command),
        }),
    }
}

fn handle_debugger(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" => Ok(serde_json::json!({})),
        "setBreakpointByUrl" => Ok(serde_json::json!({
            "breakpointId": "1",
            "locations": []
        })),
        "removeBreakpoint" => Ok(serde_json::json!({})),
        "pause" | "resume" => Ok(serde_json::json!({})),
        "stepOver" | "stepInto" | "stepOut" => Ok(serde_json::json!({})),
        "setSkipAllPauses" => Ok(serde_json::json!({})),
        "setBreakpointsActive" => Ok(serde_json::json!({})),
        "evaluateOnCallFrame" => Ok(serde_json::json!({ "result": {"type": "undefined"} })),
        "getPossibleBreakpoints" => Ok(serde_json::json!({ "locations": [] })),
        "getScriptSource" => Ok(serde_json::json!({ "scriptSource": "" })),
        "setPauseOnExceptions" => Ok(serde_json::json!({})),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Debugger.{}' wasn't found", command),
        }),
    }
}

fn handle_log(command: &str) -> HandlerResult {
    match command {
        "enable" | "disable" | "clear" => Ok(serde_json::json!({})),
        "startViolationsReport" | "stopViolationsReport" => Ok(serde_json::json!({})),
        _ => Err(CDPError {
            code: -32601,
            message: format!("'Log.{}' wasn't found", command),
        }),
    }
}
