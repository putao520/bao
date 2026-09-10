// @trace TEST-ENG-001-MEMSTATS [req:REQ-ENG-001] [level:integration]
//
// SM-EVOLUTION #27 裁决 6 → #19 soak consumption: engine-native memory
// metering (JS::CollectRuntimeStats behind the vendor mozjs jsglue glue).
//
// All SpiderMonkey-dependent assertions run within a single #[test] function
// (ENGINE_HANDLE is thread-local; same discipline as execution_control_tests).
//
// Contract under test:
//   1. A live context with a real heap load reports non-zero, non-trivial
//      numbers on every headline field — no field silently stuck at 0
//      (a zero would mean a broken copy-out, not an empty engine).
//   2. SM's own totals identity holds EXACTLY:
//      gcHeapGCThings == zTotals.sizeOfLiveGCThings()
//                        + realmTotals.sizeOfLiveGCThings()
//      (MemoryMetrics.cpp computes the total from exactly these parts —
//      all three copied from the same finalized RuntimeStats object).
//   3. The GC-heap breakdown stays chunk-consistent: chunkTotal covers
//      decommitted + unused + admin + in-use parts (SM's own
//      sanity-checking relation, up to chunk-size rounding).

use bao_engine::context::JsContext;
use bao_engine::memory_stats::collect_runtime_stats;

fn test_01_non_zero_metering(cx: &mut JsContext) {
    // Real heap load: ~30k live strings + arrays rooted on the global so
    // they are trivially reachable when the stats pass walks the heap.
    cx.eval(
        "globalThis.__memstats_sink = []; \
         for (let i = 0; i < 30000; i++) { __memstats_sink.push('payload-' + i + '-' + (i * 7)); }",
        "memstats_load.js",
    )
    .expect("heap-load eval must succeed");

    // SAFETY: cx.raw_cx() is this thread's live context; no JS is executing
    // (eval returned, job queue drained by eval).
    let stats = unsafe { collect_runtime_stats(cx.raw_cx()) }
        .expect("native CollectRuntimeStats must succeed on a live context");

    assert!(
        stats.gc_heap_chunk_total > 0,
        "gc_heap_chunk_total must be non-zero, got {:?}",
        stats
    );
    assert!(stats.gc_heap_gc_things > 0, "gc_heap_gc_things must be non-zero");
    assert!(
        stats.zone_live_gc_things > 0,
        "zone_live_gc_things must be non-zero"
    );
    assert!(
        stats.realm_live_gc_things > 0,
        "realm_live_gc_things must be non-zero"
    );
    // A for_test context has at least the (system + node-realm-bearing)
    // zone/realm hosting the global; the counts are structural, not sizes,
    // and must never be zero on a live runtime.
    assert!(stats.zone_count >= 1, "zone_count must be >= 1");
    assert!(stats.realm_count >= 1, "realm_count must be >= 1");
    assert!(stats.servo_gc_heap_used > 0, "servo_gc_heap_used must be non-zero");
    // Atoms table / script data / realm tables are malloc'd from engine
    // startup — a zero here means the malloc rollup is broken, not empty.
    assert!(stats.servo_malloc_heap > 0, "servo_malloc_heap must be non-zero");
}

fn test_02_live_gc_things_identity(ctx: &mut JsContext) {
    // SAFETY: same owner-thread quiescent discipline as test_01.
    let stats = unsafe { collect_runtime_stats(ctx.raw_cx()) }
        .expect("second collection on the same context must succeed");

    // Exact identity by construction (all three numbers leave the same
    // finalized RuntimeStats in one POD copy-out).
    assert_eq!(
        stats.gc_heap_gc_things,
        stats.live_gc_things_parts_sum(),
        "SM totals identity: gcHeapGCThings == zone + realm live GC things"
    );
}

fn test_03_gc_heap_chunk_consistency(ctx: &mut JsContext) {
    // SAFETY: same owner-thread quiescent discipline as test_01.
    let stats = unsafe { collect_runtime_stats(ctx.raw_cx()) }
        .expect("third collection on the same context must succeed");

    // RuntimeStats header breakdown: chunkTotal >=
    //   decommitted + unusedChunks + unusedArenas + zTotals.unusedGCThings
    //   + chunkAdmin + zTotals.gcHeapArenaAdmin + gcHeapGCThings
    // (SM's own relation, modulo chunk-size rounding; the strict >= keeps
    // the assertion honest without pinning allocator granularity).
    let servo_parts = stats.servo_gc_heap_decommitted
        + stats.servo_gc_heap_unused
        + stats.servo_gc_heap_admin
        + stats.gc_heap_gc_things;
    assert!(
        stats.gc_heap_chunk_total >= servo_parts,
        "gc_heap_chunk_total ({}) must cover the GC-heap parts ({})",
        stats.gc_heap_chunk_total,
        servo_parts
    );
    // Cross-rollup: the ServoSizes used-heap and the headline GC-things
    // number measure the same live GC things (used == gcHeapGCThings; the
    // servo rollup adds zone unused slots into UNUSED, not USED).
    assert_eq!(
        stats.servo_gc_heap_used, stats.gc_heap_gc_things,
        "servo_gc_heap_used must equal gc_heap_gc_things (same live things, two rollups)"
    );
}

#[test]
fn test_engine_memory_stats_native_collect() {
    let mut ctx = JsContext::for_test().expect("Failed to create JsContext");

    test_01_non_zero_metering(&mut ctx);
    test_02_live_gc_things_identity(&mut ctx);
    test_03_gc_heap_chunk_consistency(&mut ctx);

    bao_engine::context::JsContext::shutdown_thread_sm();
}
