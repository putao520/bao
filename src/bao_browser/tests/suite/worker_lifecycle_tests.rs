// @trace TEST-WK-003 [req:REQ-BRW-003] [level:e2e]
// ISSUE #23 worker face — servo-native dedicated-worker lifecycle driven
// through the page-realm evaluate path (the same face real pages use):
//   create → postMessage round-trip → terminate (shutdown) → recreate.
//
// DEC-WW-003 bypass-track note (E3 audit): the worker lifecycle face had no
// independent e2e — this file is its verification floor. The assertions pin
// the servo-native semantics (worker threads + structured-clone messaging +
// terminate) with no bao-side worker machinery involved.
//
// Harness note: workers are created from data: URLs (the established
// bao-suite pattern). Each round-trip is observed through a window-side
// flag set by the page's onmessage, polled via evaluate_js_web.

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool};
use std::time::{Duration, Instant};

fn wait_for_load_and_drain(pool_page: &bao_browser::PageHandle, max_ms: u64) {
    // Key on document readiness, NOT PageState: a page created with a URL
    // loads through the servo builder without ever writing the stored state
    // out of `Created` (W16 design gap G0 — the Navigating→Interactive
    // projection only fires from stored Navigating), so get_state() can
    // never report Interactive here and the old form burned its whole cap.
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        if pool_page.evaluate_js_web("document.readyState").ok().as_deref() == Some("complete") {
            return;
        }
        let _ = pool_page.evaluate_js(""); // pump the callback drain
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Poll `expr` (a page-realm boolean-ish expression) until it stringifies
/// truthy, or fail after the deadline with the last observed value.
fn poll_until(
    pool_page: &bao_browser::PageHandle,
    expr: &str,
    deadline: Duration,
) -> Result<String, String> {
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed() < deadline {
        last = pool_page.evaluate_js_web(expr).map_err(|e| e.to_string())?;
        if last != "0" && last != "false" && last != "" && last != "undefined" && last != "null" {
            return Ok(last);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Err(format!("condition never became true ({last:?}): {expr}"))
}

/// Poll until `expr` stringifies EXACTLY `expected` (poll_until's truthy
/// short-circuit cannot discriminate a stale non-empty value).
fn poll_until_eq(
    pool_page: &bao_browser::PageHandle,
    expr: &str,
    expected: &str,
    deadline: Duration,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if pool_page.evaluate_js_web(expr).ok().as_deref() == Some(expected) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    false
}

#[test]
// @trace ISSUE-23 [level:e2e]
fn worker_create_postmessage_shutdown_recreate_lifecycle() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let pool: &PagePool = runtime.page_pool();
    let page = pool
        .create_page(&PageConfig {
            url: Some("data:text/html,<!DOCTYPE html><html><body></body></html>".into()),
            ..Default::default()
        })
        .expect("pool.create_page failed");
    wait_for_load_and_drain(&page, 20000);

    // ── create ──────────────────────────────────────────────────────────
    // Worker #1: echoes any message back prefixed, so the round-trip proves
    // BOTH directions (page → worker → page) of the messaging pipe.
    page.evaluate_js_web(
        r#"
        window.__w1_echo = '';
        const w1 = new Worker(
            "data:text/javascript,onmessage = function(e) { postMessage('echo:' + e.data); };"
        );
        w1.onmessage = function(e) { window.__w1_echo = String(e.data); };
        window.__w1 = w1;
        w1.postMessage('ping-1');
    "#,
    )
    .expect("worker create eval must succeed");

    // ── postMessage round-trip ──────────────────────────────────────────
    let echoed = poll_until(&page, "window.__w1_echo", Duration::from_secs(10))
        .expect("worker #1 echo must arrive");
    assert_eq!(
        echoed, "echo:ping-1",
        "the worker round-trip must deliver the exact echoed payload"
    );

    // ── shutdown (terminate) ────────────────────────────────────────────
    // Deterministic shutdown face (battery-v5 red root-caused 2026-09-29):
    // the previous form armed the counter and posted the probe BEFORE
    // terminate, so the worker could legitimately process the probe and post
    // its echo pre-terminate — a parent-side task that terminate does NOT
    // discard (spec: only the worker's own tasks die; deliveries already
    // queued on the parent complete). That in-flight delivery raced the
    // assert. Now: (1) settle — one last round-trip with the creation-time
    // echo handler absorbs all in-flight traffic deterministically; (2) arm
    // the counter only AFTER settle; (3) terminate; (4) no delivery may be
    // counted afterwards; (5) post-terminate input must not be processed at
    // all (a live worker would echo it).
    page.evaluate_js_web("window.__w1.postMessage('settle-probe');")
        .expect("settle probe must be posted");
    let settled = poll_until_eq(&page, "window.__w1_echo", "echo:settle-probe", Duration::from_secs(10));
    assert!(
        settled,
        "the pipe must still round-trip immediately before terminate"
    );
    page.evaluate_js_web(
        r#"
        window.__w1_after_close = 0;
        window.__w1.onmessage = function() { window.__w1_after_close++; };
    "#,
    )
    .expect("post-settle arm must succeed");
    page.evaluate_js_web("window.__w1.terminate();")
        .expect("terminate must succeed");

    // Give any (incorrectly) live worker ample time to misbehave.
    std::thread::sleep(Duration::from_millis(1200));
    let after_close = page
        .evaluate_js_web("String(window.__w1_after_close)")
        .expect("post-terminate poll must succeed");
    assert_eq!(
        after_close, "0",
        "a terminated worker must not deliver messages (shutdown face)"
    );
    // Input face: a message posted to the dead worker must never be
    // processed (no echo back into the counter).
    page.evaluate_js_web("window.__w1.postMessage('after-terminate');")
        .expect("post-terminate probe must be posted");
    std::thread::sleep(Duration::from_millis(1200));
    let after_close2 = page
        .evaluate_js_web("String(window.__w1_after_close)")
        .expect("post-terminate input poll must succeed");
    assert_eq!(
        after_close2, "0",
        "a terminated worker must not process new input (shutdown face)"
    );

    // The page itself must remain fully usable after the shutdown.
    let value = page
        .evaluate_js_web("6 * 7")
        .expect("page must remain usable after worker shutdown");
    assert_eq!(value, "42");

    // ── recreate ────────────────────────────────────────────────────────
    // A FRESH worker on the same page round-trips independently — the
    // lifecycle is repeatable, not one-shot.
    page.evaluate_js_web(
        r#"
        window.__w2_echo = '';
        const w2 = new Worker(
            "data:text/javascript,onmessage = function(e) { postMessage('echo2:' + e.data); };"
        );
        w2.onmessage = function(e) { window.__w2_echo = String(e.data); };
        w2.postMessage('ping-2');
    "#,
    )
    .expect("worker recreate eval must succeed");

    let echoed2 = poll_until(&page, "window.__w2_echo", Duration::from_secs(10))
        .expect("worker #2 echo must arrive");
    assert_eq!(
        echoed2, "echo2:ping-2",
        "the recreated worker must round-trip independently"
    );

    // Both workers coexist: worker #1's flag is untouched by worker #2's
    // traffic and vice versa (no cross-talk).
    let w1_state = page
        .evaluate_js_web("String(window.__w1_echo)")
        .expect("w1 state poll must succeed");
    assert_eq!(
        w1_state, "echo:settle-probe",
        "worker #1's observed state must be untouched by worker #2 traffic \
         (last pre-terminate echo, no post-terminate deliveries)"
    );
}
