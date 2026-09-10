// @trace TEST-ENG-001-REALMPOLICY [req:REQ-ENG-001] [level:integration]
//
// SM-EVOLUTION #28 — engine-native realm policy reachability proofs.
//
// Locks down, with live engine evidence, the two realm-policy dimensions the
// 2026-09-10 census found Rust-reachable TODAY with zero binding work:
//
//   1. Timezone: `RealmCreationOptions::forceUTC_` is a pub bindgen field
//      (creation-time only, per-realm). Engine semantics = Firefox RFP shape:
//      all Date local methods run in UTC+0 (SM maps it to the real IANA zone
//      Atlantic/Reykjavik internally). Proven by evaluating Date expressions
//      inside a realm created with the flag flipped — deterministic offset 0
//      at both a January and a July instant regardless of host TZ.
//
//   2. Time precision: `JS::SetTimeResolutionUsec(resolution, jitter)` is
//      already bound (jsapi SetTimeResolutionUsec) and gates `Date.now()` /
//      `new Date().getTime()` clamping per `RealmBehaviors::clampAndJitterTime_`
//      (C++ default true; mozjs glue preserves it). Proven by clamping to a
//      1-second grid with jitter off and observing millisecond multiples of
//      1000, then restoring resolution 0 (no clamping) and observing raw
//      wall-clock values again.
//
// Deliberately NOT covered here (binding gap, recorded in the ledger):
// locale — `JS_SetDefaultLocale` is public C++ API but not bindgen-exposed
// because mozjs-sys' bindgen shim (`src/jsapi.cpp`) does not include
// `js/LocaleSensitive.h`; landing that include regenerates jsapi.rs and
// rebuilds every dependent crate, so it belongs to the dedicated wiring wave,
// not this test-only slice.
//
// Engine policy tests are process-global where noted: SetTimeResolutionUsec
// writes a process-wide static, so the clamp is restored BEFORE any assertion
// can fail (plain `cargo test` runs the whole suite in one process; nextest
// isolates per test anyway).

use bao_engine::context::JsContext;
use mozjs::jsapi::OnNewGlobalHookOption;
use mozjs::jsapi::SetTimeResolutionUsec;
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_NewGlobalObject;
use mozjs::rust::{CompileOptionsWrapper, SIMPLE_GLOBAL_CLASS, evaluate_script};

/// Evaluate `source` in a fresh realm created with `forceUTC_ = true` and
/// return the script result as a bool (scripts used with this helper always
/// end in a `===` comparison, so the result is a real boolean).
fn eval_bool_in_force_utc_realm(
    cx: &mut mozjs::context::JSContext,
    source: &str,
    filename: &str,
) -> bool {
    let mut options = bun_sm::node_realm_options();
    options.creationOptions_.forceUTC_ = true;

    rooted!(&in(cx) let global = unsafe {
        JS_NewGlobalObject(
            cx,
            &SIMPLE_GLOBAL_CLASS,
            std::ptr::null_mut(),
            OnNewGlobalHookOption::FireOnNewGlobalHook,
            &*options,
        )
    });
    assert!(
        !global.get().is_null(),
        "forceUTC realm global creation must succeed"
    );

    let c_filename = std::ffi::CString::new(filename)
        .unwrap_or_else(|_| std::ffi::CString::new("<realm-policy>").unwrap());
    let compile_opts = CompileOptionsWrapper::new(cx, c_filename, 1);
    rooted!(&in(cx) let mut rval = UndefinedValue());

    {
        let mut realm = AutoRealm::new_from_handle(cx, global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        let result = evaluate_script(
            realm_cx,
            global.handle(),
            source,
            rval.handle_mut(),
            compile_opts,
        );
        assert!(
            result.is_ok(),
            "evaluating in forceUTC realm must not error: {:?}",
            result.err()
        );
    }

    let jsval = unsafe { bun_sm::value::jsval_to_jsvalue(cx.raw_cx_no_gc(), rval.get()) };
    jsval
        .as_bool()
        .expect("script result must be a boolean === comparison")
}

fn test_force_utc_realm_dates_run_in_utc(ctx: &mut JsContext) {
    let mut cx = ctx.cx();

    // Offset 0 at a January instant (2023-01-01T00:00:00Z) — under host TZ
    // this is only 0 by coincidence; under forceUTC it is 0 by construction.
    assert!(
        eval_bool_in_force_utc_realm(
            &mut cx,
            "new Date(1672531200000).getTimezoneOffset() === 0",
            "force_utc_jan.js",
        ),
        "forceUTC realm must report UTC offset 0 (January instant)"
    );

    // Offset 0 at a July instant too — proves no DST leakage from the host
    // zone (a Northern-Hemisphere host would shift by its DST delta here).
    assert!(
        eval_bool_in_force_utc_realm(
            &mut cx,
            "new Date(1688169600000).getTimezoneOffset() === 0",
            "force_utc_jul.js",
        ),
        "forceUTC realm must report UTC offset 0 (July instant, no DST leak)"
    );

    // The override is a creation option on THIS realm only: the context's own
    // persistent realm (created lazily below with plain node semantics) is a
    // different realm and must keep host-derived behavior — asserting exact
    // host offsets is impossible portably, so the contract locked here is
    // that evaluating in the persistent realm still works and reports a
    // finite offset (either 0 for a UTC host or the host's real offset).
    let offset = ctx
        .eval(
            "new Date(1672531200000).getTimezoneOffset()",
            "host_offset.js",
        )
        .expect("plain realm eval must succeed");
    let offset = offset
        .as_number()
        .expect("getTimezoneOffset must be a number");
    assert!(
        offset.is_finite(),
        "host realm offset must be finite, got {}",
        offset
    );
}

fn test_time_resolution_clamp_and_restore(ctx: &mut JsContext) {
    // Clamp Date.now() to a 1-second grid, jitter OFF (jitter randomizes the
    // sub-grid midpoint and would make the multiple-of assertion flaky).
    unsafe { SetTimeResolutionUsec(1_000_000, false) };

    let clamped = ctx
        .eval("Date.now()", "time_resolution_clamped.js")
        .expect("clamped Date.now() eval must succeed");

    // Restore BEFORE asserting: a failed assertion below must not leak the
    // process-wide clamp into later tests when they share one process.
    unsafe { SetTimeResolutionUsec(0, false) };

    let clamped = clamped
        .as_number()
        .expect("Date.now() must be a number");
    assert!(
        clamped % 1000.0 == 0.0,
        "1s resolution clamp must quantize Date.now() to whole seconds, got {}",
        clamped
    );

    // After restoring resolution 0 the engine stops quantizing: raw
    // wall-clock epoch (sub-second digits return). A raw value COULD land on
    // a second boundary by chance, so the post-restore contract is "plausible
    // unclamped epoch", not "not a multiple".
    let raw = ctx
        .eval("Date.now()", "time_resolution_restored.js")
        .expect("restored Date.now() eval must succeed");
    let raw = raw.as_number().expect("Date.now() must be a number");
    assert!(
        raw >= clamped && raw > 1_700_000_000_000.0 && raw < 3_000_000_000_000.0,
        "restored Date.now() must be a plausible epoch >= the clamped sample: {} vs {}",
        raw,
        clamped
    );
}

#[test]
fn test_realm_policy_all() {
    let mut ctx = JsContext::for_test().expect("Failed to create JsContext");

    test_force_utc_realm_dates_run_in_utc(&mut ctx);
    test_time_resolution_clamp_and_restore(&mut ctx);

    bao_engine::context::JsContext::shutdown_thread_sm();
}
