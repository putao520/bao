// @trace REQ-CDP-005
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

pub struct DomHandler {
    bridge: BridgeSender,
}

impl DomHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        DomHandler { bridge }
    }
}

fn bridge_send(bridge: &BridgeSender, cmd: BridgeCommand) -> Result<Value, CdpError> {
    let resp = bridge.send(cmd);
    resp.result.map_err(|e| CdpError { code: -32603, message: e })
}

fn ps(params: &Value, key: &str) -> String {
    params.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

impl DomainHandler for DomHandler {
    fn domain_name(&self) -> &'static str { "DOM" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "DOM.enable" | "DOM.disable" => Ok(json!({})),
            "DOM.getDocument" => {
                bridge_send(&self.bridge, BridgeCommand::GetDocument)
            }
            "DOM.describeNode" => Ok(json!({ "node": { "nodeId": 1, "nodeType": 1, "nodeName": "HTML" } })),
            "DOM.querySelector" => {
                let selector = ps(&params, "selector");
                if !selector.is_empty() {
                    bridge_send(&self.bridge, BridgeCommand::QuerySelector { selector })
                } else {
                    Ok(json!({ "nodeId": 0 }))
                }
            }
            "DOM.querySelectorAll" => {
                let selector = ps(&params, "selector");
                if !selector.is_empty() {
                    bridge_send(&self.bridge, BridgeCommand::QuerySelectorAll { selector })
                } else {
                    Ok(json!({ "nodeIds": [] }))
                }
            }
            "DOM.getBoxModel" => Ok(json!({
                "model": { "width": 1920, "height": 1080, "content": [0, 0, 1920, 0, 1920, 1080, 0, 1080] }
            })),
            "DOM.setAttributeValue" => {
                let node_id = params.get("nodeId").and_then(|v| v.as_i64()).unwrap_or(0);
                let name = ps(&params, "name");
                let value = ps(&params, "value");
                bridge_send(&self.bridge, BridgeCommand::SetAttributeValue { node_id, name, value })
            }
            "DOM.removeAttribute" | "DOM.setOuterHTML" | "DOM.insertBefore" | "DOM.removeNode" => Ok(json!({})),
            "DOM.getOuterHTML" => {
                let node_id = params.get("nodeId").and_then(|v| v.as_i64());
                bridge_send(&self.bridge, BridgeCommand::GetOuterHtml { node_id })
            }
            "DOM.resolveNode" => Ok(json!({ "object": { "type": "node" } })),
            "DOM.pushNodesByBackendIdsToFrontend" => Ok(json!({ "nodeIds": [] })),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}
