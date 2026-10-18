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
// 语义真源 = 本机 node v24.19.0 oracle 实测;C13 登记的 4 项 parity 偏离
// 已于 D 波根治,以下断言即 oracle 形态:
//
// | # | 面 | 现行为(本文件钉死) | node v24 oracle |
// |---|----|----------------------|-----------------|
// | 1 | setDefaultResultOrder 非法值 | TypeError + code ERR_INVALID_ARG_VALUE,消息 "The argument 'dnsOrder' must be one of: ..." | 同 |
// | 2 | getDefaultResultOrder 默认值 | 'verbatim'(DNS_JS 初始字面) | 'verbatim'(一致) |
// | 3 | resolveAny 查询类型 | 单发 ANY 查询,聚合带 type 字段的记录(localhost 形如 `{address:'127.0.0.1',ttl:0,type:'A'}`) | 同 |
// | 4 | setServers 非 string 元素 | throw ERR_INVALID_ARG_TYPE,消息含 'servers[0]'(数组/元素双校验,throw 前零变更) | 同 |

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
/// throws a TypeError with Node's ERR_INVALID_ARG_VALUE code + exact message
/// (D-wave oracle flip; node v24.19.0 emits the same shape and inspection).
/// The failed set never mutates the current order.
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

        // Illegal values: TypeError + ERR_INVALID_ARG_VALUE + Node's exact
        // enum-validation message, and the current order stays untouched.
        var badStr = null;
        try { dns.setDefaultResultOrder('bogus'); }
        catch (e) { badStr = { name: e.name, code: e.code, msg: String(e.message) }; }
        check("order_bad_string_typeerror", function() {
            return badStr !== null && badStr.name === 'TypeError';
        });
        check("order_bad_string_err_value_code", function() {
            return badStr !== null && badStr.code === 'ERR_INVALID_ARG_VALUE';
        });
        check("order_bad_string_msg", function() {
            return badStr !== null && badStr.msg ===
                "The argument 'dnsOrder' must be one of: 'verbatim', 'ipv4first', 'ipv6first'. Received 'bogus'";
        });
        check("order_unchanged_after_bad_string", function() { return dns.getDefaultResultOrder() === 'verbatim'; });

        var badNum = null;
        try { dns.setDefaultResultOrder(42); }
        catch (e) { badNum = { code: e.code, msg: String(e.message) }; }
        check("order_bad_number_msg", function() {
            return badNum !== null && badNum.code === 'ERR_INVALID_ARG_VALUE' &&
                   badNum.msg === "The argument 'dnsOrder' must be one of: 'verbatim', 'ipv4first', 'ipv6first'. Received 42";
        });
        var badNull = null;
        try { dns.setDefaultResultOrder(null); }
        catch (e) { badNull = { code: e.code, msg: String(e.message) }; }
        check("order_bad_null_msg", function() {
            return badNull !== null && badNull.code === 'ERR_INVALID_ARG_VALUE' &&
                   badNull.msg.indexOf('Received null') !== -1;
        });
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

        // resolveAny module-level: one ANY query, mixed records with the
        // Node object shape ({address, ttl, type} for A). localhost answers
        // exactly one A record with ttl 0 on both engines.
        var any = dns.resolveAny('localhost');
        check("resolveAny_any_query_aggregates", function() {
            return Array.isArray(any) && any.length === 1 && any[0] !== null &&
                   typeof any[0] === 'object' && any[0].address === '127.0.0.1' &&
                   typeof any[0].ttl === 'number' && any[0].type === 'A';
        });
        var anyCb = null;
        dns.resolveAny('localhost', function(err, rec) { anyCb = { err: err, rec: rec }; });
        check("resolveAny_cb_records", function() {
            return anyCb.err === null && Array.isArray(anyCb.rec) && anyCb.rec.length === 1 &&
                   anyCb.rec[0].address === '127.0.0.1' && anyCb.rec[0].type === 'A';
        });

        // Explicit rrtype ANY rides the same aggregate path (node v24
        // accepts dns.resolve(host, 'ANY', cb)).
        check("resolve_explicit_any_aggregates", function() {
            var r = dns.resolve('localhost', 'ANY');
            return Array.isArray(r) && r.length === 1 && r[0].address === '127.0.0.1' &&
                   r[0].type === 'A';
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
        // resolveAny rides the ANY aggregate query: localhost fulfills with
        // the Node A-record object, key order address/ttl/type.
        check("p_any_fulfilled_any_record", function() {
            return stateOf('any').indexOf(
                'ok:[{"address":"127.0.0.1","ttl":0,"type":"A"}]') === 0;
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

        // setServers element handling (oracle flip): a non-string element
        // throws ERR_INVALID_ARG_TYPE naming its servers[i] slot, before any
        // mutation; the array itself must be an Array (Node validation).
        dns.setServers(['9.9.9.9']);
        var ssErr = null;
        try { dns.setServers([1, '9.9.9.9', {}, '8.8.4.4']); }
        catch (e) { ssErr = { name: e.name, code: e.code, msg: String(e.message) }; }
        check("setservers_nonstring_element_throws", function() {
            return ssErr !== null && ssErr.name === 'TypeError' &&
                   ssErr.code === 'ERR_INVALID_ARG_TYPE' &&
                   ssErr.msg === 'The "servers[0]" argument must be of type string. Received type number (1)';
        });
        check("setservers_unchanged_after_throw", function() {
            return dns.getServers().join(',') === '9.9.9.9';
        });
        var ssObj = '';
        try { dns.setServers([{}, '8.8.4.4']); }
        catch (e) { ssObj = String(e.message); }
        check("setservers_object_element_msg", function() {
            return ssObj === 'The "servers[0]" argument must be of type string. Received an instance of Object';
        });
        var ssNonArray = null;
        try { dns.setServers('not-an-array'); }
        catch (e) { ssNonArray = { code: e.code, msg: String(e.message) }; }
        check("setservers_nonarray_throws", function() {
            return ssNonArray !== null && ssNonArray.code === 'ERR_INVALID_ARG_TYPE' &&
                   ssNonArray.msg === 'The "servers" argument must be an instance of Array. Received type string (\'not-an-array\')';
        });
        check("setservers_unchanged_after_nonarray", function() {
            return dns.getServers().join(',') === '9.9.9.9';
        });

        // The Resolver instance shares the same validation surface.
        var rSrv = new dns.Resolver();
        var rsErr = null;
        try { rSrv.setServers([null]); }
        catch (e) { rsErr = { code: e.code, msg: String(e.message) }; }
        check("resolver_setservers_element_throws", function() {
            return rsErr !== null && rsErr.code === 'ERR_INVALID_ARG_TYPE' &&
                   rsErr.msg === 'The "servers[0]" argument must be of type string. Received null';
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
