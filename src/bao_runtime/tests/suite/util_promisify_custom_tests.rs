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
#[path = "common/mod.rs"]
mod common;

use common::setup_ctx;

use common::eval_string_full as eval_string;


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

/// Upstream oven-sh/bun c5a68b0594 (timers: store [util.promisify.custom] as
/// a per-function accessor) — bao-shape test lock. The JSC inline-cache
/// cross-contamination that commit fixes cannot exist in SpiderMonkey, but
/// the Node-parity surface it pins is assertable here: each timer function
/// carries its OWN getter-only accessor (enumerable, non-configurable) whose
/// getter ignores the receiver, and a strict-mode write / Object.assign copy
/// is rejected with a TypeError — the old value-stamp (writable data
/// property) accepted both.
#[test]
fn promisify_custom_timer_accessor_shape() {
    let mut ctx = setup_ctx();
    let verdict = eval_string(
        &mut ctx,
        r#"
        var tp = require('timers/promises');
        var custom = Symbol.for('nodejs.util.promisify.custom');
        var out = [];
        // Descriptor shape: enumerable, non-configurable, getter-only.
        var d = Object.getOwnPropertyDescriptor(setTimeout, custom);
        out.push('descriptor:' + (d && d.enumerable === true && d.configurable === false &&
            typeof d.get === 'function' && d.set === undefined));
        // Own value per timer, identity with the promise forms.
        out.push('own_value:' + (setTimeout[custom] === tp.setTimeout &&
            setImmediate[custom] === tp.setImmediate &&
            setTimeout[custom] !== setImmediate[custom]));
        // Getter ignores the receiver (Reflect.get with a foreign receiver,
        // and prototype-chain lookup through Object.create).
        out.push('receiver_ignored:' + (Reflect.get(setTimeout, custom, {}) === tp.setTimeout &&
            Object.create(setInterval)[custom] === tp.setInterval));
        // Object.assign copies through [[Set]] — getter-only must reject.
        try { var src = {}; src[custom] = 1; Object.assign(setTimeout, src);
              out.push('assign:accepted'); }
        catch (e) { out.push('assign:' + (e instanceof TypeError ? 'throws' : 'wrong:' + e)); }
        // Strict-mode assignment throws; the property is unchanged after.
        var before = setTimeout[custom];
        try { (function() { 'use strict'; setTimeout[custom] = 1; })();
              out.push('strict_write:accepted'); }
        catch (e) { out.push('strict_write:' + (e instanceof TypeError ? 'throws' : 'wrong:' + e)); }
        out.push('unchanged:' + (setTimeout[custom] === before && before === tp.setTimeout));
        out.join('|')
    "#,
    );
    assert_eq!(
        verdict,
        "descriptor:true|own_value:true|receiver_ignored:true|assign:throws|strict_write:throws|unchanged:true",
        "timer promisify.custom accessor shape diverged from Node (c5a68b0594): {}",
        verdict
    );
    bun_runtime::shutdown_thread_sm();
}
