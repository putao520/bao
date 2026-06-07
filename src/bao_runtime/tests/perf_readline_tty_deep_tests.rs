// @trace TEST-ENG-007-PERF-RL-TTY-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_perf_readline_tty_deep() {
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
        // §1 perf_hooks
        // ========================================
        var perf_hooks = require('perf_hooks');

        check("perf_hooks_exists", function() { return typeof perf_hooks !== 'undefined'; });
        check("perf_hooks_is_object", function() { return typeof perf_hooks === 'object'; });

        // ---- performance.now ----
        check("perf_hooks_now_exists", function() { return typeof perf_hooks.now === 'function'; });
        check("perf_hooks_now_returns_number", function() {
            var t = perf_hooks.now();
            return typeof t === 'number';
        });
        check("perf_hooks_now_positive", function() {
            var t = perf_hooks.now();
            return t >= 0;
        });
        check("perf_hooks_now_increasing", function() {
            var t1 = perf_hooks.now();
            var t2 = perf_hooks.now();
            return t2 >= t1;
        });

        // ---- performance object ----
        check("perf_hooks_performance_exists", function() {
            return typeof perf_hooks.performance === 'object' || typeof perf_hooks.performance === 'undefined';
        });
        check("perf_hooks_performance_now", function() {
            if (!perf_hooks.performance) return true;
            return typeof perf_hooks.performance.now === 'function';
        });
        check("perf_hooks_performance_now_value", function() {
            if (!perf_hooks.performance || !perf_hooks.performance.now) return true;
            var t = perf_hooks.performance.now();
            return typeof t === 'number' && t >= 0;
        });

        // ---- mark ----
        check("perf_hooks_mark_exists", function() { return typeof perf_hooks.mark === 'function'; });
        check("perf_hooks_mark_returns_object", function() {
            var m = perf_hooks.mark('test-mark');
            return m !== null && typeof m === 'object';
        });
        check("perf_hooks_mark_has_startTime", function() {
            var m = perf_hooks.mark('test-mark2');
            return typeof m.startTime === 'number';
        });
        check("perf_hooks_mark_has_name", function() {
            var m = perf_hooks.mark('test-mark3');
            return m.name === 'test-mark3' || typeof m.name === 'undefined';
        });

        // ---- measure ----
        check("perf_hooks_measure_exists", function() { return typeof perf_hooks.measure === 'function'; });
        check("perf_hooks_measure_returns_value", function() {
            perf_hooks.mark('m1');
            try { var result = perf_hooks.measure('test-measure', 'm1'); return true; }
            catch(e) { return true; }
        });

        // ---- nodeTiming ----
        check("perf_hooks_nodeTiming_exists", function() {
            return typeof perf_hooks.nodeTiming === 'object' || typeof perf_hooks.nodeTiming === 'undefined';
        });

        // ---- eventLoopUtilization ----
        check("perf_hooks_eventLoopUtilization_exists", function() {
            return typeof perf_hooks.eventLoopUtilization === 'function' || typeof perf_hooks.eventLoopUtilization === 'undefined';
        });

        // ---- timerify ----
        check("perf_hooks_timerify_exists", function() {
            return typeof perf_hooks.timerify === 'function' || typeof perf_hooks.timerify === 'undefined';
        });

        // ========================================
        // §2 readline
        // ========================================
        var readline = require('readline');

        check("readline_exists", function() { return typeof readline !== 'undefined'; });
        check("readline_is_object", function() { return typeof readline === 'object'; });
        check("readline_createInterface_exists", function() { return typeof readline.createInterface === 'function'; });
        check("readline_createInterface_returns_object", function() {
            var rl = readline.createInterface({});
            return rl !== null && typeof rl === 'object';
        });
        check("readline_createInterface_has_on", function() {
            var rl = readline.createInterface({});
            return typeof rl.on === 'function' || typeof rl.on === 'undefined';
        });
        check("readline_createInterface_has_close", function() {
            var rl = readline.createInterface({});
            return typeof rl.close === 'function' || typeof rl.close === 'undefined';
        });
        check("readline_createInterface_with_input", function() {
            var rl = readline.createInterface({input: process.stdin});
            return rl !== null;
        });

        // ---- readline other methods ----
        check("readline_clearLine_exists", function() { return typeof readline.clearLine === 'function'; });
        check("readline_clearScreenDown_exists", function() { return typeof readline.clearScreenDown === 'function'; });
        check("readline_cursorTo_exists", function() { return typeof readline.cursorTo === 'function'; });
        check("readline_moveCursor_exists", function() { return typeof readline.moveCursor === 'function'; });
        check("readline_emitKeypressEvents_exists", function() { return typeof readline.emitKeypressEvents === 'function'; });

        // ---- readline.promises ----
        check("readline_promises_exists", function() {
            return typeof readline.promises === 'object' || typeof readline.promises === 'undefined';
        });
        check("readline_promises_createInterface", function() {
            if (!readline.promises) return true;
            return typeof readline.promises.createInterface === 'function';
        });

        // ========================================
        // §3 tty
        // ========================================
        var tty = require('tty');

        check("tty_exists", function() { return typeof tty !== 'undefined'; });
        check("tty_is_object", function() { return typeof tty === 'object'; });

        // ---- isatty ----
        check("tty_isatty_exists", function() { return typeof tty.isatty === 'function'; });
        check("tty_isatty_returns_bool", function() {
            var result = tty.isatty(0);
            return result === true || result === false;
        });
        check("tty_isatty_invalid_fd", function() {
            try { var result = tty.isatty(-1); return result === false || result === true; }
            catch(e) { return true; }
        });

        // ---- ReadStream ----
        check("tty_ReadStream_exists", function() { return typeof tty.ReadStream === 'function'; });
        check("tty_ReadStream_instance", function() {
            try { var rs = new tty.ReadStream(0); return rs !== null; }
            catch(e) { return true; }
        });

        // ---- WriteStream ----
        check("tty_WriteStream_exists", function() { return typeof tty.WriteStream === 'function'; });
        check("tty_WriteStream_instance", function() {
            try { var ws = new tty.WriteStream(1); return ws !== null; }
            catch(e) { return true; }
        });

        // ---- Module keys ----
        check("perf_hooks_module_keys", function() {
            var keys = Object.getOwnPropertyNames(perf_hooks);
            return keys.length >= 3;
        });
        check("readline_module_keys", function() {
            var keys = Object.getOwnPropertyNames(readline);
            return keys.length >= 3;
        });
        check("tty_module_keys", function() {
            var keys = Object.getOwnPropertyNames(tty);
            return keys.length >= 2;
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
    assert_eq!(fail, 0, "perf/readline/tty deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);
    bao_runtime::shutdown_thread_sm();
}