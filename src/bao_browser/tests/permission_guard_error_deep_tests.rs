// @trace TEST-BRW-010 [req:REQ-LIB-003,REQ-LIB-004,REQ-BRW-001,REQ-BRW-002] [level:unit]
// Permission all-field exhaustive checks, PermissionGuard none/restricted paths,
// PermissionDenied Display + Error, BrowserError Display + all variants,
// BaoConfig validate edge cases, BrowserConfig From<BaoConfig>,
// PageConfig Default, ScreenshotFormat encode edge cases.

use bao_browser::*;
use bao_stealth::StealthProfile;
use std::time::Duration;

// ============================================================================
// Permission: read
// ============================================================================

#[test]
fn test_permission_read_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_read_allowed("/etc/passwd"));
    assert!(perm.is_read_allowed("/home/user/file.txt"));
    assert!(perm.is_read_allowed(""));
}

#[test]
fn test_permission_read_exact_match() {
    let perm = Permission { read: Some(vec!["/tmp".into()]), ..Default::default() };
    assert!(perm.is_read_allowed("/tmp"));
    assert!(perm.is_read_allowed("/tmp/file.txt"));
    assert!(perm.is_read_allowed("/tmpfile")); // starts_with("/tmp") matches
    assert!(!perm.is_read_allowed("/etc/passwd"));
}

#[test]
fn test_permission_read_multiple_prefixes() {
    let perm = Permission { read: Some(vec!["/home".into(), "/tmp".into()]), ..Default::default() };
    assert!(perm.is_read_allowed("/home/user/file"));
    assert!(perm.is_read_allowed("/tmp/data.json"));
    assert!(!perm.is_read_allowed("/etc/config"));
}

#[test]
fn test_permission_read_empty_vec_blocks_all() {
    let perm = Permission { read: Some(vec![]), ..Default::default() };
    assert!(!perm.is_read_allowed("/anything"));
    assert!(!perm.is_read_allowed(""));
}

// ============================================================================
// Permission: write
// ============================================================================

#[test]
fn test_permission_write_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_write_allowed("/tmp/output"));
    assert!(perm.is_write_allowed("/var/log/app.log"));
}

#[test]
fn test_permission_write_prefix_match() {
    let perm = Permission { write: Some(vec!["/var".into()]), ..Default::default() };
    assert!(perm.is_write_allowed("/var/log/test.log"));
    assert!(perm.is_write_allowed("/var"));
    assert!(!perm.is_write_allowed("/tmp/write"));
}

#[test]
fn test_permission_write_empty_vec_blocks_all() {
    let perm = Permission { write: Some(vec![]), ..Default::default() };
    assert!(!perm.is_write_allowed("/any/path"));
}

// ============================================================================
// Permission: net
// ============================================================================

#[test]
fn test_permission_net_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_net_allowed("example.com"));
    assert!(perm.is_net_allowed("api.test.com"));
    assert!(perm.is_net_allowed(""));
}

#[test]
fn test_permission_net_exact_match() {
    let perm = Permission { net: Some(vec!["example.com".into()]), ..Default::default() };
    assert!(perm.is_net_allowed("example.com"));
    assert!(perm.is_net_allowed("sub.example.com"));
    assert!(!perm.is_net_allowed("notexample.com"));
    assert!(!perm.is_net_allowed("example.com.evil.org"));
}

#[test]
fn test_permission_net_subdomain_match() {
    let perm = Permission { net: Some(vec!["api.service.io".into()]), ..Default::default() };
    assert!(perm.is_net_allowed("api.service.io"));
    assert!(perm.is_net_allowed("v2.api.service.io"));
    assert!(!perm.is_net_allowed("service.io"));
}

#[test]
fn test_permission_net_multiple_domains() {
    let perm = Permission { net: Some(vec!["a.com".into(), "b.com".into()]), ..Default::default() };
    assert!(perm.is_net_allowed("a.com"));
    assert!(perm.is_net_allowed("b.com"));
    assert!(!perm.is_net_allowed("c.com"));
}

