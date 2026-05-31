// @trace REQ-CDP-007
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};

// CSS, Overlay, Log, Fetch — stub domains that return static responses.

pub struct CssHandler;

impl DomainHandler for CssHandler {
    fn domain_name(&self) -> &'static str { "CSS" }
    fn handle_command(&self, command: &str, _params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "CSS.enable" | "CSS.disable" => Ok(json!({})),
            "CSS.getComputedStyleForNode" => Ok(json!({ "computedStyle": [] })),
            "CSS.getMatchedStylesForNode" => Ok(json!({ "matchedCSSRules": [], "inlineStyle": null, "attributesStyle": null })),
            "CSS.getInlineStylesForNode" => Ok(json!({ "inlineStyle": null })),
            "CSS.setStyleTexts" => Ok(json!({ "styles": [] })),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

pub struct OverlayHandler;

impl DomainHandler for OverlayHandler {
    fn domain_name(&self) -> &'static str { "Overlay" }
    fn handle_command(&self, command: &str, _params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Overlay.enable" | "Overlay.disable" => Ok(json!({})),
            "Overlay.highlightNode" | "Overlay.hideHighlight" | "Overlay.setInspectMode" => Ok(json!({})),
            "Overlay.setPausedInDebuggerMessage" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

pub struct LogHandler;

impl DomainHandler for LogHandler {
    fn domain_name(&self) -> &'static str { "Log" }
    fn handle_command(&self, command: &str, _params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Log.enable" | "Log.disable" | "Log.clear" => Ok(json!({})),
            "Log.startViolationsReport" | "Log.stopViolationsReport" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

pub struct FetchHandler;

impl DomainHandler for FetchHandler {
    fn domain_name(&self) -> &'static str { "Fetch" }
    fn handle_command(&self, command: &str, params: Value, _es: &dyn EventSender) -> Result<Value, CdpError> {
        let ps = |key: &str| params.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
        match command {
            "Fetch.enable" => {
                let count = params.get("patterns").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                Ok(json!({ "enabled": true, "patternCount": count }))
            }
            "Fetch.disable" => Ok(json!({})),
            "Fetch.continueRequest" | "Fetch.continueWithResponse" => {
                Ok(json!({ "requestId": ps("requestId"), "continued": true }))
            }
            "Fetch.failRequest" => Ok(json!({ "requestId": ps("requestId"), "failed": true, "reason": ps("reason") })),
            "Fetch.fulfillRequest" => {
                let status_code = params.get("responseCode").and_then(|v| v.as_u64()).unwrap_or(200);
                let body = ps("body");
                Ok(json!({ "requestId": ps("requestId"), "fulfilled": true, "responseCode": status_code, "bodyLength": body.len() }))
            }
            "Fetch.getRequestPostData" => Ok(json!({ "requestId": ps("requestId"), "postData": "" })),
            "Fetch.continueWithAuth" => Ok(json!({ "requestId": ps("requestId") })),
            "Fetch.takeResponseBodyAsStream" => Ok(json!({ "stream": format!("stream-{}", ps("requestId")) })),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdp_server::DomainHandler;

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }
    static NOOP: NoopSender = NoopSender;

    // ── CSS ──────────────────────────────────────────────────────

    #[test]
    fn css_domain_name() {
        let h = CssHandler;
        assert_eq!(h.domain_name(), "CSS");
    }

    #[test]
    fn css_enable_disable() {
        let h = CssHandler;
        assert_eq!(h.handle_command("CSS.enable", json!({}), &NOOP).unwrap(), json!({}));
        assert_eq!(h.handle_command("CSS.disable", json!({}), &NOOP).unwrap(), json!({}));
    }

    #[test]
    fn css_get_computed_style_for_node() {
        let h = CssHandler;
        let res = h.handle_command("CSS.getComputedStyleForNode", json!({}), &NOOP).unwrap();
        assert!(res.get("computedStyle").is_some());
    }

    #[test]
    fn css_get_matched_styles_for_node() {
        let h = CssHandler;
        let res = h.handle_command("CSS.getMatchedStylesForNode", json!({}), &NOOP).unwrap();
        assert!(res.get("matchedCSSRules").is_some());
        assert!(res.get("inlineStyle").is_some());
        assert!(res.get("attributesStyle").is_some());
    }

    #[test]
    fn css_get_inline_styles_for_node() {
        let h = CssHandler;
        let res = h.handle_command("CSS.getInlineStylesForNode", json!({}), &NOOP).unwrap();
        assert!(res.get("inlineStyle").is_some());
    }

    #[test]
    fn css_set_style_texts() {
        let h = CssHandler;
        let res = h.handle_command("CSS.setStyleTexts", json!({}), &NOOP).unwrap();
        assert!(res.get("styles").is_some());
    }

    #[test]
    fn css_unknown_returns_error() {
        let h = CssHandler;
        let err = h.handle_command("CSS.nonexistent", json!({}), &NOOP).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    // ── Overlay ─────────────────────────────────────────────────

    #[test]
    fn overlay_domain_name() {
        let h = OverlayHandler;
        assert_eq!(h.domain_name(), "Overlay");
    }

    #[test]
    fn overlay_enable_disable() {
        let h = OverlayHandler;
        assert_eq!(h.handle_command("Overlay.enable", json!({}), &NOOP).unwrap(), json!({}));
        assert_eq!(h.handle_command("Overlay.disable", json!({}), &NOOP).unwrap(), json!({}));
    }

    #[test]
    fn overlay_highlight_node_hide_highlight() {
        let h = OverlayHandler;
        assert_eq!(h.handle_command("Overlay.highlightNode", json!({}), &NOOP).unwrap(), json!({}));
        assert_eq!(h.handle_command("Overlay.hideHighlight", json!({}), &NOOP).unwrap(), json!({}));
    }

    #[test]
    fn overlay_set_paused_in_debugger_message() {
        let h = OverlayHandler;
        assert_eq!(h.handle_command("Overlay.setPausedInDebuggerMessage", json!({}), &NOOP).unwrap(), json!({}));
    }

    #[test]
    fn overlay_unknown_returns_error() {
        let h = OverlayHandler;
        let err = h.handle_command("Overlay.nonexistent", json!({}), &NOOP).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    // ── Log ─────────────────────────────────────────────────────

    #[test]
    fn log_domain_name() {
        let h = LogHandler;
        assert_eq!(h.domain_name(), "Log");
    }

    #[test]
    fn log_enable_disable_clear() {
        let h = LogHandler;
        assert_eq!(h.handle_command("Log.enable", json!({}), &NOOP).unwrap(), json!({}));
        assert_eq!(h.handle_command("Log.disable", json!({}), &NOOP).unwrap(), json!({}));
        assert_eq!(h.handle_command("Log.clear", json!({}), &NOOP).unwrap(), json!({}));
    }

    #[test]
    fn log_violations_report() {
        let h = LogHandler;
        assert_eq!(h.handle_command("Log.startViolationsReport", json!({}), &NOOP).unwrap(), json!({}));
        assert_eq!(h.handle_command("Log.stopViolationsReport", json!({}), &NOOP).unwrap(), json!({}));
    }

    #[test]
    fn log_unknown_returns_error() {
        let h = LogHandler;
        let err = h.handle_command("Log.nonexistent", json!({}), &NOOP).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    // ── Fetch ───────────────────────────────────────────────────

    #[test]
    fn fetch_domain_name() {
        let h = FetchHandler;
        assert_eq!(h.domain_name(), "Fetch");
    }

    #[test]
    fn fetch_enable_returns_count() {
        let h = FetchHandler;
        let res = h.handle_command("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}), &NOOP).unwrap();
        assert_eq!(res["enabled"], true);
        assert_eq!(res["patternCount"], 1);
    }

    #[test]
    fn fetch_enable_no_patterns_zero_count() {
        let h = FetchHandler;
        let res = h.handle_command("Fetch.enable", json!({}), &NOOP).unwrap();
        assert_eq!(res["enabled"], true);
        assert_eq!(res["patternCount"], 0);
    }

    #[test]
    fn fetch_disable() {
        let h = FetchHandler;
        assert_eq!(h.handle_command("Fetch.disable", json!({}), &NOOP).unwrap(), json!({}));
    }

    #[test]
    fn fetch_continue_request_returns_request_id() {
        let h = FetchHandler;
        let res = h.handle_command("Fetch.continueRequest", json!({"requestId": "r1"}), &NOOP).unwrap();
        assert_eq!(res["requestId"], "r1");
        assert_eq!(res["continued"], true);
    }

    #[test]
    fn fetch_continue_with_response_returns_request_id() {
        let h = FetchHandler;
        let res = h.handle_command("Fetch.continueWithResponse", json!({"requestId": "r1"}), &NOOP).unwrap();
        assert_eq!(res["requestId"], "r1");
        assert_eq!(res["continued"], true);
    }

    #[test]
    fn fetch_fail_request_returns_reason() {
        let h = FetchHandler;
        let res = h.handle_command("Fetch.failRequest", json!({"requestId": "r1", "reason": "Aborted"}), &NOOP).unwrap();
        assert_eq!(res["requestId"], "r1");
        assert_eq!(res["failed"], true);
        assert_eq!(res["reason"], "Aborted");
    }

    #[test]
    fn fetch_fulfill_request_returns_status_and_body_length() {
        let h = FetchHandler;
        let res = h.handle_command(
            "Fetch.fulfillRequest",
            json!({"requestId": "r1", "responseCode": 404, "body": "not found"}),
            &NOOP,
        ).unwrap();
        assert_eq!(res["requestId"], "r1");
        assert_eq!(res["fulfilled"], true);
        assert_eq!(res["responseCode"], 404);
        assert_eq!(res["bodyLength"], 9);
    }

    #[test]
    fn fetch_get_request_post_data() {
        let h = FetchHandler;
        let res = h.handle_command("Fetch.getRequestPostData", json!({"requestId": "r1"}), &NOOP).unwrap();
        assert_eq!(res["requestId"], "r1");
        assert!(res.get("postData").is_some());
    }

    #[test]
    fn fetch_continue_with_auth() {
        let h = FetchHandler;
        let res = h.handle_command("Fetch.continueWithAuth", json!({"requestId": "r1"}), &NOOP).unwrap();
        assert_eq!(res["requestId"], "r1");
    }

    #[test]
    fn fetch_take_response_body_as_stream() {
        let h = FetchHandler;
        let res = h.handle_command("Fetch.takeResponseBodyAsStream", json!({"requestId": "r1"}), &NOOP).unwrap();
        assert_eq!(res["stream"], "stream-r1");
    }

    #[test]
    fn fetch_unknown_returns_error() {
        let h = FetchHandler;
        let err = h.handle_command("Fetch.nonexistent", json!({}), &NOOP).unwrap_err();
        assert_eq!(err.code, -32601);
    }
}
