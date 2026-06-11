// @trace REQ-CDP-007
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

pub struct EmulationHandler {
    bridge: BridgeSender,
    target_id: String,
    touch_enabled: AtomicBool,
    script_execution_disabled: AtomicBool,
    cpu_throttling_rate: AtomicU64,
}

impl EmulationHandler {
    pub fn new(bridge: BridgeSender, target_id: String) -> Self {
        EmulationHandler {
            bridge,
            target_id,
            touch_enabled: AtomicBool::new(false),
            script_execution_disabled: AtomicBool::new(false),
            cpu_throttling_rate: AtomicU64::new(1.0f64.to_bits()),
        }
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
                let resp = self.bridge.send(BridgeCommand::SetViewport { target_id: self.target_id.clone(), width, height, device_scale_factor: dsf });
                resp.result.map_err(|e| CdpError { code: -32603, message: e })
            }
            "Emulation.clearDeviceMetricsOverride" => Ok(json!({})),
            "Emulation.setUserAgentOverride" => {
                let ua = ps(&params, "userAgent");
                if !ua.is_empty() {
                    let resp = self.bridge.send(BridgeCommand::SetUserAgent { target_id: self.target_id.clone(), user_agent: ua });
                    resp.result.map_err(|e| CdpError { code: -32603, message: e })
                } else {
                    Ok(json!({}))
                }
            }
            "Emulation.setTouchEmulationEnabled" => {
                self.touch_enabled.store(params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false), Ordering::Relaxed);
                Ok(json!({}))
            }
            "Emulation.setScriptExecutionDisabled" => {
                self.script_execution_disabled.store(params.get("value").and_then(|v| v.as_bool()).unwrap_or(false), Ordering::Relaxed);
                Ok(json!({}))
            }
            "Emulation.setFocusEmulationEnabled" => Ok(json!({})),
            "Emulation.setCPUThrottlingRate" => {
                self.cpu_throttling_rate.store(params.get("rate").and_then(|v| v.as_f64()).unwrap_or(1.0).to_bits(), Ordering::Relaxed);
                Ok(json!({}))
            }
            "Emulation.setDefaultBackgroundColorOverride" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TID: &str = "test-target";
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
        (EmulationHandler::new(sender, TID.into()), receiver)
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
    fn set_touch_emulation_stores_flag() {
        let (handler, _rx) = setup();
        assert!(!handler.touch_enabled.load(Ordering::Relaxed));
        handler.handle_command("Emulation.setTouchEmulationEnabled", json!({"enabled": true}), &NoopSender).unwrap();
        assert!(handler.touch_enabled.load(Ordering::Relaxed));
        handler.handle_command("Emulation.setTouchEmulationEnabled", json!({"enabled": false}), &NoopSender).unwrap();
        assert!(!handler.touch_enabled.load(Ordering::Relaxed));
    }

    #[test]
    fn set_script_execution_disabled_stores_flag() {
        let (handler, _rx) = setup();
        assert!(!handler.script_execution_disabled.load(Ordering::Relaxed));
        handler.handle_command("Emulation.setScriptExecutionDisabled", json!({"value": true}), &NoopSender).unwrap();
        assert!(handler.script_execution_disabled.load(Ordering::Relaxed));
        handler.handle_command("Emulation.setScriptExecutionDisabled", json!({"value": false}), &NoopSender).unwrap();
        assert!(!handler.script_execution_disabled.load(Ordering::Relaxed));
    }

    #[test]
    fn set_cpu_throttling_rate_stores_rate() {
        let (handler, _rx) = setup();
        assert!((f64::from_bits(handler.cpu_throttling_rate.load(Ordering::Relaxed)) - 1.0).abs() < f64::EPSILON);
        handler.handle_command("Emulation.setCPUThrottlingRate", json!({"rate": 4.0}), &NoopSender).unwrap();
        assert!((f64::from_bits(handler.cpu_throttling_rate.load(Ordering::Relaxed)) - 4.0).abs() < f64::EPSILON);
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
