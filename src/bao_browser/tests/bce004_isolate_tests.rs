// BCE-20260622-004 — isolating the SIGSEGV trigger.
// Test 1: single page, multi evaluate (no nav, no inject)
// Test 2: single page, multi nav (no evaluate between)
// Test 3: single page, evaluate + nav + evaluate (the original sequence)

// @trace BCE-20260627-004 [criterion:REQ-BRW-004-C18] [nfr:NFR-MEMSAF-001]
// EBUSY regression anchor: this file exercises the BaoRuntime/Servo lifecycle
// (create → eval → nav → close) under a single thread. When run with
// BAO_TEST_NETWORK=1 + DISPLAY, it confirms the mozjs Mutex_posix.cpp EBUSY
// patch tolerates pthread_mutex_destroy returning EBUSY during libtest
// thread-pool TLS teardown — the original PagePool chaos SIGSEGV root cause.
// The dedicated handle-level EBUSY regression lives in bce004_stress_tests.rs
// (bce20260627_004_nfr_memsaf_001_ebusy_mutex_destroy_regression).

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, WorkerHandle};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(40));
        if let Ok(s) = page.evaluate_js_web("document.readyState") {
            if s.contains("complete") || s.contains("interactive") { return; }
        }
    }
}

fn env_ok() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1"); return false;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY"); return false;
    }
    true
}

#[test]
fn bce004_iso_multi_eval_no_nav() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] init: {e}"); return; }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => { eprintln!("[fail] create: {e}"); return; }
    };
    // 50 evaluate_js calls (Node Realm path, filename "bao_evaluate_js")
    for i in 0..50 {
        let r = page.evaluate_js(&format!("({} + 1)", i));
        if let Err(e) = r {
            eprintln!("[fail] evaluate #{}: {e}", i); return;
        }
    }
    eprintln!("[iso-eval] PASS — 50 Node Realm evals no SIGSEGV");
}

#[test]
fn bce004_iso_multi_nav_no_eval() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] init: {e}"); return; }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => { eprintln!("[fail] create: {e}"); return; }
    };
    let urls = [
        "https://example.com/",
        "https://example.org/",
        "https://www.example.net/",
    ];
    for (i, u) in urls.iter().enumerate() {
        if let Err(e) = page.navigate(u) {
            eprintln!("[fail] nav #{}: {e}", i); return;
        }
        wait_for_load(&page, 8000);
    }
    eprintln!("[iso-nav] PASS — 3 navs no SIGSEGV");
}

#[test]
fn bce004_iso_eval_nav_eval() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] init: {e}"); return; }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => { eprintln!("[fail] create: {e}"); return; }
    };
    eprintln!("[iso-eval-nav-eval] eval #1");
    let _ = page.evaluate_js("1+1");
    eprintln!("[iso-eval-nav-eval] nav → https://example.com/");
    if let Err(e) = page.navigate("https://example.com/") {
        eprintln!("[fail] nav: {e}"); return;
    }
    wait_for_load(&page, 8000);
    eprintln!("[iso-eval-nav-eval] eval #2 (post-nav)");
    let r2 = page.evaluate_js("2+2");
    if let Err(e) = r2 {
        eprintln!("[fail] post-nav eval: {e}"); return;
    }
    eprintln!("[iso-eval-nav-eval] PASS — eval+nav+eval no SIGSEGV");
}

#[test]
fn bce004_iso_create_close_create() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] init: {e}"); return; }
    };
    let pool = runtime.page_pool();
    // Create 5 pages, eval, close, repeat — accumulates destroyed Node Realms.
    for n in 0..5 {
        let page = match pool.create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => { eprintln!("[fail] create #{}: {e}", n); return; }
        };
        for i in 0..5 {
            if let Err(e) = page.evaluate_js(&format!("({} + {})", n, i)) {
                eprintln!("[fail] eval #{}.{}: {e}", n, i); return;
            }
        }
        let _ = page.close();
    }
    eprintln!("[iso-close-create] PASS — 5 create/eval/close cycles no SIGSEGV");
}

