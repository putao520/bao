// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:16] [level:integration]
// Shadow-axis probes — page-realm bao-installed globals that have no
// servo-native counterpart (setImmediate) or shadow one (queueMicrotask,
// crypto). Provenance (e44 09cabe17 boundary note, 2026-09-10): the page
// fetch/timer axes were fixed and probed by fetch_axis_probe_tests.rs; three
// shadows remained unprobed. This file characterizes them live.
//
//   I axis — setImmediate: the ONE bao timer native still installed on servo
//           page realms (no servo-native counterpart; timers.rs
//           install_timer_globals defines it unconditionally, outside the
//           defined_bao_timers gate that keeps WebIDL setTimeout on pages).
//           Callbacks register into BAO_REGISTRY at deadline 0 and dispatch
//           through pump_embedder_thread (drain_bao_timers) — i.e. only when
//           a message wakes the ScriptThread; the lazily-materialized
//           MiniEventLoop is never self-ticked on this thread. Live verdict
//           recorded either way: fires under the message pump (GREEN),
//           with the chained microtask flushed by the pump's RunJobs step,
//           and the no-message-window behavior captured from timestamps.
//           clearImmediate must actually cancel (BAO_REGISTRY remove).
//   Q axis — queueMicrotask: bao's shadow (web_api.rs install_queue_microtask,
//           defined unconditionally on the page global) routes the callback
//           through CallOriginalPromiseResolve + CallOriginalPromiseThen —
//           the real SM microtask queue, FIFO with promise reactions. The
//           probe pins the canonical ordering: promise reactions and
//           queueMicrotask callbacks interleave in enqueue order, nested
//           microtasks append before the macrotask timer fires. Any
//           macrotask-ish shadow (MiniEventLoop dispatch) would reorder.
//           Spec TypeError parity (e59 ②): a non-callable argument throws
//           a real TypeError naming queueMicrotask/function.
//   C axis — crypto: install_crypto_global puts a plain object on the page
//           global (BoringSSL CSPRNG randomUUID / getRandomValues), then
//           install_crypto_subtle (tail of install_web_apis, via
//           install_fetch_classes) upgrades the SAME subtle object with a
//           Promise-returning digest over the real hashers. Probe pins
//           functional fidelity: getRandomValues fills and returns the same
//           array, randomUUID matches the v4 shape and differs across calls,
//           digest('SHA-256') resolves to the NIST "abc" vector. Object
//           class-name identity (e59 ③): Object.prototype.toString.call(crypto)
//           must be '[object Crypto]' (WebIDL [Symbol.toStringTag], not the
//           plain '[object Object]').
//
// Environment gating: real servo rendering requires DISPLAY (Xvfb). No
// network fixture is used.
//
// Usage:
//   xvfb-run cargo nt -p bao-browser -E 'test(shadow_axis_probe)'

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle};

fn should_skip() -> bool {
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return true;
    }
    false
}

