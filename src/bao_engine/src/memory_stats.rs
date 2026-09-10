// @trace REQ-ENG-001 [entity:BaoRuntime] — engine-native memory metering
// (SM-EVOLUTION #27 裁决 6, consumed by #19 soak): `JS::CollectRuntimeStats`
// behind the vendor mozjs jsglue.cpp subclass.
//!
//! # Why a glue at all
//!
//! `JS::RuntimeStats` (js/public/MemoryMetrics.h) is C++-construct-only from
//! Rust: it carries pure-virtual `initExtraRealmStats`/`initExtraZoneStats`
//! hooks and `js::Vector` members, so bindgen's field-complete struct still
//! cannot be safely constructed (zeroing it is UB on the Vector invariants).
//! The concrete subclass + headline-number copy-out therefore live in
//! `vendor/mozjs/mozjs-sys/src/jsglue.cpp` (`BaoRuntimeStats` /
//! `BaoCollectRuntimeStats`); this side only ever sees plain numbers. The
//! `ObjectPrivateVisitor` argument is null — explicitly tolerated upstream
//! (MemoryMetrics.cpp null-`opv` guard; servo shape: private data stays 0).
//!
//! # Ownership / threading model (CLAUDE.md JSContext 铁律)
//!
//! `CollectRuntimeStats` walks the whole JSRuntime of `cx` under
//! `AutoCheckCannotGC`: it MUST run on `cx`'s owning thread with no JS
//! executing on it. The bao_browser wiring runs it inside a servo
//! ScriptThread embedder callback — drained by `handle_evaluate_javascript`
//! before the paired eval executes, i.e. at a provably quiescent point on
//! the context's home thread.
//!
//! # Status: internal experimental surface
//!
//! `#[doc(hidden)]` + NOT a stable public API commitment (bench/soak
//! observability face only — zero JS-visible product surface).

use mozjs::context::JSContext;
use mozjs::glue::BaoRuntimeStatsPOD;
use mozjs::jsapi::ServoSizes;
use mozjs::rust::wrappers2::BaoCollectRuntimeStats;

/// Headline engine memory numbers for one JSRuntime (all byte counts).
///
/// Field provenance (js/public/MemoryMetrics.h):
/// - GC-heap chunk accounting: the `RuntimeStats` `FOR_EACH_SIZE` breakdown
///   (`gcHeapChunkTotal` is the chunk-level total; `gcHeapGCThings` the
///   in-use GC-thing bytes).
/// - Zone/realm rollups: `zTotals` / `realmTotals` accumulators.
/// - `servo_*`: the `JS::ServoSizes` rollup of runtime + zone + realm
///   stats (used / unused / admin / decommitted GC heap, malloc heap,
///   non-heap code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineMemoryStats {
    /// Total GC-heap bytes in chunks (the sum of all other GC-heap values).
    pub gc_heap_chunk_total: usize,
    /// In-use GC-thing bytes (live objects/scripts/strings/... slots).
    pub gc_heap_gc_things: usize,
    /// Empty GC-thing slots within non-empty arenas (zone rollup).
    pub zone_unused_gc_things: usize,
    /// Live GC things measured on the zone pass.
    pub zone_live_gc_things: usize,
    /// Live GC things measured on the realm pass.
    pub realm_live_gc_things: usize,
    /// Number of zones in the runtime.
    pub zone_count: usize,
    /// Number of realms in the runtime.
    pub realm_count: usize,
    /// ServoSizes rollup: in-use GC heap (arena things).
    pub servo_gc_heap_used: usize,
    /// ServoSizes rollup: unused GC heap (empty chunks/arenas/slots).
    pub servo_gc_heap_unused: usize,
    /// ServoSizes rollup: GC heap administration (chunk + arena headers).
    pub servo_gc_heap_admin: usize,
    /// ServoSizes rollup: decommitted GC-heap pages.
    pub servo_gc_heap_decommitted: usize,
    /// ServoSizes rollup: malloc-heap engine allocations (atoms, script
    /// data, JIT metadata, zone/realm tables, ...).
    pub servo_malloc_heap: usize,
    /// ServoSizes rollup: non-heap engine allocations (JIT code pages,
    /// wasm code, committed nursery).
    pub servo_non_heap: usize,
}

impl EngineMemoryStats {
    /// SM's own totals identity (MemoryMetrics.cpp, `gcHeapGCThings =
    /// zTotals.sizeOfLiveGCThings() + realmTotals.sizeOfLiveGCThings()`).
    /// Exact by construction: all three numbers come from the same
    /// `RuntimeStats` object the engine finalized.
    pub fn live_gc_things_parts_sum(&self) -> usize {
        self.zone_live_gc_things + self.realm_live_gc_things
    }
}

/// Collect engine-native memory stats for `cx`'s JSRuntime.
///
/// # Safety
/// - `cx` must be a live `JSContext` owned by the CALLING thread.
/// - No JS may be executing on that thread (the runtime must be quiescent);
///   `CollectRuntimeStats` traverses the heap under `AutoCheckCannotGC`.
///
/// Fail-closed: any engine failure surfaces as `Err` — never zero-filled
/// placeholder numbers.
#[doc(hidden)]
pub unsafe fn collect_runtime_stats(cx: *mut mozjs::jsapi::JSContext) -> Result<EngineMemoryStats, String> {
    let cx_nn = match std::ptr::NonNull::new(cx) {
        Some(nn) => nn,
        None => return Err("collect_runtime_stats: null JSContext".into()),
    };
    let mut cx = JSContext::from_ptr(cx_nn);

    // Both out-params are repr(C) usize-only PODs; all-zero is a valid
    // starting state and every field is unconditionally overwritten on the
    // success path (the bool below gates the read).
    let mut servo = ServoSizes {
        gcHeapUsed: 0,
        gcHeapUnused: 0,
        gcHeapAdmin: 0,
        gcHeapDecommitted: 0,
        mallocHeap: 0,
        nonHeap: 0,
    };
    let mut pod = BaoRuntimeStatsPOD {
        gcHeapChunkTotal: 0,
        gcHeapGCThings: 0,
        zoneUnusedGcThings: 0,
        zoneLiveGcThings: 0,
        realmLiveGcThings: 0,
        zoneCount: 0,
        realmCount: 0,
    };
    // SAFETY: caller guarantees a live owner-thread context at a quiescent
    // point (see the module docs); out pointers are valid PODs above.
    if !unsafe { BaoCollectRuntimeStats(&mut cx, &mut servo, &mut pod) } {
        return Err("JS::CollectRuntimeStats failed (engine OOM or traversal error)".into());
    }
    Ok(EngineMemoryStats {
        gc_heap_chunk_total: pod.gcHeapChunkTotal,
        gc_heap_gc_things: pod.gcHeapGCThings,
        zone_unused_gc_things: pod.zoneUnusedGcThings,
        zone_live_gc_things: pod.zoneLiveGcThings,
        realm_live_gc_things: pod.realmLiveGcThings,
        zone_count: pod.zoneCount,
        realm_count: pod.realmCount,
        servo_gc_heap_used: servo.gcHeapUsed,
        servo_gc_heap_unused: servo.gcHeapUnused,
        servo_gc_heap_admin: servo.gcHeapAdmin,
        servo_gc_heap_decommitted: servo.gcHeapDecommitted,
        servo_malloc_heap: servo.mallocHeap,
        servo_non_heap: servo.nonHeap,
    })
}