#[test]
fn test_permission_net_empty_vec_blocks_all() {
    let perm = Permission { net: Some(vec![]), ..Default::default() };
    assert!(!perm.is_net_allowed("any.host"));
}

// ============================================================================
// Permission: env
// ============================================================================

#[test]
fn test_permission_env_none_allows() {
    let perm = Permission::default();
    assert!(perm.is_env_allowed());
}

#[test]
fn test_permission_env_explicit_true() {
    let perm = Permission { env: Some(true), ..Default::default() };
    assert!(perm.is_env_allowed());
}

#[test]
fn test_permission_env_explicit_false() {
    let perm = Permission { env: Some(false), ..Default::default() };
    assert!(!perm.is_env_allowed());
}

// ============================================================================
// Permission: run
// ============================================================================

#[test]
fn test_permission_run_none_allows() {
    let perm = Permission::default();
    assert!(perm.is_run_allowed());
}

#[test]
fn test_permission_run_explicit_true() {
    let perm = Permission { run: Some(true), ..Default::default() };
    assert!(perm.is_run_allowed());
}

#[test]
fn test_permission_run_explicit_false() {
    let perm = Permission { run: Some(false), ..Default::default() };
    assert!(!perm.is_run_allowed());
}

// ============================================================================
// Permission: clone + debug
// ============================================================================

#[test]
fn test_permission_clone() {
    let perm = Permission {
        read: Some(vec!["/a".into()]),
        write: Some(vec!["/b".into()]),
        net: Some(vec!["c.com".into()]),
        env: Some(false),
        run: Some(true),
        ..Default::default()
    };
    let cloned = perm.clone();
    assert!(cloned.is_read_allowed("/a/file"));
    assert!(!cloned.is_read_allowed("/x"));
    assert!(cloned.is_write_allowed("/b/file"));
    assert!(!cloned.is_env_allowed());
    assert!(cloned.is_run_allowed());
}

#[test]
fn test_permission_debug() {
    let perm = Permission { read: Some(vec!["/test".into()]), ..Default::default() };
    let s = format!("{:?}", perm);
    assert!(s.contains("Permission"));
    assert!(s.contains("/test"));
}

#[test]
fn test_permission_default() {
    let d1 = Permission::default();
    let d2: Permission = Default::default();
    assert!(d1.is_read_allowed("/any"));
    assert!(d2.is_read_allowed("/any"));
}

// ============================================================================
// PermissionGuard: none mode
// ============================================================================

#[test]
fn test_guard_none_not_restricted() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
}

#[test]
fn test_guard_none_allows_read() {
    assert!(PermissionGuard::none().check_read("/secret").is_ok());
}

#[test]
fn test_guard_none_allows_write() {
    assert!(PermissionGuard::none().check_write("/readonly").is_ok());
}

#[test]
fn test_guard_none_allows_net() {
    assert!(PermissionGuard::none().check_net("evil.com").is_ok());
}

#[test]
fn test_guard_none_allows_env() {
    assert!(PermissionGuard::none().check_env().is_ok());
}

#[test]
fn test_guard_none_allows_run() {
    assert!(PermissionGuard::none().check_run().is_ok());
}

// ============================================================================
// PermissionGuard: restricted mode
// ============================================================================

