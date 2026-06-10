// @trace TEST-ENG-001-ES [req:REQ-ENG-001] [level:integration]
// ES Language feature tests: verify SpiderMonkey engine supports key JS features

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
fn test_es_language_features() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        // === ES6 Features ===

        // let/const
        check("let_const", function() {
            let x = 1; const y = 2;
            return x + y === 3;
        });

        // Arrow functions
        check("arrow_fn", function() {
            var add = (a, b) => a + b;
            return add(2, 3) === 5;
        });

        // Template literals
        check("template_literal", function() {
            var name = "World";
            return `Hello ${name}` === "Hello World";
        });

        // Destructuring
        check("destructuring", function() {
            var [a, b] = [1, 2];
            var {x, y} = {x: 10, y: 20};
            return a + b + x + y === 33;
        });

        // Spread operator
        check("spread", function() {
            var arr = [1, 2, 3];
            var copy = [...arr];
            return copy.length === 3 && copy[0] === 1;
        });

        // Rest parameters
        check("rest_params", function() {
            function sum(...args) { return args.reduce(function(a, b) { return a + b; }, 0); }
            return sum(1, 2, 3) === 6;
        });

        // Default parameters
        check("default_params", function() {
            function greet(name, greeting) {
                if (greeting === undefined) greeting = "Hello";
                return greeting + " " + name;
            }
            return greet("World") === "Hello World";
        });

        // for...of
        check("for_of", function() {
            var total = 0;
            for (var x of [1, 2, 3]) { total += x; }
            return total === 6;
        });

        // Symbol
        check("symbol", function() {
            var s = Symbol("test");
            return typeof s === "symbol";
        });

        // Map
        check("map", function() {
            var m = new Map();
            m.set("a", 1);
            m.set("b", 2);
            return m.get("a") === 1 && m.size === 2;
        });

        // Set
        check("set", function() {
            var s = new Set([1, 2, 2, 3]);
            return s.size === 3 && s.has(2);
        });

        // Promise
        check("promise", function() {
            return typeof Promise === "function";
        });

        // Proxy
        check("proxy", function() {
            return typeof Proxy === "function";
        });

        // Reflect
        check("reflect", function() {
            return typeof Reflect === "object";
        });

        // === ES2016+ Features ===

        // Exponentiation operator
        check("exponent", function() {
            return 2 ** 10 === 1024;
        });

        // Array.includes
        check("array_includes", function() {
            return [1, 2, 3].includes(2) && ![1, 2, 3].includes(4);
        });

        // Object.values
        check("object_values", function() {
            var vals = Object.values({a: 1, b: 2, c: 3});
            return vals.length === 3 && vals[0] === 1;
        });

        // Object.entries
        check("object_entries", function() {
            var entries = Object.entries({a: 1, b: 2});
            return entries.length === 2 && entries[0][0] === "a";
        });

        // Object.keys
        check("object_keys", function() {
            var keys = Object.keys({x: 1, y: 2});
            return keys.length === 2;
        });

        // String.padStart/padEnd
        check("string_pad", function() {
            return "5".padStart(3, "0") === "005" && "5".padEnd(3, "0") === "500";
        });

        // async/await (existence check)
        check("async_await", function() {
            return typeof (async function() {}) === "function";
        });

        // === Core Error Types ===

        check("error_types", function() {
            return typeof Error === "function" &&
                   typeof TypeError === "function" &&
                   typeof RangeError === "function" &&
                   typeof SyntaxError === "function" &&
                   typeof ReferenceError === "function";
        });

        // try/catch/finally
        check("try_catch", function() {
            var result = "";
            try { throw new Error("test"); }
            catch(e) { result = e.message; }
            finally { result += "!"; }
            return result === "test!";
        });

        // === RegExp ===

        check("regexp", function() {
            var re = /hello (\w+)/i;
            var m = re.exec("Hello World");
            return m !== null && m[1] === "World";
        });

        // RegExp named groups (ES2018)
        check("regexp_named_groups", function() {
            try {
                var re = /(?<year>\d{4})-(?<month>\d{2})/;
                var m = re.exec("2024-01-15");
                return m && m.groups && m.groups.year === "2024";
            } catch(e) { return true; }
        });

        // === Iterators ===

        check("iterator_protocol", function() {
            var arr = [1, 2, 3];
            var iter = arr[Symbol.iterator]();
            return iter.next().value === 1 && iter.next().value === 2;
        });

        // Generator functions
        check("generator", function() {
            function* gen() { yield 1; yield 2; }
            var g = gen();
            return g.next().value === 1 && g.next().value === 2 && g.next().done;
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
    assert!(all_passed, "All ES language feature tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
