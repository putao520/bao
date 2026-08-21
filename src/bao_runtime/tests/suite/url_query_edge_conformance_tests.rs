// @trace TEST-ENG-007-URL-QUERY-EDGE [req:REQ-ENG-007] [level:integration]
// node:url / URLSearchParams query-string boundary conformance (issue #1).
//
// Oracle: Node v24.5.0 (`node /tmp/urlq/battery.js` ground truth captured
// 2026-08-21, 79 cases). The identical battery ran under bao and every
// expectation below is the byte-exact Node output for that case.
//
// Deviation history — five classes were found in the first measurement
// (73/79) and eradicated in the follow-up pass (all now asserted green,
// battery diff vs Node is zero):
//
//   DEV-1  legacy url.parse(url, true) mangled multibyte query KEYS
//          (Latin-1 atomization of C-string property names) → fixed by
//          UTF-16 key definition (JS_DefineUCProperty2) in
//          build_query_object. Asserted below as L04.
//   DEV-2  hash-only reference dropped the base query → fixed in
//          parse_url's fragment-only branch (base minus fragment + input).
//          Asserted as R02.
//   DEV-3  empty-string reference threw instead of resolving to the base →
//          fixed in parse_url's empty-input branch. Asserted as R06.
//   DEV-4  URLSearchParams arguments were not string-coerced (get(1) missed
//          the '1' pair; non-string args silently dropped) → fixed via
//          ES-ToString conversion (sp_arg_to_string) plus Node's
//          ERR_MISSING_ARGS TypeError for missing required arguments.
//          Asserted as P39/P39b.
//   GAP-1  URLSearchParams.prototype.sort was missing → implemented as a
//          stable name-only sort in UTF-16 code-unit order (Node truth:
//          astral keys sort before U+FFFF; values keep insertion order).
//          Asserted as P44/P44b/P44c.

#[path = "conformance_common.rs"]
mod common;

use common::{eval_string, make_ctx};

/// run_checks (PASS/ERROR assertion, BCE-20260817 fake-green guard) plus an
/// exact check-count assertion so a silently truncated `results` bundle can
/// never pass as "no failures".
fn run_counted_checks(ctx: &mut bao_engine::context::JsContext, source: &str, expected: usize) {
    let results = eval_string(ctx, source);
    assert!(
        results.contains(":PASS") || results.contains(":FAIL") || results.contains(":ERROR:"),
        "suite produced no check output — top-level JS error? raw: {:?}",
        results
    );
    let items: Vec<&str> = results.split('|').filter(|s| !s.is_empty()).collect();
    let failures: Vec<&str> = items.iter().filter(|s| !s.contains(":PASS")).copied().collect();
    assert_eq!(
        items.len(),
        expected,
        "expected {} checks, got {}: {}",
        expected,
        items.len(),
        results
    );
    assert!(
        failures.is_empty(),
        "conformance failures:\n  {}\nFull results: {}",
        failures.join("\n  "),
        results
    );
}

