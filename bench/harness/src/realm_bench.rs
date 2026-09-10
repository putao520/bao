//! Bench ②: realm create / first-script / drop latency at the engine layer.
//!
//! `JsContext::for_test()` returns a fresh context wrapper over the thread's
//! shared SM Runtime; the realm global is created lazily on the first eval
//! (realm-per-context model). Each iteration therefore measures: wrapper
//! creation (cheap) + realm global creation + first script (the real realm
//! cost — the closest analog to a fresh page's first evaluate, per
//! `src/bao_engine/benches/evaluate_roundtrip.rs`) + wrapper drop (realm
//! global unrooted, reclaimable by GC).

use std::time::Instant;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

use crate::common::{Metric, Params, ResultBuilder};

pub fn run(p: &Params) -> Result<ResultBuilder, String> {
    let iterations = p.usize_of("iterations", 100);
    let warmup = p.usize_of("warmup", 5);
    let budget_ms = p.f64_of("budget-ms", 120_000.0);

    let mut b = ResultBuilder::new("realm-create-drop");
    b.param("iterations", iterations.into());
    b.param("warmup", warmup.into());
    b.param("budget_ms", budget_ms.into());

    // Cold: process-first for_test includes JSEngine + Runtime init.
    let t0 = Instant::now();
    let mut ctx = JsContext::for_test()
        .map_err(|e| format!("cold for_test failed: {}", e.message))?;
    let engine_init_ms = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = Instant::now();
    let v = ctx
        .eval("1+1", "<bench>")
        .map_err(|e| format!("cold eval failed: {}", e.message))?;
    let cold_eval_ms = t1.elapsed().as_secs_f64() * 1e3;
    match v {
        JsValue::Number(n) if n == 2.0 => {}
        other => return Err(format!("cold: expected 2.0, got {other:?}")),
    }
    drop(ctx);
    b.metric(Metric::single(
        "engine_and_runtime_init",
        "ms",
        "latency",
        false,
        Some("cold"),
        engine_init_ms,
    ));
    b.metric(Metric::single(
        "realm_first_script_cold",
        "ms",
        "latency",
        false,
        Some("cold"),
        cold_eval_ms,
    ));

    let mut wrapper_us: Vec<f64> = Vec::new();
    let mut realm_eval_us: Vec<f64> = Vec::new();
    let mut cycle_us: Vec<f64> = Vec::new();

    let start = Instant::now();
    let total = iterations + warmup;
    let mut executed = 0usize;
    let mut truncated = false;
    for i in 0..total {
        if start.elapsed().as_millis() as f64 > budget_ms {
            truncated = true;
            break;
        }
        executed += 1;

        let t0 = Instant::now();
        let mut ctx = JsContext::for_test()
            .map_err(|e| format!("iter {i}: for_test failed: {}", e.message))?;
        let t_wrap = t0.elapsed().as_secs_f64() * 1e6;

        let t1 = Instant::now();
        let v = ctx
            .eval("1+1", "<bench>")
            .map_err(|e| format!("iter {i}: eval failed: {}", e.message))?;
        let t_eval = t1.elapsed().as_secs_f64() * 1e6;
        match v {
            JsValue::Number(n) if n == 2.0 => {}
            other => return Err(format!("iter {i}: expected 2.0, got {other:?}")),
        }

        let t2 = Instant::now();
        drop(ctx);
        let t_drop = t2.elapsed().as_secs_f64() * 1e6;

        let cycle = t_wrap + t_eval + t_drop;
        if i >= warmup {
            wrapper_us.push(t_wrap);
            realm_eval_us.push(t_eval);
            cycle_us.push(cycle);
        }
    }

    if truncated {
        b.note(format!(
            "wall-clock budget hit after {executed}/{total} iterations — warm sample count is the honest n"
        ));
    }
    if realm_eval_us.is_empty() {
        return Err(format!(
            "no warm samples collected (executed {executed}, warmup {warmup})"
        ));
    }

    b.param("executed_iterations", executed.into());
    b.param("warm_samples", realm_eval_us.len().into());
    b.metric(Metric::from_samples(
        "realm_wrapper_create",
        "us",
        "latency",
        false,
        Some("warm"),
        &wrapper_us,
    ));
    b.metric(Metric::from_samples(
        "realm_create_first_script",
        "us",
        "latency",
        false,
        Some("warm"),
        &realm_eval_us,
    ));
    b.metric(Metric::from_samples(
        "realm_cycle",
        "us",
        "latency",
        false,
        Some("warm"),
        &cycle_us,
    ));
    b.note("realm = lazily-created global object per JsContext wrapper over the shared thread SM Runtime (realm-per-context model); SM JSContext itself is not destroyed per iteration");
    b.note("dropped realm globals are unrooted and reclaimed by GC — accumulation pressure during the run is honest GC workload, not a leak verdict");
    Ok(b)
}
