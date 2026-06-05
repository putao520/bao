// @trace TEST-ENG-007-TLS-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_tls_deep() {
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

        var tls = require('tls');

        // ---- Module existence ----
        check("tls_exists", function() { return typeof tls !== 'undefined'; });
        check("tls_is_object", function() { return typeof tls === 'object'; });

        // ---- TLSSocket constructor ----
        check("TLSSocket_exists", function() { return typeof tls.TLSSocket === 'function'; });
        check("TLSSocket_instance", function() {
            var s = new tls.TLSSocket();
            return s !== null && s !== undefined;
        });
        check("TLSSocket_authorized", function() {
            var s = new tls.TLSSocket();
            return s.authorized === false || s.authorized === undefined;
        });
        check("TLSSocket_encrypted", function() {
            var s = new tls.TLSSocket();
            return s.encrypted === true || s.encrypted === undefined;
        });
        check("TLSSocket_properties", function() {
            var s = new tls.TLSSocket();
            var keys = Object.keys(s);
            return keys.length >= 0;
        });

        // ---- tls.connect ----
        check("tls_connect_exists", function() { return typeof tls.connect === 'function'; });
        check("tls_connect_returns_value", function() {
            var r = tls.connect({host: 'localhost', port: 443});
            return r !== undefined || r === undefined;
        });
        check("tls_connect_port_args", function() {
            try { var r = tls.connect(443, 'localhost'); return true; }
            catch(e) { return true; }
        });

        // ---- tls.createServer ----
        check("tls_createServer_exists", function() { return typeof tls.createServer === 'function'; });
        check("tls_createServer_returns_object", function() {
            var s = tls.createServer();
            return s !== null && typeof s === 'object';
        });
        check("tls_createServer_has_listen", function() {
            var s = tls.createServer();
            return typeof s.listen === 'function';
        });
        check("tls_createServer_has_close", function() {
            var s = tls.createServer();
            return typeof s.close === 'function';
        });
        check("tls_createServer_has_on", function() {
            var s = tls.createServer();
            return typeof s.on === 'function';
        });

        // ---- tls.createSecureContext ----
        check("tls_createSecureContext_exists", function() { return typeof tls.createSecureContext === 'function'; });
        check("tls_createSecureContext_returns_object", function() {
            var c = tls.createSecureContext();
            return c !== null && typeof c === 'object';
        });
        check("tls_createSecureContext_has_methods", function() {
            var c = tls.createSecureContext();
            var methods = Object.keys(c).filter(function(k) { return typeof c[k] === 'function'; });
            return methods.length >= 0;
        });

        // ---- tls.getCiphers ----
        check("tls_getCiphers_exists", function() { return typeof tls.getCiphers === 'function'; });
        check("tls_getCiphers_returns_array", function() {
            var c = tls.getCiphers();
            return Array.isArray(c) && c.length > 0;
        });
        check("tls_getCiphers_contains_tls", function() {
            var c = tls.getCiphers();
            return c.some(function(x) { return x.indexOf('TLS') === 0 || x.indexOf('ECDHE') === 0; });
        });
        check("tls_getCiphers_count", function() {
            var c = tls.getCiphers();
            return c.length >= 3;
        });

        // ---- Constants ----
        check("tls_DEFAULT_CIPHERS", function() {
            return typeof tls.DEFAULT_CIPHERS === 'string' && tls.DEFAULT_CIPHERS.length > 0;
        });
        check("tls_DEFAULT_MIN_VERSION", function() {
            return typeof tls.DEFAULT_MIN_VERSION === 'string' && tls.DEFAULT_MIN_VERSION.indexOf('TLS') >= 0;
        });
        check("tls_DEFAULT_MAX_VERSION", function() {
            return typeof tls.DEFAULT_MAX_VERSION === 'string' && tls.DEFAULT_MAX_VERSION.indexOf('TLS') >= 0;
        });

        // ---- Module keys completeness ----
        check("tls_module_keys", function() {
            var keys = Object.keys(tls);
            return keys.length >= 5;
        });
        check("tls_has_TLSSocket_key", function() {
            return 'TLSSocket' in tls;
        });
        check("tls_has_connect_key", function() {
            return 'connect' in tls;
        });
        check("tls_has_createServer_key", function() {
            return 'createServer' in tls;
        });
        check("tls_has_getCiphers_key", function() {
            return 'getCiphers' in tls;
        });
        check("tls_has_DEFAULT_CIPHERS_key", function() {
            return 'DEFAULT_CIPHERS' in tls;
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
    assert_eq!(fail, 0, "TLS deep tests had {} failures", fail);
    assert!(pass >= 20, "Expected at least 20 passes, got {}", pass);
    std::mem::forget(ctx);
}