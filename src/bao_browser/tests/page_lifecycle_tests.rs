// @trace TEST-LIB-006-LIFECYCLE [req:REQ-LIB-001,REQ-LIB-002,REQ-BRW-001,REQ-BRW-002] [level:unit]
// PageHandle lifecycle + rendering pipeline + PageState + error path tests
// NOTE: servo Opts is single-init, so BaoRuntime tests remain in cross_crate_integration_tests.rs
// This file tests pure data types and encode_image only.

use bao_browser::{BrowserError, PageState, Permission, PermissionGuard};

// ---- PageState transitions ----

#[test]
fn test_page_state_variants_distinct() {
    let states = [PageState::Created, PageState::Navigating, PageState::Interactive, PageState::Idle, PageState::Closed];
    for i in 0..states.len() {
        for j in (i+1)..states.len() {
            assert_ne!(states[i], states[j], "{:?} should differ from {:?}", states[i], states[j]);
        }
    }
}

#[test]
fn test_page_state_debug() {
    assert!(format!("{:?}", PageState::Created).contains("Created"));
    assert!(format!("{:?}", PageState::Navigating).contains("Navigating"));
    assert!(format!("{:?}", PageState::Interactive).contains("Interactive"));
    assert!(format!("{:?}", PageState::Idle).contains("Idle"));
    assert!(format!("{:?}", PageState::Closed).contains("Closed"));
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

// ---- BrowserError comprehensive tests ----

#[test]
fn test_browser_error_all_variants_display() {
    let errors = vec![
        BrowserError::Init("init failed".into()),
        BrowserError::Navigation("nav error".into()),
        BrowserError::Rendering("render error".into()),
        BrowserError::JavaScript("js error".into()),
        BrowserError::CDP("cdp error".into()),
    ];
    let displays: Vec<String> = errors.iter().map(|e| format!("{}", e)).collect();
    assert!(displays[0].contains("init"));
    assert!(displays[1].contains("navigation"));
    assert!(displays[2].contains("rendering"));
    assert!(displays[3].contains("javascript"));
    assert!(displays[4].contains("cdp"));
}

#[test]
fn test_browser_error_debug_all_variants() {
    let err = BrowserError::JavaScript("debug test".into());
    let debug = format!("{:?}", err);
    assert!(debug.contains("JavaScript"));
}

#[test]
fn test_browser_error_is_std_error() {
    let err = BrowserError::CDP("std error test".into());
    let _: &dyn std::error::Error = &err;
}

#[test]
fn test_browser_error_init() {
    let err = BrowserError::Init("test".into());
    let display = format!("{}", err);
    assert!(display.contains("init"));
    assert!(display.contains("test"));
}

#[test]
fn test_browser_error_navigation_empty_msg() {
    let err = BrowserError::Navigation(String::new());
    let display = format!("{}", err);
    assert!(display.contains("navigation"));
}

#[test]
fn test_browser_error_rendering_unicode() {
    let err = BrowserError::Rendering("渲染失败 🎨".into());
    let display = format!("{}", err);
    assert!(display.contains("渲染失败"));
}

#[test]
fn test_browser_error_long_message() {
    let long_msg = "x".repeat(10000);
    let err = BrowserError::JavaScript(long_msg.clone());
    let display = format!("{}", err);
    assert!(display.contains(&long_msg));
}

// ---- ScreenshotFormat tests ----

#[test]
fn test_screenshot_format_png_encode() {
    use bao_browser::{encode_image, ScreenshotFormat};
    use image::RgbaImage;

    // 2x2 white image
    let img = RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    // PNG magic bytes
    assert_eq!(&data[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    assert!(data.len() > 10);
}

#[test]
fn test_screenshot_format_jpeg_encode() {
    use bao_browser::{encode_image, ScreenshotFormat};
    use image::RgbaImage;

    let img = RgbaImage::from_pixel(4, 4, image::Rgba([128, 64, 32, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    // JPEG magic bytes
    assert_eq!(&data[0..2], &[0xFF, 0xD8]);
}

#[test]
fn test_screenshot_format_large_image() {
    use bao_browser::{encode_image, ScreenshotFormat};
    use image::RgbaImage;

    // 1920x1080 black image
    let img = RgbaImage::from_pixel(1920, 1080, image::Rgba([0, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    assert!(result.unwrap().len() > 100);
}

#[test]
fn test_screenshot_format_gradient_image() {
    use bao_browser::{encode_image, ScreenshotFormat};
    use image::{Rgba, RgbaImage};

    let mut img = RgbaImage::new(256, 256);
    for y in 0..256 {
        for x in 0..256 {
            img.put_pixel(x, y, Rgba([x as u8, y as u8, 128, 255]));
        }
    }
    let png = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let jpeg = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(png.len() > 100);
    assert!(jpeg.len() > 100);
}

#[test]
fn test_screenshot_format_transparent_image() {
    use bao_browser::{encode_image, ScreenshotFormat};
    use image::RgbaImage;

    // Fully transparent image
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 0]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    // PNG supports transparency
    assert!(result.unwrap().len() > 10);
}

#[test]
fn test_screenshot_format_1x1_pixel() {
    use bao_browser::{encode_image, ScreenshotFormat};
    use image::RgbaImage;

    let img = RgbaImage::from_pixel(1, 1, image::Rgba([42, 43, 44, 255]));
    let png = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let jpeg = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(png.len() > 10);
    assert!(jpeg.len() > 10);
}

// ---- PermissionGuard edge cases ----

#[test]
fn test_permission_guard_all_allowed() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_read("/etc/passwd").is_ok());
    assert!(guard.check_write("/tmp/out").is_ok());
    assert!(guard.check_net("https://evil.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

#[test]
fn test_permission_guard_read_only() {
    let perm = Permission {
        net: Some(vec![]),
        read: Some(vec!["/data/".into()]),
        write: Some(vec![]),
        env: Some(false),
        run: Some(false),
    ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_read("/data/file.txt").is_ok());
    assert!(guard.check_read("/other/file").is_err());
    assert!(guard.check_write("/data/file").is_err());
    assert!(guard.check_net("safe.com").is_err());
    assert!(guard.check_env().is_err());
    assert!(guard.check_run().is_err());
}

#[test]
fn test_permission_guard_net_whitelist() {
    let perm = Permission {
        net: Some(vec!["api.example.com".into(), "cdn.example.com".into()]),
        read: None,
        write: None,
        env: None,
        run: None,
    ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_net("api.example.com").is_ok());
    assert!(guard.check_net("cdn.example.com").is_ok());
    assert!(guard.check_net("evil.com").is_err());
    assert!(guard.check_net("api.example.com.evil.com").is_err());
    assert!(guard.check_read("/any").is_ok());
    assert!(guard.check_write("/any").is_ok());
}

#[test]
fn test_permission_guard_empty_net_blocks_all() {
    let perm = Permission {
        net: Some(vec![]),
        read: None,
        write: None,
        env: None,
        run: None,
    ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_net("any.com").is_err());
}

#[test]
fn test_permission_guard_clone() {
    let perm = Permission {
        net: Some(vec!["safe.com".into()]),
        read: Some(vec!["/tmp/".into()]),
        write: Some(vec![]),
        env: Some(true),
        run: Some(false),
    ..Default::default()
    };
    let g1 = PermissionGuard::new(perm);
    let g2 = g1.clone();
    assert_eq!(g1.is_restricted(), g2.is_restricted());
    assert_eq!(g1.check_net("safe.com").is_ok(), g2.check_net("safe.com").is_ok());
    assert_eq!(g1.check_net("evil.com").is_err(), g2.check_net("evil.com").is_err());
}

// ---- BaoConfig default tests ----

#[test]
fn test_bao_config_defaults() {
    use bao_browser::BaoConfig;
    let config = BaoConfig::default();
    assert_eq!(config.max_pages, 50);
    assert_eq!(config.default_viewport_width, 1920);
    assert_eq!(config.default_viewport_height, 1080);
    assert!(config.stealth_profile.is_none());
    assert!(config.cdp_port.is_none());
}

#[test]
fn test_bao_config_custom() {
    use bao_browser::BaoConfig;
    use bao_stealth::StealthProfile;

    let config = BaoConfig {
        max_pages: 20,
        default_viewport_width: 1920,
        default_viewport_height: 1080,
        stealth_profile: Some(StealthProfile::chrome_default()),
        cdp_port: Some(9333),
        ..Default::default()
    };
    assert_eq!(config.max_pages, 20);
    assert!(config.stealth_profile.is_some());
    assert_eq!(config.cdp_port, Some(9333));
}

// ---- BrowserConfig conversion ----

#[test]
fn test_browser_config_into_bao_config() {
    use bao_browser::{BrowserConfig, BaoConfig};
    use bao_stealth::StealthProfile;

    let mut bc = BrowserConfig::default();
    bc.cdp_port = 9333;
    bc.viewport_width = 1920;
    bc.viewport_height = 1080;
    bc.headless = true;
    bc.url = Some("https://example.com".into());
    bc.stealth_profile = Some(StealthProfile::firefox_default());

    let config: BaoConfig = bc.into();
    assert_eq!(config.cdp_port, Some(9333));
    assert_eq!(config.default_viewport_width, 1920);
    assert_eq!(config.default_viewport_height, 1080);
    assert!(config.stealth_profile.is_some());
}

// ---- PageConfig defaults ----

#[test]
fn test_page_config_defaults() {
    use bao_browser::PageConfig;
    let config = PageConfig::default();
    assert!(config.url.is_none());
    assert!(config.stealth_profile.is_none());
    assert!(config.permission.is_none());
    assert!(config.viewport_width.is_none());
    assert!(config.viewport_height.is_none());
}

// ---- Permission default is all-None ----

#[test]
fn test_permission_default() {
    let perm = Permission::default();
    assert!(perm.net.is_none());
    assert!(perm.read.is_none());
    assert!(perm.write.is_none());
    assert!(perm.env.is_none());
    assert!(perm.run.is_none());
}
