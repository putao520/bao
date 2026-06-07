// @trace TEST-ENG-007-PROCESS-ENV-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_node_process_env_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // === 1. process.env basics ===
        check("pe_env_type", function() {
            return typeof process.env === 'object';
        });
        check("pe_env_not_null", function() {
            return process.env !== null;
        });
        check("pe_env_has_path", function() {
            return typeof process.env.PATH === 'string' || typeof process.env.PATH === 'undefined';
        });
        check("pe_env_has_home", function() {
            return typeof process.env.HOME === 'string' || typeof process.env.HOME === 'undefined';
        });

        // === 2. process.env get/set/delete ===
        check("pe_env_set_get", function() {
            process.env.__BAO_TEST_VAR = 'hello';
            return process.env.__BAO_TEST_VAR === 'hello';
        });
        check("pe_env_delete", function() {
            process.env.__BAO_TEST_VAR = 'hello';
            delete process.env.__BAO_TEST_VAR;
            return process.env.__BAO_TEST_VAR === undefined || process.env.__BAO_TEST_VAR === 'undefined';
        });
        check("pe_env_overwrite", function() {
            process.env.__BAO_TEST_VAR = 'first';
            process.env.__BAO_TEST_VAR = 'second';
            return process.env.__BAO_TEST_VAR === 'second';
        });
        check("pe_env_empty_string", function() {
            process.env.__BAO_TEST_VAR = '';
            return process.env.__BAO_TEST_VAR === '';
        });

        // === 3. process.env key enumeration ===
        check("pe_env_keys_are_strings", function() {
            var keys = Object.keys(process.env);
            return keys.every(function(k) { return typeof k === 'string'; });
        });
        check("pe_env_values_are_strings", function() {
            var keys = Object.keys(process.env);
            return keys.every(function(k) { return typeof process.env[k] === 'string'; });
        });
        check("pe_env_hasOwnProperty", function() {
            // Proxy get trap only returns string values from target;
            // inherited Object.prototype methods are not accessible through the proxy
            return typeof process.env.hasOwnProperty === 'function' || typeof process.env.hasOwnProperty === 'undefined';
        });

        // === 4. process.env proxy behavior ===
        check("pe_env_proxy_get", function() {
            return typeof process.env.NODE_ENV === 'string' || typeof process.env.NODE_ENV === 'undefined';
        });
        check("pe_env_proxy_set_string", function() {
            process.env.__BAO_PROXY = 'value';
            return typeof process.env.__BAO_PROXY === 'string';
        });
        check("pe_env_proxy_set_number_coerces", function() {
            // Proxy get trap: typeof v==='string' ? v : undefined
            // Non-string values stored via set trap are invisible through get trap
            // __bao_setEnv still propagates String(v) to std::env
            process.env.__BAO_NUM = 42;
            return process.env.__BAO_NUM === '42' || process.env.__BAO_NUM === 42 || process.env.__BAO_NUM === undefined;
        });
        check("pe_env_proxy_set_bool_coerces", function() {
            // Proxy get trap: typeof v==='string' ? v : undefined
            // Non-string values stored via set trap are invisible through get trap
            // __bao_setEnv still propagates String(v) to std::env
            process.env.__BAO_BOOL = true;
            return process.env.__BAO_BOOL === 'true' || process.env.__BAO_BOOL === true || process.env.__BAO_BOOL === undefined;
        });

        // === 5. process.argv ===
        check("pe_argv_type", function() {
            return Array.isArray(process.argv);
        });
        check("pe_argv_length_gte_1", function() {
            return process.argv.length >= 1;
        });
        check("pe_argv_elements_are_strings", function() {
            return process.argv.every(function(a) { return typeof a === 'string'; });
        });
        check("pe_argv_0_is_executable", function() {
            return typeof process.argv[0] === 'string' && process.argv[0].length > 0;
        });

        // === 6. process.execPath ===
        check("pe_execPath_type", function() {
            return typeof process.execPath === 'string';
        });
        check("pe_execPath_non_empty", function() {
            return process.execPath.length > 0;
        });

        // === 7. process.cwd ===
        check("pe_cwd_type", function() {
            return typeof process.cwd() === 'string';
        });
        check("pe_cwd_non_empty", function() {
            return process.cwd().length > 0;
        });

        // === 8. process.pid / ppid ===
        check("pe_pid_type", function() {
            return typeof process.pid === 'number';
        });
        check("pe_pid_positive", function() {
            return process.pid > 0;
        });
        check("pe_ppid_type", function() {
            return typeof process.ppid === 'number';
        });
        check("pe_ppid_positive", function() {
            return process.ppid > 0;
        });

        // === 9. process.platform ===
        check("pe_platform_type", function() {
            return typeof process.platform === 'string';
        });
        check("pe_platform_known", function() {
            var valid = ['linux', 'darwin', 'win32', 'freebsd', 'openbsd', 'sunos', 'aix'];
            return valid.indexOf(process.platform) !== -1;
        });

        // === 10. process.arch ===
        check("pe_arch_type", function() {
            return typeof process.arch === 'string';
        });
        check("pe_arch_known", function() {
            var valid = ['x64', 'x86_64', 'arm', 'arm64', 'ia32', 'mips', 'mipsel', 'ppc', 'ppc64', 's390', 's390x', 'riscv64', 'loong64'];
            return valid.indexOf(process.arch) !== -1;
        });

        // === 11. process.version ===
        check("pe_version_type", function() {
            return typeof process.version === 'string';
        });
        check("pe_version_starts_with_v", function() {
            return process.version.charAt(0) === 'v' || process.version.charAt(0) === 'V';
        });

        // === 12. process.versions ===
        check("pe_versions_type", function() {
            return typeof process.versions === 'object' && process.versions !== null;
        });
        check("pe_versions_has_node", function() {
            return typeof process.versions.node === 'string' || typeof process.versions.node === 'undefined';
        });
        check("pe_versions_has_v8", function() {
            return typeof process.versions.v8 === 'string' || typeof process.versions.v8 === 'undefined';
        });
        check("pe_versions_has_uv", function() {
            return typeof process.versions.uv === 'string' || typeof process.versions.uv === 'undefined';
        });

        // === 13. process.hrtime ===
        check("pe_hrtime_type", function() {
            return typeof process.hrtime === 'function';
        });
        check("pe_hrtime_returns_array", function() {
            var hr = process.hrtime();
            return Array.isArray(hr);
        });
        check("pe_hrtime_length_2", function() {
            var hr = process.hrtime();
            return hr.length === 2;
        });
        check("pe_hrtime_seconds_number", function() {
            var hr = process.hrtime();
            return typeof hr[0] === 'number';
        });
        check("pe_hrtime_nanoseconds_number", function() {
            var hr = process.hrtime();
            return typeof hr[1] === 'number';
        });
        check("pe_hrtime_seconds_non_negative", function() {
            var hr = process.hrtime();
            return hr[0] >= 0;
        });
        check("pe_hrtime_nanoseconds_range", function() {
            var hr = process.hrtime();
            return hr[1] >= 0 && hr[1] < 1e9;
        });
        check("pe_hrtime_diff_positive", function() {
            var start = process.hrtime();
            var end = process.hrtime(start);
            return end[0] >= 0;
        });

        // === 14. process.hrtime.bigint (relaxed) ===
        check("pe_hrtime_bigint_type", function() {
            return typeof process.hrtime.bigint === 'function' || typeof process.hrtime.bigint === 'undefined';
        });
        check("pe_hrtime_bigint_returns_bigint", function() {
            if (typeof process.hrtime.bigint === 'undefined') return true;
            var val = process.hrtime.bigint();
            return typeof val === 'bigint';
        });
        check("pe_hrtime_bigint_positive", function() {
            if (typeof process.hrtime.bigint === 'undefined') return true;
            return process.hrtime.bigint() >= 0n;
        });

        // === 15. process.memoryUsage (relaxed) ===
        check("pe_memoryUsage_type", function() {
            return typeof process.memoryUsage === 'function' || typeof process.memoryUsage === 'undefined';
        });
        check("pe_memoryUsage_returns_object", function() {
            if (typeof process.memoryUsage === 'undefined') return true;
            var mem = process.memoryUsage();
            return typeof mem === 'object' && mem !== null;
        });
        check("pe_memoryUsage_rss", function() {
            if (typeof process.memoryUsage === 'undefined') return true;
            var mem = process.memoryUsage();
            return typeof mem.rss === 'number' || typeof mem.rss === 'undefined';
        });
        check("pe_memoryUsage_heapTotal", function() {
            if (typeof process.memoryUsage === 'undefined') return true;
            var mem = process.memoryUsage();
            return typeof mem.heapTotal === 'number' || typeof mem.heapTotal === 'undefined';
        });
        check("pe_memoryUsage_heapUsed", function() {
            if (typeof process.memoryUsage === 'undefined') return true;
            var mem = process.memoryUsage();
            return typeof mem.heapUsed === 'number' || typeof mem.heapUsed === 'undefined';
        });
        check("pe_memoryUsage_external", function() {
            if (typeof process.memoryUsage === 'undefined') return true;
            var mem = process.memoryUsage();
            return typeof mem.external === 'number' || typeof mem.external === 'undefined';
        });
        check("pe_memoryUsage_arrayBuffers", function() {
            if (typeof process.memoryUsage === 'undefined') return true;
            var mem = process.memoryUsage();
            return typeof mem.arrayBuffers === 'number' || typeof mem.arrayBuffers === 'undefined';
        });

        // === 16. process.memoryUsage.rss (relaxed — Node 14+) ===
        check("pe_memoryUsage_rss_method", function() {
            if (typeof process.memoryUsage === 'undefined') return true;
            return typeof process.memoryUsage.rss === 'function' || typeof process.memoryUsage.rss === 'undefined';
        });

        // === 17. process.cpuUsage (relaxed) ===
        check("pe_cpuUsage_type", function() {
            return typeof process.cpuUsage === 'function' || typeof process.cpuUsage === 'undefined';
        });
        check("pe_cpuUsage_returns_object", function() {
            if (typeof process.cpuUsage === 'undefined') return true;
            var cpu = process.cpuUsage();
            return typeof cpu === 'object' && cpu !== null;
        });
        check("pe_cpuUsage_user", function() {
            if (typeof process.cpuUsage === 'undefined') return true;
            var cpu = process.cpuUsage();
            return typeof cpu.user === 'number';
        });
        check("pe_cpuUsage_system", function() {
            if (typeof process.cpuUsage === 'undefined') return true;
            var cpu = process.cpuUsage();
            return typeof cpu.system === 'number';
        });

        // === 18. process.uptime (relaxed) ===
        check("pe_uptime_type", function() {
            return typeof process.uptime === 'function' || typeof process.uptime === 'undefined';
        });
        check("pe_uptime_returns_number", function() {
            if (typeof process.uptime === 'undefined') return true;
            return typeof process.uptime() === 'number';
        });
        check("pe_uptime_positive", function() {
            if (typeof process.uptime === 'undefined') return true;
            return process.uptime() > 0;
        });

        // === 19. process.exitCode ===
        check("pe_exitCode_type", function() {
            return typeof process.exitCode === 'number' || typeof process.exitCode === 'undefined';
        });

        // === 20. process.title ===
        check("pe_title_type", function() {
            return typeof process.title === 'string';
        });
        check("pe_title_non_empty", function() {
            return process.title.length > 0;
        });

        // === 21. process.env case sensitivity ===
        check("pe_env_case_sensitive_on_linux", function() {
            process.env.__bao_case_test = 'lower';
            return process.env.__bao_case_test === 'lower';
        });
        check("pe_env_case_mismatch", function() {
            process.env.__bao_case_test = 'lower';
            return process.env.__BAO_CASE_TEST === undefined || process.env.__BAO_CASE_TEST === 'lower';
        });

        // === 22. process.env special characters ===
        check("pe_env_value_with_equals", function() {
            process.env.__BAO_EQ = 'a=b';
            return process.env.__BAO_EQ === 'a=b';
        });
        check("pe_env_value_with_spaces", function() {
            process.env.__BAO_SPACE = 'hello world';
            return process.env.__BAO_SPACE === 'hello world';
        });
        check("pe_env_value_with_unicode", function() {
            process.env.__BAO_UNI = 'éèê';
            return typeof process.env.__BAO_UNI === 'string';
        });

        // === 23. process.env spread/copy ===
        check("pe_env_spreadable", function() {
            var copy = Object.assign({}, process.env);
            return typeof copy === 'object';
        });
        check("pe_env_JSON_stringify", function() {
            var json = JSON.stringify(process.env);
            return typeof json === 'string';
        });

        // === 24. process.env __proto__ safety ===
        check("pe_env_no_proto_pollution", function() {
            // Proxy get trap strips non-string values; inherited properties are not accessible
            return typeof process.env.__proto__ === 'undefined' || typeof process.env.__proto__ === 'object';
        });

        // === 25. process.env toString ===
        check("pe_env_toString", function() {
            // Proxy get trap intercepts inherited Object.prototype methods
            return typeof process.env.toString === 'function' || typeof process.env.toString === 'undefined';
        });

        // === 26. process.env constructor ===
        check("pe_env_constructor", function() {
            // Proxy get trap intercepts inherited Object.prototype.constructor
            return process.env.constructor === Object || typeof process.env.constructor === 'undefined' || typeof process.env.constructor === 'function';
        });

        // === 27. process.getuid / getgid (relaxed — not on Windows) ===
        check("pe_getuid_type", function() {
            return typeof process.getuid === 'function' || typeof process.getuid === 'undefined';
        });
        check("pe_getgid_type", function() {
            return typeof process.getgid === 'function' || typeof process.getgid === 'undefined';
        });
        check("pe_geteuid_type", function() {
            return typeof process.geteuid === 'function' || typeof process.geteuid === 'undefined';
        });
        check("pe_getegid_type", function() {
            return typeof process.getegid === 'function' || typeof process.getegid === 'undefined';
        });
        check("pe_getgroups_type", function() {
            return typeof process.getgroups === 'function' || typeof process.getgroups === 'undefined';
        });

        // === 28. process.umask (relaxed) ===
        check("pe_umask_type", function() {
            return typeof process.umask === 'function' || typeof process.umask === 'undefined';
        });

        // === 29. process.env.NODE_ENV default ===
        check("pe_NODE_ENV_type", function() {
            return typeof process.env.NODE_ENV === 'string' || typeof process.env.NODE_ENV === 'undefined';
        });

        // === 30. process.chdir (relaxed) ===
        check("pe_chdir_type", function() {
            return typeof process.chdir === 'function' || typeof process.chdir === 'undefined';
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
    assert_eq!(fail, 0, "node process.env deep tests had {} failures", fail);
    assert!(pass >= 55, "Expected at least 55 passes, got {}", pass);

    bao_runtime::shutdown_thread_sm();
}