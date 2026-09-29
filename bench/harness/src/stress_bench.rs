//! bench ⑨: page-stress — concurrent LIVE-page stress on one runtime thread
//! (#19-F / Gate B hard gap; W21 design §2).
//!
//! Concurrency semantics (honest, per the W21 design §0.1): BrowserRuntime is
//! !Send — N pages live SIMULTANEOUSLY at different lifecycle stages,
//! interleaved on a single runtime thread (browser tab-farm). NOT OS-thread
//! parallelism; the per-step blocking primitives (pipeline-ready /
//! wait_for_navigation, 15s caps) stall the whole pump — that stall is the
//! embedded-model fact this bench exposes, recorded in notes, never hidden.
//!
//! Modes (W21 design §2.2):
//!   - `farm` (default): every live page is advanced one lifecycle step per
//!     pump pass through an 8-stage machine; a CLOSED page is replaced with a
//!     fresh PENDING page, keeping in-flight ≈ N for the whole run.
//!   - `loop`: N logical workers round-robin through full serial
//!     `churn_cycle`s — the control group that quantifies what "live-page
//!     reuse" costs relative to serial × N.
//!
//! Stale detection (§2.3): a page sitting in a non-terminal stage longer than
//! `--stale-secs` (default 60) is a STALE event — counted fail-closed,
//! force-closed, and replaced. Distinguishes "designed wait" (a primitive in
//! flight is bounded by its own timeout) from a wedged realm.
//!
//! Resource guards (§2.4): fd / thread / RSS ceilings (env-tunable). A trip
//! is an explicit `resource-guard` sidecar record + fail-closed abort with a
//! non-zero exit — never a silent truncation under a green face.
//!
//! Reclamation final audit (§2.5, the Gate B core): after the run every page
//! is closed, idle pages are reaped, and the audit asserts
//! `pool.stats(): active == 0 && total_created == total_destroyed` (zero
//! retention), `fd/threads ≤ baseline + 5`, and records the RSS ratio against
//! the warm-up sample (≤1.15 is the Gate B target — first run seeds it, an
//! overage is recorded as a finding, never faked green).
//!
//! W21a lessons baked in: the EVALUATED step uses the WEB-realm
//! `evaluate_js_web` primitive (the page Node-realm registry has a known
//  churn-interaction defect queued as W26 — no `evaluate_js` here), and no
//! eval primitive is called with a timeout (a timed-out evaluate arms the SM
//! interrupt; residual interrupts poison every later eval with
//! InternalError).
//!
//! Sidecar: per-sample `step` records (rss/fd/threads/live_pages/stage
//! histogram/pool stats) and `failure`/`stale`/`guard` event records, same
//! crash-safe jsonl shape as soak.

use std::io::Write;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};
use serde_json::json;

use crate::common::{self, Metric, Params, ResultBuilder};
use crate::page_bench::churn_cycle;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Pending,
    Creating,
    Ready,
    Navigating,
    Loaded,
    Evaluated,
    Closing,
    Closed,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Pending => "pending",
            Stage::Creating => "creating",
            Stage::Ready => "ready",
            Stage::Navigating => "navigating",
            Stage::Loaded => "loaded",
            Stage::Evaluated => "evaluated",
            Stage::Closing => "closing",
            Stage::Closed => "closed",
        }
    }
}

const ALL_STAGES: [Stage; 8] = [
    Stage::Pending,
    Stage::Creating,
    Stage::Ready,
    Stage::Navigating,
    Stage::Loaded,
    Stage::Evaluated,
    Stage::Closing,
    Stage::Closed,
];

struct StressPage {
    handle: Option<PageHandle>,
    stage: Stage,
    stage_since: Instant,
    /// Per-stage elapsed ms accumulated as the page walks the machine.
    stage_ms: [f64; 8],
    nav_url: String,
    marker: String,
}

impl StressPage {
    fn new_pending(idx: usize) -> Self {
        let marker = format!("stress-{idx}");
        StressPage {
            handle: None,
            stage: Stage::Pending,
            stage_since: Instant::now(),
            stage_ms: [0.0; 8],
            nav_url: format!("data:text/html,<h1 id=b>{marker}</h1>"),
            marker,
        }
    }

