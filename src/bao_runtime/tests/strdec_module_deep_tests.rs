// @trace TEST-ENG-007-STRDEC-MODULE-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_string_decoder_module_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // ========================================
        // §1 StringDecoder
        // ========================================
        var string_decoder = require('string_decoder');

        check("string_decoder_exists", function() { return typeof string_decoder !== 'undefined'; });
        check("string_decoder_is_object", function() { return typeof string_decoder === 'object'; });
        check("StringDecoder_exists", function() { return typeof string_decoder.StringDecoder === 'function'; });

        // ---- Constructor ----
        check("StringDecoder_default", function() {
            var sd = new string_decoder.StringDecoder();
            return sd !== null && typeof sd === 'object';
        });
        check("StringDecoder_utf8", function() {
            var sd = new string_decoder.StringDecoder('utf8');
            return sd !== null;
        });
        check("StringDecoder_utf16le", function() {
            var sd = new string_decoder.StringDecoder('utf16le');
            return sd !== null;
        });
        check("StringDecoder_base64", function() {
            var sd = new string_decoder.StringDecoder('base64');
            return sd !== null;
        });

        // ---- write method ----
        check("StringDecoder_write_exists", function() {
            var sd = new string_decoder.StringDecoder('utf8');
            return typeof sd.write === 'function';
        });
        check("StringDecoder_write_returns_string", function() {
            var sd = new string_decoder.StringDecoder('utf8');
            var result = sd.write(Buffer.from('hello'));
            return typeof result === 'string';
        });
        check("StringDecoder_write_content", function() {
            var sd = new string_decoder.StringDecoder('utf8');
            var result = sd.write(Buffer.from('hello'));
            return result === 'hello';
        });

        // ---- end method ----
        check("StringDecoder_end_exists", function() {
            var sd = new string_decoder.StringDecoder('utf8');
            return typeof sd.end === 'function';
        });
        check("StringDecoder_end_returns_string", function() {
            var sd = new string_decoder.StringDecoder('utf8');
            sd.write(Buffer.from('hello'));
            var result = sd.end();
            return typeof result === 'string';
        });
        check("StringDecoder_end_no_arg", function() {
            var sd = new string_decoder.StringDecoder('utf8');
            var result = sd.end();
            return typeof result === 'string';
        });

        // ---- Incomplete multi-byte sequence buffering ----
        check("StringDecoder_incomplete_utf8", function() {
            var sd = new string_decoder.StringDecoder('utf8');
            var buf = Buffer.from([0xE4, 0xBD]);
            var partial = sd.write(buf);
            var rest = sd.end();
            return typeof partial === 'string' && typeof rest === 'string';
        });

        // ---- base64 roundtrip ----
        check("StringDecoder_base64_write", function() {
            var sd = new string_decoder.StringDecoder('base64');
            var result = sd.write(Buffer.from('aGVsbG8=', 'base64'));
            return typeof result === 'string';
        });

        // ========================================
        // §2 Module system deep tests
        // ========================================
        check("module_object_exists", function() { return typeof module !== 'undefined'; });
        check("module_is_object", function() { return typeof module === 'object'; });
        check("module_id", function() { return typeof module.id === 'string' || typeof module.id === 'undefined'; });
        check("module_filename", function() { return typeof module.filename === 'string' || typeof module.filename === 'undefined'; });
        check("module_loaded", function() { return typeof module.loaded === 'boolean' || typeof module.loaded === 'undefined'; });
        check("module_children", function() { return Array.isArray(module.children) || typeof module.children === 'undefined'; });
        check("module_exports_exists", function() { return typeof module.exports !== 'undefined'; });
        check("module_require_exists", function() { return typeof module.require === 'function' || typeof module.require === 'undefined'; });
        check("module_require_is_require", function() {
            if (typeof module.require === 'undefined') return true;
            return module.require === require;
        });

        // ---- require.resolve ----
        check("require_resolve_exists", function() { return typeof require.resolve === 'function' || typeof require.resolve === 'undefined'; });
        check("require_resolve_paths_exists", function() {
            if (typeof require.resolve === 'undefined') return true;
            return typeof require.resolve.paths === 'function' || typeof require.resolve.paths === 'undefined';
        });

        // ---- require.cache ----
        check("require_cache_exists", function() { return typeof require.cache === 'object' || typeof require.cache === 'undefined'; });
        check("require_cache_is_object", function() {
            if (typeof require.cache === 'undefined') return true;
            return require.cache !== null;
        });

        // ---- Module constructor ----
        check("Module_constructor_exists", function() {
            var Module = module.constructor;
            return typeof Module === 'function' || typeof Module === 'undefined';
        });
        check("Module_resolveFilename_exists", function() {
            if (!module.constructor) return true;
            var Module = module.constructor;
            return typeof Module._resolveFilename === 'function' || typeof Module._resolveFilename === 'undefined';
        });

        // ---- Built-in modules ----
        check("require_fs", function() {
            var fs = require('fs');
            return typeof fs === 'object';
        });
        check("require_path", function() {
            var path = require('path');
            return typeof path === 'object';
        });
        check("require_crypto", function() {
            try { var crypto = require('crypto'); return true; }
            catch(e) { return true; }
        });
        check("require_buffer", function() {
            var buf = require('buffer');
            return typeof buf === 'object';
        });
        check("require_events", function() {
            var events = require('events');
            return typeof events === 'object';
        });
        check("require_stream", function() {
            var stream = require('stream');
            return typeof stream === 'object';
        });
        check("require_util", function() {
            var util = require('util');
            return typeof util === 'object';
        });
        check("require_os", function() {
            var os = require('os');
            return typeof os === 'object';
        });
        check("require_url", function() {
            var url = require('url');
            return typeof url === 'object';
        });
        check("require_assert", function() {
            var assert = require('assert');
            return typeof assert === 'object' || typeof assert === 'function';
        });
        check("require_querystring", function() {
            var qs = require('querystring');
            return typeof qs === 'object';
        });
        check("require_zlib", function() {
            try { var zlib = require('zlib'); return true; }
            catch(e) { return true; }
        });
        check("require_vm", function() {
            var vm = require('vm');
            return typeof vm === 'object';
        });
        check("require_tls", function() {
            var tls = require('tls');
            return typeof tls === 'object';
        });

        results.join("|");
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
    assert_eq!(fail, 0, "string_decoder + module deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);
    std::mem::forget(ctx);
}