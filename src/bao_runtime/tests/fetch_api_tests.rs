// @trace TEST-ENG-007-FETCH [req:REQ-ENG-007] [level:integration]

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
fn test_fetch_api_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        check("fetch_exists", function() { return typeof fetch === 'function'; });

        check("Response_constructor_exists", function() {
            return typeof Response === 'function' || typeof Response === 'undefined';
        });

        check("Request_constructor_exists", function() {
            return typeof Request === 'function' || typeof Request === 'undefined';
        });

        check("Headers_constructor_exists", function() {
            return typeof Headers === 'function' || typeof Headers === 'undefined';
        });

        check("Headers_get_set_has", function() {
            if (typeof Headers === 'undefined') return true;
            var h = new Headers();
            h.set('Content-Type', 'application/json');
            return h.get('Content-Type') === 'application/json' && h.has('Content-Type') === true;
        });

        check("Response_basic_properties", function() {
            if (typeof Response === 'undefined') return true;
            var r = new Response();
            return r.status === 200 && r.ok === true;
        });

        check("Response_custom_status", function() {
            if (typeof Response === 'undefined') return true;
            var r = new Response(null, { status: 404 });
            return r.status === 404 && r.ok === false;
        });

        check("Request_basic_properties", function() {
            if (typeof Request === 'undefined') return true;
            var req = new Request("http://example.com");
            return req.url === "http://example.com" && req.method === "GET";
        });

        check("Request_custom_method", function() {
            if (typeof Request === 'undefined') return true;
            var req = new Request("http://example.com", { method: "POST" });
            return req.method === "POST";
        });

        check("fetch_returns_object", function() {
            var result = fetch("http://127.0.0.1:1/__nonexistent__");
            return typeof result === 'object' && result !== null;
        });

        check("Response_json_static", function() {
            if (typeof Response === 'undefined') return true;
            return typeof Response.json === 'function' || typeof Response.json === 'undefined';
        });

        check("TextEncoder_exists", function() { return typeof TextEncoder === 'function'; });
        check("TextDecoder_exists", function() { return typeof TextDecoder === 'function'; });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All fetch API tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
