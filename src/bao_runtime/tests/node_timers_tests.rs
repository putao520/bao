// @trace TEST-ENG-007-TIMERS [req:REQ-ENG-007] [level:integration]
// Integration tests for node:timers API (REQ-ENG-007)

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
fn test_node_timers_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var timers = require('timers');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === require('timers') ===
        check("timers_require", function() { return typeof timers === 'object'; });
        check("setTimeout", function() { return typeof timers.setTimeout === 'function'; });
        check("clearTimeout", function() { return typeof timers.clearTimeout === 'function'; });
        check("setInterval", function() { return typeof timers.setInterval === 'function'; });
        check("clearInterval", function() { return typeof timers.clearInterval === 'function'; });
        check("setImmediate", function() { return typeof timers.setImmediate === 'function'; });
        check("clearImmediate", function() { return typeof timers.clearImmediate === 'function'; });

        // === timers.promises ===
        check("timers_promises", function() { return typeof timers.promises === 'object'; });
        check("promises_setTimeout", function() { return typeof timers.promises.setTimeout === 'function'; });
        check("promises_setImmediate", function() { return typeof timers.promises.setImmediate === 'function'; });
        check("promises_setInterval", function() { return typeof timers.promises.setInterval === 'function'; });
        check("promises_scheduler", function() { return typeof timers.promises.scheduler === 'object'; });
        check("scheduler_wait", function() { return typeof timers.promises.scheduler.wait === 'function'; });
        check("scheduler_yield", function() { return typeof timers.promises.scheduler.yield === 'function'; });

        // === setTimeout returns a value ===
        check("setTimeout_returns", function() {
            var id = timers.setTimeout(function(){}, 1000);
            return typeof id === 'number' || typeof id === 'object';
        });

        // === setInterval returns a value ===
        check("setInterval_returns", function() {
            var id = timers.setInterval(function(){}, 1000);
            return typeof id === 'number' || typeof id === 'object';
        });

        // === setImmediate returns a value ===
        check("setImmediate_returns", function() {
            var id = timers.setImmediate(function(){});
            return typeof id === 'number' || typeof id === 'object';
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
    assert!(all_passed, "All timers tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
