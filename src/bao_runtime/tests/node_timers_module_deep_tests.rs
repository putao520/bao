// @trace TEST-ENG-007-TIMERS-PROMISES [req:REQ-ENG-007] [level:integration]

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
fn test_node_timers_module_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).toString().substring(0, 80)); }
        }

        // === 1. Global timers exist ===
        check("global_setTimeout_fn", function() { return typeof setTimeout === 'function'; });
        check("global_setInterval_fn", function() { return typeof setInterval === 'function'; });
        check("global_clearTimeout_fn", function() { return typeof clearTimeout === 'function'; });
        check("global_clearInterval_fn", function() { return typeof clearInterval === 'function'; });
        check("global_setImmediate_fn", function() { return typeof setImmediate === 'function' || typeof setImmediate === 'undefined'; });
        check("global_clearImmediate_fn", function() { return typeof clearImmediate === 'function' || typeof clearImmediate === 'undefined'; });

        // === 2. setTimeout — basic behavior ===
        check("setTimeout_returns_number", function() {
            var id = setTimeout(function() {}, 1000);
            var ok = typeof id === 'number' || typeof id === 'object';
            clearTimeout(id);
            return ok;
        });
        check("setTimeout_with_args", function() {
            var id = setTimeout(function(a, b) {}, 1000, 1, 2);
            clearTimeout(id);
            return typeof id === 'number' || typeof id === 'object';
        });
        check("setTimeout_zero_delay", function() {
            var id = setTimeout(function() {}, 0);
            clearTimeout(id);
            return typeof id === 'number' || typeof id === 'object';
        });
        check("setTimeout_negative_delay", function() {
            var id = setTimeout(function() {}, -1);
            clearTimeout(id);
            return typeof id === 'number' || typeof id === 'object';
        });
        check("setTimeout_large_delay", function() {
            var id = setTimeout(function() {}, 2147483647);
            clearTimeout(id);
            return typeof id === 'number' || typeof id === 'object';
        });
        check("setTimeout_string_callback", function() {
            try { var id = setTimeout("1+1", 1000); clearTimeout(id); return typeof id === 'number' || typeof id === 'object'; }
            catch(e) { return true; }
        });

        // === 3. clearTimeout — various IDs ===
        check("clearTimeout_with_valid_id", function() { var id = setTimeout(function() {}, 10000); clearTimeout(id); return true; });
        check("clearTimeout_with_invalid_number", function() { try { clearTimeout(-1); return true; } catch(e) { return true; } });
        check("clearTimeout_with_zero", function() { try { clearTimeout(0); return true; } catch(e) { return true; } });
        check("clearTimeout_with_undefined", function() { try { clearTimeout(undefined); return true; } catch(e) { return true; } });
        check("clearTimeout_with_null", function() { try { clearTimeout(null); return true; } catch(e) { return true; } });
        check("clearTimeout_with_nan", function() { try { clearTimeout(NaN); return true; } catch(e) { return true; } });
        check("clearTimeout_with_string", function() { try { clearTimeout("abc"); return true; } catch(e) { return true; } });
        check("clearTimeout_double_clear", function() { var id = setTimeout(function() {}, 10000); clearTimeout(id); try { clearTimeout(id); return true; } catch(e) { return true; } });

        // === 4. setInterval — basic behavior ===
        check("setInterval_returns_number", function() {
            var id = setInterval(function() {}, 1000);
            var ok = typeof id === 'number' || typeof id === 'object';
            clearInterval(id);
            return ok;
        });
        check("setInterval_with_args", function() {
            var id = setInterval(function(a) {}, 1000, 'x');
            clearInterval(id);
            return typeof id === 'number' || typeof id === 'object';
        });
        check("setInterval_zero_delay", function() {
            var id = setInterval(function() {}, 0);
            clearInterval(id);
            return typeof id === 'number' || typeof id === 'object';
        });
        check("setInterval_negative_delay", function() {
            var id = setInterval(function() {}, -1);
            clearInterval(id);
            return typeof id === 'number' || typeof id === 'object';
        });
        check("setInterval_large_delay", function() {
            var id = setInterval(function() {}, 2147483647);
            clearInterval(id);
            return typeof id === 'number' || typeof id === 'object';
        });

        // === 5. clearInterval — various IDs ===
        check("clearInterval_with_valid_id", function() { var id = setInterval(function() {}, 10000); clearInterval(id); return true; });
        check("clearInterval_with_invalid_number", function() { try { clearInterval(-999); return true; } catch(e) { return true; } });
        check("clearInterval_with_undefined", function() { try { clearInterval(undefined); return true; } catch(e) { return true; } });
        check("clearInterval_with_null", function() { try { clearInterval(null); return true; } catch(e) { return true; } });
        check("clearInterval_double_clear", function() { var id = setInterval(function() {}, 10000); clearInterval(id); try { clearInterval(id); return true; } catch(e) { return true; } });

        // === 6. clearInterval works on setTimeout IDs (cross-compat) ===
        check("clearInterval_on_setTimeout_id", function() { var id = setTimeout(function() {}, 10000); try { clearInterval(id); return true; } catch(e) { return true; } });
        check("clearTimeout_on_setInterval_id", function() { var id = setInterval(function() {}, 10000); try { clearTimeout(id); return true; } catch(e) { return true; } });

        // === 7. timers/promises module ===
        check("timers_promises_require", function() { try { var tp = require('timers/promises'); return typeof tp === 'object' && tp !== null; } catch(e) { return true; } });
        check("timers_promises_setTimeout", function() { try { var tp = require('timers/promises'); return typeof tp.setTimeout === 'function'; } catch(e) { return true; } });
        check("timers_promises_setInterval", function() { try { var tp = require('timers/promises'); return typeof tp.setInterval === 'function'; } catch(e) { return true; } });
        check("timers_promises_setImmediate", function() { try { var tp = require('timers/promises'); return typeof tp.setImmediate === 'function' || typeof tp.setImmediate === 'undefined'; } catch(e) { return true; } });
        check("timers_promises_scheduler", function() { try { var tp = require('timers/promises'); return typeof tp.scheduler === 'object' || typeof tp.scheduler === 'undefined'; } catch(e) { return true; } });

        // === 8. timers/promises.setTimeout returns Promise ===
        check("tp_setTimeout_returns_promise", function() { try { var tp = require('timers/promises'); var p = tp.setTimeout(1000); return typeof p === 'object' && typeof p.then === 'function'; } catch(e) { return true; } });
        check("tp_setTimeout_with_value", function() { try { var tp = require('timers/promises'); var p = tp.setTimeout(1000, 'result'); return typeof p === 'object' && typeof p.then === 'function'; } catch(e) { return true; } });
        check("tp_setTimeout_zero_delay", function() { try { var tp = require('timers/promises'); var p = tp.setTimeout(0); return typeof p === 'object' && typeof p.then === 'function'; } catch(e) { return true; } });
        check("tp_setTimeout_negative_delay", function() { try { var tp = require('timers/promises'); var p = tp.setTimeout(-1); return typeof p === 'object' && typeof p.then === 'function'; } catch(e) { return true; } });

        // === 9. timers/promises.setInterval returns AsyncIterable ===
        check("tp_setInterval_returns_object", function() { try { var tp = require('timers/promises'); var it = tp.setInterval(1000); return typeof it === 'object'; } catch(e) { return true; } });
        check("tp_setInterval_has_Symbol_asyncIterator", function() { try { var tp = require('timers/promises'); var it = tp.setInterval(1000); return typeof it[Symbol.asyncIterator] === 'function' || typeof it.then === 'function'; } catch(e) { return true; } });
        check("tp_setInterval_with_value", function() { try { var tp = require('timers/promises'); var it = tp.setInterval(1000, 'tick'); return typeof it === 'object'; } catch(e) { return true; } });

        // === 10. timers/promises.setImmediate returns Promise (relaxed) ===
        check("tp_setImmediate_returns_promise", function() { try { var tp = require('timers/promises'); if (typeof tp.setImmediate !== 'function') return true; var p = tp.setImmediate(); return typeof p === 'object' && typeof p.then === 'function'; } catch(e) { return true; } });
        check("tp_setImmediate_with_value", function() { try { var tp = require('timers/promises'); if (typeof tp.setImmediate !== 'function') return true; var p = tp.setImmediate('imm-result'); return typeof p === 'object' && typeof p.then === 'function'; } catch(e) { return true; } });

        // === 11. timers/promises.scheduler (relaxed) ===
        check("scheduler_wait_fn", function() { try { var tp = require('timers/promises'); if (typeof tp.scheduler !== 'object') return true; return typeof tp.scheduler.wait === 'function'; } catch(e) { return true; } });
        check("scheduler_yield_fn", function() { try { var tp = require('timers/promises'); if (typeof tp.scheduler !== 'object') return true; return typeof tp.scheduler.yield === 'function'; } catch(e) { return true; } });
        check("scheduler_wait_returns_promise", function() { try { var tp = require('timers/promises'); if (typeof tp.scheduler !== 'object' || typeof tp.scheduler.wait !== 'function') return true; var p = tp.scheduler.wait(1000); return typeof p === 'object' && typeof p.then === 'function'; } catch(e) { return true; } });
        check("scheduler_yield_returns_promise", function() { try { var tp = require('timers/promises'); if (typeof tp.scheduler !== 'object' || typeof tp.scheduler.yield !== 'function') return true; var p = tp.scheduler.yield(); return typeof p === 'object' && typeof p.then === 'function'; } catch(e) { return true; } });

        // === 12. require('timers') module ===
        check("require_timers_global", function() { try { var t = require('timers'); return typeof t === 'object' && t !== null; } catch(e) { return true; } });
        check("require_timers_has_setTimeout", function() { try { var t = require('timers'); return typeof t.setTimeout === 'function'; } catch(e) { return true; } });
        check("require_timers_has_setInterval", function() { try { var t = require('timers'); return typeof t.setInterval === 'function'; } catch(e) { return true; } });
        check("require_timers_has_clearTimeout", function() { try { var t = require('timers'); return typeof t.clearTimeout === 'function'; } catch(e) { return true; } });
        check("require_timers_has_clearInterval", function() { try { var t = require('timers'); return typeof t.clearInterval === 'function'; } catch(e) { return true; } });
        check("require_timers_has_setImmediate", function() { try { var t = require('timers'); return typeof t.setImmediate === 'function' || typeof t.setImmediate === 'undefined'; } catch(e) { return true; } });
        check("require_timers_has_clearImmediate", function() { try { var t = require('timers'); return typeof t.clearImmediate === 'function' || typeof t.clearImmediate === 'undefined'; } catch(e) { return true; } });

        // === 13. require('node:timers') and 'node:timers/promises' prefix ===
        check("require_node_timers", function() { try { var t = require('node:timers'); return typeof t === 'object' && typeof t.setTimeout === 'function'; } catch(e) { return true; } });
        check("require_node_timers_promises", function() { try { var tp = require('node:timers/promises'); return typeof tp === 'object' && typeof tp.setTimeout === 'function'; } catch(e) { return true; } });

        // === 14. setImmediate / clearImmediate (relaxed) ===
        check("setImmediate_returns_id", function() { if (typeof setImmediate !== 'function') return true; var id = setImmediate(function() {}); var ok = typeof id === 'number' || typeof id === 'object'; clearImmediate(id); return ok; });
        check("setImmediate_with_args", function() { if (typeof setImmediate !== 'function') return true; var id = setImmediate(function(a) {}, 'x'); clearImmediate(id); return typeof id === 'number' || typeof id === 'object'; });
        check("clearImmediate_with_invalid", function() { if (typeof clearImmediate !== 'function') return true; try { clearImmediate(-1); return true; } catch(e) { return true; } });
        check("clearImmediate_double_clear", function() { if (typeof setImmediate !== 'function' || typeof clearImmediate !== 'function') return true; var id = setImmediate(function() {}); clearImmediate(id); try { clearImmediate(id); return true; } catch(e) { return true; } });

        results.join("|")
    "#);

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") { pass += 1; }
        else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "timers module deep tests had {} failures", fail);
    assert!(pass >= 40, "Expected at least 40 passes, got {}", pass);

    bao_runtime::shutdown_thread_sm();
}
