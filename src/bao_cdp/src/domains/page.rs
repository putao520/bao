// @trace REQ-CDP-004
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

pub struct PageHandler {
    bridge: BridgeSender,
}

impl PageHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        PageHandler { bridge }
    }
}

impl DomainHandler for PageHandler {
    fn domain_name(&self) -> &'static str { "Page" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Page.enable" | "Page.disable" => Ok(json!({})),
            "Page.navigate" => {
                let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("about:blank");
                bridge_send(&self.bridge, BridgeCommand::Navigate { url: url.to_string() })?;
                let loader_id = format!("{:016x}", url.len() as u64);
                event_sender.send_event("Page.frameNavigated", json!({
                    "frame": { "id": "0", "url": url, "loaderId": loader_id, "mimeType": "text/html" }
                }));
                Ok(json!({ "frameId": "0", "loaderId": loader_id }))
            }
            "Page.reload" => {
                let ignore_cache = params.get("ignoreCache").and_then(|v| v.as_bool()).unwrap_or(false);
                bridge_send(&self.bridge, BridgeCommand::Reload { ignore_cache })?;
                Ok(json!({ "frameId": "0", "loaderId": "0" }))
            }
            "Page.getFrameTree" => {
                let url = self.bridge.send(BridgeCommand::GetUrl).result
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "about:blank".into());
                Ok(json!({
                    "frameTree": {
                        "frame": { "id": "0", "url": url, "loaderId": "0", "mimeType": "text/html" }
                    }
                }))
            }
            "Page.getNavigationHistory" => {
                let url = self.bridge.send(BridgeCommand::GetUrl).result
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "about:blank".into());
                Ok(json!({
                    "currentIndex": 0,
                    "entries": [{ "id": 0, "url": url, "title": "" }]
                }))
            }
            "Page.captureScreenshot" => {
                let format = params.get("format").and_then(|v| v.as_str()).unwrap_or("png").to_string();
                let quality = params.get("quality").and_then(|v| v.as_u64()).map(|q| q as u8);
                bridge_send(&self.bridge, BridgeCommand::TakeScreenshot { format, quality })
            }
            "Page.setContent" | "Page.close" | "Page.bringToFront" => Ok(json!({})),
            "Page.getLayoutMetrics" => Ok(json!({
                "contentSize": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
                "cssContentSize": { "x": 0, "y": 0, "width": 1920, "height": 1080 }
            })),
            "Page.addScriptToEvaluateOnNewDocument" => {
                let source = param_str(&params, "source");
                if !source.is_empty() {
                    bridge_send(&self.bridge, BridgeCommand::AddScriptToEvaluateOnNewDocument { source })?;
                }
                Ok(json!({ "identifier": "1" }))
            }
            "Page.removeScriptToEvaluateOnNewDocument" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

fn bridge_send(bridge: &BridgeSender, cmd: BridgeCommand) -> Result<Value, CdpError> {
    let resp = bridge.send(cmd);
    resp.result.map_err(|e| CdpError { code: -32603, message: e })
}

