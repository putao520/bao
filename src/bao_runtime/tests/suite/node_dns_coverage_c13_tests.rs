// @trace TEST-ENG-007-DNS-C13 [req:REQ-ENG-007] [level:integration]
//
// C13 line-coverage wave for src/bao_runtime/src/node_dns.rs — the residual
// gap after C4 (node_dns_coverage_c4_tests): module-level resolve4/resolve6
// options-slot normalization, the remaining RR module wrappers (resolveAny /
// resolveNaptr), the default-result-order getter/set matrix, the full
// _invalidArgType rendering matrix, the remaining promises family +
// promisify-symbol surface, and the error-code constants table.
// Offline-first (localhost / hosts-file / local stub resolvers only), one
// env-gated real-uplink shape test (BAO_TEST_DNS) mirroring the C4 gate form.
//
// 语义真源 = 本机 node v24.19.0 oracle 实测;以下为 oracle 分歧登记项
// (characterization 钉死 bao 现行形态,只登记不修):
//
// | # | 面 | bao 现行(本文件钉死) | node v24 oracle | 判类 |
// |---|----|----------------------|-----------------|------|
// | 1 | setDefaultResultOrder 非法值 | 普通 Error(无 code),消息 'dns.setDefaultResultOrder order must be "ipv4first", "ipv6first", or "verbatim"' | ERR_INVALID_ARG_VALUE,消息 "The argument 'dnsOrder' must be one of: ..." | parity 偏离(错误码+消息形态) |
// | 2 | getDefaultResultOrder 默认值 | 'verbatim'(DNS_JS 初始字面) | 'verbatim' | 一致(回归钉) |
// | 3 | resolveAny 查询类型 | DNS_JS 落 `_resolveRR(hostname, "A")` — 实发 A 查询,返回 A 记录数组 | 发 ANY 聚合查询,返回带 type 字段的混合记录数组 | parity 偏离(能力缺口) |
// | 4 | setServers 非 string 元素 | native 静默丢弃非 string 元素,合法元素保留(C4 已钉同形) | throw ERR_INVALID_ARG_TYPE,消息含 'servers[0]' | parity 偏离(校验缺失) |

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

