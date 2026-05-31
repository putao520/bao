// @trace REQ-CDP-007
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

pub struct EmulationHandler {
    bridge: BridgeSender,
}

impl EmulationHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        EmulationHandler { bridge }
    }
}

fn ps(params: &Value, key: &str) -> String {
    params.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

impl DomainHandler for EmulationHandler {
    fn domain_name(&self) -> &'static str { "Emulation" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Emulation.setDeviceMetricsOverride" => {
                let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(1920) as u32;
                let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(1080) as u32;
                let dsf = params.get("deviceScaleFactor").and_then(|v| v.as_f64());
                let resp = self.bridge.send(BridgeCommand::SetViewport { width, height, device_scale_factor: dsf });
                resp.result.map_err(|e| CdpError { code: -32603, message: e })
            }
            "Emulation.clearDeviceMetricsOverride" => Ok(json!({})),
            "Emulation.setUserAgentOverride" => {
                let ua = ps(&params, "userAgent");
                if !ua.is_empty() {
                    let resp = self.bridge.send(BridgeCommand::SetUserAgent { user_agent: ua });
                    resp.result.map_err(|e| CdpError { code: -32603, message: e })
                } else {
                    Ok(json!({}))
                }
            }
            "Emulation.setTouchEmulationEnabled" | "Emulation.setScriptExecutionDisabled" => Ok(json!({})),
            "Emulation.setFocusEmulationEnabled" | "Emulation.setCPUThrottlingRate" => Ok(json!({})),
            "Emulation.setDefaultBackgroundColorOverride" => Ok(json!({})),
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

    fn setup() -> (EmulationHandler, crate::servo_bridge::BridgeReceiver) {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        (EmulationHandler::new(sender), receiver)
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
    fn domain_name_is_emulation() {
        let (handler, _rx) = setup();
        assert_eq!(handler.domain_name(), "Emulation");
    }

    #[test]
    fn clear_device_metrics_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.clearDeviceMetricsOverride", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_touch_emulation_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.setTouchEmulationEnabled", json!({"enabled": true}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_script_execution_disabled_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.setScriptExecutionDisabled", json!({"value": true}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_focus_emulation_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.setFocusEmulationEnabled", json!({"enabled": true}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_cpu_throttling_rate_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.setCPUThrottlingRate", json!({"rate": 4.0}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_default_background_color_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.setDefaultBackgroundColorOverride", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_user_agent_override_empty_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.setUserAgentOverride", json!({"userAgent": ""}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_user_agent_override_no_ua_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.setUserAgentOverride", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn unknown_command_returns_error() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Emulation.nonExistent", json!({}), &NoopSender);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, -32601);
    }

    #[test]
    fn set_device_metrics_override_with_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Emulation.setDeviceMetricsOverride", json!({"width": 1920, "height": 1080, "deviceScaleFactor": 2.0}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }

    #[test]
    fn set_device_metrics_override_defaults() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Emulation.setDeviceMetricsOverride", json!({}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }

    #[test]
    fn set_user_agent_override_nonempty_uses_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Emulation.setUserAgentOverride", json!({"userAgent": "Mozilla/5.0"}), &NoopSender);
        assert!(result.is_ok());
        responder.join().unwrap();
    }
}
