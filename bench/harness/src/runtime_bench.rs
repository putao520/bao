//! Bench ①: `bun_runtime::BaoRuntime` create / first-eval / drop latency.
//!
//! Each iteration builds the FULL production runtime (env aliasing layer,
//! resolver bridge, Node/Bun globals setup hook, JsContext + SmRuntimeGuard)
//! and tears it down — the per-iteration cost of what `bao run` pays per
//! process, isolated in-process. Drop of the guard destroys the JSContext
//! (JS_DestroyContext + CONTEXT TLS take; the process JSEngine singleton
//! survives — mozjs fork patch #2 recovers AlreadyInitialized on re-init).

use std::time::Instant;

use bao_engine::value::JsValue;

use crate::common::{Metric, Params, ResultBuilder};

pub fn run(p: &Params) -> Result<ResultBuilder, String> {
    let iterations = p.usize_of("iterations", 50);
    let warmup = p.usize_of("warmup", 3);
    let budget_ms = p.f64_of("budget-ms", 120_000.0);

    let mut b = ResultBuilder::new("runtime-create-drop");
    b.param("iterations", iterations.into());
    b.param("warmup", warmup.into());
    b.param("budget_ms", budget_ms.into());
    b.param("eval_source", "1+1".into());

    let mut create_ms: Vec<f64> = Vec::new();
    let mut first_eval_ms: Vec<f64> = Vec::new();
    let mut drop_ms: Vec<f64> = Vec::new();
    let mut cycle_ms: Vec<f64> = Vec::new();

    let start = Instant::now();
    // iteration 0 = cold (process-first: JSEngine init included), recorded
    // separately; 1..=warmup discarded; the rest are the warm sample set.
    let total_iters = iterations + warmup + 1;
    let mut executed = 0usize;
    let mut truncated = false;
    for i in 0..total_iters {
        if start.elapsed().as_millis() as f64 > budget_ms {
            truncated = true;
            break;
        }
        executed += 1;

        let t0 = Instant::now();
        let mut rt = bun_runtime::BaoRuntime::new()
            .map_err(|e| format!("iter {i}: BaoRuntime::new failed: {}", e.message))?;
        let t_create = t0.elapsed().as_secs_f64() * 1e3;

        let t1 = Instant::now();
        let v = rt
            .eval("1+1", "<bench>")
            .map_err(|e| format!("iter {i}: first eval failed: {}", e.message))?;
        let t_eval = t1.elapsed().as_secs_f64() * 1e3;
        match v {
            JsValue::Number(n) if n == 2.0 => {}
            other => return Err(format!("iter {i}: expected 1+1=2, got {other:?}")),
        }

        let t2 = Instant::now();
        drop(rt);
        let t_drop = t2.elapsed().as_secs_f64() * 1e3;

        let cycle = t_create + t_eval + t_drop;
        if i == 0 {
            b.metric(Metric::single(
                "runtime_create_cold",
                "ms",
                "latency",
                false,
                Some("cold"),
                t_create,
            ));
            b.metric(Metric::single(
                "runtime_first_eval_cold",
                "ms",
                "latency",
                false,
                Some("cold"),
                t_eval,
            ));
            b.metric(Metric::single(
                "runtime_drop_cold",
                "ms",
                "latency",
                false,
                Some("cold"),
                t_drop,
            ));
            b.metric(Metric::single(
                "runtime_cycle_cold",
                "ms",
                "latency",
                false,
                Some("cold"),
                cycle,
            ));
        } else if i > warmup {
            create_ms.push(t_create);
            first_eval_ms.push(t_eval);
            drop_ms.push(t_drop);
            cycle_ms.push(cycle);
        }
    }

    if truncated {
        b.note(format!(
            "wall-clock budget hit after {executed}/{total_iters} iterations — warm sample count is the honest n"
        ));
    }
    if create_ms.is_empty() {
        return Err(format!(
            "no warm samples collected (executed {executed} iterations, warmup {warmup})"
        ));
    }

    b.param("executed_iterations", executed.into());
    b.param("warm_samples", create_ms.len().into());
    b.metric(Metric::from_samples(
        "runtime_create",
        "ms",
        "latency",
        false,
        Some("warm"),
        &create_ms,
    ));
    b.metric(Metric::from_samples(
        "runtime_first_eval",
        "ms",
        "latency",
        false,
        Some("warm"),
        &first_eval_ms,
    ));
    b.metric(Metric::from_samples(
        "runtime_drop",
        "ms",
        "latency",
        false,
        Some("warm"),
        &drop_ms,
    ));
    b.metric(Metric::from_samples(
        "runtime_cycle",
        "ms",
        "latency",
        false,
        Some("warm"),
        &cycle_ms,
    ));
    let ops = create_ms.len() as f64 / (start.elapsed().as_secs_f64());
    b.metric(Metric::single(
        "runtime_cycles_per_s_wall",
        "ops_per_s",
        "throughput",
        true,
        None,
        ops,
    ));
    b.note("warm first_eval includes lazy realm-global creation + install_all Node/Bun globals (per-realm cost, not per-eval)");
    b.note("drop = SmRuntimeGuard drop (JS_DestroyContext + CONTEXT TLS take); process JSEngine singleton survives across iterations");
    Ok(b)
}
