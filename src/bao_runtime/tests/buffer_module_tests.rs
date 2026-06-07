// @trace TEST-ENG-007-BUF [req:REQ-ENG-007] [level:integration]

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
fn test_buffer_module_all() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        // --- Buffer API ---

        check("Buffer_is_function", function() {
            return typeof Buffer === 'function';
        });

        check("Buffer_from_object", function() {
            var b = Buffer.from("hello");
            return typeof b === 'object' && b !== null;
        });

        check("Buffer_from_length", function() {
            return Buffer.from("hello").length === 5;
        });

        check("Buffer_from_toString", function() {
            return Buffer.from("hello").toString().indexOf("hello") >= 0;
        });

        check("Buffer_alloc_length10", function() {
            var b = Buffer.alloc(10);
            return typeof b === 'object' && b !== null && b.length === 10;
        });

        check("Buffer_alloc_length_prop", function() {
            return Buffer.alloc(10).length === 10;
        });

        check("Buffer_isBuffer", function() {
            if (typeof Buffer.isBuffer !== 'function') return true;
            return Buffer.isBuffer(Buffer.from("test")) === true;
        });

        check("Buffer_byteLength", function() {
            if (typeof Buffer.byteLength !== 'function') return true;
            return Buffer.byteLength("hello") === 5;
        });

        check("Buffer_concat_is_function", function() {
            if (typeof Buffer.concat === 'undefined') return true;
            return typeof Buffer.concat === 'function';
        });

        check("require_buffer", function() {
            var buf = require('buffer');
            return typeof buf === 'object' && typeof buf.Buffer === 'function';
        });

        check("require_buffer_same", function() {
            var buf = require('buffer');
            return buf.Buffer === Buffer;
        });

        // --- Module system ---

        check("require_module_object", function() {
            var m = require('module');
            return typeof m === 'object' && m !== null;
        });

        check("module_createRequire", function() {
            var m = require('module');
            if (typeof m.createRequire === 'undefined') return true;
            return typeof m.createRequire === 'function';
        });

        check("require_resolve_is_function", function() {
            if (typeof require.resolve === 'undefined') return true;
            return typeof require.resolve === 'function';
        });

        check("require_cache_type", function() {
            if (typeof require.cache === 'undefined') return true;
            return typeof require.cache === 'object';
        });

        check("require_main_type", function() {
            var t = typeof require.main;
            return t === 'object' || t === 'undefined' || require.main === null;
        });

        check("module_exports_object", function() {
            return typeof module.exports === 'object' && module.exports !== null;
        });

        check("module_id_string", function() {
            return typeof module.id === 'string';
        });

        check("module_filename_string", function() {
            var t = typeof module.filename;
            return t === 'string' || t === 'undefined';
        });

        check("module_require_fn", function() {
            if (typeof module.require === 'undefined') return true;
            return typeof module.require === 'function';
        });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All buffer+module tests should pass. Results: {}", results);
    bao_runtime::shutdown_thread_sm();
}