#[test]
fn test_urlsearchparams_parse_edges() {
    let mut ctx = make_ctx();
    run_counted_checks(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ':' + (ok ? 'PASS' : 'FAIL')); }
            catch (e) { results.push(label + ':ERROR:' + (e && e.message ? e.message : e)); }
        }
        var S = function (p) { return p.toString(); };

        // ── init-string parse boundaries ──
        check('P01_empty_init', function() { var p = new URLSearchParams(''); return S(p) === '' && p.size === 0; });
        check('P02_qmark_only', function() { var p = new URLSearchParams('?'); return S(p) === '' && p.size === 0; });
        check('P03_empty_value', function() { return new URLSearchParams('a=').get('a') === ''; });
        check('P04_empty_key', function() { var v = new URLSearchParams('=v').getAll(''); return v.length === 1 && v[0] === 'v'; });
        check('P05_bare_eq', function() { var p = new URLSearchParams('='); return p.get('') === '' && p.size === 1; });
        check('P06_two_pairs', function() { var p = new URLSearchParams('a=1&b=2'); return p.get('a') === '1' && p.get('b') === '2'; });
        check('P07_only_amps', function() { var p = new URLSearchParams('&&&&'); return S(p) === '' && p.size === 0; });
        check('P08_gap_pair', function() { var p = new URLSearchParams('a&&b'); return p.size === 2 && p.get('a') === '' && p.get('b') === ''; });
        check('P09_amp_eq_value', function() { var v = new URLSearchParams('&=v').getAll(''); return v.length === 1 && v[0] === 'v'; });
        check('P10_encoded_amp_value', function() { return new URLSearchParams('a=b%26c').get('a') === 'b&c'; });
        check('P11_encoded_eq_key', function() { return new URLSearchParams('%3D=v').get('=') === 'v'; });
        check('P12_plus_value', function() { return new URLSearchParams('a=b+c').get('a') === 'b c'; });
        check('P13_pct20_value', function() { return new URLSearchParams('a=b%20c').get('a') === 'b c'; });
        check('P14_plus_key', function() { return new URLSearchParams('a+b=c').get('a b') === 'c'; });
        check('P15_hex_case', function() { var p = new URLSearchParams('a=%2f&b=%2F'); return p.get('a') === '/' && p.get('b') === '/'; });

        // ── malformed percent sequences decode leniently (Node unescape catch path) ──
        check('P16_invalid_pct', function() { return new URLSearchParams('x=%ZZ').get('x') === '%ZZ'; });
        check('P17_truncated_pct', function() { return new URLSearchParams('x=a%2').get('x') === 'a%2'; });

        // ── UTF-8 decode boundaries (lengths pin the U+FFFD count without
        //    embedding replacement chars in the source) ──
        check('P18_two_byte_utf8', function() { return new URLSearchParams('a=%C3%A9').get('a') === 'é'; });
        check('P19_lone_continuation', function() { return new URLSearchParams('a=%80').get('a').length === 1; });
        check('P20_raw_unicode', function() { return new URLSearchParams('包=子').get('包') === '子'; });
        check('P21_surrogate_pair', function() { return new URLSearchParams('a=%F0%9D%92%B3').get('a') === '𝒳'; });
        check('P22_lone_surrogate_utf8', function() { return new URLSearchParams('a=%ED%A0%80').get('a').length === 3; });
        check('P23_overlong_utf8', function() { return new URLSearchParams('a=%C0%AF').get('a').length === 2; });
        check('P24_long_value', function() { var big = new Array(10001).join('x'); return new URLSearchParams('k=' + big).get('k').length === 10000; });

        // ── repeated keys ──
        check('P25_repeat_keys', function() {
            var p = new URLSearchParams('a=1&a=2&a=3');
            var all = p.getAll('a');
            return p.get('a') === '1' && all.length === 3 && all[0] === '1' && all[1] === '2' && all[2] === '3' && p.size === 3;
        });
        check('P26_has_encoded_key', function() { return new URLSearchParams('k%20k=x').has('k k') === true; });
        check('P27_delete_repeats', function() { var p = new URLSearchParams('a=1&b=2&a=3'); p.delete('a'); return S(p) === 'b=2' && p.has('a') === false; });
        check('P28_delete_with_value', function() {
            var p = new URLSearchParams('a=1&b=2&a=3');
            p.delete('a', '1');
            var rest = p.getAll('a');
            return S(p) === 'b=2&a=3' && rest.length === 1 && rest[0] === '3';
        });
        check('P29_foreach_order', function() {
            var out = [];
            new URLSearchParams('a=1&b=2&a=3').forEach(function (v, k) { out.push(k + '=' + v); });
            return out.length === 3 && out[0] === 'a=1' && out[1] === 'b=2' && out[2] === 'a=3';
        });
        // Node: set() replaces all pairs of the name in place of the first.
        check('P30_set_replace_pos', function() { var p = new URLSearchParams('a=1&b=2&a=3'); p.set('a', '9'); return S(p) === 'a=9&b=2'; });
        check('P31_set_new_key', function() { var p = new URLSearchParams('a=1'); p.set('b', '2'); return S(p) === 'a=1&b=2'; });

        // ── serializer boundaries (application/x-www-form-urlencoded) ──
        check('P32_roundtrip_recase', function() { return new URLSearchParams('a=%2f').toString() === 'a=%2F'; });
        check('P33_space_plus', function() { var p = new URLSearchParams(); p.append('k', 'a b'); return S(p) === 'k=a+b'; });
        check('P34_unreserved_set', function() {
            var p = new URLSearchParams();
            p.append('k', "~!*()'@#$&=/:;,?[] ");
            return S(p) === 'k=%7E%21*%28%29%27%40%23%24%26%3D%2F%3A%3B%2C%3F%5B%5D+';
        });
        check('P35_append_reflects', function() { var p = new URLSearchParams('a=1'); p.append('b', '2'); return S(p) === 'a=1&b=2'; });
        check('P36_noarg_ctor', function() { var p = new URLSearchParams(); return S(p) === '' && p.size === 0; });
        check('P37_sequence_init', function() {
            var v = new URLSearchParams([['a', '1'], ['b', '2'], ['a', '3']]).getAll('a');
            return v.length === 2 && v[0] === '1' && v[1] === '3';
        });
        check('P38_record_init', function() { return new URLSearchParams({ a: '1', b: '2' }).toString() === 'a=1&b=2'; });

        // ── iteration surface ──
        check('P40_iterator_spread', function() {
            var a = Array.from(new URLSearchParams('a=1&b=2'));
            return a.length === 2 && a[0][0] === 'a' && a[0][1] === '1' && a[1][0] === 'b' && a[1][1] === '2';
        });
        check('P41_keys_values_order', function() {
            var p = new URLSearchParams('a=1&b=2&a=3');
            var ks = Array.from(p.keys()), vs = Array.from(p.values());
            return ks.join(',') === 'a,b,a' && vs.join(',') === '1,2,3';
        });
        check('P42_no_storage_leak', function() { return Object.keys(new URLSearchParams('a=1&b=2')).length === 0; });
        check('P43_size_type', function() { return typeof new URLSearchParams('a=1').size === 'number'; });
        // ── sort() (GAP-1 fix): stable, name-only, UTF-16 code-unit order ──
        check('P44_sort', function() {
            var p = new URLSearchParams('b=2&a=1&c=3');
            p.sort();
            return S(p) === 'a=1&b=2&c=3' && p.sort() === undefined;
        });
        // Equal names keep insertion order (values are NOT compared).
        check('P44b_sort_stable_by_name', function() {
            var p = new URLSearchParams();
            p.append('a', '9'); p.append('a', '1'); p.append('a', '5'); p.append('b', '2');
            p.sort();
            return S(p) === 'a=9&a=1&a=5&b=2';
        });
        // Code-UNIT order: astral key (U+1D4B3, surrogate D835) sorts
        // BEFORE U+FFFF — differs from code-point/UTF-8 byte order.
        check('P44c_sort_code_unit_order', function() {
            var p = new URLSearchParams();
            p.append('￿', '1'); p.append('e', '2'); p.append('𝒳', '3'); p.append('a', '4');
            p.sort();
            var ks = Array.from(p.keys()).map(function (k) { return k.codePointAt(0).toString(16); });
            return ks.join(',') === '61,65,1d4b3,ffff';
        });

        // ── argument coercion (DEV-4 fix) ──
        check('P39_numeric_coerce', function() { return new URLSearchParams('1=x').get(1) === 'x'; });
        check('P39b_coerce_reflects', function() {
            var p = new URLSearchParams('1=x');
            p.set(2, 'y');
            return S(p) === '1=x&2=y' && p.has(2) === true;
        });

        // ── more '?' handling ──
        check('P45_double_qmark', function() { var v = new URLSearchParams('??a=1').getAll('?a'); return v.length === 1 && v[0] === '1'; });
        check('P46_pct_in_key_reencode', function() { return new URLSearchParams('%7E=v').toString() === '%7E=v'; });
        check('P47_tostring_empty_key', function() { var p = new URLSearchParams(); p.append('', 'v'); return S(p) === '=v'; });

        // ── missing-required-argument errors (DEV-4): Node throws
        //    TypeError with code ERR_MISSING_ARGS ──
        check('P48_missing_args_throw', function() {
            function thrown(fn) { try { fn(); return null; } catch (e) { return e.constructor.name + ':' + e.code; } }
            var a = thrown(function() { new URLSearchParams('a=1').get(); });
            var b = thrown(function() { new URLSearchParams().set('a'); });
            var c = thrown(function() { new URLSearchParams().append('a'); });
            return a === 'TypeError:ERR_MISSING_ARGS' && b === 'TypeError:ERR_MISSING_ARGS'
                && c === 'TypeError:ERR_MISSING_ARGS';
        });
        // Symbol arguments propagate the engine TypeError from ToString.
        check('P48b_symbol_arg_throws', function() {
            try { new URLSearchParams('a=1').get(Symbol('s')); return false; } catch (e) { return e.constructor.name === 'TypeError'; }
        });

        results.join('|')
        "#,
        52,
    );
}

