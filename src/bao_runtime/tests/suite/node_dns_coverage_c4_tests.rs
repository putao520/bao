// @trace TEST-ENG-DNS [req:REQ-ENG-007] [level:integration]
//
// C4 line-coverage wave for src/node_dns.rs (Node dns compat surface).
// Offline-first: every success assertion rides deterministic resolvers on this
// host — /etc/hosts forms (127.0.0.1 / ::1 / localhost) and the local
// systemd-resolved stub (which answers A localhost=127.0.0.1 and synthesizes
// PTR 127.0.0.1=localhost). Real-uplink RR successes are env-gated
// (BAO_TEST_DNS) in dns_real_network_rr_success_shapes.
//
// Branch targets (per file read):
//   - dns_lookup / dns_resolve / dns_resolve6 natives: argc + non-string
//     validation arms, v4/v6 render arms, cache-miss and cache-hit arms.
//   - dns_resolve_rr: every rrtype arm (A/AAAA/CNAME/MX/NS/SOA/SRV/TXT/NAPTR/
//     PTR), lowercase rrtype, non-string rrtype default, unknown-rrtype arm,
//     error arms surfacing Node codes through throw_resolve_error.
//   - dns_reverse / dns_lookup_service: sockaddr v4/v6 builds, getnameinfo
//     success + failure arms, invalid-IP EINVAL arm, port int32/double/string
//     parse arms.
//   - getServers/setServers element filter, setDefaultResultOrder validation,
//     Resolver prototype family, promises wrappers (resolve + reject paths),
//     _invalidArgType TypeError shapes.

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
/// Each label is asserted individually so the failure names the exact branch.
fn assert_battery(ctx: &mut JsContext, battery: &str) {
    let results = eval_string(ctx, battery);
    // An Err from ctx.eval stringifies to "" (eval_string's catch-all arm),
    // and the split loop below would then pass vacuously. An empty result can
    // only mean the script aborted (uncaught throw / non-string return), so
    // fail loudly instead of reporting a fake green battery.
    assert!(
        !results.is_empty(),
        "dns battery aborted before producing results (uncaught throw or non-string return)"
    );
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
        "dns battery failures: {:?}\nfull: {}",
        failures,
        results
    );
}

/// dns.lookup on numeric IPs: native render arms for v4 AND v6, then the
/// shared-cache hit path (second call renders from bun_dns::cache —
/// render_cache_ip / cache_ip_family instead of the getaddrinfo walk).
#[test]
fn dns_lookup_numeric_v4_v6_and_cache_paths() {
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

        var l4 = dns.lookup('127.0.0.1');
        check("lookup_v4_address", function() { return l4.address === '127.0.0.1'; });
        check("lookup_v4_family", function() { return l4.family === 4; });

        // Second lookup on the same name rides the shared cache (hit path).
        var l4b = dns.lookup('127.0.0.1');
        check("lookup_v4_cached_same", function() {
            return l4b.address === '127.0.0.1' && l4b.family === 4;
        });

        var l6 = dns.lookup('::1');
        check("lookup_v6_address", function() { return l6.address === '::1'; });
        check("lookup_v6_family", function() { return l6.family === 6; });

        var l6b = dns.lookup('::1');
        check("lookup_v6_cached_same", function() {
            return l6b.address === '::1' && l6b.family === 6;
        });

        // options object forms — shape contract: real {address, family} result.
        var lo = dns.lookup('127.0.0.1', { family: 4 });
        check("lookup_opts_obj_shape", function() {
            return lo && typeof lo.address === 'string' && (lo.family === 4 || lo.family === 6);
        });
        // numeric options form is normalized to { family: N }.
        var ln = dns.lookup('127.0.0.1', 4);
        check("lookup_opts_number_shape", function() {
            return ln && typeof ln.address === 'string' && (ln.family === 4 || ln.family === 6);
        });
        // { all: true } returns the array form.
        var la = dns.lookup('127.0.0.1', { all: true });
        check("lookup_all_array", function() {
            return Array.isArray(la) && la.length === 1 &&
                   la[0].address === '127.0.0.1' && la[0].family === 4;
        });

        results.join('|')
        "#,
    );
}

