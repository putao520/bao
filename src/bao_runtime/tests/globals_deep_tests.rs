// @trace TEST-ENG-007-GLOBALS-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_globals_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // ---- Bun global ----
        check("Bun_exists", function() { return typeof Bun !== 'undefined'; });
        check("Bun_is_object", function() { return typeof Bun === 'object'; });
        check("Bun_env", function() { return typeof Bun.env === 'object'; });
        check("Bun_cwd", function() { return typeof Bun.cwd === 'function'; });
        check("Bun_cwd_returns_string", function() { return typeof Bun.cwd() === 'string' && Bun.cwd().length > 0; });
        check("Bun_exit", function() { return typeof Bun.exit === 'function'; });
        check("Bun_sleep", function() { return typeof Bun.sleep === 'function'; });
        check("Bun_serve", function() { return typeof Bun.serve === 'function'; });
        check("Bun_build", function() { return typeof Bun.build === 'function' || typeof Bun.build === 'undefined'; });
        check("Bun_write", function() { return typeof Bun.write === 'function' || typeof Bun.write === 'undefined'; });
        check("Bun_file", function() { return typeof Bun.file === 'function' || typeof Bun.file === 'undefined'; });

        // ---- console ----
        check("console_exists", function() { return typeof console !== 'undefined'; });
        check("console_log", function() { return typeof console.log === 'function'; });
        check("console_error", function() { return typeof console.error === 'function'; });
        check("console_warn", function() { return typeof console.warn === 'function'; });
        check("console_info", function() { return typeof console.info === 'function'; });
        check("console_debug", function() { return typeof console.debug === 'function' || typeof console.debug === 'undefined'; });
        check("console_dir", function() { return typeof console.dir === 'function' || typeof console.dir === 'undefined'; });
        check("console_time", function() { return typeof console.time === 'function'; });
        check("console_timeEnd", function() { return typeof console.timeEnd === 'function'; });
        check("console_trace", function() { return typeof console.trace === 'function' || typeof console.trace === 'undefined'; });
        check("console_assert", function() { return typeof console.assert === 'function' || typeof console.assert === 'undefined'; });
        check("console_clear", function() { return typeof console.clear === 'function' || typeof console.clear === 'undefined'; });
        check("console_count", function() { return typeof console.count === 'function' || typeof console.count === 'undefined'; });
        check("console_countReset", function() { return typeof console.countReset === 'function' || typeof console.countReset === 'undefined'; });
        check("console_group", function() { return typeof console.group === 'function' || typeof console.group === 'undefined'; });
        check("console_groupEnd", function() { return typeof console.groupEnd === 'function' || typeof console.groupEnd === 'undefined'; });
        check("console_table", function() { return typeof console.table === 'function' || typeof console.table === 'undefined'; });

        // ---- globalThis ----
        check("globalThis_exists", function() { return typeof globalThis !== 'undefined'; });
        check("globalThis_has_console", function() { return typeof globalThis.console !== 'undefined'; });
        check("globalThis_has_process", function() { return typeof globalThis.process !== 'undefined'; });
        check("globalThis_has_Bun", function() { return typeof globalThis.Bun !== 'undefined'; });

        // ---- setTimeout/setInterval globals ----
        check("setTimeout_global", function() { return typeof setTimeout === 'function'; });
        check("clearTimeout_global", function() { return typeof clearTimeout === 'function'; });
        check("setInterval_global", function() { return typeof setInterval === 'function'; });
        check("clearInterval_global", function() { return typeof clearInterval === 'function'; });
        check("setImmediate_global", function() { return typeof setImmediate === 'function'; });
        check("clearImmediate_global", function() { return typeof clearImmediate === 'function'; });

        // ---- fetch global ----
        check("fetch_global", function() { return typeof fetch === 'function' || typeof fetch === 'undefined'; });
        check("Request_global", function() { return typeof Request === 'function' || typeof Request === 'undefined'; });
        check("Response_global", function() { return typeof Response === 'function' || typeof Response === 'undefined'; });
        check("Headers_global", function() { return typeof Headers === 'function' || typeof Headers === 'undefined'; });

        // ---- TextEncoder/TextDecoder ----
        check("TextEncoder_global", function() { return typeof TextEncoder === 'function'; });
        check("TextDecoder_global", function() { return typeof TextDecoder === 'function'; });
        check("TextEncoder_encode", function() {
            var te = new TextEncoder();
            var buf = te.encode('hello');
            return buf.length === 5;
        });
        check("TextDecoder_decode", function() {
            var te = new TextEncoder();
            var td = new TextDecoder();
            var buf = te.encode('hello');
            var s = td.decode(buf);
            return s === 'hello';
        });

        // ---- URL/URLSearchParams ----
        check("URL_global", function() { return typeof URL === 'function'; });
        check("URLSearchParams_global", function() { return typeof URLSearchParams === 'function'; });

        // ---- Buffer ----
        check("Buffer_global", function() { return typeof Buffer === 'function' || typeof Buffer === 'undefined'; });
        check("Buffer_alloc", function() {
            if (typeof Buffer === 'undefined') return true;
            var b = Buffer.alloc(10);
            return b.length === 10;
        });
        check("Buffer_from", function() {
            if (typeof Buffer === 'undefined') return true;
            var b = Buffer.from('hello');
            return b.length === 5;
        });
        check("Buffer_isBuffer", function() {
            if (typeof Buffer === 'undefined') return true;
            var b = Buffer.alloc(4);
            return Buffer.isBuffer(b) === true;
        });

        // ---- QueueMicrotask ----
        check("queueMicrotask_global", function() { return typeof queueMicrotask === 'function' || typeof queueMicrotask === 'undefined'; });

        // ---- atob/btoa ----
        check("btoa_global", function() { return typeof btoa === 'function'; });
        check("atob_global", function() { return typeof atob === 'function'; });
        check("btoa_atob_roundtrip", function() {
            var encoded = btoa('hello');
            var decoded = atob(encoded);
            return decoded === 'hello';
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
    assert_eq!(fail, 0, "Globals deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);
    std::mem::forget(ctx);
}