    fn transition(&mut self, next: Stage) {
        let elapsed = self.stage_since.elapsed().as_secs_f64() * 1e3;
        self.stage_ms[self.stage as usize] += elapsed;
        self.stage = next;
        self.stage_since = Instant::now();
    }
}

fn write_rec(sc: &mut std::fs::File, value: serde_json::Value) -> Result<(), String> {
    writeln!(sc, "{value}").map_err(|e| format!("sidecar write failed: {e}"))?;
    sc.flush().map_err(|e| format!("sidecar flush failed: {e}"))
}

fn sidecar_path(out_path: &str) -> String {
    let stem = out_path.strip_suffix(".json").unwrap_or(out_path);
    format!("{stem}.segments.jsonl")
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

struct FarmOutcome {
    completed: usize,
    failures: usize,
    stale: usize,
    timeouts: usize,
}

/// Advance one live page by one lifecycle step. Returns Err on a failed step
/// (the caller applies the failure policy). A `None` return means the page
/// reached Closed and should be reaped+replaced.
#[allow(clippy::needless_return)]
fn advance_farm_page(
    runtime: &BrowserRuntime,
    page: &mut StressPage,
) -> Result<Option<()>, (bool, String)> {
    // bool = timeout-class failure (primitive cap exceeded).
    match page.stage {
        Stage::Pending => {
            let handle = runtime
                .create_page(&PageConfig {
                    url: Some("about:blank".into()),
                    ..Default::default()
                })
                .map_err(|e| (false, format!("create_page: {e}")))?;
            page.handle = Some(handle);
            page.transition(Stage::Creating);
            Ok(Some(()))
        }
        Stage::Creating => {
            let h = page.handle.as_ref().expect("creating implies handle");
            h.wait_for_pipeline_ready(Duration::from_secs(15))
                .map_err(|e| (e.to_string().contains("timed out"), format!("pipeline ready: {e}")))?;
            page.transition(Stage::Ready);
            Ok(Some(()))
        }
        Stage::Ready => {
            let h = page.handle.as_ref().expect("ready implies handle");
            h.navigate(&page.nav_url)
                .map_err(|e| (false, format!("navigate: {e}")))?;
            page.transition(Stage::Navigating);
            Ok(Some(()))
        }
        Stage::Navigating => {
            let h = page.handle.as_ref().expect("navigating implies handle");
            h.wait_for_navigation(Duration::from_secs(15))
                .map_err(|e| (e.to_string().contains("timed out"), format!("wait_for_navigation: {e}")))?;
            page.transition(Stage::Loaded);
            Ok(Some(()))
        }
        Stage::Loaded => {
            // WEB-realm verify (W26 node-realm registry defect avoided; no
            // timeout — see the module notes on SM interrupt residue).
            let h = page.handle.as_ref().expect("loaded implies handle");
            let check = h
                .evaluate_js_web(&format!(
                    "String(document.getElementById('b') && document.getElementById('b').textContent === '{}')",
                    page.marker
                ))
                .map_err(|e| (false, format!("evaluate: {e}")))?;
            if !check.contains("true") {
                return Err((false, format!("marker verification failed (got {check:?})")));
            }
            page.transition(Stage::Evaluated);
            Ok(Some(()))
        }
        Stage::Evaluated => {
            page.transition(Stage::Closing);
            Ok(Some(()))
        }
        Stage::Closing => {
            let h = page.handle.take().expect("closing implies handle");
            let pid = h.id();
            runtime
                .page_pool()
                .close_page(pid)
                .map_err(|e| (false, format!("close_page: {e}")))?;
            page.transition(Stage::Closed);
            Ok(None)
        }
        Stage::Closed => Ok(None),
    }
}

pub fn run(p: &Params, out_path: Option<&str>) -> Result<ResultBuilder, String> {
    let concurrency = p.usize_of("concurrency", 100).max(1);
    let duration_secs = p.u64_of("duration-secs", 300);
    let mode = p.str_of("mode", "farm");
    if mode != "farm" && mode != "loop" {
        return Err(format!(
            "unknown mode {mode:?} — only 'farm' and 'loop' are implemented (fail-closed)"
        ));
    }
    let ramp_secs = p.u64_of("ramp", 10);
    let stale_secs = p.u64_of("stale-secs", 60);
    let sample_ms = p.u64_of("sample-interval-ms", 1000);
    let max_fd = env_u64("BAO_STRESS_MAX_FD", 8192);
    let max_threads = env_u64("BAO_STRESS_MAX_THREADS", 2048);
    let max_rss_mib = env_u64("BAO_STRESS_MAX_RSS_MIB", 6144);
    let continue_on_fail = std::env::var("BAO_STRESS_CONTINUE_ON_FAIL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let out_path = out_path.ok_or(
        "page-stress requires --out: the step/event series streams to a sidecar derived from the result path",
    )?;
    let sidecar = sidecar_path(out_path);
    let mut sc = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sidecar)
        .map_err(|e| format!("cannot open sidecar {sidecar}: {e}"))?;

    let mut b = ResultBuilder::new("page-stress");
    b.param("concurrency", concurrency.into());
    b.param("duration_secs", duration_secs.into());
    b.param("mode", mode.clone().into());
    b.param("ramp_secs", ramp_secs.into());
    b.param("stale_secs", stale_secs.into());
    b.param("sample_interval_ms", sample_ms.into());
    b.param("max_fd", max_fd.into());
    b.param("max_threads", max_threads.into());
    b.param("max_rss_mib", max_rss_mib.into());
    b.param("continue_on_fail", continue_on_fail.into());
    b.note(format!(
        "page-stress concurrency semantics: BrowserRuntime is !Send — {concurrency} live pages interleaved on ONE runtime thread (tab-farm); NOT OS-thread parallelism. Per-step blocking primitives (pipeline-ready / wait_for_navigation, 15s caps) stall the whole pump by design — that stall is the embedded-model fact under observation"
    ));

    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        return Err("DISPLAY not set — browser stress must run under xvfb-run (bench/METHODOLOGY.md)".into());
    }

    // ── Cold start + resource baselines ─────────────────────────────────────
    // The pool ceiling must cover the live-page target: BaoConfig.max_pages
    // defaults to 50 (a product config field, not a code change) — without
    // this the N=100 farm hits "page limit exceeded: 50/50" at full ramp.
    let config = BaoConfig {
        max_pages: concurrency,
        ..Default::default()
    };
    let t0 = Instant::now();
    let runtime = BrowserRuntime::new(config)
        .map_err(|e| format!("BrowserRuntime::new failed: {e}"))?;
    b.metric(Metric::single(
        "browser_runtime_init",
        "ms",
        "latency",
        false,
        Some("cold"),
        t0.elapsed().as_secs_f64() * 1e3,
    ));
    let baseline_fd = common::fd_count().unwrap_or(0) as u64;
    let baseline_threads = common::thread_count().unwrap_or(0) as u64;
    b.param("baseline_fd", baseline_fd.into());
    b.param("baseline_threads", baseline_threads.into());

    write_rec(
        &mut sc,
        json!({
            "type": "stress-start", "t_ms": 0.0,
            "concurrency": concurrency, "duration_secs": duration_secs,
            "mode": mode, "ramp_secs": ramp_secs,
            "baseline_fd": baseline_fd, "baseline_threads": baseline_threads,
        }),
    )?;

    let start = Instant::now();
    let now_ms = || start.elapsed().as_secs_f64() * 1e3;
    let duration = Duration::from_secs(duration_secs);
    let mut completed = 0usize;
    let mut failures = 0usize;
    let mut stale_events = 0usize;
    let mut timeout_events = 0usize;
    let mut guard_events: Vec<String> = Vec::new();
    let mut live: Vec<StressPage> = Vec::new();
    let mut page_counter = 0usize;
    let mut step_samples: Vec<serde_json::Value> = Vec::new();
    let mut rss_series: Vec<f64> = Vec::new();
    let mut fd_series: Vec<f64> = Vec::new();
    let mut threads_series: Vec<f64> = Vec::new();
    let mut live_series: Vec<f64> = Vec::new();
    let mut stage_latency: [Vec<f64>; 8] = Default::default();
    let mut warmup_rss: Option<f64> = None;
    let mut last_sample = Instant::now();
    let mut loop_worker = 0usize;
    let mut loop_worker_done: Vec<usize> = vec![0; concurrency];

    // Ramp: target live count grows linearly over `ramp` seconds (0 = all at
    // once) so instantaneous fd/thread spikes cannot fake a guard trip.
    let target_live = |elapsed: Duration| -> usize {
        if ramp_secs == 0 {
            concurrency
        } else {
            let frac = elapsed.as_secs_f64() / ramp_secs as f64;
            (1.0 + frac * concurrency as f64).min(concurrency as f64) as usize
        }
    };

    if mode == "farm" {
        while start.elapsed() < duration {
            // Top up to the ramped target.
            let target = target_live(start.elapsed());
            while live.len() < target {
                live.push(StressPage::new_pending(page_counter));
                page_counter += 1;
            }

            // Advance every live page one step (single-threaded pump — a
            // blocking primitive stalls the pass; that is the observed fact).
            let mut i = 0;
            while i < live.len() {
                // Stale check first: a page stuck in a non-terminal stage past
                // the budget is force-closed, counted, and replaced.
                if live[i].stage_since.elapsed().as_secs() > stale_secs {
                    stale_events += 1;
                    let stage = live[i].stage;
                    if let Some(h) = live[i].handle.take() {
                        let _ = runtime.page_pool().close_page(h.id());
                    }
                    write_rec(
                        &mut sc,
                        json!({"type": "stale", "t_ms": now_ms(), "page": page_counter - live.len() + i,
                               "stage": stage.name(), "idle_secs": stale_secs}),
                    )?;
                    let mut replacement = StressPage::new_pending(page_counter);
                    page_counter += 1;
                    std::mem::swap(&mut live[i], &mut replacement);
                    i += 1;
                    continue;
                }
                let outcome = advance_farm_page(&runtime, &mut live[i]);
                match outcome {
                    Ok(Some(())) => {}
                    Ok(None) => {
                        // Closed: harvest stage latencies, count, replace.
                        let done = &mut live[i];
                        done.stage_ms[Stage::Closed as usize] =
                            done.stage_since.elapsed().as_secs_f64() * 1e3;
                        for (s, ms) in ALL_STAGES.iter().zip(done.stage_ms.iter()) {
                            if *ms > 0.0 {
                                stage_latency[*s as usize].push(*ms);
                            }
                        }
                        completed += 1;
                        let replacement = StressPage::new_pending(page_counter);
                        page_counter += 1;
                        live[i] = replacement;
                    }
                    Err((is_timeout, e)) => {
                        failures += 1;
                        if is_timeout {
                            timeout_events += 1;
                        }
                        let stage = live[i].stage;
                        write_rec(
                            &mut sc,
                            json!({"type": "failure", "t_ms": now_ms(), "stage": stage.name(),
                                   "timeout": is_timeout, "error": e}),
                        )?;
                        if let Some(h) = live[i].handle.take() {
                            let _ = runtime.page_pool().close_page(h.id());
                        }
                        if !continue_on_fail {
                            let _ = write_rec(
                                &mut sc,
                                json!({"type": "stress-end", "t_ms": now_ms(), "reason": "page-failure"}),
                            );
                            return Err(format!(
                                "page-stress aborted (fail-closed) at {} completed / {} live: step failure at stage {}: {e} — series preserved in {sidecar}",
                                completed,
                                live.len(),
                                stage.name()
                            ));
                        }
                        let replacement = StressPage::new_pending(page_counter);
                        page_counter += 1;
                        live[i] = replacement;
                    }
                }
                i += 1;
            }

            // Sampling + guards.
            if last_sample.elapsed().as_millis() as u64 >= sample_ms {
                last_sample = Instant::now();
                let rss = common::vm_rss_kib().unwrap_or(0) as f64;
                let fd = common::fd_count().unwrap_or(0) as f64;
                let threads = common::thread_count().unwrap_or(0) as f64;
                let pool = runtime.page_pool().stats();
                let mut hist = serde_json::Map::new();
                for s in ALL_STAGES {
                    let n = live.iter().filter(|pg| pg.stage == s).count();
                    hist.insert(s.name().to_string(), n.into());
                }
                let rec = json!({
                    "type": "step", "t_ms": now_ms(),
                    "rss_kib": rss, "fd": fd, "threads": threads,
                    "live_pages": live.len(),
                    "stage_hist": serde_json::Value::Object(hist),
                    "pool": {"active": pool.active, "idle": pool.idle,
                             "created": pool.total_created, "destroyed": pool.total_destroyed},
                });
                step_samples.push(rec.clone());
                write_rec(&mut sc, rec)?;
                rss_series.push(rss);
                fd_series.push(fd);
                threads_series.push(threads);
                live_series.push(live.len() as f64);
                if warmup_rss.is_none() && completed > 0 {
                    warmup_rss = Some(rss);
                }
                // Guards: explicit event + fail-closed abort.
                let rss_mib = rss / 1024.0;
                if fd as u64 > max_fd {
                    guard_events.push(format!("fd {} > {}", fd, max_fd));
                }
                if threads as u64 > max_threads {
                    guard_events.push(format!("threads {} > {}", threads, max_threads));
                }
                if rss_mib as u64 > max_rss_mib {
                    guard_events.push(format!("rss_mib {} > {}", rss_mib, max_rss_mib));
                }
                if !guard_events.is_empty() {
                    write_rec(
                        &mut sc,
                        json!({"type": "guard", "t_ms": now_ms(), "trips": guard_events}),
                    )?;
                    let _ = write_rec(
                        &mut sc,
                        json!({"type": "stress-end", "t_ms": now_ms(), "reason": "resource-guard"}),
                    );
                    return Err(format!(
                        "page-stress aborted: resource guard tripped ({:?}) — explicit guard event, not a silent truncation",
                        guard_events
                    ));
                }
            }
        }
    } else {
        // loop mode: N logical workers round-robin full churn_cycles.
        while start.elapsed() < duration {
            let res = churn_cycle(&runtime, loop_worker, None);
            match res {
                Ok(_) => {
                    completed += 1;
                    loop_worker_done[loop_worker % concurrency] += 1;
                }
                Err(e) => {
                    failures += 1;
                    write_rec(
                        &mut sc,
                        json!({"type": "failure", "t_ms": now_ms(), "worker": loop_worker % concurrency, "error": e}),
                    )?;
                    if !continue_on_fail {
                        let _ = write_rec(
                            &mut sc,
                            json!({"type": "stress-end", "t_ms": now_ms(), "reason": "page-failure"}),
                        );
                        return Err(format!(
                            "page-stress (loop) aborted fail-closed: {e} — series preserved in {sidecar}"
                        ));
                    }
                }
            }
            loop_worker += 1;
            if last_sample.elapsed().as_millis() as u64 >= sample_ms {
                last_sample = Instant::now();
                let rss = common::vm_rss_kib().unwrap_or(0) as f64;
                let fd = common::fd_count().unwrap_or(0) as f64;
                let threads = common::thread_count().unwrap_or(0) as f64;
                let pool = runtime.page_pool().stats();
                write_rec(
                    &mut sc,
                    json!({"type": "step", "t_ms": now_ms(), "rss_kib": rss, "fd": fd,
                           "threads": threads, "live_pages": 0,
                           "pool": {"active": pool.active, "idle": pool.idle,
                                    "created": pool.total_created, "destroyed": pool.total_destroyed}}),
                )?;
                rss_series.push(rss);
                fd_series.push(fd);
                threads_series.push(threads);
                live_series.push(0.0);
            }
        }
    }

    // ── Reclamation final audit (Gate B core) ───────────────────────────────
    runtime.page_pool().close_all();
    runtime.page_pool().check_idle_pages();
    let pool = runtime.page_pool().stats();
    let zero_retention = pool.active == 0 && pool.total_created == pool.total_destroyed;
    write_rec(
        &mut sc,
        json!({
            "type": "reclamation-audit", "t_ms": now_ms(),
            "active": pool.active, "idle": pool.idle,
            "created": pool.total_created, "destroyed": pool.total_destroyed,
            "zero_retention": zero_retention,
        }),
    )?;
    if !zero_retention {
        b.note(format!(
            "RECLAMATION FINDING: pool after drain has active={} created={} destroyed={} — \
             zero-retention violated (finding recorded, not faked)",
            pool.active, pool.total_created, pool.total_destroyed
        ));
    }

    // Bounded settle: ScriptThread teardown is asynchronous — sampling
    // fd/threads the instant close_all returns measures "reclamation not yet
    // finished", not "reclamation failed". Poll up to 5s (500ms cadence) and
    // take the settled reading; a still-elevated reading after the window is
    // a real finding.
    let settle_start = Instant::now();
    let (final_fd, final_threads, settled_ms) = loop {
        std::thread::sleep(Duration::from_millis(500));
        let fd = common::fd_count().unwrap_or(0) as u64;
        let th = common::thread_count().unwrap_or(0) as u64;
        if (fd <= baseline_fd + 5 && th <= baseline_threads + 5)
            || settle_start.elapsed() >= Duration::from_secs(5)
        {
            break (fd, th, settle_start.elapsed().as_millis() as u64);
        }
    };
    b.param("drain_settle_ms", settled_ms.into());
    let fd_ok = final_fd <= baseline_fd + 5;
    let threads_ok = final_threads <= baseline_threads + 5;
    if !fd_ok {
        b.note(format!(
            "RECLAMATION FINDING: final fd {} exceeds baseline {} + 5",
            final_fd, baseline_fd
        ));
    }
    if !threads_ok {
        b.note(format!(
            "RECLAMATION FINDING: final threads {} exceeds baseline {} + 5",
            final_threads, baseline_threads
        ));
    }

    let rss_final = common::vm_rss_kib().unwrap_or(0) as f64;
    let (rss_ratio, rss_ok) = match warmup_rss {
        Some(w) if w > 0.0 => (rss_final / w, rss_final / w <= 1.15),
        _ => (0.0, true), // no warm-up sample — nothing to compare, recorded
    };
    if warmup_rss.is_some() && !rss_ok {
        b.note(format!(
            "RSS-reclamation finding (seed run): final RSS {:.0} KiB / warm-up {:.0} KiB = {:.3} > 1.15 — \
             first run seeds the gate; overage recorded as a finding, never faked",
            rss_final,
            warmup_rss.unwrap_or(0.0),
            rss_ratio
        ));
    }

    let _ = write_rec(
        &mut sc,
        json!({
            "type": "stress-end", "t_ms": now_ms(), "reason": "duration-reached",
            "completed": completed, "failures": failures,
            "stale": stale_events, "timeouts": timeout_events,
        }),
    )?;

    // ── Metrics ─────────────────────────────────────────────────────────────
    b.param("pages_completed", completed.into());
    b.param("failure_events", failures.into());
    b.param("stale_events", stale_events.into());
    b.param("timeout_events", timeout_events.into());
    b.param("guard_events", guard_events.len().into());
    b.param("final_fd", final_fd.into());
    b.param("final_threads", final_threads.into());
    b.param("pool_active_after_drain", pool.active.into());
    b.param("pool_created", pool.total_created.into());
    b.param("pool_destroyed", pool.total_destroyed.into());
    b.param("zero_retention", zero_retention.into());
    let span = start.elapsed().as_secs_f64().max(1e-9);
    b.metric(Metric::single(
        "pages_completed_per_s",
        "ops_per_s",
        "throughput",
        true,
        None,
        completed as f64 / span,
    ));
    b.param("rss_final_kib", rss_final.into());
    if let Some(w) = warmup_rss {
        b.param("rss_warmup_kib", w.into());
        b.param("rss_reclamation_ratio", rss_ratio.into());
    }
    for (s, samples) in ALL_STAGES.iter().zip(stage_latency.iter()) {
        if !samples.is_empty() {
            b.metric(Metric::from_samples(
                &format!("stage_{}_ms", s.name()),
                "ms",
                "latency",
                false,
                Some("warm"),
                samples,
            ));
        }
    }
    if !rss_series.is_empty() {
        b.metric(Metric::from_samples("vm_rss", "KiB", "memory", false, None, &rss_series));
        b.metric(Metric::from_samples("fd_count", "count", "count", false, None, &fd_series));
        b.metric(Metric::from_samples("thread_count", "count", "count", false, None, &threads_series));
        b.metric(Metric::from_samples("live_pages", "count", "count", false, None, &live_series));
    }
    if mode == "loop" {
        for (w, n) in loop_worker_done.iter().enumerate() {
            if *n > 0 {
                // per-worker completeness only surfaces in the notes (N small)
                let _ = w;
            }
        }
        b.note(format!(
            "loop mode: {} logical workers completed {:?} cycles each (serial churn control group)",
            concurrency,
            loop_worker_done.iter().take(concurrency.min(8)).collect::<Vec<_>>()
        ));
    }
    b.note(format!(
        "raw step/event series in {sidecar} (crash-safe append); {} step samples",
        step_samples.len()
    ));
    Ok(b)
}
