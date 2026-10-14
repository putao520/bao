// @trace REQ-BRW-002 [level:e2e] — C9 单次截图延迟 ≤500ms (视口 1920×1080)
// @trace REQ-BRW-001 [level:e2e] — C1 libservo 单进程(ServoBuilder + opts.multiprocess=false)
// @trace REQ-BRW-003 [level:e2e] — C8 GC 由 servo spin_event_loop 统一管理(两 Realm 共享同一 GC)
//                                  + C11 opts.disable_script_debugger=true
//
// # Runtime invariants + latency thin-edge tests
//
// Coverage-audit thin-edge batch (.plans/coverage-audit-brw-20261014.md §2
// G1 / P1 / P3①): the acceptance semantics of these criteria are ASSERTIONS,
// not bench numbers — none of them had any test carrier before this file:
//
//   1. `screenshot_latency_1080p_within_500ms`  — REQ-BRW-002-C9: a single
//      `PageHandle::take_screenshot` at a 1920×1080 viewport must complete
//      within 500ms wall clock (composite → capture → PNG encode).
//   2. `servo_opts_single_process_no_script_debugger` — REQ-BRW-001-C1 +
//      REQ-BRW-003-C11: after `BrowserRuntime::new`, the process-wide servo
//      opts snapshot must read `multiprocess == false` and
//      `disable_script_debugger == true` (BCE-20260621-002 SIGSEGV
//      root-fix contract; see BAO_SERVO_OPTS_INIT in bao_browser/src/lib.rs).
//   3. `gc_memory_stats_stable_after_churn` — REQ-BRW-003-C8: after multi-
//      round document churn plus garbage allocation across BOTH realms
//      (page + Node), the engine-native memory metering face
//      (`collect_engine_memory_stats`) must be stable — no field grows
//      beyond a bounded budget once forced collection has run. A monotonic
//      grower here is a GC-liveness regression, not jitter.
//
// Harness reuse: the servo_render_pipeline_tests skeleton (BrowserRuntime →
// PagePool.create_page → wait_for_pipeline_ready → take_screenshot). No
// parallel harness is introduced.
//
// Process model note (opts snapshot): servo opts is a process-wide
// `OnceLock<Opts>` (first-writer-wins). Under cargo-nextest every #[test]
// runs in its own process, so the snapshot read in test 2 is the config
// THIS test's `BrowserRuntime::new` installed — it wins the race against
// any lazy `opts::get_or_init(Default)` before any servo code runs. Under
// plain `cargo test` (single process, `--test-threads=1`) the first
// `BrowserRuntime::new` in the process installs the same bao config, so the
// assertion holds there too (every runtime construction goes through the
// same `initialize_options` face).

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, ScreenshotFormat};
use std::time::{Duration, Instant};

/// servo/BrowserRuntime carry process-global slots — serialize the tests in
/// this suite file (node_realm_churn_tests precedent; a no-op under nextest).
static RUNTIME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// REQ-BRW-002-C9 acceptance budget: single screenshot ≤ 500ms @1920×1080.
const SCREENSHOT_LATENCY_BUDGET_MS: u128 = 500;

/// GC-stability absolute headroom (bytes). Combined with the proportional
/// term below this is generous for engine jitter and JIT warm-up, and far
/// below a leak signature (per-round live growth shows up as MBs).
const GC_GROWTH_SLACK_BYTES: usize = 1024 * 1024;

/// Space-free / quote-free data: URL (WHATWG percent-encoding constraint —
/// node_realm_churn_tests / page_bench precedent).
fn marker_data_url(marker: &str) -> String {
    format!("data:text/html,<h1 id=b>{marker}</h1>")
}

fn start_runtime() -> BrowserRuntime {
    match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => panic!("BrowserRuntime::new failed: {e}"),
    }
}

fn ready_page_at(runtime: &BrowserRuntime, url: &str) -> bao_browser::PageHandle {
    let page = runtime
        .create_page(&PageConfig {
            url: Some(url.to_string()),
            ..Default::default()
        })
        .expect("create_page");
    page.wait_for_pipeline_ready(Duration::from_secs(15))
        .expect("pipeline ready");
    page
}

// ─── 1. REQ-BRW-002-C9 — single screenshot latency @1920×1080 ≤ 500ms ───────

