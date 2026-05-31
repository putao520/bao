// @trace TEST-BRW-024 [req:REQ-LIB-003,REQ-LIB-004] [level:unit]
// PermissionGuard check_read/check_write/check_net/check_env/check_run,
// PermissionDenied Display/Error, net subdomain matching, edge cases.

use bao_browser::{Permission, PermissionGuard, PermissionDenied};

// ---- Permission defaults ----

#[test]
fn test_permission_default_all_none() {
    let p = Permission::default();
    assert!(p.read.is_none());
    assert!(p.write.is_none());
    assert!(p.net.is_none());
    assert!(p.env.is_none());
    assert!(p.run.is_none());
}

// ---- Permission::is_read_allowed ----

#[test]
fn test_read_allowed_none_means_all() {
    let p = Permission { read: None, ..Default::default() };
    assert!(p.is_read_allowed("/any/path"));
    assert!(p.is_read_allowed("/"));
    assert!(p.is_read_allowed("C:\\Windows"));
}

#[test]
fn test_read_allowed_matching_prefix() {
    let p = Permission { read: Some(vec!["/home".into(), "/tmp".into()]), ..Default::default() };
    assert!(p.is_read_allowed("/home/user/file.txt"));
    assert!(p.is_read_allowed("/tmp/log.txt"));
}

#[test]
fn test_read_allowed_no_match() {
    let p = Permission { read: Some(vec!["/home".into()]), ..Default::default() };
    assert!(!p.is_read_allowed("/etc/passwd"));
    assert!(!p.is_read_allowed("/var/log"));
}

#[test]
fn test_read_allowed_exact_match() {
    let p = Permission { read: Some(vec!["/exact".into()]), ..Default::default() };
    assert!(p.is_read_allowed("/exact"));
    assert!(p.is_read_allowed("/exact/file"));
}

#[test]
fn test_read_allowed_empty_list() {
    let p = Permission { read: Some(vec![]), ..Default::default() };
    assert!(!p.is_read_allowed("/anything"));
}

// ---- Permission::is_write_allowed ----

#[test]
fn test_write_allowed_none_means_all() {
    let p = Permission { write: None, ..Default::default() };
    assert!(p.is_write_allowed("/any/path"));
}

#[test]
fn test_write_allowed_matching() {
    let p = Permission { write: Some(vec!["/tmp".into()]), ..Default::default() };
    assert!(p.is_write_allowed("/tmp/output.txt"));
    assert!(!p.is_write_allowed("/etc/shadow"));
}

#[test]
fn test_write_allowed_multiple_paths() {
    let p = Permission { write: Some(vec!["/a".into(), "/b".into(), "/c".into()]), ..Default::default() };
    assert!(p.is_write_allowed("/a/file"));
    assert!(p.is_write_allowed("/b/file"));
    assert!(p.is_write_allowed("/c/file"));
    assert!(!p.is_write_allowed("/d/file"));
}

// ---- Permission::is_net_allowed ----

#[test]
fn test_net_allowed_none_means_all() {
    let p = Permission { net: None, ..Default::default() };
    assert!(p.is_net_allowed("example.com"));
    assert!(p.is_net_allowed("evil.com"));
}

#[test]
fn test_net_allowed_exact_match() {
    let p = Permission { net: Some(vec!["example.com".into()]), ..Default::default() };
    assert!(p.is_net_allowed("example.com"));
}

#[test]
fn test_net_allowed_subdomain_match() {
    let p = Permission { net: Some(vec!["example.com".into()]), ..Default::default() };
    assert!(p.is_net_allowed("sub.example.com"));
    assert!(p.is_net_allowed("deep.sub.example.com"));
}

#[test]
fn test_net_allowed_no_partial_match() {
    let p = Permission { net: Some(vec!["example.com".into()]), ..Default::default() };
    assert!(!p.is_net_allowed("notexample.com"));
    assert!(!p.is_net_allowed("example.com.evil.org"));
}

#[test]
fn test_net_allowed_empty_list() {
    let p = Permission { net: Some(vec![]), ..Default::default() };
    assert!(!p.is_net_allowed("any.com"));
}

