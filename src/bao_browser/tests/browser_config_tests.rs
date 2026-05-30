// @trace TEST-BRW-001-CONFIG [req:REQ-BRW-001] [level:unit]
// Unit tests for bao_browser config, permission, and error types

use bao_browser::{BaoConfig, BrowserConfig, PageConfig, BrowserError};
use bao_browser::{Permission, PermissionDenied, PermissionGuard};

#[test]
fn test_bao_config_default() {
    let config = BaoConfig::default();
    assert_eq!(config.cdp_port, None);
    assert_eq!(config.max_pages, 50);
    assert_eq!(config.idle_ttl, std::time::Duration::from_secs(60));
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
fn test_bao_config_validate_zero_max_pages() {
    let mut config = BaoConfig::default();
    config.max_pages = 0;
    let err = config.validate().unwrap_err();
    assert!(err.contains("max_pages must be >= 1"));
}

#[test]
fn test_bao_config_validate_small_viewport_width() {
    let mut config = BaoConfig::default();
    config.default_viewport_width = 400;
    let err = config.validate().unwrap_err();
    assert!(err.contains("viewport_width must be >= 800"));
}

#[test]
fn test_bao_config_validate_small_viewport_height() {
    let mut config = BaoConfig::default();
    config.default_viewport_height = 300;
    let err = config.validate().unwrap_err();
    assert!(err.contains("viewport_height must be >= 600"));
}

#[test]
fn test_bao_config_validate_boundary() {
    let mut config = BaoConfig::default();
    config.max_pages = 1;
    config.default_viewport_width = 800;
    config.default_viewport_height = 600;
    assert!(config.validate().is_ok());
}

#[test]
fn test_browser_config_default() {
    let config = BrowserConfig::default();
    assert_eq!(config.cdp_port, 9222);
    assert_eq!(config.viewport_width, 1920);
    assert_eq!(config.viewport_height, 1080);
    assert!(config.headless);
    assert!(config.url.is_none());
    assert!(config.stealth_profile.is_none());
}

#[test]
fn test_browser_config_into_bao_config() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(9222));
    assert_eq!(bao.default_viewport_width, 1920);
    assert_eq!(bao.default_viewport_height, 1080);
    assert_eq!(bao.max_pages, 50);
}

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
fn test_permission_default_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_read_allowed("/any/path"));
    assert!(perm.is_write_allowed("/any/path"));
    assert!(perm.is_net_allowed("example.com"));
    assert!(perm.is_env_allowed());
    assert!(perm.is_run_allowed());
}

#[test]
fn test_permission_read_whitelist() {
    let perm = Permission {
        read: Some(vec!["/tmp".into(), "/home".into()]),
        ..Default::default()
    };
    assert!(perm.is_read_allowed("/tmp/file.txt"));
    assert!(perm.is_read_allowed("/home/user"));
    assert!(!perm.is_read_allowed("/etc/passwd"));
    assert!(perm.is_write_allowed("/etc/passwd"));
}

#[test]
fn test_permission_write_whitelist() {
    let perm = Permission {
        write: Some(vec!["/tmp".into()]),
        ..Default::default()
    };
    assert!(perm.is_write_allowed("/tmp/output.log"));
    assert!(!perm.is_write_allowed("/var/log"));
    assert!(perm.is_read_allowed("/var/log"));
}

#[test]
fn test_permission_net_whitelist() {
    let perm = Permission {
        net: Some(vec!["example.com".into(), "api.test.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("example.com"));
    assert!(perm.is_net_allowed("sub.example.com"));
    assert!(perm.is_net_allowed("api.test.com"));
    assert!(!perm.is_net_allowed("evil.com"));
}

#[test]
fn test_permission_env_denied() {
    let perm = Permission {
        env: Some(false),
        ..Default::default()
    };
    assert!(!perm.is_env_allowed());
    assert!(perm.is_run_allowed());
}

#[test]
fn test_permission_run_denied() {
    let perm = Permission {
        run: Some(false),
        ..Default::default()
    };
    assert!(!perm.is_run_allowed());
    assert!(perm.is_env_allowed());
}

#[test]
fn test_permission_guard_none_mode() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_read("/any").is_ok());
    assert!(guard.check_write("/any").is_ok());
    assert!(guard.check_net("any.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

#[test]
fn test_permission_guard_restricted() {
    let guard = PermissionGuard::new(Permission {
        read: Some(vec!["/safe".into()]),
        write: Some(vec!["/safe".into()]),
        net: Some(vec!["safe.com".into()]),
        env: Some(false),
        run: Some(false),
        ..Default::default()
    });
    assert!(guard.is_restricted());
    assert!(guard.check_read("/safe/file").is_ok());
    assert!(guard.check_read("/unsafe").is_err());
    assert!(guard.check_write("/safe/out").is_ok());
    assert!(guard.check_write("/unsafe").is_err());
    assert!(guard.check_net("safe.com").is_ok());
    assert!(guard.check_net("evil.com").is_err());
    assert!(guard.check_env().is_err());
    assert!(guard.check_run().is_err());
}

#[test]
fn test_permission_denied_display() {
    let err = PermissionDenied {
        category: "read".into(),
        resource: "/secret".into(),
    };
    assert_eq!(format!("{}", err), "Permission denied: read on /secret");
}

#[test]
fn test_permission_denied_is_error() {
    let err = PermissionDenied {
        category: "net".into(),
        resource: "evil.com".into(),
    };
    let _: &dyn std::error::Error = &err;
}

#[test]
fn test_browser_error_display() {
    assert_eq!(
        format!("{}", BrowserError::Init("failed".into())),
        "browser init error: failed"
    );
    assert_eq!(
        format!("{}", BrowserError::Navigation("bad url".into())),
        "navigation error: bad url"
    );
    assert_eq!(
        format!("{}", BrowserError::Rendering("oom".into())),
        "rendering error: oom"
    );
    assert_eq!(
        format!("{}", BrowserError::JavaScript("syntax".into())),
        "javascript error: syntax"
    );
    assert_eq!(
        format!("{}", BrowserError::CDP("timeout".into())),
        "cdp error: timeout"
    );
}

#[test]
fn test_browser_error_is_std_error() {
    let err = BrowserError::Init("test".into());
    let _: &dyn std::error::Error = &err;
}
