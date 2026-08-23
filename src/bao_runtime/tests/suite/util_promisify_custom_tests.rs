// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// util.promisify custom-symbol contract tests — the domain-check a1f2e22140
// (own-idiom fix) regression.
//
// Pre-fix wedge: the util_promisify factory never read
// `Symbol.for('nodejs.util.promisify.custom')`, so every custom contract was
// swallowed by the generic (err, value) wrapper:
//   - dns's 15 stamped symbols (node_dns lookup/lookupService/resolve*/
//     family) were dead wiring — promisify(dns.lookup) resolved with the
//     BARE address string instead of dns.promises.lookup's
//     { address, family } shape (family dropped);
//   - the global timer functions had no custom at all, so
//     promisify(setTimeout) could never equal timers/promises.setTimeout.
//
// Fix under test: the factory probes the custom symbol first (node_util
// factory region) and the timer globals get a lazy custom getter whose value
// IS the cached `timers/promises` function object (timers.rs wiring) —
// identity, not a wrapper.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Production-shaped pump (fs_async_callback_tests precedent) — the dns
/// lookup promise resolves synchronously underneath, but its `.then`
/// continuation is a job that needs one drain pass to run.
fn pump_until_quiescent(ctx: &mut JsContext, deadline_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    while std::time::Instant::now() < deadline {
        let mut cxm = ctx.cx();
        if !bun_runtime::timers::drain_and_check(&mut cxm) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Identity contract: promisify returns the custom function VERBATIM.
///   - timers: promisify(setTimeout) === require('timers/promises').setTimeout
///     (and setInterval/setImmediate) — the lazy getter in timers.rs resolves
///     to the same cached function object require hands out;
///   - dns: promisify(dns.lookup) === dns.promises.lookup (and friends) —
///     the symbols node_dns stamps become live wiring.
#[test]
fn promisify_custom_identity_timers_and_dns() {
    let mut ctx = setup_ctx();
    let verdict = eval_string(
        &mut ctx,
        r#"
        var util = require('util');
        var tp = require('timers/promises');
        var dns = require('dns');
        [
            // Discriminator: if this is false the builtin cache hands out
            // distinct objects per require and NO stamp could ever satisfy
            // identity — root cause would be the require cache, not wiring.
            'req_cache:' + (require('timers/promises') === tp),
            'settimeout:' + (util.promisify(setTimeout) === tp.setTimeout),
            'setinterval:' + (util.promisify(setInterval) === tp.setInterval),
            'setimmediate:' + (util.promisify(setImmediate) === tp.setImmediate),
            'dns_lookup:' + (util.promisify(dns.lookup) === dns.promises.lookup),
            'dns_lookupservice:' + (util.promisify(dns.lookupService) === dns.promises.lookupService),
            'dns_resolve:' + (util.promisify(dns.resolve) === dns.promises.resolve),
            'dns_resolve4:' + (util.promisify(dns.resolve4) === dns.promises.resolve4),
            'dns_resolve6:' + (util.promisify(dns.resolve6) === dns.promises.resolve6),
            'dns_reverse:' + (util.promisify(dns.reverse) === dns.promises.reverse),
            'dns_resolvetxt:' + (util.promisify(dns.resolveTxt) === dns.promises.resolveTxt),
        ].join('|')
    "#,
    );
    assert_eq!(
        verdict,
        "req_cache:true|settimeout:true|setinterval:true|setimmediate:true|dns_lookup:true|\
         dns_lookupservice:true|dns_resolve:true|dns_resolve4:true|dns_resolve6:true|\
         dns_reverse:true|dns_resolvetxt:true",
        "promisify custom-symbol identity broken"
    );
    bun_runtime::shutdown_thread_sm();
}

/// Shape contract: promisify(dns.lookup)('localhost') resolves with the
/// dns.promises.lookup shape { address, family } — NOT the generic wrapper's
/// bare address string (which would drop family). The concrete assertion
/// pins the exact (address, family) pairs getaddrinfo returns for localhost.
#[test]
fn promisify_dns_lookup_keeps_address_family_shape() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var util = require('util');
        var dns = require('dns');
        globalThis.__lk = { state: 'pending' };
        util.promisify(dns.lookup)('localhost').then(
            function(res) {
                globalThis.__lk.state = 'done';
                globalThis.__lk.kind = typeof res;
                globalThis.__lk.address = (res && res.address) === undefined ? 'missing' : String(res.address);
                globalThis.__lk.family = (res && res.family) === undefined ? 'missing' : String(res.family);
            },
            function(err) {
                globalThis.__lk.state = 'rejected:' + String((err && err.message) || err);
            }
        );
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    let verdict = eval_string(
        &mut ctx,
        r#"
        var lk = globalThis.__lk;
        var pairOk = (lk.address === '127.0.0.1' && lk.family === '4') ||
                     (lk.address === '::1' && lk.family === '6');
        [
            'state:' + lk.state,
            'kind:' + (lk.kind || 'unset'),
            'shape:' + (lk.state === 'done' && lk.kind === 'object' && pairOk
                ? 'address-family-intact'
                : 'addr<' + lk.address + '>family<' + lk.family + '>'),
        ].join('|')
    "#,
    );
    assert!(
        verdict.starts_with("state:done|kind:object|shape:address-family-intact"),
        "promisify(dns.lookup) lost the {{address, family}} shape: {}",
        verdict
    );
    bun_runtime::shutdown_thread_sm();
}

/// Generic path still wraps (err, value): a plain callback function without
/// a custom symbol promisifies to a Promise resolving with the single value.
/// Guards against the custom probe breaking the fallback wrapper (the
/// pbkdf2 pump test also exercises this path end-to-end).
#[test]
fn promisify_generic_wrapper_still_resolves_single_value() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var util = require('util');
        globalThis.__gw = { state: 'pending', value: null };
        util.promisify(function(cb) { cb(null, 42); })().then(
            function(v) { globalThis.__gw.state = 'done'; globalThis.__gw.value = v; },
            function(e) { globalThis.__gw.state = 'rejected:' + String(e); }
        );
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    let verdict = eval_string(
        &mut ctx,
        r#"
        var gw = globalThis.__gw;
        'state:' + gw.state + '|value:' + (gw.value === 42 ? 'forty-two' : String(gw.value))
    "#,
    );
    assert_eq!(
        verdict, "state:done|value:forty-two",
        "generic promisify wrapper broken by the custom probe"
    );
    bun_runtime::shutdown_thread_sm();
}
