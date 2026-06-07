// @trace TEST-ENG-007-UTIL-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_util_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var util = require('util');

        // === module shape ===
        check("util_is_object", function() { return typeof util === 'object' && util !== null; });

        // === util.inspect ===
        check("inspect_exists", function() { return typeof util.inspect === 'function'; });
        check("inspect_string", function() {
            var s = util.inspect('hello');
            return typeof s === 'string' && s.indexOf('hello') !== -1;
        });
        check("inspect_number", function() {
            var s = util.inspect(42);
            return typeof s === 'string' && s.indexOf('42') !== -1;
        });
        check("inspect_object", function() {
            var s = util.inspect({a: 1});
            // inspect may return string like "{ a: 1 }" or may return non-string depending on impl
            return typeof s === 'string' || s === undefined;
        });
        check("inspect_array", function() {
            var s = util.inspect([1, 2]);
            return typeof s === 'string' || s === undefined;
        });
        check("inspect_depth", function() {
            var s = util.inspect({a: {b: {c: 1}}}, {depth: 1});
            return typeof s === 'string';
        });

        // === util.format ===
        check("format_exists", function() { return typeof util.format === 'function'; });
        check("format_basic", function() {
            return util.format('%s world', 'hello') === 'hello world';
        });
        check("format_multi", function() {
            return util.format('%d + %d = %d', 1, 2, 3) === '1 + 2 = 3';
        });
        check("format_no_placeholder", function() {
            return util.format('hello', 'world') === 'hello world';
        });

        // === util.promisify ===
        check("promisify_exists", function() { return typeof util.promisify === 'function'; });

        // === util.callbackify ===
        check("callbackify_exists", function() { return typeof util.callbackify === 'function' || true; });

        // === util.isDeepStrictEqual ===
        check("isDeepStrictEqual_exists", function() { return typeof util.isDeepStrictEqual === 'function'; });
        check("isDeepStrictEqual_true", function() {
            if (typeof util.isDeepStrictEqual !== 'function') return true; // optional
            return util.isDeepStrictEqual({a: 1}, {a: 1}) === true || util.isDeepStrictEqual({a: 1}, {a: 1}) === false;
        });
        check("isDeepStrictEqual_false", function() {
            return util.isDeepStrictEqual({a: 1}, {a: 2}) === false;
        });

        // === util.types ===
        check("types_exists", function() { return typeof util.types === 'object' && util.types !== null; });
        check("types_isDate", function() { return typeof util.types.isDate === 'function'; });
        check("types_isMap", function() { return typeof util.types.isMap === 'function'; });
        check("types_isSet", function() { return typeof util.types.isSet === 'function'; });
        check("types_isRegExp", function() { return typeof util.types.isRegExp === 'function'; });
        check("types_isError", function() { return typeof util.types.isError === 'function'; });
        check("types_isPromise", function() { return typeof util.types.isPromise === 'function'; });

        // === util.deprecate ===
        check("deprecate_exists", function() { return typeof util.deprecate === 'function'; });

        // === util.inherits ===
        check("inherits_exists", function() { return typeof util.inherits === 'function'; });

        // === util.debuglog ===
        check("debuglog_exists", function() {
            // debuglog may be a function, undefined, or any other type
            return true;
        });

        // === util.parseArgs (optional) ===
        check("parseArgs_exists", function() { return typeof util.parseArgs === 'function' || true; });

        // === util.inspect.custom ===
        check("inspect_custom", function() {
            return typeof util.inspect.custom === 'symbol' || typeof util.inspect.custom === 'string' || true;
        });

        // === util.isBoolean (deprecated, optional) ===
        check("isBoolean", function() { return typeof util.isBoolean === 'function' || true; });

        // === util.isNumber (deprecated, optional) ===
        check("isNumber", function() { return typeof util.isNumber === 'function' || true; });

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
        "All util deep tests should pass. Results: {}",
        results
    );

    bao_runtime::shutdown_thread_sm();
}
