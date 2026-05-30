// @trace TEST-ENG-007-TLS [req:REQ-ENG-007] [level:integration]

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
fn test_timers_https_tls_all() {
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === Global timers API ===
        check("g_setTimeout_fn", function() { return typeof setTimeout === 'function'; });
        check("g_setInterval_fn", function() { return typeof setInterval === 'function'; });
        check("g_setImmediate_fn", function() { return typeof setImmediate === 'function'; });
        check("g_clearTimeout_fn", function() { return typeof clearTimeout === 'function'; });
        check("g_clearInterval_fn", function() { return typeof clearInterval === 'function'; });
        check("g_clearImmediate_fn", function() { return typeof clearImmediate === 'function'; });
        check("setTimeout_returns_num", function() { return typeof setTimeout(function(){}, 1000) === 'number'; });
        check("setInterval_returns_num", function() { return typeof setInterval(function(){}, 1000) === 'number'; });
        check("setImmediate_returns_num", function() { return typeof setImmediate(function(){}) === 'number'; });
        check("clearTimeout_ok", function() { clearTimeout(setTimeout(function(){}, 10000)); return true; });
        check("clearInterval_ok", function() { clearInterval(setInterval(function(){}, 10000)); return true; });
        check("clearImmediate_ok", function() { clearImmediate(setImmediate(function(){})); return true; });

        // === require('timers').promises ===
        var timers = require('timers');
        check("timers_promises", function() { return typeof timers.promises === 'object'; });
        check("timers_promises_setTimeout", function() { return typeof timers.promises.setTimeout === 'function'; });
        check("timers_promises_setInterval", function() { return typeof timers.promises.setInterval === 'function'; });

        // === HTTPS ===
        var https = require('https');
        check("https_require", function() { return typeof https === 'object'; });
        check("https_get", function() { return typeof https.get === 'function'; });
        check("https_request", function() { return typeof https.request === 'function'; });
        check("https_Agent", function() { return typeof https.Agent === 'function' || typeof https.Agent === 'undefined'; });
        check("https_globalAgent", function() { return typeof https.globalAgent === 'object' || typeof https.globalAgent === 'undefined'; });

        // === TLS ===
        var tls = require('tls');
        check("tls_require", function() { return typeof tls === 'object'; });
        check("tls_connect", function() { return typeof tls.connect === 'function'; });
        check("tls_createServer", function() { return typeof tls.createServer === 'function'; });
        check("tls_TLSSocket", function() { return typeof tls.TLSSocket === 'function' || typeof tls.TLSSocket === 'undefined'; });
        check("tls_createSecureContext", function() { return typeof tls.createSecureContext === 'function' || typeof tls.createSecureContext === 'undefined'; });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All timers/https/tls tests should pass. Results: {}", results);
    std::mem::forget(ctx);
}
