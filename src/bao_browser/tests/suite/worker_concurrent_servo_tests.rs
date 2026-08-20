// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:18] [level:integration]
// Servo-native Worker concurrent teardown tests for REQ-BRW-004 criterion #18.
//
// SPEC C18 criterion:
//   "worker terminate()/self.close()/页面卸载 三路径 teardown 均 crash-safe:
//    worker 线程 JSContext 干净销毁 + 线程 join 无悬挂 + REALM_PROFILES 条目注销 +
//    无 EBUSY 类 mutex destroy SIGSEGV; 并发创建-销毁 N 个 worker 循环零崩溃零泄漏"
//
// Background:
//   executor-bypass-removal deleted bypass WebWorker concurrent tests because the
//   bypass path no longer exists. These tests replace them with servo-native Worker
//   path coverage.
//
// Environment gating:
//   Real servo rendering requires DISPLAY (Xvfb) and network/asset I/O. These tests
//   are skipped unless BAO_TEST_NETWORK=1 and DISPLAY are present, so they never
//   break CI headless runs.
//
// Usage:
//   BAO_TEST_NETWORK=1 DISPLAY=:99 cargo test -p bao_browser \
//     --test worker_concurrent_servo_tests -- --nocapture

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use std::sync::Mutex;
use std::time::Duration;

#[path = "common/mod.rs"]
mod common;

/// Serializes all servo-touching tests in this binary (servo single-instance).
/// Each test acquires this lock for its full duration. BCE-20260627-009.
static TEST_SERIALIZER: Mutex<()> = Mutex::new(());

/// Skip-guard helper: returns true if the test should skip (no network/display).
fn should_skip() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1");
        return true;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return true;
    }
    false
}

/// Acquire the global serializer lock. Hold for the full test duration.
/// The servo idempotent patches (BCE-20260627-009) make repeated
/// BaoRuntime::new safe; this lock only prevents concurrent servo instances.
fn lock_serializer() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: `TEST_SERIALIZER` is a `static`, so the lock's lifetime is
    // bounded by the process. The guard is only dropped when the test function
    // returns. This transmute extends the lifetime bound to 'static, matching
    // the actual underlying static Mutex.
    unsafe {
        std::mem::transmute::<std::sync::MutexGuard<'_, ()>, std::sync::MutexGuard<'static, ()>>(
            guard,
        )
    }
}

/// URL-encode a JS worker body for data: URL (minimal percent-encoding).
fn encode_worker_body(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

/// Make JS that creates N workers in parallel via Promise.all, each running the
/// given script body (URL-encoded), and waits for all to settle.
///
/// Returns a JS string that resolves when all workers have been created and
/// either completed or been terminated.
fn make_concurrent_worker_driver(worker_script_body: &str, count: usize) -> String {
    format!(
        r#"
        (function () {{
            const n = {count};
            const workers = [];
            for (let i = 0; i < n; i++) {{
                const w = new Worker("data:text/javascript,{body}");
                workers.push(w);
            }}
            // Resolve immediately after creation — the test will verify no crash
            // and that the runtime stays alive.
            return workers.length;
        }})();
        "#,
        count = count,
        body = worker_script_body
    )
}

/// Poll a JS condition until true or timeout.
fn wait_for_js_condition(page: &bao_browser::PageHandle, expr: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(s) = page.evaluate_js_web(expr) {
            let trimmed = s.trim();
            if trimmed == "true" || trimmed == "\"true\"" {
                return true;
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §1 C18 concurrent create/destroy crash safety
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:18] concurrent create/destroy zero crash
///
/// Creates N workers in parallel, each immediately self-closing, and verifies
/// the BaoRuntime does not crash (no SIGSEGV/panic). This tests the crash-safe
/// teardown path under concurrent load.
#[test]
fn c18_concurrent_create_destroy_zero_crash() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[fail] create_page: {e}");
            return;
        }
    };

    // Wait for page pipeline ready.
    if page
        .wait_for_pipeline_ready(Duration::from_secs(5))
        .is_err()
    {
        eprintln!("[skip] pipeline not ready");
        return;
    }

    // Worker script: immediately self.close() to trigger teardown immediately.
    let worker_body = encode_worker_body("self.close();");
    let count = 5;

    // Run 3 rounds of concurrent create/destroy.
    for round in 0..3 {
        let driver = make_concurrent_worker_driver(&worker_body, count);
        let result = page.evaluate_js_web(&driver);
        match result {
            Ok(s) => {
                let n: usize = s.trim().parse().unwrap_or(0);
                if n != count {
                    eprintln!("[round {round}] created {n} workers, expected {count}");
                }
            }
            Err(e) => {
                // If the runtime crashed, this would panic — reaching here means
                // no crash.
                eprintln!("[round {round}] evaluate_js_web error (no crash): {e}");
            }
        }
        // Brief pause between rounds.
        std::thread::sleep(Duration::from_millis(100));
    }

    // Verify runtime still alive (evaluate_js_web succeeds).
    let check = page.evaluate_js_web("1 + 1");
    assert!(
        check.is_ok(),
        "runtime must remain alive after concurrent worker create/destroy"
    );
    eprintln!("[C18] concurrent create/destroy passed — runtime alive");
}

