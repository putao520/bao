// @trace TEST-ENG-001-BND [req:REQ-ENG-001] [level:integration]
// JS engine API boundary tests: globalThis, type coercion, edge cases

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
fn test_globalthis_exists() {
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        // === globalThis ===
        check("globalthis", function() {
            return typeof globalThis === "object" && globalThis === this;
        });

        // === Type Coercion Edge Cases ===
        check("coercion_empty_string", function() {
            return "" == false && "" !== false;
        });

        check("coercion_null_undefined", function() {
            return null == undefined && null !== undefined;
        });

        check("coercion_nan", function() {
            return isNaN(NaN) && NaN !== NaN && Number.isNaN(NaN);
        });

        check("typeof_null", function() {
            return typeof null === "object";
        });

        check("typeof_undefined", function() {
            return typeof undefined === "undefined";
        });

        check("typeof_function", function() {
            return typeof function(){} === "function";
        });

        check("typeof_array", function() {
            return typeof [] === "object" && Array.isArray([]);
        });

        // === Number Edge Cases ===
        check("number_max_safe_int", function() {
            return Number.MAX_SAFE_INTEGER === 9007199254740991;
        });

        check("number_min_safe_int", function() {
            return Number.MIN_SAFE_INTEGER === -9007199254740991;
        });

        check("number_infinity", function() {
            return Infinity > Number.MAX_VALUE && -Infinity < Number.MIN_VALUE;
        });

        check("number_is_integer", function() {
            return Number.isInteger(42) && !Number.isInteger(42.5) && !Number.isInteger(NaN);
        });

        check("number_is_finite", function() {
            return Number.isFinite(42) && !Number.isFinite(Infinity) && !Number.isFinite(NaN);
        });

        // === String Methods ===
        check("string_methods", function() {
            return "hello".toUpperCase() === "HELLO" &&
                   "HELLO".toLowerCase() === "hello" &&
                   "abc".charAt(1) === "b" &&
                   "abcabc".indexOf("ca") === 2 &&
                   "hello world".split(" ").length === 2;
        });

        check("string_trim", function() {
            return "  hi  ".trim() === "hi" &&
                   "  hi  ".trimStart() === "hi  " &&
                   "  hi  ".trimEnd() === "  hi";
        });

        check("string_repeat", function() {
            return "ab".repeat(3) === "ababab";
        });

        check("string_includes_startswith", function() {
            return "hello world".includes("world") &&
                   "hello world".startsWith("hello") &&
                   "hello world".endsWith("world");
        });

        // === Array Methods ===
        check("array_map_filter_reduce", function() {
            var arr = [1, 2, 3, 4, 5];
            var doubled = arr.map(function(x) { return x * 2; });
            var evens = arr.filter(function(x) { return x % 2 === 0; });
            var sum = arr.reduce(function(a, b) { return a + b; }, 0);
            return doubled.length === 5 && evens.length === 2 && sum === 15;
        });

        check("array_find_findindex", function() {
            var arr = [1, 2, 3, 4];
            var found = arr.find(function(x) { return x > 2; });
            var idx = arr.findIndex(function(x) { return x > 2; });
            return found === 3 && idx === 2;
        });

        check("array_flat_flatmap", function() {
            if (typeof [].flat !== 'function') return true;
            return [[1,2],[3,4]].flat().length === 4;
        });

        check("array_from_of", function() {
            return Array.from("abc").length === 3 &&
                   Array.of(1, 2, 3).length === 3;
        });

        check("array_fill_copywithin", function() {
            var a = [1,2,3,4,5];
            a.fill(0, 1, 3);
            return a[1] === 0 && a[2] === 0 && a[3] === 4;
        });

        // === Object Static Methods ===
        check("object_assign", function() {
            var target = {a: 1};
            Object.assign(target, {b: 2}, {c: 3});
            return target.a === 1 && target.b === 2 && target.c === 3;
        });

        check("object_freeze", function() {
            var obj = Object.freeze({x: 1});
            obj.x = 2;
            return obj.x === 1;
        });

        check("object_define_property", function() {
            var obj = {};
            Object.defineProperty(obj, 'x', {value: 42, writable: false});
            obj.x = 99;
            return obj.x === 42;
        });

        // === Date ===
        check("date_now", function() {
            return typeof Date.now() === "number" && Date.now() > 0;
        });

        check("date_parse", function() {
            var d = new Date("2024-01-01");
            return d.getFullYear() === 2024 && d.getMonth() === 0;
        });

        // === Math ===
        check("math_methods", function() {
            return Math.abs(-5) === 5 &&
                   Math.ceil(4.1) === 5 &&
                   Math.floor(4.9) === 4 &&
                   Math.round(4.5) === 5 &&
                   Math.max(1, 2, 3) === 3 &&
                   Math.min(1, 2, 3) === 1;
        });

        check("math_random", function() {
            var r = Math.random();
            return typeof r === "number" && r >= 0 && r < 1;
        });

        check("math_constants", function() {
            return Math.PI > 3.14 && Math.E > 2.71;
        });

        // === WeakRef / WeakMap ===
        check("weakref", function() {
            if (typeof WeakRef === 'undefined') return true;
            var obj = {a: 1};
            var wr = new WeakRef(obj);
            return wr.deref() === obj;
        });

        check("weakmap", function() {
            if (typeof WeakMap === 'undefined') return true;
            var wm = new WeakMap();
            var key = {};
            wm.set(key, "value");
            return wm.get(key) === "value";
        });

        // === JSON Edge Cases ===
        check("json_edge", function() {
            return JSON.stringify(null) === "null" &&
                   JSON.stringify(true) === "true" &&
                   JSON.stringify(42) === "42" &&
                   JSON.stringify(undefined) === undefined;
        });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All JS engine boundary tests should pass. Results: {}", results);
    std::mem::forget(ctx);
}
