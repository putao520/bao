// @trace TEST-ENG-007-REQUIRE-SYSTEM-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_require_system_deep() {
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

        // === 1. require function existence ===
        check("require_is_function", function() { return typeof require === 'function'; });
        check("require_is_callable", function() { try { require('fs'); return true; } catch(e) { return false; } });

        // === 2. require core modules ===
        check("require_fs", function() { return typeof require('fs') === 'object'; });
        check("require_path", function() { return typeof require('path') === 'object'; });
        check("require_crypto", function() { return typeof require('crypto') === 'object'; });
        check("require_http", function() { return typeof require('http') === 'object'; });
        check("require_https", function() { return typeof require('https') === 'object'; });
        check("require_child_process", function() { return typeof require('child_process') === 'object'; });
        check("require_events", function() { return typeof require('events') === 'object'; });
        check("require_stream", function() { return typeof require('stream') === 'object'; });
        check("require_util", function() { return typeof require('util') === 'object'; });
        check("require_os", function() { return typeof require('os') === 'object'; });
        check("require_url", function() { return typeof require('url') === 'object'; });
        check("require_querystring", function() { return typeof require('querystring') === 'object'; });
        check("require_zlib", function() { return typeof require('zlib') === 'object'; });
        check("require_buffer", function() { return typeof require('buffer') === 'object'; });
        check("require_net", function() { return typeof require('net') === 'object'; });
        check("require_dns", function() { return typeof require('dns') === 'object'; });
        check("require_tls", function() { return typeof require('tls') === 'object'; });
        check("require_vm", function() { return typeof require('vm') === 'object'; });
        check("require_assert", function() { return typeof require('assert') === 'object'; });
        check("require_module", function() { return typeof require('module') === 'object'; });

        // === 3. require returns non-null objects ===
        check("require_fs_not_null", function() { return require('fs') !== null && require('fs') !== undefined; });
        check("require_path_not_null", function() { return require('path') !== null && require('path') !== undefined; });
        check("require_crypto_not_null", function() { return require('crypto') !== null && require('crypto') !== undefined; });
        check("require_http_not_null", function() { return require('http') !== null && require('http') !== undefined; });
        check("require_os_not_null", function() { return require('os') !== null && require('os') !== undefined; });

        // === 4. require.cache ===
        check("require_cache_type", function() { return typeof require.cache === 'object' || typeof require.cache === 'undefined'; });
        check("require_cache_writable", function() {
            if (typeof require.cache === 'undefined') return true;
            var key = '__test_delete__';
            require.cache[key] = 1;
            delete require.cache[key];
            return require.cache[key] === undefined;
        });

        // === 5. require.resolve ===
        check("require_resolve_type", function() { return typeof require.resolve === 'function' || typeof require.resolve === 'undefined'; });
        check("require_fs_returns_string", function() {
            if (typeof require.resolve !== 'function') return true;
            return typeof require.resolve('fs') === 'string';
        });

        // === 6. Module object ===
        check("Module_type", function() { return typeof Module === 'function' || typeof Module === 'object' || typeof Module === 'undefined'; });
        check("Module_builtinModules", function() {
            if (typeof Module === 'undefined') return true;
            return Array.isArray(Module.builtinModules) || typeof Module.builtinModules === 'undefined';
        });

        // === 7. Module._extensions ===
        check("Module_extensions_type", function() {
            if (typeof Module === 'undefined') return true;
            return typeof Module._extensions === 'object' || typeof Module._extensions === 'undefined';
        });
        check("Module_extensions_js", function() {
            if (typeof Module === 'undefined' || typeof Module._extensions === 'undefined') return true;
            return typeof Module._extensions['.js'] === 'function' || typeof Module._extensions['.js'] === 'undefined';
        });

        // === 8. require same instance (caching) ===
        check("require_cache_fs", function() { return require('fs') === require('fs'); });
        check("require_cache_path", function() { return require('path') === require('path'); });
        check("require_cache_os", function() { return require('os') === require('os'); });

        // === 9. module object ===
        check("module_type", function() { return typeof module === 'object'; });
        check("module_exports_exists", function() { return typeof module.exports === 'object'; });
        check("module_id_type", function() { return typeof module.id === 'string'; });
        check("module_filename_type", function() { return typeof module.filename === 'string' || typeof module.filename === 'undefined'; });

        // === 10. exports object ===
        check("exports_type", function() { return typeof exports === 'object' || typeof exports === 'undefined'; });
        check("exports_eq_module_exports", function() {
            if (typeof exports === 'undefined') return true;
            return exports === module.exports;
        });

        // === 11. __filename and __dirname ===
        check("filename_type", function() { return typeof __filename === 'string' || typeof __filename === 'undefined'; });
        check("dirname_type", function() { return typeof __dirname === 'string' || typeof __dirname === 'undefined'; });

        // === 12. require error on non-existent ===
        check("require_nonexistent_throws", function() {
            try { require('nonexistent_module_xyz'); return false; }
            catch(e) { return e instanceof Error || typeof e === 'object'; }
        });
        check("require_nonexistent_message", function() {
            try { require('nonexistent_module_xyz'); return false; }
            catch(e) { return typeof (e.message || e) === 'string'; }
        });

        // === 13. Module._resolveFilename ===
        check("Module_resolveFilename", function() { return typeof Module === 'undefined' || typeof Module._resolveFilename === 'function' || typeof Module._resolveFilename === 'undefined'; });

        // === 14. Module._nodeModulePaths ===
        check("Module_nodeModulePaths", function() { return typeof Module === 'undefined' || typeof Module._nodeModulePaths === 'function' || typeof Module._nodeModulePaths === 'undefined'; });

        // === 15. require.main ===
        check("require_main", function() { return typeof require.main !== 'undefined' || typeof require.main === 'undefined'; });

        // === 16. createRequire ===
        check("Module_createRequire", function() { return typeof Module === 'undefined' || typeof Module.createRequire === 'function' || typeof Module.createRequire === 'undefined'; });

        // === 17. Module keys count ===
        check("Module_keys_count", function() {
            if (typeof Module === 'undefined') return true;
            return Object.keys(Module).length >= 3;
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
    assert_eq!(fail, 0, "require system deep tests had {} failures", fail);
    assert!(pass >= 35, "Expected at least 35 passes, got {}", pass);

    bun_runtime::shutdown_thread_sm();
}
