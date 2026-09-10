//! Bench ⑤: RSS three-phase sampling on a live `bun_runtime::BaoRuntime`.
//!
//! Phases: idle (post-init steady state) → synchronous JS allocation churn
//! (blocking eval; allocation pressure with periodic drops) → post (idle
//! again — reclamation trend). A background thread samples VmRSS / VmHWM /
//! fd / thread counts for the whole run. Seed data for issue #19 G-section
//! budgets: the slope numbers are observations, not pass/fail — monotonic
//! growth must be attributable before any gate consumes them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bao_engine::value::JsValue;

use crate::common::{self, Metric, Params, ResultBuilder};

struct Probe {
    t_ms: f64,
    rss_kib: f64,
    hwm_kib: f64,
    fd: f64,
    threads: f64,
}

fn slope_per_s(probes: &[&Probe]) -> f64 {
    if probes.len() < 2 {
        return 0.0;
    }
    let first = probes[0];
    let last = probes[probes.len() - 1];
    let dt_s = (last.t_ms - first.t_ms) / 1000.0;
    if dt_s <= 0.0 {
        return 0.0;
    }
    (last.rss_kib - first.rss_kib) / dt_s
}

fn phase_rss(probes: &[&Probe]) -> Vec<f64> {
    probes.iter().map(|p| p.rss_kib).collect()
}

