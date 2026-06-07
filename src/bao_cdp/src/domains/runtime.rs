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
            "Runtime.callFunctionOn" => {
                let function_declaration = params.get("functionDeclaration").and_then(|v| v.as_str()).unwrap_or("");
                let object_id = params.get("objectId").and_then(|v| v.as_str());
                let arguments = params.get("arguments").and_then(|v| v.as_array());

                if !function_declaration.is_empty() {
                    // Build the function call expression
                    let args_str = if let Some(args) = arguments {
                        args.iter().filter_map(|a| {
                            a.get("value").map(|v| serde_json::to_string(v).unwrap_or_else(|_| "undefined".into()))
                        }).collect::<Vec<_>>().join(",")
                    } else {
                        String::new()
                    };
                    let expression = format!("({})({})", function_declaration, args_str);
                    let resp = self.bridge.send(BridgeCommand::EvaluateJs {
                        expression,
                        return_by_value: true,
                    });
                    resp.result.map(|v| json!({
                        "result": { "type": "string", "value": v.as_str().unwrap_or(&v.to_string()) }
                    })).map_err(|e| CdpError { code: -32603, message: e })
                } else if let Some(oid) = object_id {
                    // Just evaluate the object ID as an expression
                    let resp = self.bridge.send(BridgeCommand::EvaluateJs {
                        expression: oid.to_string(),
                        return_by_value: true,
                    });
                    resp.result.map(|v| json!({
                        "result": { "type": "object", "value": v }
                    })).map_err(|e| CdpError { code: -32603, message: e })
                } else {
                    Ok(json!({ "result": { "type": "undefined" } }))
                }
            }
            "Runtime.getProperties" => {
                let object_id = params.get("objectId").and_then(|v| v.as_str()).unwrap_or("");
                let own_properties = params.get("ownProperties").and_then(|v| v.as_bool()).unwrap_or(true);
                // Query properties via JS reflection
                let method = if own_properties { "getOwnPropertyNames" } else { "keys" };
                let js = format!(
                    "(function() {{ try {{ var obj = {}; var keys = Object.{}(obj); return JSON.stringify(keys.map(function(k) {{ return {{name: k, configurable: true, enumerable: true}}; }})); }} catch(e) {{ return '[]'; }} }})()",
                    if object_id.is_empty() { "globalThis" } else { object_id },
                    method,
                );
                let resp = self.bridge.send(BridgeCommand::EvaluateJs {
                    expression: js,
                    return_by_value: true,
                });
                let props_str = resp.result.ok().and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_else(|| "[]".into());
                let properties: Vec<Value> = serde_json::from_str(&props_str).unwrap_or_default();
                Ok(json!({ "result": properties }))
            }
            "Runtime.evaluateAsync" | "Runtime.runScript" => Ok(json!({ "result": { "type": "undefined" } })),
            "Runtime.releaseObject" | "Runtime.releaseObjectGroup" | "Runtime.compileScript" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servo_bridge::{bridge_channel, BridgeResponse, BridgeCommand};
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
    fn call_function_on_with_declaration_sends_bridge() {
        let (bridge, rx) = bridge_channel(TIMEOUT);
        let handler = RuntimeHandler::new(bridge);
        let responder = std::thread::spawn(move || {
            rx.recv_and_process(TIMEOUT, |cmd| {
                if let BridgeCommand::EvaluateJs { expression, .. } = cmd {
                    assert!(expression.contains("function() { return 42; }"));
                }
                BridgeResponse { result: Ok(json!("42")) }
            });
        });
        let result = handler.handle_command(
            "Runtime.callFunctionOn",
            json!({"functionDeclaration": "function() { return 42; }"}),
            &NoopSender,
        ).unwrap();
        let _ = responder.join();
        assert_eq!(result["result"]["type"], "string");
    }

    #[test]
    fn call_function_on_empty_returns_undefined() {
        let handler = setup();
        let result = handler.handle_command("Runtime.callFunctionOn", json!({}), &NoopSender).unwrap();
        assert_eq!(result["result"]["type"], "undefined");
    }

    #[test]
    fn call_function_on_with_arguments_sends_bridge() {
        let (bridge, rx) = bridge_channel(TIMEOUT);
        let handler = RuntimeHandler::new(bridge);
        let responder = std::thread::spawn(move || {
            rx.recv_and_process(TIMEOUT, |cmd| {
                if let BridgeCommand::EvaluateJs { expression, .. } = cmd {
                    assert!(expression.contains("1,2"));
                }
                BridgeResponse { result: Ok(json!("3")) }
            });
        });
        handler.handle_command(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "function(a,b) { return a+b; }",
                "arguments": [{"value": 1}, {"value": 2}]
            }),
            &NoopSender,
        ).unwrap();
        let _ = responder.join();
    }

    #[test]
    fn get_properties_sends_bridge_reflection_query() {
        let (bridge, rx) = bridge_channel(TIMEOUT);
        let handler = RuntimeHandler::new(bridge);
        let responder = std::thread::spawn(move || {
            rx.recv_and_process(TIMEOUT, |cmd| {
                if let BridgeCommand::EvaluateJs { expression, .. } = cmd {
                    assert!(expression.contains("getOwnPropertyNames") || expression.contains("keys"));
                }
                BridgeResponse { result: Ok(json!("[]")) }
            });
        });
        let result = handler.handle_command(
            "Runtime.getProperties",
            json!({"objectId": "testObj"}),
            &NoopSender,
        ).unwrap();
        let _ = responder.join();
        assert!(result.get("result").is_some());
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
