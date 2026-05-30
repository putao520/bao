// @trace TEST-ENG-007-BOUNDARY [req:REQ-ENG-007,REQ-LIB-004] [level:unit]
// Pure Rust boundary tests for bao_runtime modules that don't require JSContext.
// Tests permission_bridge, node_os::sys_info, stealth_http pure functions.

use bao_runtime::permission_bridge::{self, PermissionCheck};

// ---- permission_bridge: no permission set (default allow-all) ----

#[test]
fn test_permission_default_allows_read() {
    permission_bridge::set_permission(None);
    assert!(permission_bridge::check_fs_read("/etc/passwd").is_ok());
}

#[test]
fn test_permission_default_allows_write() {
    permission_bridge::set_permission(None);
    assert!(permission_bridge::check_fs_write("/tmp/out").is_ok());
}

#[test]
fn test_permission_default_allows_net() {
    permission_bridge::set_permission(None);
    assert!(permission_bridge::check_net("evil.com").is_ok());
}

#[test]
fn test_permission_default_allows_env() {
    permission_bridge::set_permission(None);
    assert!(permission_bridge::check_env().is_ok());
}

#[test]
fn test_permission_default_allows_run() {
    permission_bridge::set_permission(None);
    assert!(permission_bridge::check_run().is_ok());
}

// ---- permission_bridge: restricted read ----

#[test]
fn test_permission_read_whitelist_match() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec!["/data/".into(), "/tmp/".into()]),
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_fs_read("/data/file.txt").is_ok());
    assert!(permission_bridge::check_fs_read("/tmp/cache").is_ok());
}

#[test]
fn test_permission_read_whitelist_miss() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec!["/data/".into()]),
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_fs_read("/etc/passwd").is_err());
}

#[test]
fn test_permission_read_none_allows_all() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_fs_read("/any/path").is_ok());
}

#[test]
fn test_permission_read_empty_vec_blocks_all() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec![]),
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_fs_read("/anything").is_err());
}

// ---- permission_bridge: restricted write ----

#[test]
fn test_permission_write_whitelist() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: Some(vec!["/output/".into()]),
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_fs_write("/output/result.json").is_ok());
    assert!(permission_bridge::check_fs_write("/tmp/file").is_err());
}

// ---- permission_bridge: restricted net ----

#[test]
fn test_permission_net_whitelist_exact_match() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: Some(vec!["api.example.com".into()]),
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_net("api.example.com").is_ok());
}

#[test]
fn test_permission_net_whitelist_subdomain() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: Some(vec!["example.com".into()]),
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_net("api.example.com").is_ok());
    assert!(permission_bridge::check_net("deep.sub.example.com").is_ok());
}

#[test]
fn test_permission_net_whitelist_not_suffix() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: Some(vec!["example.com".into()]),
        env_allowed: true,
        run_allowed: true,
    }));
    // evil.com does not end with .example.com
    assert!(permission_bridge::check_net("evil.com").is_err());
    // notexample.com ends with "example.com" but needs dot prefix
    assert!(permission_bridge::check_net("notexample.com").is_err());
}

#[test]
fn test_permission_net_empty_vec_blocks_all() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: Some(vec![]),
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_net("any.com").is_err());
}

// ---- permission_bridge: env/run ----

#[test]
fn test_permission_env_allowed() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: false,
    }));
    assert!(permission_bridge::check_env().is_ok());
    assert!(permission_bridge::check_run().is_err());
}

#[test]
fn test_permission_env_denied() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: None,
        env_allowed: false,
        run_allowed: true,
    }));
    assert!(permission_bridge::check_env().is_err());
    assert!(permission_bridge::check_run().is_ok());
}

// ---- permission_bridge: switch permissions mid-test ----

#[test]
fn test_permission_switch_from_restricted_to_open() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec![]),
        write_paths: Some(vec![]),
        net_hosts: Some(vec![]),
        env_allowed: false,
        run_allowed: false,
    }));
    assert!(permission_bridge::check_fs_read("/x").is_err());
    assert!(permission_bridge::check_net("x.com").is_err());

    // Switch to open
    permission_bridge::set_permission(None);
    assert!(permission_bridge::check_fs_read("/x").is_ok());
    assert!(permission_bridge::check_net("x.com").is_ok());
}

