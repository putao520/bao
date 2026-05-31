// @trace TEST-BRW-025 [req:REQ-BRW-001,REQ-BRW-002,REQ-LIB-003] [level:unit]
// BaoConfig validate, BrowserConfig Default/From, PageConfig Default,
// PageState enum exhaustiveness, BrowserError Display/Error,
// ScreenshotFormat enum, BaoWebViewState Default, BaoServoDelegate new/last_error.

use bao_browser::{BaoConfig, BrowserConfig, PageConfig, PageState, BrowserError, ScreenshotFormat};
use std::time::Duration;

// ---- BaoConfig default ----

#[test]
fn test_bao_config_default_values() {
    let cfg = BaoConfig::default();
    assert!(cfg.cdp_port.is_none());
    assert_eq!(cfg.max_pages, 50);
    assert_eq!(cfg.idle_ttl, Duration::from_secs(60));
    assert_eq!(cfg.default_viewport_width, 1920);
    assert_eq!(cfg.default_viewport_height, 1080);
    assert!(cfg.stealth_profile.is_none());
}

#[test]
fn test_bao_config_debug() {
    let cfg = BaoConfig::default();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("BaoConfig"));
    assert!(debug.contains("1920"));
}

#[test]
fn test_bao_config_clone() {
    let cfg = BaoConfig::default();
    let cloned = cfg.clone();
    assert_eq!(cloned.max_pages, cfg.max_pages);
    assert_eq!(cloned.default_viewport_width, cfg.default_viewport_width);
}

// ---- BaoConfig validate ----

#[test]
fn test_validate_default_ok() {
    assert!(BaoConfig::default().validate().is_ok());
}

#[test]
fn test_validate_zero_max_pages() {
    let cfg = BaoConfig { max_pages: 0, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("max_pages must be >= 1"));
    assert!(err.contains("0"));
}