#[test]
fn test_url_search_params_linkage() {
    let mut ctx = make_ctx();
    run_counted_checks(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ':' + (ok ? 'PASS' : 'FAIL')); }
            catch (e) { results.push(label + ':ERROR:' + (e && e.message ? e.message : e)); }
        }

        // searchParams mutations write back into url.search (live linkage).
        check('U01_append_syncs_search', function() { var u = new URL('http://x/p?a=1'); u.searchParams.append('b', '2'); return u.search === '?a=1&b=2'; });
        check('U02_delete_clears_search', function() {
            var u = new URL('http://x/p?a=1');
            u.searchParams.delete('a');
            return u.search === '' && u.href === 'http://x/p';
        });
        check('U12_set_syncs_search', function() { var u = new URL('http://x/p?a=1&b=2'); u.searchParams.set('a', '9'); return u.search === '?a=9&b=2'; });

        // url.search / href / pathname setters re-sync searchParams.
        check('U03_search_setter', function() {
            var u = new URL('http://x/p?a=1');
            u.search = '?z=9';
            return u.searchParams.get('z') === '9' && u.searchParams.get('a') === null;
        });
        check('U04_search_clear', function() {
            var u = new URL('http://x/p?a=1');
            u.search = '';
            return u.searchParams.size === 0 && u.search === '' && u.href === 'http://x/p';
        });
        check('U05_href_setter_resync', function() { var u = new URL('http://x/p?a=1'); u.href = 'http://y/q?b=2'; return u.searchParams.get('b') === '2'; });
        check('U11_pathname_keeps_search', function() { var u = new URL('http://x/p?a=1'); u.pathname = '/q'; return u.search === '?a=1' && u.searchParams.get('a') === '1'; });

        // Degenerate query strings on the URL side.
        // Node: a bare trailing '?' leaves search '' (query null) while href
        // keeps the '?'; '?&' is a real (whitespace-pair) query.
        check('U06_bare_qmark_search', function() {
            var u = new URL('http://x/p?');
            return u.search === '' && u.searchParams.size === 0 && u.href === 'http://x/p?';
        });
        check('U07_qmark_amp_search', function() { var u = new URL('http://x/p?&'); return u.search === '?&' && u.searchParams.size === 0; });
        check('U08_gap_pairs_in_url', function() { var u = new URL('http://x/p?a=1&&b=2'); return u.searchParams.get('a') === '1' && u.searchParams.get('b') === '2'; });
        check('U09_eq_in_value', function() { return new URL('http://x/p?a=b=c').searchParams.get('a') === 'b=c'; });
        check('U13_second_qmark_is_data', function() { return new URL('http://x/p?a=1?b=2').searchParams.get('a') === '1?b=2'; });
        check('U14_hash_keeps_search', function() { var u = new URL('http://x/p?a=1#f'); return u.search === '?a=1' && u.searchParams.get('a') === '1'; });
        check('U15_encoded_in_url_search', function() { return new URL('http://x/p?a=b%26c%3Dd').searchParams.get('a') === 'b&c=d'; });

        // searchParams is a stable per-URL object.
        check('U10_sp_identity', function() { var u = new URL('http://x/p?a=1'); return u.searchParams === u.searchParams; });

        results.join('|')
        "#,
        15,
    );
}

