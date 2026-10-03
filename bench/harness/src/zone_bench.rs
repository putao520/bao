//! Bench ⑧ (ISSUE #29-Zone): zone reclamation evaluation — N realm
//! create/drop cycles, engine memory stats sampled before/after.
//!
//! Realm-per-context model: each `JsContext::for_test()` creates a fresh
//! realm (and, per SM policy, a zone group for it). The question this bench
//! answers with DATA: does `zone_count`/`realm_count`/gc-heap return to the
//! baseline after N create/drop cycles (zones reclaimed with their realm) or
//! grow monotonically (churn accumulation)?
//!
//! Verdict rule (S0-4): equal-or-near-equal final vs baseline → maintain the
//! status quo (per-realm independent zones, reclaim-on-drop confirmed by
//! measurement). Monotonic growth → STOP and report (architecture ruling
//! belongs to the user/main session).

use std::time::Instant;

use bao_engine::context::JsContext;
use bao_engine::memory_stats::collect_runtime_stats;

use crate::common::{Metric, Params, ResultBuilder};

pub fn run(p: &Params) -> Result<ResultBuilder, String> {
    let iterations = p.usize_of("iterations", 100);
    // W31 chunk-residency attribution: `--gc-mode normal` reproduces the
    // W5-era retention (GCOptions::Normal + inHighFrequencyGCMode skips the
    // decommit pass, and chunk unmap lives ONLY inside that pass) so the
    // Shrink-face GREEN above is attributable to Shrink, not to churn
    // semantics. Default "shrink" = zero behavior change.
    let gc_mode = p.str_of("gc-mode", "shrink");

    let mut b = ResultBuilder::new("zone-eval");
    b.param("iterations", iterations.into());
    b.param("gc-mode", gc_mode.clone().into());

    // ── Baseline: a live context, stats collected on its cx ──────────────
    let mut base_ctx = JsContext::for_test()
        .map_err(|e| format!("baseline for_test failed: {}", e.message))?;
    let baseline = unsafe { collect_runtime_stats(base_ctx.raw_cx()) }
        .map_err(|e| format!("baseline stats failed: {e}"))?;
    b.metric(Metric::single(
        "zone_count_baseline",
        "count",
        "gauge",
        true,
        None,
        baseline.zone_count as f64,
    ));
    b.metric(Metric::single(
        "realm_count_baseline",
        "count",
        "gauge",
        true,
        None,
        baseline.realm_count as f64,
    ));
    b.metric(Metric::single(
        "gc_heap_chunk_total_baseline",
        "bytes",
        "gauge",
        true,
        None,
        baseline.gc_heap_chunk_total as f64,
    ));

    // ── N realm create/drop cycles (drop INCLUDES the baseline context so
    //    the final sample observes a world where every measured realm is
    //    gone) ─────────────────────────────────────────────────────────────
    let start = Instant::now();
    for i in 0..iterations {
        let mut ctx = JsContext::for_test()
            .map_err(|e| format!("iter {i}: for_test failed: {}", e.message))?;
        ctx.eval("var z = 41 + 1;", "<zone-bench>")
            .map_err(|e| format!("iter {i}: eval failed: {}", e.message))?;
        drop(ctx);
    }
    let cycles_ms = start.elapsed().as_secs_f64() * 1e3;
    b.metric(Metric::single(
        "cycles_total",
        "ms",
        "latency",
        false,
        None,
        cycles_ms,
    ));

    // ── Final sample: a fresh context on the same (post-churn) runtime.
    //    Two samples: pre-GC (drop-and-see — SM reclaims zones at GC, not at
    //    realm drop) and post-GC (JS_GC forces a full collection — zones of
    //    unreachable realms MUST be reclaimed here; growth past this point
    //    would be an architecture-level defect). ──────────────────────────
    let mut final_ctx = JsContext::for_test()
        .map_err(|e| format!("final for_test failed: {}", e.message))?;
    let pre_gc = unsafe { collect_runtime_stats(final_ctx.raw_cx()) }
        .map_err(|e| format!("pre-GC stats failed: {e}"))?;
    // W10-impl: Shrink-options non-incremental collection — GCOptions::Shrink
    // makes shouldDecommit() unconditionally true (chunk release) and runs the
    // purge/compact tail; GCReason::API skipped the decommit pass entirely in
    // high-frequency allocation modes (design /tmp/w10-zone-fix-design.md §4-B).
    // W31: gc-mode=normal arms the W5-era control face (plain JS_GC).
    let forced_gc = |cx: *mut mozjs::jsapi::JSContext| unsafe {
        if gc_mode == "normal" {
            mozjs::jsapi::JS_GC(cx, mozjs::jsapi::GCReason::API);
        } else {
            mozjs::jsapi::NonIncrementalGC(
                cx,
                mozjs::jsapi::GCOptions::Shrink,
                mozjs::jsapi::GCReason::API,
            );
        }
    };
    forced_gc(final_ctx.raw_cx());
    // Second sweep: SM chunk release can lag the first collection by a slice;
    // two full collections bound the honest post-GC reading.
    forced_gc(final_ctx.raw_cx());
    let final_stats = unsafe { collect_runtime_stats(final_ctx.raw_cx()) }
        .map_err(|e| format!("final stats failed: {e}"))?;
    b.metric(Metric::single(
        "servo_gc_heap_decommitted_final",
        "bytes",
        "gauge",
        true,
        None,
        final_stats.servo_gc_heap_decommitted as f64,
    ));
    b.metric(Metric::single(
        "servo_gc_heap_unused_final",
        "bytes",
        "gauge",
        true,
        None,
        final_stats.servo_gc_heap_unused as f64,
    ));
    b.metric(Metric::single(
        "zone_count_pre_gc",
        "count",
        "gauge",
        true,
        None,
        pre_gc.zone_count as f64,
    ));
    b.metric(Metric::single(
        "gc_heap_chunk_total_pre_gc",
        "bytes",
        "gauge",
        true,
        None,
        pre_gc.gc_heap_chunk_total as f64,
    ));
    b.metric(Metric::single(
        "zone_count_final",
        "count",
        "gauge",
        true,
        None,
        final_stats.zone_count as f64,
    ));
    b.metric(Metric::single(
        "realm_count_final",
        "count",
        "gauge",
        true,
        None,
        final_stats.realm_count as f64,
    ));
    b.metric(Metric::single(
        "gc_heap_chunk_total_final",
        "bytes",
        "gauge",
        true,
        None,
        final_stats.gc_heap_chunk_total as f64,
    ));

    let zone_delta = final_stats.zone_count as i64 - baseline.zone_count as i64;
    let realm_delta = final_stats.realm_count as i64 - baseline.realm_count as i64;
    let heap_delta = final_stats.gc_heap_chunk_total as i64 - baseline.gc_heap_chunk_total as i64;
    b.metric(Metric::single(
        "zone_count_delta",
        "count",
        "gauge",
        true,
        None,
        zone_delta as f64,
    ));
    b.metric(Metric::single(
        "realm_count_delta",
        "count",
        "gauge",
        true,
        None,
        realm_delta as f64,
    ));
    b.metric(Metric::single(
        "gc_heap_chunk_total_delta",
        "bytes",
        "gauge",
        true,
        None,
        heap_delta as f64,
    ));

    b.note(format!(
        "zone baseline={} final={} delta={:+}; realm baseline={} final={} delta={:+}; heap-chunk baseline={} final={} delta={:+}",
        baseline.zone_count,
        final_stats.zone_count,
        zone_delta,
        baseline.realm_count,
        final_stats.realm_count,
        realm_delta,
        baseline.gc_heap_chunk_total,
        final_stats.gc_heap_chunk_total,
        heap_delta,
    ));

    // W10-impl chunk-fallout gate (design /tmp/w10-zone-fix-design.md §4):
    // after a full collection the dead-realm chunk memory MUST fall back to
    // within 1.5× of the pre-churn baseline. Under GCReason::API the
    // decommit pass is skipped in high-frequency allocation modes and the
    // chunk total stays parked near the churn peak — this gate is the RED
    // half of the Shrink-GC RED→GREEN pair.
    // W31: gc-mode=normal EXPECTS the parked pool (that parking IS the
    // W5-era retention mechanism under attribution) — report it in the note
    // instead of failing the gate, so one command yields the A/B evidence.
    let chunk_gate = baseline.gc_heap_chunk_total.saturating_mul(3) / 2;
    if final_stats.gc_heap_chunk_total > chunk_gate {
        if gc_mode == "normal" {
            b.note(format!(
                "W31 normal-GC control: pool PARKED at final={} bytes (pre_gc={}) — upstream Normal+high-frequency GC skips the decommit pass; chunk unmap lives only in that pass (GC.cpp startDecommit/expireEmptyChunkPool). This is the W5-era retention mechanism reproduced.",
                final_stats.gc_heap_chunk_total, pre_gc.gc_heap_chunk_total
            ));
        } else {
            return Err(format!(
                "W10 chunk-fallout gate RED: gc_heap_chunk_total_final ({}) > baseline×1.5 ({}) — dead-realm chunk memory not released (reason={}; Shrink wiring required)",
                final_stats.gc_heap_chunk_total,
                chunk_gate,
                "API"
            ));
        }
    }

    Ok(b)
}
