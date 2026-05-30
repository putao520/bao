// @trace TEST-LIB-005-CROSS [req:REQ-LIB-001,REQ-LIB-003,REQ-LIB-004,REQ-CDP-001,REQ-STL-007] [level:integration]
// Cross-crate integration: BaoRuntime ↔ CdpServer ↔ StealthEngine ↔ PermissionGuard
// NOTE: servo Opts is single-init (like mozjs JSEngine), so all BaoRuntime tests
// MUST be in a single test function. Other pure-data tests can be separate.

use bao_browser::{BaoConfig, BrowserConfig, PageConfig, BrowserError, PageState};
use bao_browser::{Permission, PermissionGuard};
use bao_stealth::StealthProfile;
use bao_cdp::servo_bridge::bridge_channel;
use bao_cdp::domains::{register_all_domains_into, ServoTargetProvider};

use std::time::Duration;

// ---- Pure data tests (no servo init) ----

#[test]
fn test_browser_error_display_variants() {
    let errors = vec![
        BrowserError::Init("init failed".into()),
        BrowserError::Navigation("nav failed".into()),
        BrowserError::Rendering("render failed".into()),
        BrowserError::JavaScript("js failed".into()),
        BrowserError::CDP("cdp failed".into()),
    ];
    let displays: Vec<String> = errors.iter().map(|e| format!("{}", e)).collect();
    assert!(displays[0].contains("init"));
    assert!(displays[1].contains("navigation"));
    assert!(displays[2].contains("rendering"));
    assert!(displays[3].contains("javascript"));
    assert!(displays[4].contains("cdp"));
}

#[test]
fn test_browser_error_debug_variants() {
    let err = BrowserError::Navigation("test".into());
    let debug = format!("{:?}", err);
    assert!(debug.contains("Navigation"));
}

#[test]
fn test_browser_error_is_std_error() {
    let err = BrowserError::CDP("test".into());
    let _: &dyn std::error::Error = &err;
}

#[test]
fn test_browser_config_preserves_all_fields() {
    let mut bc = BrowserConfig::default();
    bc.cdp_port = 9333;
    bc.viewport_width = 1280;
    bc.viewport_height = 720;
    bc.headless = true;
    bc.url = Some("https://example.com".into());
    bc.stealth_profile = Some(StealthProfile::chrome_default());

    let bao_config: BaoConfig = bc.into();
    assert_eq!(bao_config.cdp_port, Some(9333));
    assert_eq!(bao_config.default_viewport_width, 1280);
    assert_eq!(bao_config.default_viewport_height, 720);
    assert!(bao_config.stealth_profile.is_some());
}

#[test]
fn test_cdp_router_session_creation() {
    use bao_cdp::CdpRouter;

    let router = CdpRouter::new();
    let session = router.create_internal_session("test-target-1");

    assert!(!session.target_id().is_empty());
    assert!(!session.session_id().is_empty());
}

#[test]
fn test_cdp_router_multiple_sessions() {
    use bao_cdp::CdpRouter;

    let router = CdpRouter::new();
    let s1 = router.create_internal_session("target-1");
    let s2 = router.create_internal_session("target-2");
    let s3 = router.create_internal_session("target-3");

    let ids: Vec<&str> = vec![s1.session_id(), s2.session_id(), s3.session_id()];
    for i in 0..ids.len() {
        for j in (i+1)..ids.len() {
            assert_ne!(ids[i], ids[j], "Session IDs must be unique");
        }
    }
}

#[test]
fn test_cdp_session_backend_kind() {
    use bao_cdp::{CdpRouter, BackendKind};

    let router = CdpRouter::new();
    let session = router.create_internal_session("test-target");

    assert_eq!(session.backend_kind(), BackendKind::Internal);
}

#[test]
fn test_cdp_server_with_bridge_and_domains() {
    let (bridge_tx, _bridge_rx) = bridge_channel(Duration::from_secs(5));
    let config = bao_cdp::ServerConfig::builder()
        .host("127.0.0.1")
        .port(0)
        .build();
    let mut server = bao_cdp::CdpServer::new(config);
    register_all_domains_into(bridge_tx.clone(), server.registry());

    let provider = std::sync::Arc::new(
        ServoTargetProvider::new(bridge_tx, "127.0.0.1".into(), 0)
    );
    server.set_target_provider(provider);

    assert_eq!(server.port(), 0);
}

