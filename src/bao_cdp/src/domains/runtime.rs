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
        event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Runtime.enable" => {
                event_sender.send_event(
                    "Runtime.executionContextCreated",
                    json!({
                        "context": {
                            "id": 1,
                            "origin": "",
                            "name": ""
                        }
                    }),
                );
                Ok(json!({ "executionContextId": 1 }))
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servo_bridge::bridge_channel;
    use cdp_server::EventSender;
    use std::time::Duration;

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }

    const TIMEOUT: Duration = Duration::from_millis(500);

    fn setup() -> RuntimeHandler {
        let (sender, _rx) = bridge_channel(TIMEOUT);
        RuntimeHandler::new(sender)
    }

    #[test]
    fn domain_name_is_runtime() {
        let handler = setup();
        assert_eq!(handler.domain_name(), "Runtime");
    }

    #[test]
    fn enable_returns_execution_context() {
        let handler = setup();
        let result = handler.handle_command("Runtime.enable", json!({}), &NoopSender).unwrap();
        assert_eq!(result["executionContextId"], 1);
    }

    #[test]
    fn disable_returns_empty() {
        let handler = setup();
        let result = handler.handle_command("Runtime.disable", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn evaluate_empty_expression_returns_undefined() {
        let handler = setup();
        let result = handler.handle_command("Runtime.evaluate", json!({"expression": ""}), &NoopSender).unwrap();
        assert_eq!(result["result"]["type"], "undefined");
        assert!(result["exceptionDetails"].is_null());
    }

    #[test]
    fn evaluate_no_expression_returns_undefined() {
        let handler = setup();
        let result = handler.handle_command("Runtime.evaluate", json!({}), &NoopSender).unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    #[test]
    fn call_function_on_returns_undefined() {
        let handler = setup();
        let result = handler.handle_command("Runtime.callFunctionOn", json!({}), &NoopSender).unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    #[test]
    fn get_properties_returns_empty_array() {
        let handler = setup();
        let result = handler.handle_command("Runtime.getProperties", json!({}), &NoopSender).unwrap();
        assert_eq!(result["result"], json!([]));
    }

    #[test]
    fn evaluate_async_returns_undefined() {
        let handler = setup();
        let result = handler.handle_command("Runtime.evaluateAsync", json!({}), &NoopSender).unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    #[test]
    fn run_script_returns_undefined() {
        let handler = setup();
        let result = handler.handle_command("Runtime.runScript", json!({}), &NoopSender).unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    #[test]
    fn release_object_returns_empty() {
        let handler = setup();
        let result = handler.handle_command("Runtime.releaseObject", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn release_object_group_returns_empty() {
        let handler = setup();
        let result = handler.handle_command("Runtime.releaseObjectGroup", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn compile_script_returns_empty() {
        let handler = setup();
        let result = handler.handle_command("Runtime.compileScript", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn unknown_command_returns_error() {
        let handler = setup();
        let result = handler.handle_command("Runtime.nonExistent", json!({}), &NoopSender);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, -32601);
    }
}
