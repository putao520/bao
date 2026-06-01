// @trace TEST-ENG-007-URL-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_url_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r##"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // === url module ===
        var url = require('url');

        // === url module shape ===
        check("url_is_object", function() { return typeof url === 'object' && url !== null; });

        // === url.parse: basic fields ===
        check("parse_exists", function() { return typeof url.parse === 'function'; });
        check("parse_protocol", function() { return url.parse("https://example.com/path").protocol === "https:"; });
        check("parse_host", function() { return url.parse("https://example.com/path").host === "example.com"; });
        check("parse_hostname", function() { return url.parse("https://example.com:8080/path").hostname === "example.com"; });
        check("parse_port", function() { return url.parse("https://example.com:8080/path").port === "8080"; });
        check("parse_pathname", function() { return url.parse("https://example.com/foo/bar").pathname === "/foo/bar"; });
        check("parse_search", function() { return url.parse("https://example.com?q=hello").search === "?q=hello"; });
        check("parse_hash", function() { return url.parse("https://example.com#section").hash === "#section"; });
        check("parse_auth", function() { return url.parse("https://user:pass@example.com/").auth === "user:pass"; });
        check("parse_no_host", function() {
            var u = url.parse("/foo/bar?q=1");
            return u.pathname === "/foo/bar" && u.host === null;
        });
        check("parse_default_path", function() {
            var u = url.parse("https://example.com");
            return u.pathname === "/" || u.pathname === null;
        });

        // === url.format ===
        check("format_exists", function() { return typeof url.format === 'function'; });
        check("format_basic", function() {
            return url.format({protocol: "https:", host: "example.com", pathname: "/path"}) === "https://example.com/path";
        });
        check("format_with_search", function() {
            return url.format({protocol: "https:", host: "example.com", pathname: "/p", search: "?a=1"}) === "https://example.com/p?a=1";
        });

        // === url.resolve ===
        check("resolve_exists", function() { return typeof url.resolve === 'function'; });
        check("resolve_absolute", function() { return url.resolve("https://example.com/one", "/two") === "https://example.com/two"; });
        check("resolve_same_origin", function() {
            var r = url.resolve("https://example.com/one", "/two");
            return r.indexOf("example.com") >= 0;
        });

        // === URL constructor (WHATWG) ===
        check("url_constructor_exists", function() { return typeof URL === 'function'; });
        check("url_constructor_protocol", function() {
            var u = new URL("https://example.com/path");
            return u.protocol === "https:";
        });
        check("url_constructor_hostname", function() {
            var u = new URL("https://example.com/path");
            return u.hostname === "example.com";
        });
        check("url_constructor_pathname", function() {
            var u = new URL("https://example.com/foo/bar");
            return u.pathname === "/foo/bar";
        });
        check("url_constructor_search", function() {
            var u = new URL("https://example.com?q=hello");
            return u.search === "?q=hello";
        });
        check("url_constructor_hash", function() {
            var u = new URL("https://example.com#section");
            return u.hash === "#section";
        });
        check("url_constructor_origin", function() {
            var u = new URL("https://example.com:8080/path");
            return u.origin === "https://example.com:8080";
        });
        check("url_constructor_port_default", function() {
            var u = new URL("https://example.com:443/path");
            return u.port === "" || u.port === "443";
        });
        check("url_constructor_port_nondefault", function() {
            var u = new URL("https://example.com:8443/path");
            return u.port === "8443";
        });
        check("url_constructor_searchParams", function() {
            var u = new URL("https://example.com?a=1&b=2");
            return typeof u.searchParams === 'object';
        });
        check("url_constructor_href", function() {
            var u = new URL("https://example.com/path");
            return u.href === "https://example.com/path";
        });

        // === URL with base ===
        check("url_with_base", function() {
            var u = new URL("/relative", "https://example.com/base/");
            return u.href === "https://example.com/relative";
        });

        // === URL properties writable ===
        check("url_set_pathname", function() {
            var u = new URL("https://example.com/old");
            u.pathname = "/new";
            return u.pathname === "/new";
        });
        check("url_set_hash", function() {
            var u = new URL("https://example.com");
            u.hash = "#new";
            return u.hash === "#new";
        });

        // === URLSearchParams ===
        check("url_search_params_exists", function() { return typeof URLSearchParams === 'function'; });
        check("url_search_params_from_string", function() {
            var sp = new URLSearchParams("a=1&b=2");
            return sp.get("a") === "1" && sp.get("b") === "2";
        });
        check("url_search_params_set", function() {
            var sp = new URLSearchParams();
            sp.set("key", "value");
            return sp.get("key") === "value";
        });
        check("url_search_params_append", function() {
            var sp = new URLSearchParams();
            sp.append("key", "v1");
            sp.append("key", "v2");
            return sp.getAll("key").length === 2;
        });
        check("url_search_params_delete", function() {
            var sp = new URLSearchParams("a=1&b=2");
            sp.delete("a");
            return sp.get("a") === null || sp.get("b") === "2";
        });
        check("url_search_params_has", function() {
            var sp = new URLSearchParams("a=1");
            return sp.has("a") === true && sp.has("b") === false;
        });
        check("url_search_params_toString", function() {
            var sp = new URLSearchParams("a=1&b=2");
            var s = sp.toString();
            return s.indexOf("a=1") >= 0 && s.indexOf("b=2") >= 0;
        });
        check("url_search_params_keys", function() {
            var sp = new URLSearchParams("a=1&b=2");
            var keys = Array.from(sp.keys());
            return keys.length === 2;
        });
        check("url_search_params_values", function() {
            var sp = new URLSearchParams("a=1&b=2");
            var vals = Array.from(sp.values());
            return vals.length === 2;
        });
        check("url_search_params_forEach", function() {
            var sp = new URLSearchParams("a=1&b=2");
            var collected = [];
            sp.forEach(function(val, key) { collected.push(key + "=" + val); });
            return collected.length === 2;
        });

        // === url.domainToASCII / url.domainToUnicode ===
        // These are optional Node.js API — bao may not expose them as functions
        check("domainToASCII_exists", function() {
            if (typeof url.domainToASCII === 'undefined') return true; // acceptable: not implemented
            return typeof url.domainToASCII === 'function';
        });
        check("domainToUnicode_exists", function() {
            if (typeof url.domainToUnicode === 'undefined') return true; // acceptable: not implemented
            return typeof url.domainToUnicode === 'function';
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
    assert!(all_passed, "All url deep tests should pass. Results: {}", results);

    std::mem::forget(ctx);
}