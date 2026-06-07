// @trace REQ-CDP-006
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::BridgeSender;

/// JS interceptor script injected into the page when Network.enable is called.
/// Monitors fetch and XMLHttpRequest, reports real network events via console channel.
const NETWORK_INTERCEPTOR_JS: &str = r#"
(function() {
    if (window.__bao_network_interceptor_active) return;
    window.__bao_network_interceptor_active = true;
    window.__bao_network_request_counter = 0;

    const origFetch = window.fetch;
    window.fetch = function(...args) {
        const id = 'net-' + (++window.__bao_network_request_counter);
        const input = args[0];
        const init = args[1] || {};
        const url = typeof input === 'string' ? input : (input instanceof Request ? input.url : String(input));
        const method = init.method || (input instanceof Request ? input.method : 'GET');
        const headers = {};
        if (init.headers) {
            if (init.headers instanceof Headers) {
                init.headers.forEach((v, k) => { headers[k] = v; });
            } else if (typeof init.headers === 'object') {
                Object.assign(headers, init.headers);
            }
        }
        const ts = Date.now() / 1000;

        // Report request
        const payload = JSON.stringify({
            id, url, method, headers, type: 'Fetch',
            request: { url, method, headers },
            timestamp: ts,
        });
        console.log('__BAO_NETWORK_REQUEST__' + payload);

        return origFetch.apply(this, args).then(response => {
            // Report response
            const respPayload = JSON.stringify({
                id, url, status: response.status, statusText: response.statusText,
                headers: Object.fromEntries(response.headers.entries()),
                type: 'Fetch',
                timestamp: Date.now() / 1000,
            });
            console.log('__BAO_NETWORK_RESPONSE__' + respPayload);
            return response;
        });
    };

    const origOpen = XMLHttpRequest.prototype.open;
    const origSend = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function(method, url) {
        this.__bao_net_id = 'xhr-' + (++window.__bao_network_request_counter);
        this.__bao_method = method;
        this.__bao_url = url;
        return origOpen.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function(body) {
        const id = this.__bao_net_id;
        const ts = Date.now() / 1000;
        const payload = JSON.stringify({
            id, url: this.__bao_url, method: this.__bao_method, type: 'XHR',
            request: { url: this.__bao_url, method: this.__bao_method },
            timestamp: ts,
        });
        console.log('__BAO_NETWORK_REQUEST__' + payload);

        this.addEventListener('load', function() {
            const respPayload = JSON.stringify({
                id, url: this.__bao_url, status: this.status, statusText: this.statusText,
                type: 'XHR', timestamp: Date.now() / 1000,
            });
            console.log('__BAO_NETWORK_RESPONSE__' + respPayload);
        });
        this.addEventListener('error', function() {
            const errPayload = JSON.stringify({
                id, url: this.__bao_url, error: true, type: 'XHR', timestamp: Date.now() / 1000,
            });
            console.log('__BAO_NETWORK_LOADING_FAILED__' + errPayload);
        });
        return origSend.apply(this, arguments);
    };
})();
"#;

/// Network domain handler — real network request/response monitoring via JS interceptor.
///
/// When Network.enable is called, injects a JS interceptor that patches window.fetch
/// and XMLHttpRequest to report real request/response data through the console channel.
/// The CDP server detects __BAO_NETWORK_REQUEST__/__BAO_NETWORK_RESPONSE__ prefixes
/// and broadcasts them as Network.requestWillBeSent / Network.responseReceived events.
pub struct NetworkHandler {
    bridge: BridgeSender,
    enabled: std::sync::Mutex<bool>,
}

impl NetworkHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        NetworkHandler {
            bridge,
            enabled: std::sync::Mutex::new(false),
        }
    }
}