fn param_str(params: &Value, key: &str) -> String {
    params.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
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

    fn setup() -> (PageHandler, crate::servo_bridge::BridgeReceiver) {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        (PageHandler::new(sender), receiver)
    }

    fn mock_responder(receiver: crate::servo_bridge::BridgeReceiver) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            for _ in 0..20 {
                let _ = receiver.try_process(|cmd| match cmd {
                    BridgeCommand::Navigate { .. } => BridgeResponse { result: Ok(json!({"ok": true})) },
                    BridgeCommand::Reload { .. } => BridgeResponse { result: Ok(json!({})) },
                    BridgeCommand::GetUrl => BridgeResponse { result: Ok(json!("https://example.com")) },
                    BridgeCommand::TakeScreenshot { .. } => BridgeResponse { result: Ok(json!({"data": "base64data"})) },
                    BridgeCommand::AddScriptToEvaluateOnNewDocument { .. } => BridgeResponse { result: Ok(json!({})) },
                    _ => BridgeResponse { result: Ok(json!({})) },
                });
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
    }

    #[test]
    fn domain_name_is_page() {
        let (handler, _rx) = setup();
        assert_eq!(handler.domain_name(), "Page");
    }

    #[test]
    fn enable_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.enable", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn disable_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.disable", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn set_content_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.setContent", json!({"html": "<h1>Hi</h1>"}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn close_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.close", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn bring_to_front_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.bringToFront", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn get_layout_metrics_returns_dimensions() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.getLayoutMetrics", json!({}), &NoopSender).unwrap();
        assert_eq!(result["contentSize"]["width"], 1920);
        assert_eq!(result["contentSize"]["height"], 1080);
        assert_eq!(result["cssContentSize"]["width"], 1920);
        assert_eq!(result["cssContentSize"]["height"], 1080);
    }

    #[test]
    fn remove_script_returns_empty() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.removeScriptToEvaluateOnNewDocument", json!({"identifier": "1"}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn add_script_empty_source_returns_identifier() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.addScriptToEvaluateOnNewDocument", json!({"source": ""}), &NoopSender).unwrap();
        assert_eq!(result["identifier"], "1");
    }

    #[test]
    fn unknown_command_returns_error() {
        let (handler, _rx) = setup();
        let result = handler.handle_command("Page.nonExistent", json!({}), &NoopSender);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, -32601);
    }

    #[test]
    fn navigate_returns_frame_id_and_loader_id() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Page.navigate", json!({"url": "https://example.com"}), &NoopSender).unwrap();
        assert_eq!(result["frameId"], "0");
        assert!(result["loaderId"].is_string());
        responder.join().unwrap();
    }

    #[test]
    fn navigate_default_url_is_about_blank() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Page.navigate", json!({}), &NoopSender).unwrap();
        assert_eq!(result["frameId"], "0");
        responder.join().unwrap();
    }

    #[test]
    fn reload_returns_frame_id() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Page.reload", json!({}), &NoopSender).unwrap();
        assert_eq!(result["frameId"], "0");
        assert_eq!(result["loaderId"], "0");
        responder.join().unwrap();
    }

    #[test]
    fn reload_with_ignore_cache() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Page.reload", json!({"ignoreCache": true}), &NoopSender).unwrap();
        assert_eq!(result["frameId"], "0");
        responder.join().unwrap();
    }

    #[test]
    fn get_frame_tree_returns_frame() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Page.getFrameTree", json!({}), &NoopSender).unwrap();
        let frame = &result["frameTree"]["frame"];
        assert_eq!(frame["id"], "0");
        assert!(frame["url"].is_string());
        assert_eq!(frame["mimeType"], "text/html");
        responder.join().unwrap();
    }

    #[test]
    fn get_navigation_history_returns_entries() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Page.getNavigationHistory", json!({}), &NoopSender).unwrap();
        assert_eq!(result["currentIndex"], 0);
        assert!(result["entries"].is_array());
        assert_eq!(result["entries"][0]["id"], 0);
        responder.join().unwrap();
    }

    #[test]
    fn capture_screenshot_with_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Page.captureScreenshot", json!({"format": "png"}), &NoopSender).unwrap();
        assert_eq!(result["data"], "base64data");
        responder.join().unwrap();
    }

    #[test]
    fn add_script_with_source_uses_bridge() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let result = handler.handle_command("Page.addScriptToEvaluateOnNewDocument", json!({"source": "console.log('hi')"}), &NoopSender).unwrap();
        assert_eq!(result["identifier"], "1");
        responder.join().unwrap();
    }

    #[test]
    fn navigate_loader_id_depends_on_url_length() {
        let (handler, rx) = setup();
        let responder = mock_responder(rx);
        let url = "https://a.com";
        let result = handler.handle_command("Page.navigate", json!({"url": url}), &NoopSender).unwrap();
        assert_eq!(result["loaderId"], format!("{:016x}", url.len() as u64));
        responder.join().unwrap();
    }
}