#[test]
fn test_url_relative_query_resolution() {
    let mut ctx = make_ctx();
    run_counted_checks(
        &mut ctx,
        r#"
        var url = require('url');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ':' + (ok ? 'PASS' : 'FAIL')); }
            catch (e) { results.push(label + ':ERROR:' + (e && e.message ? e.message : e)); }
        }

        check('R01_query_only_base', function() { return new URL('?b=2', 'http://x/p?a=1').href === 'http://x/p?b=2'; });
        // DEV-2 fix: fragment-only reference inherits the base query.
        check('R02_hash_only_keeps_query', function() {
            var u = new URL('#f', 'http://x/p?a=1');
            return u.search === '?a=1' && u.href === 'http://x/p?a=1#f';
        });
        check('R03_rel_path_new_query', function() { return new URL('c?d=4', 'http://x/a/b?z=1').href === 'http://x/a/c?d=4'; });
        check('R04_resolve_query_only', function() { return url.resolve('http://x/a/b?y=1', '?z=2') === 'http://x/a/b?z=2'; });
        check('R05_resolve_dotdot_query', function() { return url.resolve('http://x/a/b?y=1', '../c?z=2') === 'http://x/c?z=2'; });
        // DEV-3 fix: empty reference resolves to the base, with and
        // without a query on the base.
        check('R06_empty_ref_resolves_to_base', function() {
            return new URL('', 'http://x/p?a=1').href === 'http://x/p?a=1'
                && new URL('', 'http://x/p').href === 'http://x/p';
        });
        // Node also throws for a relative base — conforming error path.
        check('R06c_relative_base_throws', function() {
            try { new URL('', 'relative-base'); return false; } catch (e) { return true; }
        });

        results.join('|')
        "#,
        7,
    );
}

