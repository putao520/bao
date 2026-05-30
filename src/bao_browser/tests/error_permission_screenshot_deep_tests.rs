// @trace TEST-BRW-017 [req:REQ-BRW-001] [level:unit]
// @trace TEST-BRW-018 [req:REQ-LIB-003] [level:unit]
// BrowserError display/debug/error, PermissionDenied display/debug/error,
// ScreenshotFormat encode, PermissionGuard edge cases, Permission struct
// default and custom.

use bao_browser::{BrowserError, Permission, PermissionDenied, PermissionGuard, ScreenshotFormat};
use bao_browser::encode_image;
use image::RgbaImage;

// ---- BrowserError ----

#[test]
fn test_browser_error_init() {
    let err = BrowserError::Init("failed to start".into());
    assert_eq!(format!("{}", err), "browser init error: failed to start");
}

#[test]
fn test_browser_error_navigation() {
    let err = BrowserError::Navigation("timeout".into());
    assert_eq!(format!("{}", err), "navigation error: timeout");
}

#[test]
fn test_browser_error_rendering() {
    let err = BrowserError::Rendering("gpu crash".into());
    assert_eq!(format!("{}", err), "rendering error: gpu crash");
}

#[test]
fn test_browser_error_javascript() {
    let err = BrowserError::JavaScript("syntax error".into());
    assert_eq!(format!("{}", err), "javascript error: syntax error");
}

#[test]
fn test_browser_error_cdp() {
    let err = BrowserError::CDP("connection refused".into());
    assert_eq!(format!("{}", err), "cdp error: connection refused");
}

#[test]
fn test_browser_error_debug() {
    let err = BrowserError::Init("test".into());
    let debug = format!("{:?}", err);
    assert!(debug.contains("Init"));
    assert!(debug.contains("test"));
}

#[test]
fn test_browser_error_is_std_error() {
    let err = BrowserError::Navigation("test".into());
    let _: &dyn std::error::Error = &err;
}

#[test]
fn test_browser_error_empty_message() {
    let err = BrowserError::CDP(String::new());
    assert_eq!(format!("{}", err), "cdp error: ");
}

#[test]
fn test_browser_error_multiline_message() {
    let err = BrowserError::JavaScript("line1\nline2\nline3".into());
    let msg = format!("{}", err);
    assert!(msg.contains("line1"));
    assert!(msg.contains("line3"));
}

// ---- PermissionDenied ----

#[test]
fn test_permission_denied_display() {
    let pd = PermissionDenied {
        category: "read".into(),
        resource: "/etc/passwd".into(),
    };
    assert_eq!(format!("{}", pd), "Permission denied: read on /etc/passwd");
}

#[test]
fn test_permission_denied_debug() {
    let pd = PermissionDenied {
        category: "write".into(),
        resource: "/tmp".into(),
    };
    let debug = format!("{:?}", pd);
    assert!(debug.contains("write"));
    assert!(debug.contains("/tmp"));
}

#[test]
fn test_permission_denied_clone() {
    let pd = PermissionDenied {
        category: "net".into(),
        resource: "example.com".into(),
    };
    let cloned = pd.clone();
    assert_eq!(cloned.category, "net");
    assert_eq!(cloned.resource, "example.com");
}

#[test]
fn test_permission_denied_is_std_error() {
    let pd = PermissionDenied {
        category: "env".into(),
        resource: "*".into(),
    };
    let _: &dyn std::error::Error = &pd;
}

#[test]
fn test_permission_denied_empty_fields() {
    let pd = PermissionDenied {
        category: String::new(),
        resource: String::new(),
    };
    assert_eq!(format!("{}", pd), "Permission denied:  on ");
}

// ---- Permission ----

#[test]
fn test_permission_default_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_read_allowed("/any/path"));
    assert!(perm.is_write_allowed("/any/path"));
    assert!(perm.is_net_allowed("any.host.com"));
    assert!(perm.is_env_allowed());
    assert!(perm.is_run_allowed());
}

#[test]
fn test_permission_read_with_allowed_prefix() {
    let perm = Permission {
        read: Some(vec!["/home/user".into(), "/tmp".into()]),
        ..Default::default()
    };
    assert!(perm.is_read_allowed("/home/user/file.txt"));
    assert!(perm.is_read_allowed("/tmp/data"));
    assert!(!perm.is_read_allowed("/etc/passwd"));
}

