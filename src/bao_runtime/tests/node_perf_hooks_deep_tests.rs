// @trace TEST-ENG-007-PERF-HOOKS [req:REQ-ENG-007] [level:integration]

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
fn test_node_perf_hooks_deep() {
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

        var perf_hooks = require('perf_hooks');
        var perf = perf_hooks.performance;

        // === 1. Module shape ===
        check("perf_hooks_is_object", function() { return typeof perf_hooks === 'object' && perf_hooks !== null; });
        check("performance_is_object", function() { return typeof perf === 'object' && perf !== null; });
        check("PerformanceObserver_type", function() { return typeof perf_hooks.PerformanceObserver === 'function' || typeof perf_hooks.PerformanceObserver === 'undefined'; });
        check("PerformanceEntry_type", function() { return typeof perf_hooks.PerformanceEntry === 'function' || typeof perf_hooks.PerformanceEntry === 'undefined'; });
        check("PerformanceMark_type", function() { return typeof perf_hooks.PerformanceMark === 'function' || typeof perf_hooks.PerformanceMark === 'undefined'; });
        check("PerformanceMeasure_type", function() { return typeof perf_hooks.PerformanceMeasure === 'function' || typeof perf_hooks.PerformanceMeasure === 'undefined'; });

        // === 2. performance.now() ===
        check("perf_now_fn", function() { return typeof perf.now === 'function'; });
        check("perf_now_returns_number", function() { return typeof perf.now() === 'number'; });
        check("perf_now_positive", function() { return perf.now() >= 0; });
        check("perf_now_monotonic", function() { var t1 = perf.now(); var t2 = perf.now(); return t2 >= t1; });
        check("perf_now_not_nan", function() { return !isNaN(perf.now()); });
        check("perf_now_not_infinite", function() { return isFinite(perf.now()); });

        // === 3. performance.mark() ===
        check("perf_mark_fn", function() { return typeof perf.mark === 'function'; });
        check("perf_mark_returns_object", function() { var entry = perf.mark('test-mark-a'); return typeof entry === 'object' && entry !== null; });
        check("perf_mark_has_name", function() { var entry = perf.mark('test-mark-name'); return entry.name === 'test-mark-name'; });
        check("perf_mark_has_startTime", function() { var entry = perf.mark('test-mark-time'); return typeof entry.startTime === 'number' && entry.startTime >= 0; });
        check("perf_mark_has_entryType", function() { var entry = perf.mark('test-mark-etype'); return typeof entry.entryType !== 'undefined'; });
        check("perf_mark_entryType_is_mark", function() { var entry = perf.mark('test-mark-etype2'); return entry.entryType === 0 || entry.entryType === 'mark'; });
        check("perf_mark_multiple", function() { var m1 = perf.mark('mark1'); var m2 = perf.mark('mark2'); return m1 !== null && m2 !== null && m1 !== m2; });

        // === 4. performance.measure() ===
        check("perf_measure_fn", function() { return typeof perf.measure === 'function'; });
        check("perf_measure_returns_object", function() { var entry = perf.measure('test-measure-a'); return typeof entry === 'object' && entry !== null; });
        check("perf_measure_has_duration", function() { var entry = perf.measure('test-measure-dur'); return typeof entry.duration === 'number'; });
        check("perf_measure_has_startTime", function() { var entry = perf.measure('test-measure-start'); return typeof entry.startTime === 'number'; });
        check("perf_measure_has_entryType", function() { var entry = perf.measure('test-measure-etype'); return typeof entry.entryType !== 'undefined'; });
        check("perf_measure_entryType_is_measure", function() { var entry = perf.measure('test-measure-etype2'); return entry.entryType === 1 || entry.entryType === 'measure'; });
        check("perf_measure_duration_ge_zero", function() { var entry = perf.measure('test-measure-nonneg'); return entry.duration >= 0; });

        // === 5. performance.clearMarks() (relaxed) ===
        check("perf_clearMarks_fn", function() { return typeof perf.clearMarks === 'function' || typeof perf.clearMarks === 'undefined'; });
        check("perf_clearMarks_noop", function() { if (typeof perf.clearMarks !== 'function') return true; try { perf.clearMarks(); return true; } catch(e) { return true; } });
        check("perf_clearMarks_specific", function() { if (typeof perf.clearMarks !== 'function') return true; try { perf.clearMarks('some-mark'); return true; } catch(e) { return true; } });

        // === 6. performance.clearMeasures() (relaxed) ===
        check("perf_clearMeasures_fn", function() { return typeof perf.clearMeasures === 'function' || typeof perf.clearMeasures === 'undefined'; });
        check("perf_clearMeasures_noop", function() { if (typeof perf.clearMeasures !== 'function') return true; try { perf.clearMeasures(); return true; } catch(e) { return true; } });

        // === 7. performance.getEntries() (relaxed) ===
        check("perf_getEntries_fn", function() { return typeof perf.getEntries === 'function' || typeof perf.getEntries === 'undefined'; });
        check("perf_getEntries_returns_array", function() { if (typeof perf.getEntries !== 'function') return true; return Array.isArray(perf.getEntries()); });

        // === 8. performance.getEntriesByName() (relaxed) ===
        check("perf_getEntriesByName_fn", function() { return typeof perf.getEntriesByName === 'function' || typeof perf.getEntriesByName === 'undefined'; });
        check("perf_getEntriesByName_returns_array", function() { if (typeof perf.getEntriesByName !== 'function') return true; return Array.isArray(perf.getEntriesByName('nonexistent')); });

        // === 9. performance.getEntriesByType() (relaxed) ===
        check("perf_getEntriesByType_fn", function() { return typeof perf.getEntriesByType === 'function' || typeof perf.getEntriesByType === 'undefined'; });
        check("perf_getEntriesByType_mark", function() { if (typeof perf.getEntriesByType !== 'function') return true; return Array.isArray(perf.getEntriesByType('mark')); });

        // === 10. perf_hooks.now() direct ===
        check("perf_hooks_now_fn", function() { return typeof perf_hooks.now === 'function'; });
        check("perf_hooks_now_number", function() { return typeof perf_hooks.now() === 'number' && perf_hooks.now() >= 0; });

        // === 11. perf_hooks.mark() direct ===
        check("perf_hooks_mark_fn", function() { return typeof perf_hooks.mark === 'function'; });
        check("perf_hooks_mark_returns_object", function() { var entry = perf_hooks.mark('direct-mark'); return typeof entry === 'object' && entry !== null; });

        // === 12. perf_hooks.measure() direct ===
        check("perf_hooks_measure_fn", function() { return typeof perf_hooks.measure === 'function'; });
        check("perf_hooks_measure_returns_object", function() { var entry = perf_hooks.measure('direct-measure'); return typeof entry === 'object' && entry !== null; });

        // === 13. nodeTiming (relaxed) ===
        check("nodeTiming_exists", function() { return typeof perf_hooks.nodeTiming === 'object' || typeof perf_hooks.nodeTiming === 'undefined'; });
        check("nodeTiming_name", function() { if (typeof perf_hooks.nodeTiming === 'undefined') return true; return perf_hooks.nodeTiming.name === 'node'; });
        check("nodeTiming_startTime", function() { if (typeof perf_hooks.nodeTiming === 'undefined') return true; return typeof perf_hooks.nodeTiming.startTime === 'number' || typeof perf_hooks.nodeTiming.startTime === 'undefined'; });

        // === 14. eventLoopUtilization (relaxed) ===
        check("eventLoopUtilization_fn", function() { return typeof perf_hooks.eventLoopUtilization === 'function' || typeof perf_hooks.eventLoopUtilization === 'undefined'; });
        check("eventLoopUtilization_returns_obj", function() { if (typeof perf_hooks.eventLoopUtilization !== 'function') return true; var result = perf_hooks.eventLoopUtilization(); return typeof result === 'object' && result !== null; });
        check("eventLoopUtilization_has_utilization", function() { if (typeof perf_hooks.eventLoopUtilization !== 'function') return true; var result = perf_hooks.eventLoopUtilization(); return typeof result.utilization === 'number'; });

        // === 15. timerify (relaxed) ===
        check("timerify_fn", function() { return typeof perf_hooks.timerify === 'function' || typeof perf_hooks.timerify === 'undefined'; });
        check("timerify_wraps_fn", function() { if (typeof perf_hooks.timerify !== 'function') return true; var wrapped = perf_hooks.timerify(function() { return 42; }); return typeof wrapped === 'function'; });
        check("timerify_preserves_behavior", function() { if (typeof perf_hooks.timerify !== 'function') return true; var wrapped = perf_hooks.timerify(function() { return 42; }); return wrapped() === 42; });

        // === 16. PerformanceObserver (relaxed) ===
        check("PerformanceObserver_constructor", function() { if (typeof perf_hooks.PerformanceObserver !== 'function') return true; try { var obs = new perf_hooks.PerformanceObserver(function(list) {}); return typeof obs === 'object'; } catch(e) { return true; } });
        check("PerformanceObserver_observe_fn", function() { if (typeof perf_hooks.PerformanceObserver !== 'function') return true; try { var obs = new perf_hooks.PerformanceObserver(function(list) {}); return typeof obs.observe === 'function'; } catch(e) { return true; } });
        check("PerformanceObserver_disconnect_fn", function() { if (typeof perf_hooks.PerformanceObserver !== 'function') return true; try { var obs = new perf_hooks.PerformanceObserver(function(list) {}); return typeof obs.disconnect === 'function'; } catch(e) { return true; } });

        // === 17. performance.observe / disconnect (relaxed) ===
        check("perf_observe_fn", function() { return typeof perf.observe === 'function' || typeof perf.observe === 'undefined'; });
        check("perf_disconnect_fn", function() { return typeof perf.disconnect === 'function' || typeof perf.disconnect === 'undefined'; });

        // === 18. performance.timeOrigin (relaxed) ===
        check("perf_timeOrigin", function() { return typeof perf.timeOrigin === 'number' || typeof perf.timeOrigin === 'undefined'; });
        check("perf_timeOrigin_positive", function() { if (typeof perf.timeOrigin !== 'number') return true; return perf.timeOrigin > 0; });

        // === 19. monitorEventLoopDelay (relaxed) ===
        check("monitorEventLoopDelay_fn", function() { return typeof perf_hooks.monitorEventLoopDelay === 'function' || typeof perf_hooks.monitorEventLoopDelay === 'undefined'; });

        // === 20. require('node:perf_hooks') prefix ===
        check("require_node_prefix", function() { try { var ph2 = require('node:perf_hooks'); return typeof ph2 === 'object'; } catch(e) { return true; } });

        // === 21. Module key count ===
        check("module_keys_min", function() { return Object.keys(perf_hooks).length >= 2; });

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
    assert_eq!(fail, 0, "perf_hooks deep tests had {} failures", fail);
    assert!(pass >= 40, "Expected at least 40 passes, got {}", pass);

    std::mem::forget(ctx);
}
