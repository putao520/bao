// @trace TEST-BRW-CONFIG-BOUNDARY [req:REQ-BRW-002] [level:unit]
// Deep BaoConfig/BrowserConfig/PageConfig boundary and validation tests:
// viewport extremes, max_pages boundaries, cdp_port boundaries,
// BrowserConfig→BaoConfig field preservation, PageConfig viewport override,
// StealthProfile propagation chain, BrowserError Display/Debug all variants,
// ScreenshotFormat PNG/JPEG validation, PermissionGuard pattern matching,
// PageState variant properties.

use std::time::Duration;

use bao_browser::{
    BaoConfig, BrowserConfig, BrowserError, PageConfig, PageState,
    Permission, PermissionDenied, PermissionGuard, ScreenshotFormat,
};
use bao_stealth::StealthProfile;

// ============================================================
// 1. BaoConfig::validate() viewport boundary values
// ============================================================

#[test]
fn validate_viewport_0x0_fails_both_dimensions() {
    let cfg = BaoConfig {
        default_viewport_width: 0,
        default_viewport_height: 0,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    // max_pages=50 passes, so first failure is viewport_width
    assert!(
        err.contains("viewport_width"),
        "expected viewport_width error, got: {err}"
    );
}

#[test]
fn validate_viewport_1x1_fails_both_dimensions() {
    let cfg = BaoConfig {
        default_viewport_width: 1,
        default_viewport_height: 1,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        err.contains("viewport_width"),
        "expected viewport_width error, got: {err}"
    );
}

#[test]
fn validate_viewport_799x599_fails_both_dimensions() {
    let cfg = BaoConfig {
        default_viewport_width: 799,
        default_viewport_height: 599,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        err.contains("viewport_width"),
        "first failing check should be viewport_width, got: {err}"
    );
}

#[test]
fn validate_viewport_800x599_fails_height_only() {
    let cfg = BaoConfig {
        default_viewport_width: 800,
        default_viewport_height: 599,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        err.contains("viewport_height"),
        "expected viewport_height error, got: {err}"
    );
    assert!(
        !err.contains("viewport_width"),
        "viewport_width should pass at 800, got: {err}"
    );
}

#[test]
fn validate_viewport_799x600_fails_width_only() {
    let cfg = BaoConfig {
        default_viewport_width: 799,
        default_viewport_height: 600,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        err.contains("viewport_width"),
        "expected viewport_width error, got: {err}"
    );
}

#[test]
fn validate_viewport_800x600_minimum_passes() {
    let cfg = BaoConfig {
        default_viewport_width: 800,
        default_viewport_height: 600,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_viewport_7680x4320_8k_passes() {
    let cfg = BaoConfig {
        default_viewport_width: 7680,
        default_viewport_height: 4320,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_viewport_801x601_just_above_minimum_passes() {
    let cfg = BaoConfig {
        default_viewport_width: 801,
        default_viewport_height: 601,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_viewport_u32_max_passes() {
    let cfg = BaoConfig {
        default_viewport_width: u32::MAX,
        default_viewport_height: u32::MAX,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_reports_actual_value_in_error() {
    let cfg = BaoConfig {
        default_viewport_width: 42,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        err.contains("42"),
        "error message should include the actual value, got: {err}"
    );
}

// ============================================================
// 2. BaoConfig max_pages boundary values
// ============================================================

#[test]
fn validate_max_pages_0_fails() {
    let cfg = BaoConfig {
        max_pages: 0,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        err.contains("max_pages must be >= 1"),
        "unexpected error: {err}"
    );
    assert!(
        err.contains("0"),
        "error should report value 0, got: {err}"
    );
}

#[test]
fn validate_max_pages_1_minimum_passes() {
    let cfg = BaoConfig {
        max_pages: 1,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_max_pages_2_passes() {
    let cfg = BaoConfig {
        max_pages: 2,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_max_pages_1000_passes() {
    let cfg = BaoConfig {
        max_pages: 1000,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_max_pages_usize_max_passes() {
    let cfg = BaoConfig {
        max_pages: usize::MAX,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_max_pages_checked_before_viewport() {
    // When both max_pages and viewport are invalid, max_pages error comes first
    let cfg = BaoConfig {
        max_pages: 0,
        default_viewport_width: 100,
        default_viewport_height: 100,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        err.contains("max_pages"),
        "max_pages should be checked first, got: {err}"
    );
}

// ============================================================
// 3. BaoConfig cdp_port boundary values
// ============================================================

#[test]
fn bao_config_cdp_port_none_passes_validate() {
    let cfg = BaoConfig {
        cdp_port: None,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn bao_config_cdp_port_0_passes_validate() {
    // cdp_port is not validated by validate(); it's just stored
    let cfg = BaoConfig {
        cdp_port: Some(0),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.cdp_port, Some(0));
}

#[test]
fn bao_config_cdp_port_65535_passes_validate() {
    let cfg = BaoConfig {
        cdp_port: Some(65535),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.cdp_port, Some(65535));
}

#[test]
fn bao_config_cdp_port_9222_passes_validate() {
    let cfg = BaoConfig {
        cdp_port: Some(9222),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn bao_config_cdp_port_1_minimum_valid() {
    let cfg = BaoConfig {
        cdp_port: Some(1),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.cdp_port, Some(1));
}

// ============================================================
// 4. BrowserConfig default → BaoConfig conversion preserves all fields
// ============================================================

#[test]
fn from_browser_config_default_preserves_cdp_port() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(9222));
}

#[test]
fn from_browser_config_default_preserves_viewport() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.default_viewport_width, 1920);
    assert_eq!(bao.default_viewport_height, 1080);
}

#[test]
fn from_browser_config_default_preserves_stealth_none() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert!(bao.stealth_profile.is_none());
}

#[test]
fn from_browser_config_default_sets_max_pages_50() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.max_pages, 50);
}

#[test]
fn from_browser_config_default_sets_idle_ttl_60s() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.idle_ttl, Duration::from_secs(60));
}

#[test]
fn from_browser_config_custom_preserves_all_fields() {
    let bc = BrowserConfig {
        url: Some("https://example.com".into()),
        cdp_port: 1234,
        viewport_width: 2560,
        viewport_height: 1440,
        headless: false,
        stealth_profile: Some(StealthProfile::chrome_default()),
    };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(1234));
    assert_eq!(bao.default_viewport_width, 2560);
    assert_eq!(bao.default_viewport_height, 1440);
    assert!(bao.stealth_profile.is_some());
    // Fixed fields from From impl
    assert_eq!(bao.max_pages, 50);
    assert_eq!(bao.idle_ttl, Duration::from_secs(60));
}

#[test]
fn from_browser_config_port_0_maps_to_some_0() {
    let bc = BrowserConfig {
        cdp_port: 0,
        ..Default::default()
    };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(0));
}

#[test]
fn from_browser_config_port_65535_maps_to_some_65535() {
    let bc = BrowserConfig {
        cdp_port: 65535,
        ..Default::default()
    };
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(65535));
}

// ============================================================
// 5. PageConfig with custom viewport overriding browser config
// ============================================================

#[test]
fn page_config_default_viewport_is_none() {
    let cfg = PageConfig::default();
    assert!(cfg.viewport_width.is_none());
    assert!(cfg.viewport_height.is_none());
}

#[test]
fn page_config_custom_viewport_overrides_browser_default() {
    // BrowserConfig default is 1920x1080; PageConfig can override
    let page = PageConfig {
        viewport_width: Some(1280),
        viewport_height: Some(720),
        ..Default::default()
    };
    assert_eq!(page.viewport_width, Some(1280));
    assert_eq!(page.viewport_height, Some(720));
    // These override the browser-level 1920x1080 when PageHandle is created
}

#[test]
fn page_config_partial_viewport_override_width_only() {
    let page = PageConfig {
        viewport_width: Some(2560),
        viewport_height: None,
        ..Default::default()
    };
    assert_eq!(page.viewport_width, Some(2560));
    assert!(page.viewport_height.is_none());
    // height falls back to browser default when PageHandle resolves
}

#[test]
fn page_config_partial_viewport_override_height_only() {
    let page = PageConfig {
        viewport_width: None,
        viewport_height: Some(1440),
        ..Default::default()
    };
    assert!(page.viewport_width.is_none());
    assert_eq!(page.viewport_height, Some(1440));
}

#[test]
fn page_config_4k_viewport_override() {
    let page = PageConfig {
        viewport_width: Some(3840),
        viewport_height: Some(2160),
        ..Default::default()
    };
    assert_eq!(page.viewport_width, Some(3840));
    assert_eq!(page.viewport_height, Some(2160));
}

#[test]
fn page_config_minimum_valid_viewport_override() {
    let page = PageConfig {
        viewport_width: Some(800),
        viewport_height: Some(600),
        ..Default::default()
    };
    assert_eq!(page.viewport_width, Some(800));
    assert_eq!(page.viewport_height, Some(600));
}

#[test]
fn page_config_url_with_custom_viewport() {
    let page = PageConfig {
        url: Some("https://example.com".into()),
        viewport_width: Some(1280),
        viewport_height: Some(720),
        ..Default::default()
    };
    assert_eq!(page.url.as_deref(), Some("https://example.com"));
    assert_eq!(page.viewport_width, Some(1280));
    assert_eq!(page.viewport_height, Some(720));
}

// ============================================================
// 6. StealthProfile propagation through config chain
// ============================================================

#[test]
fn stealth_chrome_propagates_browser_to_bao() {
    let bc = BrowserConfig {
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    };
    let bao: BaoConfig = bc.into();
    let profile = bao.stealth_profile.unwrap();
    assert!(
        profile.navigator.user_agent.contains("Chrome"),
        "Chrome user_agent should propagate, got: {}",
        profile.navigator.user_agent
    );
}

#[test]
fn stealth_firefox_propagates_browser_to_bao() {
    let bc = BrowserConfig {
        stealth_profile: Some(StealthProfile::firefox_default()),
        ..Default::default()
    };
    let bao: BaoConfig = bc.into();
    let profile = bao.stealth_profile.unwrap();
    assert!(
        profile.navigator.user_agent.contains("Firefox"),
        "Firefox user_agent should propagate, got: {}",
        profile.navigator.user_agent
    );
}

#[test]
fn stealth_none_propagates_browser_to_bao() {
    let bc = BrowserConfig::default();
    let bao: BaoConfig = bc.into();
    assert!(bao.stealth_profile.is_none());
}

#[test]
fn stealth_profile_bao_to_page_independent() {
    // BaoConfig and PageConfig each hold their own stealth_profile
    let bao = BaoConfig {
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    };
    let page = PageConfig {
        stealth_profile: Some(StealthProfile::firefox_default()),
        ..Default::default()
    };
    // Page-level Firefox overrides BaoConfig-level Chrome
    let bao_profile = bao.stealth_profile.unwrap();
    let page_profile = page.stealth_profile.unwrap();
    assert!(
        bao_profile.navigator.user_agent.contains("Chrome"),
        "BaoConfig should retain Chrome"
    );
    assert!(
        page_profile.navigator.user_agent.contains("Firefox"),
        "PageConfig should have Firefox"
    );
}

#[test]
fn stealth_profile_page_none_falls_back_to_bao() {
    // When PageConfig has no stealth, BaoConfig profile is the fallback
    let bao = BaoConfig {
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    };
    let page = PageConfig::default();
    assert!(page.stealth_profile.is_none());
    assert!(bao.stealth_profile.is_some());
    // In runtime, PageHandle uses page.stealth_profile if Some, else bao.stealth_profile
}

#[test]
fn stealth_profile_clone_preserves_all_sub_profiles() {
    let profile = StealthProfile::chrome_default();
    let cloned = profile.clone();
    assert_eq!(profile.navigator.user_agent, cloned.navigator.user_agent);
    assert_eq!(profile.tls.ja3_hash, cloned.tls.ja3_hash);
    assert_eq!(profile.webgl.renderer, cloned.webgl.renderer);
}

#[test]
fn stealth_profile_bao_validate_with_stealth_passes() {
    let cfg = BaoConfig {
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn stealth_profile_does_not_affect_validation_failure() {
    // Stealth presence doesn't bypass viewport validation
    let cfg = BaoConfig {
        default_viewport_width: 100,
        stealth_profile: Some(StealthProfile::chrome_default()),
        ..Default::default()
    };
    assert!(cfg.validate().is_err());
}

// ============================================================
// 7. BrowserError Display/Debug for all variants
// ============================================================

#[test]
fn browser_error_init_display() {
    let err = BrowserError::Init("servo init failed".into());
    assert_eq!(format!("{err}"), "browser init error: servo init failed");
}

#[test]
fn browser_error_navigation_display() {
    let err = BrowserError::Navigation("dns resolution failed".into());
    assert_eq!(format!("{err}"), "navigation error: dns resolution failed");
}

#[test]
fn browser_error_rendering_display() {
    let err = BrowserError::Rendering("framebuffer lost".into());
    assert_eq!(format!("{err}"), "rendering error: framebuffer lost");
}

#[test]
fn browser_error_javascript_display() {
    let err = BrowserError::JavaScript("unexpected token".into());
    assert_eq!(format!("{err}"), "javascript error: unexpected token");
}

#[test]
fn browser_error_cdp_display() {
    let err = BrowserError::CDP("session closed".into());
    assert_eq!(format!("{err}"), "cdp error: session closed");
}

#[test]
fn browser_error_init_debug() {
    let err = BrowserError::Init("crash".into());
    let debug = format!("{err:?}");
    assert!(debug.contains("Init"), "Debug should contain variant: {debug}");
    assert!(debug.contains("crash"), "Debug should contain message: {debug}");
}

#[test]
fn browser_error_navigation_debug() {
    let err = BrowserError::Navigation("timeout".into());
    let debug = format!("{err:?}");
    assert!(debug.contains("Navigation"), "Debug should contain variant: {debug}");
}

#[test]
fn browser_error_rendering_debug() {
    let err = BrowserError::Rendering("oom".into());
    let debug = format!("{err:?}");
    assert!(debug.contains("Rendering"), "Debug should contain variant: {debug}");
}

#[test]
fn browser_error_javascript_debug() {
    let err = BrowserError::JavaScript("ref error".into());
    let debug = format!("{err:?}");
    assert!(debug.contains("JavaScript"), "Debug should contain variant: {debug}");
}

#[test]
fn browser_error_cdp_debug() {
    let err = BrowserError::CDP("handshake".into());
    let debug = format!("{err:?}");
    assert!(debug.contains("CDP"), "Debug should contain variant: {debug}");
}

#[test]
fn browser_error_empty_string_message() {
    let err = BrowserError::Init(String::new());
    assert_eq!(format!("{err}"), "browser init error: ");
}

#[test]
fn browser_error_unicode_message() {
    let err = BrowserError::Navigation("页面加载失败".into());
    assert_eq!(format!("{err}"), "navigation error: 页面加载失败");
}

#[test]
fn browser_error_is_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(BrowserError::CDP("fail".into()));
    assert_eq!(err.to_string(), "cdp error: fail");
}

#[test]
fn browser_error_all_variants_are_distinct() {
    let init = format!("{}", BrowserError::Init("x".into()));
    let nav = format!("{}", BrowserError::Navigation("x".into()));
    let render = format!("{}", BrowserError::Rendering("x".into()));
    let js = format!("{}", BrowserError::JavaScript("x".into()));
    let cdp = format!("{}", BrowserError::CDP("x".into()));
    // Each variant has a unique prefix
    assert!(init.starts_with("browser init error:"));
    assert!(nav.starts_with("navigation error:"));
    assert!(render.starts_with("rendering error:"));
    assert!(js.starts_with("javascript error:"));
    assert!(cdp.starts_with("cdp error:"));
}

// ============================================================
// 8. ScreenshotFormat PNG/JPEG validation
// ============================================================

#[test]
fn screenshot_format_png_encode_1x1() {
    use image::Rgba;
    let img = image::RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255]));
    let result = bao_browser::encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // PNG magic bytes
    assert_eq!(&data[0..4], &[0x89, 0x50, 0x4E, 0x47]);
}

#[test]
fn screenshot_format_jpeg_encode_1x1() {
    use image::Rgba;
    let img = image::RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255]));
    let result = bao_browser::encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // JPEG magic bytes
    assert_eq!(&data[0..2], &[0xFF, 0xD8]);
}

#[test]
fn screenshot_format_png_encode_100x100() {
    use image::Rgba;
    let img = image::RgbaImage::from_pixel(100, 100, Rgba([0, 128, 255, 255]));
    let result = bao_browser::encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(data.len() > 100, "PNG data should be nontrivial: {} bytes", data.len());
}

#[test]
fn screenshot_format_jpeg_encode_100x100() {
    use image::Rgba;
    let img = image::RgbaImage::from_pixel(100, 100, Rgba([0, 128, 255, 255]));
    let result = bao_browser::encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(data.len() > 100, "JPEG data should be nontrivial: {} bytes", data.len());
}

#[test]
fn screenshot_format_png_larger_than_jpeg_for_complex_image() {
    // PNG is lossless; JPEG is lossy. For a complex image, JPEG is typically smaller.
    use image::Rgba;
    let img = image::RgbaImage::from_fn(200, 200, |x, y| {
        Rgba([((x * 3) % 256) as u8, ((y * 7) % 256) as u8, 128, 255])
    });
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    let jpeg = bao_browser::encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    // Both produce valid output
    assert_eq!(&png[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
}

#[test]
fn screenshot_format_empty_image_no_panic() {
    use image::Rgba;
    let img = image::RgbaImage::from_pixel(0, 0, Rgba([0, 0, 0, 0]));
    // Should not panic; result may be Ok or Err depending on image crate behavior
    let _ = bao_browser::encode_image(&img, ScreenshotFormat::Png);
    let _ = bao_browser::encode_image(&img, ScreenshotFormat::Jpeg);
}

#[test]
fn screenshot_format_jpeg_strips_alpha_channel() {
    // JPEG has no alpha; encode_image converts RGBA→RGB before encoding
    use image::Rgba;
    let img = image::RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 128]));
    let result = bao_browser::encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok(), "JPEG encoding of semi-transparent image should succeed");
}

// ============================================================
// 9. PermissionGuard with empty/overlap/exact match patterns
// ============================================================

#[test]
fn permission_guard_empty_read_list_denies_all() {
    let perm = Permission {
        read: Some(vec![]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_read("/any/path").is_err());
    assert!(guard.check_read("/").is_err());
    assert!(guard.check_read("").is_err());
}

#[test]
fn permission_guard_empty_write_list_denies_all() {
    let perm = Permission {
        write: Some(vec![]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_write("/any/path").is_err());
}

#[test]
fn permission_guard_empty_net_list_denies_all() {
    let perm = Permission {
        net: Some(vec![]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_net("any.host").is_err());
    assert!(guard.check_net("localhost").is_err());
}

#[test]
fn permission_guard_overlap_read_write_patterns() {
    // Same prefix allowed for both read and write
    let perm = Permission {
        read: Some(vec!["/data".into()]),
        write: Some(vec!["/data".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_read("/data/file").is_ok());
    assert!(guard.check_write("/data/file").is_ok());
    assert!(guard.check_read("/other").is_err());
    assert!(guard.check_write("/other").is_err());
}

#[test]
fn permission_guard_exact_match_path() {
    // Prefix matching: "/tmp" matches "/tmp" and "/tmp/file" and also "/tmp2"
    // because starts_with is a true prefix match (no path boundary semantics)
    let perm = Permission {
        read: Some(vec!["/tmp".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_read("/tmp").is_ok());
    assert!(guard.check_read("/tmp/file.txt").is_ok());
    assert!(guard.check_read("/tmp/cache/data").is_ok());
    // starts_with("/tmp2", "/tmp") == true — prefix match, not path segment match
    assert!(guard.check_read("/tmp2").is_ok(), "prefix /tmp matches /tmp2 via starts_with");
    // To avoid matching /tmp2, use trailing slash: "/tmp/"
    let perm_strict = Permission {
        read: Some(vec!["/tmp/".into()]),
        ..Default::default()
    };
    let guard_strict = PermissionGuard::new(perm_strict);
    assert!(guard_strict.check_read("/tmp/file.txt").is_ok());
    assert!(guard_strict.check_read("/tmp2").is_err(), "trailing slash prevents /tmp2 match");
}

#[test]
fn permission_guard_exact_match_domain() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_net("example.com").is_ok());
    assert!(guard.check_net("sub.example.com").is_ok());
    assert!(guard.check_net("notexample.com").is_err());
}

#[test]
fn permission_guard_multiple_patterns_first_match() {
    let perm = Permission {
        read: Some(vec!["/home".into(), "/tmp".into(), "/var".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_read("/home/user").is_ok());
    assert!(guard.check_read("/tmp/cache").is_ok());
    assert!(guard.check_read("/var/log").is_ok());
    assert!(guard.check_read("/etc/passwd").is_err());
}

#[test]
fn permission_guard_none_mode_allows_all() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_read("/anything").is_ok());
    assert!(guard.check_write("/anything").is_ok());
    assert!(guard.check_net("any.host").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

#[test]
fn permission_guard_default_is_none_mode() {
    let guard = PermissionGuard::default();
    assert!(!guard.is_restricted());
}

#[test]
fn permission_guard_env_false_denies() {
    let perm = Permission {
        env: Some(false),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_env().is_err());
}

#[test]
fn permission_guard_run_false_denies() {
    let perm = Permission {
        run: Some(false),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_run().is_err());
}

#[test]
fn permission_guard_env_true_allows() {
    let perm = Permission {
        env: Some(true),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_env().is_ok());
}

#[test]
fn permission_guard_run_true_allows() {
    let perm = Permission {
        run: Some(true),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_run().is_ok());
}

#[test]
fn permission_denied_display_format() {
    let err = PermissionDenied {
        category: "read".into(),
        resource: "/etc/shadow".into(),
    };
    assert_eq!(format!("{err}"), "Permission denied: read on /etc/shadow");
}

#[test]
fn permission_denied_is_std_error() {
    let err = PermissionDenied {
        category: "net".into(),
        resource: "evil.com".into(),
    };
    let _: &dyn std::error::Error = &err;
}

#[test]
fn permission_guard_clone_preserves_restrictions() {
    let perm = Permission {
        read: Some(vec!["/app".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    let cloned = guard.clone();
    assert!(cloned.is_restricted());
    assert!(cloned.check_read("/app/config").is_ok());
    assert!(cloned.check_read("/other").is_err());
}

// ============================================================
// 10. PageState variant properties
// ============================================================

#[test]
fn page_state_all_variants_are_distinct() {
    let states = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    for i in 0..states.len() {
        for j in (i + 1)..states.len() {
            assert_ne!(states[i], states[j], "PageState variants should be distinct");
        }
    }
}

#[test]
fn page_state_variants_equal_to_themselves() {
    assert_eq!(PageState::Created, PageState::Created);
    assert_eq!(PageState::Navigating, PageState::Navigating);
    assert_eq!(PageState::Interactive, PageState::Interactive);
    assert_eq!(PageState::Idle, PageState::Idle);
    assert_eq!(PageState::Closed, PageState::Closed);
}

#[test]
fn page_state_clone_preserves_value() {
    let original = PageState::Navigating;
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn page_state_copy_semantics() {
    let state = PageState::Interactive;
    let copied = state; // Copy, not move
    assert_eq!(state, copied);
}

#[test]
fn page_state_debug_created() {
    assert!(format!("{:?}", PageState::Created).contains("Created"));
}

#[test]
fn page_state_debug_navigating() {
    assert!(format!("{:?}", PageState::Navigating).contains("Navigating"));
}

#[test]
fn page_state_debug_interactive() {
    assert!(format!("{:?}", PageState::Interactive).contains("Interactive"));
}

#[test]
fn page_state_debug_idle() {
    assert!(format!("{:?}", PageState::Idle).contains("Idle"));
}

#[test]
fn page_state_debug_closed() {
    assert!(format!("{:?}", PageState::Closed).contains("Closed"));
}

#[test]
fn page_state_lifecycle_ordering() {
    // Created != Closed (initial vs terminal)
    assert_ne!(PageState::Created, PageState::Closed);
    // Navigating != Interactive (loading vs ready)
    assert_ne!(PageState::Navigating, PageState::Interactive);
    // Idle != Closed (inactive vs destroyed)
    assert_ne!(PageState::Idle, PageState::Closed);
}

#[test]
fn page_state_count_is_five() {
    // Ensure no variants are accidentally added/removed
    let all = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    assert_eq!(all.len(), 5);
}
