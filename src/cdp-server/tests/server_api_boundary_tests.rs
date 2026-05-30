// @trace TEST-CDS-012-SERVER-API [req:REQ-CDS-001,REQ-CDS-002,REQ-CDS-006] [level:unit]
// CdpServer public API boundary tests: construction, config integration,
// TargetInfo serialization edge cases, ServerConfig builder patterns.

use cdp_server::{
    CdpServer, ServerConfig, TargetInfo, DomainRegistry,
    CdpError, CdpMessage,
};
use serde_json::{Value, json};

// ---- CdpServer construction ----

#[test]
fn test_cdp_server_from_config() {
    let config = ServerConfig::builder()
        .host("0.0.0.0")
        .port(9444)
        .http_timeout_seconds(15)
        .max_sessions(25)
        .browser_name("TestBrowser/1.0")
        .user_agent("TestAgent")
        .v8_version("11.0")
        .webkit_version("537.36")
        .build();
    let server = CdpServer::new(config);
    assert_eq!(server.port(), 9444);
}

#[test]
fn test_cdp_server_default_config() {
    let config = ServerConfig::default();
    let server = CdpServer::new(config);
    assert_eq!(server.port(), 9222);
}

#[test]
fn test_cdp_server_ws_url_for_target() {
    let config = ServerConfig::builder().host("192.168.1.100").port(9333).build();
    let server = CdpServer::new(config);
    let url = server.ws_url_for_target("abc123");
    assert_eq!(url, "ws://192.168.1.100:9333/devtools/page/abc123");
}

#[test]
fn test_cdp_server_ws_url_empty_target() {
    let config = ServerConfig::builder().host("127.0.0.1").port(9222).build();
    let server = CdpServer::new(config);
    let url = server.ws_url_for_target("");
    assert_eq!(url, "ws://127.0.0.1:9222/devtools/page/");
}

#[test]
fn test_cdp_server_ws_url_unicode_target() {
    let server = CdpServer::new(ServerConfig::default());
    let url = server.ws_url_for_target("目标-123");
    assert!(url.contains("目标-123"));
}

#[test]
fn test_cdp_server_registry_accessible() {
    let server = CdpServer::new(ServerConfig::default());
    let _reg = server.registry();
}

#[test]
fn test_cdp_server_broadcaster_accessible() {
    let server = CdpServer::new(ServerConfig::default());
    let _bc = server.broadcaster();
}

// ---- TargetInfo serialization edge cases ----

#[test]
fn test_target_info_empty_fields() {
    let info = TargetInfo {
        id: String::new(),
        target_type: String::new(),
        title: String::new(),
        url: String::new(),
        web_socket_debugger_url: String::new(),
    };
    let serialized = serde_json::to_string(&info).unwrap();
    let parsed: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["id"], "");
    assert_eq!(parsed["type"], "");
    assert_eq!(parsed["title"], "");
}

#[test]
fn test_target_info_unicode_fields() {
    let info = TargetInfo {
        id: "目标-1".into(),
        target_type: "page".into(),
        title: "日本語テスト 🎉".into(),
        url: "https://example.com/路径?参数=值".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/目标-1".into(),
    };
    let serialized = serde_json::to_string(&info).unwrap();
    let deserialized: TargetInfo = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.id, "目标-1");
    assert_eq!(deserialized.title, "日本語テスト 🎉");
    assert_eq!(deserialized.url, "https://example.com/路径?参数=值");
}

#[test]
fn test_target_info_type_field_rename() {
    let info = TargetInfo {
        id: "test".into(),
        target_type: "iframe".into(),
        title: "T".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: "ws://localhost:9222/devtools/page/test".into(),
    };
    let serialized = serde_json::to_string(&info).unwrap();
    // `target_type` field should serialize as "type" (serde rename)
    assert!(serialized.contains("\"type\":\"iframe\""));
    assert!(!serialized.contains("target_type"));
}

#[test]
fn test_target_info_deserialize_with_type_rename() {
    let raw = r#"{"id":"x","type":"worker","title":"W","url":"about:blank","web_socket_debugger_url":"ws://127.0.0.1:9222/devtools/page/x"}"#;
    let info: TargetInfo = serde_json::from_str(raw).unwrap();
    assert_eq!(info.target_type, "worker");
}

