//! Bench ③: browser page create / navigate / evaluate / close churn, with
//! per-iteration resource probes (RSS / fd / threads — issue #19 G-section
//! observability seed).
//!
//! One browser `BaoRuntime` (servo stack, xvfb DISPLAY required — run under
//! `xvfb-run`) hosts the whole churn loop; the churn unit is the page:
//! create(about:blank) → pipeline ready → navigate(data: URL) → load complete
//! → verified evaluate → close. Iteration 0 doubles as the cold observation
//! (first-page costs after runtime init are recorded separately as n=1).

use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};

use crate::common::{self, Metric, Params, ResultBuilder};

pub fn run(p: &Params) -> Result<ResultBuilder, String> {
    let iterations = p.usize_of("iterations", 15);
    let warmup = p.usize_of("warmup", 2);
    let budget_ms = p.f64_of("budget-ms", 300_000.0);

    let mut b = ResultBuilder::new("page-churn");
    b.param("iterations", iterations.into());
    b.param("warmup", warmup.into());
    b.param("budget_ms", budget_ms.into());
    b.param("navigate_target", "data:text/html (per-iteration marker)".into());

    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        return Err("DISPLAY not set — browser bench must run under xvfb-run (bench/METHODOLOGY.md)".into());
    }

    // ── Cold: browser runtime init + first page ────────────────────────────
    let t0 = Instant::now();
    let runtime = BaoRuntime::new(BaoConfig::default())
        .map_err(|e| format!("BaoRuntime::new failed: {e}"))?;
    let rt_init_ms = t0.elapsed().as_secs_f64() * 1e3;
    b.metric(Metric::single(
        "browser_runtime_init",
        "ms",
        "latency",
        false,
        Some("cold"),
        rt_init_ms,
    ));

    let t1 = Instant::now();
    let first_page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .map_err(|e| format!("first create_page failed: {e}"))?;
    first_page
        .wait_for_pipeline_ready(Duration::from_secs(15))
        .map_err(|e| format!("first pipeline not ready: {e}"))?;
    let first_page_ms = t1.elapsed().as_secs_f64() * 1e3;
    b.metric(Metric::single(
        "first_page_ready",
        "ms",
        "latency",
        false,
        Some("cold"),
        first_page_ms,
    ));
    let first_id = first_page.id();
    let t2 = Instant::now();
    runtime
        .page_pool()
        .close_page(first_id)
        .map_err(|e| format!("first close_page failed: {e}"))?;
    b.metric(Metric::single(
        "page_close_cold",
        "ms",
        "latency",
        false,
        Some("cold"),
        t2.elapsed().as_secs_f64() * 1e3,
    ));

    // ── Warm churn loop ────────────────────────────────────────────────────
    let mut create_ms: Vec<f64> = Vec::new();
    let mut ready_ms: Vec<f64> = Vec::new();
    let mut nav_ms: Vec<f64> = Vec::new();
    let mut load_ms: Vec<f64> = Vec::new();
    let mut eval_ms: Vec<f64> = Vec::new();
    let mut close_ms: Vec<f64> = Vec::new();
    let mut cycle_ms: Vec<f64> = Vec::new();
    let mut rss_after_close: Vec<f64> = Vec::new();
    let mut hwm_after_close: Vec<f64> = Vec::new();
    let mut fd_after_close: Vec<f64> = Vec::new();
    let mut threads_after_close: Vec<f64> = Vec::new();

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
        let marker = format!("benchmark-{i}");
        // Space-free / quote-free HTML: the WHATWG URL parser percent-encodes
        // both inside a data: URL path, which would corrupt the markup — so
        // the attribute is unquoted (`id=b`) and the marker has no spaces.
        let data_url = format!("data:text/html,<h1 id=b>{marker}</h1>");

        let tc = Instant::now();
        let page = runtime
            .create_page(&PageConfig {
                url: Some("about:blank".into()),
                ..Default::default()
            })
            .map_err(|e| format!("iter {i}: create_page failed: {e}"))?;
        let t_create = tc.elapsed().as_secs_f64() * 1e3;

        let tr = Instant::now();
        page.wait_for_pipeline_ready(Duration::from_secs(15))
            .map_err(|e| format!("iter {i}: pipeline not ready: {e}"))?;
        let t_ready = tr.elapsed().as_secs_f64() * 1e3;

        let tn = Instant::now();
        page.navigate(&data_url)
            .map_err(|e| format!("iter {i}: navigate failed: {e}"))?;
        let t_nav = tn.elapsed().as_secs_f64() * 1e3;

        // wait_for_navigation (not a readyState poll): about:blank is already
        // "complete", so polling readyState would observe the STALE previous
        // load and check the marker before the new document commits — the
        // primitive waits for the fresh Started→Complete cycle instead.
        let tl = Instant::now();
        page.wait_for_navigation(Duration::from_secs(15))
            .map_err(|e| format!("iter {i}: {e}"))?;
        let t_load = tl.elapsed().as_secs_f64() * 1e3;

        let te = Instant::now();
        let check = page
            .evaluate_js_web(&format!(
                "String(document.getElementById('b') && document.getElementById('b').textContent === '{marker}')"
            ))
            .map_err(|e| format!("iter {i}: marker eval failed: {e}"))?;
        let t_eval = te.elapsed().as_secs_f64() * 1e3;
        if !check.contains("true") {
            return Err(format!(
                "iter {i}: marker verification failed (got {check:?}) — fail-closed, no green numbers on wrong results"
            ));
        }

        let pid = page.id();
        let tcl = Instant::now();
        runtime
            .page_pool()
            .close_page(pid)
            .map_err(|e| format!("iter {i}: close_page failed: {e}"))?;
        let t_close = tcl.elapsed().as_secs_f64() * 1e3;

        let cycle = t_create + t_ready + t_nav + t_load + t_eval + t_close;
        if i >= warmup {
            create_ms.push(t_create);
            ready_ms.push(t_ready);
            nav_ms.push(t_nav);
            load_ms.push(t_load);
            eval_ms.push(t_eval);
            close_ms.push(t_close);
            cycle_ms.push(cycle);
            rss_after_close.push(common::vm_rss_kib().unwrap_or(0) as f64);
            hwm_after_close.push(common::vm_hwm_kib().unwrap_or(0) as f64);
            fd_after_close.push(common::fd_count().unwrap_or(0) as f64);
            threads_after_close.push(common::thread_count().unwrap_or(0) as f64);
        }
        // settle: let servo teardown work drain before the next page
        std::thread::sleep(Duration::from_millis(50));
    }

    if truncated {
        b.note(format!(
            "wall-clock budget hit after {executed}/{total} iterations — warm sample count is the honest n"
        ));
    }
    if cycle_ms.is_empty() {
        return Err(format!(
            "no warm samples collected (executed {executed}, warmup {warmup})"
        ));
    }

    b.param("executed_iterations", executed.into());
    b.param("warm_samples", cycle_ms.len().into());
    b.metric(Metric::from_samples("page_create", "ms", "latency", false, Some("warm"), &create_ms));
    b.metric(Metric::from_samples("pipeline_ready", "ms", "latency", false, Some("warm"), &ready_ms));
    b.metric(Metric::from_samples("navigate_dispatch", "ms", "latency", false, Some("warm"), &nav_ms));
    b.metric(Metric::from_samples("load_complete", "ms", "latency", false, Some("warm"), &load_ms));
    b.metric(Metric::from_samples("evaluate_web", "ms", "latency", false, Some("warm"), &eval_ms));
    b.metric(Metric::from_samples("page_close", "ms", "latency", false, Some("warm"), &close_ms));
    b.metric(Metric::from_samples("churn_cycle", "ms", "latency", false, Some("warm"), &cycle_ms));
    let cycles_per_s =
        cycle_ms.len() as f64 / (start.elapsed().as_secs_f64());
    b.metric(Metric::single("churn_pages_per_s_wall", "ops_per_s", "throughput", true, None, cycles_per_s));
    b.metric(Metric::from_samples("vm_rss_after_close", "KiB", "memory", false, None, &rss_after_close));
    b.metric(Metric::from_samples("vm_hwm", "KiB", "memory", false, None, &hwm_after_close));
    b.metric(Metric::from_samples("fd_count_after_close", "count", "count", false, None, &fd_after_close));
    b.metric(Metric::from_samples("thread_count_after_close", "count", "count", false, None, &threads_after_close));

    let rss_first = rss_after_close.first().copied().unwrap_or(0.0);
    let rss_last = rss_after_close.last().copied().unwrap_or(0.0);
    let span_s = start.elapsed().as_secs_f64();
    b.metric(Metric::single(
        "vm_rss_slope_over_churn",
        "KiB_per_s",
        "memory",
        false,
        None,
        (rss_last - rss_first) / span_s,
    ));
    b.note("rss/fd/thread probes taken after each warm close — monotonic growth must be attributable before any release gate (issue #19 G); seed data only records the trajectory");
    b.note("browser benches use fewer iterations (seconds per cycle) — warm n is smaller by design, see bench/METHODOLOGY.md §3");
    Ok(b)
}