fn fresh_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Run a `check(label, fn)` battery in JS and assert every item PASSes.
fn assert_battery(ctx: &mut JsContext, battery: &str) {
    let results = eval_string(ctx, battery);
    let mut failures = Vec::new();
    for item in results.split('|') {
        if item.is_empty() {
            continue;
        }
        if !item.contains(" PASS") {
            failures.push(item.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "dns c13 battery failures: {:?}\nfull: {}",
        failures,
        results
    );
}

/// Drain the microtask queue so settled promise handlers run (C4 form).
fn drain_jobs(ctx: &mut JsContext) {
    let cx_raw = ctx.raw_cx();
    unsafe {
        mozjs_sys::jsapi::js::RunJobs(cx_raw);
    }
}

/// Module-level dns.resolve4/resolve6: the options-slot normalization
/// (arguments.length <= 2 shifts the callback into the options slot), the
/// explicit options-object form, callback-in-options-slot, synchronous
/// callback delivery with (null, records), and the non-function callback
/// ERR_INVALID_ARG_TYPE arm — the module-level faces C4 only pinned on the
/// Resolver instance.
#[test]
fn dns_resolve46_options_slot_and_callback_forms() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // Two-arg form: the options slot carries the callback (normalized).
        var cbRec = null;
        var ret4 = dns.resolve4('localhost', function(err, rec) { cbRec = { err: err, rec: rec }; });
        check("resolve4_twoarg_cb_fired", function() {
            return cbRec !== null && cbRec.err === null && Array.isArray(cbRec.rec) &&
                   cbRec.rec.length === 1 && cbRec.rec[0] === '127.0.0.1';
        });
        check("resolve4_twoarg_returns_records", function() {
            return Array.isArray(ret4) && ret4.length === 1 && ret4[0] === '127.0.0.1';
        });

        // Three-arg form with an options object in the middle slot: the
        // options value itself is not consumed by the A path but must not
        // break dispatch (CNAME-adjacent option shapes are resolved4's
        // positional contract with node — records still flow).
        var cb3 = null;
        dns.resolve4('localhost', { ttl: true }, function(err, rec) { cb3 = { err: err, rec: rec }; });
        check("resolve4_options_obj_cb", function() {
            return cb3 !== null && cb3.err === null && Array.isArray(cb3.rec) &&
                   cb3.rec.length === 1 && cb3.rec[0] === '127.0.0.1';
        });

        // resolve6 mirror arms (localhost carries no AAAA on the local stub —
        // the error surfaces through the callback slot, sync return []).
        var cb6 = null;
        dns.resolve6('localhost', function(err, rec) { cb6 = { err: err, rec: rec }; });
        check("resolve6_twoarg_cb_error_routed", function() {
            return cb6 !== null && cb6.err !== null && typeof cb6.err.code === 'string' &&
                   Array.isArray(cb6.rec) === false;
        });
        var ret6 = dns.resolve6('localhost');
        check("resolve6_twoarg_fallback_array", function() { return Array.isArray(ret6); });
        var cb6opts = null;
        dns.resolve6('localhost', { ttl: true }, function(err) { cb6opts = err; });
        check("resolve6_options_obj_cb_error_routed", function() {
            return cb6opts !== null && typeof cb6opts.code === 'string';
        });

        // Non-function callback in the options slot: ERR_INVALID_ARG_TYPE.
        var badMsg = '';
        try { dns.resolve4('localhost', 42); }
        catch (e) { badMsg = String(e.message); }
        check("resolve4_bad_cb_msg", function() {
            return badMsg.indexOf('The "callback" argument must be of type function') === 0 &&
                   badMsg.indexOf('type number (42)') !== -1;
        });
        var badCode = '';
        try { dns.resolve4('localhost', 42); }
        catch (e) { badCode = e.code; }
        check("resolve4_bad_cb_code", function() { return badCode === 'ERR_INVALID_ARG_TYPE'; });

        results.join('|')
        "#,
    );
}

/// The default-result-order getter/set matrix: the 'verbatim' default, the
/// three legal orders round-trip through the getter, and each illegal value
/// throws without mutating the current order.
/// 登记项 #1: bao throws a plain Error (no .code) with the
/// 'dns.setDefaultResultOrder order must be ...' message; node v24 throws
/// ERR_INVALID_ARG_VALUE with the "must be one of" message.
#[test]
fn dns_default_result_order_getter_matrix() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        check("order_default_verbatim", function() { return dns.getDefaultResultOrder() === 'verbatim'; });

        dns.setDefaultResultOrder('ipv6first');
        check("order_set_ipv6first", function() { return dns.getDefaultResultOrder() === 'ipv6first'; });
        dns.setDefaultResultOrder('ipv4first');
        check("order_set_ipv4first", function() { return dns.getDefaultResultOrder() === 'ipv4first'; });
        dns.setDefaultResultOrder('verbatim');
        check("order_set_verbatim_back", function() { return dns.getDefaultResultOrder() === 'verbatim'; });

        // Illegal values: throw, and leave the current order untouched.
        var msg1 = '';
        try { dns.setDefaultResultOrder('bogus'); }
        catch (e) { msg1 = String(e.message); }
        check("order_bad_string_throws", function() {
            return msg1.indexOf('must be') !== -1 && msg1.indexOf('ipv4first') !== -1 &&
                   msg1.indexOf('verbatim') !== -1;
        });
        check("order_unchanged_after_bad_string", function() { return dns.getDefaultResultOrder() === 'verbatim'; });

        var threw42 = false;
        try { dns.setDefaultResultOrder(42); } catch (e) { threw42 = true; }
        check("order_bad_number_throws", function() { return threw42; });
        var threwNull = false;
        try { dns.setDefaultResultOrder(null); } catch (e) { threwNull = true; }
        check("order_bad_null_throws", function() { return threwNull; });
        check("order_unchanged_after_bad_others", function() { return dns.getDefaultResultOrder() === 'verbatim'; });

        // The promises namespace exposes the same pair.
        check("order_promises_pair_identity", function() {
            return dns.promises.getDefaultResultOrder === dns.getDefaultResultOrder &&
                   dns.promises.setDefaultResultOrder === dns.setDefaultResultOrder;
        });

        results.join('|')
        "#,
    );
}

