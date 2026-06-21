// @trace REQ-CDP-008 [entity:ServoTargetProvider]
//
// TASK-6 (DEC-CDP-001): 11 个 evaluate_js 注入式 domain handler 已废弃并删除,
// CDP 命令分发统一由 bao_cdp_client::CDPRdpBridge 接管。本模块仅保留
// `ServoTargetProvider`,因为它是 CdpServer 启动 Playwright 兼容服务时
// 必需的 TargetProvider(通过 `set_target_provider` 注入),独立于 domain
// handler 注册,且属于 CDP server 基础设施。

use cdp_server::TargetInfo;

use crate::servo_bridge::{BridgeCommand, BridgeSender};

/// TargetProvider backed by servo via the bridge channel.
///
/// `CdpServer::set_target_provider` 调用 `list_targets/create_target/...`
/// 时,本 provider 把它们翻译为 `BridgeCommand::ListTargets /
/// CreateTarget / ClosePage` 等命令发给 servo 端的 BridgeReceiver。
///
/// @trace REQ-CDP-008 [entity:ServoTargetProvider]
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

impl cdp_server::TargetProvider for ServoTargetProvider {
    fn list_targets(&self) -> Vec<TargetInfo> {
        // ListTargets bridge command enumerates all active pages
        if let Some(targets) = self.bridge.send(BridgeCommand::ListTargets).result.ok() {
            if let Some(arr) = targets.as_array() {
                return arr.iter().filter_map(|entry| {
                    let id = entry.get("id")?.as_str()?.to_string();
                    let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("Bao").to_string();
                    // BCE-20260621-EMPTY-STR: empty url "" falls back to "about:blank"
                    // (CDP TargetInfo semantics: empty/missing url = fresh page = about:blank).
                    let url = entry.get("url").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("about:blank").to_string();
                    let ws_url = format!("ws://{}:{}/devtools/page/{}", self.host, self.port, id);
                    Some(TargetInfo {
                        id,
                        target_type: "page".into(),
                        title,
                        url,
                        web_socket_debugger_url: ws_url,
                    })
                }).collect();
            }
        }
        // Fallback: return the default target
        let title = self.bridge.send(BridgeCommand::GetTitle { target_id: self.target_id.clone() }).result
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "Bao".into());
        let url = self.bridge.send(BridgeCommand::GetUrl { target_id: self.target_id.clone() }).result
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "about:blank".into());
        vec![TargetInfo {
            id: self.target_id.clone(),
            target_type: "page".into(),
            title,
            url,
            web_socket_debugger_url: format!("ws://{}:{}/devtools/page/{}", self.host, self.port, self.target_id),
        }]
    }

    fn create_target(&self, url: &str) -> Result<TargetInfo, String> {
        let result = self.bridge.send(BridgeCommand::CreateTarget { url: url.to_string() }).result;
        match result {
            Ok(val) => {
                let new_id = val.get("targetId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "CreateTarget returned no targetId".to_string())?;
                if new_id == self.target_id {
                    return Err("CreateTarget returned existing targetId, not a new page".to_string());
                }
                Ok(TargetInfo {
                    id: new_id.to_string(),
                    target_type: "page".into(),
                    title: String::new(),
                    url: url.to_string(),
                    web_socket_debugger_url: format!("ws://{}:{}/devtools/page/{}", self.host, self.port, new_id),
                })
            }
            Err(e) => Err(e),
        }
    }

    fn close_target(&self, target_id: &str) -> Result<(), String> {
        self.bridge.send_fire_and_forget(BridgeCommand::ClosePage { target_id: target_id.to_string() });
        Ok(())
    }

    fn activate_target(&self, _target_id: &str) -> Result<(), String> {
        Ok(())
    }
}

