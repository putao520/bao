// @trace TEST-BRW-023 [req:REQ-BRW-001,REQ-BRW-002] [level:unit]
// BaoConfig validate edge cases, BrowserConfig defaults + From conversion,
// PageConfig field completeness.

use std::time::Duration;

use bao_browser::{BaoConfig, BrowserConfig, PageConfig, Permission};

// ---- BaoConfig defaults ----

#[test]
fn test_bao_config_default_cdp_port_none() {
    let cfg = BaoConfig::default();
    assert!(cfg.cdp_port.is_none());
}

#[test]
fn test_bao_config_default_max_pages() {
    assert_eq!(BaoConfig::default().max_pages, 50);
}

#[test]
fn test_bao_config_default_idle_ttl() {
    assert_eq!(BaoConfig::default().idle_ttl, Duration::from_secs(60));
}

#[test]
fn test_bao_config_default_viewport() {
    let cfg = BaoConfig::default();
    assert_eq!(cfg.default_viewport_width, 1920);
    assert_eq!(cfg.default_viewport_height, 1080);
}

#[test]
fn test_bao_config_default_stealth_none() {
    assert!(BaoConfig::default().stealth_profile.is_none());
}

// ---- BaoConfig validate ----

#[test]
fn test_validate_ok_defaults() {
    assert!(BaoConfig::default().validate().is_ok());
}

#[test]
fn test_validate_max_pages_zero() {
    let cfg = BaoConfig { max_pages: 0, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("max_pages"));
    assert!(err.contains("0"));
}

#[test]
fn test_validate_max_pages_one() {
    let cfg = BaoConfig { max_pages: 1, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_viewport_width_below_800() {
    let cfg = BaoConfig { default_viewport_width: 799, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("viewport_width"));
    assert!(err.contains("799"));
}

#[test]
fn test_validate_viewport_width_800() {
    let cfg = BaoConfig { default_viewport_width: 800, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_viewport_height_below_600() {
    let cfg = BaoConfig { default_viewport_height: 599, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("viewport_height"));
    assert!(err.contains("599"));
}

#[test]
fn test_validate_viewport_height_600() {
    let cfg = BaoConfig { default_viewport_height: 600, ..Default::default() };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_validate_max_pages_first_check() {
    // max_pages=0 catches before viewport checks
    let cfg = BaoConfig { max_pages: 0, default_viewport_width: 100, default_viewport_height: 100, ..Default::default() };
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("max_pages"));
}

#[test]
fn test_validate_large_values() {
    let cfg = BaoConfig {
        max_pages: usize::MAX,
        default_viewport_width: u32::MAX,
        default_viewport_height: u32::MAX,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_bao_config_debug() {
    let cfg = BaoConfig::default();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("50"));
    assert!(debug.contains("1920"));
}

#[test]
fn test_bao_config_clone() {
    let cfg = BaoConfig { max_pages: 42, ..Default::default() };
    let cloned = cfg.clone();
    assert_eq!(cloned.max_pages, 42);
    assert_eq!(cloned.default_viewport_width, cfg.default_viewport_width);
}

// ---- BrowserConfig defaults ----

#[test]
fn test_browser_config_default_url_none() {
    assert!(BrowserConfig::default().url.is_none());
}

#[test]
fn test_browser_config_default_cdp_port() {
    assert_eq!(BrowserConfig::default().cdp_port, 9222);
}

#[test]
fn test_browser_config_default_viewport() {
    let cfg = BrowserConfig::default();
    assert_eq!(cfg.viewport_width, 1920);
    assert_eq!(cfg.viewport_height, 1080);
}

#[test]
fn test_browser_config_default_headless() {
    assert!(BrowserConfig::default().headless);
}

#[test]
fn test_browser_config_default_stealth_none() {
    assert!(BrowserConfig::default().stealth_profile.is_none());
}

#[test]
fn test_browser_config_debug() {
    let cfg = BrowserConfig::default();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("9222"));
    assert!(debug.contains("headless"));
}

#[test]
fn test_browser_config_clone() {
    let cfg = BrowserConfig { url: Some("http://test".into()), cdp_port: 8080, ..Default::default() };
    let cloned = cfg.clone();
    assert_eq!(cloned.url, cfg.url);
    assert_eq!(cloned.cdp_port, 8080);
}

// ---- BrowserConfig → BaoConfig conversion ----

#[test]
fn test_from_browser_config_cdp_port() {
    let bc = BrowserConfig { cdp_port: 9333, ..Default::default() };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(9333));
}

#[test]
fn test_from_browser_config_max_pages_default() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.max_pages, 50);
}

#[test]
fn test_from_browser_config_idle_ttl() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.idle_ttl, Duration::from_secs(60));
}

#[test]
fn test_from_browser_config_viewport() {
    let bc = BrowserConfig { viewport_width: 1280, viewport_height: 720, ..Default::default() };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.default_viewport_width, 1280);
    assert_eq!(bao.default_viewport_height, 720);
}

#[test]
fn test_from_browser_config_stealth_none() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert!(bao.stealth_profile.is_none());
}

#[test]
fn test_from_browser_config_stealth_some() {
    let bc = BrowserConfig {
        stealth_profile: Some(bao_stealth::StealthProfile::firefox_default()),
        ..Default::default()
    };
    let bao: BaoConfig = bc.into();
    assert!(bao.stealth_profile.is_some());
    assert!(bao.stealth_profile.unwrap().navigator.user_agent.contains("Firefox"));
}

#[test]
fn test_from_preserves_all_fields() {
    let bc = BrowserConfig {
        url: Some("http://example.com".into()),
        cdp_port: 9999,
        viewport_width: 2560,
        viewport_height: 1440,
        headless: false,
        stealth_profile: None,
    };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(9999));
    assert_eq!(bao.default_viewport_width, 2560);
    assert_eq!(bao.default_viewport_height, 1440);
}

// ---- PageConfig ----

#[test]
fn test_page_config_default_all_none() {
    let cfg = PageConfig::default();
    assert!(cfg.url.is_none());
    assert!(cfg.viewport_width.is_none());
    assert!(cfg.viewport_height.is_none());
    assert!(cfg.stealth_profile.is_none());
    assert!(cfg.permission.is_none());
}

#[test]
fn test_page_config_with_url() {
    let cfg = PageConfig { url: Some("http://test".into()), ..Default::default() };
    assert_eq!(cfg.url.as_deref(), Some("http://test"));
}

#[test]
fn test_page_config_with_viewport() {
    let cfg = PageConfig { viewport_width: Some(1280), viewport_height: Some(720), ..Default::default() };
    assert_eq!(cfg.viewport_width, Some(1280));
    assert_eq!(cfg.viewport_height, Some(720));
}

#[test]
fn test_page_config_with_permission() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    let cfg = PageConfig { permission: Some(perm), ..Default::default() };
    assert!(cfg.permission.is_some());
    let p = cfg.permission.unwrap();
    assert!(p.read.is_some());
}

#[test]
fn test_page_config_debug() {
    let cfg = PageConfig { url: Some("http://debug".into()), ..Default::default() };
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("http://debug"));
}

#[test]
fn test_page_config_clone() {
    let cfg = PageConfig { url: Some("http://clone".into()), ..Default::default() };
    let cloned = cfg.clone();
    assert_eq!(cloned.url, cfg.url);
}

#[test]
fn test_page_config_stealth_profile() {
    let cfg = PageConfig {
        stealth_profile: Some(bao_stealth::StealthProfile::chrome_default()),
        ..Default::default()
    };
    assert!(cfg.stealth_profile.is_some());
}
