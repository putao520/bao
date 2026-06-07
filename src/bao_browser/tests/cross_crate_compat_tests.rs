// @trace TEST-INTEGRATION-001 [req:REQ-CDP-001,REQ-STL-007,REQ-LIB-004] [level:integration]
// Cross-crate type compatibility and API consistency tests.
// Validates that types flow correctly across bao_browser → bao_cdp → cdp-server
// and bao_browser → bao_stealth boundaries.

use bao_browser::{
    BaoConfig, BrowserConfig, PageConfig, PageState, BrowserError,
    Permission, PermissionGuard, encode_image, ScreenshotFormat,
};
use bao_cdp::{
    CdpRouter, BackendKind, CDPServer, CDPServerError,
    bridge_channel, BridgeCommand, BridgeResponse,
};
use bao_stealth::StealthProfile;
use cdp_server::{
    CdpMessage, CdpError, SessionState,
    DomainRegistry, TargetInfo,
};
use serde_json::json;

// ---- BaoConfig ↔ StealthProfile cross-crate ----

#[test]
fn test_bao_config_with_stealth_profile() {
    let config = BaoConfig {
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    };
    assert!(config.stealth_profile.is_some());
    let profile = config.stealth_profile.as_ref().unwrap();
    assert!(!profile.navigator.user_agent.is_empty());
}

#[test]
fn test_bao_config_with_firefox_stealth() {
    let config = BaoConfig {
        stealth_profile: Some(StealthProfile::firefox_default()),
        ..Default::default()
    };
    let profile = config.stealth_profile.unwrap();
    assert!(!profile.tls.cipher_suites.is_empty());
    assert!(!profile.http2.settings_frame_payload().is_empty());
}

#[test]
fn test_browser_config_stealth_into_bao_config() {
    let mut bc = BrowserConfig::default();
    bc.stealth_profile = Some(StealthProfile::chrome_default());
    let config: BaoConfig = bc.into();
    assert!(config.stealth_profile.is_some());
}

// ---- PermissionGuard ↔ bao_cdp permission_bridge cross-crate ----

#[test]
fn test_permission_guard_allows_when_none() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_net("any.com").is_ok());
    assert!(guard.check_read("/any").is_ok());
}

#[test]
fn test_permission_guard_restricts_net() {
    let perm = Permission {
        net: Some(vec!["api.example.com".into()]),
        read: None,
        write: None,
        env: None,
        run: None,
    ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_net("api.example.com").is_ok());
    assert!(guard.check_net("evil.com").is_err());
}

#[test]
fn test_permission_default_is_unrestricted() {
    let perm = Permission::default();
    assert!(perm.net.is_none());
    assert!(perm.read.is_none());
    assert!(perm.write.is_none());
}

// ---- bao_cdp ↔ cdp-server type compatibility ----

#[test]
fn test_cdp_message_parses_in_bao_cdp_context() {
    let msg: CdpMessage = serde_json::from_str(
        r#"{"id":1,"method":"Page.navigate","params":{"url":"https://example.com"}}"#,
    ).unwrap();
    assert_eq!(msg.method, "Page.navigate");
    assert_eq!(msg.params.unwrap()["url"], "https://example.com");
}

#[test]
fn test_cdp_error_compatible_across_crates() {
    let err = CdpError { code: -32601, message: "Not found".into() };
    assert_eq!(err.code, -32601);
    let serialized = serde_json::to_string(&err).unwrap();
    assert!(serialized.contains("-32601"));
}

#[test]
fn test_session_state_from_cdp_server() {
    assert_ne!(SessionState::Created, SessionState::Active);
    assert_ne!(SessionState::Active, SessionState::Closed);
}

#[test]
fn test_target_info_cross_crate() {
    let info = TargetInfo {
        id: "test-id".into(),
        target_type: "page".into(),
        title: "Test".into(),
        url: "about:blank".into(),
        web_socket_debugger_url: "ws://127.0.0.1:9222/devtools/page/test-id".into(),
    };
    let serialized = serde_json::to_string(&info).unwrap();
    assert!(serialized.contains("test-id"));
    // TargetInfo is used by both cdp-server and bao_cdp transport
}

// ---- DomainRegistry from cdp-server used via bao_cdp ----

struct CrossDomain;
impl cdp_server::DomainHandler for CrossDomain {
    fn domain_name(&self) -> &'static str { "Cross" }
    fn handle_command(&self, cmd: &str, _params: serde_json::Value, _: &dyn cdp_server::EventSender) -> Result<serde_json::Value, CdpError> {
        match cmd {
            "Cross.ping" => Ok(json!({"pong": true})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' not found", cmd) }),
        }
    }
}

struct NopSender;
impl cdp_server::EventSender for NopSender {
    fn send_event(&self, _: &str, _: serde_json::Value) {}
}

#[test]
fn test_domain_registry_cross_crate_dispatch() {
    let reg = DomainRegistry::new();
    reg.register(Box::new(CrossDomain)).unwrap();
    let sender = NopSender;
    let result = reg.dispatch_command("Cross.ping", json!({}), &sender);
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());
}

