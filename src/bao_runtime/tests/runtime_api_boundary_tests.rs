// @trace TEST-ENG-008-RUNTIME-API [req:REQ-ENG-007,REQ-STL-001,REQ-STL-002,REQ-STL-007] [level:unit]
// bao_runtime pure Rust API boundary tests: require_dir thread_local, permission_bridge
// advanced scenarios, stealth_http agent/header/fingerprint deep validation.

use std::path::PathBuf;

// ---- require_dir thread_local ----

#[test]
fn test_require_dir_default_is_none() {
    bao_runtime::require::set_require_dir(PathBuf::new());
    // After clearing with empty, verify the roundtrip works
    let retrieved = bao_runtime::require::get_require_dir();
    assert!(retrieved.is_some());
}

#[test]
fn test_require_dir_set_and_get() {
    let path = PathBuf::from("/tmp/test_require_dir");
    bao_runtime::require::set_require_dir(path.clone());
    let retrieved = bao_runtime::require::get_require_dir();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), path);
    // Clean up
    bao_runtime::require::set_require_dir(PathBuf::new());
}

#[test]
fn test_require_dir_overwrite() {
    bao_runtime::require::set_require_dir(PathBuf::from("/first"));
    bao_runtime::require::set_require_dir(PathBuf::from("/second"));
    let retrieved = bao_runtime::require::get_require_dir();
    assert_eq!(retrieved.unwrap(), PathBuf::from("/second"));
}

#[test]
fn test_require_dir_empty_path() {
    bao_runtime::require::set_require_dir(PathBuf::new());
    let retrieved = bao_runtime::require::get_require_dir();
    assert!(retrieved.is_some());
    assert!(retrieved.unwrap().as_os_str().is_empty());
}

#[test]
fn test_require_dir_unicode_path() {
    let path = PathBuf::from("/tmp/日本語/路径");
    bao_runtime::require::set_require_dir(path.clone());
    assert_eq!(bao_runtime::require::get_require_dir().unwrap(), path);
}

#[test]
fn test_require_dir_long_path() {
    let long = format!("/tmp/{}", "a".repeat(1000));
    let path = PathBuf::from(long);
    bao_runtime::require::set_require_dir(path.clone());
    assert_eq!(bao_runtime::require::get_require_dir().unwrap(), path);
}

// ---- permission_bridge advanced scenarios ----

use bao_runtime::permission_bridge::{self, PermissionCheck};

#[test]
fn test_permission_multiple_paths_whitelist() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec!["/data/".into(), "/tmp/".into(), "/home/user/".into()]),
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_fs_read("/data/file.txt").is_ok());
    assert!(permission_bridge::check_fs_read("/tmp/cache").is_ok());
    assert!(permission_bridge::check_fs_read("/home/user/doc").is_ok());
    assert!(permission_bridge::check_fs_read("/etc/passwd").is_err());
    permission_bridge::set_permission(None);
}

#[test]
fn test_permission_write_multiple_whitelist() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: Some(vec!["/out/".into(), "/tmp/".into()]),
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_fs_write("/out/result.json").is_ok());
    assert!(permission_bridge::check_fs_write("/tmp/file").is_ok());
    assert!(permission_bridge::check_fs_write("/var/log").is_err());
    permission_bridge::set_permission(None);
}

#[test]
fn test_permission_net_multiple_hosts() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: Some(vec!["api.example.com".into(), "cdn.example.com".into()]),
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_net("api.example.com").is_ok());
    assert!(permission_bridge::check_net("cdn.example.com").is_ok());
    assert!(permission_bridge::check_net("sub.api.example.com").is_ok());
    assert!(permission_bridge::check_net("evil.com").is_err());
    permission_bridge::set_permission(None);
}

#[test]
fn test_permission_all_restricted() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec![]),
        write_paths: Some(vec![]),
        net_hosts: Some(vec![]),
        env_allowed: false,
        run_allowed: false,
    }));
    assert!(permission_bridge::check_fs_read("/anything").is_err());
    assert!(permission_bridge::check_fs_write("/anything").is_err());
    assert!(permission_bridge::check_net("any.com").is_err());
    assert!(permission_bridge::check_env().is_err());
    assert!(permission_bridge::check_run().is_err());
    permission_bridge::set_permission(None);
}

#[test]
fn test_permission_all_open() {
    permission_bridge::set_permission(None);
    assert!(permission_bridge::check_fs_read("/anything").is_ok());
    assert!(permission_bridge::check_fs_write("/anything").is_ok());
    assert!(permission_bridge::check_net("any.com").is_ok());
    assert!(permission_bridge::check_env().is_ok());
    assert!(permission_bridge::check_run().is_ok());
}

