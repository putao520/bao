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