#[test]
// @trace REQ-BRW-002 [level:e2e]
fn screenshot_latency_1080p_within_500ms() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = start_runtime();
    // The viewport IS the criterion's shape: pinned by construction at
    // 1920×1080 so a config-default change cannot silently move this test
    // off its acceptance viewport (PageInner reads
    // config.viewport_width.unwrap_or(runtime default)).
    let page = runtime
        .create_page(&PageConfig {
            url: Some(marker_data_url("screenshot-latency")),
            viewport_width: Some(1920),
            viewport_height: Some(1080),
            ..Default::default()
        })
        .expect("create_page 1920x1080");
    page.wait_for_pipeline_ready(Duration::from_secs(15))
        .expect("pipeline ready");

    // Act: ONE timed screenshot — composite (WebView::paint) → capture →
    // PNG encode, the full criterion face.
    let start = Instant::now();
    let png = page.take_screenshot(ScreenshotFormat::Png).expect("take_screenshot");
    let elapsed = start.elapsed();

    eprintln!(
        "[screenshot-latency] viewport=1920x1080 elapsed={}ms png_bytes={}",
        elapsed.as_millis(),
        png.len()
    );

    // The frame must be real (not an error-shaped stub) before timing counts.
    let png_magic: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert!(
        png.len() > 1000 && png.len() >= 8 && png[..8] == png_magic,
        "screenshot must be a real PNG frame (>1KB with magic bytes), got {} bytes",
        png.len()
    );

    // The acceptance assertion itself (REQ-BRW-002-C9).
    assert!(
        elapsed.as_millis() <= SCREENSHOT_LATENCY_BUDGET_MS,
        "single screenshot latency {}ms exceeds the {}ms budget (REQ-BRW-002-C9 @1920x1080)",
        elapsed.as_millis(),
        SCREENSHOT_LATENCY_BUDGET_MS
    );

    let _ = page.close();
}

// ─── 2. REQ-BRW-001-C1 + REQ-BRW-003-C11 — servo opts invariant pin ─────────

#[test]
// @trace REQ-BRW-001 [level:e2e]
// @trace REQ-BRW-003 [level:e2e]
fn servo_opts_single_process_no_script_debugger() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // opts is process-wide first-writer-wins: constructing the runtime forces
    // bao's `initialize_options` BEFORE any servo code can lazily lock in
    // `Opts::default()` (BAO_SERVO_OPTS_INIT + the `BrowserRuntime::new`
    // face) — so this post-construction snapshot is bao's embedder contract.
    let _runtime = start_runtime();
    let opts = servo::opts::get();
    eprintln!(
        "[opts-invariants] multiprocess={} disable_script_debugger={} force_isolate_event_loops={}",
        opts.multiprocess, opts.disable_script_debugger, opts.force_isolate_event_loops
    );

    // REQ-BRW-001-C1: libservo runs single-process.
    assert!(
        !opts.multiprocess,
        "servo must run single-process (opts.multiprocess == false), got multiprocess=true"
    );

    // REQ-BRW-003-C11: the servo Debugger face stays disabled — bao ships its
    // own CDP; enabling fire_add_debuggee is the BCE-20260621-002 OSR
    // SIGSEGV path this flag exists to bypass.
    assert!(
        opts.disable_script_debugger,
        "opts.disable_script_debugger must be true (REQ-BRW-003-C11 / BCE-20260621-002), got false"
    );
}

// ─── 3. REQ-BRW-003-C8 — GC stability after churn (engine memory face) ──────