pub fn run(p: &Params) -> Result<ResultBuilder, String> {
    let idle_secs = p.u64_of("idle-secs", 3);
    let churn_secs = p.u64_of("churn-secs", 8);
    let post_secs = p.u64_of("post-secs", 3);
    let interval_ms = p.u64_of("interval-ms", 200);

    let mut b = ResultBuilder::new("rss-sample");
    b.param("idle_secs", idle_secs.into());
    b.param("churn_secs", churn_secs.into());
    b.param("post_secs", post_secs.into());
    b.param("interval_ms", interval_ms.into());

    // Sampler thread (reads /proc/self — same process as the JS runtime).
    let stop = Arc::new(AtomicBool::new(false));
    let stop_sampler = stop.clone();
    let t0 = Instant::now();
    let sampler = std::thread::spawn(move || {
        let mut probes: Vec<Probe> = Vec::new();
        while !stop_sampler.load(Ordering::Relaxed) {
            probes.push(Probe {
                t_ms: t0.elapsed().as_secs_f64() * 1e3,
                rss_kib: common::vm_rss_kib().unwrap_or(0) as f64,
                hwm_kib: common::vm_hwm_kib().unwrap_or(0) as f64,
                fd: common::fd_count().unwrap_or(0) as f64,
                threads: common::thread_count().unwrap_or(0) as f64,
            });
            std::thread::sleep(Duration::from_millis(interval_ms));
        }
        probes
    });

    // Runtime init (cold, n=1) + a warm eval.
    let ti = Instant::now();
    let mut rt = bun_runtime::BaoRuntime::new()
        .map_err(|e| format!("BaoRuntime::new failed: {}", e.message))?;
    let init_ms = ti.elapsed().as_secs_f64() * 1e3;
    let v = rt
        .eval("1+1", "<bench>")
        .map_err(|e| format!("warm eval failed: {}", e.message))?;
    match v {
        JsValue::Number(n) if n == 2.0 => {}
        other => return Err(format!("expected 2.0, got {other:?}")),
    }
    let idle_start_ms = t0.elapsed().as_secs_f64() * 1e3;

    // Phase 1: idle.
    std::thread::sleep(Duration::from_secs(idle_secs));
    let churn_start_ms = t0.elapsed().as_secs_f64() * 1e3;

    // Phase 2: synchronous allocation churn (blocking eval, no timers — no
    // event-loop dependency, pure allocation pressure on the SM heap).
    let churn_script = format!(
        r#"(function() {{
            const CHURN_MS = {ms};
            const t0 = Date.now();
            let i = 0;
            let sink = null;
            while (Date.now() - t0 < CHURN_MS) {{
                sink = new Array(2048);
                for (let j = 0; j < 64; j++) sink[j] = j * 1.5 + 'x';
                sink = ('' + i) + sink.length + Math.random();
                if (++i % 64 === 0) sink = null;
            }}
            return i;
        }})()"#,
        ms = churn_secs * 1000
    );
    let tc = Instant::now();
    let churn_iters = rt
        .eval(&churn_script, "<bench>")
        .map_err(|e| format!("churn eval failed: {}", e.message))?;
    let churn_eval_ms = tc.elapsed().as_secs_f64() * 1e3;
    let churn_iters_n = match churn_iters {
        JsValue::Number(n) => n,
        other => return Err(format!("churn returned non-number {other:?}")),
    };
    let post_start_ms = t0.elapsed().as_secs_f64() * 1e3;

    // Phase 3: post-churn idle (reclamation trend).
    std::thread::sleep(Duration::from_secs(post_secs));

    stop.store(true, Ordering::Relaxed);
    let probes = sampler
        .join()
        .map_err(|_| "sampler thread panicked".to_string())?;

    // Slice probes by phase boundaries.
    let idle: Vec<&Probe> = probes
        .iter()
        .filter(|p| p.t_ms >= idle_start_ms && p.t_ms < churn_start_ms)
        .collect();
    let churn: Vec<&Probe> = probes
        .iter()
        .filter(|p| p.t_ms >= churn_start_ms && p.t_ms < post_start_ms)
        .collect();
    let post: Vec<&Probe> = probes.iter().filter(|p| p.t_ms >= post_start_ms).collect();

    b.param("churn_iterations_js", churn_iters_n.into());
    b.param("churn_eval_ms", churn_eval_ms.into());
    b.param("probe_count", probes.len().into());

    b.metric(Metric::single("runtime_init", "ms", "latency", false, Some("cold"), init_ms));

    let idle_rss: Vec<f64> = phase_rss(&idle);
    let churn_rss: Vec<f64> = phase_rss(&churn);
    let post_rss: Vec<f64> = phase_rss(&post);
    if idle_rss.is_empty() || churn_rss.is_empty() || post_rss.is_empty() {
        return Err(format!(
            "empty probe phase (idle={} churn={} post={}) — sampler interval too coarse for phase durations",
            idle_rss.len(),
            churn_rss.len(),
            post_rss.len()
        ));
    }
    b.metric(Metric::from_samples("vm_rss_idle", "KiB", "memory", false, None, &idle_rss));
    b.metric(Metric::from_samples("vm_rss_churn", "KiB", "memory", false, None, &churn_rss));
    b.metric(Metric::from_samples("vm_rss_post", "KiB", "memory", false, None, &post_rss));
    b.metric(Metric::single("vm_rss_slope_churn", "KiB_per_s", "memory", false, None, slope_per_s(&churn)));
    b.metric(Metric::single("vm_rss_slope_post", "KiB_per_s", "memory", false, None, slope_per_s(&post)));

    let peak_hwm = probes.iter().map(|p| p.hwm_kib).fold(0.0f64, f64::max);
    b.metric(Metric::single("vm_hwm_peak", "KiB", "memory", false, None, peak_hwm));
    let fd_max = probes.iter().map(|p| p.fd).fold(0.0f64, f64::max);
    let threads_max = probes.iter().map(|p| p.threads).fold(0.0f64, f64::max);
    b.metric(Metric::single("fd_count_max", "count", "count", false, None, fd_max));
    b.metric(Metric::single("thread_count_max", "count", "count", false, None, threads_max));

    b.note("slopes are observations for issue #19 G budgets — monotonic growth must be attributable before any gate consumes them; SM does not return heap to the OS eagerly");
    b.note("churn is a synchronous blocking eval (arrays + strings + Math.random) — no timers/network, no event-loop dependency");
    Ok(b)
}
