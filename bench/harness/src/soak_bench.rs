//! Bench ⑥: soak — bounded-duration page-churn long run with segmented
//! resource sampling and periodic forced-GC probes (issue #19 soak window;
//! quantifies the SM-EVOLUTION #29 S2 bounded-leak inventory on a live
//! process — 72h soak runs use the same subcommand with a larger duration).
//!
//! Shape: the page-churn cycle (shared helper in `page_bench` — identical
//! cycle semantics, numbers stay comparable with the Phase A seed) repeated
//! until `duration-mins` elapses, sampled per cycle (RSS / HWM / fd / threads
//! right after close). At each `segment-mins` boundary the churn pauses for a
//! forced-GC probe: `Bun.gc()` ×2 evaluated in the Node Realm of a long-lived
//! probe page — the only long-lived JS heap in the process, since each
//! JSContext owns its own JSRuntime (per-ScriptThread) and churn-page heaps
//! die with their ScriptThreads on close. RSS is read pre/post around settle
//! windows. After the churn a post-idle phase samples the reclamation trend.
//!
//! Raw per-cycle / per-segment / per-probe records stream to a
//! `<out>.segments.jsonl` sidecar (crash-safe append — a killed run keeps its
//! series); the final `--out` document is schema-conformant.
//!
//! Fail-closed contract: cycle verification failure aborts the run (error
//! document + non-zero exit; the series so far stays in the sidecar). A
//! forced-GC probe failure degrades verdict-③ data only — recorded in the
//! sidecar and `notes`, never silently skipped, never fatal to the series.

use std::io::Write;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle};
use serde_json::json;

use crate::common::{self, Metric, Params, ResultBuilder};
use crate::page_bench::churn_cycle;

struct CycleRec {
    i: usize,
    t_ms: f64,
    cycle_ms: f64,
    create_ms: f64,
    ready_ms: f64,
    nav_ms: f64,
    load_ms: f64,
    eval_ms: f64,
    close_ms: f64,
    rss_kib: f64,
    hwm_kib: f64,
    fd: f64,
    threads: f64,
}

struct SegmentRec {
    index: usize,
    cycles: usize,
    t_start_ms: f64,
    t_end_ms: f64,
    rss_first_kib: f64,
    rss_last_kib: f64,
    rss_growth_kib: f64,
}

#[derive(Clone, Copy)]
struct ProbeRec {
    t_ms: f64,
    segment: Option<usize>,
    rss_pre_kib: f64,
    rss_post_kib: f64,
    rss_drop_kib: f64,
    gc_eval_ms: f64,
    fd: f64,
    threads: f64,
}

/// Ordinary least-squares slope of ys over ts (seconds → KiB/s). Returns None
/// when there are fewer than 3 points or zero time span (slope undefined —
/// caller records the gap honestly instead of fabricating 0).
fn lsq_slope_kib_per_s(ts_s: &[f64], ys_kib: &[f64]) -> Option<f64> {
    if ts_s.len() != ys_kib.len() || ts_s.len() < 3 {
        return None;
    }
    let n = ts_s.len() as f64;
    let mean_t = ts_s.iter().sum::<f64>() / n;
    let mean_y = ys_kib.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut var = 0.0;
    for (t, y) in ts_s.iter().zip(ys_kib.iter()) {
        cov += (t - mean_t) * (y - mean_y);
        var += (t - mean_t) * (t - mean_t);
    }
    if var <= 0.0 {
        return None;
    }
    Some(cov / var)
}