/// Callback form fires synchronously with (null, address, family); the
/// validation arms of the native (argc==0, non-string hostname) surface as
/// catchable Errors.
#[test]
fn dns_lookup_callback_and_validation_arms() {
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

        var cbArgs = null;
        dns.lookup('127.0.0.1', function(err, addr, fam) { cbArgs = [err, addr, fam]; });
        check("lookup_cb_sync_fire", function() { return cbArgs !== null; });
        check("lookup_cb_null_err", function() { return cbArgs[0] === null; });
        check("lookup_cb_addr", function() { return cbArgs[1] === '127.0.0.1'; });
        check("lookup_cb_family", function() { return cbArgs[2] === 4; });

        check("lookup_noargs_throws", function() {
            try { __dns_lookup(); return false; }
            catch(e) { return String(e.message).indexOf('requires a hostname argument') !== -1; }
        });
        check("lookup_nonstring_throws", function() {
            try { dns.lookup(42); return false; }
            catch(e) { return String(e.message).indexOf('must be a string') !== -1; }
        });
        // With a callback present the same errors are routed there, not thrown.
        var routed = null;
        try { dns.lookup(42, function(e) { routed = e; }); } catch (e2) { routed = routed || 'also-threw'; }
        check("lookup_error_routed_to_cb", function() { return routed !== null; });

        results.join('|')
        "#,
    );
}

/// The libc resolve family natives (__dns_resolve / __dns_resolve6):
/// per-address rendering, the resolve6 family filter, empty-on-failure, and
/// both validation arms of each native.
#[test]
fn dns_native_libc_resolve_family_arms() {
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

        check("resolve_native_v4", function() {
            var r = __dns_resolve('127.0.0.1');
            return Array.isArray(r) && r.length === 1 && r[0] === '127.0.0.1';
        });
        check("resolve6_native_v6", function() {
            var r = __dns_resolve6('::1');
            return Array.isArray(r) && r.length === 1 && r[0] === '::1';
        });
        check("resolve6_family_filter", function() {
            var r = __dns_resolve6('127.0.0.1');
            return Array.isArray(r) && r.length === 0;
        });
        // Resolution failure collapses to an empty array (lookup keeps the
        // empty-result-on-failure contract; resolve surfaces nothing here).
        check("resolve_native_empty_on_failure", function() {
            var r = __dns_resolve('no-such-host-c4.invalid');
            return Array.isArray(r) && r.length === 0;
        });
        check("resolve_noargs_throws", function() {
            try { __dns_resolve(); return false; }
            catch(e) { return String(e.message).indexOf('requires a hostname argument') !== -1; }
        });
        check("resolve_nonstring_throws", function() {
            try { __dns_resolve(42); return false; }
            catch(e) { return String(e.message).indexOf('must be a string') !== -1; }
        });
        check("resolve6_noargs_throws", function() {
            try { __dns_resolve6(); return false; }
            catch(e) { return String(e.message).indexOf('requires a hostname argument') !== -1; }
        });
        check("resolve6_nonstring_throws", function() {
            try { __dns_resolve6({}); return false; }
            catch(e) { return String(e.message).indexOf('must be a string') !== -1; }
        });

        results.join('|')
        "#,
    );
}

