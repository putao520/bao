// @trace TEST-BRW-015 [req:REQ-LIB-004] [level:unit]
// @trace TEST-BRW-016 [req:REQ-BRW-001] [level:unit]
// BaoConfig/PageConfig/BrowserConfig deep tests: validation, defaults,
// conversion, field coverage, Clone/Debug, PoolStats construction.

use bao_browser::{BaoConfig, BrowserConfig, PageConfig};
use std::time::Duration;

// ---- BaoConfig defaults ----

#[test]
fn test_bao_config_default_values() {
    let config = BaoConfig::default();
    assert!(config.cdp_port.is_none());
    assert_eq!(config.max_pages, 50);
    assert_eq!(config.idle_ttl, Duration::from_secs(60));
    assert_eq!(config.default_viewport_width, 1920);
    assert_eq!(config.default_viewport_height, 1080);
    assert!(config.stealth_profile.is_none());
}

#[test]
fn test_bao_config_validate_ok() {
    let config = BaoConfig::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_bao_config_validate_max_pages_zero() {
    let config = BaoConfig { max_pages: 0, ..Default::default() };
    let err = config.validate().unwrap_err();
    assert!(err.contains("max_pages"));
    assert!(err.contains("0"));
}

#[test]
fn test_bao_config_validate_max_pages_one() {
    let config = BaoConfig { max_pages: 1, ..Default::default() };
    assert!(config.validate().is_ok());
}

#[test]
fn test_bao_config_validate_viewport_width_too_small() {
    let config = BaoConfig { default_viewport_width: 799, ..Default::default() };
    let err = config.validate().unwrap_err();
    assert!(err.contains("viewport_width"));
    assert!(err.contains("799"));
}

#[test]
fn test_bao_config_validate_viewport_width_800() {
    let config = BaoConfig { default_viewport_width: 800, ..Default::default() };
    assert!(config.validate().is_ok());
}

#[test]
fn test_bao_config_validate_viewport_height_too_small() {
    let config = BaoConfig { default_viewport_height: 599, ..Default::default() };
    let err = config.validate().unwrap_err();
    assert!(err.contains("viewport_height"));
}

#[test]
fn test_bao_config_validate_viewport_height_600() {
    let config = BaoConfig { default_viewport_height: 600, ..Default::default() };
    assert!(config.validate().is_ok());
}

#[test]
fn test_bao_config_validate_large_values() {
    let config = BaoConfig {
        max_pages: 10000,
        default_viewport_width: 7680,
        default_viewport_height: 4320,
        ..Default::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn test_bao_config_clone() {
    let config = BaoConfig {
        cdp_port: Some(9222),
        max_pages: 10,
        ..Default::default()
    };
    let cloned = config.clone();
    assert_eq!(cloned.cdp_port, Some(9222));
    assert_eq!(cloned.max_pages, 10);
}

#[test]
fn test_bao_config_debug() {
    let config = BaoConfig::default();
    let debug = format!("{:?}", config);
    assert!(debug.contains("max_pages"));
    assert!(debug.contains("cdp_port"));
}

// ---- PageConfig ----

#[test]
fn test_page_config_default() {
    let config = PageConfig::default();
    assert!(config.url.is_none());
    assert!(config.viewport_width.is_none());
    assert!(config.viewport_height.is_none());
    assert!(config.stealth_profile.is_none());
    assert!(config.permission.is_none());
}

#[test]
fn test_page_config_with_url() {
    let config = PageConfig {
        url: Some("https://example.com".into()),
        ..Default::default()
    };
    assert_eq!(config.url.as_deref(), Some("https://example.com"));
}

#[test]
fn test_page_config_with_viewport() {
    let config = PageConfig {
        viewport_width: Some(1280),
        viewport_height: Some(720),
        ..Default::default()
    };
    assert_eq!(config.viewport_width, Some(1280));
    assert_eq!(config.viewport_height, Some(720));
}

#[test]
fn test_page_config_clone() {
    let config = PageConfig {
        url: Some("https://test.com".into()),
        ..Default::default()
    };
    let cloned = config.clone();
    assert_eq!(cloned.url, config.url);
}

#[test]
fn test_page_config_debug() {
    let config = PageConfig { url: Some("test".into()), ..Default::default() };
    let debug = format!("{:?}", config);
    assert!(debug.contains("url"));
}

// ---- BrowserConfig ----

#[test]
fn test_browser_config_default() {
    let config = BrowserConfig::default();
    assert!(config.url.is_none());
    assert_eq!(config.cdp_port, 9222);
    assert_eq!(config.viewport_width, 1920);
    assert_eq!(config.viewport_height, 1080);
    assert!(config.headless);
    assert!(config.stealth_profile.is_none());
}

#[test]
fn test_browser_config_custom() {
    let config = BrowserConfig {
        url: Some("https://example.com".into()),
        cdp_port: 8080,
        viewport_width: 1280,
        viewport_height: 720,
        headless: false,
        stealth_profile: None,
    };
    assert_eq!(config.cdp_port, 8080);
    assert!(!config.headless);
}

#[test]
fn test_browser_config_clone() {
    let config = BrowserConfig {
        url: Some("https://example.com".into()),
        cdp_port: 3000,
        ..Default::default()
    };
    let cloned = config.clone();
    assert_eq!(cloned.cdp_port, 3000);
    assert_eq!(cloned.url, config.url);
}

#[test]
fn test_browser_config_debug() {
    let config = BrowserConfig { cdp_port: 9999, ..Default::default() };
    let debug = format!("{:?}", config);
    assert!(debug.contains("9999"));
}

// ---- BrowserConfig → BaoConfig conversion ----

#[test]
fn test_browser_config_to_bao_config() {
    let bc = BrowserConfig {
        url: Some("https://example.com".into()),
        cdp_port: 9333,
        viewport_width: 1280,
        viewport_height: 720,
        headless: true,
        stealth_profile: None,
    };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(9333));
    assert_eq!(bao.default_viewport_width, 1280);
    assert_eq!(bao.default_viewport_height, 720);
    assert_eq!(bao.max_pages, 50);
    assert_eq!(bao.idle_ttl, Duration::from_secs(60));
}

#[test]
fn test_browser_config_to_bao_preserves_defaults() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(9222));
    assert_eq!(bao.default_viewport_width, 1920);
    assert_eq!(bao.default_viewport_height, 1080);
}

