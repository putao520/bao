// @trace REQ-CDP-008

use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender, TargetProvider, TargetInfo};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

pub struct ServoTargetProvider {
    bridge: BridgeSender,
    target_id: String,
    port: u16,
    host: String,
}

impl ServoTargetProvider {
    pub fn new(bridge: BridgeSender, target_id: String, host: String, port: u16) -> Self {
        ServoTargetProvider { bridge, target_id, host, port }
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
        vec![TargetInfo {
            id: self.target_id.clone(),
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

// ---------------------------------------------------------------------------
// TargetHandler — DomainHandler for the Target CDP domain
// ---------------------------------------------------------------------------

pub struct TargetHandler {
    bridge: BridgeSender,
    target_id: String,
}

impl TargetHandler {
    pub fn new(bridge: BridgeSender, target_id: String) -> Self {
        TargetHandler { bridge, target_id }
    }

    fn live_target_info(&self) -> Value {
        let title = self.bridge.send(BridgeCommand::GetTitle).result
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "Bao".into());
        let url = self.bridge.send(BridgeCommand::GetUrl).result
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "about:blank".into());
        json!({
            "targetId": self.target_id,
            "type": "page",
            "title": title,
            "url": url,
            "attached": true
        })
    }
}

impl DomainHandler for TargetHandler {
    fn domain_name(&self) -> &'static str { "Target" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Target.getTargets" | "Target.getTargetTargets" => {
                Ok(json!({ "targetInfos": [self.live_target_info()] }))
            }
            "Target.createTarget" => Ok(json!({ "targetId": self.target_id })),
            "Target.closeTarget" => {
                self.bridge.send_fire_and_forget(BridgeCommand::ClosePage);
                Ok(json!({ "success": true }))
            }
            "Target.setDiscoverTargets" => {
                let discover = params.get("discover").and_then(|v| v.as_bool()).unwrap_or(true);
                if discover {
                    event_sender.send_event("Target.targetCreated", json!({
                        "targetInfo": {
                            "targetId": self.target_id,
                            "type": "page",
                            "title": "",
                            "url": "about:blank",
                            "attached": false
                        }
                    }));
                }
                Ok(json!({}))
            }
            "Target.setAutoAttach" => {
                let auto_attach = params.get("autoAttach").and_then(|v| v.as_bool()).unwrap_or(false);
                if auto_attach {
                    event_sender.send_event("Target.attachedToTarget", json!({
                        "sessionId": format!("{:016x}", self.target_id.chars().map(|c| c as u64).sum::<u64>()),
                        "targetInfo": {
                            "targetId": self.target_id,
                            "type": "page",
                            "title": "",
                            "url": "about:blank",
                            "attached": true
                        },
                        "waitingForDebuggerOnStart": false
                    }));
                }
                Ok(json!({}))
            }
            "Target.getTargetInfo" => {
                Ok(json!({ "targetInfo": self.live_target_info() }))
            }
            "Target.attachToTarget" => {
                let session_id = format!("{:016x}",
                    self.target_id.chars().map(|c| c as u64).sum::<u64>()
                );
                Ok(json!({ "sessionId": session_id }))
            }
            "Target.detachFromTarget" | "Target.sendMessageToTarget" => Ok(json!({})),
            _ => Err(CdpError {
                code: -32601,
                message: format!("'{}' wasn't found", command),
            }),
        }
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
        (ServoTargetProvider::new(sender, "test-target-id".into(), "127.0.0.1".into(), 9222), receiver)
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

    #[test]
    fn target_id_matches_fixed_value() {
        let (provider, rx) = setup();
        let responder = mock_responder(rx);
        let targets = provider.list_targets();
        assert_eq!(targets[0].id, "test-target-id");
        responder.join().unwrap();
    }

    #[test]
    fn target_id_consistent_across_calls() {
        let (provider, rx) = setup();
        let responder = mock_responder(rx);
        let id1 = provider.list_targets()[0].id.clone();
        let id2 = provider.list_targets()[0].id.clone();
        assert_eq!(id1, id2);
        responder.join().unwrap();
    }

    #[test]
    fn target_handler_and_provider_share_id() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        let target_id = "shared-abc123".to_string();
        let provider = ServoTargetProvider::new(sender.clone(), target_id.clone(), "127.0.0.1".into(), 9222);
        let handler = TargetHandler::new(sender, target_id.clone());

        let responder = mock_responder(receiver);
        let provider_id = provider.list_targets()[0].id.clone();
        let handler_info = handler.live_target_info();
        let handler_id = handler_info["targetId"].as_str().unwrap().to_string();
        assert_eq!(provider_id, target_id);
        assert_eq!(handler_id, target_id);
        assert_eq!(provider_id, handler_id);
        responder.join().unwrap();
    }
}
