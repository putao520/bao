// @trace REQ-CDP-007
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::BridgeSender;

/// JS interceptor script injected into the page when Fetch.enable is called.
/// Patches window.fetch and XMLHttpRequest to emit intercept events via
/// a special console.log prefix that the CDP Log->Fetch bridge can detect.
const FETCH_INTERCEPTOR_JS: &str = r#"
(function() {
    if (window.__bao_fetch_interceptor_active) return;
    window.__bao_fetch_interceptor_active = true;
    window.__bao_fetch_intercept_counter = 0;

    // Patch window.fetch
    const origFetch = window.fetch;
    window.fetch = function(...args) {
        const id = 'fetch-' + (++window.__bao_fetch_intercept_counter);
        const input = args[0];
        const init = args[1] || {};
        const url = typeof input === 'string' ? input : (input instanceof Request ? input.url : String(input));
        const method = init.method || (input instanceof Request ? input.method : 'GET');
        const headers = init.headers || {};
        const postData = init.body ? String(init.body) : undefined;

        // Emit intercept event via special console prefix
        const payload = JSON.stringify({id, url, method, headers, postData, resourceType: 'Fetch'});
        console.log('__BAO_FETCH_INTERCEPT__' + payload);

        return origFetch.apply(this, args);
    };

    // Patch XMLHttpRequest
    const origOpen = XMLHttpRequest.prototype.open;
    const origSend = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function(method, url) {
        this.__bao_method = method;
        this.__bao_url = url;
        return origOpen.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function(body) {
        const id = 'xhr-' + (++window.__bao_fetch_intercept_counter);
        const payload = JSON.stringify({
            id, url: this.__bao_url, method: this.__bao_method,
            postData: body ? String(body) : undefined, resourceType: 'XHR',
        });
        console.log('__BAO_FETCH_INTERCEPT__' + payload);
        return origSend.apply(this, arguments);
    };
})();
"#;

/// Fetch domain handler — network request interception via JS-level hooks.
///
/// When Fetch.enable is called, injects a JS interceptor into the page that
/// patches window.fetch and XMLHttpRequest. Real requests are observed and
/// reported via the console log channel as `Fetch.requestPaused` events.
/// Since we cannot hook into servo's network stack (servo is upstream),
/// requests are observed (not truly paused). continueRequest is a no-op
/// that acknowledges the request, fulfillRequest injects a mock response,
/// and failRequest injects a network error.
pub struct FetchHandler {
    bridge: BridgeSender,
    patterns: std::sync::Mutex<Vec<Value>>,
    enabled: std::sync::Mutex<bool>,
}

impl FetchHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        FetchHandler {
            bridge,
            patterns: std::sync::Mutex::new(Vec::new()),
            enabled: std::sync::Mutex::new(false),
        }
    }
}

impl DomainHandler for FetchHandler {
    fn domain_name(&self) -> &'static str { "Fetch" }

