// @trace TEST-ENG-TIMERS-DEEP [req:REQ-ENG-007] [level:integration]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => {
            if b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        _ => String::new(),
    }
}

#[test]
fn test_timers_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === Global timers API existence (type checks) ===
        check("setTimeout_fn", function() { return typeof setTimeout === 'function'; });
        check("clearTimeout_fn", function() { return typeof clearTimeout === 'function'; });
        check("setInterval_fn", function() { return typeof setInterval === 'function'; });
        check("clearInterval_fn", function() { return typeof clearInterval === 'function'; });
        check("setImmediate_fn", function() { return typeof setImmediate === 'function'; });
        check("clearImmediate_fn", function() { return typeof clearImmediate === 'function'; });

        // === Return types ===
        check("setTimeout_returns_num", function() {
            return typeof setTimeout(function(){}, 0) === 'number';
        });
        check("setTimeout_positive", function() {
            return setTimeout(function(){}, 0) > 0;
        });
        check("setTimeout_ids_sequential", function() {
            var id1 = setTimeout(function(){}, 0);
            var id2 = setTimeout(function(){}, 0);
            return id2 > id1;
        });
        check("setInterval_returns_num", function() {
            return typeof setInterval(function(){}, 100) === 'number';
        });
        check("setImmediate_returns_num", function() {
            return typeof setImmediate(function(){}) === 'number';
        });

        // === clearTimeout behavior ===
        check("clearTimeout_returns_undefined", function() {
            return typeof clearTimeout(1) === 'undefined';
        });
        check("clearTimeout_no_args", function() {
            clearTimeout();
            return true;
        });
        check("clearTimeout_invalid_id", function() {
            clearTimeout(99999);
            return true;
        });

        // === Timer ID differentiation ===
        check("setInterval_different_id", function() {
            var tid = setTimeout(function(){}, 0);
            var iid = setInterval(function(){}, 100);
            return iid !== tid && iid > 0 && tid > 0;
        });

        // === Default delay ===
        check("setTimeout_no_delay", function() {
            var id = setTimeout(function(){});
            return typeof id === 'number' && id > 0;
        });

        // === setImmediate with args (no crash) ===
        check("setImmediate_with_args", function() {
            setImmediate(function(a, b){}, 'x', 'y');
            return true;
        });

        // === window.setTimeout aliasing ===
        check("window_setTimeout", function() {
            if (typeof window === 'undefined') { return true; }
            return window.setTimeout === setTimeout;
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
    assert!(
        all_passed,
        "All deep timers tests should pass. Results: {}",
        results
    );
    bao_runtime::shutdown_thread_sm();
}