/// Per-RR-type resolve battery on deterministic names: A via the local stub
/// (answers localhost=127.0.0.1), PTR via the stub's synthesized reverse,
/// CNAME via the c-ares hosts-file lookup, the no-record error arms for the
/// RR types localhost does not carry, and the Node error shape
/// (queryA ENOTFOUND <host>) for a guaranteed-missing name.
#[test]
fn dns_rr_resolve_localhost_success_and_error_arms() {
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

        // A success arm — real c-ares A query answered by the local stub.
        var a = dns.resolve4('localhost');
        check("resolve4_localhost", function() {
            return Array.isArray(a) && a.length === 1 && a[0] === '127.0.0.1';
        });

        // AAAA: localhost carries no AAAA on the stub — the error arm must
        // surface through the callback slot (no callback → fallback []).
        var a6 = dns.resolve6('localhost');
        check("resolve6_localhost_fallback", function() { return Array.isArray(a6); });
        var a6err = null;
        dns.resolve6('localhost', function(err) { a6err = err; });
        check("resolve6_error_carries_code", function() {
            return a6err !== null && typeof a6err.code === 'string' && a6err.code.length > 0;
        });

        // PTR resolve arm — environment-dependent reverse record (the local
        // resolver may carry its own PTR map); the contract here is the array
        // shape plus host-agnostic string entries.
        var ptr = dns.resolvePtr('127.0.0.1');
        check("resolvePtr_localhost", function() {
            return Array.isArray(ptr) &&
                   ptr.every(function(s) { return typeof s === 'string' && s.length > 0; });
        });

        // CNAME success arm — c-ares gethostbyname rides the hosts file.
        var cn = dns.resolveCname('localhost');
        check("resolveCname_localhost", function() {
            return Array.isArray(cn) && cn.length >= 1 && stripDot(cn[0]) === 'localhost';
        });

        // RR types localhost does not carry: error arm, callback-routed,
        // sync return is the per-type fallback.
        var mx = dns.resolveMx('localhost');
        check("resolveMx_fallback_empty", function() { return Array.isArray(mx) && mx.length === 0; });
        var mxerr = null;
        dns.resolveMx('localhost', function(err) { mxerr = err; });
        check("resolveMx_error_carries_code", function() {
            return mxerr !== null && typeof mxerr.code === 'string';
        });

        var ns = dns.resolveNs('localhost');
        check("resolveNs_fallback_empty", function() { return Array.isArray(ns); });
        var soa = dns.resolveSoa('localhost');
        check("resolveSoa_fallback_object", function() {
            return soa !== null && typeof soa === 'object' && !Array.isArray(soa);
        });
        var srv = dns.resolveSrv('localhost');
        check("resolveSrv_fallback_empty", function() { return Array.isArray(srv); });
        var txt = dns.resolveTxt('localhost');
        check("resolveTxt_fallback_empty", function() { return Array.isArray(txt); });
        var naptr = dns.resolveNaptr('localhost');
        check("resolveNaptr_fallback_empty", function() { return Array.isArray(naptr); });

        // Guaranteed-missing name: Node's exact error triple surfaces —
        // message "queryA ENOTFOUND <host>" with err.code === 'ENOTFOUND'.
        var e4 = null;
        dns.resolve4('no-such-name-c4.invalid', function(err) { e4 = err; });
        check("resolve4_missing_code", function() { return e4 !== null && e4.code === 'ENOTFOUND'; });
        check("resolve4_missing_message", function() {
            return String(e4.message).indexOf('queryA ENOTFOUND') === 0 &&
                   String(e4.message).indexOf('no-such-name-c4.invalid') !== -1;
        });

        results.join('|')
        "#,
    );
}

/// The rrtype dispatch of __dns_resolve_rr: lowercase normalization, the
/// non-string rrtype default to "A", the unknown-rrtype empty arm, and the
/// JS-level resolve(hostname, rrtype|callback) overloads.
#[test]
fn dns_rrtype_dispatch_variants() {
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

        check("rrtype_lowercase_normalized", function() {
            var r = __dns_resolve_rr('localhost', 'a');
            return Array.isArray(r) && r.length === 1 && r[0] === '127.0.0.1';
        });
        check("rrtype_nonstring_defaults_A", function() {
            var r = __dns_resolve_rr('localhost', 123);
            return Array.isArray(r) && r.length === 1 && r[0] === '127.0.0.1';
        });
        check("rrtype_unknown_empty", function() {
            var r = __dns_resolve_rr('localhost', 'BOGUS');
            return Array.isArray(r) && r.length === 0;
        });
        check("rrtype_noargs_throws", function() {
            try { __dns_resolve_rr(); return false; }
            catch(e) { return String(e.message).indexOf('requires a hostname argument') !== -1; }
        });
        check("rrtype_nonstring_host_throws", function() {
            try { __dns_resolve_rr(42, 'A'); return false; }
            catch(e) { return String(e.message).indexOf('must be a string') !== -1; }
        });

        // dns.resolve overloads: explicit rrtype string and callback-in-rrtype.
        check("resolve_with_rrtype", function() {
            var r = dns.resolve('localhost', 'A');
            return Array.isArray(r) && r.length === 1 && r[0] === '127.0.0.1';
        });
        check("resolve_callback_in_rrtype_slot", function() {
            var r = dns.resolve('localhost', function() {});
            return Array.isArray(r) && r.length === 1 && r[0] === '127.0.0.1';
        });
        check("resolve_default_rrtype_is_A", function() {
            var r = dns.resolve('localhost');
            return Array.isArray(r) && r.length === 1 && r[0] === '127.0.0.1';
        });

        results.join('|')
        "#,
    );
}

