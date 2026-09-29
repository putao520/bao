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
//! Engine-native heap metering (SM-EVOLUTION #27 裁决 6 → #19): each probe
//! also samples `JS::CollectRuntimeStats` on the probe page's ScriptThread
//! (via the bao_engine glue — pre and post around the forced GC), so the
//! verdict data carries engine-level GC-heap / live-GC-things / malloc-heap
//! numbers next to the process RSS proxy. Engine-metering failure degrades
//! only those fields — recorded honestly, never zero-filled.
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

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};
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
    /// Engine-native heap sample taken before the forced GC (None = the
    /// engine metering failed for this probe; recorded honestly).
    engine_pre: Option<EngineHeapSample>,
    /// Engine-native heap sample taken after the forced GC + settle.
    engine_post: Option<EngineHeapSample>,
}

/// Engine-native heap numbers for one probe sample, KiB / counts
/// (`JS::CollectRuntimeStats` via bao_engine, on the probe page's
/// ScriptThread — the same runtime the forced GC covers).
#[derive(Clone, Copy)]
struct EngineHeapSample {
    gc_heap_chunk_total_kib: f64,
    gc_heap_gc_things_kib: f64,
    zone_live_gc_things_kib: f64,
    malloc_heap_kib: f64,
    zones: f64,
    realms: f64,
}

impl EngineHeapSample {
    fn collect(probe: &PageHandle) -> Result<Self, String> {
        let s = probe
            .collect_engine_memory_stats()
            .map_err(|e| format!("engine stats collection failed: {e}"))?;
        Ok(EngineHeapSample {
            gc_heap_chunk_total_kib: s.gc_heap_chunk_total as f64 / 1024.0,
            gc_heap_gc_things_kib: s.gc_heap_gc_things as f64 / 1024.0,
            zone_live_gc_things_kib: s.zone_live_gc_things as f64 / 1024.0,
            malloc_heap_kib: s.servo_malloc_heap as f64 / 1024.0,
            zones: s.zone_count as f64,
            realms: s.realm_count as f64,
        })
    }