/// Drive servo's ScriptThread with no-op evaluates while polling a page-side
/// sink variable (the same pump pattern every live harness here uses).
fn poll_sink(page: &PageHandle, sink: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(v) = page.evaluate_js_web(&format!("window.{sink} === null ? '' : String(window.{sink})")) {
            let t = v.trim().trim_matches('"').to_string();
            if !t.is_empty() && t != "null" && t != "undefined" {
                return Some(t);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// I axis — page-realm `setImmediate` settlement + cancellation.
///
/// Arms two immediates: one live (records a fire timestamp and chains a
/// microtask onto itself — the pump's RunJobs step must flush it), one
/// cleared synchronously via clearImmediate (must never run). After a
/// no-message park, the first timestamped read discriminates "fired by this
/// read's own pump wake" (stamp == read-now) from "servo self-woke the
/// thread" (stamp < read-now); either verdict satisfies the axis, the
/// deterministic assertion is fires-under-pump.
#[test]
fn shadow_axis_probe_i_set_immediate_page_realm() {
    if !common::run_isolated("shadow_axis_probe_tests::shadow_axis_probe_i_set_immediate_page_realm") {
        return;
    }
    if should_skip() {
        return;
    }
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("data:text/html;charset=utf-8,<html><body>shadow</body></html>".into()),
            ..Default::default()
        })
        .expect("create_page");

    let armed = page
        .evaluate_js_web(
            "(function() { \
             window.__ir = null; window.__clr = null; window.__t0 = null; \
             window.__t0 = Date.now(); \
             if (typeof setImmediate !== 'function') { return 'NO-FN:' + typeof setImmediate; } \
             setImmediate(function() { \
               window.__ir = 'fired@' + Date.now(); \
               Promise.resolve().then(function() { window.__ir += '+mt'; }); \
             }); \
             var h = setImmediate(function() { window.__clr = 'CLEARED-HANDLER-RAN'; }); \
             clearImmediate(h); \
             return 'armed'; })()",
        )
        .expect("setImmediate arm");
    assert!(armed.contains("armed"), "setImmediate arm failed: {armed:?}");

    // No-message window: park WITHOUT driving the page, then take ONE
    // timestamped read. The read itself wakes the thread, so `fired@now`
    // equal to the read's `now` means the pump dispatched it on THIS wake;
    // an earlier stamp means servo delivered a wakeup on its own.
    std::thread::sleep(Duration::from_millis(400));
    let first_state = page
        .evaluate_js_web(
            "JSON.stringify({t0:window.__t0, ir:window.__ir, clr:window.__clr, now:Date.now()})",
        )
        .expect("no-message-window read");
    eprintln!("[i-setimmediate] no-message-window state={first_state}");

    let fired = poll_sink(&page, "__ir", Duration::from_secs(10));
    eprintln!("[i-setimmediate] fired={fired:?}");
    match fired {
        Some(v) => {
            assert!(
                v.starts_with("fired@") && v.ends_with("+mt"),
                "I axis: setImmediate fired but the chained microtask is missing from the \
                 verdict (pump RunJobs step not flushing page-realm reactions): {v:?}"
            );
        },
        None => panic!(
            "I axis BLACK: page-realm setImmediate never fired even under the message pump \
             (fire-time realm resolution dropped the callback — registration-time global \
             capture + AutoRealm dispatch is the timers.rs fix)"
        ),
    }

    let cleared = poll_sink(&page, "__clr", Duration::from_secs(2));
    eprintln!("[i-setimmediate] cleared={cleared:?}");
    assert!(
        cleared.is_none(),
        "I axis: clearImmediate did not cancel — the cleared handler ran: {cleared:?}"
    );
}

/// Q axis — page-realm microtask ordering under the bao queueMicrotask
/// shadow. Canonical spec order: promise reactions and queueMicrotask
/// callbacks drain FIFO at the script-end job checkpoint; a nested
/// queueMicrotask appends before the macrotask timer task runs.
#[test]
fn shadow_axis_probe_q_queue_microtask_order() {
    if !common::run_isolated("shadow_axis_probe_tests::shadow_axis_probe_q_queue_microtask_order") {
        return;
    }
    if should_skip() {
        return;
    }
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("data:text/html;charset=utf-8,<html><body>shadow</body></html>".into()),
            ..Default::default()
        })
        .expect("create_page");

    let armed = page
        .evaluate_js_web(
            "(function() { \
             window.__mq = null; window.__qarg = 'unobserved'; \
             try { queueMicrotask(42); } catch (e) { window.__qarg = 'threw:' + e; } \
             var o = []; \
             Promise.resolve().then(function() { o.push('p1'); }); \
             queueMicrotask(function() { o.push('q1'); \
               queueMicrotask(function() { o.push('q1b'); }); }); \
             queueMicrotask(function() { o.push('q2'); }); \
             Promise.resolve().then(function() { o.push('p2'); }); \
             setTimeout(function() { window.__mq = o.join(',') + '|t'; }, 30); \
             return 'armed'; })()",
        )
        .expect("queueMicrotask arm");
    assert!(armed.contains("armed"), "queueMicrotask arm failed: {armed:?}");

    // Spec TypeError parity (e59 ②): a non-callable argument (42) must throw
    // a REAL TypeError whose message names queueMicrotask/function — the old
    // shadow silently ignored non-object args.
    let qarg = page.evaluate_js_web("String(window.__qarg)").ok();
    eprintln!("[q-microtask] non-function-arg verdict={qarg:?}");
    let qarg = qarg.unwrap_or_default();
    assert!(
        qarg.contains("threw:TypeError")
            && qarg.contains("queueMicrotask")
            && qarg.contains("function"),
        "Q axis: queueMicrotask(42) must throw a TypeError naming queueMicrotask/function \
         (spec: non-callable argument throws TypeError; a silent ignore is a \
         typeof-probe-detectable divergence), got: {qarg:?}"
    );

    let order = poll_sink(&page, "__mq", Duration::from_secs(10));
    eprintln!("[q-microtask] order={order:?}");
    assert_eq!(
        order.as_deref(),
        Some("p1,q1,q2,p2,q1b|t"),
        "Q axis BLACK: page-realm microtask ordering deviates from FIFO spec semantics — \
         promise reactions and queueMicrotask must interleave in enqueue order and nested \
         microtasks must append before the macrotask timer fires (a macrotask-dispatched \
         queueMicrotask shadow reorders)"
    );
}