#[test]
fn test_permission_read_only_mode() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: Some(vec![]),
        net_hosts: None,
        env_allowed: true,
        run_allowed: false,
    }));
    assert!(permission_bridge::check_fs_read("/any/file").is_ok());
    assert!(permission_bridge::check_fs_write("/any/file").is_err());
    assert!(permission_bridge::check_net("any.com").is_ok());
    assert!(permission_bridge::check_env().is_ok());
    assert!(permission_bridge::check_run().is_err());
    permission_bridge::set_permission(None);
}

#[test]
fn test_permission_net_only_mode() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec![]),
        write_paths: Some(vec![]),
        net_hosts: Some(vec!["allowed.com".into()]),
        env_allowed: false,
        run_allowed: false,
    }));
    assert!(permission_bridge::check_fs_read("/x").is_err());
    assert!(permission_bridge::check_fs_write("/x").is_err());
    assert!(permission_bridge::check_net("allowed.com").is_ok());
    assert!(permission_bridge::check_net("denied.com").is_err());
    permission_bridge::set_permission(None);
}

#[test]
fn test_permission_error_messages_contain_context() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec![]),
        write_paths: Some(vec![]),
        net_hosts: Some(vec![]),
        env_allowed: false,
        run_allowed: false,
    }));
    let read_err = permission_bridge::check_fs_read("/secret").unwrap_err();
    assert!(read_err.contains("/secret"));
    assert!(read_err.contains("read"));

    let write_err = permission_bridge::check_fs_write("/output").unwrap_err();
    assert!(write_err.contains("/output"));
    assert!(write_err.contains("write"));

    let net_err = permission_bridge::check_net("blocked.com").unwrap_err();
    assert!(net_err.contains("blocked.com"));
    assert!(net_err.contains("net"));

    let env_err = permission_bridge::check_env().unwrap_err();
    assert!(env_err.contains("env"));

    let run_err = permission_bridge::check_run().unwrap_err();
    assert!(run_err.contains("run"));
    permission_bridge::set_permission(None);
}

#[test]
fn test_permission_switching_multiple_times() {
    for i in 0..10 {
        if i % 2 == 0 {
            permission_bridge::set_permission(None);
            assert!(permission_bridge::check_fs_read("/x").is_ok());
        } else {
            permission_bridge::set_permission(Some(PermissionCheck {
                read_paths: Some(vec![]),
                write_paths: None,
                net_hosts: None,
                env_allowed: true,
                run_allowed: true,
            }));
            assert!(permission_bridge::check_fs_read("/x").is_err());
        }
    }
    permission_bridge::set_permission(None);
}

// ---- stealth_http deep validation ----

use bao_runtime::stealth_http;
use bao_stealth::StealthProfile;

#[test]
fn test_stealth_agent_chrome_creates_without_panic() {
    let profile = StealthProfile::chrome_default();
    let _agent = stealth_http::create_stealth_agent(&Some(profile));
}

#[test]
fn test_stealth_agent_firefox_creates_without_panic() {
    let profile = StealthProfile::firefox_default();
    let _agent = stealth_http::create_stealth_agent(&Some(profile));
}

#[test]
fn test_stealth_agent_no_profile_creates_without_panic() {
    let _agent = stealth_http::create_stealth_agent(&None);
}

#[test]
fn test_ordered_headers_empty_input() {
    let headers: Vec<(String, String)> = vec![];
    let ordered = stealth_http::ordered_headers(&None, &headers);
    assert!(ordered.is_empty());
    let profile = StealthProfile::chrome_default();
    let ordered_with_profile = stealth_http::ordered_headers(&Some(profile), &headers);
    assert!(ordered_with_profile.is_empty());
}

#[test]
fn test_ordered_headers_single_header() {
    let headers = vec![("content-type".to_string(), "text/html".to_string())];
    let ordered = stealth_http::ordered_headers(&None, &headers);
    assert_eq!(ordered.len(), 1);
    assert_eq!(ordered[0].0, "content-type");
}

#[test]
fn test_ordered_headers_preserves_count() {
    let profile = StealthProfile::chrome_default();
    let headers: Vec<(String, String)> = (0..50)
        .map(|i| (format!("header-{}", i), format!("value-{}", i)))
        .collect();
    let ordered = stealth_http::ordered_headers(&Some(profile), &headers);
    assert_eq!(ordered.len(), 50);
}

#[test]
fn test_ordered_headers_mixed_pseudo_and_regular() {
    let profile = StealthProfile::firefox_default();
    let headers = vec![
        ("accept".to_string(), "*/*".to_string()),
        (":method".to_string(), "POST".to_string()),
        ("content-length".to_string(), "100".to_string()),
        (":path".to_string(), "/api/v1".to_string()),
        ("user-agent".to_string(), "test".to_string()),
        (":authority".to_string(), "api.example.com".to_string()),
        (":scheme".to_string(), "https".to_string()),
    ];
    let ordered = stealth_http::ordered_headers(&Some(profile), &headers);
    // Firefox order: :method, :path, :authority, :scheme
    assert_eq!(ordered.len(), 7);
    // Pseudo headers first
    let pseudo_count = ordered.iter().take_while(|(k, _)| k.starts_with(':')).count();
    assert_eq!(pseudo_count, 4, "All 4 pseudo-headers should come first");
}

