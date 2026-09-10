//! Bench ⑥: stencil-cost — SM-EVOLUTION #26 judgment bench.
//!
//! Question (ledger completion clause 6: "Stencil/cache 只有在 benchmark 证明
//! 收益后保留"): how much of today's per-realm script injection cost is
//! parse+bytecode-compile, and how much of it would a compile-once /
//! instantiate-per-realm Stencil cache eliminate?
//!
//! Current path fact (verified in the vendored SM source,
//! `vm/CompilationAndEvaluation.cpp` `EvaluateSourceBuffer`): every
//! `JS::Evaluate` — which is what `JsContext::eval` reaches through
//! `mozjs::rust::evaluate_script` — runs `frontend::CompileGlobalScript` on
//! EVERY call (`setIsRunOnce(true)`; this SM snapshot has no eval cache).
//! The production surface that pays it per realm: the stealth combined JS
//! blob (`inject_js_hooks` runs inside `install_stealth_props`, i.e. once per
//! page realm AND once per worker/SW realm), CDP-injected scripts, and any
//! user script compiled in a fresh realm.
//!
//! Payloads (realm-agnostic — the stealth blob is typeof-guarded for worker
//! realms, so it executes cleanly in a bare test realm):
//! - `stealth`      — the verbatim `StealthHooks::from_profile(firefox_default
//!                    ).combined_js()` production per-realm blob
//! - `stealth_x10`  — 10 concatenated copies + a marker assignment (synthetic
//!                    scaling point; marker gives a hard execution check)
//! - `tiny_1p1`     — `"1+1"` floor reference
//!
//! Phases per payload (fresh realm per iteration, warmup dropped):
//! - A1 `current_first_eval`  — `ctx.eval(payload)` on a brand-new context
//!   (realm global creation + compile + execute + RunJobs: the production
//!   first-injection shape, timed as a whole)
//! - A2 `current_warm_realm_eval` — realm pre-created UNTIMED via
//!   `ensure_realm_global`, then timed `ctx.eval(payload)` = pure
//!   compile+execute on an established realm (the repeated-compile cost a
//!   stencil cache would remove)
//! - B  `stencil_compile_only` — timed `JS::CompileGlobalScriptToStencil`
//!   (parse+frontend, no instantiate/execute) + untimed `StencilRelease`, on
//!   one warm realm; the first compile is reported cold, separately
//! - C  `stencil_instantiate_exec` — one stencil compiled UNTIMED up front,
//!   then per fresh realm (pre-created untimed): timed
//!   `JS::InstantiateGlobalStencil` + `JS_ExecuteScript` + `RunJobs`
//!   (the #26 candidate's per-realm cost)
//!
//! Derived judgment metrics (phase="derived", from warm p50s):
//! - `compile_share_of_warm_eval_pct` = (A2−C)/A2 ×100 — how much of the
//!   per-realm injection a stencil cache would eliminate
//! - `stencil_speedup_x` = A2/C
//! - `breakeven_realms` = B_p50/(A2_p50−C_p50) — realm-injections needed to
//!   amortize the one-time stencil compile
//!
//! Verification is fail-closed: payload eval must return Ok (the same success
//! criterion production `inject_js_hooks` uses), `tiny` must yield 2.0, the
//! x10 payload must leave its marker set (probed untimed after the timed
//! window), and every C-phase `JS_ExecuteScript` must return true.

use std::ffi::CString;
use std::ptr;
use std::time::Instant;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use mozjs::jsapi::{self, DelazificationOption, InstantiateOptions};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::{transform_str_to_source_text, wrappers2, CompileOptionsWrapper};

use crate::common::{Metric, Params, ResultBuilder};