/// C axis — page-realm crypto shadow functional fidelity: getRandomValues
/// fills and returns the same array, randomUUID is v4-shaped and fresh per
/// call, subtle.digest resolves to the NIST SHA-256("abc") vector.
#[test]
fn shadow_axis_probe_c_crypto_page_realm() {
    if !common::run_isolated("shadow_axis_probe_tests::shadow_axis_probe_c_crypto_page_realm") {
        return;
    }
    if should_skip() {
        return;
    }
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("data:text/html;charset=utf-8,<html><body>shadow</body></html>".into()),
            ..Default::default()
        })
        .expect("create_page");

    let sent = page
        .evaluate_js_web(
            "(function() { \
             window.__cr = null; \
             try { \
               var kind = Object.prototype.toString.call(crypto); \
               var arr = new Uint8Array(16); \
               var ret = crypto.getRandomValues(arr); \
               var g = (ret === arr) && arr.some(function(v) { return v !== 0; }); \
               var u1 = crypto.randomUUID(); var u2 = crypto.randomUUID(); \
               var re = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/; \
               var u = re.test(u1) && re.test(u2) && u1 !== u2; \
               crypto.subtle.digest('SHA-256', new Uint8Array([97, 98, 99])).then(function(d) { \
                 var b = new Uint8Array(d); \
                 var hex = ''; \
                 for (var i = 0; i < b.length; i++) hex += ('0' + b[i].toString(16)).slice(-2); \
                 window.__cr = 'g=' + g + '|u=' + u + '|len=' + b.length + '|hex=' + hex + \
                   '|kind=' + kind + '|isP=' + (d instanceof ArrayBuffer); \
               }, function(e) { \
                 window.__cr = 'DIGEST-REJ:' + ((e && e.message) || e); \
               }); \
               return 'sent:' + kind; \
             } catch (e) { window.__cr = 'THREW:' + ((e && e.message) || e); return 'threw'; } })()",
        )
        .expect("crypto probe dispatch");
    assert!(
        sent.contains("sent"),
        "crypto probe threw at dispatch: {sent:?}"
    );

    let settled = poll_sink(&page, "__cr", Duration::from_secs(15));
    eprintln!("[c-crypto] settled={settled:?}");
    let v = match settled {
        Some(v) => v.trim_matches('"').to_string(),
        None => panic!(
            "C axis BLACK: page-realm crypto probe never settled (digest promise hung — \
             pump RunJobs not flushing subtle reactions)"
        ),
    };
    assert!(
        !v.starts_with("THREW:") && !v.starts_with("DIGEST-REJ:"),
        "C axis: crypto probe errored live: {v}"
    );
    assert!(
        v.starts_with("g=true|u=true|"),
        "C axis: getRandomValues (same-array, filled) / randomUUID (v4 shape, fresh) \
         fidelity failed: {v}"
    );
    // Class-name fidelity (e59 ③): Object.prototype.toString.call(crypto) must
    // be '[object Crypto]' (WebIDL [Symbol.toStringTag]); a bare plain object
    // stringifies as '[object Object]' — probe-detectable against native realms.
    assert!(
        v.contains("|kind=[object Crypto]"),
        "C axis: crypto must stringify as '[object Crypto]' (WebIDL interface class name; \
         a plain object is '[object Object]'), got: {v}"
    );
    assert!(
        v.contains("|len=32|hex=ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad|"),
        "C axis: subtle.digest('SHA-256') known-answer failed (NIST 'abc' vector): {v}"
    );
}
