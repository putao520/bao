// @trace TEST-ISSUE-24 [req:REQ-BRW-003] [level:e2e]
// ISSUE #24 servo wiring — engine-native execution control (SM interrupt)
// over the servo evaluation paths:
//   * Page realm embedder eval (servo chain, `evaluate_javascript_with_timeout`)
//   * Node Realm eval (bao_browser in-callback arming)
//   * Worker realm script run (per-WebView timeout registry + bridge)
// Runaway scripts (`while(true)`) with a timeout parameter must terminate
// deterministically (<2s armed) with diagnosable timeout text; without the
// parameter the unbounded behavior is preserved (zero regression).
//
// Engine facts (ExecutionControl): the deadline watcher requests the interrupt
// at the deadline; SM fires the interrupt callback at the next loop back-edge;
// a `false` return terminates the script with an uncatchable exception (the
// engine clears the pending exception — the diagnosable text comes from
// `terminal_message`, not the JS exception state).

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool};
use std::time::{Duration, Instant};

const TIMEOUT_TEXT_FRAGMENT: &str = "deadline exceeded";

fn wait_for_load_and_drain(pool_page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = pool_page.evaluate_js("");
        if matches!(
            pool_page.get_state(),
            bao_browser::PageState::Interactive | bao_browser::PageState::Idle
        ) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Wait until the webview's CURRENT document is the target data: URL document.
///
/// The page-state gate alone can flip while the initial placeholder pipeline is
/// still current: servo processes the placeholder→data: pipeline swap lazily on
/// the next message-pump spin, and an `ExitPipeline` for the placeholder
/// pipeline tears down every AutoCloseWorker spawned on it (the worker's
/// closing flag goes true and its running script is interrupted). A worker
/// created before that swap dies within ~50ms for reasons unrelated to any
/// execution timeout — so worker tests MUST settle the navigation first.
fn wait_for_document_settled(pool_page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        if let Ok(url) = pool_page.evaluate_js_web("document.URL") {
            if url.contains("data:text/html") {
                // Hold one extra beat so the placeholder's ExitPipeline has
                // been pumped before the test spawns anything on this page.
                std::thread::sleep(Duration::from_millis(300));
                let _ = pool_page.evaluate_js("");
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn fresh_page<'a>(runtime: &'a BrowserRuntime) -> bao_browser::PageHandle {
    let pool: &PagePool = runtime.page_pool();
    pool.create_page(&PageConfig {
        url: Some("data:text/html,<!DOCTYPE html><html><body></body></html>".into()),
        ..Default::default()
    })
    .expect("pool.create_page failed")
}

// ═══════════════════════════════════════════════════════════════════════
// Page realm — Node Realm face (evaluate_js_with_timeout)
// ═══════════════════════════════════════════════════════════════════════

#[test]
// @trace ISSUE-24 [level:e2e]
fn node_realm_while_true_with_timeout_terminates_with_timeout_text() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = fresh_page(&runtime);
    wait_for_load_and_drain(&page, 20000);

    let started = Instant::now();
    let error = page
        .evaluate_js_with_timeout("while (true) {}", Some(Duration::from_secs(1)))
        .expect_err("a runaway script under an armed timeout must surface as Err");
    let elapsed = started.elapsed();

    let text = error.to_string();
    assert!(
        text.contains(TIMEOUT_TEXT_FRAGMENT),
        "error text must carry timeout semantics, got: {text}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "runaway script must terminate within the armed deadline + engine slack, got: {elapsed:?}"
    );

    // The page must stay fully usable after a control termination.
    let value = page
        .evaluate_js("6 * 7")
        .expect("page must remain usable after a timed-out eval");
    assert_eq!(value, "42");
}

// ═══════════════════════════════════════════════════════════════════════
// Page realm — web face (evaluate_js_web_with_timeout, servo chain)
// ═══════════════════════════════════════════════════════════════════════

#[test]
// @trace ISSUE-24 [level:e2e]
fn web_realm_while_true_with_timeout_terminates_with_timeout_text() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = fresh_page(&runtime);
    wait_for_load_and_drain(&page, 20000);

    let started = Instant::now();
    let error = page
        .evaluate_js_web_with_timeout("while (true) {}", Some(Duration::from_secs(1)))
        .expect_err("a runaway page-realm script under an armed timeout must surface as Err");
    let elapsed = started.elapsed();

    let text = error.to_string();
    assert!(
        text.contains(TIMEOUT_TEXT_FRAGMENT),
        "error text must carry timeout semantics, got: {text}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "runaway script must terminate within the armed deadline + engine slack, got: {elapsed:?}"
    );

    let value = page
        .evaluate_js_web("1 + 1")
        .expect("page realm must remain usable after a timed-out eval");
    assert_eq!(value, "2");
}

// ═══════════════════════════════════════════════════════════════════════
// Worker realm — per-WebView timeout registry + bridge
// ═══════════════════════════════════════════════════════════════════════

#[test]
// @trace ISSUE-24 [level:e2e]
fn worker_realm_while_true_with_timeout_terminates_and_reports() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = fresh_page(&runtime);
    wait_for_load_and_drain(&page, 20000);
    wait_for_document_settled(&page, 20000);

    // Arm the per-WebView worker-script deadline. The termination observable
    // is the PARENT-side error event: the control latches TimedOut at the
    // deadline → the worker's script run is aborted → `report_an_error` fires
    // (no handler inside the worker) → Step 7.2 forwards the real message to
    // the Worker object (`forward_error_to_worker_object`) → `w.onerror`.
    page.set_worker_script_timeout(Some(Duration::from_secs(1)));

    page.evaluate_js_web(
        r#"
        window.__worker_started = 0;
        window.__worker_error = '';
        const w = new Worker("data:text/javascript,postMessage('worker-started-marker'); for(;;){}");
        w.onmessage = () => { window.__worker_started = 1; };
        w.onerror = (e) => { window.__worker_error = String(e.message || 'error'); };
    "#,
    )
    .expect("worker-creating eval must succeed");

    let started = Instant::now();
    let mut worker_error = String::new();
    while started.elapsed() < Duration::from_secs(6) {
        worker_error = page
            .evaluate_js_web("window.__worker_error")
            .expect("poll eval must succeed");
        if worker_error.contains(TIMEOUT_TEXT_FRAGMENT) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        worker_error.contains(TIMEOUT_TEXT_FRAGMENT),
        "parent worker.onerror must carry the timeout message once the armed deadline terminates the runaway worker script, got: {worker_error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "worker script must terminate within the armed deadline + engine slack, got: {:?}",
        started.elapsed()
    );

    // The page must stay fully usable after the worker was terminated.
    let value = page
        .evaluate_js("6 * 7")
        .expect("page must remain usable after a timed-out worker script");
    assert_eq!(value, "42");

    page.set_worker_script_timeout(None);
}