/// The production stealth blob: the exact source `inject_js_hooks` evaluates
/// in every page/worker realm (typeof-guarded, bare-realm safe).
fn stealth_payload() -> String {
    let profile = bao_stealth::StealthProfile::firefox_default();
    let hooks = bao_stealth::StealthHooks::from_profile(
        &profile.canvas,
        &profile.audio,
        &profile.navigator,
        &profile.screen,
        &profile.webgl,
        &profile.font,
        &profile.battery,
        profile.webrtc_mode,
        &profile.timing,
        &profile.clientrects,
        &profile.screen_display,
        &profile.plugin,
        &profile.speech,
        &profile.media_devices,
        &profile.permissions,
        &profile.webgl_context,
        &profile.connection,
        &profile.iframe,
    );
    hooks.combined_js()
}

/// C++ defaults (js/public/CompileOptions.h `InstantiateOptions`): all-false
/// + OnDemandOnly — mirrors what a plain `JS::Evaluate` compile would use.
fn default_instantiate_options() -> InstantiateOptions {
    InstantiateOptions {
        skipFilenameValidation: false,
        hideScriptFromDebugger: false,
        deferDebugMetadata: false,
        eagerDelazificationStrategy_: DelazificationOption::OnDemandOnly,
    }
}

/// Establish this context's realm global (untimed helper — mirrors what
/// `JsContext::eval` does internally on first use) and return the global
/// pointer (the same pointer `ensure_realm_global` hands to production).
fn ensure_realm(
    ctx: &mut JsContext,
    label: &str,
) -> Result<*mut mozjs::jsapi::JSObject, String> {
    let mut cx = ctx.cx();
    ctx.ensure_realm_global(&mut cx, None)
        .map_err(|e| format!("{label}: realm init failed: {}", e.message))
}