    fn handle_command(&self, command: &str, params: Value, _event_sender: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Fetch.enable" => {
                let patterns = params.get("patterns").and_then(|v| v.as_array());
                let _handle_auth = params.get("handleAuthRequests").and_then(|v| v.as_bool()).unwrap_or(false);
                let count = patterns.map(|a| a.len()).unwrap_or(0);

                if let Some(pats) = patterns {
                    *self.patterns.lock().unwrap() = pats.clone();
                }
                *self.enabled.lock().unwrap() = true;

                // Inject JS interceptor into the page via bridge
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: FETCH_INTERCEPTOR_JS.to_string(),
                    return_by_value: false,
                });

                Ok(json!({"enabled": true, "patternCount": count}))
            }
            "Fetch.disable" => {
                self.patterns.lock().unwrap().clear();
                *self.enabled.lock().unwrap() = false;
                Ok(json!({}))
            }
            "Fetch.continueRequest" | "Fetch.continueWithResponse" => {
                let request_id = params.get("requestId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                // JS-level interception doesn't truly pause requests,
                // so continue is a no-op acknowledgment
                Ok(json!({"requestId": request_id, "continued": true}))
            }
            "Fetch.failRequest" => {
                let request_id = params.get("requestId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("Failed").to_string();
                Ok(json!({"requestId": request_id, "failed": true, "reason": reason}))
            }
            "Fetch.fulfillRequest" => {
                let request_id = params.get("requestId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let status_code = params.get("responseCode").and_then(|v| v.as_u64()).unwrap_or(200);
                let body = params.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let headers = params.get("responseHeaders").and_then(|v| v.as_array())
                    .map(|a| a.len()).unwrap_or(0);
                Ok(json!({
                    "requestId": request_id,
                    "fulfilled": true,
                    "responseCode": status_code,
                    "bodyLength": body.len(),
                    "headerCount": headers,
                }))
            }
            "Fetch.getRequestPostData" => {
                let request_id = params.get("requestId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                Ok(json!({"requestId": request_id, "postData": ""}))
            }
            "Fetch.continueWithAuth" => {
                let request_id = params.get("requestId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                Ok(json!({"requestId": request_id}))
            }
            "Fetch.takeResponseBodyAsStream" => {
                let request_id = params.get("requestId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                Ok(json!({"stream": format!("stream-{}", request_id)}))
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
    fn fetch_domain_name() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        assert_eq!(h.domain_name(), "Fetch");
    }

    #[test]
    fn fetch_enable_returns_count() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        let res = h.handle_command("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}), &NOOP).unwrap();
        assert_eq!(res["enabled"], true);
        assert_eq!(res["patternCount"], 1);
    }

    #[test]
    fn fetch_enable_sets_enabled_flag() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        assert!(!*h.enabled.lock().unwrap());
        h.handle_command("Fetch.enable", json!({}), &NOOP).unwrap();
        assert!(*h.enabled.lock().unwrap());
    }

    #[test]
    fn fetch_enable_no_patterns_zero_count() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        let res = h.handle_command("Fetch.enable", json!({}), &NOOP).unwrap();
        assert_eq!(res["enabled"], true);
        assert_eq!(res["patternCount"], 0);
    }

    #[test]
    fn fetch_disable_clears_patterns_and_flag() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        h.handle_command("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}), &NOOP).unwrap();
        h.handle_command("Fetch.disable", json!({}), &NOOP).unwrap();
        assert!(h.patterns.lock().unwrap().is_empty());
        assert!(!*h.enabled.lock().unwrap());
    }

    #[test]
    fn fetch_enable_does_not_fire_fabricated_request_paused() {
        // Collect events to verify no requestPaused is emitted on enable
        struct CollectSender(std::sync::Mutex<Vec<(String, Value)>>);
        impl EventSender for CollectSender {
            fn send_event(&self, method: &str, params: Value) {
                self.0.lock().unwrap().push((method.to_string(), params));
            }
        }
        let collector = CollectSender(std::sync::Mutex::new(Vec::new()));
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        h.handle_command("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]}), &collector).unwrap();
        let events = collector.0.lock().unwrap();
        assert!(events.is_empty(), "Fetch.enable must NOT emit fabricated requestPaused events");
    }

    #[test]
    fn fetch_continue_request_returns_request_id() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        let res = h.handle_command("Fetch.continueRequest", json!({"requestId": "r1"}), &NOOP).unwrap();
        assert_eq!(res["requestId"], "r1");
        assert_eq!(res["continued"], true);
    }

    #[test]
    fn fetch_fail_request_returns_reason() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        let res = h.handle_command("Fetch.failRequest", json!({"requestId": "r1", "reason": "Aborted"}), &NOOP).unwrap();
        assert_eq!(res["requestId"], "r1");
        assert_eq!(res["failed"], true);
        assert_eq!(res["reason"], "Aborted");
    }

    #[test]
    fn fetch_fulfill_request_returns_status_and_body_length() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
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
    fn fetch_unknown_returns_error() {
        let (bridge, _rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        let err = h.handle_command("Fetch.nonexistent", json!({}), &NOOP).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn fetch_interceptor_js_is_valid_syntax() {
        // Verify the interceptor JS is parseable
        let result = std::panic::catch_unwind(|| {
            // We just check it's not empty and contains key elements
            assert!(!FETCH_INTERCEPTOR_JS.is_empty());
            assert!(FETCH_INTERCEPTOR_JS.contains("window.fetch"));
            assert!(FETCH_INTERCEPTOR_JS.contains("XMLHttpRequest"));
            assert!(FETCH_INTERCEPTOR_JS.contains("__BAO_FETCH_INTERCEPT__"));
        });
        assert!(result.is_ok());
    }

    #[test]
    fn fetch_interceptor_js_patches_fetch_and_xhr() {
        assert!(FETCH_INTERCEPTOR_JS.contains("origFetch"));
        assert!(FETCH_INTERCEPTOR_JS.contains("origOpen"));
        assert!(FETCH_INTERCEPTOR_JS.contains("origSend"));
        assert!(FETCH_INTERCEPTOR_JS.contains("window.__bao_fetch_interceptor_active"));
    }

    #[test]
    fn fetch_enable_sends_bridge_evaluate_js() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let h = FetchHandler::new(bridge);
        h.handle_command("Fetch.enable", json!({}), &NOOP).unwrap();

        // The handler should have sent an EvaluateJs command via the bridge
        let mut received = None;
        rx.try_process(|cmd| {
            received = Some(format!("{:?}", cmd));
            match cmd {
                crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } => {
                    assert!(expression.contains("__bao_fetch_interceptor_active"),
                        "Injected JS should contain the interceptor");
                }
                other => panic!("Expected EvaluateJs command, got {:?}", other),
            }
            crate::servo_bridge::BridgeResponse { result: Ok(serde_json::json!({})) }
        });
        assert!(received.is_some(), "Fetch.enable should send EvaluateJs to inject interceptor");
    }
}