impl DomainHandler for NetworkHandler {
    fn domain_name(&self) -> &'static str { "Network" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Network.enable" => {
                *self.enabled.lock().unwrap() = true;
                // Inject network monitoring interceptor into the page
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: NETWORK_INTERCEPTOR_JS.to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Network.disable" => {
                *self.enabled.lock().unwrap() = false;
                Ok(json!({}))
            }
            "Network.getResponseBody" => {
                let request_id = params.get("requestId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                // Try to fetch response body via bridge evaluate
                let js = format!(
                    "(function() {{ var r = window.__bao_response_bodies && window.__bao_response_bodies[{}]; return r || ''; }})()",
                    serde_json::to_string(&request_id).unwrap_or_else(|_| "\"\"".into())
                );
                let resp = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: js,
                    return_by_value: true,
                });
                let body = resp.result.ok().and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                Ok(json!({ "body": body, "base64Encoded": false }))
            }
            "Network.setCacheDisabled" | "Network.setExtraHTTPHeaders" => Ok(json!({})),
            "Network.emulateNetworkConditions" | "Network.setRequestInterception" => Ok(json!({})),
            "Network.continueInterceptedRequest" => Ok(json!({})),
            "Network.getCookies" | "Network.getAllCookies" => {
                // Query document.cookie for real cookies
                let resp = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: "document.cookie || ''".to_string(),
                    return_by_value: true,
                });
                let cookie_str = resp.result.ok().and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                let cookies: Vec<Value> = if cookie_str.is_empty() {
                    Vec::new()
                } else {
                    cookie_str.split(';').filter_map(|pair| {
                        let parts: Vec<&str> = pair.trim().splitn(2, '=').collect();
                        if parts.len() == 2 {
                            Some(json!({ "name": parts[0].trim(), "value": parts[1].trim(), "domain": "", "path": "/" }))
                        } else {
                            None
                        }
                    }).collect()
                };
                Ok(json!({ "cookies": cookies }))
            }
            "Network.deleteCookies" | "Network.setCookie" => Ok(json!({})),
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

    #[test]
    fn domain_name_returns_Network() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = NetworkHandler::new(bridge);
        assert_eq!(handler.domain_name(), "Network");
    }

    #[test]
    fn enable_sets_flag_and_sends_bridge_command() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = NetworkHandler::new(bridge);
        assert!(!*handler.enabled.lock().unwrap());
        handler.handle_command("Network.enable", json!({}), &NoopSender).unwrap();
        assert!(*handler.enabled.lock().unwrap());
        // Verify bridge received EvaluateJs
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_network_interceptor_active"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Network.enable should inject interceptor via bridge");
    }

    #[test]
    fn enable_does_not_fire_fabricated_request_event() {
        struct CollectSender(std::sync::Mutex<Vec<String>>);
        impl EventSender for CollectSender {
            fn send_event(&self, method: &str, _params: Value) {
                self.0.lock().unwrap().push(method.to_string());
            }
        }
        let collector = CollectSender(std::sync::Mutex::new(Vec::new()));
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = NetworkHandler::new(bridge);
        handler.handle_command("Network.enable", json!({}), &collector).unwrap();
        let events = collector.0.lock().unwrap();
        assert!(events.is_empty(), "Network.enable must NOT emit fabricated requestWillBeSent");
    }

    #[test]
    fn disable_clears_flag() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = NetworkHandler::new(bridge);
        handler.handle_command("Network.enable", json!({}), &NoopSender).unwrap();
        handler.handle_command("Network.disable", json!({}), &NoopSender).unwrap();
        assert!(!*handler.enabled.lock().unwrap());
    }

    #[test]
    fn getResponseBody_returns_structure() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = NetworkHandler::new(bridge);
        let result = handler.handle_command("Network.getResponseBody", json!({"requestId": "test-1"}), &NoopSender).unwrap();
        assert!(result.get("body").is_some());
        assert_eq!(result["base64Encoded"], false);
    }

    #[test]
    fn setCacheDisabled_returns_ok_empty() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = NetworkHandler::new(bridge);
        let result = handler.handle_command("Network.setCacheDisabled", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn getCookies_returns_structure() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = NetworkHandler::new(bridge);
        let result = handler.handle_command("Network.getCookies", json!({}), &NoopSender).unwrap();
        assert!(result.get("cookies").is_some());
        assert!(result["cookies"].is_array());
    }

    #[test]
    fn unknown_command_returns_error_32601() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = NetworkHandler::new(bridge);
        let err = handler.handle_command("Network.nonexistent", json!({}), &NoopSender).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn network_interceptor_js_is_valid() {
        assert!(NETWORK_INTERCEPTOR_JS.contains("__bao_network_interceptor_active"));
        assert!(NETWORK_INTERCEPTOR_JS.contains("__BAO_NETWORK_REQUEST__"));
        assert!(NETWORK_INTERCEPTOR_JS.contains("__BAO_NETWORK_RESPONSE__"));
        assert!(NETWORK_INTERCEPTOR_JS.contains("__BAO_NETWORK_LOADING_FAILED__"));
    }

    #[test]
    fn network_interceptor_patches_fetch_and_xhr() {
        assert!(NETWORK_INTERCEPTOR_JS.contains("origFetch"));
        assert!(NETWORK_INTERCEPTOR_JS.contains("origOpen"));
        assert!(NETWORK_INTERCEPTOR_JS.contains("origSend"));
    }
}
