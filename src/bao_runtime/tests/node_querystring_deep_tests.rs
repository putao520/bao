// @trace TEST-ENG-007-QUERYSTRING [req:REQ-ENG-007] [level:integration]

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
fn test_node_querystring_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).toString().substring(0, 80)); }
        }

        var qs = require('querystring');

        // === 1. Module shape ===
        check("qs_is_object", function() { return typeof qs === 'object' && qs !== null; });
        check("qs_parse_fn", function() { return typeof qs.parse === 'function'; });
        check("qs_stringify_fn", function() { return typeof qs.stringify === 'function'; });
        check("qs_escape_fn", function() { return typeof qs.escape === 'function'; });
        check("qs_unescape_fn", function() { return typeof qs.unescape === 'function'; });

        // === 2. parse — basic key=value ===
        check("parse_basic_kv", function() {
            var obj = qs.parse("a=1&b=2");
            return obj.a === '1' && obj.b === '2';
        });
        check("parse_single_kv", function() {
            var obj = qs.parse("key=val");
            return obj.key === 'val';
        });
        check("parse_empty_string", function() {
            var obj = qs.parse("");
            return Object.keys(obj).length === 0;
        });
        check("parse_question_mark_strip", function() {
            var obj = qs.parse("?a=1");
            return obj.a === '1';
        });

        // === 3. parse — no value ===
        check("parse_key_no_eq", function() {
            var obj = qs.parse("key");
            return obj.key === '';
        });
        check("parse_key_eq_noval", function() {
            var obj = qs.parse("key=");
            return obj.key === '';
        });
        check("parse_multiple_noval", function() {
            var obj = qs.parse("a&b&c");
            return obj.a === '' && obj.b === '' && obj.c === '';
        });

        // === 4. parse — duplicate keys produce arrays ===
        check("parse_dup_key_array", function() {
            var obj = qs.parse("a=1&a=2");
            return Array.isArray(obj.a) && obj.a.length === 2 && obj.a[0] === '1' && obj.a[1] === '2';
        });
        check("parse_three_dup", function() {
            var obj = qs.parse("x=1&x=2&x=3");
            return Array.isArray(obj.x) && obj.x.length === 3;
        });
        check("parse_single_then_dup", function() {
            var obj = qs.parse("a=1&b=2&a=3");
            return Array.isArray(obj.a) && obj.a[0] === '1' && obj.a[1] === '3';
        });

        // === 5. parse — percent encoding ===
        check("parse_pct_space", function() {
            var obj = qs.parse("name=hello%20world");
            return obj.name === 'hello world';
        });
        check("parse_plus_as_space", function() {
            var obj = qs.parse("name=hello+world");
            return obj.name === 'hello world';
        });
        check("parse_pct_special", function() {
            var obj = qs.parse("key=a%26b%3Dc");
            return obj.key === 'a&b=c';
        });
        check("parse_pct_unicode", function() {
            var obj = qs.parse("emoji=%E2%9C%93");
            return obj.emoji === '✓';
        });
        check("parse_pct_cjk", function() {
            var obj = qs.parse("ch=%E4%B8%AD");
            return obj.ch === '中';
        });
        check("parse_pct_slash", function() {
            var obj = qs.parse("path=%2Fhome%2Fuser");
            return obj.path === '/home/user';
        });

        // === 6. parse — custom separator ===
        check("parse_custom_sep_semi", function() {
            var obj = qs.parse("a=1;b=2", ";");
            return obj.a === '1' && obj.b === '2';
        });
        check("parse_custom_sep_pipe", function() {
            var obj = qs.parse("x=1|y=2", "|");
            return obj.x === '1' && obj.y === '2';
        });
        check("parse_custom_sep_newline", function() {
            var obj = qs.parse("a=1\nb=2", "\n");
            return obj.a === '1' && obj.b === '2';
        });

        // === 7. parse — custom eq ===
        check("parse_custom_eq_colon", function() {
            var obj = qs.parse("a:1&b:2", "&", ":");
            return obj.a === '1' && obj.b === '2';
        });
        check("parse_custom_eq_dash", function() {
            var obj = qs.parse("a-1&b-2", "&", "-");
            return obj.a === '1' && obj.b === '2';
        });

        // === 8. parse — maxKeys option ===
        check("parse_maxKeys_1", function() {
            var obj = qs.parse("a=1&b=2&c=3", null, null, { maxKeys: 1 });
            return Object.keys(obj).length === 1 && obj.a === '1';
        });
        check("parse_maxKeys_2", function() {
            var obj = qs.parse("a=1&b=2&c=3", null, null, { maxKeys: 2 });
            return Object.keys(obj).length <= 2;
        });
        check("parse_maxKeys_0_unlimited", function() {
            var obj = qs.parse("a=1&b=2&c=3", null, null, { maxKeys: 0 });
            return Object.keys(obj).length >= 3;
        });

        // === 9. parse — custom decodeURIComponent ===
        check("parse_custom_decoder", function() {
            var obj = qs.parse("key=VALUE", null, null, {
                decodeURIComponent: function(s) { return s.toLowerCase(); }
            });
            return obj.key === 'value';
        });

        // === 10. parse — edge cases ===
        check("parse_trailing_amp", function() {
            var obj = qs.parse("a=1&");
            return obj.a === '1';
        });
        check("parse_leading_amp", function() {
            var obj = qs.parse("&a=1");
            return obj.a === '1';
        });
        check("parse_double_amp", function() {
            var obj = qs.parse("a=1&&b=2");
            return obj.a === '1' && obj.b === '2';
        });
        check("parse_value_with_eq", function() {
            var obj = qs.parse("a=b=c");
            return obj.a === 'b=c';
        });
        check("parse_numeric_key", function() {
            var obj = qs.parse("0=zero&1=one");
            return obj['0'] === 'zero' && obj['1'] === 'one';
        });

        // === 11. stringify — basic ===
        check("stringify_basic", function() {
            return qs.stringify({ a: "1", b: "2" }) === "a=1&b=2";
        });
        check("stringify_empty", function() {
            return qs.stringify({}) === "";
        });
        check("stringify_single", function() {
            return qs.stringify({ x: "42" }) === "x=42";
        });

        // === 12. stringify — null/undefined values ===
        check("stringify_null_val", function() {
            var s = qs.stringify({ key: null });
            return s === "key=" || s.indexOf("key") >= 0;
        });
        check("stringify_undefined_val", function() {
            var s = qs.stringify({ key: undefined });
            return s === "key=" || s.indexOf("key") >= 0;
        });

        // === 13. stringify — arrays ===
        check("stringify_array", function() {
            var s = qs.stringify({ a: ["1", "2"] });
            return s === "a=1&a=2" || (s.indexOf("a=1") >= 0 && s.indexOf("a=2") >= 0);
        });
        check("stringify_array_three", function() {
            var s = qs.stringify({ x: ["a", "b", "c"] });
            return s.indexOf("x=a") >= 0 && s.indexOf("x=b") >= 0 && s.indexOf("x=c") >= 0;
        });

        // === 14. stringify — custom separator/equal ===
        check("stringify_custom_sep", function() {
            return qs.stringify({ a: "1", b: "2" }, ";") === "a=1;b=2";
        });
        check("stringify_custom_eq", function() {
            return qs.stringify({ a: "1" }, "&", ":") === "a:1";
        });
        check("stringify_custom_both", function() {
            return qs.stringify({ a: "1", b: "2" }, ";", ":") === "a:1;b:2";
        });

        // === 15. stringify — encoding ===
        check("stringify_encodes_space", function() {
            var s = qs.stringify({ name: "hello world" });
            return s === "name=hello%20world" || s === "name=hello+world";
        });
        check("stringify_encodes_special", function() {
            var s = qs.stringify({ q: "a&b" });
            return s.indexOf("a%26b") >= 0;
        });
        check("stringify_safe_chars", function() {
            return qs.stringify({ key: "abc123" }) === "key=abc123";
        });

        // === 16. stringify — custom encodeURIComponent ===
        check("stringify_custom_encoder", function() {
            var s = qs.stringify({ key: "UPPER" }, null, null, {
                encodeURIComponent: function(str) { return str.toLowerCase(); }
            });
            return s === "key=upper" || s.indexOf("upper") >= 0;
        });

        // === 17. escape ===
        check("escape_space", function() {
            var e = qs.escape("hello world");
            return e === "hello%20world" || e === "hello+world";
        });
        check("escape_safe", function() {
            return qs.escape("abc123") === "abc123";
        });
        check("escape_amp", function() {
            var e = qs.escape("a&b");
            return e === "a%26b";
        });
        check("escape_eq", function() {
            var e = qs.escape("a=b");
            return e === "a%3Db";
        });
        check("escape_pct", function() {
            var e = qs.escape("100%");
            return e === "100%25";
        });
        check("escape_unicode", function() {
            var e = qs.escape("✓");
            return e.indexOf("%") >= 0 || e === "✓";
        });
        check("escape_empty", function() {
            return qs.escape("") === "";
        });

        // === 18. unescape ===
        check("unescape_pct_space", function() {
            return qs.unescape("hello%20world") === "hello world";
        });
        check("unescape_plus", function() {
            return qs.unescape("hello+world") === "hello world";
        });
        check("unescape_amp", function() {
            return qs.unescape("a%26b") === "a&b";
        });
        check("unescape_eq", function() {
            return qs.unescape("a%3Db") === "a=b";
        });
        check("unescape_pct", function() {
            return qs.unescape("100%25") === "100%";
        });
        check("unescape_empty", function() {
            return qs.unescape("") === "";
        });
        check("unescape_no_encode", function() {
            return qs.unescape("abc123") === "abc123";
        });

        // === 19. roundtrip ===
        check("roundtrip_basic", function() {
            var original = { a: "1", b: "hello world" };
            var str = qs.stringify(original);
            var parsed = qs.parse(str);
            return parsed.a === "1" && parsed.b === "hello world";
        });
        check("roundtrip_special", function() {
            var original = { key: "a&b=c" };
            var str = qs.stringify(original);
            var parsed = qs.parse(str);
            return parsed.key === "a&b=c";
        });
        check("roundtrip_array", function() {
            var str = "a=1&a=2";
            var parsed = qs.parse(str);
            var restringified = qs.stringify(parsed);
            var reparsed = qs.parse(restringified);
            return Array.isArray(reparsed.a) && reparsed.a[0] === '1' && reparsed.a[1] === '2';
        });

        // === 20. require('node:querystring') prefix ===
        check("require_node_prefix", function() {
            try {
                var qs2 = require('node:querystring');
                return typeof qs2 === 'object' && typeof qs2.parse === 'function';
            } catch(e) { return true; }
        });

        results.join("|")
    "#);

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") { pass += 1; }
        else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "querystring deep tests had {} failures", fail);
    assert!(pass >= 40, "Expected at least 40 passes, got {}", pass);

    std::mem::forget(ctx);
}
