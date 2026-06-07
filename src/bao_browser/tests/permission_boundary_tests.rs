// @trace TEST-LIB-004-PERM [req:REQ-LIB-004] [level:unit]
// Permission boundary tests: subdomain matching, env/run booleans, Display, guard modes

use bao_browser::{Permission, PermissionGuard, PermissionDenied};

// ---- Permission::is_net_allowed subdomain matching ----

#[test]
fn test_net_exact_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("example.com"));
}

#[test]
fn test_net_subdomain_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("sub.example.com"));
    assert!(perm.is_net_allowed("a.b.example.com"));
}

#[test]
fn test_net_no_partial_match() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    assert!(!perm.is_net_allowed("notexample.com"));
    assert!(!perm.is_net_allowed("example.com.evil.org"));
}

#[test]
fn test_net_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_net_allowed("any.host"));
    assert!(perm.is_net_allowed("evil.org"));
}

#[test]
fn test_net_multiple_domains() {
    let perm = Permission {
        net: Some(vec!["a.com".into(), "b.com".into()]),
        ..Default::default()
    };
    assert!(perm.is_net_allowed("a.com"));
    assert!(perm.is_net_allowed("sub.b.com"));
    assert!(!perm.is_net_allowed("c.com"));
}

// ---- Permission::is_read_allowed / is_write_allowed prefix match ----

#[test]
fn test_read_prefix_match() {
    let perm = Permission {
        read: Some(vec!["/home/user/".into(), "/tmp/".into()]),
        ..Default::default()
    };
    assert!(perm.is_read_allowed("/home/user/file.txt"));
    assert!(perm.is_read_allowed("/tmp/cache"));
    assert!(!perm.is_read_allowed("/etc/passwd"));
}

#[test]
fn test_write_prefix_match() {
    let perm = Permission {
        write: Some(vec!["/var/log/".into()]),
        ..Default::default()
    };
    assert!(perm.is_write_allowed("/var/log/app.log"));
    assert!(!perm.is_write_allowed("/etc/shadow"));
}

#[test]
fn test_read_write_none_allows_all() {
    let perm = Permission::default();
    assert!(perm.is_read_allowed("/any/path"));
    assert!(perm.is_write_allowed("/any/path"));
}

#[test]
fn test_empty_allowed_list_denies_all() {
    let perm = Permission {
        read: Some(vec![]),
        write: Some(vec![]),
        ..Default::default()
    };
    assert!(!perm.is_read_allowed("/anything"));
    assert!(!perm.is_write_allowed("/anything"));
}

// ---- Permission env/run booleans ----

#[test]
fn test_env_allowed_default() {
    let perm = Permission::default();
    assert!(perm.is_env_allowed());
}

#[test]
fn test_env_explicit_true() {
    let perm = Permission { env: Some(true), ..Default::default() };
    assert!(perm.is_env_allowed());
}

#[test]
fn test_env_explicit_false() {
    let perm = Permission { env: Some(false), ..Default::default() };
    assert!(!perm.is_env_allowed());
}

#[test]
fn test_run_allowed_default() {
    let perm = Permission::default();
    assert!(perm.is_run_allowed());
}

#[test]
fn test_run_explicit_false() {
    let perm = Permission { run: Some(false), ..Default::default() };
    assert!(!perm.is_run_allowed());
}

// ---- PermissionGuard modes ----

#[test]
fn test_guard_none_allows_all() {
    let guard = PermissionGuard::none();
    assert!(!guard.is_restricted());
    assert!(guard.check_read("/secret").is_ok());
    assert!(guard.check_write("/secret").is_ok());
    assert!(guard.check_net("evil.com").is_ok());
    assert!(guard.check_env().is_ok());
    assert!(guard.check_run().is_ok());
}

#[test]
fn test_guard_with_permission_restricts() {
    let perm = Permission {
        read: Some(vec!["/allowed/".into()]),
        write: Some(vec!["/allowed/".into()]),
        net: Some(vec!["safe.com".into()]),
        env: Some(false),
        run: Some(false),
        ..Default::default()
    };
    let guard = PermissionGuard::new(perm);
    assert!(guard.is_restricted());

    // Read: only /allowed/ prefix
    assert!(guard.check_read("/allowed/file").is_ok());
    assert!(guard.check_read("/forbidden").is_err());

    // Write: only /allowed/ prefix
    assert!(guard.check_write("/allowed/out").is_ok());
    assert!(guard.check_write("/etc/passwd").is_err());

    // Net: only safe.com
    assert!(guard.check_net("safe.com").is_ok());
    assert!(guard.check_net("sub.safe.com").is_ok());
    assert!(guard.check_net("unsafe.com").is_err());

    // Env/Run: denied
    assert!(guard.check_env().is_err());
    assert!(guard.check_run().is_err());
}

#[test]
fn test_guard_default_is_none() {
    let guard = PermissionGuard::default();
    assert!(!guard.is_restricted());
}

// ---- PermissionDenied Display ----

#[test]
fn test_permission_denied_display() {
    let err = PermissionDenied {
        category: "read".into(),
        resource: "/etc/shadow".into(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("read"));
    assert!(msg.contains("/etc/shadow"));
}

#[test]
fn test_permission_denied_is_error() {
    let err = PermissionDenied {
        category: "net".into(),
        resource: "evil.com".into(),
    };
    let _: &dyn std::error::Error = &err;
}

// ---- Cross-field independence ----

#[test]
fn test_read_write_independent() {
    let perm = Permission {
        read: Some(vec!["/read/".into()]),
        write: Some(vec!["/write/".into()]),
        ..Default::default()
    };
    assert!(perm.is_read_allowed("/read/file"));
    assert!(!perm.is_read_allowed("/write/file"));
    assert!(perm.is_write_allowed("/write/file"));
    assert!(!perm.is_write_allowed("/read/file"));
}

#[test]
fn test_net_with_port() {
    let perm = Permission {
        net: Some(vec!["example.com".into()]),
        ..Default::default()
    };
    // Port is not part of domain matching — host should be domain only
    assert!(!perm.is_net_allowed("example.com:8080"));
    assert!(perm.is_net_allowed("example.com"));
}
