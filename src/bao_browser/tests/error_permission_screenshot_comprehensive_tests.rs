// @trace TEST-BRW-022 [req:REQ-BRW-001,REQ-BRW-002,REQ-LIB-003,REQ-LIB-004,REQ-CDP-007] [level:unit]
// BrowserError all variants + Display + Error trait,
// Permission field-level checks + PermissionGuard + PermissionDenied,
// ScreenshotFormat + encode_image with real RGBA data.

use bao_browser::{BrowserError, Permission, PermissionGuard, PermissionDenied};
use bao_browser::{ScreenshotFormat, encode_image};
use image::RgbaImage;

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
    let err = BrowserError::Navigation("timeout".into());
    let msg = format!("{}", err);
    assert!(msg.contains("navigation"));
    assert!(msg.contains("timeout"));
}

#[test]
fn test_browser_error_rendering() {
    let err = BrowserError::Rendering("gpu crash".into());
    let msg = format!("{}", err);
    assert!(msg.contains("rendering"));
    assert!(msg.contains("gpu crash"));
}

#[test]
fn test_browser_error_javascript() {
    let err = BrowserError::JavaScript("syntax".into());
    let msg = format!("{}", err);
    assert!(msg.contains("javascript"));
    assert!(msg.contains("syntax"));
}

#[test]
fn test_browser_error_cdp() {
    let err = BrowserError::CDP("ws closed".into());
    let msg = format!("{}", err);
    assert!(msg.contains("cdp"));
    assert!(msg.contains("ws closed"));
}

#[test]
fn test_browser_error_debug() {
    let err = BrowserError::Init("d".into());
    let debug = format!("{:?}", err);
    assert!(debug.contains("Init") || debug.contains("d"));
}

#[test]
fn test_browser_error_is_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(BrowserError::Navigation("x".into()));
    let _ = format!("{}", err);
}

// ---- Permission defaults ----

#[test]
fn test_permission_default_all_none() {
    let perm = Permission::default();
    assert!(perm.read.is_none());
    assert!(perm.write.is_none());
    assert!(perm.net.is_none());
    assert!(perm.env.is_none());
    assert!(perm.run.is_none());
}

#[test]
fn test_permission_default_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_read_allowed("/any/path"));
    assert!(perm.is_write_allowed("/any/path"));
    assert!(perm.is_net_allowed("evil.com"));
    assert!(perm.is_env_allowed());
    assert!(perm.is_run_allowed());
}

// ---- Permission read checks ----

#[test]
fn test_permission_read_allowed_prefix() {
    let perm = Permission {
        read: Some(vec!["/home".into(), "/tmp".into()]),
        ..Default::default()
    };
    assert!(perm.is_read_allowed("/home/user/file.txt"));
    assert!(perm.is_read_allowed("/tmp/data"));
    assert!(!perm.is_read_allowed("/etc/passwd"));
}

#[test]
fn test_permission_read_exact_match() {
    let perm = Permission {
        read: Some(vec!["/exact/".into()]),
        ..Default::default()
    };
    assert!(perm.is_read_allowed("/exact/sub"));
    assert!(!perm.is_read_allowed("/exactother"));
}

#[test]
fn test_permission_read_none_allows() {
    let perm = Permission { read: None, ..Default::default() };
    assert!(perm.is_read_allowed("/anything"));
}

// ---- Permission write checks ----

#[test]
fn test_permission_write_allowed_prefix() {
    let perm = Permission {
        write: Some(vec!["/var/log".into()]),
        ..Default::default()
    };
    assert!(perm.is_write_allowed("/var/log/app.log"));
    assert!(!perm.is_write_allowed("/usr/bin/app"));
}

#[test]
fn test_permission_write_none_allows() {
    let perm = Permission { write: None, ..Default::default() };
    assert!(perm.is_write_allowed("/anything"));
}

// ---- Permission net checks ----

#[test]
fn test_permission_net_exact_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("example.com"));
}

#[test]
fn test_permission_net_subdomain_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("sub.example.com"));
    assert!(perm.is_net_allowed("deep.sub.example.com"));
}

#[test]
fn test_permission_net_no_partial_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(!perm.is_net_allowed("notexample.com"));
    assert!(!perm.is_net_allowed("xexample.com"));
}

#[test]
fn test_permission_net_none_allows() {
    let perm = Permission { net: None, ..Default::default() };
    assert!(perm.is_net_allowed("any.host"));
}