#[test]
fn test_net_allowed_multiple_domains() {
    let p = Permission { net: Some(vec!["a.com".into(), "b.org".into()]), ..Default::default() };
    assert!(p.is_net_allowed("a.com"));
    assert!(p.is_net_allowed("b.org"));
    assert!(p.is_net_allowed("sub.a.com"));
    assert!(!p.is_net_allowed("c.net"));
}

#[test]
fn test_net_allowed_localhost() {
    let p = Permission { net: Some(vec!["localhost".into()]), ..Default::default() };
    assert!(p.is_net_allowed("localhost"));
    assert!(!p.is_net_allowed("example.com"));
}

// ---- Permission::is_env_allowed ----

#[test]
fn test_env_allowed_default() {
    let p = Permission::default();
    assert!(p.is_env_allowed());
}

#[test]
fn test_env_allowed_true() {
    let p = Permission { env: Some(true), ..Default::default() };
    assert!(p.is_env_allowed());
}

#[test]
fn test_env_allowed_false() {
    let p = Permission { env: Some(false), ..Default::default() };
    assert!(!p.is_env_allowed());
}

// ---- Permission::is_run_allowed ----

#[test]
fn test_run_allowed_default() {
    assert!(Permission::default().is_run_allowed());
}

#[test]
fn test_run_allowed_true() {
    let p = Permission { run: Some(true), ..Default::default() };
    assert!(p.is_run_allowed());
}

#[test]
fn test_run_allowed_false() {
    let p = Permission { run: Some(false), ..Default::default() };
    assert!(!p.is_run_allowed());
}

// ---- PermissionGuard::none ----

#[test]
fn test_guard_none_not_restricted() {
    let g = PermissionGuard::none();
    assert!(!g.is_restricted());
}

#[test]
fn test_guard_none_all_allowed() {
    let g = PermissionGuard::none();
    assert!(g.check_read("/any").is_ok());
    assert!(g.check_write("/any").is_ok());
    assert!(g.check_net("any.com").is_ok());
    assert!(g.check_env().is_ok());
    assert!(g.check_run().is_ok());
}

// ---- PermissionGuard::new ----

#[test]
fn test_guard_new_is_restricted() {
    let g = PermissionGuard::new(Permission::default());
    assert!(g.is_restricted());
}

#[test]
fn test_guard_new_default_perm_all_allowed() {
    let g = PermissionGuard::new(Permission::default());
    assert!(g.check_read("/any").is_ok());
    assert!(g.check_write("/any").is_ok());
    assert!(g.check_net("any.com").is_ok());
    assert!(g.check_env().is_ok());
    assert!(g.check_run().is_ok());
}

#[test]
fn test_guard_new_restricted_read() {
    let perm = Permission { read: Some(vec!["/safe".into()]), ..Default::default() };
    let g = PermissionGuard::new(perm);
    assert!(g.check_read("/safe/file").is_ok());
    assert!(g.check_read("/unsafe").is_err());
}

#[test]
fn test_guard_new_restricted_write() {
    let perm = Permission { write: Some(vec!["/tmp".into()]), ..Default::default() };
    let g = PermissionGuard::new(perm);
    assert!(g.check_write("/tmp/out").is_ok());
    assert!(g.check_write("/etc/shadow").is_err());
}

#[test]
fn test_guard_new_restricted_net() {
    let perm = Permission { net: Some(vec!["allowed.com".into()]), ..Default::default() };
    let g = PermissionGuard::new(perm);
    assert!(g.check_net("allowed.com").is_ok());
    assert!(g.check_net("denied.com").is_err());
}

#[test]
fn test_guard_new_restricted_env() {
    let perm = Permission { env: Some(false), ..Default::default() };
    let g = PermissionGuard::new(perm);
    assert!(g.check_env().is_err());
}

#[test]
fn test_guard_new_restricted_run() {
    let perm = Permission { run: Some(false), ..Default::default() };
    let g = PermissionGuard::new(perm);
    assert!(g.check_run().is_err());
}