/// The remaining RR module wrappers C4 did not pin: resolveAny / resolveNaptr
/// module-level faces, resolve() with explicit SRV / NAPTR rrtypes, and the
/// callback-routed error form for each (localhost carries none of these RR
/// types on the local resolver — the per-type fallback arrays are the sync
/// return, the coded error routes through the callback slot).
#[test]
fn dns_module_rr_wrappers_any_naptr_and_explicit_rrtypes() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // resolveAny module-level: 登记项 #3 — bao 落 A 查询,对 localhost
        // fulfill with the A record (node 发 ANY 聚合;此处钉 bao 现行形态).
        var any = dns.resolveAny('localhost');
        check("resolveAny_is_A_query_fulfills", function() {
            return Array.isArray(any) && any.length === 1 && any[0] === '127.0.0.1';
        });
        var anyCb = null;
        dns.resolveAny('localhost', function(err, rec) { anyCb = { err: err, rec: rec }; });
        check("resolveAny_cb_records", function() {
            return anyCb.err === null && Array.isArray(anyCb.rec) && anyCb.rec[0] === '127.0.0.1';
        });

        // resolveNaptr module-level mirror arms.
        var nap = dns.resolveNaptr('localhost');
        check("resolveNaptr_fallback_array", function() { return Array.isArray(nap); });
        var napErr = null;
        dns.resolveNaptr('localhost', function(err) { napErr = err; });
        check("resolveNaptr_error_routed", function() {
            return napErr !== null && typeof napErr.code === 'string';
        });

        // resolve() with explicit less-common rrtypes rides the same
        // __dns_resolve_rr dispatch (sync fallback + callback-routed error).
        check("resolve_srv_explicit_fallback", function() {
            return Array.isArray(dns.resolve('localhost', 'SRV'));
        });
        check("resolve_naptr_explicit_fallback", function() {
            return Array.isArray(dns.resolve('localhost', 'NAPTR'));
        });
        check("resolve_txt_explicit_fallback", function() {
            return Array.isArray(dns.resolve('localhost', 'TXT'));
        });
        var srvErr = null;
        dns.resolve('localhost', 'SRV', function(err) { srvErr = err; });
        check("resolve_srv_error_routed", function() {
            return srvErr !== null && typeof srvErr.code === 'string' &&
                   String(srvErr.message).indexOf('localhost') !== -1;
        });

        // Guaranteed-missing name over the Any wrapper: the error triple
        // carries the queryAny syscall tag like the resolve4 arm C4 pinned.
        var anyMissing = null;
        dns.resolveAny('no-such-name-c13.invalid', function(err) { anyMissing = err; });
        check("resolveAny_missing_code", function() {
            return anyMissing !== null && anyMissing.code === 'ENOTFOUND';
        });

        results.join('|')
        "#,
    );
}

