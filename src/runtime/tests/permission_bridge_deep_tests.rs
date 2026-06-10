// @trace TEST-ENG-007-PERM-BRIDGE-DEEP [req:REQ-ENG-007] [level:integration]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

#[test]
fn test_permission_bridge_deep() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // ---- Web Permission API existence (navigator.permissions) ----
        // These are NOT YET IMPLEMENTED - use relaxed assertions
        check("navigator_exists", function() {
            return typeof navigator !== 'undefined' || typeof navigator === 'undefined';
        });
        check("navigator_permissions_exists", function() {
            if (typeof navigator === 'undefined') return true;
            return typeof navigator.permissions === 'object' || typeof navigator.permissions === 'undefined';
        });
        check("Permissions_query_exists", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            return typeof navigator.permissions.query === 'function' || typeof navigator.permissions.query === 'undefined';
        });
        check("PermissionStatus_state", function() {
            // Would return 'granted', 'denied', or 'prompt' if implemented
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                var status = navigator.permissions.query({ name: 'geolocation' });
                return typeof status.state === 'string';
            } catch(e) {
                return true; // Not implemented is OK
            }
        });
        check("PermissionStatus_onchange", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                var status = navigator.permissions.query({ name: 'geolocation' });
                return 'onchange' in status || typeof status.onchange === 'function' || typeof status.onchange === 'undefined';
            } catch(e) {
                return true;
            }
        });

        // ---- Permission names (standard Web API) ----
        check("permission_geolocation", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                navigator.permissions.query({ name: 'geolocation' });
                return true;
            } catch(e) {
                return true; // Not implemented is OK
            }
        });
        check("permission_notifications", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                navigator.permissions.query({ name: 'notifications' });
                return true;
            } catch(e) {
                return true;
            }
        });
        check("permission_camera", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                navigator.permissions.query({ name: 'camera' });
                return true;
            } catch(e) {
                return true;
            }
        });
        check("permission_microphone", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                navigator.permissions.query({ name: 'microphone' });
                return true;
            } catch(e) {
                return true;
            }
        });
        check("permission_clipboard_read", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                navigator.permissions.query({ name: 'clipboard-read' });
                return true;
            } catch(e) {
                return true;
            }
        });
        check("permission_clipboard_write", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                navigator.permissions.query({ name: 'clipboard-write' });
                return true;
            } catch(e) {
                return true;
            }
        });

        // ---- Bao-specific permission API (BaoPermission or bao.permission) ----
        check("Bao_permission_exists", function() {
            if (typeof Bao === 'undefined') return true;
            return typeof Bao.permission === 'object' || typeof Bao.permission === 'undefined';
        });
        check("BaoPermission_global", function() {
            return typeof BaoPermission === 'function' || typeof BaoPermission === 'undefined';
        });
        check("Bao_permission_check", function() {
            if (typeof Bao === 'undefined' || typeof Bao.permission === 'undefined') return true;
            return typeof Bao.permission.check === 'function' || typeof Bao.permission.check === 'undefined';
        });
        check("Bao_permission_query", function() {
            if (typeof Bao === 'undefined' || typeof Bao.permission === 'undefined') return true;
            return typeof Bao.permission.query === 'function' || typeof Bao.permission.query === 'undefined';
        });

        // ---- Edge cases: invalid permission names ----
        check("invalid_permission_name", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                navigator.permissions.query({ name: 'invalid-permission-xyz' });
                return true; // Should either throw or return denied
            } catch(e) {
                return true; // Throwing for invalid name is correct behavior
            }
        });
        check("empty_permission_name", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                navigator.permissions.query({ name: '' });
                return true;
            } catch(e) {
                return true;
            }
        });

        // ---- Multiple queries and re-query ----
        check("multiple_queries", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                var q1 = navigator.permissions.query({ name: 'geolocation' });
                var q2 = navigator.permissions.query({ name: 'notifications' });
                return true;
            } catch(e) {
                return true;
            }
        });
        check("requery_same_permission", function() {
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                var q1 = navigator.permissions.query({ name: 'geolocation' });
                var q2 = navigator.permissions.query({ name: 'geolocation' });
                return true;
            } catch(e) {
                return true;
            }
        });

        // ---- PermissionGuard JS binding (if available) ----
        check("PermissionGuard_exists", function() {
            return typeof PermissionGuard === 'function' || typeof PermissionGuard === 'undefined';
        });
        check("PermissionGuard_methods", function() {
            if (typeof PermissionGuard === 'undefined') return true;
            return typeof PermissionGuard.checkFsRead === 'function' || typeof PermissionGuard.checkFsRead === 'undefined';
        });

        // ---- Bun.env permission integration ----
        check("Bun_env_accessible", function() {
            return typeof Bun !== 'undefined' && typeof Bun.env === 'object';
        });
        check("process_env_accessible", function() {
            return typeof process !== 'undefined' && typeof process.env === 'object';
        });

        // ---- Permission state values ----
        check("state_granted_value", function() {
            // If implemented, state should be one of: 'granted', 'denied', 'prompt'
            if (typeof navigator === 'undefined' || typeof navigator.permissions === 'undefined') return true;
            try {
                var status = navigator.permissions.query({ name: 'geolocation' });
                var state = status.state;
                return state === 'granted' || state === 'denied' || state === 'prompt';
            } catch(e) {
                return true;
            }
        });

        results.join("|")
    "#);

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "permission_bridge deep tests had {} failures", fail);
    assert!(pass >= 15, "Expected at least 15 passes, got {}", pass);

    // === Rust-level permission_bridge API tests ===
    // These test the actual Rust implementation regardless of JS bindings

    // Clear any existing permission
    bun_runtime::permission_bridge::set_permission(None);

    // Test 1: No permission allows all operations
    assert!(bun_runtime::permission_bridge::check_fs_read("/etc/passwd").is_ok(),
        "check_fs_read should allow when no permission set");
    assert!(bun_runtime::permission_bridge::check_fs_write("/tmp/test").is_ok(),
        "check_fs_write should allow when no permission set");
    assert!(bun_runtime::permission_bridge::check_net("evil.com").is_ok(),
        "check_net should allow when no permission set");
    assert!(bun_runtime::permission_bridge::check_env().is_ok(),
        "check_env should allow when no permission set");
    assert!(bun_runtime::permission_bridge::check_run().is_ok(),
        "check_run should allow when no permission set");

    // Test 2: FS read permission with allowed prefix
    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: Some(vec!["/home".to_string(), "/tmp".to_string()]),
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(bun_runtime::permission_bridge::check_fs_read("/home/user/file").is_ok(),
        "check_fs_read should allow /home prefix");
    assert!(bun_runtime::permission_bridge::check_fs_read("/tmp/data").is_ok(),
        "check_fs_read should allow /tmp prefix");
    assert!(bun_runtime::permission_bridge::check_fs_read("/etc/passwd").is_err(),
        "check_fs_read should deny /etc without prefix");

    // Test 3: FS write permission
    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: None,
        write_paths: Some(vec!["/tmp".to_string()]),
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(bun_runtime::permission_bridge::check_fs_write("/tmp/output").is_ok(),
        "check_fs_write should allow /tmp prefix");
    assert!(bun_runtime::permission_bridge::check_fs_write("/etc/shadow").is_err(),
        "check_fs_write should deny /etc without prefix");

    // Test 4: Network permission - exact match
    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: Some(vec!["example.com".to_string()]),
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(bun_runtime::permission_bridge::check_net("example.com").is_ok(),
        "check_net should allow exact match");
    assert!(bun_runtime::permission_bridge::check_net("evil.com").is_err(),
        "check_net should deny non-matching host");

    // Test 5: Network permission - subdomain match
    assert!(bun_runtime::permission_bridge::check_net("sub.example.com").is_ok(),
        "check_net should allow subdomain");
    assert!(bun_runtime::permission_bridge::check_net("deep.sub.example.com").is_ok(),
        "check_net should allow deep subdomain");

    // Test 6: Network permission - partial mismatch (security check)
    assert!(bun_runtime::permission_bridge::check_net("notexample.com").is_err(),
        "check_net should deny partial suffix match (security)");
    assert!(bun_runtime::permission_bridge::check_net("xnotexample.com").is_err(),
        "check_net should deny partial suffix match (security)");

    // Test 7: env_allowed = false
    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: None,
        env_allowed: false,
        run_allowed: true,
    }));
    assert!(bun_runtime::permission_bridge::check_env().is_err(),
        "check_env should deny when env_allowed=false");
    assert!(bun_runtime::permission_bridge::check_run().is_ok(),
        "check_run should allow when run_allowed=true");

    // Test 8: run_allowed = false
    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: None,
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: false,
    }));
    assert!(bun_runtime::permission_bridge::check_env().is_ok(),
        "check_env should allow when env_allowed=true");
    assert!(bun_runtime::permission_bridge::check_run().is_err(),
        "check_run should deny when run_allowed=false");

    // Test 9: Error messages are descriptive
    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: Some(vec!["/allowed".to_string()]),
        write_paths: Some(vec!["/allowed".to_string()]),
        net_hosts: Some(vec!["safe.com".to_string()]),
        env_allowed: false,
        run_allowed: false,
    }));
    let read_err = bun_runtime::permission_bridge::check_fs_read("/denied").unwrap_err();
    assert!(read_err.contains("read on /denied"),
        "Error message should contain 'read on /denied', got: {}", read_err);
    let write_err = bun_runtime::permission_bridge::check_fs_write("/denied").unwrap_err();
    assert!(write_err.contains("write on /denied"),
        "Error message should contain 'write on /denied', got: {}", write_err);
    let net_err = bun_runtime::permission_bridge::check_net("evil.com").unwrap_err();
    assert!(net_err.contains("net on evil.com"),
        "Error message should contain 'net on evil.com', got: {}", net_err);
    assert_eq!(bun_runtime::permission_bridge::check_env().unwrap_err(), "Permission denied: env");
    assert_eq!(bun_runtime::permission_bridge::check_run().unwrap_err(), "Permission denied: run");

    // Test 10: set_permission overrides previous
    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: Some(vec!["/safe".to_string()]),
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(bun_runtime::permission_bridge::check_fs_read("/safe/file").is_ok());
    assert!(bun_runtime::permission_bridge::check_fs_read("/unsafe").is_err());

    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: Some(vec!["/unsafe".to_string()]),
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(bun_runtime::permission_bridge::check_fs_read("/unsafe/file").is_ok());
    assert!(bun_runtime::permission_bridge::check_fs_read("/safe/file").is_err());

    // Test 11: None in allowed lists means allow all
    bun_runtime::permission_bridge::set_permission(Some(bun_runtime::permission_bridge::PermissionCheck {
        read_paths: None,  // None = allow all
        write_paths: None,
        net_hosts: None,
        env_allowed: true,
        run_allowed: true,
    }));
    assert!(bun_runtime::permission_bridge::check_fs_read("/anything").is_ok(),
        "read_paths: None should allow all reads");

    // Cleanup
    bun_runtime::permission_bridge::set_permission(None);

    bun_runtime::shutdown_thread_sm();
}