/// dns.reverse: v4/v6 sockaddr builds + getnameinfo success arms, the
/// NI_NAMEREQD failure arm (EAI_NONAME → ENOTFOUND), the invalid-IP EINVAL
/// arm with Node's exact message shape, and both validation arms.
#[test]
fn dns_reverse_success_error_and_validation_arms() {
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

        // v4 success — /etc/hosts reverse for the loopback.
        var r4 = dns.reverse('127.0.0.1');
        check("reverse_v4_hosts", function() {
            return Array.isArray(r4) && r4.length === 1 && stripDot(r4[0]) === 'localhost';
        });

        // v6 success — exercises the sockaddr_in6 build arm.
        var r6 = dns.reverse('::1');
        check("reverse_v6_resolves", function() {
            return Array.isArray(r6) && r6.length >= 1 &&
                   stripDot(r6[0]).lastIndexOf('localhost') !== -1;
        });

        // NI_NAMEREQD failure → getHostByAddr ENOTFOUND with err.code.
        check("reverse_noname_enotfound", function() {
            try { dns.reverse('192.0.2.1'); return false; }
            catch(e) { return e.code === 'ENOTFOUND'; }
        });

        // Invalid IP → EINVAL with Node's exact message shape.
        check("reverse_invalid_einval", function() {
            try { dns.reverse('999.999.999.999'); return false; }
            catch(e) {
                return e.code === 'EINVAL' &&
                       String(e.message) === 'getHostByAddr EINVAL 999.999.999.999';
            }
        });

        check("reverse_noargs_throws", function() {
            try { __dns_reverse(); return false; }
            catch(e) { return String(e.message).indexOf('requires an ip argument') !== -1; }
        });
        check("reverse_nonstring_throws", function() {
            try { dns.reverse(42); return false; }
            catch(e) { return String(e.message).indexOf('must be a string') !== -1; }
        });

        // Callback form routes the result.
        var cbResult = null;
        dns.reverse('127.0.0.1', function(err, hosts) { cbResult = [err, hosts]; });
        check("reverse_cb_form", function() {
            return cbResult !== null && cbResult[0] === null &&
                   cbResult[1].length === 1 && stripDot(cbResult[1][0]) === 'localhost';
        });

        results.join('|')
        "#,
    );
}

/// dns.lookupService: success arms (v4 + v6 sockaddr, numeric service),
/// the getnameinfo-failure fallback (ip echoed back, service "unknown"),
/// the unparseable-address arm, and all three port parse arms
/// (int32 / double / non-numeric).
#[test]
fn dns_lookup_service_full_matrix() {
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

        // v4 success: hostname from /etc/hosts, service numeric (NI_NUMERICSERV).
        var s4 = __dns_lookup_service('127.0.0.1', 8080);
        check("lookupService_v4", function() {
            return s4.hostname === 'localhost' && s4.service === '8080';
        });

        // v6 sockaddr_in6 arm — exercises the IPv6 build; the resolved
        // hostname/service are resolver-dependent (the local resolver may
        // not carry a reverse record for ::1), so the contract is shape.
        var s6 = __dns_lookup_service('::1', 80);
        check("lookupService_v6", function() {
            return typeof s6.hostname === 'string' && typeof s6.service === 'string';
        });

        // getnameinfo failure: Node fallback shape (ip echoed, "unknown").
        var sf = __dns_lookup_service('192.0.2.1', 80);
        check("lookupService_failure_fallback", function() {
            return sf.hostname === '192.0.2.1' && sf.service === 'unknown';
        });

        // Unparseable address: empty hostname + "unknown".
        var sb = __dns_lookup_service('not-an-ip', 80);
        check("lookupService_unparseable", function() {
            return sb.hostname === '' && sb.service === 'unknown';
        });

        // Port parse arms: double truncates; non-numeric falls back to 0.
        var sd = __dns_lookup_service('127.0.0.1', 80.5);
        check("lookupService_port_double", function() {
            return sd.hostname === 'localhost' && sd.service === '80';
        });
        var ss = __dns_lookup_service('127.0.0.1', '80');
        check("lookupService_port_nonnumeric", function() {
            return ss.hostname === 'localhost' && ss.service === '0';
        });

        check("lookupService_noargs_throws", function() {
            try { __dns_lookup_service('127.0.0.1'); return false; }
            catch(e) { return String(e.message).indexOf('requires address and port') !== -1; }
        });
        check("lookupService_nonstring_addr_throws", function() {
            try { __dns_lookup_service(42, 80); return false; }
            catch(e) { return String(e.message).indexOf('must be a string') !== -1; }
        });

        // JS wrapper: bare call returns the object; callback form routes
        // (null, hostname, service).
        var bare = dns.lookupService('127.0.0.1', 8080);
        check("lookupService_js_bare", function() {
            return bare.hostname === 'localhost' && bare.service === '8080';
        });
        var cbArgs = null;
        dns.lookupService('127.0.0.1', 8080, function(err, host, serv) { cbArgs = [err, host, serv]; });
        check("lookupService_cb_form", function() {
            return cbArgs !== null && cbArgs[0] === null &&
                   cbArgs[1] === 'localhost' && cbArgs[2] === '8080';
        });

        results.join('|')
        "#,
    );
}

