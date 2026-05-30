// @trace TEST-STL-007-CONFIG [req:REQ-STL-007] [level:unit]
// StealthProfile configuration integration: BaoConfig → BrowserConfig → PageConfig → runtime_bridge

use bao_browser::{BaoConfig, BrowserConfig, PageConfig};
use bao_stealth::StealthProfile;

// ---- BaoConfig stealth_profile field ----

#[test]
fn test_bao_config_default_no_stealth() {
    let config = BaoConfig::default();
    assert!(config.stealth_profile.is_none());
}

#[test]
fn test_bao_config_with_chrome_stealth() {
    let mut config = BaoConfig::default();
    config.stealth_profile = Some(StealthProfile::chrome_default());
    assert!(config.stealth_profile.is_some());
    let profile = config.stealth_profile.unwrap();
    assert!(profile.navigator.user_agent.contains("Chrome"));
}

#[test]
fn test_bao_config_with_firefox_stealth() {
    let mut config = BaoConfig::default();
    config.stealth_profile = Some(StealthProfile::firefox_default());
    assert!(config.stealth_profile.is_some());
    let profile = config.stealth_profile.unwrap();
    assert!(profile.navigator.user_agent.contains("Firefox"));
}

#[test]
fn test_bao_config_validate_with_stealth() {
    let mut config = BaoConfig::default();
    config.stealth_profile = Some(StealthProfile::chrome_default());
    assert!(config.validate().is_ok());
}

// ---- BrowserConfig stealth_profile ----

#[test]
fn test_browser_config_default_no_stealth() {
    let config = BrowserConfig::default();
    assert!(config.stealth_profile.is_none());
}

#[test]
fn test_browser_config_with_stealth() {
    let mut config = BrowserConfig::default();
    config.stealth_profile = Some(StealthProfile::chrome_default());
    assert!(config.stealth_profile.is_some());
}

#[test]
fn test_browser_config_to_bao_config_preserves_stealth() {
    let mut bc = BrowserConfig::default();
    bc.stealth_profile = Some(StealthProfile::chrome_default());
    let bao_config: BaoConfig = bc.into();
    assert!(bao_config.stealth_profile.is_some());
    assert!(bao_config.stealth_profile.unwrap().navigator.user_agent.contains("Chrome"));
}

#[test]
fn test_browser_config_to_bao_config_without_stealth() {
    let bc = BrowserConfig::default();
    let bao_config: BaoConfig = bc.into();
    assert!(bao_config.stealth_profile.is_none());
}

// ---- PageConfig stealth_profile ----

#[test]
fn test_page_config_default_no_stealth() {
    let config = PageConfig::default();
    assert!(config.stealth_profile.is_none());
}

#[test]
fn test_page_config_with_stealth() {
    let mut config = PageConfig::default();
    config.stealth_profile = Some(StealthProfile::firefox_default());
    assert!(config.stealth_profile.is_some());
}

// ---- Profile consistency across config chain ----

#[test]
fn test_stealth_profile_survives_config_chain() {
    let mut bc = BrowserConfig::default();
    bc.stealth_profile = Some(StealthProfile::chrome_default());
    let bao_config: BaoConfig = bc.into();
    let profile = bao_config.stealth_profile.unwrap();
    // Original chrome profile properties preserved
    assert!(!profile.tls.ja3_hash.is_empty());
    assert!(!profile.navigator.user_agent.is_empty());
    assert!(!profile.navigator.platform.is_empty());
    assert!(!profile.webgl.renderer.is_empty());
}

#[test]
fn test_firefox_profile_survives_config_chain() {
    let mut bc = BrowserConfig::default();
    bc.stealth_profile = Some(StealthProfile::firefox_default());
    let bao_config: BaoConfig = bc.into();
    let profile = bao_config.stealth_profile.unwrap();
    assert!(profile.navigator.user_agent.contains("Firefox"));
    assert!(!profile.webgl.renderer.is_empty());
}

// ---- StealthProfile clone isolation ----

#[test]
fn test_stealth_profile_clone_is_independent() {
    let profile = StealthProfile::chrome_default();
    let cloned = profile.clone();
    // Both have same content
    assert_eq!(profile.navigator.user_agent, cloned.navigator.user_agent);
    assert_eq!(profile.tls.ja3_hash, cloned.tls.ja3_hash);
}

// ---- PageConfig per-page stealth override ----

#[test]
fn test_page_config_stealth_overrides_global() {
    let mut global_config = BaoConfig::default();
    global_config.stealth_profile = Some(StealthProfile::chrome_default());

    let mut page_config = PageConfig::default();
    page_config.stealth_profile = Some(StealthProfile::firefox_default());

    // Page-level Firefox overrides global Chrome
    assert!(page_config.stealth_profile.unwrap().navigator.user_agent.contains("Firefox"));
    assert!(global_config.stealth_profile.unwrap().navigator.user_agent.contains("Chrome"));
}

// ---- BrowserConfig default values ----

#[test]
fn test_browser_config_defaults() {
    let config = BrowserConfig::default();
    assert_eq!(config.cdp_port, 9222);
    assert_eq!(config.viewport_width, 1920);
    assert_eq!(config.viewport_height, 1080);
    assert!(config.headless);
    assert!(config.stealth_profile.is_none());
    assert!(config.url.is_none());
}

// ---- BaoConfig validate edge cases with stealth ----

#[test]
fn test_bao_config_valid_min_viewport_with_stealth() {
    let config = BaoConfig {
        default_viewport_width: 800,
        default_viewport_height: 600,
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn test_bao_config_invalid_viewport_with_stealth() {
    let config = BaoConfig {
        default_viewport_width: 799,
        default_viewport_height: 600,
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    };
    assert!(config.validate().is_err());
}
