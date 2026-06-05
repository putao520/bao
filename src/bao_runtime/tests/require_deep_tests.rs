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
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
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

        // === require.resolve (not yet implemented) ===
        check("require_resolve_not_impl", function() { return typeof require.resolve === 'undefined'; });

        // === require.cache (not yet implemented) ===
        check("require_cache_not_impl", function() { return typeof require.cache === 'undefined'; });

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
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All require deep tests should pass. Results: {}", results);

    std::mem::forget(ctx);
}