    fn to_json(&self) -> serde_json::Value {
        json!({
            "gc_heap_chunk_total_kib": self.gc_heap_chunk_total_kib,
            "gc_heap_gc_things_kib": self.gc_heap_gc_things_kib,
            "zone_live_gc_things_kib": self.zone_live_gc_things_kib,
            "malloc_heap_kib": self.malloc_heap_kib,
            "zones": self.zones,
            "realms": self.realms,
        })
    }
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
/// (W10-impl: GCOptions::Shrink non-incremental collection — decommit +
/// purge tail; second pass sweeps finalized survivors), RSS read
/// before the first and after a settle window following the second so malloc
/// trim / mmap changes become visible in /proc. Engine-native heap samples
/// (`JS::CollectRuntimeStats`, probe page's ScriptThread) bracket the GC —
/// their failures degrade only the engine fields (returned for honest
/// reporting), never the GC/RSS probe itself.
fn forced_gc_probe(
    probe: &PageHandle,
    settle_ms: u64,
) -> Result<(ProbeRec, Vec<String>), String> {
    std::thread::sleep(Duration::from_millis(settle_ms));
    let rss_pre = common::vm_rss_kib().unwrap_or(0) as f64;
    let fd = common::fd_count().unwrap_or(0) as f64;
    let threads = common::thread_count().unwrap_or(0) as f64;
    let mut engine_failures: Vec<String> = Vec::new();
    let engine_pre = match EngineHeapSample::collect(probe) {
        Ok(s) => Some(s),
        Err(e) => {
            engine_failures.push(format!("pre: {e}"));
            None
        }
    };

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
    let engine_post = match EngineHeapSample::collect(probe) {
        Ok(s) => Some(s),
        Err(e) => {
            engine_failures.push(format!("post: {e}"));
            None
        }
    };
    Ok((
        ProbeRec {
            t_ms: 0.0, // caller stamps wall time
            segment: None,
            rss_pre_kib: rss_pre,
            rss_post_kib: rss_post,
            rss_drop_kib: rss_pre - rss_post,
            gc_eval_ms,
            fd,
            threads,
            engine_pre,
            engine_post,
        },
        engine_failures,
    ))
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
    match scenario.as_str() {
        "page-churn" => {}
        "mixed" => return run_mixed(p, out_path),
        _ => {
            return Err(format!(
                "unknown scenario {scenario:?} — only 'page-churn' and 'mixed' are implemented (fail-closed, no silent default)"
            ));
        }
    }
    let duration_mins = p.u64_of("duration-mins", 60);
    let segment_mins = p.u64_of("segment-mins", 10);
    let post_mins = p.u64_of("post-mins", 2);
    let interval_ms = p.u64_of("interval-ms", 1000);
    let settle_ms = p.u64_of("settle-ms", 50);
    let gc_settle_ms = p.u64_of("gc-settle-ms", 3000);
    // #40: continue-on-cycle-failure mode. Default off = the original
    // fail-closed abort (a cycle error kills the run). With
    // BAO_SOAK_CONTINUE_ON_FAIL=1 a bounded phase timeout (post-fix churn
    // phases return Err instead of hanging) becomes a COUNTED event —
    // written to the sidecar and the result doc, never silently swallowed —
    // so a 72h soak survives isolated cycle failures and quantifies them.
    let continue_on_fail = std::env::var("BAO_SOAK_CONTINUE_ON_FAIL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    // W6 A/B knob: BAO_SOAK_URL=<url> makes every churn cycle navigate to
    // <url> instead of the built-in data: marker (default: unset — byte-zero
    // behavior change). An http:// URL turns the churn into a network-stack
    // exercise (each cycle = fresh page + navigation + the page's own fetch
    // traffic); used with an external static server for the leak A/B.
    let soak_url = std::env::var("BAO_SOAK_URL").ok().filter(|v| !v.is_empty());


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
    if let Some(u) = &soak_url {
        b.param("soak_url_override", serde_json::json!(u.clone()));
    }
    b.param(
        "forced_gc",
        "Bun.gc() x2 via probe-page Node Realm (GCReason::API)".into(),
    );
    b.param(
        "engine_metering",
        "JS::CollectRuntimeStats via bao_engine glue (vendor mozjs jsglue.cpp), \
         sampled pre/post around each forced GC on the probe page's \
         ScriptThread — engine heap/GC numbers next to the process RSS proxy \
         (SM-EVOLUTION #27 裁决 6 → #19)"
            .into(),
    );

    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        return Err(
            "DISPLAY not set — browser soak must run under xvfb-run (bench/METHODOLOGY.md)".into()
        );
    }

    // ── Cold: browser runtime init + long-lived probe page ─────────────────
    let t0 = Instant::now();
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .map_err(|e| format!("BrowserRuntime::new failed: {e}"))?;
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
    let mut engine_metering_failures: Vec<String> = Vec::new();
    let mut segment_cycles_start = 0usize;
    let mut steady_from_cycle: Option<usize> = None;
    let mut next_boundary = segment_secs;
    let mut cycle_failures: Vec<(usize, String)> = Vec::new();
    let mut i = 0usize;

    while start.elapsed().as_secs() < duration_secs {
        let t = match churn_cycle(&runtime, i, soak_url.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                let _ = write_rec(
                    &mut sc,
                    json!({"type": "cycle-failure", "i": i, "t_ms": now_ms(),
                           "error": e}),
                );
                if continue_on_fail {
                    // Counted failure, run continues (fail-closed reporting —
                    // the event is in the sidecar AND the final doc).
                    cycle_failures.push((i, e));
                    i += 1;
                    continue;
                }
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
                Ok((mut pr, engine_errs)) => {
                    pr.t_ms = now_ms();
                    pr.segment = Some(seg_idx);
                    if !engine_errs.is_empty() {
                        engine_metering_failures.push(format!(
                            "segment {seg_idx}: {}",
                            engine_errs.join("; ")
                        ));
                    }
                    write_rec(
                        &mut sc,
                        json!({
                            "type": "gc-probe", "t_ms": pr.t_ms, "segment": seg_idx,
                            "rss_pre_kib": pr.rss_pre_kib, "rss_post_kib": pr.rss_post_kib,
                            "rss_drop_kib": pr.rss_drop_kib, "gc_eval_ms": pr.gc_eval_ms,
                            "fd": pr.fd, "threads": pr.threads,
                            "engine_pre": pr.engine_pre.map(|s| s.to_json()),
                            "engine_post": pr.engine_post.map(|s| s.to_json()),
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
        Ok((mut pr, engine_errs)) => {
            pr.t_ms = now_ms();
            if !engine_errs.is_empty() {
                engine_metering_failures
                    .push(format!("final probe: {}", engine_errs.join("; ")));
            }
            write_rec(
                &mut sc,
                json!({
                    "type": "gc-probe", "t_ms": pr.t_ms, "segment": null,
                    "rss_pre_kib": pr.rss_pre_kib, "rss_post_kib": pr.rss_post_kib,
                    "rss_drop_kib": pr.rss_drop_kib, "gc_eval_ms": pr.gc_eval_ms,
                    "fd": pr.fd, "threads": pr.threads,
                    "engine_pre": pr.engine_pre.map(|s| s.to_json()),
                    "engine_post": pr.engine_post.map(|s| s.to_json()),
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
            "cycle_failures": cycle_failures.len(),
        }),
    )?;

    if cycles.is_empty() {
        return Err(format!(
            "zero churn cycles executed (duration {duration_mins}min too short for one cycle)"
        ));
    }

    // ── Metrics ─────────────────────────────────────────────────────────────
    b.param("executed_cycles", cycles.len().into());
    b.param("cycle_failures", cycle_failures.len().into());
    if !cycle_failures.is_empty() {
        b.note(format!(
            "{} churn cycle failure(s) under BAO_SOAK_CONTINUE_ON_FAIL — \
             every failure is a per-cycle sidecar record of type \
             cycle-failure (phase timeouts are counted, not swallowed): first \
             = {:?}",
            cycle_failures.len(),
            cycle_failures.first().map(|(i, e)| format!("cycle {i}: {e}"))
        ));
    }
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

        // Engine-native heap metering (n = probes with a successful post-GC
        // engine sample; metering failures are counted, never fabricated).
        let engine_post: Vec<&EngineHeapSample> = all_probes
            .iter()
            .filter_map(|p| p.engine_post.as_ref())
            .collect();
        if !engine_post.is_empty() {
            let heap: Vec<f64> = engine_post
                .iter()
                .map(|s| s.gc_heap_chunk_total_kib)
                .collect();
            b.metric(Metric::from_samples(
                "forced_gc_engine_gc_heap_kib",
                "KiB",
                "memory",
                false,
                None,
                &heap,
            ));
            let things: Vec<f64> = engine_post
                .iter()
                .map(|s| s.gc_heap_gc_things_kib)
                .collect();
            b.metric(Metric::from_samples(
                "forced_gc_engine_gc_things_kib",
                "KiB",
                "memory",
                false,
                None,
                &things,
            ));
            let malloc: Vec<f64> = engine_post
                .iter()
                .map(|s| s.malloc_heap_kib)
                .collect();
            b.metric(Metric::from_samples(
                "forced_gc_engine_malloc_heap_kib",
                "KiB",
                "memory",
                false,
                None,
                &malloc,
            ));
            let zones: Vec<f64> = engine_post.iter().map(|s| s.zones).collect();
            b.metric(Metric::from_samples(
                "forced_gc_engine_zone_count",
                "count",
                "count",
                false,
                None,
                &zones,
            ));
            let realms: Vec<f64> = engine_post.iter().map(|s| s.realms).collect();
            b.metric(Metric::from_samples(
                "forced_gc_engine_realm_count",
                "count",
                "count",
                false,
                None,
                &realms,
            ));

            // Engine-level GC reclamation: live-GC-things bytes freed by the
            // forced GC (probes with BOTH samples; a probe missing either
            // side is excluded, not zero-filled).
            let drops: Vec<f64> = all_probes
                .iter()
                .filter_map(|p| match (p.engine_pre, p.engine_post) {
                    (Some(pre), Some(post)) => {
                        Some(pre.gc_heap_gc_things_kib - post.gc_heap_gc_things_kib)
                    }
                    _ => None,
                })
                .collect();
            if !drops.is_empty() {
                b.metric(Metric::from_samples(
                    "forced_gc_engine_gc_things_drop_kib",
                    "KiB",
                    "memory",
                    false,
                    None,
                    &drops,
                ));
            }
        }
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
        "forced GC (Bun.gc x2, GCOptions::Shrink non-incremental) covers the long-lived probe page's JSRuntime only — each JSContext owns its JSRuntime and churn-page heaps are reclaimed by ScriptThread exit on close; post-GC RSS delta is therefore a lower bound on reclaimable memory",
    );
    b.note(
        "engine metering (forced_gc_engine_*) is JS::CollectRuntimeStats on the SAME probe-page runtime the forced GC covers — engine-level GC-heap/live-things/malloc numbers, not process totals",
    );
    if !engine_metering_failures.is_empty() {
        b.note(format!(
            "{} engine-metering failure(s) — engine heap fields are degraded for those probes, not fabricated: {:?}",
            engine_metering_failures.len(),
            engine_metering_failures
        ));
    }
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

// ═══════════════════════════════════════════════════════════════════════════
// mixed scenario (#19-F / Gate D) — 7 workload classes truly interleaved on
// ONE runtime thread.
//
// Concurrency semantics (honest, per the W21 design §0.1): BrowserRuntime is
// !Send (`Servo(Rc<ServoInner>)`), so "mixed" means long-lived pages at
// different lifecycle stages interleaved on a single runtime thread — a
// browser tab-farm, NOT OS-thread concurrency. Never reported as parallelism.
//
// Mixedness is mechanically adjudicated per cycle (design §1.4):
//   - class_transitions ≥ 2×class_count (default ≥14) — adjacent ops from
//     different classes; round-robin expansion makes this structural, the
//     counter is the post-hoc proof.
//   - overlap_events ≥ 1 — ≥1 sync op (fs/sqlite/web_eval/cdp) executed while
//     ≥1 fetch/timer was armed in flight; 0 = serial concatenation.
// Violations degrade the verdict (counted + noted), never silently ignored.
//
// Fail-closed (design §1.5): any op verification failure aborts the cycle
// (default) or is counted under BAO_SOAK_CONTINUE_ON_FAIL, with the failing
// class named in the sidecar record.
//
// First-day probe (design §1.8 risk mitigation): before any load is laid
// down, a 1-page fetch/timer minimal closed loop proves the spin_event_loop
// pump actually progresses Page-realm promises and both timer realms. Probe
// failure = abort before the churn starts (no green numbers on an
// unprogressable pump).
// ═══════════════════════════════════════════════════════════════════════════

use bao_browser::handle_bridge_command;
use bao_cdp::BridgeCommand;

const MIXED_CLASSES: [&str; 7] = ["browser", "web_eval", "fetch", "fs", "sqlite", "timers", "cdp"];
const CLS_BROWSER: usize = 0;
const CLS_WEB_EVAL: usize = 1;
const CLS_FETCH: usize = 2;
const CLS_FS: usize = 3;
const CLS_SQLITE: usize = 4;
const CLS_TIMERS: usize = 5;
const CLS_CDP: usize = 6;
/// fetch + timers arm async completions; the pump phase settles them.
const CLASS_IS_ASYNC: [bool; 7] = [false, false, true, false, false, true, false];

#[derive(Default, Clone, Copy)]
struct MixedCounts([u64; 7]);

impl MixedCounts {
    fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (i, name) in MIXED_CLASSES.iter().enumerate() {
            m.insert((*name).to_string(), self.0[i].into());
        }
        serde_json::Value::Object(m)
    }
}

/// The servo bridge stringifies eval results; strip defensive quotes so the
/// numeric/string markers compare cleanly.
fn strip_quotes(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// One armed async op awaiting pump settlement.
struct ArmedOp {
    class: usize,
    pages_idx: usize,
    /// globalThis key holding the settlement flag (0=armed,1=ok,2+=failure).
    key: String,
    seq: usize,
}

struct MixedOutcome {
    cycle_ms: f64,
    counts: MixedCounts,
    class_ms: [f64; 7],
    transitions: usize,
    overlap_events: usize,
}

/// Expand the per-class budgets into a round-robin op sequence: each pass
/// takes one op from every class with remaining budget, so adjacent ops come
/// from different classes by construction (the transitions counter is the
/// mechanical proof of that, not the assumption).
fn expand_round_robin(budget: &[usize; 7], cycle_i: usize, pages_n: usize) -> Vec<(usize, usize)> {
    let mut remaining = *budget;
    let mut seq = Vec::new();
    let mut op_seq = 0usize;
    while remaining.iter().any(|&r| r > 0) {
        for class in 0..7 {
            if remaining[class] == 0 {
                continue;
            }
            remaining[class] -= 1;
            // Long-lived pages rotate per class position AND per cycle so a
            // given page sees every class across cycles.
            let pages_idx = (op_seq + cycle_i) % pages_n.max(1);
            seq.push((class, pages_idx));
            op_seq += 1;
        }
    }
    seq
}

/// First-day probe (design §1.8): one page, one fetch + two timer chains —
/// proves `spin_event_loop` progresses the Page-realm fetch promise, the
/// Web-realm timer chain (depth 3) and the Node-realm timer before any load.
/// RED shape: without a working pump the flags stay 0 and this times out —
/// the run then aborts with reason=probe-failure instead of laying down load.
/// Returns `(fetch_ms, web_timer_ms, node_timer_ms, node_timer_ok)`. The
/// first two are FAIL-CLOSED (they are the design's primary pump risk); the
/// Node-realm timer is a NON-BLOCKING probe: the page Node realm's
/// setTimeout registers in the ScriptThread's thread-local BAO_REGISTRY,
/// drained only by the pump_embedder_thread bridge on that thread's own
/// handle_msgs wake — no main-thread primitive ticks it on a quiet page
/// (servo spin, main-thread bun tick and no-op-eval wakes all verified
/// inert). That is recorded honestly and the mixed timers class stays
/// Web-realm only (its ops are all really verified), never faked green.
fn probe_async_pump(
    runtime: &BrowserRuntime,
    page: &PageHandle,
    base_url: &str,
    body_len: usize,
) -> Result<(f64, f64, f64, bool), String> {
    // ① Page-realm fetch (arm → pump → read back).
    let t0 = Instant::now();
    page.evaluate_js_web(&format!(
        "globalThis.__probe_f=0;fetch('{base_url}/probe').then(function(r){{return r.text().then(function(t){{globalThis.__probe_f=(r.status===200&&t.length==={body_len})?1:2;}});}},function(e){{globalThis.__probe_f=3;}});'armed'"
    ))
    .map_err(|e| format!("probe fetch arm failed: {e}"))?;
    let mut fetch_flag = String::from("0");
    while t0.elapsed().as_secs() < 10 {
        runtime.spin_event_loop();
        bun_runtime::timers::with_event_loop(|l| l.tick_without_idle(std::ptr::null_mut()));
        std::thread::sleep(Duration::from_millis(2));
        fetch_flag = strip_quotes(
            &page
                .evaluate_js_web("String(globalThis.__probe_f)")
                .map_err(|e| format!("probe fetch poll failed: {e}"))?,
        );
        if fetch_flag != "0" {
            break;
        }
    }
    let fetch_ms = t0.elapsed().as_secs_f64() * 1e3;
    match fetch_flag.as_str() {
        "1" => {}
        "2" => return Err("probe fetch: status/body verification failed (pump progressed, content wrong)".into()),
        "3" => return Err("probe fetch: page-realm fetch() REJECTED (pump progressed; CORS or stack failure — see servo log)".into()),
        _ => return Err(format!(
            "probe fetch: flag stuck at {fetch_flag:?} after 10s — spin_event_loop pump does NOT progress the page-realm fetch promise (design risk realized; aborting before load)"
        )),
    }

    // ② Web-realm timer chain, depth 3.
    let t1 = Instant::now();
    page.evaluate_js_web(
        "globalThis.__probe_wt=0;setTimeout(function(){setTimeout(function(){setTimeout(function(){globalThis.__probe_wt=1;},10);},10);},10);'armed'",
    )
    .map_err(|e| format!("probe web timer arm failed: {e}"))?;
    let mut wt_flag = String::from("0");
    while t1.elapsed().as_secs() < 10 {
        runtime.spin_event_loop();
        bun_runtime::timers::with_event_loop(|l| l.tick_without_idle(std::ptr::null_mut()));
        std::thread::sleep(Duration::from_millis(2));
        wt_flag = strip_quotes(
            &page
                .evaluate_js_web("String(globalThis.__probe_wt)")
                .map_err(|e| format!("probe web timer poll failed: {e}"))?,
        );
        if wt_flag != "0" {
            break;
        }
    }
    let web_timer_ms = t1.elapsed().as_secs_f64() * 1e3;
    if wt_flag != "1" {
        return Err(format!(
            "probe web timer: flag {wt_flag:?} after 10s — pump does NOT progress the Web-realm setTimeout chain (aborting before load)"
        ));
    }

    // ③ Node-realm timer (evaluate_js realm). The bun-timer queue is NOT
    // driven by the servo spin — the embedder pump primitive for it is
    // `timers::with_event_loop(tick_without_idle)` (the fetch_bench
    // embedder-pumped shape). The poll rides a short eval timeout with
    // spin-and-retry so a single quiet-page race cannot read as a dead pump.
    let t2 = Instant::now();
    page.evaluate_js("globalThis.__probe_nt=0;setTimeout(function(){globalThis.__probe_nt=1;},10);'armed'")
        .map_err(|e| format!("probe node timer arm failed: {e}"))?;
    // Bounded probe: 10 passes of (spin + bun tick + no-op drain wake +
    // short poll). The wake and the poll ride SHORT eval timeouts — a timed
    // out evaluate arms the SM interrupt, and a residual interrupt turns
    // every later eval into InternalError, so the probe must NOT spam
    // timeouts on a quiet page; it walks away after 10 passes instead.
    let mut nt_flag = String::from("0");
    for _ in 0..10 {
        runtime.spin_event_loop();
        bun_runtime::timers::with_event_loop(|l| l.tick_without_idle(std::ptr::null_mut()));
        // Best-effort wake of the hosting ScriptThread (its handle_msgs wake
        // runs the pump_embedder_thread bridge that drains BAO_REGISTRY).
        let _ = page.evaluate_js_web(";");
        std::thread::sleep(Duration::from_millis(50));
        match page.evaluate_js_with_timeout("String(globalThis.__probe_nt)", Some(Duration::from_millis(250))) {
            Ok(v) => {
                nt_flag = strip_quotes(&v);
                if nt_flag != "0" {
                    break;
                }
            }
            Err(_) => break, // a poll error here means the page is unhealthy — walk away
        }
    }
    let node_timer_ms = t2.elapsed().as_secs_f64() * 1e3;
    let node_timer_ok = nt_flag == "1";
    Ok((fetch_ms, web_timer_ms, node_timer_ms, node_timer_ok))
}

/// One mixed cycle: the round-robin op sequence executed with async classes
/// armed mid-sequence and settled by the pump phase afterwards.
#[allow(clippy::too_many_arguments)]
fn mixed_cycle(
    runtime: &BrowserRuntime,
    pages: &[PageHandle],
    scratch: &str,
    base_url: &str,
    body_len: usize,
    budget: &[usize; 7],
    cycle_i: usize,
    arm_timeout: Duration,
) -> Result<MixedOutcome, String> {
    let t_cycle = Instant::now();
    let seq = expand_round_robin(budget, cycle_i, pages.len());
    let mut counts = MixedCounts::default();
    let mut class_ms = [0f64; 7];
    let mut transitions = 0usize;
    let mut overlap_events = 0usize;
    let mut armed: Vec<ArmedOp> = Vec::new();
    let mut async_in_flight = 0usize;
    let mut op_seq = 0usize;

    for &(class, pages_idx) in &seq {
        if op_seq > 0 && class != seq[op_seq - 1].0 {
            transitions += 1;
        }
        op_seq += 1;
        let page = &pages[pages_idx];
        let marker = format!("m{cycle_i}_{op_seq}");
        let t0 = Instant::now();
        match class {
            CLS_BROWSER => {
                // The shared churn shape (byte-identical semantics with the
                // page-churn bench — keeps numbers comparable).
                churn_cycle(runtime, cycle_i, None)
                    .map_err(|e| format!("[browser] {e}"))?;
            }
            CLS_WEB_EVAL => {
                let out = page
                    .evaluate_js_web(&format!(
                        "(function(){{var k='__we{marker}';var v=(globalThis[k]||0)+1;globalThis[k]=v;\
                         document.body.setAttribute('data-mixed','{marker}');\
                         return document.body.getAttribute('data-mixed')==='{marker}'?'ok':'bad';}})()",
                    ))
                    .map_err(|e| format!("[web_eval] {e}"))?;
                if strip_quotes(&out) != "ok" {
                    return Err(format!("[web_eval] DOM round-trip verification failed (got {out:?})"));
                }
            }
            CLS_FETCH => {
                let key = format!("__f{marker}");
                page.evaluate_js_web(&format!(
                    "globalThis.{key}=0;fetch('{base_url}/{marker}').then(function(r){{return r.text().then(function(t){{globalThis.{key}=(r.status===200&&t.length==={body_len})?1:2;}});}},function(e){{globalThis.{key}=3;}});'armed'",
                ))
                .map_err(|e| format!("[fetch] arm failed: {e}"))?;
                armed.push(ArmedOp { class, pages_idx, key, seq: op_seq });
                async_in_flight += 1;
            }
            CLS_FS => {
                let path = format!("{scratch}/{marker}.txt");
                let out = page
                    .evaluate_js(&format!(
                        "(function(){{var fs=require('fs');var p='{path}';\
                         fs.writeFileSync(p,'hello-mixed');var r1=fs.readFileSync(p,'utf8');\
                         fs.appendFileSync(p,'-append');var r2=fs.readFileSync(p,'utf8');\
                         var st=fs.statSync(p);fs.unlinkSync(p);var gone=!fs.existsSync(p);\
                         return (r1==='hello-mixed'&&r2==='hello-mixed-append'&&st.size===18&&gone)?'ok':'bad:'+r1+':'+r2+':'+st.size+':'+gone;}})()",
                    ))
                    .map_err(|e| format!("[fs] page#{pages_idx} {e}"))?;
                if strip_quotes(&out) != "ok" {
                    return Err(format!("[fs] write/read-back/append/stat/delete verification failed (got {out:?})"));
                }
            }
            CLS_SQLITE => {
                let path = format!("{scratch}/{marker}.db");
                let out = page
                    .evaluate_js(&format!(
                        "(function(){{var {{Database}}=require('bun:sqlite');var db=new Database('{path}');\
                         db.exec('CREATE TABLE t(a INTEGER)');\
                         var ins=db.prepare('INSERT INTO t VALUES (?)');\
                         for(var i=0;i<50;i++){{ins.run(i);}}\
                         var c=db.prepare('SELECT COUNT(*) AS c FROM t').get().c;\
                         db.exec('DROP TABLE t');db.close();\
                         return c===50?'ok':'bad:'+c;}})()",
                    ))
                    .map_err(|e| format!("[sqlite] page#{pages_idx} {e}"))?;
                if strip_quotes(&out) != "ok" {
                    return Err(format!("[sqlite] 50-row insert/count verification failed (got {out:?})"));
                }
            }
            CLS_TIMERS => {
                // Web-realm ONLY (see the probe note): the page Node realm's
                // setTimeout is pump-unreachable from the bench thread, so
                // arming it would fake load. The Web realm chain rides servo's
                // TimerScheduler and is fully verified on settlement.
                let key = format!("__t{marker}");
                page.evaluate_js_web(&format!(
                    "globalThis.{key}=0;setTimeout(function(){{setTimeout(function(){{setTimeout(function(){{globalThis.{key}=1;}},10);}},10);}},10);'armed'",
                ))
                .map_err(|e| format!("[timers] web arm failed: {e}"))?;
                armed.push(ArmedOp { class, pages_idx, key, seq: op_seq });
                async_in_flight += 1;
            }
            CLS_CDP => {
                // The production CDP command face: EvaluateJs through the real
                // bridge dispatch (same layer the WS server drains).
                let resp = handle_bridge_command(
                    BridgeCommand::EvaluateJs {
                        target_id: page.id().to_string(),
                        expression: format!("(function(){{return '{marker}';}})()"),
                        return_by_value: true,
                    },
                    runtime.page_pool(),
                );
                let value = resp
                    .result
                    .map_err(|e| format!("[cdp] bridge error: {e}"))?;
                let got = value
                    .get("result")
                    .and_then(|r| r.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("<missing>");
                if got != marker {
                    return Err(format!("[cdp] evaluate round-trip verification failed (got {got:?}, want {marker:?})"));
                }
            }
            _ => unreachable!("class index out of range"),
        }
        class_ms[class] += t0.elapsed().as_secs_f64() * 1e3;
        counts.0[class] += 1;
        // Overlap telemetry: a sync op executed while an async op is armed —
        // the mechanical witness that this is NOT serial concatenation.
        if !CLASS_IS_ASYNC[class] && async_in_flight > 0 {
            overlap_events += 1;
        }
    }

    // ── Pump phase: settle every armed op (spin + bun-timer tick + read-back)
    // Two pump primitives: the servo spin drives Page-realm promises/timers;
    // `timers::with_event_loop` drives the Node realm's bun-timer queue (the
    // fetch_bench embedder-pumped shape). Polls ride short eval timeouts with
    // a bounded consecutive-failure budget — a single quiet-page dispatch race
    // must not read as a dead pump, but a persistent one still fails closed.
    let t_pump = Instant::now();
    let mut settle_error: Option<String> = None;
    let mut poll_fail_streak = 0usize;
    while !armed.is_empty() && settle_error.is_none() && t_pump.elapsed() < arm_timeout {
        runtime.spin_event_loop();
        bun_runtime::timers::with_event_loop(|l| l.tick_without_idle(std::ptr::null_mut()));
        // Wake each hosting ScriptThread so its pump_embedder_thread drains
        // due Node-realm timers (see the probe note — a quiet page is never
        // woken by the main-thread spins alone). No timeout: a timed-out
        // evaluate arms the SM interrupt and poisons the page's later evals.
        for page in pages.iter() {
            let _ = page.evaluate_js_web(";");
        }
        std::thread::sleep(Duration::from_millis(1));
        let mut still_armed = Vec::with_capacity(armed.len());
        for op in armed.drain(..) {
            let page = &pages[op.pages_idx];
            // The timers class alternates realms by arm parity — the read-back
            // must use the same realm the arm wrote the flag in.
            // No timeout on the read-back: a timed-out evaluate arms the SM
            // interrupt (residual interrupts turn later evals into
            // InternalError). evaluate_js_web's own spin_servo bounds it.
            let probe = page.evaluate_js_web(&format!("String(globalThis.{})", op.key));
            match probe.map(|v| strip_quotes(&v)) {
                Ok(flag) if flag == "1" => {
                    poll_fail_streak = 0;
                } // settled ok — drop from armed set
                Ok(flag) if flag != "0" => {
                    // Any non-0/1 flag is the JS-side failure encoding
                    // (fetch: 2=content mismatch, 3=rejected).
                    settle_error = Some(format!(
                        "[{}] armed op seq {} settled with failure flag {flag:?} (key {})",
                        MIXED_CLASSES[op.class],
                        op.seq,
                        op.key
                    ));
                }
                Ok(_) => {
                    poll_fail_streak = 0;
                    still_armed.push(op) // still armed — keep pumping
                }
                Err(e) => {
                    poll_fail_streak += 1;
                    let (cls, seq) = (op.class, op.seq);
                    still_armed.push(op);
                    if poll_fail_streak >= 2 {
                        settle_error =
                            Some(format!("[{}] settle poll failed {}x consecutively (seq {}): {e}", MIXED_CLASSES[cls], poll_fail_streak, seq));
                    }
                }
            }
            if settle_error.is_some() {
                break;
            }
        }
        armed = still_armed;
    }
    let pump_ms = t_pump.elapsed().as_secs_f64() * 1e3;
    class_ms[CLS_FETCH] += pump_ms / 2.0;
    class_ms[CLS_TIMERS] += pump_ms / 2.0;
    if let Some(e) = settle_error {
        return Err(e);
    }
    if !armed.is_empty() {
        let stuck: Vec<String> = armed
            .iter()
            .map(|op| format!("{}#{} key={}", MIXED_CLASSES[op.class], op.seq, op.key))
            .collect();
        return Err(format!(
            "[pump] {} armed op(s) did not settle within {arm_timeout:?} — spin_event_loop pump stalled or the async chains died: {stuck:?}",
            armed.len()
        ));
    }

    Ok(MixedOutcome {
        cycle_ms: t_cycle.elapsed().as_secs_f64() * 1e3,
        counts,
        class_ms,
        transitions,
        overlap_events,
    })
}

/// mixed scenario driver — see the module docs above.
fn run_mixed(p: &Params, out_path: Option<&str>) -> Result<ResultBuilder, String> {
    let duration_mins = p.u64_of("duration-mins", 60);
    let segment_mins = p.u64_of("segment-mins", 10);
    let post_mins = p.u64_of("post-mins", 2);
    let interval_ms = p.u64_of("interval-ms", 1000);
    let gc_settle_ms = p.u64_of("gc-settle-ms", 3000);
    let settle_ms = p.u64_of("settle-ms", 50);
    let pages_n = p.usize_of("mixed-pages", 4).max(1);
    let budget = [
        // browser: 1 churn page per cycle (comparability anchor); 0 disables
        // the class — the churn-interaction A/B knob (see the node-realm
        // registry finding in the run notes).
        p.usize_of("mixed-browser", 1),
        p.usize_of("mixed-web-eval", 8),
        p.usize_of("mixed-fetch", 4),
        p.usize_of("mixed-fs", 8),
        p.usize_of("mixed-sqlite", 2),
        p.usize_of("mixed-timers", 4),
        p.usize_of("mixed-cdp", 4),
    ];
    let arm_timeout = Duration::from_millis(p.u64_of("mixed-arm-timeout-ms", 15_000));
    let continue_on_fail = std::env::var("BAO_SOAK_CONTINUE_ON_FAIL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let out_path = out_path.ok_or(
        "soak requires --out: the per-cycle series streams to a sidecar derived from the result path",
    )?;
    let sidecar = sidecar_path(out_path);
    let mut sc = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sidecar)
        .map_err(|e| format!("cannot open sidecar {sidecar}: {e}"))?;

    let mut b = ResultBuilder::new("soak");
    b.param("scenario", "mixed".into());
    b.param("duration_mins", duration_mins.into());
    b.param("segment_mins", segment_mins.into());
    b.param("post_mins", post_mins.into());
    b.param("interval_ms", interval_ms.into());
    b.param("gc_settle_ms", gc_settle_ms.into());
    for (i, name) in MIXED_CLASSES.iter().enumerate() {
        b.param(format!("mixed_{name}_per_cycle").as_str(), budget[i].into());
    }
    b.param("mixed_pages", pages_n.into());
    b.note(format!(
        "mixed concurrency semantics: BrowserRuntime is !Send — N long-lived pages interleaved on ONE runtime thread (tab-farm), not OS-thread parallelism"
    ));

    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        return Err("DISPLAY not set — browser soak must run under xvfb-run (bench/METHODOLOGY.md)".into());
    }

    // ── Cold: runtime + probe page + long-lived mixed pages ────────────────
    let t0 = Instant::now();
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .map_err(|e| format!("BrowserRuntime::new failed: {e}"))?;
    b.metric(Metric::single(
        "browser_runtime_init",
        "ms",
        "latency",
        false,
        Some("cold"),
        t0.elapsed().as_secs_f64() * 1e3,
    ));

    let probe_page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .map_err(|e| format!("probe create_page failed: {e}"))?;
    probe_page
        .wait_for_pipeline_ready(Duration::from_secs(15))
        .map_err(|e| format!("probe pipeline not ready: {e}"))?;
    let gc_type = probe_page
        .evaluate_js("typeof Bun.gc")
        .map_err(|e| format!("probe Node Realm check failed: {e}"))?;
    if !gc_type.contains("function") {
        return Err(format!("Bun.gc is not a function in the probe Node Realm (got {gc_type:?})"));
    }
    let probe_id = probe_page.id();

    let mut pages: Vec<PageHandle> = Vec::with_capacity(pages_n);
    for i in 0..pages_n {
        let page = runtime
            .create_page(&PageConfig {
                url: Some("about:blank".into()),
                ..Default::default()
            })
            .map_err(|e| format!("mixed page {i} create failed: {e}"))?;
        page.wait_for_pipeline_ready(Duration::from_secs(15))
            .map_err(|e| format!("mixed page {i} pipeline not ready: {e}"))?;
        // Fail-closed startup diagnosis: every long-lived page must have BOTH
        // realms live before the churn starts (the fs/sqlite classes dispatch
        // into the Node realm on rotation — a missing eager init must abort
        // with the page named here, not mid-cycle as a bare null-realm error).
        let gc_type = page
            .evaluate_js("typeof Bun.gc")
            .map_err(|e| format!("mixed page {i} Node Realm check failed: {e}"))?;
        if !gc_type.contains("function") {
            return Err(format!("mixed page {i}: Bun.gc is not a function (got {gc_type:?}) — Node Realm not usable"));
        }
        pages.push(page);
    }

    // ── Local loopback server + scratch dir ─────────────────────────────────
    let body = "M".repeat(256);
    let body_len = body.len();
    let (port, _served) = crate::fetch_bench::spawn_server_with(&body, true)
        .map_err(|e| format!("mixed loopback server spawn failed: {e}"))?;
    let base_url = format!("http://127.0.0.1:{port}");
    let scratch = std::env::var("BAO_SOAK_TMP")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::temp_dir().join("bao-soak-mixed").display().to_string());
    // Crash-safe: clear at start, keep at end (sidecar philosophy — a later
    // run can inspect what a killed run left behind).
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)
        .map_err(|e| format!("cannot create scratch dir {scratch}: {e}"))?;
    b.param("scratch_dir", scratch.clone().into());
    b.param("loopback", format!("127.0.0.1:{port} (ACAO:* — page-realm fetch contract)").into());

    // ── First-day probe: pump semantics before any load ─────────────────────
    match probe_async_pump(&runtime, &pages[0], &base_url, body_len) {
        Ok((fetch_ms, web_ms, node_ms, node_ok)) => {
            write_rec(
                &mut sc,
                json!({
                    "type": "async-pump-probe", "t_ms": 0.0,
                    "fetch_ok": true, "fetch_ms": fetch_ms,
                    "web_timer_ok": true, "web_timer_ms": web_ms,
                    "node_timer_ok": node_ok, "node_timer_ms": node_ms,
                }),
            )?;
            b.note(format!(
                "async pump probe GREEN: page-realm fetch settled in {fetch_ms:.0}ms, web timer chain {web_ms:.0}ms"
            ));
            if !node_ok {
                b.note(format!(
                    "Node-realm timer probe RED (non-blocking, {node_ms:.0}ms): the page Node realm's setTimeout                      registers in the ScriptThread's thread-local BAO_REGISTRY, drained only by the                      pump_embedder_thread bridge on that thread's own handle_msgs wake — no main-thread                      primitive ticks it on a quiet page (servo spin / main-thread bun tick / no-op-eval wake                      all verified inert). The mixed timers class is therefore Web-realm ONLY; its ops are                      all really verified. Product-code driver-chain finding, recorded not faked."
                ));
            }
        }
        Err(e) => {
            let _ = write_rec(
                &mut sc,
                json!({"type": "async-pump-probe", "t_ms": 0.0, "ok": false, "error": e}),
            );
            let _ = write_rec(
                &mut sc,
                json!({"type": "soak-end", "t_ms": 0.0, "reason": "probe-failure"}),
            );
            return Err(format!("mixed soak aborted at the async-pump probe (no load laid down): {e}"));
        }
    }

    // ── Mixed cycle loop with segment boundaries ────────────────────────────
    let start = Instant::now();
    let now_ms = || start.elapsed().as_secs_f64() * 1e3;
    let duration_secs = duration_mins * 60;
    let segment_secs = segment_mins * 60;
    let mut total_counts = MixedCounts::default();
    let mut total_class_ms = [0f64; 7];
    let mut per_class_samples: [Vec<f64>; 7] = Default::default();
    let mut cycle_wall: Vec<f64> = Vec::new();
    let mut transitions_min: Option<usize> = None;
    let mut overlap_total = 0usize;
    let mut mixedness_violations = 0usize;
    let mut cycle_failures: Vec<String> = Vec::new();
    let mut seg_counts_start = total_counts;
    let mut segment_idx = 0usize;
    let mut next_boundary = segment_secs;
    let mut steady_started = false;
    let mut steady_series: Vec<(f64, f64)> = Vec::new(); // (t_s, rss)
    let mut i = 0usize;

    while start.elapsed().as_secs() < duration_secs {
        let t_rss_pre = common::vm_rss_kib().unwrap_or(0) as f64;
        match mixed_cycle(
            &runtime, &pages, &scratch, &base_url, body_len, &budget, i, arm_timeout,
        ) {
            Ok(outcome) => {
                let rec = json!({
                    "type": "cycle", "i": i, "t_ms": now_ms(),
                    "cycle_ms": outcome.cycle_ms,
                    "rss_kib": t_rss_pre,
                    "hwm_kib": common::vm_hwm_kib().unwrap_or(0) as f64,
                    "fd": common::fd_count().unwrap_or(0) as f64,
                    "threads": common::thread_count().unwrap_or(0) as f64,
                    "class_counts": outcome.counts.to_json(),
                    "class_ms": {
                        "browser": outcome.class_ms[0], "web_eval": outcome.class_ms[1],
                        "fetch": outcome.class_ms[2], "fs": outcome.class_ms[3],
                        "sqlite": outcome.class_ms[4], "timers": outcome.class_ms[5],
                        "cdp": outcome.class_ms[6],
                    },
                    "class_transitions": outcome.transitions,
                    "overlap_events": outcome.overlap_events,
                });
                write_rec(&mut sc, rec)?;
                for c in 0..7 {
                    if outcome.counts.0[c] > 0 {
                        per_class_samples[c].push(outcome.class_ms[c] / outcome.counts.0[c] as f64);
                    }
                    total_counts.0[c] += outcome.counts.0[c];
                    total_class_ms[c] += outcome.class_ms[c];
                }
                cycle_wall.push(outcome.cycle_ms);
                transitions_min = Some(transitions_min.map_or(outcome.transitions, |m: usize| m.min(outcome.transitions)));
                overlap_total += outcome.overlap_events;
                if outcome.transitions < 2 * MIXED_CLASSES.len() || outcome.overlap_events == 0 {
                    mixedness_violations += 1;
                    let _ = write_rec(
                        &mut sc,
                        json!({"type": "mixedness-violation", "i": i,
                               "transitions": outcome.transitions,
                               "overlap_events": outcome.overlap_events}),
                    );
                }
                steady_series.push((now_ms() / 1000.0, t_rss_pre));
                if !steady_started && start.elapsed().as_secs() >= segment_secs {
                    steady_started = true; // exclude segment 0 from the steady slope
                }
            }
            Err(e) => {
                let class = e.split(']').next().unwrap_or("?").trim_start_matches('[').to_string();
                let _ = write_rec(
                    &mut sc,
                    json!({"type": "cycle-failure", "i": i, "t_ms": now_ms(), "class": class, "error": e}),
                );
                if continue_on_fail {
                    cycle_failures.push(e);
                    i += 1;
                    continue;
                }
                let _ = write_rec(
                    &mut sc,
                    json!({"type": "soak-end", "t_ms": now_ms(), "reason": "cycle-failure", "error": e}),
                );
                return Err(format!(
                    "mixed soak aborted at cycle {i}: {e} — series preserved in {sidecar}"
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(settle_ms));

        if start.elapsed().as_secs() >= next_boundary {
            let seg_counts: serde_json::Map<String, serde_json::Value> = MIXED_CLASSES
                .iter()
                .enumerate()
                .map(|(c, n)| {
                    ((*n).to_string(), serde_json::Value::from(total_counts.0[c] - seg_counts_start.0[c]))
                })
                .collect();
            write_rec(
                &mut sc,
                json!({
                    "type": "segment", "segment": segment_idx,
                    "t_ms": now_ms(),
                    "class_ops": serde_json::Value::Object(seg_counts),
                }),
            )?;
            seg_counts_start = total_counts;
            segment_idx += 1;
            next_boundary += segment_secs;
        }
        i += 1;
    }

    // ── Final forced-GC probe + post-idle (shared shape, zero change) ───────
    let final_probe = match forced_gc_probe(&probe_page, gc_settle_ms) {
        Ok((mut pr, _)) => {
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
            write_rec(
                &mut sc,
                json!({"type": "gc-probe", "t_ms": now_ms(), "segment": null, "ok": false, "error": e}),
            )?;
            None
        }
    };

    let mut post_rss: Vec<(f64, f64)> = Vec::new();
    let post_deadline = Duration::from_secs(post_mins * 60);
    let post_start = Instant::now();
    while post_start.elapsed() < post_deadline {
        std::thread::sleep(Duration::from_millis(interval_ms));
        let t_ms = now_ms();
        let rss = common::vm_rss_kib().unwrap_or(0) as f64;
        write_rec(
            &mut sc,
            json!({"type": "post", "t_ms": t_ms, "rss_kib": rss,
                   "fd": common::fd_count().unwrap_or(0) as f64,
                   "threads": common::thread_count().unwrap_or(0) as f64}),
        )?;
        post_rss.push((t_ms / 1000.0, rss));
    }

    if let Err(e) = runtime.page_pool().close_page(probe_id) {
        b.note(format!("probe page close failed at soak end: {e}"));
    }

    let total_ops: u64 = total_counts.0.iter().sum();
    write_rec(
        &mut sc,
        json!({
            "type": "soak-end", "t_ms": now_ms(), "reason": "duration-reached",
            "cycles": i, "segments": segment_idx, "total_ops": total_ops,
            "cycle_failures": cycle_failures.len(),
            "mixedness_violations": mixedness_violations,
        }),
    )?;

    if i == 0 {
        return Err(format!(
            "zero mixed cycles executed (duration {duration_mins}min too short for one cycle)"
        ));
    }

    // ── Metrics ─────────────────────────────────────────────────────────────
    b.param("executed_cycles", i.into());
    b.param("cycle_failures", cycle_failures.len().into());
    b.param("total_ops", total_ops.into());
    b.param("mixedness_violations", mixedness_violations.into());
    b.param("overlap_events", overlap_total.into());
    b.param("full_segments", segment_idx.into());
    b.param("forced_gc_probes_ok", usize::from(final_probe.is_some()).into());
    if let Some(m) = transitions_min {
        b.metric(Metric::single(
            "mixedness_transitions_min",
            "count",
            "count",
            false,
            None,
            m as f64,
        ));
    }
    if !cycle_wall.is_empty() {
        b.metric(Metric::from_samples("churn_cycle", "ms", "latency", false, Some("warm"), &cycle_wall));
    }
    let span_s = start.elapsed().as_secs_f64().max(1e-9);
    for (c, name) in MIXED_CLASSES.iter().enumerate() {
        if !per_class_samples[c].is_empty() {
            b.metric(Metric::from_samples(
                &format!("mixed_{name}_op"),
                "ms",
                "latency",
                false,
                Some("warm"),
                &per_class_samples[c],
            ));
            b.metric(Metric::single(
                &format!("mixed_{name}_ops_per_s"),
                "ops_per_s",
                "throughput",
                true,
                None,
                total_counts.0[c] as f64 / span_s,
            ));
        }
    }
    let ts: Vec<f64> = steady_series.iter().map(|p| p.0).collect();
    let ys: Vec<f64> = steady_series.iter().map(|p| p.1).collect();
    if let Some(slope) = lsq_slope_kib_per_s(&ts, &ys) {
        b.metric(Metric::single(
            "vm_rss_slope_steady",
            "KiB_per_s",
            "memory",
            false,
            None,
            slope,
        ));
    }
    if !post_rss.is_empty() {
        let ys: Vec<f64> = post_rss.iter().map(|p| p.1).collect();
        b.metric(Metric::from_samples("vm_rss_post_idle", "KiB", "memory", false, None, &ys));
    }
    if mixedness_violations > 0 {
        b.note(format!(
            "MIXEDNESS DEGRADED: {mixedness_violations} cycle(s) violated the interleaving contract \
             (class_transitions ≥ 14 AND overlap_events ≥ 1) — verdict degraded, never silently passed"
        ));
    }
    if !cycle_failures.is_empty() {
        b.note(format!(
            "{} mixed cycle failure(s) under BAO_SOAK_CONTINUE_ON_FAIL — each is a sidecar record with the failing class named",
            cycle_failures.len()
        ));
    }
    b.note(
        "the forced-GC probe page remains the only long-lived JS-heap metering anchor; the N mixed pages are LOAD, not probes (semantics unchanged from page-churn soak)",
    );
    b.note(format!("raw mixed cycle/segment/probe series in {sidecar} (crash-safe append)"));
    Ok(b)
}
