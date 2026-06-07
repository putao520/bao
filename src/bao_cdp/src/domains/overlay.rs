// @trace REQ-CDP-007
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

/// Overlay domain handler — screenshot-based node highlighting.
pub struct OverlayHandler {
    bridge: BridgeSender,
    enabled: std::sync::atomic::AtomicBool,
}

impl OverlayHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        OverlayHandler {
            bridge,
            enabled: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl DomainHandler for OverlayHandler {
    fn domain_name(&self) -> &'static str { "Overlay" }

    fn handle_command(&self, command: &str, params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Overlay.enable" => {
                self.enabled.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(json!({}))
            }
            "Overlay.disable" => {
                self.enabled.store(false, std::sync::atomic::Ordering::SeqCst);
                Ok(json!({}))
            }
            "Overlay.highlightNode" => {
                let node_id = params.get("nodeId")
                    .or_else(|| params.get("highlightConfig").and_then(|c| c.get("nodeId")))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let js = format!(
                    "(function() {{ \
                        var el = document.querySelector('[data-cdp-node-id=\"{node_id}\"]') || \
                                document.querySelectorAll('*')[{node_id}]; \
                        if (!el) return JSON.stringify({{highlighted: false}}); \
                        var rect = el.getBoundingClientRect(); \
                        var highlight = document.createElement('div'); \
                        highlight.id = '__bao_cdp_highlight'; \
                        highlight.style.cssText = 'position:fixed;pointer-events:none;z-index:999999;' \
                            + 'border:2px solid #1a73e8;background:rgba(26,115,232,0.15);' \
                            + 'left:' + rect.left + 'px;top:' + rect.top + 'px;' \
                            + 'width:' + rect.width + 'px;height:' + rect.height + 'px;'; \
                        var old = document.getElementById('__bao_cdp_highlight'); \
                        if (old) old.remove(); \
                        document.body.appendChild(highlight); \
                        return JSON.stringify({{highlighted: true, rect: {{ \
                            x: rect.left, y: rect.top, w: rect.width, h: rect.height \
                        }}}}); \
                    }})()"
                );
                let response = self.bridge.send(BridgeCommand::EvaluateJs {
                    expression: js,
                    return_by_value: true,
                });
                match response.result {
                    Ok(v) => {
                        let parsed: Value = serde_json::from_str(v.as_str().unwrap_or("{}"))
                            .unwrap_or_else(|_| json!({}));
                        Ok(parsed)
                    }
                    Err(e) => Ok(json!({"highlighted": false, "error": e})),
                }
            }
            "Overlay.hideHighlight" => {
                let js = "(function() { \
                    var el = document.getElementById('__bao_cdp_highlight'); \
                    if (el) el.remove(); \
                    return '{}'; \
                })()";
                let _ = self.bridge.send(BridgeCommand::EvaluateJs {
                    expression: js.to_string(),
                    return_by_value: true,
                });
                Ok(json!({}))
            }
            "Overlay.setInspectMode" => {
                let mode = params.get("mode").and_then(|v| v.as_str()).unwrap_or("none");
                Ok(json!({"mode": mode}))
            }
            "Overlay.setPausedInDebuggerMessage" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servo_bridge::bridge_channel;
    use std::time::Duration;

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }
    static NOOP: NoopSender = NoopSender;

    #[test]
    fn overlay_domain_name() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = OverlayHandler::new(bridge);
        assert_eq!(h.domain_name(), "Overlay");
    }

    #[test]
    fn overlay_enable_disable() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = OverlayHandler::new(bridge);
        assert_eq!(h.handle_command("Overlay.enable", json!({}), &NOOP).unwrap(), json!({}));
        assert_eq!(h.handle_command("Overlay.disable", json!({}), &NOOP).unwrap(), json!({}));
        assert!(!h.enabled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn overlay_highlight_node_returns_structure() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = OverlayHandler::new(bridge);
        let res = h.handle_command("Overlay.highlightNode", json!({"nodeId": 1}), &NOOP).unwrap();
        assert!(res.get("highlighted").is_some(), "should have highlighted field");
    }

    #[test]
    fn overlay_hide_highlight_returns_empty() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = OverlayHandler::new(bridge);
        let res = h.handle_command("Overlay.hideHighlight", json!({}), &NOOP).unwrap();
        assert_eq!(res, json!({}));
    }

    #[test]
    fn overlay_set_inspect_mode_returns_mode() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = OverlayHandler::new(bridge);
        let res = h.handle_command("Overlay.setInspectMode", json!({"mode": "searchForNode"}), &NOOP).unwrap();
        assert_eq!(res["mode"], "searchForNode");
    }

    #[test]
    fn overlay_unknown_returns_error() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = OverlayHandler::new(bridge);
        let err = h.handle_command("Overlay.nonexistent", json!({}), &NOOP).unwrap_err();
        assert_eq!(err.code, -32601);
    }
}