/// Compile `src` to a stencil inside the given realm. Caller owns the
/// returned pointer and must `mozjs::jsapi::StencilRelease` it. Stencils are
/// realm-independent (JSStencil.h: "may be instantiated into any Realm on
/// the current runtime and may be used multiple times").
///
/// # Safety
/// `global_ptr` must be a live global of a realm on this context.
unsafe fn compile_stencil(
    cx: &mut mozjs::context::JSContext,
    global_ptr: *mut mozjs::jsapi::JSObject,
    src: &str,
    label: &str,
) -> Result<*mut jsapi::Stencil, String> {
    unsafe {
        rooted!(&in(cx) let global = global_ptr);
        let mut realm = AutoRealm::new_from_handle(cx, global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        let filename = CString::new("<bench-stencil>").unwrap();
        let opts = CompileOptionsWrapper::new(realm_cx, filename, 1);
        let mut source = transform_str_to_source_text(src);
        let addrefed =
            wrappers2::CompileGlobalScriptToStencil(realm_cx, opts.ptr, &mut source);
        if addrefed.mRawPtr.is_null() {
            return Err(format!("{label}: CompileGlobalScriptToStencil returned null"));
        }
        Ok(addrefed.mRawPtr)
    }
}

/// # Safety
/// `stencil` must be a live stencil compiled from `src`-shaped global code.
unsafe fn instantiate_and_execute(
    cx: &mut mozjs::context::JSContext,
    global_ptr: *mut mozjs::jsapi::JSObject,
    stencil: *mut jsapi::Stencil,
    inst_opts: &InstantiateOptions,
    label: &str,
) -> Result<(), String> {
    unsafe {
        rooted!(&in(cx) let global = global_ptr);
        let mut realm = AutoRealm::new_from_handle(cx, global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        rooted!(&in(realm_cx) let script = wrappers2::InstantiateGlobalStencil(
            realm_cx,
            inst_opts as *const InstantiateOptions,
            stencil,
            ptr::null_mut(), // InstantiationStorage is an optional param (JSStencil.h)
        ));
        if script.get().is_null() {
            return Err(format!("{label}: InstantiateGlobalStencil returned null"));
        }
        rooted!(&in(realm_cx) let mut rval = mozjs::jsval::UndefinedValue());
        if !wrappers2::JS_ExecuteScript(realm_cx, script.handle(), rval.handle_mut()) {
            return Err(format!("{label}: JS_ExecuteScript failed"));
        }
        // Shape parity with JsContext::eval (which drains the job queue inside
        // its own timed window).
        mozjs::jsapi::js::RunJobs(realm_cx.raw_cx());
        Ok(())
    }
}

/// Untimed post-run probe on an established realm: proves the realm is
/// functional and (for marker payloads) that execution really happened.
fn probe_eval(ctx: &mut JsContext, expr: &str, expected: f64, label: &str) -> Result<(), String> {
    match ctx.eval(expr, "<bench-probe>") {
        Ok(JsValue::Number(n)) if n == expected => Ok(()),
        other => Err(format!("{label}: probe '{expr}' expected {expected}, got {other:?}")),
    }
}

fn p50(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    crate::common::stats(&sorted).p50
}

pub fn run(p: &Params) -> Result<ResultBuilder, String> {
    let iterations = p.usize_of("iterations", 60);
    let warmup = p.usize_of("warmup", 5);
    let budget_ms = p.f64_of("budget-ms", 90_000.0);

    let stealth = stealth_payload();
    let stealth_x10 = {
        let mut s = String::with_capacity(stealth.len() * 10 + 64);
        for _ in 0..10 {
            s.push_str(&stealth);
            s.push('\n');
        }
        // Synthetic payload carries its own execution marker (probed untimed
        // after every timed window — hard verification in all phases).
        s.push_str("globalThis.__bao_bench_x10_marker = 7;");
        s
    };
    let payloads: Vec<(&str, String, bool)> = vec![
        ("tiny_1p1", "1+1".to_string(), false),
        ("stealth", stealth, false),
        ("stealth_x10", stealth_x10, true),
    ];

    let mut b = ResultBuilder::new("stencil-cost");
    b.param("iterations", iterations.into());
    b.param("warmup", warmup.into());
    b.param("budget_ms_per_phase", budget_ms.into());
    b.note("engine-direct bench (no servo/browser); same realm-per-context model as realm-create-drop; JS::Evaluate in this SM snapshot re-compiles on every call (no eval cache) — A2 is pure repeated-compile cost");
    b.note("C-phase InstantiationStorage is passed null (JSStencil.h optional param; avoids the un-bound non-trivial C++ destructor)");

    // ── Engine init (cold single sample) + sanity ───────────────────────────
    let t0 = Instant::now();
    let mut ctx0 = JsContext::for_test()
        .map_err(|e| format!("init for_test failed: {}", e.message))?;
    let engine_init_ms = t0.elapsed().as_secs_f64() * 1e3;
    match ctx0.eval("1+1", "<bench>") {
        Ok(JsValue::Number(n)) if n == 2.0 => {}
        other => return Err(format!("init sanity: expected 2.0, got {other:?}")),
    }
    b.metric(Metric::single(
        "engine_and_runtime_init",
        "ms",
        "latency",
        false,
        Some("cold"),
        engine_init_ms,
    ));
    drop(ctx0);

    for (key, src, has_marker) in &payloads {
        b.param(&format!("{key}_bytes"), serde_json::json!(src.len()));
        run_payload(&mut b, key, src, *has_marker, iterations, warmup, budget_ms)?;
    }
    Ok(b)
}

fn run_payload(
    b: &mut ResultBuilder,
    key: &str,
    src: &str,
    has_marker: bool,
    iterations: usize,
    warmup: usize,
    budget_ms: f64,
) -> Result<(), String> {
    // ── Phase A1: first eval per fresh realm (realm+compile+exec, timed) ───
    let a1 = fresh_realm_eval_phase(
        b, src, key, "current_first_eval",
        iterations, warmup, budget_ms, has_marker, true,
    )?;

    // ── Phase A2: warm-realm eval (compile+exec only — realm untimed) ──────
    let a2 = fresh_realm_eval_phase(
        b, src, key, "current_warm_realm_eval",
        iterations, warmup, budget_ms, has_marker, false,
    )?;

    // ── Phase B: stencil compile-only on one warm realm ────────────────────
    let (b_cold_us, b_warm_us) = {
        let mut ctx = JsContext::for_test()
            .map_err(|e| format!("{key} B: for_test failed: {}", e.message))?;
        let global_ptr = ensure_realm(&mut ctx, &format!("{key} B"))?;
        let mut cx = ctx.cx();

        // Cold single compile.
        let t0 = Instant::now();
        let stencil = unsafe { compile_stencil(&mut cx, global_ptr, src, key)? };
        let cold_us = t0.elapsed().as_secs_f64() * 1e6;
        unsafe { jsapi::StencilRelease(stencil) };

        let mut warm: Vec<f64> = Vec::with_capacity(iterations);
        let start = Instant::now();
        let mut truncated = false;
        for i in 0..(iterations + warmup) {
            if start.elapsed().as_millis() as f64 > budget_ms {
                truncated = true;
                break;
            }
            let t0 = Instant::now();
            let st = unsafe { compile_stencil(&mut cx, global_ptr, src, key)? };
            let dt = t0.elapsed().as_secs_f64() * 1e6;
            unsafe { jsapi::StencilRelease(st) }; // release untimed
            if i >= warmup {
                warm.push(dt);
            }
        }
        if truncated {
            b.note(format!("{key} B: wall-clock budget hit — warm n is the honest sample count"));
        }
        (cold_us, warm)
    };
    if b_warm_us.is_empty() {
        return Err(format!("{key} B: no warm samples"));
    }
    b.metric(Metric::single(
        &format!("{key}.stencil_compile_cold"),
        "us",
        "latency",
        false,
        Some("cold"),
        b_cold_us,
    ));
    b.metric(Metric::from_samples(
        &format!("{key}.stencil_compile_only"),
        "us",
        "latency",
        false,
        Some("warm"),
        &b_warm_us,
    ));

    // ── Phase C: compile once (untimed), instantiate+execute per realm ─────
    let c_warm_us = {
        let mut owner = JsContext::for_test()
            .map_err(|e| format!("{key} C owner: for_test failed: {}", e.message))?;
        let owner_global = ensure_realm(&mut owner, &format!("{key} C owner"))?;
        let mut ocx = owner.cx();
        // Stencil compiled once, untimed (the cache-hit steady state).
        let stencil = unsafe { compile_stencil(&mut ocx, owner_global, src, key)? };
        let inst_opts = default_instantiate_options();

        let mut warm: Vec<f64> = Vec::with_capacity(iterations);
        let start = Instant::now();
        let mut truncated = false;
        let mut executed = 0usize;
        for i in 0..(iterations + warmup) {
            if start.elapsed().as_millis() as f64 > budget_ms {
                truncated = true;
                break;
            }
            executed += 1;

            let mut ctx = JsContext::for_test()
                .map_err(|e| format!("{key} C iter {i}: for_test failed: {}", e.message))?;
            let global_ptr = ensure_realm(&mut ctx, &format!("{key} C iter {i}"))?;
            let mut cx = ctx.cx();

            // Timed window: realm already established (untimed above).
            let t0 = Instant::now();
            unsafe {
                instantiate_and_execute(&mut cx, global_ptr, stencil, &inst_opts, &format!("{key} C iter {i}"))?;
            }
            let dt = t0.elapsed().as_secs_f64() * 1e6;

            if i >= warmup {
                warm.push(dt);
            }
            // Untimed verification probe on the same realm.
            if has_marker {
                probe_eval(&mut ctx, "__bao_bench_x10_marker", 7.0, &format!("{key} C iter {i}"))?;
            } else {
                probe_eval(&mut ctx, "1+1", 2.0, &format!("{key} C iter {i}"))?;
            }
            drop(ctx);
        }
        let _ = executed;
        if truncated {
            b.note(format!("{key} C: wall-clock budget hit — warm n is the honest sample count"));
        }
        unsafe { jsapi::StencilRelease(stencil) };
        warm
    };
    if c_warm_us.is_empty() {
        return Err(format!("{key} C: no warm samples"));
    }
    b.metric(Metric::from_samples(
        &format!("{key}.stencil_instantiate_exec"),
        "us",
        "latency",
        false,
        Some("warm"),
        &c_warm_us,
    ));

    // ── Derived judgment metrics (from warm p50s) ──────────────────────────
    let a1_p50 = p50(&a1);
    let a2_p50 = p50(&a2);
    let b_p50 = p50(&b_warm_us);
    let c_p50 = p50(&c_warm_us);
    let saved = a2_p50 - c_p50;
    b.metric(Metric::single(
        &format!("{key}.compile_share_of_warm_eval_pct"),
        "percent",
        "ratio",
        false,
        Some("derived"),
        (saved / a2_p50) * 100.0,
    ));
    b.metric(Metric::single(
        &format!("{key}.stencil_speedup_x"),
        "x",
        "ratio",
        true,
        Some("derived"),
        a2_p50 / c_p50,
    ));
    b.metric(Metric::single(
        &format!("{key}.breakeven_realms"),
        "count",
        "ratio",
        false,
        Some("derived"),
        b_p50 / saved,
    ));
    b.note(format!(
        "{key}: A1(realm+compile+exec) p50={a1_p50:.1}us A2(compile+exec) p50={a2_p50:.1}us B(stencil compile) p50={b_p50:.1}us C(instantiate+exec) p50={c_p50:.1}us → stencil saves {saved:.1}us/realm"
    ));
    Ok(())
}

/// One fresh-realm-per-iteration eval phase. `timed_realm_creation=true` →
/// the whole `ctx.eval` (realm+compile+exec) is timed (phase A1); `false` →
/// the realm is established untimed first and only the payload eval is timed
/// (phase A2). Returns the warm samples (already reported as a metric).
fn fresh_realm_eval_phase(
    b: &mut ResultBuilder,
    src: &str,
    key: &str,
    phase: &str,
    iterations: usize,
    warmup: usize,
    budget_ms: f64,
    has_marker: bool,
    timed_realm_creation: bool,
) -> Result<Vec<f64>, String> {
    let mut warm_us: Vec<f64> = Vec::with_capacity(iterations);
    let start = Instant::now();
    let mut truncated = false;
    let mut executed = 0usize;
    for i in 0..(iterations + warmup) {
        if start.elapsed().as_millis() as f64 > budget_ms {
            truncated = true;
            break;
        }
        executed += 1;
        let mut ctx = JsContext::for_test()
            .map_err(|e| format!("{key} {phase} iter {i}: for_test failed: {}", e.message))?;
        if !timed_realm_creation {
            ensure_realm(&mut ctx, &format!("{key} {phase} iter {i}"))?;
        }
        let t0 = Instant::now();
        let v = ctx.eval(src, "<bench>").map_err(|e| {
            format!("{key} {phase} iter {i}: payload eval failed: {} (bare-realm execution of the verbatim stealth blob must succeed — same criterion as production inject_js_hooks)", e.message)
        })?;
        let dt = t0.elapsed().as_secs_f64() * 1e6;

        // Verification (untimed).
        if key == "tiny_1p1" {
            match v {
                JsValue::Number(n) if n == 2.0 => {}
                other => return Err(format!("{key} {phase} iter {i}: expected 2.0, got {other:?}")),
            }
        }
        if has_marker {
            probe_eval(&mut ctx, "__bao_bench_x10_marker", 7.0, &format!("{key} {phase} iter {i}"))?;
        } else {
            probe_eval(&mut ctx, "1+1", 2.0, &format!("{key} {phase} iter {i}"))?;
        }

        if i >= warmup {
            warm_us.push(dt);
        }
    }
    if truncated {
        b.note(format!(
            "{key} {phase}: wall-clock budget hit after {executed} iters — warm n is the honest sample count"
        ));
    }
    if warm_us.is_empty() {
        return Err(format!("{key} {phase}: no warm samples (executed {executed})"));
    }
    b.metric(Metric::from_samples(
        &format!("{key}.{phase}"),
        "us",
        "latency",
        false,
        Some("warm"),
        &warm_us,
    ));
    Ok(warm_us)
}