#[test]
fn test_permission_write_with_allowed_prefix() {
    let perm = Permission {
        write: Some(vec!["/tmp".into()]),
        ..Default::default()
    };
    assert!(perm.is_write_allowed("/tmp/output"));
    assert!(!perm.is_write_allowed("/home/user/file"));
}

#[test]
fn test_permission_net_exact_and_subdomain() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("example.com"));
    assert!(perm.is_net_allowed("sub.example.com"));
    assert!(perm.is_net_allowed("a.b.example.com"));
    assert!(!perm.is_net_allowed("notexample.com"));
    assert!(!perm.is_net_allowed("example.com.evil.org"));
}

#[test]
fn test_permission_env_explicit_false() {
    let perm = Permission {
        env: Some(false),
        ..Default::default()
    };
    assert!(!perm.is_env_allowed());
}

#[test]
fn test_permission_env_explicit_true() {
    let perm = Permission {
        env: Some(true),
        ..Default::default()
    };
    assert!(perm.is_env_allowed());
}

#[test]
fn test_permission_run_explicit_false() {
    let perm = Permission {
        run: Some(false),
        ..Default::default()
    };
    assert!(!perm.is_run_allowed());
}

#[test]
fn test_permission_empty_allowed_list_denies() {
    let perm = Permission {
        read: Some(vec![]),
        write: Some(vec![]),
        net: Some(vec![]),
        ..Default::default()
    };
    assert!(!perm.is_read_allowed("/any"));
    assert!(!perm.is_write_allowed("/any"));
    assert!(!perm.is_net_allowed("any.com"));
}

#[test]
fn test_permission_clone() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    let cloned = perm.clone();
    assert!(cloned.is_read_allowed("/home/user"));
}

#[test]
fn test_permission_debug() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    let debug = format!("{:?}", perm);
    assert!(debug.contains("example.com"));
}

// ---- PermissionGuard ----

#[test]
fn test_guard_none_allows_all() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_read("/any").is_ok());
    assert!(guard.check_write("/any").is_ok());
    assert!(guard.check_net("any.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

#[test]
fn test_guard_new_is_restricted() {
    let guard = PermissionGuard::new(Permission::default());
    assert!(guard.is_restricted());
}

#[test]
fn test_guard_default_is_none() {
    let guard = PermissionGuard::default();
    assert!(!guard.is_restricted());
}

#[test]
fn test_guard_read_denied() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_read("/home/file").is_ok());
    assert!(guard.check_read("/etc/passwd").is_err());
    let err = guard.check_read("/etc/passwd").unwrap_err();
    assert_eq!(err.category, "read");
    assert_eq!(err.resource, "/etc/passwd");
}

#[test]
fn test_guard_write_denied() {
    let perm = Permission {
        write: Some(vec!["/tmp".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_write("/tmp/data").is_ok());
    assert!(guard.check_write("/root/.bashrc").is_err());
}

#[test]
fn test_guard_net_denied() {
    let perm = Permission {
        net: Some(vec!["allowed.com".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_net("allowed.com").is_ok());
    assert!(guard.check_net("denied.com").is_err());
}

#[test]
fn test_guard_env_denied() {
    let perm = Permission {
        env: Some(false),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_env().is_err());
}

#[test]
fn test_guard_run_denied() {
    let perm = Permission {
        run: Some(false),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_run().is_err());
}

#[test]
fn test_guard_clone() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    let cloned = guard.clone();
    assert!(cloned.is_restricted());
    assert!(cloned.check_read("/home/file").is_ok());
    assert!(cloned.check_read("/etc/passwd").is_err());
}

#[test]
fn test_guard_debug() {
    let guard = PermissionGuard::none();
    let debug = format!("{:?}", guard);
    assert!(debug.contains("PermissionGuard"));
}

// ---- Screenshot encode ----

#[test]
fn test_encode_png_1x1() {
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // PNG header
    assert!(data[0] == 0x89 && data[1] == 0x50);
}

#[test]
fn test_encode_jpeg_1x1() {
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // JPEG header
    assert!(data[0] == 0xFF && data[1] == 0xD8);
}

#[test]
fn test_encode_png_larger_image() {
    let img = RgbaImage::from_pixel(100, 100, image::Rgba([128, 128, 128, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    assert!(result.unwrap().len() > 100);
}

#[test]
fn test_encode_jpeg_larger_image() {
    let img = RgbaImage::from_pixel(64, 64, image::Rgba([200, 100, 50, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}

#[test]
fn test_encode_png_transparent_pixel() {
    let img = RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 0]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
}