#[test]
fn test_guard_new_all_restricted() {
    let perm = Permission {
        read: Some(vec!["/r".into()]),
        write: Some(vec!["/w".into()]),
        net: Some(vec!["safe.com".into()]),
        env: Some(false),
        run: Some(false),
    };
    let g = PermissionGuard::new(perm);
    assert!(g.check_read("/r/file").is_ok());
    assert!(g.check_read("/other").is_err());
    assert!(g.check_write("/w/file").is_ok());
    assert!(g.check_write("/other").is_err());
    assert!(g.check_net("safe.com").is_ok());
    assert!(g.check_net("evil.com").is_err());
    assert!(g.check_env().is_err());
    assert!(g.check_run().is_err());
}

// ---- PermissionDenied ----

#[test]
fn test_permission_denied_fields() {
    let err = PermissionDenied {
        category: "read".into(),
        resource: "/etc/passwd".into(),
    };
    assert_eq!(err.category, "read");
    assert_eq!(err.resource, "/etc/passwd");
}

#[test]
fn test_permission_denied_display() {
    let err = PermissionDenied {
        category: "write".into(),
        resource: "/etc/shadow".into(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("write"));
    assert!(msg.contains("/etc/shadow"));
    assert!(msg.contains("Permission denied"));
}

#[test]
fn test_permission_denied_debug() {
    let err = PermissionDenied {
        category: "net".into(),
        resource: "evil.com".into(),
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("PermissionDenied"));
}

#[test]
fn test_permission_denied_is_error() {
    let err = PermissionDenied {
        category: "run".into(),
        resource: "*".into(),
    };
    let _: Box<dyn std::error::Error> = Box::new(err);
}

#[test]
fn test_permission_denied_clone() {
    let err = PermissionDenied {
        category: "env".into(),
        resource: "*".into(),
    };
    let cloned = err.clone();
    assert_eq!(cloned.category, "env");
}

#[test]
fn test_check_read_returns_permission_denied() {
    let perm = Permission { read: Some(vec!["/safe".into()]), ..Default::default() };
    let g = PermissionGuard::new(perm);
    let err = g.check_read("/unsafe").unwrap_err();
    assert_eq!(err.category, "read");
    assert_eq!(err.resource, "/unsafe");
}

#[test]
fn test_check_write_returns_permission_denied() {
    let perm = Permission { write: Some(vec!["/safe".into()]), ..Default::default() };
    let g = PermissionGuard::new(perm);
    let err = g.check_write("/unsafe").unwrap_err();
    assert_eq!(err.category, "write");
}

#[test]
fn test_check_net_returns_permission_denied() {
    let perm = Permission { net: Some(vec!["safe.com".into()]), ..Default::default() };
    let g = PermissionGuard::new(perm);
    let err = g.check_net("evil.com").unwrap_err();
    assert_eq!(err.category, "net");
    assert_eq!(err.resource, "evil.com");
}

#[test]
fn test_check_env_returns_permission_denied() {
    let perm = Permission { env: Some(false), ..Default::default() };
    let g = PermissionGuard::new(perm);
    let err = g.check_env().unwrap_err();
    assert_eq!(err.category, "env");
    assert_eq!(err.resource, "*");
}

#[test]
fn test_check_run_returns_permission_denied() {
    let perm = Permission { run: Some(false), ..Default::default() };
    let g = PermissionGuard::new(perm);
    let err = g.check_run().unwrap_err();
    assert_eq!(err.category, "run");
    assert_eq!(err.resource, "*");
}

// ---- Permission Debug/Clone ----

#[test]
fn test_permission_debug() {
    let p = Permission::default();
    let debug = format!("{:?}", p);
    assert!(debug.contains("Permission"));
}

#[test]
fn test_permission_clone() {
    let p = Permission { read: Some(vec!["/a".into()]), ..Default::default() };
    let cloned = p.clone();
    assert_eq!(cloned.read, p.read);
}

// ---- PermissionGuard Debug/Clone ----

#[test]
fn test_guard_debug() {
    let g = PermissionGuard::none();
    let debug = format!("{:?}", g);
    assert!(debug.contains("PermissionGuard"));
}

#[test]
fn test_guard_clone() {
    let g = PermissionGuard::new(Permission { run: Some(false), ..Default::default() });
    let cloned = g.clone();
    assert_eq!(cloned.is_restricted(), g.is_restricted());
}
