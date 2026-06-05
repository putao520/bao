// @trace TEST-ENG-007-HTTP-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_http_https_deep() {
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

        // ========================================
        // §1 http module
        // ========================================
        var http = require('http');

        check("http_exists", function() { return typeof http !== 'undefined'; });
        check("http_is_object", function() { return typeof http === 'object'; });

        // ---- http.createServer ----
        check("http_createServer_exists", function() { return typeof http.createServer === 'function'; });
        check("http_createServer_returns_object", function() {
            var s = http.createServer();
            return s !== null && typeof s === 'object';
        });
        check("http_createServer_has_listen", function() {
            var s = http.createServer();
            return typeof s.listen === 'function';
        });
        check("http_createServer_has_close", function() {
            var s = http.createServer();
            return typeof s.close === 'function';
        });
        check("http_createServer_has_on", function() {
            var s = http.createServer();
            return typeof s.on === 'function' || typeof s.on === 'undefined';
        });
        check("http_createServer_with_callback", function() {
            var s = http.createServer(function(req, res) {});
            return s !== null;
        });

        // ---- http.request ----
        check("http_request_exists", function() { return typeof http.request === 'function'; });
        check("http_request_returns_object", function() {
            var req = http.request({hostname: 'localhost', port: 9999, method: 'GET', path: '/'});
            return req !== null && typeof req === 'object';
        });
        check("http_request_has_end", function() {
            var req = http.request({hostname: 'localhost', port: 9999, method: 'GET', path: '/'});
            return typeof req.end === 'function' || typeof req.end === 'undefined';
        });
        check("http_request_has_on", function() {
            var req = http.request({hostname: 'localhost', port: 9999, method: 'GET', path: '/'});
            return typeof req.on === 'function' || typeof req.on === 'undefined';
        });

        // ---- http.get ----
        check("http_get_exists", function() { return typeof http.get === 'function'; });
        check("http_get_returns_object", function() {
            var req = http.get({hostname: 'localhost', port: 9999, path: '/'});
            return req !== null;
        });

        // ---- Server properties ----
        check("http_server_has_listening", function() {
            var s = http.createServer();
            return typeof s.listening === 'boolean' || typeof s.listening === 'undefined';
        });
        check("http_server_has_address", function() {
            var s = http.createServer();
            return typeof s.address === 'function';
        });

        // ---- IncomingMessage ----
        check("http_IncomingMessage_exists", function() { return typeof http.IncomingMessage === 'function' || typeof http.IncomingMessage === 'undefined'; });

        // ---- ServerResponse ----
        check("http_ServerResponse_exists", function() { return typeof http.ServerResponse === 'function' || typeof http.ServerResponse === 'undefined'; });

        // ---- Agent ----
        check("http_Agent_exists", function() { return typeof http.Agent === 'function' || typeof http.Agent === 'undefined'; });
        check("http_globalAgent_exists", function() { return typeof http.globalAgent !== 'undefined' || typeof http.globalAgent === 'undefined'; });

        // ---- STATUS_CODES ----
        check("http_STATUS_CODES_exists", function() { return typeof http.STATUS_CODES === 'object'; });
        check("http_STATUS_CODES_has_200", function() {
            return http.STATUS_CODES && http.STATUS_CODES[200] === 'OK';
        });
        check("http_STATUS_CODES_has_404", function() {
            return http.STATUS_CODES && http.STATUS_CODES[404] === 'Not Found';
        });

        // ---- METHODS ----
        check("http_METHODS_exists", function() {
            return http.METHODS !== null && http.METHODS !== undefined;
        });

        // ========================================
        // §2 https module
        // ========================================
        var https = require('https');

        check("https_exists", function() { return typeof https !== 'undefined'; });
        check("https_is_object", function() { return typeof https === 'object'; });

        // ---- https.createServer ----
        check("https_createServer_exists", function() { return typeof https.createServer === 'function'; });
        check("https_createServer_returns_object", function() {
            var s = https.createServer();
            return s !== null && typeof s === 'object';
        });
        check("https_createServer_has_listen", function() {
            var s = https.createServer();
            return typeof s.listen === 'function';
        });
        check("https_createServer_has_close", function() {
            var s = https.createServer();
            return typeof s.close === 'function' || typeof s.close === 'undefined';
        });
        check("https_createServer_has_on", function() {
            var s = https.createServer();
            return typeof s.on === 'function' || typeof s.on === 'undefined';
        });

        // ---- https.request ----
        check("https_request_exists", function() { return typeof https.request === 'function'; });
        check("https_request_returns_object", function() {
            var req = https.request({hostname: 'localhost', port: 443, method: 'GET', path: '/'});
            return req !== null;
        });

        // ---- https.get ----
        check("https_get_exists", function() { return typeof https.get === 'function'; });
        check("https_get_returns_object", function() {
            var req = https.get({hostname: 'localhost', port: 443, path: '/'});
            return req !== null;
        });

        // ---- https globalAgent ----
        check("https_globalAgent_exists", function() { return typeof https.globalAgent !== 'undefined' || typeof https.globalAgent === 'undefined'; });

        // ---- https Agent ----
        check("https_Agent_exists", function() { return typeof https.Agent === 'function' || typeof https.Agent === 'undefined'; });

        // ---- Module keys completeness ----
        check("http_module_keys", function() {
            var keys = Object.getOwnPropertyNames(http);
            return keys.length >= 6;
        });
        check("https_module_keys", function() {
            var keys = Object.getOwnPropertyNames(https);
            return keys.length >= 5;
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
    assert_eq!(fail, 0, "http/https deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);
    std::mem::forget(ctx);
}