// ═══════════════════════════════════════════════════════════════════════
// No timeout parameter — unbounded behavior preserved (zero regression)
// ═══════════════════════════════════════════════════════════════════════

#[test]
// @trace ISSUE-24 [level:e2e]
fn explicit_none_timeout_preserves_normal_evaluation() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = fresh_page(&runtime);
    wait_for_load_and_drain(&page, 20000);

    let value = page
        .evaluate_js_with_timeout("6 * 7", None)
        .expect("None timeout must behave exactly like the unbounded face");
    assert_eq!(value, "42");

    let value = page
        .evaluate_js_web_with_timeout("1 + 1", None)
        .expect("None timeout must behave exactly like the unbounded face");
    assert_eq!(value, "2");

    // Errors still flow with their real text under the None path.
    let error = page
        .evaluate_js_with_timeout("throw new Error('none-path-marker');", None)
        .expect_err("errors must surface under the None path");
    assert!(
        error.to_string().contains("none-path-marker"),
        "real exception text must survive the None path, got: {error}"
    );
}

// ---------------------------------------------------------------------------
// ISSUE #32 row21 / W3b: browser contexts are PER-REALM for console
// time/count (servo's GlobalScope maps — vendor
// script/dom/console.rs Count → global.increment_console_count), unlike the
// CLI/Node-realm process-global face. Two pages counting the same label must
// not interfere: page A counts twice (logs w3b: 1, w3b: 2) and page B counts
// once (logs w3b: 1 again) — a shared map would log w3b: 3 instead.
// ---------------------------------------------------------------------------

#[test]
fn w3b_console_counters_per_realm_across_pages() {
    use std::sync::mpsc;

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let (console_tx, console_rx) = mpsc::channel::<cdp_server::ConsoleMessage>();
    runtime.set_console_log_channel(console_tx);

    let pool: &PagePool = runtime.page_pool();
    let page_a = pool
        .create_page(&PageConfig {
            url: Some("data:text/html,<!DOCTYPE html><html><body>A</body></html>".into()),
            ..Default::default()
        })
        .expect("page A");
    let page_b = pool
        .create_page(&PageConfig {
            url: Some("data:text/html,<!DOCTYPE html><html><body>B</body></html>".into()),
            ..Default::default()
        })
        .expect("page B");
    wait_for_load_and_drain(&page_a, 20000);
    wait_for_load_and_drain(&page_b, 20000);

    page_a
        .evaluate_js_web("console.count('w3b'); console.count('w3b');")
        .expect("page A count eval");
    page_b
        .evaluate_js_web("console.count('w3b');")
        .expect("page B count eval");

    // Drain the console channel (bounded) and collect the count log lines.
    let mut texts: Vec<String> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match console_rx.try_recv() {
            Ok(cdp_server::ConsoleMessage::Log { text, .. }) => {
                if text.contains("w3b:") {
                    texts.push(text);
                }
            },
            Ok(_) => {},
            Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(50)),
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
        if texts.len() >= 3 {
            break;
        }
    }

    let ones = texts.iter().filter(|t| t.contains("w3b: 1")).count();
    let twos = texts.iter().filter(|t| t.contains("w3b: 2")).count();
    let threes = texts.iter().filter(|t| t.contains("w3b: 3")).count();
    assert_eq!(
        ones, 2,
        "each page's counter must start at 1 (per-realm maps), got: {texts:?}"
    );
    assert_eq!(
        twos, 1,
        "page A's second count must be 2, got: {texts:?}"
    );
    assert_eq!(
        threes, 0,
        "a shared counter would produce w3b: 3 — cross-page interference, got: {texts:?}"
    );
}
