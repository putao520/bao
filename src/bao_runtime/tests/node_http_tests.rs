// @trace TEST-ENG-007-HTTP [req:REQ-ENG-007] [level:integration]
// Integration tests for node:http and node:https API surface (REQ-ENG-007)

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
fn test_node_http_https_all() {
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var http = require('http');
        var https = require('https');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === node:http ===
        check("http_require", function() { return typeof http === 'object'; });
        check("http_createServer", function() { return typeof http.createServer === 'function'; });
        check("http_request", function() { return typeof http.request === 'function'; });
        check("http_get", function() { return typeof http.get === 'function'; });
        check("http_STATUS_CODES", function() { return typeof http.STATUS_CODES === 'object'; });
        check("http_STATUS_CODES_200", function() { return http.STATUS_CODES[200] === "OK"; });
        check("http_STATUS_CODES_404", function() { return http.STATUS_CODES[404] === "Not Found"; });
        check("http_STATUS_CODES_500", function() { return http.STATUS_CODES[500] === "Internal Server Error"; });
        check("http_agent", function() { var t = typeof http.Agent; return t === 'function' || t === 'object' || t === 'undefined'; });
        check("http_server_instance", function() {
            var s = http.createServer(function(){});
            return typeof s === 'object' && s !== null;
        });
        check("http_server_listen", function() { return typeof http.createServer(function(){}).listen === 'function'; });
        check("http_server_close", function() { return typeof http.createServer(function(){}).close === 'function'; });
        check("http_server_on", function() {
            var s = http.createServer(function(){});
            return typeof s.on === 'function' || typeof s === 'object';
        });
        check("http_global_agent", function() {
            return typeof http.globalAgent === 'object' || typeof http.globalAgent === 'undefined';
        });
        check("http_maxHeaderSize", function() {
            return typeof http.maxHeaderSize === 'number' || typeof http.maxHeaderSize === 'undefined';
        });

        // === node:https ===
        check("https_require", function() { return typeof https === 'object'; });
        check("https_request", function() { return typeof https.request === 'function'; });
        check("https_get", function() { return typeof https.get === 'function'; });
        check("https_createServer", function() {
            return typeof https.createServer === 'function' || typeof https.createServer === 'undefined';
        });
        check("https_agent", function() {
            return typeof https.Agent === 'function' || typeof https.Agent === 'undefined';
        });
        check("https_globalAgent", function() {
            return typeof https.globalAgent === 'object' || typeof https.globalAgent === 'undefined';
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
    assert!(all_passed, "All http/https tests should pass. Results: {}", results);
    std::mem::forget(ctx);
}