#[test]
fn test_target_info_long_values() {
    let long_url = format!("https://example.com/{}", "a".repeat(10000));
    let info = TargetInfo {
        id: "long-target".into(),
        target_type: "page".into(),
        title: "X".repeat(5000),
        url: long_url.clone(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/long".into(),
    };
    let serialized = serde_json::to_string(&info).unwrap();
    let deserialized: TargetInfo = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.url.len(), long_url.len());
    assert_eq!(deserialized.title.len(), 5000);
}

#[test]
fn test_target_info_special_chars_in_url() {
    let info = TargetInfo {
        id: "spec".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "https://example.com/path?q=hello&lang=zh-CN#section".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/spec".into(),
    };
    let serialized = serde_json::to_string(&info).unwrap();
    let deserialized: TargetInfo = serde_json::from_str(&serialized).unwrap();
    assert!(deserialized.url.contains("?q=hello&lang=zh-CN#section"));
}

// ---- DomainRegistry multi-domain dispatch ----

struct PageDomain;
impl cdp_server::DomainHandler for PageDomain {
    fn domain_name(&self) -> &'static str { "Page" }
    fn handle_command(&self, cmd: &str, params: Value, _: &dyn cdp_server::EventSender) -> Result<Value, CdpError> {
        match cmd {
            "Page.navigate" => {
                let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("about:blank");
                Ok(json!({"frameId": "main", "loaderId": "1", "url": url}))
            },
            "Page.enable" | "Page.disable" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' not found", cmd) }),
        }
    }
}

struct RuntimeDomain;
impl cdp_server::DomainHandler for RuntimeDomain {
    fn domain_name(&self) -> &'static str { "Runtime" }
    fn handle_command(&self, cmd: &str, params: Value, _: &dyn cdp_server::EventSender) -> Result<Value, CdpError> {
        match cmd {
            "Runtime.evaluate" => {
                let expr = params.get("expression").and_then(|v| v.as_str()).unwrap_or("");
                Ok(json!({"result": {"type": "string", "value": expr}}))
            },
            "Runtime.enable" | "Runtime.disable" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' not found", cmd) }),
        }
    }
}

struct DomDomain;
impl cdp_server::DomainHandler for DomDomain {
    fn domain_name(&self) -> &'static str { "DOM" }
    fn handle_command(&self, cmd: &str, params: Value, _: &dyn cdp_server::EventSender) -> Result<Value, CdpError> {
        match cmd {
            "DOM.getDocument" => Ok(json!({"root": {"nodeId": 1, "nodeName": "#document"}})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' not found", cmd) }),
        }
    }
}

struct NopSender;
impl cdp_server::EventSender for NopSender {
    fn send_event(&self, _: &str, _: Value) {}
}

#[test]
fn test_multi_domain_registry_dispatch() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(PageDomain)).unwrap();
    reg.register(Box::new(RuntimeDomain)).unwrap();
    reg.register(Box::new(DomDomain)).unwrap();
    let sender = NopSender;

    // Page domain
    let r1 = reg.dispatch_command("Page.navigate", json!({"url": "https://example.com"}), &sender);
    assert!(r1.unwrap().unwrap()["url"] == "https://example.com");

    // Runtime domain
    let r2 = reg.dispatch_command("Runtime.evaluate", json!({"expression": "1+1"}), &sender);
    assert!(r2.unwrap().unwrap()["result"]["value"] == "1+1");

    // DOM domain
    let r3 = reg.dispatch_command("DOM.getDocument", json!({}), &sender);
    assert!(r3.unwrap().unwrap()["root"]["nodeId"] == 1);
}

#[test]
fn test_multi_domain_unknown_command_in_known_domain() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(PageDomain)).unwrap();
    let sender = NopSender;

    let result = reg.dispatch_command("Page.nonexistent", json!({}), &sender);
    let err = result.unwrap().unwrap_err();
    assert_eq!(err.code, -32601);
}

#[test]
fn test_multi_domain_unknown_domain_returns_none() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(PageDomain)).unwrap();
    let sender = NopSender;

    let result = reg.dispatch_command("Network.enable", json!({}), &sender);
    assert!(result.is_none());
}