#[test]
fn test_permission_net_multiple_domains() {
    let perm = Permission {
        net: Some(vec!["a.com".into(), "b.org".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("a.com"));
    assert!(perm.is_net_allowed("b.org"));
    assert!(!perm.is_net_allowed("c.net"));
}

// ---- Permission env/run ----

#[test]
fn test_permission_env_allowed_default() {
    let perm = Permission { env: None, ..Default::default() };
    assert!(perm.is_env_allowed());
}

#[test]
fn test_permission_env_allowed_true() {
    let perm = Permission { env: Some(true), ..Default::default() };
    assert!(perm.is_env_allowed());
}

#[test]
fn test_permission_env_denied() {
    let perm = Permission { env: Some(false), ..Default::default() };
    assert!(!perm.is_env_allowed());
}

#[test]
fn test_permission_run_allowed_default() {
    let perm = Permission { run: None, ..Default::default() };
    assert!(perm.is_run_allowed());
}

#[test]
fn test_permission_run_denied() {
    let perm = Permission { run: Some(false), ..Default::default() };
    assert!(!perm.is_run_allowed());
}

#[test]
fn test_permission_clone() {
    let perm = Permission {
        read: Some(vec!["/a".into()]),
        write: None,
        net: Some(vec!["b.com".into()]),
        env: Some(true),
        run: Some(false),
    };
    let cloned = perm.clone();
    assert_eq!(perm.read, cloned.read);
    assert_eq!(perm.net, cloned.net);
    assert_eq!(perm.env, cloned.env);
    assert_eq!(perm.run, cloned.run);
}

#[test]
fn test_permission_debug() {
    let perm = Permission::default();
    let debug = format!("{:?}", perm);
    assert!(debug.contains("Permission") || debug.contains("read"));
}

// ---- PermissionGuard ----

#[test]
fn test_guard_none_not_restricted() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
}

#[test]
fn test_guard_none_allows_all() {
    let guard = PermissionGuard::none();
    assert!(guard.check_read("/secret").is_ok());
    assert!(guard.check_write("/root").is_ok());
    assert!(guard.check_net("evil.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

#[test]
fn test_guard_new_restricted() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());
}

#[test]
fn test_guard_check_read_allowed() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_read("/home/user").is_ok());
}

#[test]
fn test_guard_check_read_denied() {
    let perm = Permission {
        read: Some(vec!["/home".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    let result = guard.check_read("/etc/passwd");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.category, "read");
    assert_eq!(err.resource, "/etc/passwd");
}

#[test]
fn test_guard_check_write_allowed() {
    let perm = Permission {
        write: Some(vec!["/tmp".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_write("/tmp/out").is_ok());
}

#[test]
fn test_guard_check_write_denied() {
    let perm = Permission {
        write: Some(vec!["/tmp".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    let result = guard.check_write("/usr/bin");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.category, "write");
}

#[test]
fn test_guard_check_net_allowed() {
    let perm = Permission {
        net: Some(vec!["api.example.com".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.check_net("api.example.com").is_ok());
    assert!(guard.check_net("sub.api.example.com").is_ok());
}

#[test]
fn test_guard_check_net_denied() {
    let perm = Permission {
        net: Some(vec!["api.example.com".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    let result = guard.check_net("evil.com");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.category, "net");
    assert_eq!(err.resource, "evil.com");
}

#[test]
fn test_guard_check_env_denied() {
    let perm = Permission {
        env: Some(false),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    let result = guard.check_env();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.category, "env");
}

#[test]
fn test_guard_check_run_denied() {
    let perm = Permission {
        run: Some(false),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    let result = guard.check_run();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().category, "run");
}

#[test]
fn test_guard_default_is_none() {
    let guard = PermissionGuard::default();
    assert!(!guard.is_restricted());
}

#[test]
fn test_guard_clone() {
    let perm = Permission {
        read: Some(vec!["/a".into()]),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    let cloned = guard.clone();
    assert!(cloned.is_restricted());
}

// ---- PermissionDenied ----

#[test]
fn test_permission_denied_display() {
    let err = PermissionDenied {
        category: "read".into(),
        resource: "/secret".into(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("read"));
    assert!(msg.contains("/secret"));
}

#[test]
fn test_permission_denied_debug() {
    let err = PermissionDenied {
        category: "net".into(),
        resource: "evil.com".into(),
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("net") || debug.contains("evil"));
}

#[test]
fn test_permission_denied_is_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(PermissionDenied {
        category: "run".into(),
        resource: "*".into(),
    });
    let _ = format!("{}", err);
}

#[test]
fn test_permission_denied_clone() {
    let err = PermissionDenied {
        category: "write".into(),
        resource: "/root".into(),
    };
    let cloned = err.clone();
    assert_eq!(cloned.category, err.category);
    assert_eq!(cloned.resource, err.resource);
}

// ---- Screenshot encode ----

#[test]
fn test_encode_png_small() {
    let img = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // PNG header
    assert!(data[0] == 0x89 && data[1] == 0x50);
}

#[test]
fn test_encode_jpeg_small() {
    let img = RgbaImage::from_pixel(2, 2, image::Rgba([0, 255, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
    let data = result.unwrap();
    assert!(!data.is_empty());
    // JPEG header
    assert!(data[0] == 0xFF && data[1] == 0xD8);
}

#[test]
fn test_encode_png_1x1() {
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([128, 128, 128, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
}

#[test]
fn test_encode_jpeg_1x1() {
    let img = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}

#[test]
fn test_encode_png_larger() {
    let img = RgbaImage::from_pixel(100, 100, image::Rgba([255, 255, 255, 255]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
    assert!(result.unwrap().len() > 100);
}

#[test]
fn test_encode_jpeg_larger() {
    let img = RgbaImage::from_pixel(100, 100, image::Rgba([100, 150, 200, 255]));
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    assert!(result.is_ok());
}

#[test]
fn test_encode_png_transparent() {
    let img = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 0]));
    let result = encode_image(&img, ScreenshotFormat::Png);
    assert!(result.is_ok());
}

#[test]
fn test_encode_png_deterministic() {
    let img = RgbaImage::from_pixel(5, 5, image::Rgba([42, 42, 42, 255]));
    let d1 = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let d2 = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert_eq!(d1, d2);
}