// @trace TEST-CDP-008 [req:REQ-CDP-008] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::servo_bridge::{bridge_channel, BridgeCommand, BridgeResponse};
    use cdp_server::TargetProvider;
    use serde_json::json;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_millis(100);

    #[test]
    fn provider_new_constructs() {
        let (bridge, _rx) = bridge_channel(TIMEOUT);
        let p = ServoTargetProvider::new(bridge, "tid".into(), "127.0.0.1".into(), 9222);
        // Constructor should not panic
        assert_eq!(p.target_id, "tid");
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 9222);
    }

    #[test]
    fn list_targets_fallback_when_no_responder() {
        // No responder thread → bridge returns Err → fallback to single default target
        let (bridge, _rx) = bridge_channel(TIMEOUT);
        let provider = ServoTargetProvider::new(bridge, "default-target".into(), "127.0.0.1".into(), 9222);
        let targets = provider.list_targets();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "default-target");
        assert_eq!(targets[0].target_type, "page");
        assert!(targets[0].web_socket_debugger_url.contains("/devtools/page/default-target"));
    }

    #[test]
    fn list_targets_uses_list_targets_bridge_command_when_available() {
        let (bridge, rx) = bridge_channel(Duration::from_secs(2));
        let keeper = bridge.clone();
        std::thread::spawn(move || {
            let _keeper = keeper;
            loop {
                let handled = rx.try_process(|cmd| match cmd {
                    BridgeCommand::ListTargets => BridgeResponse {
                        result: Ok(json!([
                            { "id": "p1", "title": "Page 1", "url": "https://example.com/1" },
                            { "id": "p2", "title": "Page 2", "url": "https://example.com/2" },
                        ])),
                    },
                    _ => BridgeResponse { result: Ok(json!({})) },
                });
                if !handled {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });
        let provider = ServoTargetProvider::new(bridge, "default-target".into(), "127.0.0.1".into(), 9222);
        let targets = provider.list_targets();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].id, "p1");
        assert_eq!(targets[1].id, "p2");
    }

    #[test]
    fn create_target_returns_new_target_info() {
        let (bridge, rx) = bridge_channel(Duration::from_secs(2));
        let keeper = bridge.clone();
        std::thread::spawn(move || {
            let _keeper = keeper;
            loop {
                let handled = rx.try_process(|cmd| match cmd {
                    BridgeCommand::CreateTarget { .. } => BridgeResponse {
                        result: Ok(json!({ "targetId": "new-page-1" })),
                    },
                    _ => BridgeResponse { result: Ok(json!({})) },
                });
                if !handled {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });
        let provider = ServoTargetProvider::new(bridge, "default-target".into(), "127.0.0.1".into(), 9222);
        let info = provider.create_target("https://example.com").unwrap();
        assert_eq!(info.id, "new-page-1");
        assert_eq!(info.url, "https://example.com");
    }

    #[test]
    fn create_target_rejects_existing_target_id() {
        let (bridge, rx) = bridge_channel(Duration::from_secs(2));
        let keeper = bridge.clone();
        let parent_target = "default-target".to_string();
        std::thread::spawn(move || {
            let _keeper = keeper;
            loop {
                let handled = rx.try_process(|cmd| match cmd {
                    BridgeCommand::CreateTarget { .. } => BridgeResponse {
                        // Returns the SAME target_id → must be rejected
                        result: Ok(json!({ "targetId": "default-target" })),
                    },
                    _ => BridgeResponse { result: Ok(json!({})) },
                });
                if !handled {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });
        let provider = ServoTargetProvider::new(bridge, parent_target, "127.0.0.1".into(), 9222);
        let result = provider.create_target("https://example.com");
        assert!(result.is_err(), "should reject fallback to existing target_id");
    }

    #[test]
    fn close_target_sends_close_page_command() {
        // close_target returns Ok(()) and sends ClosePage fire-and-forget
        let (bridge, _rx) = bridge_channel(TIMEOUT);
        let provider = ServoTargetProvider::new(bridge, "default-target".into(), "127.0.0.1".into(), 9222);
        let result = provider.close_target("target-1");
        assert!(result.is_ok());
    }

    #[test]
    fn activate_target_is_noop_ok() {
        let (bridge, _rx) = bridge_channel(TIMEOUT);
        let provider = ServoTargetProvider::new(bridge, "default-target".into(), "127.0.0.1".into(), 9222);
        let result = provider.activate_target("any-target");
        assert!(result.is_ok());
    }
}