// ---- CdpRouter ↔ cdp-server BackendKind ----

#[test]
fn test_router_internal_backend_kind() {
    let router = CdpRouter::new();
    let session = router.create_internal_session("target");
    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

#[test]
fn test_backend_kind_cross_crate_copy() {
    let kind = BackendKind::Internal;
    let kind2 = kind;
    assert_eq!(kind, kind2);
}

// ---- Bridge channel cross-crate ----

#[test]
fn test_bridge_command_variants() {
    let commands = vec![
        BridgeCommand::Navigate { url: "https://example.com".into() },
        BridgeCommand::EvaluateJs { expression: "1+1".into(), return_by_value: true },
        BridgeCommand::TakeScreenshot { format: "png".into(), quality: None },
        BridgeCommand::GetTitle,
        BridgeCommand::GetUrl,
        BridgeCommand::SetViewport { width: 1920, height: 1080, device_scale_factor: Some(2.0) },
        BridgeCommand::Reload { ignore_cache: false },
        BridgeCommand::StopLoading,
        BridgeCommand::ClosePage,
    ];
    // Verify all variants construct without panic
    for cmd in &commands {
        let debug = format!("{:?}", cmd);
        assert!(!debug.is_empty());
    }
}

#[test]
fn test_bridge_response_cross_crate() {
    let (sender, receiver) = bridge_channel(std::time::Duration::from_secs(1));
    sender.send_fire_and_forget(BridgeCommand::GetTitle);
    let processed = receiver.try_process(|cmd| {
        match cmd {
            BridgeCommand::GetTitle => BridgeResponse { result: Ok(json!("Test Title")) },
            _ => BridgeResponse { result: Err("unexpected".into()) },
        }
    });
    assert!(processed);
}

// ---- CDPServer ↔ StealthProfile ----

#[test]
fn test_cdp_server_with_stealth_bridge() {
    let (sender, _receiver) = bridge_channel(std::time::Duration::from_secs(5));
    let _server = CDPServer::with_bridge(19999, sender);
}

#[test]
fn test_cdp_server_error_types() {
    let errors = vec![
        CDPServerError::Bind("port busy".into()),
        CDPServerError::Io("read error".into()),
        CDPServerError::WebSocket("upgrade fail".into()),
        CDPServerError::Protocol("bad msg".into()),
    ];
    for err in &errors {
        let display = format!("{}", err);
        assert!(!display.is_empty());
    }
}

// ---- PageState ↔ BrowserError cross-crate consistency ----

#[test]
fn test_page_state_all_variants() {
    let states = [
        PageState::Created, PageState::Navigating,
        PageState::Interactive, PageState::Idle, PageState::Closed,
    ];
    // All unique
    for i in 0..states.len() {
        for j in (i+1)..states.len() {
            assert_ne!(states[i], states[j]);
        }
    }
}

#[test]
fn test_browser_error_cross_crate_display() {
    let errors = vec![
        BrowserError::Init("init fail".into()),
        BrowserError::Navigation("nav fail".into()),
        BrowserError::Rendering("render fail".into()),
        BrowserError::JavaScript("js fail".into()),
        BrowserError::CDP("cdp fail".into()),
    ];
    for err in &errors {
        let display = format!("{}", err);
        assert!(!display.is_empty());
    }
    // Verify std::error::Error trait
    let _: &dyn std::error::Error = &errors[0];
}

// ---- ScreenshotFormat cross-crate ----

#[test]
fn test_screenshot_png_cross_crate() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(4, 4, image::Rgba([255, 128, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert_eq!(&data[0..4], &[0x89, 0x50, 0x4E, 0x47]);
}

#[test]
fn test_screenshot_jpeg_cross_crate() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(4, 4, image::Rgba([100, 200, 50, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert_eq!(&data[0..2], &[0xFF, 0xD8]);
}

// ---- StealthProfile → bao_browser config → bao_cdp integration ----

#[test]
fn test_stealth_profile_propagation() {
    let profile = StealthProfile::chrome_default();
    let config = BaoConfig {
        stealth_profile: Some(profile),
        ..Default::default()
    };
    assert!(config.stealth_profile.is_some());
    let p = config.stealth_profile.as_ref().unwrap();
    assert!(!p.navigator.user_agent.is_empty());
    assert!(!p.tls.cipher_suites.is_empty());
    assert!(!p.canvas.noise_amplitude().is_nan());
    assert!(!p.webgl.vendor.is_empty());
}

#[test]
fn test_stealth_profile_firefox_propagation() {
    let profile = StealthProfile::firefox_default();
    let page_config = PageConfig {
        stealth_profile: Some(profile),
        ..Default::default()
    };
    assert!(page_config.stealth_profile.is_some());
    let p = page_config.stealth_profile.unwrap();
    assert!(!p.audio.noise_amplitude().is_nan());
    assert!(p.behavior.seed() > 0);
}