/// Forced-GC probe on the long-lived probe page. Two `Bun.gc()` passes
/// (GCReason::API full GC; second pass sweeps finalized survivors), RSS read
/// before the first and after a settle window following the second so malloc
/// trim / mmap changes become visible in /proc.
fn forced_gc_probe(probe: &PageHandle, settle_ms: u64) -> Result<ProbeRec, String> {
    std::thread::sleep(Duration::from_millis(settle_ms));
    let rss_pre = common::vm_rss_kib().unwrap_or(0) as f64;
    let fd = common::fd_count().unwrap_or(0) as f64;
    let threads = common::thread_count().unwrap_or(0) as f64;

    let t0 = Instant::now();
    probe
        .evaluate_js("Bun.gc()")
        .map_err(|e| format!("forced GC pass 1 failed: {e}"))?;
    probe
        .evaluate_js("Bun.gc()")
        .map_err(|e| format!("forced GC pass 2 failed: {e}"))?;
    let gc_eval_ms = t0.elapsed().as_secs_f64() * 1e3;

    std::thread::sleep(Duration::from_millis(settle_ms));
    let rss_post = common::vm_rss_kib().unwrap_or(0) as f64;
    Ok(ProbeRec {
        t_ms: 0.0, // caller stamps wall time
        segment: None,
        rss_pre_kib: rss_pre,
        rss_post_kib: rss_post,
        rss_drop_kib: rss_pre - rss_post,
        gc_eval_ms,
        fd,
        threads,
    })
}

fn write_rec(sc: &mut std::fs::File, value: serde_json::Value) -> Result<(), String> {
    writeln!(sc, "{value}").map_err(|e| format!("sidecar write failed: {e}"))?;
    sc.flush().map_err(|e| format!("sidecar flush failed: {e}"))
}

fn sidecar_path(out_path: &str) -> String {
    let stem = out_path.strip_suffix(".json").unwrap_or(out_path);
    format!("{stem}.segments.jsonl")
}

