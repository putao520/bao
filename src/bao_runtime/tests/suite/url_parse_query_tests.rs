// @trace TEST-ENG-007 [req:REQ-ENG-007] [level:integration]
// url.parse `query` field tests — Node legacy url.parse truth:
//   query = search minus the leading '?' (string), null when there is no
//   search; url.parse(url, true) parses it with querystring semantics
//   (decode '+' / %XX, duplicate keys aggregate into arrays).
// Under the old implementation the field was entirely missing (undefined).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<url-query>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Ok(_) => "[other]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

#[test]
fn test_url_parse_query_field() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let out = eval_string(
        &mut ctx,
        r#"
globalThis.__r = {};
var url = require('url');
var results = [];
function check(name, fn) {
  try { results.push(name + ':' + (fn() ? 'PASS' : 'FAIL')); }
  catch (e) { results.push(name + ':ERROR:' + (e.message || e)); }
}

// ── string form (default): search minus '?' ──
check('query_string', function() {
  return url.parse('http://x/p?a=1').query === 'a=1';
});
check('query_string_multi_pair', function() {
  return url.parse('http://x/p?a=1&b=2').query === 'a=1&b=2';
});
check('query_keeps_raw_pair_text', function() {
  return url.parse('http://x/p?a=1&a=2').query === 'a=1&a=2';
});
check('query_null_when_no_search', function() {
  return url.parse('http://x/p').query === null;
});
check('query_relative_form', function() {
  var u = url.parse('/foo/bar?q=1');
  return u.query === 'q=1' && u.pathname === '/foo/bar' && u.search === '?q=1';
});

// ── object form: url.parse(url, true) — querystring.parse semantics ──
check('query_object_basic', function() {
  return url.parse('http://x/p?a=1', true).query.a === '1';
});
check('query_object_multiple_keys', function() {
  var q = url.parse('http://x/p?a=1&b=2', true).query;
  return q.a === '1' && q.b === '2';
});
check('query_object_dup_keys_aggregate', function() {
  var q = url.parse('http://x/p?a=1&a=2&b=3', true).query;
  return Array.isArray(q.a) && q.a.length === 2 && q.a[0] === '1' && q.a[1] === '2' &&
         q.b === '3';
});
check('query_object_bare_flag_empty_string', function() {
  return url.parse('http://x/p?flag&x=1', true).query.flag === '';
});
check('query_object_decodes_percent_and_plus', function() {
  var q = url.parse('http://x/p?x=a%20b&y=c+d', true).query;
  return q.x === 'a b' && q.y === 'c d';
});
check('query_object_decodes_utf8', function() {
  return url.parse('http://x/p?q=%E5%8C%85', true).query.q === '包';
});

// ── no regression on the neighboring fields ──
check('neighbors_intact', function() {
  var u = url.parse('http://x/p?a=1');
  return u.search === '?a=1' && u.pathname === '/p' && u.href.indexOf('/p?a=1') >= 0 &&
         u.protocol === 'http:' && u.host === 'x' &&
         u.path === '/p?a=1';
});
check('query_null_true_form_no_search', function() {
  return url.parse('http://x/p', true).query === null;
});

globalThis.__r.all = results.join('|');
"#,
    );

    for item in out.split('|') {
        assert!(
            item.ends_with(":PASS"),
            "url.parse query check failed: {}",
            item
        );
    }
    let count = out.split('|').count();
    assert_eq!(count, 13, "expected 13 checks, got {}: {}", count, out);
}