#[test]
fn test_validate_one_max_pages() {
    let cfg = BaoConfig { max_pages: 1, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_large_max_pages() {
    let cfg = BaoConfig { max_pages: 10000, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_viewport_width_too_small() {
    let cfg = BaoConfig { default_viewport_width: 799, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("viewport_width must be >= 800"));
    assert!(err.contains("799"));
}

#[test]
fn test_validate_viewport_width_800_ok() {
    let cfg = BaoConfig { default_viewport_width: 800, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_viewport_height_too_small() {
    let cfg = BaoConfig { default_viewport_height: 599, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("viewport_height must be >= 600"));
    assert!(err.contains("599"));
}

#[test]
fn test_validate_viewport_height_600_ok() {
    let cfg = BaoConfig { default_viewport_height: 600, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_multiple_errors_returns_first() {
    let cfg = BaoConfig {
        max_pages: 0,
        default_viewport_width: 100,
        default_viewport_height: 100,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    // Should return the first error (max_pages)
    assert!(err.contains("max_pages must be >= 1"));
}

#[test]
fn test_validate_custom_ok() {
    let cfg = BaoConfig {
        max_pages: 10,
        default_viewport_width: 1280,
        default_viewport_height: 720,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_with_cdp_port() {
    let cfg = BaoConfig { cdp_port: Some(9222), ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_with_stealth_profile() {
    let cfg = BaoConfig {
        stealth_profile: Some(bao_stealth::StealthProfile::firefox_default()),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

// ---- PageConfig default ----

#[test]
fn test_page_config_default() {
    let cfg = PageConfig::default();
    assert!(cfg.url.is_none());
    assert!(cfg.viewport_width.is_none());
    assert!(cfg.viewport_height.is_none());
    assert!(cfg.stealth_profile.is_none());
    assert!(cfg.permission.is_none());
}

#[test]
fn test_page_config_debug() {
    let cfg = PageConfig::default();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("PageConfig"));
}

#[test]
fn test_page_config_clone() {
    let cfg = PageConfig {
        url: Some("https://example.com".into()),
        viewport_width: Some(1280),
        viewport_height: Some(720),
        stealth_profile: None,
        permission: None,
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.url, cfg.url);
    assert_eq!(cloned.viewport_width, cfg.viewport_width);
}

#[test]
fn test_page_config_with_url() {
    let cfg = PageConfig {
        url: Some("https://test.com".into()),
        ..Default::default()
    };
    assert_eq!(cfg.url.as_deref(), Some("https://test.com"));
}

#[test]
fn test_page_config_with_permission() {
    let perm = bao_browser::Permission {
        read: Some(vec!["/safe".into()]),
        ..Default::default()
    };
    let cfg = PageConfig {
        permission: Some(perm),
        ..Default::default()
    };
    assert!(cfg.permission.is_some());
}

// ---- BrowserConfig default ----

#[test]
fn test_browser_config_default_values() {
    let cfg = BrowserConfig::default();
    assert!(cfg.url.is_none());
    assert_eq!(cfg.cdp_port, 9222);
    assert_eq!(cfg.viewport_width, 1920);
    assert_eq!(cfg.viewport_height, 1080);
    assert!(cfg.headless);
    assert!(cfg.stealth_profile.is_none());
}

#[test]
fn test_browser_config_debug() {
    let cfg = BrowserConfig::default();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("BrowserConfig"));
    assert!(debug.contains("9222"));
}

#[test]
fn test_browser_config_clone() {
    let cfg = BrowserConfig::default();
    let cloned = cfg.clone();
    assert_eq!(cloned.cdp_port, cfg.cdp_port);
    assert_eq!(cloned.headless, cfg.headless);
}

#[test]
fn test_browser_config_custom() {
    let cfg = BrowserConfig {
        url: Some("https://example.com".into()),
        cdp_port: 8080,
        viewport_width: 1280,
        viewport_height: 720,
        headless: false,
        stealth_profile: Some(bao_stealth::StealthProfile::chrome_default()),
    };
    assert_eq!(cfg.url.as_deref(), Some("https://example.com"));
    assert_eq!(cfg.cdp_port, 8080);
    assert_eq!(cfg.viewport_width, 1280);
    assert!(!cfg.headless);
    assert!(cfg.stealth_profile.is_some());
}

// ---- BrowserConfig → BaoConfig From ----

#[test]
fn test_from_browser_config() {
    let bc = BrowserConfig {
        url: Some("https://test.com".into()),
        cdp_port: 3000,
        viewport_width: 1280,
        viewport_height: 720,
        headless: true,
        stealth_profile: None,
    };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(3000));
    assert_eq!(bao.max_pages, 50);
    assert_eq!(bao.default_viewport_width, 1280);
    assert_eq!(bao.default_viewport_height, 720);
    assert!(bao.stealth_profile.is_none());
}

#[test]
fn test_from_browser_config_with_stealth() {
    let bc = BrowserConfig {
        stealth_profile: Some(bao_stealth::StealthProfile::firefox_default()),
        ..Default::default()
    };
    let bao: BaoConfig = bc.into();
    assert!(bao.stealth_profile.is_some());
}

#[test]
fn test_from_browser_config_preserves_viewport() {
    let bc = BrowserConfig {
        viewport_width: 3840,
        viewport_height: 2160,
        ..Default::default()
    };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.default_viewport_width, 3840);
    assert_eq!(bao.default_viewport_height, 2160);
}

// ---- PageState enum ----

#[test]
fn test_page_state_variants() {
    let states = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    // Verify all variants are constructible and distinguishable
    assert_eq!(states[0], PageState::Created);
    assert_eq!(states[1], PageState::Navigating);
    assert_eq!(states[2], PageState::Interactive);
    assert_eq!(states[3], PageState::Idle);
    assert_eq!(states[4], PageState::Closed);
}

#[test]
fn test_page_state_copy() {
    let s1 = PageState::Created;
    let s2 = s1;
    assert_eq!(s1, s2);
}

#[test]
fn test_page_state_equality() {
    assert_eq!(PageState::Created, PageState::Created);
    assert_ne!(PageState::Created, PageState::Closed);
    assert_ne!(PageState::Navigating, PageState::Interactive);
}

#[test]
fn test_page_state_debug() {
    assert!(format!("{:?}", PageState::Created).contains("Created"));
    assert!(format!("{:?}", PageState::Navigating).contains("Navigating"));
    assert!(format!("{:?}", PageState::Interactive).contains("Interactive"));
    assert!(format!("{:?}", PageState::Idle).contains("Idle"));
    assert!(format!("{:?}", PageState::Closed).contains("Closed"));
}

#[test]
fn test_page_state_ordering() {
    // Verify Copy + Clone + PartialEq + Eq + Debug
    assert_ne!(PageState::Created, PageState::Closed);
    assert_ne!(PageState::Idle, PageState::Navigating);
}

// ---- BrowserError ----

#[test]
fn test_browser_error_init() {
    let err = BrowserError::Init("test error".into());
    let msg = format!("{}", err);
    assert!(msg.contains("browser init error"));
    assert!(msg.contains("test error"));
}

#[test]
fn test_browser_error_navigation() {
    let err = BrowserError::Navigation("bad url".into());
    assert!(format!("{}", err).contains("navigation error"));
    assert!(format!("{}", err).contains("bad url"));
}

#[test]
fn test_browser_error_rendering() {
    let err = BrowserError::Rendering("gpu fail".into());
    assert!(format!("{}", err).contains("rendering error"));
}

#[test]
fn test_browser_error_javascript() {
    let err = BrowserError::JavaScript("syntax error".into());
    assert!(format!("{}", err).contains("javascript error"));
}

#[test]
fn test_browser_error_cdp() {
    let err = BrowserError::CDP("ws closed".into());
    assert!(format!("{}", err).contains("cdp error"));
}

#[test]
fn test_browser_error_is_std_error() {
    let err = BrowserError::Init("test".into());
    let _: Box<dyn std::error::Error> = Box::new(err);
}

#[test]
fn test_browser_error_debug() {
    let err = BrowserError::Navigation("x".into());
    let debug = format!("{:?}", err);
    assert!(debug.contains("Navigation"));
}

// ---- ScreenshotFormat ----

#[test]
fn test_screenshot_format_variants() {
    // Verify both variants are constructible
    let _png = ScreenshotFormat::Png;
    let _jpeg = ScreenshotFormat::Jpeg;
}