#[test]
fn test_multi_domain_duplicate_registration_fails() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(PageDomain)).unwrap();
    let result = reg.register(Box::new(PageDomain));
    assert!(result.is_err());
}

// ---- CdpMessage + CdpError edge cases ----

#[test]
fn test_cdp_message_with_very_long_method() {
    let method = format!("{}.{}", "A".repeat(1000), "B".repeat(1000));
    let raw = json!({"id": 1, "method": method}).to_string();
    let msg: CdpMessage = serde_json::from_str(&raw).unwrap();
    assert!(msg.method.len() > 2000);
}

#[test]
fn test_cdp_error_with_very_long_message() {
    let msg = "X".repeat(10000);
    let err = CdpError { code: -32000, message: msg.clone() };
    let serialized = serde_json::to_string(&err).unwrap();
    let parsed: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed["message"].as_str().unwrap().len(), 10000);
}

#[test]
fn test_cdp_message_with_boolean_params() {
    let raw = r#"{"id":1,"method":"Test.cmd","params":{"flag":true,"count":0,"name":""}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["flag"], true);
    assert_eq!(params["count"], 0);
    assert_eq!(params["name"], "");
}

#[test]
fn test_cdp_message_with_null_params_is_none() {
    let raw = r#"{"id":1,"method":"Test.cmd","params":null}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    assert!(msg.params.is_none());
}

#[test]
fn test_cdp_message_with_nested_array_params() {
    let raw = r#"{"id":1,"method":"Test.cmd","params":{"items":[1,2,3],"nested":{"deep":{"value":42}}}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let params = msg.params.unwrap();
    assert_eq!(params["items"].as_array().unwrap().len(), 3);
    assert_eq!(params["nested"]["deep"]["value"], 42);
}

// ---- ServerConfig builder immutability ----

#[test]
fn test_server_config_builder_can_reuse() {
    let base = ServerConfig::builder()
        .host("0.0.0.0")
        .http_timeout_seconds(60);
    let c1 = base.port(9222).build();
    // builder is consumed, can't reuse — verify the result
    assert_eq!(c1.host, "0.0.0.0");
    assert_eq!(c1.port, 9222);
    assert_eq!(c1.http_timeout_seconds, 60);
}

#[test]
fn test_server_config_all_optional_fields() {
    let config = ServerConfig::builder()
        .user_agent("UA")
        .v8_version("V8")
        .webkit_version("WK")
        .build();
    assert_eq!(config.user_agent.as_deref(), Some("UA"));
    assert_eq!(config.v8_version.as_deref(), Some("V8"));
    assert_eq!(config.webkit_version.as_deref(), Some("WK"));
}

#[test]
fn test_server_config_no_optional_fields_default_none() {
    let config = ServerConfig::default();
    assert!(config.user_agent.is_none());
    assert!(config.v8_version.is_none());
    assert!(config.webkit_version.is_none());
}

// ---- Full roundtrip: parse → dispatch → response ----

#[test]
fn test_full_cdp_roundtrip_page_navigate() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(PageDomain)).unwrap();
    let sender = NopSender;

    let raw = r#"{"id":42,"method":"Page.navigate","params":{"url":"https://example.com"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap(), &sender);
    let value = result.unwrap().unwrap();
    assert_eq!(value["url"], "https://example.com");
    assert_eq!(value["frameId"], "main");
}

#[test]
fn test_full_cdp_roundtrip_runtime_evaluate() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(RuntimeDomain)).unwrap();
    let sender = NopSender;

    let raw = r#"{"id":100,"method":"Runtime.evaluate","params":{"expression":"document.title"}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap(), &sender);
    let value = result.unwrap().unwrap();
    assert_eq!(value["result"]["value"], "document.title");
}

#[test]
fn test_full_cdp_roundtrip_dom_getdocument() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(DomDomain)).unwrap();
    let sender = NopSender;

    let raw = r#"{"id":200,"method":"DOM.getDocument","params":{}}"#;
    let msg: CdpMessage = serde_json::from_str(raw).unwrap();
    let result = reg.dispatch_command(&msg.method, msg.params.unwrap_or_default(), &sender);
    let value = result.unwrap().unwrap();
    assert_eq!(value["root"]["nodeName"], "#document");
}
