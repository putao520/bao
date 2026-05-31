// @trace REQ-CDP-002
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

pub struct RuntimeHandler {
    bridge: BridgeSender,
}

impl RuntimeHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        RuntimeHandler { bridge }
    }
}

impl DomainHandler for RuntimeHandler {
    fn domain_name(&self) -> &'static str { "Runtime" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Runtime.enable" => Ok(json!({ "executionContextId": 1 })),
            "Runtime.disable" => Ok(json!({})),
            "Runtime.evaluate" => {
                let expression = params.get("expression").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let return_by_value = params.get("returnByValue").and_then(|v| v.as_bool()).unwrap_or(true);
                if !expression.is_empty() {
                    let resp = self.bridge.send(BridgeCommand::EvaluateJs { expression, return_by_value });
                    resp.result.map_err(|e| CdpError { code: -32603, message: e })
                } else {
                    Ok(json!({ "result": { "type": "undefined" }, "exceptionDetails": null }))
                }
            }
            "Runtime.callFunctionOn" => Ok(json!({ "result": { "type": "undefined" } })),
            "Runtime.getProperties" => Ok(json!({ "result": [] })),
            "Runtime.evaluateAsync" | "Runtime.runScript" => Ok(json!({ "result": { "type": "undefined" } })),
            "Runtime.releaseObject" | "Runtime.releaseObjectGroup" | "Runtime.compileScript" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}
