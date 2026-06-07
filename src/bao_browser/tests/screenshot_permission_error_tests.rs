// @trace TEST-BRW-020 [req:REQ-BRW-002,REQ-LIB-004,REQ-BRW-001] [level:unit]
// Deep tests: screenshot encode_image with PNG/JPEG, Permission is_*_allowed edge cases,
// BrowserError Display/Debug variants, PermissionGuard integration.

use bao_browser::{BrowserError, Permission, PermissionGuard, ScreenshotFormat, encode_image};
use image::RgbaImage;

// ---- encode_image PNG ----

#[test]
fn test_encode_image_png_basic() {
    let img = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // PNG magic bytes
    assert_eq!(&data[0..4], &[0x89, 0x50, 0x4E, 0x47]);
}

#[test]
fn test_encode_image_png_1x1() {
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
}

#[test]
fn test_encode_image_png_transparent() {
    let img = RgbaImage::from_pixel(4, 4, image::Rgba([128, 128, 128, 0]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
}

#[test]
fn test_encode_image_png_large() {
    let img = RgbaImage::from_pixel(1920, 1080, image::Rgba([100, 150, 200, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    assert!(result.unwrap().len() > 100);
}

#[test]
fn test_encode_image_png_white() {
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]));
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn test_encode_image_png_black() {
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn test_encode_image_png_deterministic() {
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([42, 84, 168, 255]));
    let d1 = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let d2 = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert_eq!(d1, d2);
}

// ---- encode_image JPEG ----

#[test]
fn test_encode_image_jpeg_basic() {
    let img = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // JPEG magic bytes
    assert_eq!(&data[0..2], &[0xFF, 0xD8]);
}

#[test]
fn test_encode_image_jpeg_1x1() {
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}

#[test]
fn test_encode_image_jpeg_large() {
    let img = RgbaImage::from_pixel(640, 480, image::Rgba([100, 150, 200, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}

#[test]
fn test_encode_image_jpeg_transparent_alpha() {
    // JPEG doesn't support alpha, should convert to RGB
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([128, 128, 128, 50]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}

// ---- PNG vs JPEG format differences ----

#[test]
fn test_encode_image_png_jpeg_different_output() {
    let img = RgbaImage::from_pixel(100, 100, image::Rgba([128, 64, 32, 255]));
    let png = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let jpeg = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    // Different formats produce different byte sequences
    assert_ne!(png, jpeg);
}

#[test]
fn test_encode_image_png_lossless_same_input() {
    let img = RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
    let d1 = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let d2 = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert_eq!(d1, d2);
}

// ---- Permission is_*_allowed edge cases ----

#[test]
fn test_permission_read_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_read_allowed("/any/path"));
    assert!(perm.is_read_allowed("/etc/passwd"));
    assert!(perm.is_read_allowed(""));
}

#[test]
fn test_permission_read_with_prefix() {
    let perm = Permission {
        read: Some(vec!["/home".into(), "/tmp".into()]),
        ..Default::default()
    };
    assert!(perm.is_read_allowed("/home/user/file"));
    assert!(perm.is_read_allowed("/tmp/test"));
    assert!(!perm.is_read_allowed("/etc/passwd"));
    assert!(!perm.is_read_allowed("/var/log"));
}

#[test]
fn test_permission_read_exact_match() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    assert!(perm.is_read_allowed("/home"));
    assert!(perm.is_read_allowed("/home/"));
    assert!(perm.is_read_allowed("/homeother")); // starts_with("/home") matches
    assert!(!perm.is_read_allowed("/etc/passwd"));
}

#[test]
fn test_permission_read_empty_list() {
    let perm = Permission {
        read: Some(vec![]),
        ..Default::default()
    };
    assert!(!perm.is_read_allowed("/any"));
}

#[test]
fn test_permission_write_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_write_allowed("/any/path"));
}

#[test]
fn test_permission_write_with_prefix() {
    let perm = Permission {
        write: Some(vec!["/tmp".into()]),
        ..Default::default()
    };
    assert!(perm.is_write_allowed("/tmp/output"));
    assert!(!perm.is_write_allowed("/home/user/file"));
}

#[test]
fn test_permission_net_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_net_allowed("evil.com"));
    assert!(perm.is_net_allowed("example.com"));
}

#[test]
fn test_permission_net_exact_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("example.com"));
    assert!(!perm.is_net_allowed("evil.com"));
}

