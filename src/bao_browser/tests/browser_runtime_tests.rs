// @trace TEST-BRW-002 [req:REQ-BRW-002] [level:unit]
// @trace TEST-BRW-003 [req:REQ-BRW-003] [level:unit]
// @trace TEST-LIB-001 [req:REQ-LIB-001] [level:unit]
// @trace TEST-LIB-003 [req:REQ-LIB-003] [level:unit]
// Unit tests for browser rendering, runtime bridge, page pool, CDP abstraction

use bao_browser::{BaoConfig, BrowserConfig, PageConfig, BrowserError};
use bao_browser::{Permission, PermissionGuard};
use bao_stealth::StealthProfile;

#[test]
fn test_bao_config_with_stealth_profile() {
    let profile = StealthProfile::chrome_default();
    let mut config = BaoConfig::default();
    config.stealth_profile = Some(profile);
    assert!(config.stealth_profile.is_some());
    assert!(config.validate().is_ok());
}

#[test]
fn test_bao_config_with_cdp_port() {
    let mut config = BaoConfig::default();
    config.cdp_port = Some(9333);
    assert_eq!(config.cdp_port, Some(9333));
    assert!(config.validate().is_ok());
}

#[test]
fn test_browser_config_with_stealth() {
    let profile = StealthProfile::firefox_default();
    let mut bc = BrowserConfig::default();
    bc.stealth_profile = Some(profile);
    let bao: BaoConfig = bc.into();
    assert!(bao.stealth_profile.is_some());
}

#[test]
fn test_page_config_with_stealth() {
    let profile = StealthProfile::chrome_default();
    let mut pc = PageConfig::default();
    pc.stealth_profile = Some(profile);
    assert!(pc.stealth_profile.is_some());
}

#[test]
fn test_page_config_with_permission() {
    let perm = Permission {
        read: Some(vec!["/safe".into()]),
        write: Some(vec!["/tmp".into()]),
        net: Some(vec!["example.com".into()]),
        env: Some(false),
        run: Some(false),
    };
    let mut pc = PageConfig::default();
    pc.permission = Some(perm.clone());
    assert!(pc.permission.is_some());
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_read("/safe/file").is_ok());
    assert!(guard.check_read("/unsafe").is_err());
}

#[test]
fn test_bao_config_idle_ttl() {
    let mut config = BaoConfig::default();
    config.idle_ttl = std::time::Duration::from_secs(120);
    assert_eq!(config.idle_ttl, std::time::Duration::from_secs(120));
}

#[test]
fn test_bao_config_max_pages_boundary() {
    let mut config = BaoConfig::default();
    config.max_pages = 1;
    assert!(config.validate().is_ok());
    config.max_pages = 200;
    assert!(config.validate().is_ok());
}

#[test]
fn test_browser_error_variants() {
    let errors = vec![
        BrowserError::Init("test".into()),
        BrowserError::Navigation("bad url".into()),
        BrowserError::Rendering("oom".into()),
        BrowserError::JavaScript("syntax".into()),
        BrowserError::CDP("timeout".into()),
    ];
    for err in &errors {
        let msg = format!("{}", err);
        assert!(!msg.is_empty());
        let _: &dyn std::error::Error = err;
    }
}

#[test]
fn test_permission_guard_none_allows_all() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_read("/any").is_ok());
    assert!(guard.check_write("/any").is_ok());
    assert!(guard.check_net("any.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}
