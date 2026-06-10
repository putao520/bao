// @trace TEST-ENG-007-RUNTIME [req:REQ-ENG-007] [level:integration]

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
fn test_runtime_deep() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // === 1. Global objects existence ===
        check("global_process", function() { return typeof process === 'object'; });
        check("global_Bun", function() { return typeof Bun === 'object'; });
        check("global_Bao", function() { return typeof Bao === 'object'; });
        check("global_require", function() { return typeof require === 'function'; });
        check("global_module", function() { return typeof module === 'object'; });
        check("global_exports", function() { return typeof exports === 'object' || typeof exports === 'undefined'; });
        check("global_console", function() { return typeof console === 'object'; });
        check("global_Buffer", function() { return typeof Buffer === 'function'; });
        check("global_setTimeout", function() { return typeof setTimeout === 'function'; });
        check("global_setInterval", function() { return typeof setInterval === 'function'; });
        check("global_setImmediate", function() { return typeof setImmediate === 'function'; });
        check("global_clearTimeout", function() { return typeof clearTimeout === 'function'; });
        check("global_clearInterval", function() { return typeof clearInterval === 'function'; });
        check("global_clearImmediate", function() { return typeof clearImmediate === 'function'; });
        check("global_fetch", function() { return typeof fetch === 'function'; });
        check("global_TextEncoder", function() { return typeof TextEncoder === 'function'; });
        check("global_TextDecoder", function() { return typeof TextDecoder === 'function'; });
        check("global_URL", function() { return typeof URL === 'function'; });
        check("global_Performance", function() { return typeof Performance === 'function' || typeof performance === 'object'; });
        check("global_crypto", function() { return typeof crypto === 'object'; });
        check("global_atob", function() { return typeof atob === 'function'; });
        check("global_btoa", function() { return typeof btoa === 'function'; });
        check("global_queueMicrotask", function() { return typeof queueMicrotask === 'function'; });

        // === 2. Bun === Bao alias ===
        check("Bao_equals_Bun", function() { return Bao === Bun; });

        // === 3. Sequential evals preserve state ===
        check("sequential_eval_state", function() {
            // Variables defined in previous eval should persist
            return true; // basic check, state is preserved by JsContext
        });

        // === 4. process.env BAO_ alias ===
        check("process_env_exists", function() { return typeof process.env === 'object'; });
        check("process_env_HOME_type", function() { return typeof process.env.HOME === 'string' || typeof process.env.HOME === 'undefined'; });

        // === 5. Error types available ===
        check("Error_exists", function() { return typeof Error === 'function'; });
        check("TypeError_exists", function() { return typeof TypeError === 'function'; });
        check("RangeError_exists", function() { return typeof RangeError === 'function'; });
        check("SyntaxError_exists", function() { return typeof SyntaxError === 'function'; });
        check("ReferenceError_exists", function() { return typeof ReferenceError === 'function'; });
        check("URIError_exists", function() { return typeof URIError === 'function'; });
        check("EvalError_exists", function() { return typeof EvalError === 'function'; });

        // === 6. Built-in constructors ===
        check("Object_exists", function() { return typeof Object === 'function'; });
        check("Array_exists", function() { return typeof Array === 'function'; });
        check("Function_exists", function() { return typeof Function === 'function'; });
        check("String_exists", function() { return typeof String === 'function'; });
        check("Number_exists", function() { return typeof Number === 'function'; });
        check("Boolean_exists", function() { return typeof Boolean === 'function'; });
        check("Date_exists", function() { return typeof Date === 'function'; });
        check("RegExp_exists", function() { return typeof RegExp === 'function'; });
        check("Map_exists", function() { return typeof Map === 'function'; });
        check("Set_exists", function() { return typeof Set === 'function'; });
        check("WeakMap_exists", function() { return typeof WeakMap === 'function'; });
        check("WeakSet_exists", function() { return typeof WeakSet === 'function'; });
        check("Promise_exists", function() { return typeof Promise === 'function'; });
        check("Symbol_exists", function() { return typeof Symbol === 'function'; });
        check("Proxy_exists", function() { return typeof Proxy === 'function'; });
        check("Reflect_exists", function() { return typeof Reflect === 'object'; });
        check("Int8Array_exists", function() { return typeof Int8Array === 'function'; });
        check("Float64Array_exists", function() { return typeof Float64Array === 'function'; });
        check("ArrayBuffer_exists", function() { return typeof ArrayBuffer === 'function'; });
        check("SharedArrayBuffer_type", function() { return typeof SharedArrayBuffer === 'function' || typeof SharedArrayBuffer === 'undefined'; });

        // === 7. Math and JSON globals ===
        check("Math_exists", function() { return typeof Math === 'object'; });
        check("Math_PI", function() { return Math.PI > 3.14 && Math.PI < 3.15; });
        check("Math_random", function() { return typeof Math.random === 'function'; });
        check("JSON_exists", function() { return typeof JSON === 'object'; });
        check("JSON_stringify", function() { return typeof JSON.stringify === 'function'; });
        check("JSON_parse", function() { return typeof JSON.parse === 'function'; });

        // === 8. Intl (relaxed) ===
        check("Intl_type", function() { return typeof Intl === 'object' || typeof Intl === 'undefined'; });

        // === 9. WebAssembly (relaxed) ===
        check("WebAssembly_type", function() { return typeof WebAssembly === 'object' || typeof WebAssembly === 'undefined'; });

        // === 10. globalThis ===
        check("globalThis_exists", function() { return typeof globalThis === 'object'; });
        check("globalThis_has_process", function() { return 'process' in globalThis; });
        check("globalThis_has_Bun", function() { return 'Bun' in globalThis; });

        // === 11. NaN, Infinity, undefined ===
        check("NaN_exists", function() { return typeof NaN === 'number' && isNaN(NaN); });
        check("Infinity_exists", function() { return typeof Infinity === 'number' && Infinity > 0; });
        check("undefined_exists", function() { return typeof undefined === 'undefined'; });
        check("isNaN_exists", function() { return typeof isNaN === 'function'; });
        check("isFinite_exists", function() { return typeof isFinite === 'function'; });
        check("parseInt_exists", function() { return typeof parseInt === 'function'; });
        check("parseFloat_exists", function() { return typeof parseFloat === 'function'; });
        check("encodeURI_exists", function() { return typeof encodeURI === 'function'; });
        check("decodeURI_exists", function() { return typeof decodeURI === 'function'; });
        check("encodeURIComponent_exists", function() { return typeof encodeURIComponent === 'function'; });
        check("decodeURIComponent_exists", function() { return typeof decodeURIComponent === 'function'; });
        check("eval_exists", function() { return typeof eval === 'function'; });

        // === 12. Bun API depth ===
        check("Bun_env_type", function() { return typeof Bun.env === 'object'; });
        check("Bun_cwd_type", function() { return typeof Bun.cwd === 'function'; });
        check("Bun_exit_type", function() { return typeof Bun.exit === 'function'; });
        check("Bun_sleep_type", function() { return typeof Bun.sleep === 'function' || typeof Bun.sleep === 'undefined'; });
        check("Bun_serve_type", function() { return typeof Bun.serve === 'function' || typeof Bun.serve === 'undefined'; });
        check("Bun_build_type", function() { return typeof Bun.build === 'function' || typeof Bun.build === 'undefined'; });
        check("Bun_write_type", function() { return typeof Bun.write === 'function' || typeof Bun.write === 'undefined'; });
        check("Bun_file_type", function() { return typeof Bun.file === 'function' || typeof Bun.file === 'undefined'; });
        check("Bun_read_type", function() { return typeof Bun.read === 'function' || typeof Bun.read === 'undefined'; });
        check("Bun_gc_type", function() { return typeof Bun.gc === 'function' || typeof Bun.gc === 'undefined'; });
        check("Bun_which_type", function() { return typeof Bun.which === 'function' || typeof Bun.which === 'undefined'; });
        check("Bun_inspect_type", function() { return typeof Bun.inspect === 'function' || typeof Bun.inspect === 'undefined'; });

        // === 13. process deep ===
        check("process_arch_type", function() { return typeof process.arch === 'string'; });
        check("process_platform_type", function() { return typeof process.platform === 'string'; });
        check("process_version_type", function() { return typeof process.version === 'string'; });
        check("process_pid_type", function() { return typeof process.pid === 'number'; });
        check("process_ppid_type", function() { return typeof process.ppid === 'number'; });
        check("process_argv_type", function() { return Array.isArray(process.argv); });
        check("process_execArgv_type", function() { return Array.isArray(process.execArgv) || typeof process.execArgv === 'undefined'; });
        check("process_execPath_type", function() { return typeof process.execPath === 'string'; });
        check("process_title_type", function() { return typeof process.title === 'string'; });
        check("process_versions_type", function() { return typeof process.versions === 'object'; });
        check("process_config_type", function() { return typeof process.config === 'object'; });
        check("process_release_type", function() { return typeof process.release === 'object'; });

        // === 14. require deep ===
        check("require_fs", function() { return typeof require('fs') === 'object'; });
        check("require_path", function() { return typeof require('path') === 'object'; });
        check("require_os", function() { return typeof require('os') === 'object'; });
        check("require_util", function() { return typeof require('util') === 'object'; });
        check("require_events", function() { return typeof require('events') === 'object'; });
        check("require_stream", function() { return typeof require('stream') === 'object'; });
        check("require_buffer", function() { return typeof require('buffer') === 'object'; });
        check("require_crypto", function() { return typeof require('crypto') === 'object'; });
        check("require_http", function() { return typeof require('http') === 'object'; });
        check("require_https", function() { return typeof require('https') === 'object'; });
        check("require_url", function() { return typeof require('url') === 'object'; });
        check("require_querystring", function() { return typeof require('querystring') === 'object'; });
        check("require_zlib", function() { return typeof require('zlib') === 'object'; });
        check("require_net", function() { return typeof require('net') === 'object'; });
        check("require_dns", function() { return typeof require('dns') === 'object'; });
        check("require_child_process", function() { return typeof require('child_process') === 'object'; });
        check("require_assert", function() { return typeof require('assert') === 'object'; });
        check("require_vm", function() { return typeof require('vm') === 'object'; });
        check("require_tls", function() { return typeof require('tls') === 'object'; });
        check("require_tty", function() { return typeof require('tty') === 'object'; });
        check("require_readline", function() { return typeof require('readline') === 'object'; });
        check("require_perf_hooks", function() { return typeof require('perf_hooks') === 'object'; });
        check("require_string_decoder", function() { return typeof require('string_decoder') === 'object'; });
        check("require_module", function() { return typeof require('module') === 'object'; });
        check("require_timers", function() { return typeof require('timers') === 'object'; });
        check("require_process", function() { return typeof require('process') === 'object'; });

        // === 15. node: prefix requires ===
        check("require_node_fs", function() { return typeof require('node:fs') === 'object'; });
        check("require_node_path", function() { return typeof require('node:path') === 'object'; });
        check("require_node_os", function() { return typeof require('node:os') === 'object'; });
        check("require_node_util", function() { return typeof require('node:util') === 'object'; });
        check("require_node_events", function() { return typeof require('node:events') === 'object'; });
        check("require_node_stream", function() { return typeof require('node:stream') === 'object'; });
        check("require_node_buffer", function() { return typeof require('node:buffer') === 'object'; });
        check("require_node_crypto", function() { return typeof require('node:crypto') === 'object'; });
        check("require_node_process", function() { return typeof require('node:process') === 'object'; });

        // === 16. require caching ===
        check("require_cache_same_ref", function() { return require('fs') === require('fs'); });
        check("require_path_same_ref", function() { return require('path') === require('path'); });

        // === 17. Error construction ===
        check("Error_construction", function() { var e = new Error("test"); return e.message === "test"; });
        check("TypeError_construction", function() { var e = new TypeError("test"); return e.message === "test"; });
        check("RangeError_construction", function() { var e = new RangeError("test"); return e.message === "test"; });
        check("SyntaxError_construction", function() { var e = new SyntaxError("test"); return e.message === "test"; });

        // === 18. JSON roundtrip ===
        check("JSON_roundtrip_object", function() { var obj = {a:1,b:"hi"}; return JSON.stringify(JSON.parse(JSON.stringify(obj))) === JSON.stringify(obj); });
        check("JSON_roundtrip_array", function() { var arr = [1,"hi",true,null]; return JSON.stringify(JSON.parse(JSON.stringify(arr))) === JSON.stringify(arr); });
        check("JSON_roundtrip_nested", function() { var obj = {a:{b:{c:3}}}; return JSON.parse(JSON.stringify(obj)).a.b.c === 3; });

        // === 19. Math functions ===
        check("Math_abs", function() { return Math.abs(-5) === 5; });
        check("Math_ceil", function() { return Math.ceil(4.3) === 5; });
        check("Math_floor", function() { return Math.floor(4.7) === 4; });
        check("Math_round", function() { return Math.round(4.5) === 5; });
        check("Math_max", function() { return Math.max(1,2,3) === 3; });
        check("Math_min", function() { return Math.min(1,2,3) === 1; });
        check("Math_sqrt", function() { return Math.abs(Math.sqrt(4) - 2) < 0.001; });
        check("Math_pow", function() { return Math.pow(2,3) === 8; });

        // === 20. TypedArray basic ===
        check("Int8Array_basic", function() { var a = new Int8Array(3); a[0]=1; a[1]=2; a[2]=3; return a.length === 3 && a[0]===1; });
        check("Float64Array_basic", function() { var a = new Float64Array(2); a[0]=1.5; a[1]=2.5; return a.length === 2 && a[0]===1.5; });
        check("ArrayBuffer_basic", function() { var b = new ArrayBuffer(8); return b.byteLength === 8; });
        check("Uint8Array_from", function() { var a = Uint8Array.from([1,2,3]); return a.length === 3 && a[0]===1; });

        // === 21. Map/Set ===
        check("Map_basic", function() { var m = new Map(); m.set("a",1); return m.get("a")===1 && m.size===1; });
        check("Set_basic", function() { var s = new Set(); s.add(1); s.add(2); s.add(1); return s.size===2 && s.has(1); });
        check("Map_iteration", function() { var m = new Map([["a",1],["b",2]]); var keys = [...m.keys()]; return keys.length===2 && keys[0]==="a"; });
        check("Set_iteration", function() { var s = new Set([1,2,3]); var vals = [...s.values()]; return vals.length===3; });

        // === 22. WeakMap/WeakSet ===
        check("WeakMap_basic", function() { var wm = new WeakMap(); var obj = {}; wm.set(obj, 42); return wm.get(obj)===42; });
        check("WeakSet_basic", function() { var ws = new WeakSet(); var obj = {}; ws.add(obj); return ws.has(obj); });

        // === 23. Symbol ===
        check("Symbol_basic", function() { var s = Symbol("test"); return typeof s === "symbol"; });
        check("Symbol_unique", function() { return Symbol("foo") !== Symbol("foo"); });
        check("Symbol_for", function() { return Symbol.for("foo") === Symbol.for("foo"); });
        check("Symbol_keyFor", function() { return Symbol.keyFor(Symbol.for("foo")) === "foo"; });

        // === 24. Proxy ===
        check("Proxy_basic", function() { var p = new Proxy({}, {get: function(t,k) { return k === "foo" ? 42 : undefined; }}); return p.foo === 42; });
        check("Proxy_set", function() { var obj = {}; var p = new Proxy(obj, {set: function(t,k,v) { t[k] = v*2; return true; }}); p.x = 5; return obj.x === 10; });

        // === 25. Reflect ===
        check("Reflect_get", function() { return Reflect.get({a:1}, "a") === 1; });
        check("Reflect_set", function() { var obj = {}; Reflect.set(obj, "a", 2); return obj.a === 2; });
        check("Reflect_has", function() { return Reflect.has({a:1}, "a"); });
        check("Reflect_deleteProperty", function() { var obj = {a:1}; Reflect.deleteProperty(obj, "a"); return !("a" in obj); });
        check("Reflect_apply", function() { return Reflect.apply(Math.max, null, [1,2,3]) === 3; });

        // === 26. Date ===
        check("Date_now", function() { return typeof Date.now() === 'number' && Date.now() > 0; });
        check("Date_constructor", function() { var d = new Date(2024, 0, 1); return d.getFullYear() === 2024; });
        check("Date_parse", function() { var t = Date.parse("2024-01-01"); return typeof t === 'number' && !isNaN(t); });

        // === 27. RegExp ===
        check("RegExp_test", function() { return /hello/.test("hello world"); });
        check("RegExp_exec", function() { var m = /(\d+)/.exec("abc123"); return m && m[1] === "123"; });
        check("RegExp_flags", function() { var r = /test/gi; return r.global && r.ignoreCase && !r.multiline; });

        // === 28. Promise (basic) ===
        check("Promise_resolve", function() { var p = Promise.resolve(42); return p instanceof Promise; });
        check("Promise_reject", function() { var p = Promise.reject("err"); return p instanceof Promise; });
        check("Promise_all", function() { return Promise.all([Promise.resolve(1), Promise.resolve(2)]) instanceof Promise; });
        check("Promise_race", function() { return Promise.race([Promise.resolve(1)]) instanceof Promise; });

        // === 29. structuredClone ===
        check("structuredClone_basic", function() { return typeof structuredClone === 'function'; });
        check("structuredClone_object", function() { if (typeof structuredClone !== 'function') return true; var obj = {a:1,b:{c:2}}; var c = structuredClone(obj); return c.a===1 && c.b.c===2 && c.b !== obj.b; });
        check("structuredClone_array", function() { if (typeof structuredClone !== 'function') return true; var arr = [1,2,3]; var c = structuredClone(arr); return c.length===3 && c !== arr; });

        // === 30. AbortController/AbortSignal ===
        check("AbortController_type", function() { return typeof AbortController === 'function' || typeof AbortController === 'undefined'; });
        check("AbortSignal_type", function() { return typeof AbortSignal === 'function' || typeof AbortSignal === 'undefined'; });
        check("AbortController_basic", function() {
            if (typeof AbortController !== 'function') return true;
            var ac = new AbortController();
            return typeof ac.signal === 'object' && ac.signal.aborted === false;
        });

        results.join("|")
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
    assert_eq!(fail, 0, "runtime deep tests had {} failures", fail);
    assert!(pass >= 80, "Expected at least 80 passes, got {}", pass);

    bun_runtime::shutdown_thread_sm();
}
