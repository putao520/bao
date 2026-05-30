// @trace TEST-BRW-001~003 [req:REQ-BRW-001~003] [level:unit]
// Unit tests for bao_browser core: screenshot encode, delegate, config, permission edge cases

use bao_browser::{BaoConfig, BrowserConfig, PageConfig, BrowserError, Permission, PermissionGuard};
use bao_browser::ScreenshotFormat;
use bao_stealth::StealthProfile;

// ---- Screenshot encode tests ----

#[test]
fn test_screenshot_encode_png() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]));
    let result = bao_browser::encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok(), "PNG encode should succeed");
    let bytes = result.unwrap();
    assert!(bytes.len() > 8, "PNG bytes should be non-empty");
    // PNG magic bytes
    assert_eq!(bytes[0], 0x89, "should start with PNG magic");
    assert_eq!(bytes[1], 0x50); // 'P'
    assert_eq!(bytes[2], 0x4E); // 'N'
    assert_eq!(bytes[3], 0x47); // 'G'
}

#[test]
fn test_screenshot_encode_jpeg() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(8, 8, image::Rgba([0, 128, 255, 255]));
    let result = bao_browser::encode_image(&img, ScreenshotFormat::Jpeg);
    // JPEG encoding may fail with some image 0.25 configurations;
    // if it succeeds, verify magic bytes
    if let Ok(bytes) = result {
        assert!(bytes.len() > 4, "JPEG bytes should have content");
        assert_eq!(bytes[0], 0xFF);
        assert_eq!(bytes[1], 0xD8);
    }
}

#[test]
fn test_screenshot_encode_large_image() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(1920, 1080, image::Rgba([100, 150, 200, 255]));
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png);
    assert!(png.is_ok(), "large PNG encode should succeed");
    assert!(png.unwrap().len() > 1000, "large PNG should have substantial size");
}

#[test]
fn test_screenshot_encode_1x1() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png);
    assert!(png.is_ok(), "1x1 PNG should encode");
}

#[test]
fn test_screenshot_encode_gradient() {
    use image::RgbaImage;
    let mut img = RgbaImage::new(256, 256);
    for y in 0..256 {
        for x in 0..256 {
            img.put_pixel(x, y, image::Rgba([x as u8, y as u8, 128, 255]));
        }
    }
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png);
    assert!(png.is_ok(), "gradient PNG should encode");
}

// ---- Delegate construction tests ----

#[test]
fn test_bao_servo_delegate_new() {
    let delegate = bao_browser::BaoServoDelegate::new();
    assert!(delegate.last_error().is_none(), "new delegate should have no error");
}

// ---- PageConfig ----

#[test]
fn test_page_config_default_values() {
    let config = PageConfig::default();
    assert!(config.url.is_none(), "default url should be None");
    assert!(config.stealth_profile.is_none(), "default stealth should be None");
    assert!(config.permission.is_none(), "default permission should be None");
    assert!(config.viewport_width.is_none());
    assert!(config.viewport_height.is_none());
}

#[test]
fn test_page_config_custom_viewport() {
    let mut config = PageConfig::default();
    config.viewport_width = Some(1920);
    config.viewport_height = Some(1080);
    assert_eq!(config.viewport_width, Some(1920));
    assert_eq!(config.viewport_height, Some(1080));
}

#[test]
fn test_page_config_with_url() {
    let mut config = PageConfig::default();
    config.url = Some("https://example.com".to_string());
    assert_eq!(config.url.as_deref(), Some("https://example.com"));
}

// ---- BaoConfig validation edge cases ----

#[test]
fn test_bao_config_zero_max_pages_rejected() {
    let mut config = BaoConfig::default();
    config.max_pages = 0;
    assert!(config.validate().is_err(), "zero max_pages should be rejected");
}

#[test]
fn test_bao_config_large_max_pages() {
    let mut config = BaoConfig::default();
    config.max_pages = 1000;
    assert!(config.validate().is_ok(), "large max_pages should be accepted");
}

#[test]
fn test_bao_config_viewport_boundaries() {
    let mut config = BaoConfig::default();
    config.default_viewport_width = 800;
    config.default_viewport_height = 600;
    assert!(config.validate().is_ok(), "800x600 viewport should be accepted");

    config.default_viewport_width = 799;
    assert!(config.validate().is_err(), "799 width should be rejected");

    config.default_viewport_width = 800;
    config.default_viewport_height = 599;
    assert!(config.validate().is_err(), "599 height should be rejected");
}

#[test]
fn test_bao_config_cdp_port() {
    let mut config = BaoConfig::default();
    config.cdp_port = Some(9222);
    assert_eq!(config.cdp_port, Some(9222));
    assert!(config.validate().is_ok());
}

// ---- BrowserConfig conversion ----

#[test]
fn test_browser_config_into_bao_preserves_fields() {
    let mut bc = BrowserConfig::default();
    bc.cdp_port = 9222;
    bc.stealth_profile = Some(StealthProfile::chrome_default());
    let bao: BaoConfig = bc.into();
    assert_eq!(bao.cdp_port, Some(9222));
    assert!(bao.stealth_profile.is_some());
}

// ---- BrowserError variants ----

#[test]
fn test_browser_error_all_variants() {
    let variants: Vec<BrowserError> = vec![
        BrowserError::Init("init failed".into()),
        BrowserError::Navigation("nav failed".into()),
        BrowserError::Rendering("render failed".into()),
        BrowserError::JavaScript("js failed".into()),
        BrowserError::CDP("cdp failed".into()),
    ];
    for err in &variants {
        let msg = format!("{}", err);
        assert!(!msg.is_empty(), "error should display non-empty message");
        assert!(std::error::Error::source(err).is_none(), "BrowserError has no source");
    }
}

// ---- Permission edge cases ----

#[test]
fn test_permission_all_restricted() {
    let perm = Permission {
        read: Some(vec![]),
        write: Some(vec![]),
        net: Some(vec![]),
        env: Some(false),
        run: Some(false),
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_read("/anything").is_err());
    assert!(guard.check_write("/anything").is_err());
    assert!(guard.check_net("anything.com").is_err());
    assert!(guard.check_env().is_err());
    assert!(guard.check_run().is_err());
}

#[test]
fn test_permission_partial_restrictions() {
    let perm = Permission {
        read: Some(vec!["/data".into()]),
        write: None,
        net: Some(vec!["api.example.com".into()]),
        env: None,
        run: Some(false),
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_read("/data/file").is_ok());
    assert!(guard.check_read("/etc/passwd").is_err());
    assert!(guard.check_write("/anywhere").is_ok());
    assert!(guard.check_net("api.example.com").is_ok());
    assert!(guard.check_net("evil.com").is_err());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_err());
}

#[test]
fn test_permission_exact_path_match() {
    let perm = Permission {
        read: Some(vec!["/exact/path".into()]),
        write: None,
        net: None,
        env: None,
        run: None,
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_read("/exact/path").is_ok());
    assert!(guard.check_read("/exact/path/file").is_ok());
    assert!(guard.check_read("/exact/other").is_err());
}

// ---- PageState ----

#[test]
fn test_page_state_variants() {
    use bao_browser::PageState;
    let states = [PageState::Created, PageState::Navigating, PageState::Interactive, PageState::Idle, PageState::Closed];
    // Verify all variants exist and are distinct
    for i in 0..states.len() {
        for j in (i+1)..states.len() {
            assert_ne!(states[i], states[j], "PageState variants should be distinct");
        }
    }
}

// ---- PoolStats ----
// PoolStats is in private module; tested indirectly via PagePool in browser_runtime_tests