// ---- permission_bridge: error message format ----

#[test]
fn test_permission_error_message_contains_path() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: Some(vec![]),
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    let err = permission_bridge::check_fs_read("/secret/file").unwrap_err();
    assert!(err.contains("/secret/file"));
    assert!(err.contains("read"));
}

#[test]
fn test_permission_net_error_message_contains_host() {
    permission_bridge::set_permission(Some(PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: Some(vec![]),
        env_allowed: true,
        run_allowed: true,
    }));
    let err = permission_bridge::check_net("evil.com").unwrap_err();
    assert!(err.contains("evil.com"));
    assert!(err.contains("net"));
}

// ---- stealth_http pure functions ----

#[test]
fn test_stealth_ordered_headers_no_profile_passthrough() {
    use bao_runtime::stealth_http;
    let headers = vec![
        ("content-type".into(), "text/html".into()),
        ("accept".into(), "*/*".into()),
    ];
    let ordered = stealth_http::ordered_headers(&None, &headers);
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].0, "content-type");
    assert_eq!(ordered[1].0, "accept");
}

#[test]
fn test_stealth_ja3_hash_none() {
    use bao_runtime::stealth_http;
    assert!(stealth_http::ja3_hash(&None).is_none());
}

#[test]
fn test_stealth_akamai_fingerprint_none() {
    use bao_runtime::stealth_http;
    assert!(stealth_http::akamai_fingerprint(&None).is_none());
}

#[test]
fn test_stealth_ja3_firefox_starts_with_771() {
    use bao_runtime::stealth_http;
    use bao_stealth::StealthProfile;
    let profile = StealthProfile::firefox_default();
    let hash = stealth_http::ja3_hash(&Some(profile)).unwrap();
    assert!(hash.starts_with("771,"));
}

#[test]
fn test_stealth_ja3_chrome_starts_with_771() {
    use bao_runtime::stealth_http;
    use bao_stealth::StealthProfile;
    let profile = StealthProfile::chrome_default();
    let hash = stealth_http::ja3_hash(&Some(profile)).unwrap();
    assert!(hash.starts_with("771,"));
}

#[test]
fn test_stealth_profiles_different_ja3() {
    use bao_runtime::stealth_http;
    use bao_stealth::StealthProfile;
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    let ff_hash = stealth_http::ja3_hash(&Some(ff)).unwrap();
    let ch_hash = stealth_http::ja3_hash(&Some(ch)).unwrap();
    assert_ne!(ff_hash, ch_hash);
}

#[test]
fn test_stealth_profiles_different_akamai() {
    use bao_runtime::stealth_http;
    use bao_stealth::StealthProfile;
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    let ff_fp = stealth_http::akamai_fingerprint(&Some(ff)).unwrap();
    let ch_fp = stealth_http::akamai_fingerprint(&Some(ch)).unwrap();
    assert_ne!(ff_fp, ch_fp);
}

// ---- PermissionCheck struct traits ----

#[test]
fn test_permission_check_debug() {
    let pc = PermissionCheck {
        read_paths: Some(vec!["/tmp/".into()]),
        write_paths: None,
        net_hosts: Some(vec!["safe.com".into()]),
        env_allowed: true,
        run_allowed: false,
    };
    let debug = format!("{:?}", pc);
    assert!(debug.contains("read_paths"));
    assert!(debug.contains("/tmp/"));
}

#[test]
fn test_permission_check_clone() {
    let pc = PermissionCheck {
        read_paths: Some(vec!["/data/".into()]),
        write_paths: Some(vec![]),
        net_hosts: None,
        env_allowed: false,
        run_allowed: true,
    };
    let cloned = pc.clone();
    assert_eq!(pc.read_paths, cloned.read_paths);
    assert_eq!(pc.write_paths, cloned.write_paths);
    assert_eq!(pc.net_hosts, cloned.net_hosts);
    assert_eq!(pc.env_allowed, cloned.env_allowed);
    assert_eq!(pc.run_allowed, cloned.run_allowed);
}
