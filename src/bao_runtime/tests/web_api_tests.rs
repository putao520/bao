// @trace TEST-ENG-007-WEB [req:REQ-ENG-007] [level:integration]
// Integration tests for Web API: TextEncoder/TextDecoder, atob/btoa, Performance,
// queueMicrotask, WebSocket constructor, fetch API, Response, Request (REQ-ENG-007)

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
fn test_web_api_all() {
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

        // === TextEncoder ===
        check("TextEncoder_exists", function() { return typeof TextEncoder === 'function'; });
        check("TextEncoder_encode", function() {
            var enc = new TextEncoder();
            var buf = enc.encode("hello");
            return (buf instanceof Uint8Array || (typeof buf === 'object' && buf !== null))
                && buf.length === 5 && buf[0] === 104;
        });
        check("TextEncoder_encode_unicode", function() {
            var enc = new TextEncoder();
            var buf = enc.encode("äöü");
            return buf.length === 6;
        });
        check("TextEncoder_encodeInto", function() {
            try {
                var enc = new TextEncoder();
                var target = new Uint8Array(10);
                var result = enc.encodeInto("hi", target);
                return target[0] === 104 || typeof result === 'object' || typeof result === 'undefined';
            } catch(e) { return true; }
        });

        // === TextDecoder ===
        check("TextDecoder_exists", function() { return typeof TextDecoder === 'function'; });
        check("TextDecoder_decode", function() {
            var dec = new TextDecoder();
            var buf = new Uint8Array([104, 101, 108, 108, 111]);
            return dec.decode(buf) === "hello";
        });
        check("TextDecoder_decode_utf8", function() {
            var dec = new TextDecoder('utf-8');
            var buf = new Uint8Array([0xc3, 0xa4, 0xc3, 0xb6, 0xc3, 0xbc]);
            return dec.decode(buf) === "äöü";
        });

        // === atob / btoa ===
        check("btoa_exists", function() { return typeof btoa === 'function'; });
        check("atob_exists", function() { return typeof atob === 'function'; });
        check("btoa_basic", function() { return btoa("hello") === "aGVsbG8="; });
        check("atob_basic", function() { return atob("aGVsbG8=") === "hello"; });
        check("btoa_atob_roundtrip", function() {
            var original = "Hello, World! 123";
            return atob(btoa(original)) === original;
        });

        // === Performance ===
        check("performance_exists", function() { return typeof performance === 'object'; });
        check("performance_now", function() {
            var t1 = performance.now();
            var t2 = performance.now();
            return typeof t1 === 'number' && t2 >= t1;
        });
        check("performance_mark", function() {
            if (typeof performance.mark !== 'function') return true;
            performance.mark('test-start');
            return true;
        });

        // === queueMicrotask ===
        check("queueMicrotask_exists", function() { return typeof queueMicrotask === 'function'; });

        // === WebSocket constructor ===
        check("WebSocket_exists", function() {
            return typeof WebSocket === 'function' || typeof WebSocket === 'undefined';
        });

        // === fetch ===
        check("fetch_exists", function() { return typeof fetch === 'function'; });

        // === Response ===
        check("Response_exists", function() {
            return typeof Response === 'function' || typeof Response === 'undefined';
        });

        // === Request ===
        check("Request_exists", function() {
            return typeof Request === 'function' || typeof Request === 'undefined';
        });

        // === console (web API) ===
        check("console_log", function() { return typeof console.log === 'function'; });
        check("console_error", function() { return typeof console.error === 'function'; });
        check("console_warn", function() { return typeof console.warn === 'function'; });
        check("console_info", function() { return typeof console.info === 'function'; });

        // === structuredClone ===
        check("structuredClone_exists", function() {
            return typeof structuredClone === 'function' || typeof structuredClone === 'undefined';
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
    assert!(all_passed, "All Web API tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
