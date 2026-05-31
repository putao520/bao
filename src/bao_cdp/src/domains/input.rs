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

    fn setup() -> (InputHandler, crate::servo_bridge::BridgeReceiver) {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        (InputHandler::new(sender), receiver)
    }

    fn mock_responder(receiver: crate::servo_bridge::BridgeReceiver) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            for _ in 0..20 {
                let _ = receiver.try_process(|_| BridgeResponse { result: Ok(json!({})) });
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
    }

    #[test]
    fn domain_name_is_input() {
        let (handler, _rx) = setup();
        assert_eq!(handler.domain_name(), "Input");
    }

    #[test]
    fn dispatch_touch_event_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Input.dispatchTouchEvent", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_ignore_input_events_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Input.setIgnoreInputEvents", json!({"ignore": true}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_intercept_drags_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Input.setInterceptDrags", json!({"enabled": true}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn insert_text_empty_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Input.insertText", json!({"text": ""}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn insert_text_no_text_param_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Input.insertText", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn unknown_command_returns_error() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Input.nonExistent", json!({}), &NoopSender);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, -32601);
    }

    #[test]
    fn dispatch_mouse_event_with_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Input.dispatchMouseEvent", json!({"type": "mousePressed", "x": 100, "y": 200}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }

    #[test]
    fn dispatch_mouse_event_defaults() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Input.dispatchMouseEvent", json!({}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }

    #[test]
    fn dispatch_key_event_with_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Input.dispatchKeyEvent", json!({"type": "keyDown", "key": "Enter", "code": "Enter"}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }

    #[test]
    fn dispatch_key_event_with_text() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Input.dispatchKeyEvent", json!({"type": "keyDown", "key": "a", "code": "KeyA", "text": "a"}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }

    #[test]
    fn insert_text_nonempty_uses_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Input.insertText", json!({"text": "hello"}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }
}
