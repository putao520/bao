// @trace REQ-CDP-007
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

pub struct InputHandler {
    bridge: BridgeSender,
}

impl InputHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        InputHandler { bridge }
    }
}

fn ps(params: &Value, key: &str) -> String {
    params.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

impl DomainHandler for InputHandler {
    fn domain_name(&self) -> &'static str { "Input" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Input.dispatchMouseEvent" => {
                let event_type = ps(&params, "type");
                let x = params.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = params.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let button = params.get("button").and_then(|v| v.as_i64());
                let click_count = params.get("clickCount").and_then(|v| v.as_i64());
                let resp = self.bridge.send(BridgeCommand::DispatchMouseEvent { event_type, x, y, button, click_count });
                resp.result.map_err(|e| CdpError { code: -32603, message: e })
            }
            "Input.dispatchKeyEvent" => {
                let event_type = ps(&params, "type");
                let key = ps(&params, "key");
                let code = ps(&params, "code");
                let text = params.get("text").and_then(|v| v.as_str()).map(|s| s.to_string());
                let resp = self.bridge.send(BridgeCommand::DispatchKeyEvent { event_type, key, code, text });
                resp.result.map_err(|e| CdpError { code: -32603, message: e })
            }
            "Input.dispatchTouchEvent" => Ok(json!({})),
            "Input.insertText" => {
                let text = ps(&params, "text");
                if !text.is_empty() {
                    let resp = self.bridge.send(BridgeCommand::InsertText { text });
                    resp.result.map_err(|e| CdpError { code: -32603, message: e })
                } else {
                    Ok(json!({}))
                }
            }
            "Input.setIgnoreInputEvents" | "Input.setInterceptDrags" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}