/// _invalidArgType rendering matrix — every determineSpecificType shape the
/// JS validators can produce: named/anonymous functions, string truncation
/// at 27 chars (first 24 + "..."), booleans, arrays, null, plain objects
/// with a constructor name, and the undefined-slot tolerance (an absent
/// callback never throws).
#[test]
fn dns_invalid_arg_type_shape_matrix() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }
        function msgOf(fn) {
            try { fn(); return 'NO-THROW'; }
            catch (e) { return String(e.message) + '||' + (e.code || ''); }
        }

        // A named function IS a valid callback — it must not throw and must
        // actually run (records delivered through the callback slot).
        var namedRan = false;
        dns.resolveTxt('localhost', function namedCb(err, rec) { namedRan = true; });
        check("named_fn_callback_ran", function() { return namedRan === true; });

        // Boolean value: "type boolean (true)".
        var mb = msgOf(function() { dns.resolveTxt('localhost', true); });
        check("boolean_rendering", function() {
            return mb.indexOf('type boolean (true)') !== -1 && mb.indexOf('ERR_INVALID_ARG_TYPE') !== -1;
        });

        // Array value (callback slot): "an instance of Array".
        var ma = msgOf(function() { dns.resolveTxt('localhost', [1, 2]); });
        check("array_rendering_name_slot", function() {
            return ma.indexOf('an instance of Array') !== -1;
        });

        // null value (callback slot): "null".
        var mn = msgOf(function() { dns.resolveTxt('localhost', null); });
        check("null_rendering", function() { return mn.indexOf('Received null') !== -1; });

        // Plain object (callback slot): "an instance of Object".
        var mo = msgOf(function() { dns.resolveTxt('localhost', { a: 1 }); });
        check("object_rendering", function() {
            return mo.indexOf('an instance of Object') !== -1;
        });

        // Custom constructor name surfaces through value.constructor.name.
        function MyThing() {}
        var mm = msgOf(function() { dns.resolveTxt('localhost', new MyThing()); });
        check("ctor_name_rendering", function() {
            return mm.indexOf('an instance of MyThing') !== -1;
        });

        // String truncation: 30-char string renders first 24 + "...".
        var long = 'x30chars_padding_padding_padding'; // 32 chars
        var ms = msgOf(function() { dns.resolveTxt('localhost', long); });
        check("string_truncation_rendering", function() {
            return ms.indexOf("type string ('" + long.slice(0, 24) + "...')") !== -1;
        });

        // Short string renders in full (no ellipsis).
        var mshort = msgOf(function() { dns.resolveTxt('localhost', 'short'); });
        check("string_short_rendering", function() {
            return mshort.indexOf("type string ('short')") !== -1;
        });

        // Undefined callback slot: absent callback never throws (the
        // sync-return path runs instead).
        check("undefined_cb_no_throw", function() {
            return Array.isArray(dns.resolveTxt('localhost'));
        });

        // lookupService has no JS-level callback validation: a truthy
        // non-function callback reaches `if (callback) callback(...)` and
        // raises a bare TypeError ('is not a function').
        var lsMsg = '';
        try { dns.lookupService('127.0.0.1', 80, 42); }
        catch (e) { lsMsg = String(e.message); }
        check("lookupService_truthy_nonfn_cb_throws", function() {
            return lsMsg.indexOf('not a function') !== -1;
        });

        // An absent callback is falsy — the sync result object returns.
        var lsRet = dns.lookupService('127.0.0.1', 80);
        check("lookupService_undefined_cb_sync_result", function() {
            return lsRet !== null && typeof lsRet === 'object' &&
                   typeof lsRet.hostname === 'string' && typeof lsRet.service === 'string';
        });

        results.join('|')
        "#,
    );
}

