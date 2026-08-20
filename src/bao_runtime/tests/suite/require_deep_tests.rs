// @trace TEST-ENG-REQUIRE [req:REQ-ENG-005] [level:integration]

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
fn test_require_deep() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 50)); }
        }

        // === require function ===
        check("require_is_function", function() { return typeof require === 'function'; });

        // === built-in module loading ===
        check("require_fs", function() { return typeof require('fs') === 'object'; });
        check("require_path", function() { return typeof require('path') === 'object'; });
        check("require_os", function() { return typeof require('os') === 'object'; });
        check("require_url", function() { return typeof require('url') === 'object'; });
        check("require_util", function() { return typeof require('util') === 'object'; });
        check("require_assert", function() { return typeof require('assert') === 'object'; });
        check("require_buffer", function() { return typeof require('buffer') === 'object'; });
        check("require_crypto", function() { return typeof require('crypto') === 'object'; });
        check("require_events", function() { return typeof require('events') === 'object'; });
        check("require_stream", function() { return typeof require('stream') === 'object'; });
        check("require_dns", function() { return typeof require('dns') === 'object'; });
        check("require_net", function() { return typeof require('net') === 'object'; });
        check("require_http", function() { return typeof require('http') === 'object'; });
        check("require_https", function() { return typeof require('https') === 'object'; });
        check("require_child_process", function() { return typeof require('child_process') === 'object'; });
        check("require_querystring", function() { return typeof require('querystring') === 'object'; });
        check("require_timers", function() { return typeof require('timers') === 'object'; });

        // === node: prefix ===
        check("require_node_fs", function() { return typeof require('node:fs') === 'object'; });
        check("require_node_path", function() { return typeof require('node:path') === 'object'; });
        check("require_node_os", function() { return typeof require('node:os') === 'object'; });
        check("require_node_http", function() { return typeof require('node:http') === 'object'; });
        check("node_prefix_same_as_bare", function() { return require('node:fs') === require('fs'); });

        // === assert/strict sub-path ===
        check("require_assert_strict", function() { return typeof require('assert/strict') === 'object'; });
        check("assert_strict_has_equal", function() { return typeof require('assert/strict').equal === 'function'; });

        // === require caching ===
        check("require_cache_same", function() {
            var a = require('fs');
            var b = require('fs');
            return a === b;
        });

        // === module object ===
        check("module_exists", function() { return typeof module === 'object'; });
        check("module_exports_exists", function() { return typeof module.exports === 'object'; });
        check("module_id_type", function() { return typeof module.id === 'string'; });

        // === require.resolve (BUG-ENG-365: now implemented) ===
        check("require_resolve_is_function", function() { return typeof require.resolve === 'function'; });

        // === require.cache (REAL since 23b76dbd — Node semantics) ===
        // A per-context singleton object; file modules are recorded under
        // their canonical path as { id, filename, exports, loaded };
        // builtins are NOT recorded (they never appear in require.cache).
        check("require_cache_is_object", function() {
            return typeof require.cache === 'object' && require.cache !== null;
        });
        check("require_cache_singleton_writable", function() {
            require.cache.__deep_probe = 1;
            return require.cache.__deep_probe === 1;
        });
        check("require_cache_file_module_semantics", function() {
            var fs = require('fs');
            var os = require('os');
            var path = require('path');
            var modPath = path.join(os.tmpdir(), 'require_deep_cache_probe_' + Date.now() + '.js');
            fs.writeFileSync(modPath, 'module.exports = { marker: 42 };');
            try {
                var a = require(modPath);
                var b = require(modPath);
                var keys = Object.keys(require.cache).filter(function(k) {
                    return k.indexOf('require_deep_cache_probe_') >= 0;
                });
                if (keys.length !== 1) return false;
                var entry = require.cache[keys[0]];
                return a === b                          // second require hits the cache (reference equal)
                    && entry.exports === a              // cache entry's exports IS the require return
                    && entry.id === keys[0]             // entry.id is the canonical path key
                    && a.marker === 42;                 // the loaded module's real exports
            } finally {
                try { fs.rmSync(modPath); } catch (e) {}
            }
        });
        check("require_cache_builtin_absent", function() {
            require('fs'); require('node:path');        // builtins load...
            return !('fs' in require.cache)             // ...but never appear in require.cache
                && !('node:path' in require.cache)
                && !('path' in require.cache);
        });

        // === unknown module throws ===
        check("require_unknown_throws", function() {
            try { require('nonexistent_module_xyz'); return false; }
            catch(e) { return true; }
        });

        // === module.exports round-trip ===
        check("module_exports_roundtrip", function() {
            module.exports.testVal = 42;
            return module.exports.testVal === 42;
        });

        results.join("|")
    "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "All require deep tests should pass. Results: {}",
        results
    );

    bun_runtime::shutdown_thread_sm();
}
