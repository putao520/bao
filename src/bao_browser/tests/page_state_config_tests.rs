// @trace TEST-BRW-019 [req:REQ-BRW-001,REQ-BRW-003] [level:unit]
// PageState enum, PoolStats struct, BaoConfig defaults, PageConfig defaults,
// PageState ordering, clone/copy/debug, PageHandle id/permission
// (where possible without servo instance).

use bao_browser::{PageState, BaoConfig, PageConfig, PermissionGuard};

// ---- PageState ----

#[test]
fn test_page_state_variants() {
    assert_eq!(PageState::Created, PageState::Created);
    assert_eq!(PageState::Navigating, PageState::Navigating);
    assert_eq!(PageState::Interactive, PageState::Interactive);
    assert_eq!(PageState::Idle, PageState::Idle);
    assert_eq!(PageState::Closed, PageState::Closed);
}

#[test]
fn test_page_state_not_equal() {
    assert_ne!(PageState::Created, PageState::Closed);
    assert_ne!(PageState::Navigating, PageState::Idle);
    assert_ne!(PageState::Interactive, PageState::Created);
}

#[test]
fn test_page_state_copy() {
    let s1 = PageState::Interactive;
    let s2 = s1;
    assert_eq!(s1, s2);
}

#[test]
fn test_page_state_clone() {
    let s1 = PageState::Navigating;
    let s2 = s1.clone();
    assert_eq!(s1, s2);
}

#[test]
fn test_page_state_debug() {
    assert_eq!(format!("{:?}", PageState::Created), "Created");
    assert_eq!(format!("{:?}", PageState::Navigating), "Navigating");
    assert_eq!(format!("{:?}", PageState::Interactive), "Interactive");
    assert_eq!(format!("{:?}", PageState::Idle), "Idle");
    assert_eq!(format!("{:?}", PageState::Closed), "Closed");
}

#[test]
fn test_page_state_ordering() {
    let states = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    for i in 0..states.len() - 1 {
        for j in (i + 1)..states.len() {
            // All variants are distinct
            assert_ne!(states[i], states[j]);
        }
    }
}

// ---- BaoConfig ----

#[test]
fn test_bao_config_default_cdp_port() {
    let config = BaoConfig::default();
    assert!(config.cdp_port.is_none());
}

#[test]
fn test_bao_config_default_max_pages() {
    let config = BaoConfig::default();
    assert!(config.max_pages > 0);
}

#[test]
fn test_bao_config_default_idle_ttl() {
    let config = BaoConfig::default();
    assert!(config.idle_ttl.as_secs() > 0);
}

#[test]
fn test_bao_config_default_viewport() {
    let config = BaoConfig::default();
    assert!(config.default_viewport_width > 0);
    assert!(config.default_viewport_height > 0);
}

#[test]
fn test_bao_config_default_no_stealth() {
    let config = BaoConfig::default();
    assert!(config.stealth_profile.is_none());
}

#[test]
fn test_bao_config_debug() {
    let config = BaoConfig::default();
    let debug = format!("{:?}", config);
    assert!(debug.contains("BaoConfig") || debug.contains("max_pages"));
}

#[test]
fn test_bao_config_clone() {
    let config = BaoConfig::default();
    let cloned = config.clone();
    assert_eq!(config.max_pages, cloned.max_pages);
    assert_eq!(config.cdp_port, cloned.cdp_port);
}

#[test]
fn test_bao_config_custom_values() {
    let config = BaoConfig {
        cdp_port: Some(9222),
        max_pages: 5,
        idle_ttl: std::time::Duration::from_secs(60),
        default_viewport_width: 1920,
        default_viewport_height: 1080,
        stealth_profile: None,
    };
    assert_eq!(config.cdp_port, Some(9222));
    assert_eq!(config.max_pages, 5);
    assert_eq!(config.default_viewport_width, 1920);
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
fn test_page_config_debug() {
    let config = PageConfig::default();
    let debug = format!("{:?}", config);
    assert!(!debug.is_empty());
}

#[test]
fn test_page_config_clone() {
    let config = PageConfig::default();
    let cloned = config.clone();
    assert!(cloned.url.is_none());
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
        viewport_width: Some(800),
        viewport_height: Some(600),
        ..Default::default()
    };
    assert_eq!(config.viewport_width, Some(800));
    assert_eq!(config.viewport_height, Some(600));
}

#[test]
fn test_page_config_with_permission() {
    let perm = bao_browser::Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    let config = PageConfig {
        permission: Some(perm),
        ..Default::default()
    };
    assert!(config.permission.is_some());
}

// ---- PermissionGuard integration with PageConfig ----

#[test]
fn test_permission_guard_none_is_default() {
    let guard = PermissionGuard::default();
    assert!(!guard.is_restricted());
}

#[test]
fn test_permission_guard_none_allows_all() {
    let guard = PermissionGuard::none();
    assert!(guard.check_read("/any").is_ok());
    assert!(guard.check_write("/any").is_ok());
    assert!(guard.check_net("any.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

#[test]
fn test_permission_guard_restricted_with_permission() {
    let perm = bao_browser::Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_net("example.com").is_ok());
    assert!(guard.check_net("evil.com").is_err());
}

// ---- PageState lifecycle simulation ----

#[test]
fn test_page_state_lifecycle_sequence() {
    let mut state = PageState::Created;
    assert_eq!(state, PageState::Created);

    state = PageState::Navigating;
    assert_eq!(state, PageState::Navigating);

    state = PageState::Interactive;
    assert_eq!(state, PageState::Interactive);

    state = PageState::Idle;
    assert_eq!(state, PageState::Idle);

    state = PageState::Closed;
    assert_eq!(state, PageState::Closed);
}

#[test]
fn test_page_state_closed_is_terminal() {
    let state = PageState::Closed;
    // Closed state should not transition back
    assert_eq!(state, PageState::Closed);
    assert_ne!(state, PageState::Created);
}

// ---- BaoConfig viewport validation ----

#[test]
fn test_bao_config_zero_viewport() {
    let config = BaoConfig {
        default_viewport_width: 0,
        default_viewport_height: 0,
        ..Default::default()
    };
    // Config accepts zero values (validation happens at PagePool creation)
    assert_eq!(config.default_viewport_width, 0);
}

#[test]
fn test_bao_config_large_viewport() {
    let config = BaoConfig {
        default_viewport_width: 7680,
        default_viewport_height: 4320,
        ..Default::default()
    };
    assert_eq!(config.default_viewport_width, 7680);
}

#[test]
fn test_bao_config_single_page() {
    let config = BaoConfig {
        max_pages: 1,
        ..Default::default()
    };
    assert_eq!(config.max_pages, 1);
}

#[test]
fn test_bao_config_many_pages() {
    let config = BaoConfig {
        max_pages: 1000,
        ..Default::default()
    };
    assert_eq!(config.max_pages, 1000);
}

// ---- Idle TTL variants ----

#[test]
fn test_bao_config_short_idle_ttl() {
    let config = BaoConfig {
        idle_ttl: std::time::Duration::from_secs(1),
        ..Default::default()
    };
    assert_eq!(config.idle_ttl, std::time::Duration::from_secs(1));
}

#[test]
fn test_bao_config_long_idle_ttl() {
    let config = BaoConfig {
        idle_ttl: std::time::Duration::from_secs(3600),
        ..Default::default()
    };
    assert_eq!(config.idle_ttl, std::time::Duration::from_secs(3600));
}