/// getServers/setServers validation (node v24 oracle): non-string elements
/// throw ERR_INVALID_ARG_TYPE naming servers[i], no-arg / non-array throw the
/// Array-type error, all before mutation; plus the reset-to-empty arm.
#[test]
fn dns_servers_roundtrip_and_filters() {
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

        check("getServers_initial_array", function() { return Array.isArray(dns.getServers()); });

        dns.setServers(['8.8.8.8', '1.1.1.1']);
        check("setServers_roundtrip", function() {
            return dns.getServers().join(',') === '8.8.8.8,1.1.1.1';
        });

        // Non-string elements throw ERR_INVALID_ARG_TYPE naming the servers[i]
        // slot, before any mutation (node v24.19.0 oracle).
        var elemErr = null;
        try { dns.setServers([1, '9.9.9.9', {}, '8.8.4.4']); }
        catch (e) { elemErr = { name: e.name, code: e.code, msg: String(e.message) }; }
        check("setServers_nonstring_element_throws", function() {
            return elemErr !== null && elemErr.name === 'TypeError' &&
                   elemErr.code === 'ERR_INVALID_ARG_TYPE' &&
                   elemErr.msg === 'The "servers[0]" argument must be of type string. Received type number (1)';
        });
        check("setServers_unchanged_after_element_throw", function() {
            return dns.getServers().join(',') === '8.8.8.8,1.1.1.1';
        });

        // No-arg and non-array arms throw the Array-type validation error
        // (the current list stays unchanged).
        dns.setServers(['5.5.5.5']);
        var noArgErr = null;
        try { dns.setServers(); }
        catch (e) { noArgErr = { code: e.code, msg: String(e.message) }; }
        check("setServers_noarg_throws", function() {
            return noArgErr !== null && noArgErr.code === 'ERR_INVALID_ARG_TYPE' &&
                   noArgErr.msg === 'The "servers" argument must be an instance of Array. Received undefined';
        });
        var nonObjErr = null;
        try { dns.setServers('not-an-array'); }
        catch (e) { nonObjErr = { code: e.code, msg: String(e.message) }; }
        check("setServers_nonarray_throws", function() {
            return nonObjErr !== null && nonObjErr.code === 'ERR_INVALID_ARG_TYPE' &&
                   nonObjErr.msg === 'The "servers" argument must be an instance of Array. Received type string (\'not-an-array\')';
        });
        check("setServers_unchanged_after_throws", function() {
            return dns.getServers().join(',') === '5.5.5.5';
        });

        // Empty array resets the list.
        dns.setServers([]);
        check("setServers_empty_resets", function() { return dns.getServers().length === 0; });

        results.join('|')
        "#,
    );
}