#[test]
fn test_guard_restricted_is_restricted() {
    let perm = Permission { read: Some(vec!["/safe".into()]), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
}

#[test]
fn test_guard_restricted_read_allowed() {
    let perm = Permission { read: Some(vec!["/safe".into()]), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_read("/safe/file.txt").is_ok());
}

#[test]
fn test_guard_restricted_read_denied() {
    let perm = Permission { read: Some(vec!["/safe".into()]), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    let err = guard.check_read("/etc/passwd").unwrap_err();
    assert_eq!(err.category, "read");
    assert_eq!(err.resource, "/etc/passwd");
}

#[test]
fn test_guard_restricted_write_denied() {
    let perm = Permission { write: Some(vec![]), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    let err = guard.check_write("/any").unwrap_err();
    assert_eq!(err.category, "write");
}

#[test]
fn test_guard_restricted_net_denied() {
    let perm = Permission { net: Some(vec!["trusted.com".into()]), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_net("untrusted.com").is_err());
    assert!(guard.check_net("trusted.com").is_ok());
}

#[test]
fn test_guard_restricted_env_denied() {
    let perm = Permission { env: Some(false), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    let err = guard.check_env().unwrap_err();
    assert_eq!(err.category, "env");
    assert_eq!(err.resource, "*");
}

#[test]
fn test_guard_restricted_run_denied() {
    let perm = Permission { run: Some(false), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    let err = guard.check_run().unwrap_err();
    assert_eq!(err.category, "run");
    assert_eq!(err.resource, "*");
}

#[test]
fn test_guard_restricted_env_allowed() {
    let perm = Permission { env: Some(true), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_env().is_ok());
}

#[test]
fn test_guard_restricted_run_allowed() {
    let perm = Permission { run: Some(true), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_run().is_ok());
}

// ============================================================================
// PermissionGuard: clone + debug + default
// ============================================================================

#[test]
fn test_guard_clone_preserves_state() {
    let perm = Permission { read: Some(vec!["/a".into()]), ..Default::default() };
    let guard = PermissionGuard::new(perm);
    let cloned = guard.clone();
    assert!(cloned.is_restricted());
    assert!(cloned.check_read("/a/file").is_ok());
    assert!(cloned.check_read("/b/file").is_err());
}

#[test]
fn test_guard_default_is_none() {
    let guard = PermissionGuard::default();
    assert!(!guard.is_restricted());
}

#[test]
fn test_guard_debug() {
    let guard = PermissionGuard::none();
    let s = format!("{:?}", guard);
    assert!(s.contains("PermissionGuard"));
}

// ============================================================================
// PermissionDenied
// ============================================================================

#[test]
fn test_permission_denied_display() {
    let err = PermissionDenied {
        category: "read".into(),
        resource: "/etc/passwd".into(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("read"));
    assert!(msg.contains("/etc/passwd"));
    assert!(msg.contains("Permission denied"));
}

#[test]
fn test_permission_denied_clone() {
    let err = PermissionDenied {
        category: "net".into(),
        resource: "evil.com".into(),
    };
    let cloned = err.clone();
    assert_eq!(cloned.category, "net");
    assert_eq!(cloned.resource, "evil.com");
}

#[test]
fn test_permission_denied_debug() {
    let err = PermissionDenied {
        category: "write".into(),
        resource: "/tmp".into(),
    };
    let s = format!("{:?}", err);
    assert!(s.contains("write"));
}

#[test]
fn test_permission_denied_is_error() {
    let err = PermissionDenied {
        category: "test".into(),
        resource: "res".into(),
    };
    let _: &dyn std::error::Error = &err;
}

// ============================================================================
// BrowserError: all variants
// ============================================================================

#[test]
fn test_browser_error_init() {
    let err = BrowserError::Init("test init".into());
    let msg = format!("{}", err);
    assert!(msg.contains("init"));
    assert!(msg.contains("test init"));
}

#[test]
fn test_browser_error_navigation() {
    let err = BrowserError::Navigation("bad url".into());
    let msg = format!("{}", err);
    assert!(msg.contains("navigation"));
    assert!(msg.contains("bad url"));
}

#[test]
fn test_browser_error_rendering() {
    let err = BrowserError::Rendering("gpu fail".into());
    let msg = format!("{}", err);
    assert!(msg.contains("rendering"));
    assert!(msg.contains("gpu fail"));
}

#[test]
fn test_browser_error_javascript() {
    let err = BrowserError::JavaScript("syntax error".into());
    let msg = format!("{}", err);
    assert!(msg.contains("javascript"));
    assert!(msg.contains("syntax error"));
}

#[test]
fn test_browser_error_cdp() {
    let err = BrowserError::CDP("ws closed".into());
    let msg = format!("{}", err);
    assert!(msg.contains("cdp"));
    assert!(msg.contains("ws closed"));
}

#[test]
fn test_browser_error_is_std_error() {
    let err = BrowserError::Init("test".into());
    let _: &dyn std::error::Error = &err;
}

#[test]
fn test_browser_error_debug() {
    let err = BrowserError::Navigation("dbg".into());
    let s = format!("{:?}", err);
    assert!(s.contains("Navigation"));
}

// ============================================================================
// PageState
// ============================================================================

#[test]
fn test_page_state_variants() {
    let states = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    assert_eq!(states.len(), 5);
}

#[test]
fn test_page_state_equality() {
    assert_eq!(PageState::Created, PageState::Created);
    assert_ne!(PageState::Created, PageState::Navigating);
    assert_ne!(PageState::Navigating, PageState::Interactive);
    assert_ne!(PageState::Interactive, PageState::Idle);
    assert_ne!(PageState::Idle, PageState::Closed);
}

#[test]
fn test_page_state_copy() {
    let s = PageState::Interactive;
    let s2 = s;
    assert_eq!(s, s2);
}

#[test]
fn test_page_state_debug() {
    assert!(format!("{:?}", PageState::Created).contains("Created"));
    assert!(format!("{:?}", PageState::Closed).contains("Closed"));
}

#[test]
fn test_page_state_clone() {
    let s = PageState::Navigating;
    let s2 = s.clone();
    assert_eq!(s, s2);
}

// ============================================================================
// BaoConfig
// ============================================================================

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
fn test_bao_config_validate_viewport_width_low() {
    let config = BaoConfig { default_viewport_width: 799, ..Default::default() };
    let err = config.validate().unwrap_err();
    assert!(err.contains("viewport_width"));
    assert!(err.contains("799"));
}

#[test]
fn test_bao_config_validate_viewport_height_low() {
    let config = BaoConfig { default_viewport_height: 599, ..Default::default() };
    let err = config.validate().unwrap_err();
    assert!(err.contains("viewport_height"));
    assert!(err.contains("599"));
}

#[test]
fn test_bao_config_validate_min_boundaries() {
    let config = BaoConfig {
        max_pages: 1,
        default_viewport_width: 800,
        default_viewport_height: 600,
        ..Default::default()
    };
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
fn test_bao_config_with_stealth() {
    let config = BaoConfig {
        stealth_profile: Some(StealthProfile::firefox_default()),
        ..Default::default()
    };
    assert!(config.stealth_profile.is_some());
    assert!(config.validate().is_ok());
}

#[test]
fn test_bao_config_debug() {
    let config = BaoConfig::default();
    let s = format!("{:?}", config);
    assert!(s.contains("BaoConfig"));
    assert!(s.contains("1920"));
}

#[test]
fn test_bao_config_clone() {
    let config = BaoConfig::default();
    let cloned = config.clone();
    assert_eq!(cloned.max_pages, config.max_pages);
    assert_eq!(cloned.cdp_port, config.cdp_port);
}

// ============================================================================
// BrowserConfig
// ============================================================================

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
fn test_browser_config_from_into_bao_config() {
    let bc = BrowserConfig {
        url: Some("https://example.com".into()),
        cdp_port: 1234,
        viewport_width: 1280,
        viewport_height: 720,
        headless: false,
        stealth_profile: Some(StealthProfile::chrome_default()),
    };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(1234));
    assert_eq!(bao.default_viewport_width, 1280);
    assert_eq!(bao.default_viewport_height, 720);
    assert!(bao.stealth_profile.is_some());
}

#[test]
fn test_browser_config_default_from() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(9222));
    assert_eq!(bao.default_viewport_width, 1920);
    assert_eq!(bao.default_viewport_height, 1080);
}

#[test]
fn test_browser_config_debug() {
    let bc = BrowserConfig::default();
    let s = format!("{:?}", bc);
    assert!(s.contains("BrowserConfig"));
    assert!(s.contains("9222"));
}

#[test]
fn test_browser_config_clone() {
    let bc = BrowserConfig {
        url: Some("https://test.com".into()),
        ..Default::default()
    };
    let cloned = bc.clone();
    assert_eq!(cloned.url, bc.url);
    assert_eq!(cloned.cdp_port, bc.cdp_port);
}

// ============================================================================
// PageConfig
// ============================================================================

#[test]
fn test_page_config_default() {
    let pc = PageConfig::default();
    assert!(pc.url.is_none());
    assert!(pc.viewport_width.is_none());
    assert!(pc.viewport_height.is_none());
    assert!(pc.stealth_profile.is_none());
    assert!(pc.permission.is_none());
}

#[test]
fn test_page_config_with_url() {
    let pc = PageConfig {
        url: Some("https://example.com".into()),
        ..Default::default()
    };
    assert_eq!(pc.url.as_deref(), Some("https://example.com"));
}

#[test]
fn test_page_config_with_custom_viewport() {
    let pc = PageConfig {
        viewport_width: Some(2560),
        viewport_height: Some(1440),
        ..Default::default()
    };
    assert_eq!(pc.viewport_width, Some(2560));
    assert_eq!(pc.viewport_height, Some(1440));
}

#[test]
fn test_page_config_with_stealth() {
    let pc = PageConfig {
        stealth_profile: Some(StealthProfile::firefox_default()),
        ..Default::default()
    };
    assert!(pc.stealth_profile.is_some());
}

#[test]
fn test_page_config_with_permission() {
    let perm = Permission {
        read: Some(vec!["/safe".into()]),
        ..Default::default()
    };
    let pc = PageConfig {
        permission: Some(perm),
        ..Default::default()
    };
    assert!(pc.permission.is_some());
    let p = pc.permission.unwrap();
    assert!(p.is_read_allowed("/safe/file"));
}

#[test]
fn test_page_config_debug() {
    let pc = PageConfig::default();
    let s = format!("{:?}", pc);
    assert!(s.contains("PageConfig"));
}

#[test]
fn test_page_config_clone() {
    let pc = PageConfig {
        url: Some("https://test.com".into()),
        viewport_width: Some(1024),
        viewport_height: Some(768),
        ..Default::default()
    };
    let cloned = pc.clone();
    assert_eq!(cloned.url, pc.url);
    assert_eq!(cloned.viewport_width, pc.viewport_width);
}

// ============================================================================
// ScreenshotFormat: encode_image
// ============================================================================

#[test]
fn test_encode_image_png_1x1() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    assert!(data.starts_with(&[0x89, 0x50, 0x4E, 0x47])); // PNG magic bytes
}

#[test]
fn test_encode_image_jpeg_1x1() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    assert!(data.starts_with(&[0xFF, 0xD8])); // JPEG magic bytes
}

#[test]
fn test_encode_image_png_transparent() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 0]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
}

#[test]
fn test_encode_image_png_gradient() {
    use image::RgbaImage;
    let mut img = RgbaImage::new(256, 256);
    for y in 0..256 {
        for x in 0..256 {
            img.put_pixel(x, y, image::Rgba([x as u8, y as u8, 128, 255]));
        }
    }
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    assert!(result.unwrap().len() > 1000);
}

#[test]
fn test_encode_image_jpeg_larger() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(100, 100, image::Rgba([128, 128, 128, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}

#[test]
fn test_encode_image_png_vs_jpeg_size() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 100, 50, 255]));
    let png = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let jpeg = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    // JPEG is usually smaller for photos, PNG for solid colors
    // Just verify both are non-empty
    assert!(!png.is_empty());
    assert!(!jpeg.is_empty());
}
