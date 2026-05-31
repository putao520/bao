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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servo_bridge::{bridge_channel, BridgeResponse};
    use serde_json::json;
    use std::time::Duration;
    use std::thread;

    const TIMEOUT: Duration = Duration::from_millis(500);

    fn setup() -> (ServoTargetProvider, crate::servo_bridge::BridgeReceiver) {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        (ServoTargetProvider::new(sender, "127.0.0.1".into(), 9222), receiver)
    }

    fn mock_responder(receiver: crate::servo_bridge::BridgeReceiver) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            for _ in 0..20 {
                let _ = receiver.try_process(|cmd| match cmd {
                    BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Test Page")) },
                    BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!("https://example.com")) },
                    _ => BridgeResponse { result: Ok(json!({})) },
                });
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
    }

    #[test]
    fn activate_target_returns_ok() {
        let (provider, _rx) = setup();
        assert!(provider.activate_target("any-id").is_ok());
    }

    #[test]
    fn close_target_sends_fire_and_forget() {
        let (provider, rx) = setup();
        let responder = mock_responder(rx);
        assert!(provider.close_target("any-id").is_ok());
        responder.join().unwrap();
    }

    #[test]
    fn list_targets_returns_one_target() {
        let (provider, rx) = setup();
        let responder = mock_responder(rx);
        let targets = provider.list_targets();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target_type, "page");
        assert_eq!(targets[0].title, "Test Page");
        assert_eq!(targets[0].url, "https://example.com");
        assert!(targets[0].web_socket_debugger_url.contains("127.0.0.1:9222"));
        responder.join().unwrap();
    }

    #[test]
    fn create_target_returns_existing() {
        let (provider, rx) = setup();
        let responder = mock_responder(rx);
        let result = provider.create_target("https://new.com");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().target_type, "page");
        responder.join().unwrap();
    }

    #[test]
    fn target_id_is_nonempty() {
        let (provider, rx) = setup();
        let responder = mock_responder(rx);
        let targets = provider.list_targets();
        assert!(!targets[0].id.is_empty());
        responder.join().unwrap();
    }

    #[test]
    fn target_ws_url_format() {
        let (provider, rx) = setup();
        let responder = mock_responder(rx);
        let targets = provider.list_targets();
        assert!(targets[0].web_socket_debugger_url.starts_with("ws://127.0.0.1:9222/devtools/page/"));
        responder.join().unwrap();
    }
}