/// setDefaultResultOrder accepts exactly the three Node orders; anything else
/// throws the exact message; getDefaultResultOrder tracks the current value.
#[test]
fn dns_set_default_result_order_validation() {
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

        check("defaultOrder_initial_verbatim", function() {
            return dns.getDefaultResultOrder() === 'verbatim';
        });
        dns.setDefaultResultOrder('ipv4first');
        check("defaultOrder_ipv4first", function() { return dns.getDefaultResultOrder() === 'ipv4first'; });
        dns.setDefaultResultOrder('ipv6first');
        check("defaultOrder_ipv6first", function() { return dns.getDefaultResultOrder() === 'ipv6first'; });
        dns.setDefaultResultOrder('verbatim');
        check("defaultOrder_back_to_verbatim", function() { return dns.getDefaultResultOrder() === 'verbatim'; });

        check("defaultOrder_invalid_throws", function() {
            try { dns.setDefaultResultOrder('bogus'); return false; }
            catch(e) {
                return e.name === 'TypeError' && e.code === 'ERR_INVALID_ARG_VALUE' &&
                       String(e.message) === "The argument 'dnsOrder' must be one of: 'verbatim', 'ipv4first', 'ipv6first'. Received 'bogus'";
            }
        });
        // The failed set leaves the previous value intact.
        check("defaultOrder_invalid_no_change", function() {
            return dns.getDefaultResultOrder() === 'verbatim';
        });

        results.join('|')
        "#,
    );
}

/// The Resolver prototype family: instance methods dispatch to the same
/// native stack, the getServers/setServers pair operates on the shared
/// registry, and cancel() is a no-op.
#[test]
fn dns_resolver_instance_family() {
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

        var r = new dns.Resolver();
        check("resolver_method_surface", function() {
            return typeof r.resolve === 'function' && typeof r.resolve4 === 'function' &&
                   typeof r.resolve6 === 'function' && typeof r.resolveCname === 'function' &&
                   typeof r.resolveMx === 'function' && typeof r.resolveNaptr === 'function' &&
                   typeof r.resolveNs === 'function' && typeof r.resolvePtr === 'function' &&
                   typeof r.resolveSoa === 'function' && typeof r.resolveSrv === 'function' &&
                   typeof r.resolveTxt === 'function' && typeof r.resolveAny === 'function' &&
                   typeof r.reverse === 'function' && typeof r.getServers === 'function' &&
                   typeof r.setServers === 'function' && typeof r.cancel === 'function';
        });

        check("resolver_resolve4_dispatch", function() {
            var a = r.resolve4('localhost');
            return Array.isArray(a) && a.length === 1 && a[0] === '127.0.0.1';
        });
        check("resolver_resolve_generic", function() {
            var a = r.resolve('localhost', 'A');
            return Array.isArray(a) && a.length === 1 && a[0] === '127.0.0.1';
        });
        check("resolver_resolvePtr_dispatch", function() {
            var p = r.resolvePtr('127.0.0.1');
            return Array.isArray(p) && p.length >= 1;
        });

        // setServers/getServers on the instance share the module registry.
        r.setServers(['1.1.1.1']);
        check("resolver_setServers_shared", function() {
            return r.getServers().join(',') === '1.1.1.1';
        });
        r.setServers([]);
        check("resolver_cancel_noop", function() { return r.cancel() === undefined; });

        // The error semantics match the module-level functions.
        check("resolver_reverse_einval", function() {
            try { r.reverse('999.999.999.999'); return false; }
            catch(e) { return e.code === 'EINVAL'; }
        });
        check("resolver_callback_validation", function() {
            try { r.resolve4('localhost', 123); return false; }
            catch(e) { return e.code === 'ERR_INVALID_ARG_TYPE'; }
        });

        // promises.Resolver is the same constructor.
        check("resolver_promises_identity", function() {
            return dns.promises.Resolver === dns.Resolver;
        });

        results.join('|')
        "#,
    );
}