#[test]
fn test_url_legacy_query_edges() {
    let mut ctx = make_ctx();
    run_counted_checks(
        &mut ctx,
        r#"
        var url = require('url');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ':' + (ok ? 'PASS' : 'FAIL')); }
            catch (e) { results.push(label + ':ERROR:' + (e && e.message ? e.message : e)); }
        }

        check('L01_bare_qmark_legacy', function() { var u = url.parse('http://x/p?'); return u.query === '' && u.search === '?'; });
        check('L02_legacy_gap_pairs', function() { var q = url.parse('http://x/p?a=1&&b=2', true).query; return q.a === '1' && q.b === '2'; });
        check('L03_legacy_eq_in_value', function() { return url.parse('http://x/p?a=b=c', true).query.a === 'b=c'; });
        // DEV-1 fix: multibyte query KEYS survive the legacy object path.
        check('L04_unicode_key', function() { return url.parse('http://x/p?%E5%8C%85=%F0%9D%92%B3', true).query['包'] === '𝒳'; });
        check('L04b_unicode_value', function() { return url.parse('http://x/p?a=%E5%8C%85', true).query.a === '包'; });
        check('L05_legacy_invalid_pct', function() { return url.parse('http://x/p?x=%ZZ', true).query.x === '%ZZ'; });
        check('L06_legacy_repeat_array', function() { var a = url.parse('http://x/p?a=1&a=2', true).query.a; return Array.isArray(a) && a[0] === '1' && a[1] === '2'; });
        check('L07_legacy_encoded_plus', function() { return url.parse('http://x/p?a=%2B', true).query.a === '+'; });
        check('L08_whatwg_vs_legacy_amp', function() {
            return url.parse('http://x/p?a=b%26c', true).query.a === 'b&c'
                && new URL('http://x/p?a=b%26c').searchParams.get('a') === 'b&c';
        });
        check('L09_legacy_bare_flag', function() { return url.parse('http://x/p?flag', true).query.flag === ''; });
        check('L10_qs_vs_whatwg', function() {
            var qs = require('querystring').parse('a=1&a=2&a=3').a;
            var sp = new URLSearchParams('a=1&a=2&a=3').getAll('a');
            return Array.isArray(qs) && qs.join(',') === '1,2,3' && sp.join(',') === '1,2,3';
        });

        results.join('|')
        "#,
        11,
    );
}
