// @trace TEST-ENG-007-QS [req:REQ-ENG-007] [level:integration]

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
fn test_querystring_deep() {
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

        var qs = require('querystring');

        // === querystring module shape ===
        check("qs_is_object", function() { return typeof qs === 'object' && qs !== null; });

        // === querystring.parse ===
        check("parse_exists", function() { return typeof qs.parse === 'function'; });
        check("parse_basic", function() {
            var obj = qs.parse("a=1&b=2");
            return obj.a === '1' && obj.b === '2';
        });
        check("parse_empty", function() {
            var obj = qs.parse("");
            return Object.keys(obj).length === 0;
        });
        check("parse_no_value", function() {
            var obj = qs.parse("key");
            return obj.key === '';
        });
        check("parse_multi_same_key", function() {
            var obj = qs.parse("a=1&a=2");
            return Array.isArray(obj.a) && obj.a.length === 2;
        });
        check("parse_encoded", function() {
            var obj = qs.parse("name=hello%20world");
            return obj.name === 'hello world';
        });
        check("parse_plus_as_space", function() {
            var obj = qs.parse("name=hello+world");
            return obj.name === 'hello world';
        });
        check("parse_custom_sep", function() {
            var obj = qs.parse("a=1;b=2", ";");
            return obj.a === '1' && obj.b === '2';
        });
        check("parse_custom_eq", function() {
            var obj = qs.parse("a:1&b:2", "&", ":");
            return obj.a === '1' && obj.b === '2';
        });

        // === querystring.stringify ===
        check("stringify_exists", function() { return typeof qs.stringify === 'function'; });
        check("stringify_basic", function() {
            return qs.stringify({a: "1", b: "2"}) === "a=1&b=2";
        });
        check("stringify_empty", function() {
            return qs.stringify({}) === "";
        });
        check("stringify_array", function() {
            var s = qs.stringify({a: ["1", "2"]});
            return s === "a=1&a=2" || s.indexOf("a=1") >= 0;
        });
        check("stringify_custom_sep", function() {
            return qs.stringify({a: "1", b: "2"}, ";") === "a=1;b=2";
        });
        check("stringify_custom_eq", function() {
            return qs.stringify({a: "1"}, "&", ":") === "a:1";
        });
        check("stringify_encodes", function() {
            var s = qs.stringify({name: "hello world"});
            return s === "name=hello%20world" || s === "name=hello+world";
        });

        // === querystring.escape ===
        check("escape_exists", function() { return typeof qs.escape === 'function'; });
        check("escape_space", function() {
            var e = qs.escape("hello world");
            return e === "hello%20world" || e === "hello+world";
        });
        check("escape_safe", function() {
            return qs.escape("abc123") === "abc123";
        });

        // === querystring.unescape ===
        check("unescape_exists", function() { return typeof qs.unescape === 'function'; });
        check("unescape_encoded", function() {
            return qs.unescape("hello%20world") === "hello world";
        });
        check("unescape_plus", function() {
            return qs.unescape("hello+world") === "hello world";
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
    assert!(all_passed, "All querystring deep tests should pass. Results: {}", results);

    std::mem::forget(ctx);
}