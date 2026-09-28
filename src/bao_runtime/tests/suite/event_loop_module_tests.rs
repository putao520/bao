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

    let results = eval_string(
        &mut ctx,
        r#"
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
    "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "All event loop + module tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}

// ---------------------------------------------------------------------------
// ISSUE #25② — process.nextTick runs on an INDEPENDENT queue (Node
// semantics), not degraded into the promise microtask queue. Ordering
// contract pinned here:
//   a) nextTick(A); Promise.resolve().then(B)  ->  A before B
//   b) a nextTick callback enqueueing another nextTick drains within the
//      same phase (still before the promise microtasks)
//   c) nextTick(cb, ...args) passes the extra arguments through
//   d) a throwing nextTick callback does not block later callbacks (the
//      throw routes to the uncaught-exception hook)
// ---------------------------------------------------------------------------
#[test]
fn test_next_tick_independent_queue_ordering() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    eval_string(
        &mut ctx,
        r#"
        globalThis.__order = [];
        // (a) nextTick before the promise reaction
        process.nextTick(function () { __order.push("t1"); });
        Promise.resolve().then(function () { __order.push("p1"); });
        // (b) recursive nextTick drains within the same phase
        process.nextTick(function () {
          __order.push("t2");
          process.nextTick(function () { __order.push("t3"); });
        });
        // (c) extra arguments pass through
        process.nextTick(function (a, b) { __order.push("args:" + a + b); }, 1, 2);
        // (d) a throw does not block later callbacks
        process.nextTick(function () { __order.push("t4-before-throw"); throw new Error("tick boom"); });
        process.nextTick(function () { __order.push("t5-after-throw"); });
        Promise.resolve().then(function () { __order.push("p2"); });
        "#,
    );

    // The eval tail runs the checkpoint (run_jobs): nextTick phase first,
    // then the promise microtasks (p1 p2). Within the nextTick phase the
    // queue is FIFO (Node semantics): t3 is enqueued by t2 and lands behind
    // args/t4/t5 — it does NOT jump the queue.
    let order = eval_string(&mut ctx, "globalThis.__order.join(',')");

    let expected = "t1,t2,args:12,t4-before-throw,t5-after-throw,t3,p1,p2";
    assert_eq!(
        order, expected,
        "nextTick ordering contract: expected {expected:?}, got {order:?}"
    );
    bun_runtime::shutdown_thread_sm();
}
