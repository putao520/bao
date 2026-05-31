// @trace REQ-CDP-006
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};

pub struct NetworkHandler;

impl DomainHandler for NetworkHandler {
    fn domain_name(&self) -> &'static str { "Network" }

    fn handle_command(
        &self,
        command: &str,
        _params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Network.enable" | "Network.disable" => Ok(json!({})),
            "Network.getResponseBody" => Ok(json!({ "body": "", "base64Encoded": false })),
            "Network.setCacheDisabled" | "Network.setExtraHTTPHeaders" => Ok(json!({})),
            "Network.emulateNetworkConditions" | "Network.setRequestInterception" => Ok(json!({})),
            "Network.continueInterceptedRequest" => Ok(json!({})),
            "Network.getCookies" | "Network.getAllCookies" => Ok(json!({ "cookies": [] })),
            "Network.deleteCookies" | "Network.setCookie" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }

    #[test]
    fn domain_name_returns_Network() {
        let handler = NetworkHandler;
        assert_eq!(handler.domain_name(), "Network");
    }

    #[test]
    fn enable_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.enable", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn disable_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.disable", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn getResponseBody_returns_body_and_base64Encoded_false() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.getResponseBody", json!({}), &es).unwrap();
        assert_eq!(result, json!({ "body": "", "base64Encoded": false }));
    }

    #[test]
    fn setCacheDisabled_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.setCacheDisabled", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn setExtraHTTPHeaders_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.setExtraHTTPHeaders", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn emulateNetworkConditions_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.emulateNetworkConditions", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn setRequestInterception_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.setRequestInterception", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn continueInterceptedRequest_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.continueInterceptedRequest", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn getCookies_returns_empty_cookies_array() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.getCookies", json!({}), &es).unwrap();
        assert_eq!(result, json!({ "cookies": [] }));
    }

    #[test]
    fn getAllCookies_returns_empty_cookies_array() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.getAllCookies", json!({}), &es).unwrap();
        assert_eq!(result, json!({ "cookies": [] }));
    }

    #[test]
    fn deleteCookies_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.deleteCookies", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn setCookie_returns_ok_empty() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let result = handler.handle_command("Network.setCookie", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn unknown_command_returns_error_32601() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let err = handler.handle_command("Network.nonexistent", json!({}), &es).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn unknown_command_error_message_contains_command_name() {
        let handler = NetworkHandler;
        let es = NoopSender;
        let err = handler.handle_command("Network.nonexistent", json!({}), &es).unwrap_err();
        assert!(err.message.contains("Network.nonexistent"));
    }
}
