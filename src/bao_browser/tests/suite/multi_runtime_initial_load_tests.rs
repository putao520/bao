// e134 (REQ-BRW-002) — same-process sequential multi-runtime initial load.
//
// e133's A/B 2×2 registered this as PRE-EXISTING stock (memory:
// same-process-sequential-runtime-poisoning): under libtest's sequential form
// (--test-threads=1, one process, one BrowserRuntime per test), every runtime
// AFTER THE FIRST dies at NewWebView→pipeline initial load — 7/12 pagestate
// tests red, byte-identical with and without the e133 LoadUrl fix (i.e. NOT
// the constellation navigate face). Production shape (one runtime per bao
// process) never triggers it; the embedder multi-runtime scenario
// (REQ-BRW-002) hits it for real: drop a BrowserRuntime, build another in the
// same process, and the new runtime's pages never load.
//
// This file pins BOTH observed failure forms as one-process reproduction
// vehicles (each test builds its own runtime pair, so nextest's per-test
// process isolation exercises the same poisoning inside a single test):
//   - initial-URL form: create_page(url) direct load — pre-fix died at
//     `wait_for_pipeline_ready` ("pipeline not ready after timeout");
//   - navigate form: create_page(about:blank) → navigate(url) — pre-fix the
//     URL never moved off about:blank.
//
// Forensics: BAO_NAV_RACE_PROBE=1 (same key as the e131/e133 family) enables
// the vendor-side hop probes; this file prints per-stage markers regardless
// (libtest swallows eprintln unless --nocapture).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle, PageState};

const TITLE: &str = "multi-runtime-fixture";

/// servo/BrowserRuntime carry process-global slots — the two runtimes inside
/// each test are strictly SEQUENTIAL (build → load → drop → build), which is
/// exactly the poisoned shape under test. The lock serializes this file's own
/// tests against each other under libtest's default parallel threading (the
/// pagestate RUNTIME_LOCK convention); nextest's per-test process makes it a
/// no-op there.
static RUNTIME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn spawn_origin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let port = listener.local_addr().unwrap().port();
    std::thread::Builder::new()
        .name("multi-runtime-fixture".into())
        .spawn(move || {
            let body = format!(
                "<!DOCTYPE html><html><head><title>{TITLE}</title></head><body><p>fixture</p></body></html>"
            )
            .into_bytes();
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                );
                let _ = s.write_all(&body);
            }
        })
        .expect("spawn fixture");
    port
}

fn display_guard() -> bool {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return false;
    }
    true
}

fn make_runtime() -> Option<BrowserRuntime> {
    match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            None
        }
    }
}

fn poll_interactive(page: &PageHandle, timeout: Duration, stage: &str) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let _ = page.evaluate_js_web(""); // pump the callback drain
        if page.get_state() == PageState::Interactive {
            eprintln!("[e134] {stage}: Interactive in {:?}", start.elapsed());
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "[e134] {stage}: page never reached Interactive within {timeout:?} \
         (state {:?}, url {:?}) — same-process 2nd runtime initial load death",
        page.get_state(),
        page.current_url().unwrap_or_default(),
    );
}

/// Runtime A loads a real URL to Interactive, is dropped; runtime B (same
/// process) must load the SAME way. Pre-fix: B's create_page dies at
/// `wait_for_pipeline_ready` (10s) — "pipeline not ready after timeout".
#[test]
fn second_runtime_initial_load_completes_in_process() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !display_guard() {
        return;
    }
    let port = spawn_origin();
    let url = format!("http://127.0.0.1:{port}/");

    // ── Stage A: the first runtime in this process (control arm) ──
    let rt_a = match make_runtime() {
        Some(r) => r,
        None => return,
    };
    let p_a = rt_a
        .create_page(&PageConfig {
            url: Some(url.clone()),
            ..Default::default()
        })
        .expect("stage A: create_page");
    poll_interactive(&p_a, Duration::from_secs(30), "stage A initial load");
    let title_a = p_a.evaluate_js_web("document.title").unwrap_or_default();
    assert!(title_a.contains(TITLE), "stage A loaded the real document");
    drop(p_a);
    drop(rt_a);
    eprintln!("[e134] stage A runtime dropped");

    // ── Stage B: the second runtime in the SAME process (poisoned arm) ──
    let rt_b = match make_runtime() {
        Some(r) => r,
        None => return,
    };
    let p_b = rt_b
        .create_page(&PageConfig {
            url: Some(url),
            ..Default::default()
        })
        .expect("stage B: create_page (initial load died here pre-fix)");
    poll_interactive(&p_b, Duration::from_secs(30), "stage B initial load");
    let title_b = p_b.evaluate_js_web("document.title").unwrap_or_default();
    assert!(
        title_b.contains(TITLE),
        "stage B loaded the real document (got {title_b:?})"
    );
}

/// Runtime A loads to Interactive, is dropped; runtime B's page is created at
/// about:blank (pipeline spawn alone) and then navigated — the navigation must
/// land. Pre-fix: the URL never moved off about:blank (the load died with no
/// edge, exactly the e133-registered "verbatim wait" failure form).
#[test]
fn second_runtime_navigate_load_completes_in_process() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !display_guard() {
        return;
    }
    let port = spawn_origin();
    let url = format!("http://127.0.0.1:{port}/");

    let rt_a = match make_runtime() {
        Some(r) => r,
        None => return,
    };
    let p_a = rt_a
        .create_page(&PageConfig {
            url: Some(url.clone()),
            ..Default::default()
        })
        .expect("stage A: create_page");
    poll_interactive(&p_a, Duration::from_secs(30), "stage A initial load");
    drop(p_a);
    drop(rt_a);
    eprintln!("[e134] stage A runtime dropped");

    let rt_b = match make_runtime() {
        Some(r) => r,
        None => return,
    };
    let p_b = rt_b
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .expect("stage B: create_page about:blank");
    p_b.navigate(&url).expect("stage B: navigate");
    p_b.wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("stage B: wait_for_pipeline_ready");
    let url_b = p_b.current_url().unwrap_or_default();
    assert!(
        url_b.contains("127.0.0.1"),
        "stage B navigate must land on the fixture (got {url_b:?})"
    );
    poll_interactive(&p_b, Duration::from_secs(30), "stage B navigate load");
}
