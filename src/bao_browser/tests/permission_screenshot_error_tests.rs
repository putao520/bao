// @trace TEST-BRW-014-PERM-SCREENSHOT [req:REQ-LIB-003,REQ-LIB-004,REQ-BRW-001,REQ-BRW-002] [level:unit]
// Permission sandbox deep tests + screenshot encoding + BrowserError variants.

use bao_browser::{Permission, PermissionGuard, PermissionDenied, BrowserError};
use bao_browser::{encode_image, ScreenshotFormat};

// ---- Permission: read ----

#[test]
fn test_permission_read_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_read_allowed("/any/path"));
    assert!(perm.is_read_allowed("/etc/passwd"));
    assert!(perm.is_read_allowed(""));
}

#[test]
fn test_permission_read_whitelist() {
    let perm = Permission { read: Some(vec!["/data/".into(), "/tmp/".into()]), ..Default::default() };
    assert!(perm.is_read_allowed("/data/file.txt"));
    assert!(perm.is_read_allowed("/tmp/cache"));
    assert!(!perm.is_read_allowed("/etc/passwd"));
    assert!(!perm.is_read_allowed("/data")); // no trailing slash, "/data/" prefix doesn't match
    assert!(perm.is_read_allowed("/data/")); // matches
}

#[test]
fn test_permission_read_empty_whitelist_blocks_all() {
    let perm = Permission { read: Some(vec![]), ..Default::default() };
    assert!(!perm.is_read_allowed("/anything"));
    assert!(!perm.is_read_allowed(""));
}

#[test]
fn test_permission_read_long_path() {
    let perm = Permission { read: Some(vec!["/data/".into()]), ..Default::default() };
    let long_path = format!("/data/{}", "a".repeat(10000));
    assert!(perm.is_read_allowed(&long_path));
}

// ---- Permission: write ----

#[test]
fn test_permission_write_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_write_allowed("/any/path"));
}

#[test]
fn test_permission_write_whitelist() {
    let perm = Permission { write: Some(vec!["/out/".into()]), ..Default::default() };
    assert!(perm.is_write_allowed("/out/result.json"));
    assert!(!perm.is_write_allowed("/var/log"));
}

#[test]
fn test_permission_write_empty_blocks_all() {
    let perm = Permission { write: Some(vec![]), ..Default::default() };
    assert!(!perm.is_write_allowed("/anything"));
}

// ---- Permission: net ----

#[test]
fn test_permission_net_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_net_allowed("any.com"));
    assert!(perm.is_net_allowed("evil.example.com"));
}

#[test]
fn test_permission_net_whitelist() {
    let perm = Permission { net: Some(vec!["example.com".into()]), ..Default::default() };
    assert!(perm.is_net_allowed("example.com"));
    assert!(perm.is_net_allowed("sub.example.com"));
    assert!(perm.is_net_allowed("deep.sub.example.com"));
    assert!(!perm.is_net_allowed("notexample.com"));
    assert!(!perm.is_net_allowed("example.com.evil.com"));
}

#[test]
fn test_permission_net_empty_blocks_all() {
    let perm = Permission { net: Some(vec![]), ..Default::default() };
    assert!(!perm.is_net_allowed("any.com"));
}

#[test]
fn test_permission_net_subdomain_matching() {
    let perm = Permission { net: Some(vec!["api.example.com".into()]), ..Default::default() };
    assert!(perm.is_net_allowed("api.example.com"));
    assert!(perm.is_net_allowed("v2.api.example.com"));
    assert!(!perm.is_net_allowed("example.com"));
}

// ---- Permission: env ----

#[test]
fn test_permission_env_default_allowed() {
    let perm = Permission::default();
    assert!(perm.is_env_allowed());
}

#[test]
fn test_permission_env_explicit_true() {
    let perm = Permission { env: Some(true), ..Default::default() };
    assert!(perm.is_env_allowed());
}

#[test]
fn test_permission_env_false() {
    let perm = Permission { env: Some(false), ..Default::default() };
    assert!(!perm.is_env_allowed());
}

// ---- Permission: run ----

#[test]
fn test_permission_run_default_allowed() {
    let perm = Permission::default();
    assert!(perm.is_run_allowed());
}

#[test]
fn test_permission_run_false() {
    let perm = Permission { run: Some(false), ..Default::default() };
    assert!(!perm.is_run_allowed());
}

// ---- Permission: Clone + Debug ----

#[test]
fn test_permission_clone() {
    let perm = Permission {
        read: Some(vec!["/r/".into()]),
        write: Some(vec!["/w/".into()]),
        net: Some(vec!["h.com".into()]),
        env: Some(false),
        run: Some(false),
    };
    let cloned = perm.clone();
    assert!(cloned.is_read_allowed("/r/file"));
    assert!(!cloned.is_env_allowed());
}

#[test]
fn test_permission_debug() {
    let perm = Permission { read: Some(vec!["/x/".into()]), ..Default::default() };
    let debug = format!("{:?}", perm);
    assert!(debug.contains("read"));
}

// ---- PermissionGuard: none mode ----

