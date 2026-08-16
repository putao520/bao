// @trace TEST-ENG-007-URL-FORMAT-QUERY [req:REQ-ENG-007] [level:integration]
// url.format({query: object}) — query-object serialization into search,
// with every expectation pinned to Node 24 ground truth (`node -e` output
// captured 2026-08-17). Inputs pin `protocol`/`host` explicitly: the two
// KNOWN pre-existing deviations outside this file's mandate (a missing
// `protocol` gains a defaulted "http:" instead of no scheme, and a numeric
// `port` is dropped instead of String()-coerced) are reported separately
// and deliberately not asserted here.
//
//   * object/array/nested-shallow shapes via querystring.stringify
//     semantics (%20 spaces, repeated keys for arrays, bare `key=` for
//     null/undefined/nested-object/function values);
//   * precedence: non-empty string `search` wins over `query`; empty-string
//     search falls through to the query object; a *string* query is
//     ignored (Node only serializes object queries);
//   * bare search gains its leading '?';
//   * regressions: href short-circuit, string passthrough, parse→format
//     round-trip.

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
fn test_url_format_query_object() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r##"
        var url = require('url');
        // Pinned scheme/host so the assertions isolate the QUERY behavior.
        var B = {protocol:'http:', host:'h.co'};
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }
        function fmt(extra) {
            var init = Object.assign({}, B, extra);
            return url.format(init);
        }

        // Object/array/nested-shallow value shapes (Node: qs.stringify via
        // url.format).
        check("qs_shapes", function() {
            return fmt({pathname:'/p', query:{a:'x y', b:[1,2], c:{d:1}, e:null, f:true, g:'a&b=c'}})
                === 'http://h.co/p?a=x%20y&b=1&b=2&c=&e=&f=true&g=a%26b%3Dc';
        });
        // search wins over query (Node precedence).
        check("search_wins", function() {
            return fmt({pathname:'/p', search:'?a=1', query:{b:2}}) === 'http://h.co/p?a=1';
        });
        // A string query is ignored (Node only serializes object queries).
        check("string_query_ignored", function() {
            return fmt({pathname:'/p', query:'a=1&b=2'}) === 'http://h.co/p';
        });
        // A bare search gains its leading '?'.
        check("bare_search_prefix", function() {
            return fmt({pathname:'/p', search:'a=1'}) === 'http://h.co/p?a=1';
        });
        // Empty-string search is falsy in Node — the query takes over.
        check("empty_search_falls_to_query", function() {
            return fmt({pathname:'/p', search:'', query:{b:2}}) === 'http://h.co/p?b=2';
        });
        // Empty query object → no '?' at all.
        check("empty_query_object", function() {
            return fmt({pathname:'/p', query:{}}) === 'http://h.co/p';
        });
        // undefined value emits the bare key.
        check("undefined_value", function() {
            return fmt({pathname:'/p', query:{a:undefined}}) === 'http://h.co/p?a=';
        });
        // Function values emit the bare key (Node type dispatch).
        check("fn_value_bare", function() {
            return fmt({pathname:'/p', query:{a:function(){}}}) === 'http://h.co/p?a=';
        });
        // bigint serializes via String(value).
        check("bigint_value", function() {
            return fmt({pathname:'/p', query:{a:1n}}) === 'http://h.co/p?a=1';
        });
        // Array as the query itself: index keys.
        check("array_query", function() {
            return fmt({pathname:'/p', query:[1,2]}) === 'http://h.co/p?0=1&1=2';
        });
        // %20 (not '+') with the encodeURIComponent unescaped set
        // (-_.!~*'()).
        check("encoding_set", function() {
            return fmt({pathname:'/p', query:{'k k':'v v'}}) === 'http://h.co/p?k%20k=v%20v'
                && fmt({pathname:'/p', query:{"k~!()*'": 'v$'}}) === "http://h.co/p?k~!()*'=v%24";
        });
        // Full shape: protocol (colon-suffixed — colon normalization for a
        // bare "https" is a reported pre-existing deviation, not asserted)
        // + hostname + string port (numeric-port coercion likewise
        // reported) + pathname + query + hash.
        check("full_shape", function() {
            return url.format({protocol:'https:', hostname:'h.co', port:'8080',
                               pathname:'/x', query:{k:1}, hash:'#f'})
                === 'https://h.co:8080/x?k=1#f';
        });
        // Regressions: href short-circuit, string passthrough, parse
        // round-trip.
        check("href_shortcircuit", function() {
            return url.format({href:'http://a.b/c'}) === 'http://a.b/c';
        });
        check("string_passthrough", function() {
            return url.format('http://a.b/c') === 'http://a.b/c';
        });
        check("parse_roundtrip", function() {
            return url.format(url.parse('https://e.com/p?k=1')) === 'https://e.com/p?k=1';
        });
        results.join("\n")
        "##,
    );

    let fails: Vec<&str> = results.lines().filter(|l| !l.ends_with(" PASS")).collect();
    assert!(
        fails.is_empty(),
        "url.format query expectations failed:\n{}",
        results
    );
    eprintln!(
        "[PASS] TEST-ENG-007-URL-FORMAT-QUERY: {} checks vs Node 24 ground truth",
        results.lines().count()
    );
    bun_runtime::shutdown_thread_sm();
}
