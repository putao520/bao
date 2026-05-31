// @trace TEST-BRW-021 [req:REQ-BRW-001,REQ-CDP-008] [level:unit]
// BaoConfig, PageConfig, BrowserConfig deep tests:
// default values, validation boundaries, From<BrowserConfig> conversion,
// clone/debug, edge cases.

use bao_browser::{BaoConfig, PageConfig, BrowserConfig};
use std::time::Duration;

// ---- BaoConfig defaults ----

#[test]
fn test_bao_config_default_cdp_port_none() {
    let cfg = BaoConfig::default();
    assert!(cfg.cdp_port.is_none());
}

#[test]
fn test_bao_config_default_max_pages() {
    let cfg = BaoConfig::default();
    assert_eq!(cfg.max_pages, 50);
}

#[test]
fn test_bao_config_default_idle_ttl() {
    let cfg = BaoConfig::default();
    assert_eq!(cfg.idle_ttl, Duration::from_secs(60));
}

#[test]
fn test_bao_config_default_viewport() {
    let cfg = BaoConfig::default();
    assert_eq!(cfg.default_viewport_width, 1920);
    assert_eq!(cfg.default_viewport_height, 1080);
}

#[test]
fn test_bao_config_default_stealth_none() {
    let cfg = BaoConfig::default();
    assert!(cfg.stealth_profile.is_none());
}

// ---- BaoConfig::validate() ----

#[test]
fn test_bao_config_validate_ok() {
    let cfg = BaoConfig::default();
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_bao_config_validate_max_pages_zero() {
    let cfg = BaoConfig { max_pages: 0, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("max_pages"));
    assert!(err.contains("0"));
}

#[test]
fn test_bao_config_validate_viewport_width_below_800() {
    let cfg = BaoConfig { default_viewport_width: 799, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("viewport_width"));
    assert!(err.contains("799"));
}

#[test]
fn test_bao_config_validate_viewport_width_800() {
    let cfg = BaoConfig { default_viewport_width: 800, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_bao_config_validate_viewport_height_below_600() {
    let cfg = BaoConfig { default_viewport_height: 599, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("viewport_height"));
    assert!(err.contains("599"));
}

#[test]
fn test_bao_config_validate_viewport_height_600() {
    let cfg = BaoConfig { default_viewport_height: 600, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_bao_config_validate_max_pages_1() {
    let cfg = BaoConfig { max_pages: 1, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_bao_config_validate_large_viewport() {
    let cfg = BaoConfig {
        default_viewport_width: 7680,
        default_viewport_height: 4320,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_bao_config_validate_cdp_port_set() {
    let cfg = BaoConfig { cdp_port: Some(9222), ..Default::default() };
    assert!(cfg.validate().is_ok());
}

// ---- BaoConfig clone/debug ----

#[test]
fn test_bao_config_clone() {
    let cfg = BaoConfig { cdp_port: Some(8080), max_pages: 10, ..Default::default() };
    let cloned = cfg.clone();
    assert_eq!(cloned.cdp_port, cfg.cdp_port);
    assert_eq!(cloned.max_pages, cfg.max_pages);
}

#[test]
fn test_bao_config_debug() {
    let cfg = BaoConfig::default();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("max_pages") || debug.contains("BaoConfig"));
}

// ---- PageConfig ----

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
fn test_page_config_custom_url() {
    let cfg = PageConfig {
        url: Some("https://example.com".into()),
        ..Default::default()
    };
    assert_eq!(cfg.url.as_deref(), Some("https://example.com"));
}

#[test]
fn test_page_config_custom_viewport() {
    let cfg = PageConfig {
        viewport_width: Some(2560),
        viewport_height: Some(1440),
        ..Default::default()
    };
    assert_eq!(cfg.viewport_width, Some(2560));
    assert_eq!(cfg.viewport_height, Some(1440));
}

#[test]
fn test_page_config_clone() {
    let cfg = PageConfig {
        url: Some("http://test".into()),
        ..Default::default()
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.url, cfg.url);
}

#[test]
fn test_page_config_debug() {
    let cfg = PageConfig {
        url: Some("http://debug".into()),
        ..Default::default()
    };
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("debug") || debug.contains("PageConfig"));
}

// ---- BrowserConfig defaults ----

#[test]
fn test_browser_config_default_url_none() {
    let cfg = BrowserConfig::default();
    assert!(cfg.url.is_none());
}

#[test]
fn test_browser_config_default_cdp_port() {
    let cfg = BrowserConfig::default();
    assert_eq!(cfg.cdp_port, 9222);
}

#[test]
fn test_browser_config_default_viewport() {
    let cfg = BrowserConfig::default();
    assert_eq!(cfg.viewport_width, 1920);
    assert_eq!(cfg.viewport_height, 1080);
}

#[test]
fn test_browser_config_default_headless() {
    let cfg = BrowserConfig::default();
    assert!(cfg.headless);
}

#[test]
fn test_browser_config_default_stealth_none() {
    let cfg = BrowserConfig::default();
    assert!(cfg.stealth_profile.is_none());
}

// ---- BrowserConfig custom ----

#[test]
fn test_browser_config_custom() {
    let cfg = BrowserConfig {
        url: Some("https://test.com".into()),
        cdp_port: 8080,
        viewport_width: 1280,
        viewport_height: 720,
        headless: false,
        stealth_profile: None,
    };
    assert_eq!(cfg.url.as_deref(), Some("https://test.com"));
    assert_eq!(cfg.cdp_port, 8080);
    assert!(!cfg.headless);
}

#[test]
fn test_browser_config_clone() {
    let cfg = BrowserConfig {
        url: Some("http://clone".into()),
        ..Default::default()
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.url, cfg.url);
    assert_eq!(cloned.cdp_port, cfg.cdp_port);
}

#[test]
fn test_browser_config_debug() {
    let cfg = BrowserConfig::default();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("headless") || debug.contains("BrowserConfig"));
}

// ---- From<BrowserConfig> for BaoConfig ----

#[test]
fn test_from_browser_config_port() {
    let bc = BrowserConfig { cdp_port: 9999, ..Default::default() };
    let ac: BaoConfig = bc.into();
    assert_eq!(ac.cdp_port, Some(9999));
}

#[test]
fn test_from_browser_config_viewport() {
    let bc = BrowserConfig {
        viewport_width: 2560,
        viewport_height: 1440,
        ..Default::default()
    };
    let ac: BaoConfig = bc.into();
    assert_eq!(ac.default_viewport_width, 2560);
    assert_eq!(ac.default_viewport_height, 1440);
}

#[test]
fn test_from_browser_config_preserves_defaults() {
    let bc = BrowserConfig::default();
    let ac: BaoConfig = bc.into();
    assert_eq!(ac.max_pages, 50);
    assert_eq!(ac.idle_ttl, Duration::from_secs(60));
}

#[test]
fn test_from_browser_config_stealth_propagates() {
    let bc = BrowserConfig {
        stealth_profile: Some(bao_stealth::StealthProfile::firefox_default()),
        ..Default::default()
    };
    let ac: BaoConfig = bc.into();
    assert!(ac.stealth_profile.is_some());
}

// ---- Edge: boundary port values ----

#[test]
fn test_browser_config_port_zero() {
    let cfg = BrowserConfig { cdp_port: 0, ..Default::default() };
    assert_eq!(cfg.cdp_port, 0);
}

#[test]
fn test_browser_config_port_max() {
    let cfg = BrowserConfig { cdp_port: 65535, ..Default::default() };
    assert_eq!(cfg.cdp_port, 65535);
}
