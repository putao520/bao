// @trace TEST-ENG-004 [req:REQ-ENG-004] [level:integration]
// @trace TEST-ENG-005 [req:REQ-ENG-005] [level:integration]
// Integration tests for Event Loop bridge and Module Loader bridge

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
fn test_event_loop_and_modules() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === Event Loop (REQ-ENG-004) ===
        check("setTimeout_fn", function() { return typeof setTimeout === 'function'; });
        check("setInterval_fn", function() { return typeof setInterval === 'function'; });
        check("setImmediate_fn", function() { return typeof setImmediate === 'function'; });
        check("clearTimeout_fn", function() { return typeof clearTimeout === 'function'; });
        check("clearInterval_fn", function() { return typeof clearInterval === 'function'; });
        check("clearImmediate_fn", function() { return typeof clearImmediate === 'function'; });
        check("timer_id_number", function() { var id = setTimeout(function(){}, 1000); clearTimeout(id); return typeof id === 'number'; });
        check("interval_id_number", function() { var id = setInterval(function(){}, 1000); clearInterval(id); return typeof id === 'number'; });
        check("process_nextTick", function() { return typeof process.nextTick === 'function'; });
        check("Promise_exists", function() { return typeof Promise === 'function'; });
        check("Promise_resolve", function() { return typeof Promise.resolve === 'function'; });
        check("Promise_reject", function() { return typeof Promise.reject === 'function'; });
        check("Promise_then", function() { return typeof Promise.resolve().then === 'function'; });
        check("Promise_catch", function() { return typeof Promise.reject().catch === 'function'; });
        check("queueMicrotask_fn", function() { return typeof queueMicrotask === 'function'; });

        // === Module Loader (REQ-ENG-005) ===
        check("require_fn", function() { return typeof require === 'function'; });
        check("req_path", function() { var p = require('path'); return typeof p === 'object' && typeof p.join === 'function'; });
        check("req_fs", function() { return typeof require('fs') === 'object'; });
        check("req_crypto", function() { return typeof require('crypto') === 'object'; });
        check("req_events", function() { var e = require('events'); return typeof e === 'object' || typeof e === 'function'; });
        check("req_url", function() { return typeof require('url') === 'object'; });
        check("req_util", function() { return typeof require('util') === 'object'; });
        check("req_os", function() { return typeof require('os') === 'object'; });
        check("req_stream", function() { return typeof require('stream') === 'object'; });
        check("req_buffer", function() { return typeof require('buffer') === 'object'; });
        check("req_assert", function() { var a = require('assert'); return typeof a === 'object' || typeof a === 'function'; });
        check("req_dns", function() { return typeof require('dns') === 'object'; });
        check("req_net", function() { return typeof require('net') === 'object'; });
        check("req_child_process", function() { return typeof require('child_process') === 'object'; });
        check("req_timers", function() { return typeof require('timers') === 'object'; });
        check("req_querystring", function() { return typeof require('querystring') === 'object'; });
        check("req_module", function() { return typeof require('module') === 'object'; });
        check("module_caching", function() { return require('path') === require('path'); });
        check("module_exports", function() { return typeof module === 'object' && typeof module.exports === 'object'; });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All event loop + module tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