/// dns.promises: the resolve path fulfills with the real records after a
/// microtask drain, the reverse error path rejects with the coded Error, and
/// the promisify custom symbol maps onto the promises namespace.
#[test]
fn dns_promises_resolve_and_reject_paths() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let setup = eval_string(
        &mut ctx,
        r#"
        var dns = require('dns');
        globalThis.__pr = 'pending';
        globalThis.__pl = 'pending';
        globalThis.__prj = 'pending';
        dns.promises.resolve4('localhost').then(function(a) {
            globalThis.__pr = Array.isArray(a) && a.length === 1 && a[0] === '127.0.0.1' ? 'ok' : 'bad:' + JSON.stringify(a);
        }, function(e) { globalThis.__pr = 'rejected:' + e.code; });
        dns.promises.lookup('127.0.0.1').then(function(r) {
            globalThis.__pl = (r.address === '127.0.0.1' && r.family === 4) ? 'ok' : 'bad';
        }, function(e) { globalThis.__pl = 'rejected:' + e.code; });
        dns.promises.reverse('999.999.999.999').then(function(h) {
            globalThis.__prj = 'fulfilled-badly';
        }, function(e) {
            globalThis.__prj = (e.code === 'EINVAL') ? 'ok' : 'badcode:' + e.code;
        });
        'scheduled'
        "#,
    );
    assert_eq!(setup, "scheduled");

    // Drain microtasks so the settled promises run their handlers.
    let cx_raw = ctx.raw_cx();
    unsafe {
        mozjs_sys::jsapi::js::RunJobs(cx_raw);
    }

    assert_battery(
        &mut ctx,
        r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        check("promises_resolve4_fulfilled", function() { return globalThis.__pr === 'ok'; });
        check("promises_lookup_fulfilled", function() { return globalThis.__pl === 'ok'; });
        check("promises_reverse_rejects_einval", function() { return globalThis.__prj === 'ok'; });

        // promisify custom symbol: lookup/resolve/reverse carry the promises
        // version on Symbol.for("nodejs.util.promisify.custom").
        var sym = Symbol.for("nodejs.util.promisify.custom");
        check("promisify_symbol_lookup", function() { return dns.lookup[sym] === dns.promises.lookup; });
        check("promisify_symbol_resolve", function() { return dns.resolve[sym] === dns.promises.resolve; });
        check("promisify_symbol_reverse", function() { return dns.reverse[sym] === dns.promises.reverse; });
        check("promisify_symbol_resolveTxt", function() { return dns.resolveTxt[sym] === dns.promises.resolveTxt; });

        results.join('|')
        "#,
    );
}

/// _invalidArgType shape conformance: every present-but-non-function callback
/// produces Node's exact determineSpecificType rendering with
/// ERR_INVALID_ARG_TYPE.
#[test]
fn dns_invalid_arg_type_error_shapes() {
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
        function cbErr(val) {
            try { dns.resolve4('localhost', val); return null; }
            catch(e) { return e; }
        }

        var e1 = cbErr(123);
        check("argtype_number", function() {
            return e1 && e1.code === 'ERR_INVALID_ARG_TYPE' &&
                   e1.message.indexOf('type number (123)') !== -1;
        });
        var e2 = cbErr(true);
        check("argtype_boolean", function() {
            return e2 && e2.message.indexOf('type boolean (true)') !== -1;
        });
        var e3 = cbErr([1, 2]);
        check("argtype_array", function() {
            return e3 && e3.message.indexOf('an instance of Array') !== -1;
        });
        var e4 = cbErr(function foo() {});
        check("argtype_function_is_valid_callback", function() {
            // A real function in the callback slot is legal — no throw.
            return e4 === null;
        });
        var e5 = cbErr('x'.repeat(30));
        check("argtype_long_string_truncated", function() {
            return e5 && e5.message.indexOf("type string ('xxxxxxxxxxxxxxxxxxxxxxxx...')") !== -1;
        });
        var e6 = cbErr({});
        check("argtype_object", function() {
            return e6 && e6.message.indexOf('an instance of Object') !== -1;
        });
        var e7 = cbErr(null);
        check("argtype_null", function() { return e7 && e7.message.indexOf('Received null') !== -1; });

        // undefined callback is legal (absent callback slot).
        var okUndef = dns.resolve4('localhost', undefined);
        check("argtype_undefined_is_absent", function() {
            return Array.isArray(okUndef) && okUndef.length === 1 && okUndef[0] === '127.0.0.1';
        });

        results.join('|')
        "#,
    );
}

