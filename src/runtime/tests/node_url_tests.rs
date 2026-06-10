// @trace TEST-ENG-007-URL [req:REQ-ENG-007] [level:integration]
// Integration tests for node:url and URL/URLSearchParams API (REQ-ENG-007)
// All JS assertions in one eval() call.
// Uses r##"..."## because JS strings contain "#" which conflicts with r#"..."#.

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
fn test_node_url_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r##"
        var url = require('url');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        check("require", function() { return typeof url === 'object'; });
        check("parse_basic", function() {
            var u = url.parse("https://example.com/path?q=1#hash");
            return u.protocol === "https:" && u.hostname === "example.com" && u.pathname === "/path";
        });
        check("parse_query", function() {
            var u = url.parse("https://a.b/c?q=hello");
            var hasQuery = (u.query !== undefined && u.query !== null) || (u.search !== undefined && u.search !== null);
            return hasQuery;
        });
        check("parse_hash", function() { return url.parse("https://a.b/c#section").hash === "#section"; });
        check("parse_port", function() { return url.parse("https://a.b:8080/c").port === "8080"; });
        check("format", function() {
            var u = url.parse("https://example.com/path");
            return url.format(u) === "https://example.com/path";
        });
        check("resolve", function() {
            return url.resolve("https://example.com/a/b", "/c") === "https://example.com/c";
        });
        check("resolve_relative", function() {
            return url.resolve("https://example.com/a/b", "c") === "https://example.com/a/c";
        });
        check("URL_constructor", function() {
            var u = new URL("https://example.com/path?q=1");
            return u.protocol === "https:" && u.search === "?q=1";
        });
        check("URL_props", function() {
            var u = new URL("https://user:pass@example.com:8080/p/a/t/h?q=1#frag");
            return u.username === "user" && u.password === "pass" && u.port === "8080" && u.hash === "#frag";
        });
        check("URL_origin", function() {
            return new URL("https://example.com/path").origin === "https://example.com";
        });
        check("URL_modify", function() {
            var u = new URL("https://example.com/");
            u.pathname = "/hello";
            return u.href === "https://example.com/hello";
        });
        check("URLSearchParams", function() {
            var sp = new URLSearchParams("a=1&b=2");
            return sp.get("a") === "1" && sp.get("b") === "2";
        });
        check("URLSearchParams_set", function() {
            var sp = new URLSearchParams();
            sp.set("key", "val");
            return sp.get("key") === "val";
        });
        check("URLSearchParams_has", function() {
            var sp = new URLSearchParams("x=1");
            return sp.has("x") === true && sp.has("y") === false;
        });
        check("URLSearchParams_delete", function() {
            var sp = new URLSearchParams("a=1&b=2");
            sp.delete("a");
            return sp.has("a") === false && sp.has("b") === true;
        });
        check("URLSearchParams_toString", function() {
            var sp = new URLSearchParams();
            sp.set("k", "v");
            return sp.toString() === "k=v";
        });

        results.join("|")
    "##);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All URL tests should pass");
    bun_runtime::shutdown_thread_sm();
}
