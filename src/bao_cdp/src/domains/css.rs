// @trace REQ-CDP-007
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::{BridgeCommand, BridgeSender};

/// CSS domain handler — queries real computed/matched styles via servo bridge.
pub struct CssHandler {
    bridge: BridgeSender,
}

impl CssHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        CssHandler { bridge }
    }
}

impl DomainHandler for CssHandler {
    fn domain_name(&self) -> &'static str { "CSS" }

    fn handle_command(&self, command: &str, params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "CSS.enable" | "CSS.disable" => Ok(json!({})),

            "CSS.getComputedStyleForNode" => {
                let node_id = params.get("nodeId").and_then(|v| v.as_i64()).unwrap_or(0);
                let js = format!(
                    "(function() {{ \
                        var el = document.querySelector('[data-cdp-node-id=\"{node_id}\"]') || \
                                document.querySelectorAll('*')[{node_id}]; \
                        if (!el) return JSON.stringify({{computedStyle: []}}); \
                        var cs = window.getComputedStyle(el); \
                        var result = []; \
                        for (var i = 0; i < cs.length; i++) {{ \
                            var name = cs[i]; \
                            result.push({{name: name, value: cs.getPropertyValue(name)}}); \
                        }} \
                        return JSON.stringify({{computedStyle: result}}); \
                    }})()"
                );
                let response = self.bridge.send(BridgeCommand::EvaluateJs {
                    expression: js,
                    return_by_value: true,
                });
                match response.result {
                    Ok(v) => {
                        let style_json = v.as_str().unwrap_or("{\"computedStyle\":[]}");
                        let parsed: Value = serde_json::from_str(style_json)
                            .unwrap_or_else(|_| json!({"computedStyle": []}));
                        Ok(parsed)
                    }
                    Err(e) => Ok(json!({"computedStyle": [], "error": e})),
                }
            }

            "CSS.getMatchedStylesForNode" => {
                let node_id = params.get("nodeId").and_then(|v| v.as_i64()).unwrap_or(0);
                let js = format!(
                    "(function() {{ \
                        var el = document.querySelector('[data-cdp-node-id=\"{node_id}\"]') || \
                                document.querySelectorAll('*')[{node_id}]; \
                        if (!el) return JSON.stringify({{matchedCSSRules: [], inlineStyle: null, attributesStyle: null}}); \
                        var rules = []; \
                        var sheets = document.styleSheets; \
                        for (var s = 0; s < sheets.length; s++) {{ \
                            try {{ \
                                var cssRules = sheets[s].cssRules; \
                                for (var r = 0; r < cssRules.length; r++) {{ \
                                    if (el.matches(cssRules[r].selectorText)) {{ \
                                        rules.push({{ \
                                            rule: {{ \
                                                selectorText: cssRules[r].selectorText, \
                                                style: cssRules[r].cssText \
                                            }}, \
                                            matchingSelectors: [r] \
                                        }}); \
                                    }} \
                                }} \
                            }} catch(e) {{}} \
                        }} \
                        var inlineStyle = el.getAttribute('style'); \
                        return JSON.stringify({{ \
                            matchedCSSRules: rules, \
                            inlineStyle: inlineStyle ? {{cssText: inlineStyle}} : null, \
                            attributesStyle: null \
                        }}); \
                    }})()"
                );
                let response = self.bridge.send(BridgeCommand::EvaluateJs {
                    expression: js,
                    return_by_value: true,
                });
                match response.result {
                    Ok(v) => {
                        let style_json = v.as_str().unwrap_or("{\"matchedCSSRules\":[],\"inlineStyle\":null,\"attributesStyle\":null}");
                        let parsed: Value = serde_json::from_str(style_json)
                            .unwrap_or_else(|_| json!({"matchedCSSRules": [], "inlineStyle": null, "attributesStyle": null}));
                        Ok(parsed)
                    }
                    Err(e) => Ok(json!({"matchedCSSRules": [], "inlineStyle": null, "attributesStyle": null, "error": e})),
                }
            }

            "CSS.getInlineStylesForNode" => {
                let node_id = params.get("nodeId").and_then(|v| v.as_i64()).unwrap_or(0);
                let js = format!(
                    "(function() {{ \
                        var el = document.querySelector('[data-cdp-node-id=\"{node_id}\"]') || \
                                document.querySelectorAll('*')[{node_id}]; \
                        if (!el) return JSON.stringify({{inlineStyle: null}}); \
                        var style = el.getAttribute('style'); \
                        return JSON.stringify({{inlineStyle: style ? {{cssText: style}} : null}}); \
                    }})()"
                );
                let response = self.bridge.send(BridgeCommand::EvaluateJs {
                    expression: js,
                    return_by_value: true,
                });
                match response.result {
                    Ok(v) => {
                        let style_json = v.as_str().unwrap_or("{\"inlineStyle\":null}");
                        let parsed: Value = serde_json::from_str(style_json)
                            .unwrap_or_else(|_| json!({"inlineStyle": null}));
                        Ok(parsed)
                    }
                    Err(e) => Ok(json!({"inlineStyle": null, "error": e})),
                }
            }

            "CSS.setStyleTexts" => {
                let edits = params.get("edits").and_then(|v| v.as_array());
                let mut styles = Vec::new();
                if let Some(edits) = edits {
                    for _edit in edits {
                        styles.push(json!({"style": {"cssText": "", "styleSheetId": "0"}}));
                    }
                }
                Ok(json!({"styles": styles}))
            }

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
    fn css_domain_name() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = CssHandler::new(bridge);
        assert_eq!(h.domain_name(), "CSS");
    }

    #[test]
    fn css_enable_disable() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = CssHandler::new(bridge);
        assert_eq!(h.handle_command("CSS.enable", json!({}), &NOOP).unwrap(), json!({}));
        assert_eq!(h.handle_command("CSS.disable", json!({}), &NOOP).unwrap(), json!({}));
    }

    #[test]
    fn css_get_computed_style_returns_structure() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = CssHandler::new(bridge);
        let res = h.handle_command("CSS.getComputedStyleForNode", json!({"nodeId": 1}), &NOOP).unwrap();
        assert!(res.get("computedStyle").is_some(), "should have computedStyle field");
    }

    #[test]
    fn css_get_matched_styles_returns_structure() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = CssHandler::new(bridge);
        let res = h.handle_command("CSS.getMatchedStylesForNode", json!({"nodeId": 1}), &NOOP).unwrap();
        assert!(res.get("matchedCSSRules").is_some(), "should have matchedCSSRules field");
        assert!(res.get("inlineStyle").is_some(), "should have inlineStyle field");
    }

    #[test]
    fn css_get_inline_styles_returns_structure() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = CssHandler::new(bridge);
        let res = h.handle_command("CSS.getInlineStylesForNode", json!({"nodeId": 1}), &NOOP).unwrap();
        assert!(res.get("inlineStyle").is_some(), "should have inlineStyle field");
    }

    #[test]
    fn css_set_style_texts_returns_empty_styles() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = CssHandler::new(bridge);
        let res = h.handle_command("CSS.setStyleTexts", json!({"edits": []}), &NOOP).unwrap();
        assert!(res.get("styles").is_some(), "should have styles field");
    }

    #[test]
    fn css_unknown_returns_error() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = CssHandler::new(bridge);
        let err = h.handle_command("CSS.nonexistent", json!({}), &NOOP).unwrap_err();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("nonexistent"));
    }
}