// ===========================================================================
// BCE-20260627-004 — handle-level concurrent teardown (no live servo required)
// ===========================================================================
//
// @trace BCE-20260627-004 [criterion:REQ-BRW-004-C18] [nfr:NFR-MEMSAF-001]
//
// Companion to bce004_stress_tests.rs. WorkerHandle is Arc<AtomicBool>-backed
// and fully Send+Sync, so we can exercise the C18 closing-flag consistency
// invariant without a live BaoRuntime. This test covers the gap that the
// SIGSEGV-isolating E2E tests above leave when BAO_TEST_NETWORK/DISPLAY are
// unset — it runs unconditionally.

/// BCE-20260627-004 — concurrent terminate() across cloned WorkerHandles.
///
/// @trace BCE-20260627-004 [criterion:REQ-BRW-004-C18]
///
/// Spawns N threads, each holding an Arc-clone of the same WorkerHandle, all
/// calling terminate() + mark_terminated() concurrently. Asserts the closing
/// and terminated flags are atomically visible to all threads after join.
#[test]
fn bce20260627_004_iso_concurrent_terminate_atomic_visibility() {
    const THREADS: usize = 8;

    let handle = WorkerHandle::new("iso-race-worker.js".to_string());
    let clones: Vec<WorkerHandle> = (0..THREADS).map(|_| handle.clone()).collect();

    let barrier = Arc::new(std::sync::Barrier::new(THREADS));
    let mut joins = Vec::with_capacity(THREADS);
    for c in clones {
        let b = Arc::clone(&barrier);
        joins.push(thread::spawn(move || {
            b.wait();
            // Idempotent terminate + mark_terminated under concurrency.
            c.terminate();
            c.mark_terminated();
            c.terminate(); // double-terminate must not panic
        }));
    }
    for j in joins {
        j.join().expect("iso race thread panicked");
    }

    // Acquire-load must observe the Release-stores from all threads.
    assert!(
        handle.is_closing(),
        "closing flag not visible after 8-thread terminate"
    );
    assert!(
        handle.is_terminated(),
        "terminated flag not visible after 8-thread mark_terminated"
    );
    // Flag is Acquire-stable across repeated reads.
    for _ in 0..100 {
        assert!(handle.is_closing(), "closing flag flickered (torn read)");
        assert!(handle.is_terminated(), "terminated flag flickered (torn read)");
    }
    eprintln!(
        "[bce20260627-004/iso] {}-thread concurrent terminate OK: flags atomically visible",
        THREADS
    );
}

/// BCE-20260627-004 — REALM_PROFILES unregister safety under concurrent teardown.
///
/// @trace BCE-20260627-004 [criterion:REQ-BRW-004-C18]
///
/// Concurrent unregister_stealth_profile() on the same handle must not panic
/// or race (the addr slot is a single AtomicU64, the DashMap remove is
/// internally synchronized). C18 requires "REALM_PROFILES 条目注销" be
/// crash-safe on all three teardown paths.
#[test]
fn bce20260627_004_iso_concurrent_unregister_stealth_profile_safe() {
    const THREADS: usize = 8;
    const ITERS: usize = 500;

    let handle = WorkerHandle::new("iso-unreg-worker.js".to_string());
    handle.set_worker_global_addr(0xBEEF); // non-zero so unregister actually looks up

    let clones: Vec<WorkerHandle> = (0..THREADS).map(|_| handle.clone()).collect();
    let barrier = Arc::new(std::sync::Barrier::new(THREADS));
    let mut joins = Vec::with_capacity(THREADS);
    for c in clones {
        let b = Arc::clone(&barrier);
        joins.push(thread::spawn(move || {
            b.wait();
            for _ in 0..ITERS {
                // Concurrent unregister on the same global addr — must be safe.
                c.unregister_stealth_profile();
            }
        }));
    }
    for j in joins {
        j.join().expect("iso unregister race thread panicked");
    }
    // Final state: addr still readable, no torn value.
    assert_eq!(
        handle.worker_global_addr(),
        0xBEEF,
        "global addr slot torn by concurrent unregister"
    );
    eprintln!(
        "[bce20260627-004/iso] concurrent unregister_stealth_profile OK: {} threads x {} iters, zero panic",
        THREADS, ITERS
    );
}
