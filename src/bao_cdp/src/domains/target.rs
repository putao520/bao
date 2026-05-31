// @trace REQ-CDP-008

use cdp_server::{TargetProvider, TargetInfo};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

pub struct ServoTargetProvider {
    bridge: BridgeSender,
    port: u16,
    host: String,
}

impl ServoTargetProvider {
    pub fn new(bridge: BridgeSender, host: String, port: u16) -> Self {
        ServoTargetProvider { bridge, host, port }
    }
}

impl TargetProvider for ServoTargetProvider {
    fn list_targets(&self) -> Vec<TargetInfo> {
        let title = self.bridge.send(BridgeCommand::GetTitle).result
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "Bao".into());
        let url = self.bridge.send(BridgeCommand::GetUrl).result
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "about:blank".into());
        let id = format!("{:016x}", title.len() as u64 | (url.len() as u64) << 16);
        vec![TargetInfo {
            id,
            target_type: "page".into(),
            title,
            url,
            web_socket_debugger_url: format!("ws://{}:{}/devtools/page/{}", self.host, self.port, "default"),
        }]
    }

    fn create_target(&self, _url: &str) -> Result<TargetInfo, String> {
        // Single-target mode: return the existing target
        let targets = self.list_targets();
        targets.into_iter().next().ok_or_else(|| "no targets available".into())
    }

    fn close_target(&self, _target_id: &str) -> Result<(), String> {
        self.bridge.send_fire_and_forget(BridgeCommand::ClosePage);
        Ok(())
    }

    fn activate_target(&self, _target_id: &str) -> Result<(), String> {
        Ok(())
    }
}