#[test]
fn test_permission_guard_none_allows_all() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_read("/secret").is_ok());
    assert!(guard.check_write("/secret").is_ok());
    assert!(guard.check_net("evil.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

#[test]
fn test_permission_guard_restricted() {
    let perm = Permission {
        net: Some(vec!["safe.com".into()]),
        read: Some(vec!["/tmp/".into()]),
        write: Some(vec![]),
        env: Some(false),
        run: Some(false),
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_net("safe.com").is_ok());
    assert!(guard.check_net("evil.com").is_err());
    assert!(guard.check_read("/tmp/file").is_ok());
    assert!(guard.check_write("/any").is_err());
    assert!(guard.check_env().is_err());
    assert!(guard.check_run().is_err());
}

// ---- Single BaoRuntime test (servo Opts single-init constraint) ----
// NOTE: BaoRuntime::create_page calls inject_all_with_profile → evaluate_js,
// which requires servo event loop. Use PagePool::create_page directly (no JS injection).

#[test]
fn test_bao_runtime_cross_crate_all() {
    // --- Create runtime ---
    let config = BaoConfig {
        max_pages: 5,
        ..Default::default()
    };
    let runtime = bao_browser::BaoRuntime::new(config).unwrap();
    assert_eq!(runtime.page_pool().stats().active, 0);

    // --- Create pages via pool (no JS injection) ---
    let p1 = runtime.page_pool().create_page(&PageConfig {
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    }).unwrap();
    let p2 = runtime.page_pool().create_page(&PageConfig {
        stealth_profile: Some(StealthProfile::firefox_default()),
        ..Default::default()
    }).unwrap();
    let p3 = runtime.page_pool().create_page(&PageConfig::default()).unwrap();

    // --- IDs are unique ---
    assert_ne!(p1.id(), p2.id());
    assert_ne!(p2.id(), p3.id());

    // --- Pool stats ---
    let stats = runtime.page_pool().stats();
    assert_eq!(stats.active, 3);
    assert_eq!(stats.total_created, 3);

    // --- Close page via pool (updates pool stats) ---
    runtime.page_pool().close_page(p2.id()).unwrap();
    assert_eq!(p2.get_state(), PageState::Closed);
    assert!(!p2.is_alive());
    // Operations on closed page fail
    assert!(p2.navigate("https://example.com").is_err());
    assert!(p2.evaluate_js("1+1").is_err());
    assert!(p2.page_title().is_none());
    assert!(p2.current_url().is_none());

    // --- Pool stats after close ---
    let stats = runtime.page_pool().stats();
    assert_eq!(stats.active, 2);
    assert_eq!(stats.total_destroyed, 1);

    // --- Navigate error paths ---
    let bad_nav = p1.navigate("not a url ::::invalid");
    assert!(bad_nav.is_err());
    match bad_nav {
        Err(BrowserError::Navigation(msg)) => assert!(msg.contains("invalid URL")),
        _ => panic!("Expected Navigation error"),
    }
    assert!(p1.navigate("").is_err());

    // --- Page with restricted permission ---
    let perm = Permission {
        net: Some(vec!["allowed.com".into()]),
        read: Some(vec!["/tmp/".into()]),
        write: Some(vec![]),
        env: Some(false),
        run: Some(false),
    };
    let restricted = runtime.page_pool().create_page(&PageConfig {
        permission: Some(perm),
        ..Default::default()
    }).unwrap();
    let guard = restricted.permission();
    assert!(guard.is_restricted());
    assert!(guard.check_net("allowed.com").is_ok());
    assert!(guard.check_net("evil.com").is_err());
    assert!(guard.check_read("/tmp/file").is_ok());
    assert!(guard.check_write("/any").is_err());
    assert!(guard.check_env().is_err());

    // --- Open page permission ---
    let open = runtime.page_pool().create_page(&PageConfig::default()).unwrap();
    let guard = open.permission();
    assert!(!guard.is_restricted());
    assert!(guard.check_net("any.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());

    // --- Capacity limit (max_pages=5, active=4: p1 + p3 + restricted + open) ---
    assert_eq!(runtime.page_pool().stats().active, 4);
    let fifth = runtime.page_pool().create_page(&PageConfig::default());
    assert!(fifth.is_ok());
    let overflow = runtime.page_pool().create_page(&PageConfig::default());
    assert!(overflow.is_err());
    match overflow {
        Err(BrowserError::Init(msg)) => assert!(msg.contains("limit exceeded")),
        _ => panic!("Expected page limit error"),
    }

    // --- Close all (p2 already destroyed by close_page, close_all destroys remaining 4) ---
    runtime.page_pool().close_all();
    let stats = runtime.page_pool().stats();
    assert_eq!(stats.active, 0);
    assert_eq!(stats.idle, 0);
    assert_eq!(stats.total_created, 6);
    assert_eq!(stats.total_destroyed, 6); // 1 (close_page) + 5 (close_all)
}