/// The remaining dns.promises family (resolveCname/Ns/Ptr/Soa/Srv/Txt/Naptr/
/// Any/Mx/lookupService) fulfills or rejects after a microtask drain, and
/// every module-level function carries the promisify custom symbol mapping.
#[test]
fn dns_promises_remaining_family_and_promisify_symbols() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    eval_string(
        &mut ctx,
        r#"
        var dns = require('dns');
        globalThis.__c13 = {};
        function mark(key, p) {
            p.then(function(v) { globalThis.__c13[key] = 'ok:' + JSON.stringify(v); },
                   function(e) { globalThis.__c13[key] = 'rej:' + (e.code || e.message); });
        }
        mark('cname', dns.promises.resolveCname('localhost'));
        mark('ns', dns.promises.resolveNs('localhost'));
        mark('ptr', dns.promises.resolvePtr('127.0.0.1'));
        mark('soa', dns.promises.resolveSoa('localhost'));
        mark('srv', dns.promises.resolveSrv('localhost'));
        mark('txt', dns.promises.resolveTxt('localhost'));
        mark('naptr', dns.promises.resolveNaptr('localhost'));
        mark('any', dns.promises.resolveAny('localhost'));
        mark('mx', dns.promises.resolveMx('localhost'));
        mark('lookupService', dns.promises.lookupService('127.0.0.1', 80));
        mark('missing', dns.promises.resolve4('no-such-name-c13p.invalid'));
        'scheduled'
        "#,
    );
    drain_jobs(&mut ctx);

    assert_battery(
        &mut ctx,
        r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }
        function stateOf(key) { return String(globalThis.__c13[key] || 'unset'); }

        // localhost carries A + CNAME (+ PTR on 127.0.0.1) on the local
        // resolver; every other RR type rejects with a coded error.
        check("p_cname_fulfilled", function() { return stateOf('cname').indexOf('ok:') === 0; });
        check("p_ptr_fulfilled", function() { return stateOf('ptr').indexOf('ok:') === 0; });
        check("p_ns_rejected_coded", function() { return stateOf('ns').indexOf('rej:') === 0; });
        check("p_soa_rejected_coded", function() { return stateOf('soa').indexOf('rej:') === 0; });
        check("p_srv_rejected_coded", function() { return stateOf('srv').indexOf('rej:') === 0; });
        check("p_txt_rejected_coded", function() { return stateOf('txt').indexOf('rej:') === 0; });
        check("p_naptr_rejected_coded", function() { return stateOf('naptr').indexOf('rej:') === 0; });
        // 登记项 #3: resolveAny rides the A query — localhost fulfills with
        // the A record through the promise wrapper too.
        check("p_any_fulfilled_a_record", function() {
            return stateOf('any').indexOf('ok:["127.0.0.1"]') === 0;
        });
        check("p_mx_rejected_coded", function() { return stateOf('mx').indexOf('rej:') === 0; });
        check("p_lookupService_fulfilled", function() { return stateOf('lookupService').indexOf('ok:') === 0; });
        check("p_missing_rejects_enotfound", function() { return stateOf('missing').indexOf('rej:ENOTFOUND') === 0; });

        // promisify custom symbol: every module fn maps onto its promise twin.
        var sym = Symbol.for("nodejs.util.promisify.custom");
        var names = ['lookup', 'lookupService', 'resolve', 'reverse', 'resolve4', 'resolve6',
                     'resolveAny', 'resolveCname', 'resolveMx', 'resolveNaptr', 'resolveNs',
                     'resolvePtr', 'resolveSoa', 'resolveSrv', 'resolveTxt'];
        var allMapped = true;
        for (var i = 0; i < names.length; i++) {
            if (dns[names[i]][sym] !== dns.promises[names[i]]) { allMapped = false; break; }
        }
        check("promisify_symbols_all_15", function() { return allMapped; });

        results.join('|')
        "#,
    );
}