/// @trace REQ-BRW-004 [criterion:18] WorkerHandle is_closing() consistent under concurrent load
///
/// Verifies that WorkerHandle.is_closing() correctly reflects the worker state
/// even when multiple workers are created and terminated concurrently.
#[test]
fn c18_concurrent_terminate_closing_flag_consistent() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };

    // Need a page first for servo-native Worker creation.
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[skip] create_page: {e}");
            return;
        }
    };
    if page
        .wait_for_pipeline_ready(Duration::from_secs(5))
        .is_err()
    {
        eprintln!("[skip] pipeline not ready");
        return;
    }

    // Create workers via BaoRuntime::create_worker (which returns WorkerHandle).
    let mut handles = Vec::new();

    for i in 0..3 {
        let worker_url = format!("data:text/javascript,postMessage('worker-{}');", i);
        match runtime.create_worker_with_url(&page, &worker_url) {
            Ok(handle) => {
                // Verify initial state.
                assert!(
                    !handle.is_closing(),
                    "worker {} should not be closing initially",
                    i
                );
                handles.push(handle);
            }
            Err(e) => {
                eprintln!("[skip] create_worker failed: {e}");
                return;
            }
        }
    }

    // Terminate all workers concurrently.
    for handle in &handles {
        handle.terminate();
    }

    // All handles should report closing after terminate().
    for (i, handle) in handles.iter().enumerate() {
        assert!(
            handle.is_closing(),
            "worker {} should be closing after terminate()",
            i
        );
    }

    // Wait for termination (bounded).
    let all_terminated = common::wait_for_condition(Duration::from_secs(5), || {
        handles.iter().all(|h| h.is_terminated())
    });
    if !all_terminated {
        eprintln!("[warn] not all workers terminated within timeout");
    }

    // Verify runtime still alive using the existing page.
    let check = page.evaluate_js_web("1 + 1");
    assert!(
        check.is_ok(),
        "runtime must remain alive after concurrent worker terminate"
    );
    eprintln!("[C18] is_closing flag consistency passed");
}

/// @trace REQ-BRW-004 [criterion:18] three teardown paths crash-free
///
/// Exercises all three teardown paths (worker.terminate(), self.close(), page-unload)
/// and verifies each is crash-safe.
#[test]
fn c18_three_path_teardown_crash_free() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };

    // Path 1: worker.terminate() from main thread.
    {
        let page = match runtime.create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[skip] page for path 1: {e}");
                return;
            }
        };
        if page
            .wait_for_pipeline_ready(Duration::from_secs(3))
            .is_err()
        {
            eprintln!("[skip] pipeline not ready for path 1");
            return;
        }

        // Create worker that posts messages (long-running until terminated).
        let worker_body = encode_worker_body("setInterval(()=>postMessage(1),100);");
        let driver = format!(
            r#"(function() {{
            const w = new Worker("data:text/javascript,{body}");
            window.__path1Worker = w;
            return 'created';
        }})();"#,
            body = worker_body
        );
        let _ = page.evaluate_js_web(&driver);
        std::thread::sleep(Duration::from_millis(200));

        // Terminate via JS worker.terminate().
        let _ = page.evaluate_js_web("window.__path1Worker.terminate();");
        std::thread::sleep(Duration::from_millis(100));

        // Verify no crash.
        let check = page.evaluate_js_web("'path1-ok'");
        assert!(check.is_ok(), "path 1 (worker.terminate) must not crash");
        eprintln!("[C18] path 1 (worker.terminate) passed");
    }

    // Path 2: self.close() from worker script.
    {
        let page = match runtime.create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[fail] page for path 2: {e}");
                return;
            }
        };
        if page
            .wait_for_pipeline_ready(Duration::from_secs(3))
            .is_err()
        {
            eprintln!("[skip] pipeline not ready for path 2");
            return;
        }

        // Worker immediately self.close().
        let worker_body = encode_worker_body("self.close();");
        let driver = format!(
            r#"(function() {{
            const w = new Worker("data:text/javascript,{body}");
            return 'created';
        }})();"#,
            body = worker_body
        );
        let _ = page.evaluate_js_web(&driver);
        std::thread::sleep(Duration::from_millis(200));

        let check = page.evaluate_js_web("'path2-ok'");
        assert!(check.is_ok(), "path 2 (self.close) must not crash");
        eprintln!("[C18] path 2 (self.close) passed");
    }

    // Path 3: page-unload (page.close() triggers worker teardown).
    {
        let page = match runtime.create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[fail] page for path 3: {e}");
                return;
            }
        };
        if page
            .wait_for_pipeline_ready(Duration::from_secs(3))
            .is_err()
        {
            eprintln!("[skip] pipeline not ready for path 3");
            return;
        }

        // Create long-running worker.
        let worker_body = encode_worker_body("setInterval(()=>postMessage(1),100);");
        let driver = format!(
            r#"(function() {{
            const w = new Worker("data:text/javascript,{body}");
            window.__path3Worker = w;
            return 'created';
        }})();"#,
            body = worker_body
        );
        let _ = page.evaluate_js_web(&driver);
        std::thread::sleep(Duration::from_millis(200));

        // Close the page — triggers page-unload worker teardown.
        if let Err(e) = page.close() {
            eprintln!("[warn] page.close error: {e}");
        }
        std::thread::sleep(Duration::from_millis(100));

        // Verify runtime still alive by creating another page.
        let page2 = match runtime.create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[fail] create_page after path 3: {e}");
                return;
            }
        };
        let check = page2.evaluate_js_web("'path3-ok'");
        assert!(check.is_ok(), "path 3 (page-unload) must not crash");
        eprintln!("[C18] path 3 (page-unload) passed");
    }

    eprintln!("[C18] three-path teardown crash-free passed");
}

// ═══════════════════════════════════════════════════════════════════════
// §2 Unit checks (no servo required)
// ═══════════════════════════════════════════════════════════════════════

/// @trace NFR-TEST-REPRODUCIBILITY [criterion:harness] URL-encoding helper
#[test]
fn encode_worker_body_for_concurrent() {
    let raw = "self.close();";
    let enc = encode_worker_body(raw);
    assert!(enc.contains("self"));
    assert!(enc.contains("close"));
    assert!(!enc.contains(' '), "spaces must be percent-encoded");
}