#[test]
fn test_permission_net_subdomain_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("sub.example.com"));
    assert!(perm.is_net_allowed("a.b.example.com"));
}

#[test]
fn test_permission_net_partial_not_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(!perm.is_net_allowed("notexample.com"));
    assert!(!perm.is_net_allowed("xnotexample.com"));
}

#[test]
fn test_permission_net_empty_list() {
    let perm = Permission {
        net: Some(vec![]),
        ..Default::default()
    };
    assert!(!perm.is_net_allowed("any.com"));
}

#[test]
fn test_permission_env_none_allowed() {
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

#[test]
fn test_permission_run_none_allowed() {
    let perm = Permission::default();
    assert!(perm.is_run_allowed());
}

#[test]
fn test_permission_run_explicit_false() {
    let perm = Permission { run: Some(false), ..Default::default() };
    assert!(!perm.is_run_allowed());
}

// ---- PermissionGuard edge cases ----

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

#[test]
fn test_permission_guard_restricted() {
    let perm = Permission {
        read: Some(vec!["/safe".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
    assert!(guard.check_read("/safe/file").is_ok());
    assert!(guard.check_read("/unsafe").is_err());
}

#[test]
fn test_permission_guard_default_is_none() {
    let guard = PermissionGuard::default();
    assert!(!guard.is_restricted());
}

#[test]
fn test_permission_guard_debug() {
    let guard = PermissionGuard::none();
    let debug = format!("{:?}", guard);
    assert!(!debug.is_empty());
}

#[test]
fn test_permission_guard_clone() {
    let guard = PermissionGuard::none();
    let cloned = guard.clone();
    assert!(!cloned.is_restricted());
}

// ---- Permission clone/debug ----

#[test]
fn test_permission_clone() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        write: Some(vec!["/tmp".into()]),
        net: Some(vec!["example.com".into()]),
        env: Some(false),
        run: Some(true),
        ..Default::default()
    };
    let cloned = perm.clone();
    assert!(cloned.is_read_allowed("/home/user"));
    assert!(!cloned.is_env_allowed());
    assert!(cloned.is_run_allowed());
}

#[test]
fn test_permission_debug() {
    let perm = Permission {
        read: Some(vec!["/test".into()]),
        ..Default::default()
    };
    let debug = format!("{:?}", perm);
    assert!(debug.contains("read"));
}

// ---- BrowserError ----

#[test]
fn test_browser_error_init() {
    let err = BrowserError::Init("failed".into());
    assert_eq!(format!("{}", err), "browser init error: failed");
}

#[test]
fn test_browser_error_navigation() {
    let err = BrowserError::Navigation("timeout".into());
    assert_eq!(format!("{}", err), "navigation error: timeout");
}

#[test]
fn test_browser_error_rendering() {
    let err = BrowserError::Rendering("gpu fail".into());
    assert_eq!(format!("{}", err), "rendering error: gpu fail");
}

#[test]
fn test_browser_error_javascript() {
    let err = BrowserError::JavaScript("syntax".into());
    assert_eq!(format!("{}", err), "javascript error: syntax");
}

#[test]
fn test_browser_error_cdp() {
    let err = BrowserError::CDP("ws closed".into());
    assert_eq!(format!("{}", err), "cdp error: ws closed");
}

#[test]
fn test_browser_error_debug() {
    let err = BrowserError::Init("test".into());
    let debug = format!("{:?}", err);
    assert!(debug.contains("Init") || debug.contains("test"));
}

#[test]
fn test_browser_error_all_variants_debug() {
    let variants = [
        format!("{:?}", BrowserError::Init("a".into())),
        format!("{:?}", BrowserError::Navigation("b".into())),
        format!("{:?}", BrowserError::Rendering("c".into())),
        format!("{:?}", BrowserError::JavaScript("d".into())),
        format!("{:?}", BrowserError::CDP("e".into())),
    ];
    // All variants have distinct debug output
    for i in 0..variants.len() {
        for j in (i+1)..variants.len() {
            assert_ne!(variants[i], variants[j]);
        }
    }
}

#[test]
fn test_browser_error_is_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(BrowserError::Init("test".into()));
    assert!(!err.to_string().is_empty());
}

#[test]
fn test_browser_error_empty_message() {
    let err = BrowserError::Init(String::new());
    assert_eq!(format!("{}", err), "browser init error: ");
}
