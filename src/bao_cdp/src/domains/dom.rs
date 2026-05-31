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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servo_bridge::{bridge_channel, BridgeResponse};
    use cdp_server::EventSender;
    use std::time::Duration;
    use std::thread;

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }

    const TIMEOUT: Duration = Duration::from_millis(500);

    fn setup() -> (DomHandler, crate::servo_bridge::BridgeReceiver) {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        (DomHandler::new(sender), receiver)
    }

    fn mock_responder(receiver: crate::servo_bridge::BridgeReceiver) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            for _ in 0..20 {
                let _ = receiver.try_process(|cmd| match cmd {
                    BridgeCommand::GetDocument => BridgeResponse { result: Ok(json!({"root": {"nodeId": 1}})) },
                    BridgeCommand::QuerySelector { .. } => BridgeResponse { result: Ok(json!({"nodeId": 5})) },
                    BridgeCommand::QuerySelectorAll { .. } => BridgeResponse { result: Ok(json!({"nodeIds": [1, 2, 3]})) },
                    BridgeCommand::SetAttributeValue { .. } => BridgeResponse { result: Ok(json!({})) },
                    BridgeCommand::GetOuterHtml { .. } => BridgeResponse { result: Ok(json!({"outerHTML": "<html></html>"})) },
                    _ => BridgeResponse { result: Ok(json!({})) },
                });
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
    }

    #[test]
    fn domain_name_is_dom() {
        let (handler, _rx) = setup();
        assert_eq!(handler.domain_name(), "DOM");
    }

    #[test]
    fn enable_returns_empty() {
        let (handler, _rx) = setup();
        assert_eq!(handler.handle_command("DOM.enable", json!({}), &NoopSender).unwrap(), json!({}));
    }

    #[test]
    fn disable_returns_empty() {
        let (handler, _rx) = setup();
        assert_eq!(handler.handle_command("DOM.disable", json!({}), &NoopSender).unwrap(), json!({}));
    }

    #[test]
    fn describe_node_returns_node_info() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("DOM.describeNode", json!({}), &NoopSender).unwrap();
        assert_eq!(result["node"]["nodeId"], 1);
        assert_eq!(result["node"]["nodeType"], 1);
        assert_eq!(result["node"]["nodeName"], "HTML");
    }

    #[test]
    fn query_selector_empty_returns_zero() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("DOM.querySelector", json!({"selector": ""}), &NoopSender).unwrap();
        assert_eq!(result["nodeId"], 0);
    }

    #[test]
    fn query_selector_no_selector_returns_zero() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("DOM.querySelector", json!({}), &NoopSender).unwrap();
        assert_eq!(result["nodeId"], 0);
    }

    #[test]
    fn query_selector_all_empty_returns_empty_array() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("DOM.querySelectorAll", json!({"selector": ""}), &NoopSender).unwrap();
        assert_eq!(result["nodeIds"], json!([]));
    }

    #[test]
    fn get_box_model_returns_dimensions() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("DOM.getBoxModel", json!({}), &NoopSender).unwrap();
        assert_eq!(result["model"]["width"], 1920);
        assert_eq!(result["model"]["height"], 1080);
    }

    #[test]
    fn remove_attribute_returns_empty() {
        let (handler, _rx) = setup();
        assert_eq!(handler.handle_command("DOM.removeAttribute", json!({}), &NoopSender).unwrap(), json!({}));
    }

    #[test]
    fn set_outer_html_returns_empty() {
        let (handler, _rx) = setup();
        assert_eq!(handler.handle_command("DOM.setOuterHTML", json!({}), &NoopSender).unwrap(), json!({}));
    }

    #[test]
    fn insert_before_returns_empty() {
        let (handler, _rx) = setup();
        assert_eq!(handler.handle_command("DOM.insertBefore", json!({}), &NoopSender).unwrap(), json!({}));
    }

    #[test]
    fn remove_node_returns_empty() {
        let (handler, _rx) = setup();
        assert_eq!(handler.handle_command("DOM.removeNode", json!({}), &NoopSender).unwrap(), json!({}));
    }

    #[test]
    fn resolve_node_returns_object() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("DOM.resolveNode", json!({}), &NoopSender).unwrap();
        assert_eq!(result["object"]["type"], "node");
    }

    #[test]
    fn push_nodes_by_backend_ids_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("DOM.pushNodesByBackendIdsToFrontend", json!({}), &NoopSender).unwrap();
        assert_eq!(result["nodeIds"], json!([]));
    }

    #[test]
    fn unknown_command_returns_error() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("DOM.nonExistent", json!({}), &NoopSender);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, -32601);
    }

    #[test]
    fn get_document_with_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("DOM.getDocument", json!({}), &NoopSender).unwrap();
        assert_eq!(result["root"]["nodeId"], 1);
        responder.join().unwrap();
    }

    #[test]
    fn query_selector_nonempty_uses_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("DOM.querySelector", json!({"selector": "div"}), &NoopSender).unwrap();
        assert_eq!(result["nodeId"], 5);
        responder.join().unwrap();
    }

    #[test]
    fn query_selector_all_nonempty_uses_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("DOM.querySelectorAll", json!({"selector": "div"}), &NoopSender).unwrap();
        assert_eq!(result["nodeIds"], json!([1, 2, 3]));
        responder.join().unwrap();
    }

    #[test]
    fn set_attribute_value_uses_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("DOM.setAttributeValue", json!({"nodeId": 1, "name": "class", "value": "active"}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }

    #[test]
    fn get_outer_html_uses_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("DOM.getOuterHTML", json!({"nodeId": 1}), &NoopSender).unwrap();
        assert_eq!(result["outerHTML"], "<html></html>");
        responder.join().unwrap();
    }
}