#[test]
fn test_guard_none_allows_everything() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_read("/secret").is_ok());
    assert!(guard.check_write("/output").is_ok());
    assert!(guard.check_net("evil.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

// ---- PermissionGuard: restricted mode ----

#[test]
fn test_guard_restricted_read() {
    let guard = PermissionGuard::new(Permission {
        read: Some(vec!["/data/".into()]),
        ..Default::default()
    });
    assert!(guard.is_restricted());
    assert!(guard.check_read("/data/file").is_ok());
    assert!(guard.check_read("/etc/passwd").is_err());
}

#[test]
fn test_guard_restricted_write() {
    let guard = PermissionGuard::new(Permission {
        write: Some(vec!["/out/".into()]),
        ..Default::default()
    });
    assert!(guard.check_write("/out/file").is_ok());
    assert!(guard.check_write("/var/log").is_err());
}

#[test]
fn test_guard_restricted_net() {
    let guard = PermissionGuard::new(Permission {
        net: Some(vec!["safe.com".into()]),
        ..Default::default()
    });
    assert!(guard.check_net("safe.com").is_ok());
    assert!(guard.check_net("unsafe.com").is_err());
}

#[test]
fn test_guard_restricted_env() {
    let guard = PermissionGuard::new(Permission {
        env: Some(false),
        ..Default::default()
    });
    assert!(guard.check_env().is_err());
}

#[test]
fn test_guard_restricted_run() {
    let guard = PermissionGuard::new(Permission {
        run: Some(false),
        ..Default::default()
    });
    assert!(guard.check_run().is_err());
}

#[test]
fn test_guard_all_restricted() {
    let guard = PermissionGuard::new(Permission {
        read: Some(vec![]),
        write: Some(vec![]),
        net: Some(vec![]),
        env: Some(false),
        run: Some(false),
    });
    assert!(guard.check_read("/x").is_err());
    assert!(guard.check_write("/x").is_err());
    assert!(guard.check_net("x.com").is_err());
    assert!(guard.check_env().is_err());
    assert!(guard.check_run().is_err());
}

// ---- PermissionDenied ----

#[test]
fn test_permission_denied_display() {
    let err = PermissionDenied {
        category: "read".into(),
        resource: "/secret/file".into(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("read"));
    assert!(msg.contains("/secret/file"));
    assert!(msg.contains("Permission denied"));
}

#[test]
fn test_permission_denied_debug() {
    let err = PermissionDenied {
        category: "net".into(),
        resource: "evil.com".into(),
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("net"));
}

#[test]
fn test_permission_denied_is_error() {
    let err = PermissionDenied {
        category: "run".into(),
        resource: "*".into(),
    };
    let _: &dyn std::error::Error = &err;
}

// ---- PermissionGuard: Clone + Debug + Default ----

#[test]
fn test_guard_default_is_none() {
    let guard = PermissionGuard::default();
    assert!(!guard.is_restricted());
}

#[test]
fn test_guard_clone_preserves_state() {
    let guard = PermissionGuard::new(Permission {
        read: Some(vec!["/a/".into()]),
        ..Default::default()
    });
    let cloned = guard.clone();
    assert!(cloned.is_restricted());
    assert!(cloned.check_read("/a/file").is_ok());
    assert!(cloned.check_read("/b/file").is_err());
}

#[test]
fn test_guard_debug() {
    let guard = PermissionGuard::new(Permission::default());
    let debug = format!("{:?}", guard);
    assert!(debug.contains("PermissionGuard") || debug.contains("inner"));
}

// ---- BrowserError variants ----

#[test]
fn test_browser_error_init() {
    let err = BrowserError::Init("failed".into());
    let msg = format!("{}", err);
    assert!(msg.contains("init"));
    assert!(msg.contains("failed"));
}

#[test]
fn test_browser_error_navigation() {
    let err = BrowserError::Navigation("bad url".into());
    let msg = format!("{}", err);
    assert!(msg.contains("navigation"));
}

#[test]
fn test_browser_error_rendering() {
    let err = BrowserError::Rendering("gpu crash".into());
    let msg = format!("{}", err);
    assert!(msg.contains("rendering"));
}

#[test]
fn test_browser_error_javascript() {
    let err = BrowserError::JavaScript("syntax error".into());
    let msg = format!("{}", err);
    assert!(msg.contains("javascript"));
}

#[test]
fn test_browser_error_cdp() {
    let err = BrowserError::CDP("ws closed".into());
    let msg = format!("{}", err);
    assert!(msg.contains("cdp"));
}

#[test]
fn test_browser_error_is_std_error() {
    let err = BrowserError::Init("test".into());
    let _: &dyn std::error::Error = &err;
}

#[test]
fn test_browser_error_debug() {
    let err = BrowserError::Navigation("x".into());
    let debug = format!("{:?}", err);
    assert!(debug.contains("Navigation"));
}

// ---- Screenshot encoding ----

use image::RgbaImage;

#[test]
fn test_encode_png_small_image() {
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([128, 128, 128, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // PNG magic bytes
    assert_eq!(&data[0..4], &[0x89, 0x50, 0x4E, 0x47]);
}

#[test]
fn test_encode_jpeg_small_image() {
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([128, 128, 128, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // JPEG magic bytes
    assert_eq!(&data[0..2], &[0xFF, 0xD8]);
}

#[test]
fn test_encode_png_1x1() {
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
}

#[test]
fn test_encode_jpeg_1x1() {
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}

#[test]
fn test_encode_png_large_image() {
    let img = RgbaImage::from_pixel(1920, 1080, image::Rgba([100, 150, 200, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(data.len() > 1000);
}

#[test]
fn test_encode_png_transparent_pixels() {
    let img = RgbaImage::from_pixel(5, 5, image::Rgba([0, 0, 0, 0]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
}

#[test]
fn test_encode_jpeg_converts_rgba_to_rgb() {
    // RGBA with alpha should still encode (converted to RGB internally)
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([128, 128, 128, 128]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}