#[test]
// @trace REQ-BRW-003 [level:e2e]
fn gc_memory_stats_stable_after_churn() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = start_runtime();
    let page = ready_page_at(&runtime, "about:blank");

    // ── churn: navigate cycles across distinct documents ───────────────────
    for i in 0..6 {
        let marker = format!("gc-churn-{i}");
        page.navigate(&marker_data_url(&marker))
            .unwrap_or_else(|e| panic!("churn {i}: navigate failed: {e}"));
        page.wait_for_navigation(Duration::from_secs(15))
            .unwrap_or_else(|e| panic!("churn {i}: wait_for_navigation failed: {e}"));
        let check = page
            .evaluate_js_web(&format!(
                "String(document.getElementById('b') && document.getElementById('b').textContent === '{marker}')"
            ))
            .unwrap_or_else(|e| panic!("churn {i}: marker eval failed: {e}"));
        assert!(
            check.contains("true"),
            "churn {i}: marker verification failed (got {check:?})"
        );
    }

    // ── settle, then sample A ──────────────────────────────────────────────
    // `Bun.gc()` = SM `GCOptions::Shrink` full collection over the runtime
    // hosting BOTH realms — the same scope `collect_engine_memory_stats`
    // measures (page + Node share one GC, C8's "两个 Realm 共享同一 GC").
    page.evaluate_js("Bun.gc(); 'ok'")
        .expect("node-realm Bun.gc");
    let a = page
        .collect_engine_memory_stats()
        .expect("engine memory stats (sample A)");
    eprintln!(
        "[gc-stability] A gc_things={}B zone_live={}B realm_live={}B chunks={}B malloc_heap={}B non_heap={}B",
        a.gc_heap_gc_things,
        a.zone_live_gc_things,
        a.realm_live_gc_things,
        a.gc_heap_chunk_total,
        a.servo_malloc_heap,
        a.servo_non_heap
    );

    // ── garbage rounds across BOTH realms (C8: one shared GC face) ─────────
    for r in 0..3 {
        page.evaluate_js(&format!(
            "(function(){{ let acc=[]; for(let i=0;i<20000;i++){{ acc.push({{i:i,s:'garbage-{r}-'+i}}); }} acc=null; return 'ok'; }})()"
        ))
        .unwrap_or_else(|e| panic!("round {r}: node-realm garbage failed: {e}"));
        page.evaluate_js_web(&format!(
            "(function(){{ let acc=[]; for(let i=0;i<20000;i++){{ acc.push(new Array(8).fill('g{r}')); }} acc=null; return 'ok'; }})()"
        ))
        .unwrap_or_else(|e| panic!("round {r}: page-realm garbage failed: {e}"));
        page.evaluate_js("Bun.gc(); 'ok'")
            .unwrap_or_else(|e| panic!("round {r}: forced GC failed: {e}"));
        // C8's management face: the servo event-loop spin owns GC liveness.
        runtime.spin_event_loop();
    }

    page.evaluate_js("Bun.gc(); 'ok'")
        .expect("node-realm Bun.gc (settle)");
    let b = page
        .collect_engine_memory_stats()
        .expect("engine memory stats (sample B)");
    eprintln!(
        "[gc-stability] B gc_things={}B zone_live={}B realm_live={}B chunks={}B malloc_heap={}B non_heap={}B",
        b.gc_heap_gc_things,
        b.zone_live_gc_things,
        b.realm_live_gc_things,
        b.gc_heap_chunk_total,
        b.servo_malloc_heap,
        b.servo_non_heap
    );

    // Stability assertion: every metered face is bounded after churn + forced
    // collection (slack = max(1 MiB, 25% of sample A)). A healthy GC keeps
    // dead churn/garbage reclaimed — flat or shrinking; a monotonic grower
    // here is the C8 liveness failure this test exists to catch.
    let samples = [
        ("gc_heap_gc_things", a.gc_heap_gc_things, b.gc_heap_gc_things),
        (
            "realm_live_gc_things",
            a.realm_live_gc_things,
            b.realm_live_gc_things,
        ),
        (
            "zone_live_gc_things",
            a.zone_live_gc_things,
            b.zone_live_gc_things,
        ),
        (
            "gc_heap_chunk_total",
            a.gc_heap_chunk_total,
            b.gc_heap_chunk_total,
        ),
        ("servo_malloc_heap", a.servo_malloc_heap, b.servo_malloc_heap),
        ("servo_non_heap", a.servo_non_heap, b.servo_non_heap),
    ];
    for (field, a_v, b_v) in samples {
        let slack = GC_GROWTH_SLACK_BYTES.max(a_v / 4);
        assert!(
            b_v <= a_v + slack,
            "engine memory face `{field}` grew beyond the stability budget after churn + forced GC: \
             A={a_v}B B={b_v}B slack={slack}B (REQ-BRW-003-C8)"
        );
    }

    let _ = page.close();
}