// ---- Multiple validations ----

#[test]
fn test_validate_all_fields_invalid() {
    let config = BaoConfig {
        max_pages: 0,
        default_viewport_width: 100,
        default_viewport_height: 100,
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    // Should catch first error (max_pages)
    assert!(err.contains("max_pages"));
}

#[test]
fn test_validate_only_height_invalid() {
    let config = BaoConfig {
        default_viewport_height: 500,
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("viewport_height"));
}

// ---- PageState ----

use bao_browser::PageState;

#[test]
fn test_page_state_variants() {
    let states = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    let names: Vec<String> = states.iter().map(|s| format!("{:?}", s)).collect();
    assert!(names[0].contains("Created"));
    assert!(names[1].contains("Navigating"));
    assert!(names[2].contains("Interactive"));
    assert!(names[3].contains("Idle"));
    assert!(names[4].contains("Closed"));
}

#[test]
fn test_page_state_equality() {
    assert_eq!(PageState::Created, PageState::Created);
    assert_ne!(PageState::Created, PageState::Navigating);
    assert_ne!(PageState::Interactive, PageState::Idle);
    assert_ne!(PageState::Idle, PageState::Closed);
}

#[test]
fn test_page_state_clone() {
    let state = PageState::Interactive;
    let cloned = state.clone();
    assert_eq!(cloned, PageState::Interactive);
}

#[test]
fn test_page_state_copy() {
    let state = PageState::Navigating;
    let copied = state;
    assert_eq!(copied, PageState::Navigating);
}
