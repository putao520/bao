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
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Page.enable" | "Page.disable" => Ok(json!({})),
            "Page.navigate" => {
                let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("about:blank");
                bridge_send(&self.bridge, BridgeCommand::Navigate { url: url.to_string() })?;
                let loader_id = format!("{:016x}", url.len() as u64);
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