pub fn run(p: &Params, out_path: Option<&str>) -> Result<ResultBuilder, String> {
    let scenario = p.str_of("scenario", "page-churn");
    if scenario != "page-churn" {
        return Err(format!(
            "unknown scenario {scenario:?} — only 'page-churn' is implemented (fail-closed, no silent default)"
        ));
    }
    let duration_mins = p.u64_of("duration-mins", 60);
    let segment_mins = p.u64_of("segment-mins", 10);
    let post_mins = p.u64_of("post-mins", 2);
    let interval_ms = p.u64_of("interval-ms", 1000);
    let settle_ms = p.u64_of("settle-ms", 50);
    let gc_settle_ms = p.u64_of("gc-settle-ms", 3000);

    let out_path = out_path.ok_or(
        "soak requires --out: the per-cycle series streams to a sidecar derived from the result path (stdout carries servo log noise)",
    )?;
    let sidecar = sidecar_path(out_path);
    let mut sc = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sidecar)
        .map_err(|e| format!("cannot open sidecar {sidecar}: {e}"))?;

    let mut b = ResultBuilder::new("soak");
    b.param("scenario", scenario.into());
    b.param("duration_mins", duration_mins.into());
    b.param("segment_mins", segment_mins.into());
    b.param("post_mins", post_mins.into());
    b.param("interval_ms", interval_ms.into());
    b.param("settle_ms", settle_ms.into());
    b.param("gc_settle_ms", gc_settle_ms.into());
    b.param(
        "cycle_shape",
        "page_bench::churn_cycle (shared with page-churn bench)".into(),
    );
    b.param(
        "forced_gc",
        "Bun.gc() x2 via probe-page Node Realm (GCReason::API)".into(),
    );

    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        return Err(
            "DISPLAY not set — browser soak must run under xvfb-run (bench/METHODOLOGY.md)".into()
        );
    }

    // ── Cold: browser runtime init + long-lived probe page ─────────────────
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
    let probe_page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .map_err(|e| format!("probe create_page failed: {e}"))?;
    probe_page
        .wait_for_pipeline_ready(Duration::from_secs(15))
        .map_err(|e| format!("probe pipeline not ready: {e}"))?;
    // Sanity: the Node Realm must answer (the forced-GC probe depends on it).
    let gc_type = probe_page
        .evaluate_js("typeof Bun.gc")
        .map_err(|e| format!("probe Node Realm check failed: {e}"))?;
    if !gc_type.contains("function") {
        return Err(format!(
            "Bun.gc is not a function in the probe Node Realm (got {gc_type:?}) — forced-GC probes cannot run"
        ));
    }
    let probe_ready_ms = t1.elapsed().as_secs_f64() * 1e3;
    b.metric(Metric::single(
        "probe_page_ready",
        "ms",
        "latency",
        false,
        Some("cold"),
        probe_ready_ms,
    ));
    let probe_id = probe_page.id();

    write_rec(
        &mut sc,
        json!({
            "type": "soak-start", "t_ms": 0.0,
            "duration_mins": duration_mins, "segment_mins": segment_mins,
            "post_mins": post_mins, "interval_ms": interval_ms,
            "settle_ms": settle_ms, "gc_settle_ms": gc_settle_ms,
        }),
    )?;

    // ── Churn loop with segment boundaries ─────────────────────────────────
    let start = Instant::now();
    let now_ms = || start.elapsed().as_secs_f64() * 1e3;
    let duration_secs = duration_mins * 60;
    let segment_secs = segment_mins * 60;
    let mut cycles: Vec<CycleRec> = Vec::new();
    let mut segments: Vec<SegmentRec> = Vec::new();
    let mut probes: Vec<ProbeRec> = Vec::new();
    let mut probe_failures: Vec<String> = Vec::new();
    let mut segment_cycles_start = 0usize;
    let mut steady_from_cycle: Option<usize> = None;
    let mut next_boundary = segment_secs;
    let mut i = 0usize;

    while start.elapsed().as_secs() < duration_secs {
        let t = match churn_cycle(&runtime, i) {
            Ok(t) => t,
            Err(e) => {
                let _ = write_rec(
                    &mut sc,
                    json!({"type": "soak-end", "t_ms": now_ms(),
                           "reason": "cycle-failure", "error": e}),
                );
                return Err(format!(
                    "soak aborted after {} cycles / {} full segments at cycle {i}: {e} — per-cycle series preserved in {sidecar}",
                    cycles.len(),
                    segments.len()
                ));
            }
        };
        std::thread::sleep(Duration::from_millis(settle_ms));

        let rec = CycleRec {
            i,
            t_ms: now_ms(),
            cycle_ms: t.cycle_ms,
            create_ms: t.create_ms,
            ready_ms: t.ready_ms,
            nav_ms: t.nav_ms,
            load_ms: t.load_ms,
            eval_ms: t.eval_ms,
            close_ms: t.close_ms,
            rss_kib: common::vm_rss_kib().unwrap_or(0) as f64,
            hwm_kib: common::vm_hwm_kib().unwrap_or(0) as f64,
            fd: common::fd_count().unwrap_or(0) as f64,
            threads: common::thread_count().unwrap_or(0) as f64,
        };
        let rec_json = json!({
            "type": "cycle", "i": rec.i, "t_ms": rec.t_ms,
            "cycle_ms": rec.cycle_ms, "create_ms": rec.create_ms,
            "ready_ms": rec.ready_ms, "nav_ms": rec.nav_ms,
            "load_ms": rec.load_ms, "eval_ms": rec.eval_ms,
            "close_ms": rec.close_ms,
            "rss_kib": rec.rss_kib, "hwm_kib": rec.hwm_kib,
            "fd": rec.fd, "threads": rec.threads,
        });
        cycles.push(rec);
        write_rec(&mut sc, rec_json)?;

        // Segment boundary: forced-GC probe + segment summary.
        if start.elapsed().as_secs() >= next_boundary {
            let seg_idx = segments.len();
            match forced_gc_probe(&probe_page, gc_settle_ms) {
                Ok(mut pr) => {
                    pr.t_ms = now_ms();
                    pr.segment = Some(seg_idx);
                    write_rec(
                        &mut sc,
                        json!({
                            "type": "gc-probe", "t_ms": pr.t_ms, "segment": seg_idx,
                            "rss_pre_kib": pr.rss_pre_kib, "rss_post_kib": pr.rss_post_kib,
                            "rss_drop_kib": pr.rss_drop_kib, "gc_eval_ms": pr.gc_eval_ms,
                            "fd": pr.fd, "threads": pr.threads,
                        }),
                    )?;
                    probes.push(pr);
                }
                Err(e) => {
                    probe_failures.push(format!("segment {seg_idx}: {e}"));
                    write_rec(
                        &mut sc,
                        json!({"type": "gc-probe", "t_ms": now_ms(), "segment": seg_idx,
                               "ok": false, "error": e}),
                    )?;
                }
            }

            let first = &cycles[segment_cycles_start];
            let last = cycles.last().expect("boundary implies ≥1 cycle");
            let (seg_cycles, seg_rss_first, seg_rss_last) = (
                cycles.len() - segment_cycles_start,
                first.rss_kib,
                last.rss_kib,
            );
            let seg_rss_growth = seg_rss_last - seg_rss_first;
            let seg = SegmentRec {
                index: seg_idx,
                cycles: seg_cycles,
                t_start_ms: first.t_ms,
                t_end_ms: last.t_ms,
                rss_first_kib: seg_rss_first,
                rss_last_kib: seg_rss_last,
                rss_growth_kib: seg_rss_last - seg_rss_first,
            };
            write_rec(
                &mut sc,
                json!({
                    "type": "segment", "segment": seg.index, "cycles": seg.cycles,
                    "t_start_ms": seg.t_start_ms, "t_end_ms": seg.t_end_ms,
                    "rss_first_kib": seg.rss_first_kib, "rss_last_kib": seg.rss_last_kib,
                    "rss_growth_kib": seg.rss_growth_kib,
                }),
            )?;
            segments.push(seg);

            segment_cycles_start = cycles.len();
            if steady_from_cycle.is_none() {
                steady_from_cycle = Some(cycles.len());
            }
            next_boundary += segment_secs;
            eprintln!(
                "[soak] segment {} closed: {} cycles, rss {:.0}→{:.0} KiB (growth {:+.0} KiB)",
                seg_idx,
                seg_cycles,
                seg_rss_first,
                seg_rss_last,
                seg_rss_growth
            );
        }
        i += 1;
    }

    // ── Final probe + post-idle reclamation phase ──────────────────────────
    let final_probe = match forced_gc_probe(&probe_page, gc_settle_ms) {
        Ok(mut pr) => {
            pr.t_ms = now_ms();
            write_rec(
                &mut sc,
                json!({
                    "type": "gc-probe", "t_ms": pr.t_ms, "segment": null,
                    "rss_pre_kib": pr.rss_pre_kib, "rss_post_kib": pr.rss_post_kib,
                    "rss_drop_kib": pr.rss_drop_kib, "gc_eval_ms": pr.gc_eval_ms,
                    "fd": pr.fd, "threads": pr.threads,
                }),
            )?;
            Some(pr)
        }
        Err(e) => {
            probe_failures.push(format!("final probe: {e}"));
            write_rec(
                &mut sc,
                json!({"type": "gc-probe", "t_ms": now_ms(), "segment": null,
                       "ok": false, "error": e}),
            )?;
            None
        }
    };

    let mut post_rss: Vec<(f64, f64)> = Vec::new(); // (t_s, rss KiB)
    let post_deadline = Duration::from_secs(post_mins * 60);
    let post_start = Instant::now();
    while post_start.elapsed() < post_deadline {
        std::thread::sleep(Duration::from_millis(interval_ms));
        let t_ms = now_ms();
        let rss = common::vm_rss_kib().unwrap_or(0) as f64;
        let rec = json!({
            "type": "post", "t_ms": t_ms, "rss_kib": rss,
            "hwm_kib": common::vm_hwm_kib().unwrap_or(0) as f64,
            "fd": common::fd_count().unwrap_or(0) as f64,
            "threads": common::thread_count().unwrap_or(0) as f64,
        });
        write_rec(&mut sc, rec)?;
        post_rss.push((t_ms / 1000.0, rss));
    }

    // Best-effort probe-page close (its cost is not a soak metric).
    if let Err(e) = runtime.page_pool().close_page(probe_id) {
        b.note(format!("probe page close failed at soak end: {e}"));
    }

    write_rec(
        &mut sc,
        json!({
            "type": "soak-end", "t_ms": now_ms(), "reason": "duration-reached",
            "cycles": cycles.len(), "segments": segments.len(),
            "probes_ok": probes.len() + usize::from(final_probe.is_some()),
            "probe_failures": probe_failures.len(),
        }),
    )?;

    if cycles.is_empty() {
        return Err(format!(
            "zero churn cycles executed (duration {duration_mins}min too short for one cycle)"
        ));
    }

    // ── Metrics ─────────────────────────────────────────────────────────────
    b.param("executed_cycles", cycles.len().into());
    b.param("full_segments", segments.len().into());
    b.param("forced_gc_probes_ok", (probes.len() + usize::from(final_probe.is_some())).into());
    b.param("forced_gc_probe_failures", probe_failures.len().into());

    let cycle_ms_all: Vec<f64> = cycles.iter().map(|c| c.cycle_ms).collect();
    b.metric(Metric::from_samples(
        "churn_cycle",
        "ms",
        "latency",
        false,
        Some("warm"),
        &cycle_ms_all,
    ));
    let churn_span_s = (cycles.last().unwrap().t_ms - cycles[0].t_ms) / 1000.0;
    if churn_span_s > 0.0 {
        b.metric(Metric::single(
            "churn_pages_per_s_wall",
            "ops_per_s",
            "throughput",
            true,
            None,
            cycles.len() as f64 / churn_span_s,
        ));
    }

    let rss_series: Vec<f64> = cycles.iter().map(|c| c.rss_kib).collect();
    let t_series: Vec<f64> = cycles.iter().map(|c| c.t_ms / 1000.0).collect();
    b.metric(Metric::from_samples(
        "vm_rss_after_close",
        "KiB",
        "memory",
        false,
        None,
        &rss_series,
    ));
    let hwm_series: Vec<f64> = cycles.iter().map(|c| c.hwm_kib).collect();
    b.metric(Metric::from_samples("vm_hwm", "KiB", "memory", false, None, &hwm_series));
    let fd_series: Vec<f64> = cycles.iter().map(|c| c.fd).collect();
    b.metric(Metric::from_samples(
        "fd_count_after_close",
        "count",
        "count",
        false,
        None,
        &fd_series,
    ));
    let threads_series: Vec<f64> = cycles.iter().map(|c| c.threads).collect();
    b.metric(Metric::from_samples(
        "thread_count_after_close",
        "count",
        "count",
        false,
        None,
        &threads_series,
    ));

    // Whole-run slope (warm-up dominated — comparability with page-churn's
    // vm_rss_slope_over_churn; interpretation belongs to the steady slope).
    b.metric(Metric::single(
        "vm_rss_slope_over_soak",
        "KiB_per_s",
        "memory",
        false,
        None,
        (rss_series.last().unwrap() - rss_series[0]) / churn_span_s.max(1e-9),
    ));

    // Steady slope: least squares over cycles after segment 0 (warm-up:
    // allocator arenas, JIT/code caches, servo module init).
    if let Some(from) = steady_from_cycle {
        let ts: Vec<f64> = t_series[from.min(t_series.len())..].to_vec();
        let ys: Vec<f64> = rss_series[from.min(rss_series.len())..].to_vec();
        match lsq_slope_kib_per_s(&ts, &ys) {
            Some(slope) => {
                b.metric(Metric::single(
                    "vm_rss_slope_steady",
                    "KiB_per_s",
                    "memory",
                    false,
                    None,
                    slope,
                ));
            }
            None => b.note("steady slope not computable (<3 cycles after segment 0)"),
        }
    } else {
        b.note("no full segment closed — steady (warm-up-excluded) slope not computed; whole-run slope is warm-up dominated");
    }

    // Segment metrics (n = full segments; samples visible for auditability).
    if !segments.is_empty() {
        let growth: Vec<f64> = segments.iter().map(|s| s.rss_growth_kib).collect();
        b.metric(Metric::from_samples(
            "segment_rss_growth_kib",
            "KiB",
            "memory",
            false,
            None,
            &growth,
        ));
        let firsts: Vec<f64> = segments.iter().map(|s| s.rss_first_kib).collect();
        b.metric(Metric::from_samples(
            "segment_rss_first_kib",
            "KiB",
            "memory",
            false,
            None,
            &firsts,
        ));
        let lasts: Vec<f64> = segments.iter().map(|s| s.rss_last_kib).collect();
        b.metric(Metric::from_samples(
            "segment_rss_last_kib",
            "KiB",
            "memory",
            false,
            None,
            &lasts,
        ));
        let seg_cycles: Vec<f64> = segments.iter().map(|s| s.cycles as f64).collect();
        b.metric(Metric::from_samples(
            "segment_cycles",
            "count",
            "count",
            false,
            None,
            &seg_cycles,
        ));
    }

    // Forced-GC probe metrics (n = successful probes).
    let mut all_probes = probes.clone();
    if let Some(pr) = final_probe {
        all_probes.push(pr);
    }
    if !all_probes.is_empty() {
        let pre: Vec<f64> = all_probes.iter().map(|p| p.rss_pre_kib).collect();
        b.metric(Metric::from_samples(
            "forced_gc_rss_pre_kib",
            "KiB",
            "memory",
            false,
            None,
            &pre,
        ));
        let post: Vec<f64> = all_probes.iter().map(|p| p.rss_post_kib).collect();
        b.metric(Metric::from_samples(
            "forced_gc_rss_post_kib",
            "KiB",
            "memory",
            false,
            None,
            &post,
        ));
        let drop: Vec<f64> = all_probes.iter().map(|p| p.rss_drop_kib).collect();
        b.metric(Metric::from_samples(
            "forced_gc_rss_drop_kib",
            "KiB",
            "memory",
            false,
            None,
            &drop,
        ));
        let gc_ms: Vec<f64> = all_probes.iter().map(|p| p.gc_eval_ms).collect();
        b.metric(Metric::from_samples(
            "forced_gc_eval_ms",
            "ms",
            "latency",
            false,
            None,
            &gc_ms,
        ));
    }

    // Post-idle reclamation trend.
    if !post_rss.is_empty() {
        let ys: Vec<f64> = post_rss.iter().map(|p| p.1).collect();
        b.metric(Metric::from_samples(
            "vm_rss_post_idle",
            "KiB",
            "memory",
            false,
            None,
            &ys,
        ));
        let ts: Vec<f64> = post_rss.iter().map(|p| p.0).collect();
        if let Some(slope) = lsq_slope_kib_per_s(&ts, &ys) {
            b.metric(Metric::single(
                "vm_rss_slope_post",
                "KiB_per_s",
                "memory",
                false,
                None,
                slope,
            ));
        }
    }

    b.note(format!(
        "raw per-cycle/segment/probe series in {sidecar} (crash-safe append); in-doc per-cycle sample arrays are capped at 2000 (METHODOLOGY.md §7)"
    ));
    b.note(
        "forced GC (Bun.gc x2, GCReason::API) covers the long-lived probe page's JSRuntime only — each JSContext owns its JSRuntime and churn-page heaps are reclaimed by ScriptThread exit on close; post-GC RSS delta is therefore a lower bound on reclaimable memory",
    );
    b.note(
        "segment 0 is warm-up (allocator arenas / JIT / code caches / servo init): vm_rss_slope_over_soak is warm-up dominated — leak-rate adjudication must use vm_rss_slope_steady (excludes segment 0)",
    );
    if !probe_failures.is_empty() {
        b.note(format!(
            "{} forced-GC probe failure(s) — verdict on GC reclamation is degraded, not fabricated: {:?}",
            probe_failures.len(),
            probe_failures
        ));
    }
    Ok(b)
}