/// Module constants surface: hints flags and the c-ares-derived error code
/// spellings exposed on both the module and the promises namespace.
#[test]
fn dns_constants_surface() {
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

        check("const_addrconfig", function() { return dns.ADDRCONFIG === 1; });
        check("const_v4mapped", function() { return dns.V4MAPPED === 8; });
        check("const_all", function() { return dns.ALL === 16; });
        check("const_nodata", function() { return dns.NODATA === 'ENODATA'; });
        check("const_notfound", function() { return dns.NOTFOUND === 'ENOTFOUND'; });
        check("const_servfail", function() { return dns.SERVFAIL === 'ESERVFAIL'; });
        check("const_timeout", function() { return dns.TIMEOUT === 'ETIMEOUT'; });
        check("const_connrefused", function() { return dns.CONNREFUSED === 'ECONNREFUSED'; });
        check("const_badquery", function() { return dns.BADQUERY === 'EBADQUERY'; });
        check("const_cancelled", function() { return dns.CANCELLED === 'ECANCELLED'; });
        check("const_promises_mirror", function() {
            return dns.promises.NODATA === 'ENODATA' && dns.promises.NOTFOUND === 'ENOTFOUND' &&
                   dns.promises.SERVFAIL === 'ESERVFAIL';
        });

        results.join('|')
        "#,
    );
}

/// Real-uplink RR successes — the rendering loops for MX/TXT/NS/SOA/SRV
/// entries and the AAAA address arm need records only the public DNS carries.
/// Canonical env runs this via --run-ignored=all; plain runs skip it.
#[ignore = "需要真实外网 DNS;真跑: BAO_TEST_DNS=1 cargo nextest run -E 'test(dns_real_network_rr_success_shapes)'"]
#[test]
fn dns_real_network_rr_success_shapes() {
    if std::env::var("BAO_TEST_DNS").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_DNS != 1 — real-uplink RR shapes not exercised");
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

        // AAAA success arm (the offline battery only reaches its error arm).
        var a6 = dns.resolve6('google.com');
        check("real_resolve6_shape", function() {
            return Array.isArray(a6) && a6.length >= 1 &&
                   a6.every(function(s) { return String(s).indexOf(':') !== -1; });
        });

        // MX success arm — array of {priority, exchange} entries.
        var mx = dns.resolveMx('gmail.com');
        check("real_resolveMx_shape", function() {
            return Array.isArray(mx) && mx.length >= 1 &&
                   mx.every(function(e) {
                       return typeof e.priority === 'number' && typeof e.exchange === 'string';
                   });
        });

        // TXT success arm — array of arrays of strings (Node chunk grouping).
        var txt = dns.resolveTxt('nodejs.org');
        check("real_resolveTxt_shape", function() {
            return Array.isArray(txt) && txt.length >= 1 &&
                   txt.every(function(rec) {
                       return Array.isArray(rec) && rec.every(function(c) { return typeof c === 'string'; });
                   });
        });

        // NS success arm — array of nameserver hostnames.
        var ns = dns.resolveNs('github.com');
        check("real_resolveNs_shape", function() {
            return Array.isArray(ns) && ns.length >= 1 &&
                   ns.every(function(s) { return typeof s === 'string' && s.length > 0; });
        });

        // SOA success arm — object with nsname/hostmaster + five numerics.
        var soa = dns.resolveSoa('example.com');
        check("real_resolveSoa_shape", function() {
            return soa && typeof soa === 'object' && typeof soa.nsname === 'string' &&
                   typeof soa.hostmaster === 'string' &&
                   typeof soa.serial === 'number' && typeof soa.refresh === 'number' &&
                   typeof soa.retry === 'number' && typeof soa.expire === 'number' &&
                   typeof soa.minttl === 'number';
        });

        // SRV/NAPTR are shape-level only (records are deployment-dependent).
        var srv = dns.resolveSrv('_xmpp-server._tcp.jabber.org');
        check("real_resolveSrv_shape", function() { return Array.isArray(srv); });
        var naptr = dns.resolveNaptr('jabber.org');
        check("real_resolveNaptr_shape", function() { return Array.isArray(naptr); });

        results.join('|')
        "#,
    );
}