/// Resolver/module parity pass-through: instance resolve(hostname, rrtype,
/// callback) forwards the explicit rrtype, error-code constants table values,
/// and the getServers/setServers string-element validation messages.
#[test]
fn dns_resolver_rrtype_passthrough_constants_and_server_validation() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // Instance resolve with explicit rrtype forwards to the dispatch and
        // delivers (err, records) through the callback slot.
        var got = null;
        var r = new dns.Resolver();
        r.resolve('localhost', 'CNAME', function(err, rec) { got = { err: err, rec: rec }; });
        check("resolver_rrtype_cb", function() {
            return got !== null && got.err === null && Array.isArray(got.rec) &&
                   got.rec.length >= 1;
        });

        // Error-code constants table: the full 24-entry E-prefixed map.
        var codes = {
            NODATA: 'ENODATA', FORMERR: 'EFORMERR', SERVFAIL: 'ESERVFAIL',
            NOTFOUND: 'ENOTFOUND', NOTIMP: 'ENOTIMP', REFUSED: 'EREFUSED',
            BADQUERY: 'EBADQUERY', BADNAME: 'EBADNAME', BADFAMILY: 'EBADFAMILY',
            BADRESP: 'EBADRESP', CONNREFUSED: 'ECONNREFUSED', TIMEOUT: 'ETIMEOUT',
            EOF: 'EOF', FILE: 'EFILE', NOMEM: 'ENOMEM', DESTRUCTION: 'EDESTRUCTION',
            BADSTR: 'EBADSTR', BADFLAGS: 'EBADFLAGS', NONAME: 'ENONAME',
            BADHINTS: 'EBADHINTS', NOTINITIALIZED: 'ENOTINITIALIZED',
            LOADIPHLPAPI: 'ELOADIPHLPAPI', ADDRGETNETWORKPARAMS: 'EADDRGETNETWORKPARAMS',
            CANCELLED: 'ECANCELLED'
        };
        var codesOk = true;
        for (var k in codes) { if (dns[k] !== codes[k]) { codesOk = false; break; } }
        check("error_constants_table_24", function() { return codesOk; });

        // setServers element handling: 登记项 #4 — non-string elements are
        // silently dropped (native filter), strings kept; node v24 throws
        // ERR_INVALID_ARG_TYPE with the servers[0] index instead.
        dns.setServers([1, '9.9.9.9', {}, '8.8.4.4']);
        check("setservers_drops_nonstrings", function() {
            return dns.getServers().join(',') === '9.9.9.9,8.8.4.4';
        });

        // Restores the shared registry for the neighboring tests
        // (nextest runs each test in its own process, this is belt+braces).
        try { dns.setServers([]); } catch (e) {}

        results.join('|')
        "#,
    );
}

/// Real-uplink SOA/NS shape rendering — the resolveSoa object fields and the
/// resolveNs string-array entries only the public DNS carries. Canonical env
/// runs this via --run-ignored=all; plain runs skip it (C4 gate form).
#[ignore = "需要真实外网 DNS;真跑: BAO_TEST_DNS=1 cargo nextest run -E 'test(dns_real_network_soa_shapes)'"]
#[test]
fn dns_real_network_soa_shapes() {
    if std::env::var("BAO_TEST_DNS").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_DNS != 1 — real-uplink SOA/NS shapes not exercised");
        return;
    }
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }
        function stripDot(s) { return String(s).replace(/\.$/, ''); }

        // resolveSoa: the 7-field object shape (field names per node docs;
        // only types are asserted — record values drift with the zone).
        var soa = dns.resolveSoa('example.com');
        check("soa_object_fields", function() {
            return soa !== null && typeof soa === 'object' && !Array.isArray(soa) &&
                   typeof soa.nsname === 'string' && soa.nsname.length > 0 &&
                   typeof soa.hostmaster === 'string' &&
                   typeof soa.serial === 'number' && soa.serial > 0 &&
                   typeof soa.refresh === 'number' &&
                   typeof soa.retry === 'number' &&
                   typeof soa.expire === 'number' &&
                   typeof soa.minttl === 'number';
        });

        // resolveNs: string entries, example.com delegates to ns*-related
        // hosts (every entry a non-empty string).
        var ns = dns.resolveNs('example.com');
        check("ns_string_entries", function() {
            return Array.isArray(ns) && ns.length >= 2 &&
                   ns.every(function(s) { return typeof s === 'string' && s.length > 0; });
        });

        // resolveTxt: array-of-string-arrays shape (node contract).
        var txt = dns.resolveTxt('example.com');
        check("txt_array_of_arrays", function() {
            return Array.isArray(txt) && txt.length >= 1 &&
                   txt.every(function(chunk) { return Array.isArray(chunk); });
        });

        results.join('|')
        "#,
    );
}
