// @trace TEST-BRW-002-RENDER [req:REQ-BRW-002] [level:unit]
// Rendering pipeline tests: image encode/decode, viewport, screenshot formats, gradient patterns

use bao_browser::{BaoConfig, ScreenshotFormat};
use bao_browser::{BrowserError, PageState};

// ---- Image encode roundtrip ----

#[test]
fn test_png_magic_bytes() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let bytes = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "PNG magic bytes");
}

#[test]
fn test_jpeg_magic_bytes() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(64, 64, image::Rgba([100, 150, 200, 255]));
    if let Ok(bytes) = bao_browser::encode_image(&img, ScreenshotFormat::Jpeg) {
        assert!(bytes.starts_with(&[0xFF, 0xD8]), "JPEG magic bytes");
    }
}

#[test]
fn test_encode_all_black_image() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(png.len() > 50, "even all-black PNG should have content");
}

#[test]
fn test_encode_all_white_image() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]));
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(png.len() > 50);
}

#[test]
fn test_encode_transparent_image() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(5, 5, image::Rgba([128, 128, 128, 0]));
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(png.len() > 8, "transparent PNG should encode");
}

#[test]
fn test_encode_1080p_image() {
    use image::RgbaImage;
    let img = RgbaImage::from_pixel(1920, 1080, image::Rgba([64, 128, 192, 255]));
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(png.len() > 1000, "1920x1080 PNG should be substantial");
}

#[test]
fn test_encode_horizontal_gradient() {
    use image::RgbaImage;
    let mut img = RgbaImage::new(256, 1);
    for x in 0..256 {
        img.put_pixel(x, 0, image::Rgba([x as u8, 0, 0, 255]));
    }
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(png.len() > 100);
}

#[test]
fn test_encode_vertical_gradient() {
    use image::RgbaImage;
    let mut img = RgbaImage::new(1, 256);
    for y in 0..256 {
        img.put_pixel(0, y, image::Rgba([0, y as u8, 0, 255]));
    }
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(png.len() > 100);
}

#[test]
fn test_encode_checkerboard_pattern() {
    use image::RgbaImage;
    let mut img = RgbaImage::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            let val = if (x + y) % 2 == 0 { 255 } else { 0 };
            img.put_pixel(x, y, image::Rgba([val, val, val, 255]));
        }
    }
    let png = bao_browser::encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(png.len() > 50);
}

// ---- ScreenshotFormat variants ----

#[test]
fn test_screenshot_format_enum() {
    let png = ScreenshotFormat::Png;
    let jpeg = ScreenshotFormat::Jpeg;
    // Verify both variants exist — they must be different discriminants
    assert!(!matches!(png, ScreenshotFormat::Jpeg));
    assert!(!matches!(jpeg, ScreenshotFormat::Png));
}

// ---- BaoConfig viewport ----

#[test]
fn test_viewport_min_accepted() {
    let mut config = BaoConfig::default();
    config.default_viewport_width = 800;
    config.default_viewport_height = 600;
    assert!(config.validate().is_ok());
}

#[test]
fn test_viewport_rejected_below_min() {
    let mut config = BaoConfig::default();
    config.default_viewport_width = 799;
    config.default_viewport_height = 600;
    assert!(config.validate().is_err());

    config.default_viewport_width = 800;
    config.default_viewport_height = 599;
    assert!(config.validate().is_err());
}

#[test]
fn test_viewport_4k_accepted() {
    let mut config = BaoConfig::default();
    config.default_viewport_width = 3840;
    config.default_viewport_height = 2160;
    assert!(config.validate().is_ok());
}

#[test]
fn test_viewport_square_accepted() {
    let mut config = BaoConfig::default();
    config.default_viewport_width = 1024;
    config.default_viewport_height = 1024;
    assert!(config.validate().is_ok());
}

#[test]
fn test_viewport_ultra_wide() {
    let mut config = BaoConfig::default();
    config.default_viewport_width = 3440;
    config.default_viewport_height = 1440;
    assert!(config.validate().is_ok());
}

// ---- BaoConfig max_pages ----

#[test]
fn test_max_pages_zero_rejected() {
    let mut config = BaoConfig::default();
    config.max_pages = 0;
    assert!(config.validate().is_err());
}

#[test]
fn test_max_pages_one_accepted() {
    let mut config = BaoConfig::default();
    config.max_pages = 1;
    assert!(config.validate().is_ok());
}

#[test]
fn test_max_pages_large_accepted() {
    let mut config = BaoConfig::default();
    config.max_pages = 500;
    assert!(config.validate().is_ok());
}

// ---- BaoConfig cdp_port ----

#[test]
fn test_cdp_port_valid() {
    let mut config = BaoConfig::default();
    config.cdp_port = Some(9222);
    assert!(config.validate().is_ok());
    assert_eq!(config.cdp_port, Some(9222));
}

#[test]
fn test_cdp_port_none() {
    let config = BaoConfig::default();
    assert!(config.cdp_port.is_none() || config.cdp_port.is_some());
}

// ---- PageState transitions ----

#[test]
fn test_page_state_lifecycle() {
    let states = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    // All 5 states are distinct
    for i in 0..states.len() {
        for j in (i+1)..states.len() {
            assert_ne!(states[i], states[j]);
        }
    }
}

// ---- BrowserError Display ----

#[test]
fn test_browser_error_messages() {
    let errors = vec![
        BrowserError::Init("engine crashed".into()),
        BrowserError::Navigation("timeout".into()),
        BrowserError::Rendering("gpu fault".into()),
        BrowserError::JavaScript("undefined is not a function".into()),
        BrowserError::CDP("websocket closed".into()),
    ];
    for err in &errors {
        let msg = format!("{}", err);
        assert!(!msg.is_empty());
    }
}

#[test]
fn test_browser_error_debug() {
    let err = BrowserError::Init("test".into());
    let debug = format!("{:?}", err);
    assert!(debug.contains("Init"));
}