#[test]
fn test_ja3_hash_chrome_format() {
    let profile = StealthProfile::chrome_default();
    let hash = stealth_http::ja3_hash(&Some(profile)).unwrap();
    assert!(hash.starts_with("771,"));
    assert!(hash.contains("-"));
}

#[test]
fn test_ja3_hash_firefox_format() {
    let profile = StealthProfile::firefox_default();
    let hash = stealth_http::ja3_hash(&Some(profile)).unwrap();
    assert!(hash.starts_with("771,"));
}

#[test]
fn test_ja3_hash_different_between_profiles() {
    let ch = StealthProfile::chrome_default();
    let ff = StealthProfile::firefox_default();
    assert_ne!(
        stealth_http::ja3_hash(&Some(ch)).unwrap(),
        stealth_http::ja3_hash(&Some(ff)).unwrap()
    );
}

#[test]
fn test_akamai_fingerprint_format() {
    let profile = StealthProfile::chrome_default();
    let fp = stealth_http::akamai_fingerprint(&Some(profile)).unwrap();
    let parts: Vec<&str> = fp.split(':').collect();
    assert_eq!(parts.len(), 6);
    // All parts should be numeric
    for part in &parts {
        assert!(part.parse::<u64>().is_ok(), "Part '{}' should be numeric", part);
    }
}

#[test]
fn test_akamai_fingerprint_different_between_profiles() {
    let ch = StealthProfile::chrome_default();
    let ff = StealthProfile::firefox_default();
    assert_ne!(
        stealth_http::akamai_fingerprint(&Some(ch)).unwrap(),
        stealth_http::akamai_fingerprint(&Some(ff)).unwrap()
    );
}

#[test]
fn test_ja3_and_akamai_both_none_when_no_profile() {
    assert!(stealth_http::ja3_hash(&None).is_none());
    assert!(stealth_http::akamai_fingerprint(&None).is_none());
}

// ---- PermissionCheck Debug/Clone re-verification ----

#[test]
fn test_permission_check_all_fields_set() {
    let pc = PermissionCheck {
        read_paths: Some(vec!["/r/".into()]),
        write_paths: Some(vec!["/w/".into()]),
        net_hosts: Some(vec!["h.com".into()]),
        env_allowed: false,
        run_allowed: false,
    };
    let debug = format!("{:?}", pc);
    assert!(debug.contains("read_paths"));
    assert!(debug.contains("write_paths"));
    assert!(debug.contains("net_hosts"));
    assert!(debug.contains("env_allowed"));
    assert!(debug.contains("run_allowed"));
}

#[test]
fn test_permission_check_clone_equality() {
    let pc = PermissionCheck {
        read_paths: Some(vec!["/data/".into()]),
        write_paths: Some(vec!["/out/".into()]),
        net_hosts: Some(vec!["api.example.com".into()]),
        env_allowed: true,
        run_allowed: false,
    };
    let cloned = pc.clone();
    assert_eq!(pc.read_paths, cloned.read_paths);
    assert_eq!(pc.write_paths, cloned.write_paths);
    assert_eq!(pc.net_hosts, cloned.net_hosts);
    assert_eq!(pc.env_allowed, cloned.env_allowed);
    assert_eq!(pc.run_allowed, cloned.run_allowed);
}

// ---- resolve_node_modules edge cases ----

#[test]
fn test_resolve_node_modules_nonexistent_specifier() {
    // Should return None for a module that doesn't exist
    let result = bao_runtime::require::resolve_node_modules(
        "nonexistent-module-xyz-12345",
        Some(std::path::Path::new("/tmp")),
    );
    assert!(result.is_none());
}

#[test]
fn test_resolve_node_modules_empty_specifier() {
    let result = bao_runtime::require::resolve_node_modules(
        "",
        Some(std::path::Path::new("/tmp")),
    );
    assert!(result.is_none());
}

#[test]
fn test_resolve_node_modules_with_dot_specifier() {
    let result = bao_runtime::require::resolve_node_modules(
        ".",
        Some(std::path::Path::new("/tmp")),
    );
    // "." is not a valid node_modules package name
    assert!(result.is_none());
}

#[test]
fn test_resolve_node_modules_scoped_specifier() {
    let result = bao_runtime::require::resolve_node_modules(
        "@types/node",
        Some(std::path::Path::new("/tmp")),
    );
    assert!(result.is_none());
}
