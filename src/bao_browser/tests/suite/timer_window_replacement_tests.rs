// @trace REQ-BRW-002 [level:e2e]
// Timers × same-origin window_for_replacement reuse arm — pending-timer
// invariant regression (E5 forensic: the W55 WPT driver's polling INJECT —
// a setTimeout self-continuation chain — panic'd the ScriptThread at
// vendor/servo/components/script/event_loop/timers.rs:912
// `assert_eq!(pipeline, global.pipeline_id())` during same-origin
// navigation, forcing the driver to a zero-timer DCL+load workaround).
//
// Root cause: OneshotTimers live per-Document, but the TimerListener freezes
// `TimerSource::FromWindow(pipeline)` at schedule time and delivers to the
// Trusted **Window**. HTML spec "initialise the document object" step 6
// reuses the active Window when the active document is the initial
// about:blank and origins are same origin-domain — exactly what an iframe
// does on its first same-origin navigation. After `Window::init_document`
// swaps D1→D2 on the reused Window, `Window::pipeline_id()` (=
// `Document().pipeline_id()`) reports the NEW pipeline, so a pending timer of
// the superseded blank document fires its frozen `FromWindow(D1)` event into
// a window that now reports D2 → assertion panic (ScriptThread death).
// Spec semantics: a superseded document is not fully active; its tasks are
// skipped, not fatal.
//
// Probe shape (RED before the fix / GREEN after):
//   parent page (http://127.0.0.1:P/parent.html)
//     └─ <iframe> without src → nested browsing context with initial
//        about:blank (origin inherited from parent = same-origin-domain)
//        └─ setTimeout self-continuation chain (the E5 INJECT form)
//           + a burst of staggered timers
//   f.src = /frame.html (same-origin, fixture-delimited 80ms) → reuse arm,
//   while a 200ms parent-side busy-wait blocks the ScriptThread so the Load
//   message and the due blank-document timers pile up and are co-categorized
//   by the next event-loop iteration (stale `fully_active` snapshot).
// RED observable pre-fix: the ScriptThread dies on the pending timer
// assertion; post-fix the page answers, the superseded chain is frozen at
// the swap (no zombie fire), and the replacement document's own timers
// still fire.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle, PageState};

/// Browser boots are serialized within this binary: two servo BrowserRuntimes
/// racing in one process competes for the single embedder slot — the failure
/// would be flaky infra, not the class under test.
static BOOT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn boot_lock() -> MutexGuard<'static, ()> {
    let m = BOOT_LOCK.get_or_init(|| Mutex::new(()));
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Minimal same-origin HTTP/1.1 fixture: /parent.html + /frame.html on an
/// ephemeral 127.0.0.1 port. Both documents share one origin so the iframe's
/// initial about:blank (origin inherited from the parent) satisfies
/// `window_for_replacement`'s same-origin-domain filter on first navigation.
fn http_fixture() -> (u16, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let port = listener.local_addr().expect("local_addr").port();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if stop_thread.load(Ordering::Relaxed) {
                return;
            }
            let mut s = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            // Read until end of headers; the fixture body is never large.
            loop {
                match s.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    },
                    Err(_) => break,
                }
            }
            let req = String::from_utf8_lossy(&buf);
            let path = req.split_whitespace().nth(1).unwrap_or("/");
            // /frame.html is deliberately SLOW (80ms): the iframe Load message
            // must land while the test's busy-wait task is blocking the
            // ScriptThread, so that the Load and the already-due blank-doc
            // timers accumulate together and are co-categorized by the next
            // event-loop iteration (see the test body).
            if path.starts_with("/frame.html") {
                std::thread::sleep(Duration::from_millis(80));
            }
            let body = match path {
                "/frame.html" => {
                    "<!DOCTYPE html><html><head><title>frame</title></head>\
                     <body><p id=\"frame\">frame</p></body></html>"
                },
                _ => {
                    "<!DOCTYPE html><html><head><title>parent</title></head>\
                     <body><p id=\"parent\">parent</p></body></html>"
                },
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.flush();
        }
    });
    (port, stop)
}

fn wait_thread_alive(page: &PageHandle, max_ms: u64) -> bool {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return true;
        }
        if let Ok(v) = page.evaluate_js_web("1") {
            if v.trim().trim_matches('"') == "1" {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    matches!(page.get_state(), PageState::Interactive | PageState::Idle)
}

/// Poll a JS condition through the page realm (each evaluate pumps the servo
/// loop) until it stringifies to "true" or the deadline expires.
fn wait_until(page: &PageHandle, max_ms: u64, js: &str) -> bool {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        if let Ok(v) = page.evaluate_js_web(js) {
            if v.trim().trim_matches('"') == "true" {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn pending_timer_chain_survives_same_origin_iframe_window_replacement() {
    let _guard = boot_lock();
    bun_core::Output::init_test();

    let (port, _fixture) = http_fixture();
    let base = format!("http://127.0.0.1:{port}");

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(format!("{base}/parent.html")),
            ..Default::default()
        })
        .expect("create_page(parent)");
    assert!(
        wait_thread_alive(&page, 15_000),
        "parent page never became interactive"
    );

    // §1 Trigger build — all in ONE parent eval so no free event-loop window
    // separates the pieces:
    //   • iframe without src → nested browsing context whose initial
    //     about:blank document inherits the parent origin;
    //   • the E5 INJECT form — a setTimeout self-continuation chain — started
    //     INSIDE that blank document;
    //   • a burst of 30 staggered timers (10ms..300ms) in the same document;
    //   • f.src = /frame.html (the fixture delays that response 80ms).
    // §2 then busy-waits 200ms on the ScriptThread. During the busy-wait the
    // Load message (80ms) AND several timer deadlines (10ms..200ms) pile up
    // unprocessed. When the wait ends, the next event-loop iteration
    // snapshots `fully_active` (the blank pipeline still active), dispatches
    // all due timers into the task queue and drains the constellation Load —
    // co-categorizing [Load, timer-tasks] with the STALE snapshot, then
    // running them in order: Load first (window_for_replacement swap —
    // `Window::init_document` re-points the reused Window at the new
    // document), timer task second, against a Window whose
    // `global.pipeline_id()` now reports the NEW pipeline. Pre-fix that is
    // exactly the timers.rs `assert_eq!(pipeline, global.pipeline_id())`
    // panic; post-fix the stale event is dropped.
    let started = page
        .evaluate_js_web(&format!(
            "(function(){{\
               var f = document.createElement('iframe');\
               document.body.appendChild(f);\
               window.__f = f;\
               try {{\
                 f.contentWindow.eval('window.__chain = 0; window.__burst = 0; \
                    function __poll(){{ window.__chain++; window.setTimeout(__poll, 4); }} \
                    __poll(); \
                    for (var i = 1; i <= 30; i++) {{ window.setTimeout(function(){{ window.__burst++; }}, i * 10); }}');\
                 f.src = '{base}/frame.html';\
               }} catch (e) {{ return 'eval-failed:' + e.name; }}\
               return 'started';\
             }})()"
        ))
        .expect("iframe + chain setup eval failed");
    assert_eq!(
        started.trim().trim_matches('"'),
        "started",
        "setTimeout chain could not be started in the iframe's initial about:blank document"
    );

    // §2 Block the ScriptThread across the delayed Load's arrival window
    // (80ms fixture delay + 120ms of margin) so the Load and the due timers
    // accumulate together.
    let waited = page
        .evaluate_js_web(
            "(function(){ var t0 = Date.now(); while (Date.now() - t0 < 200); \
               return 'waited-' + String(Date.now() - t0); })()",
        )
        .expect("busy-wait eval failed");
    eprintln!(
        "[timer-replacement] script-thread blocked for {}",
        waited.trim().trim_matches('"')
    );

    // Probe validity: the chain's first tick runs synchronously at injection
    // (before the navigation even starts), so this is race-free.
    let pre_ticks = page
        .evaluate_js_web("String(window.__f.contentWindow.__chain)")
        .expect("pre-swap chain sample failed");
    eprintln!(
        "[timer-replacement] chain ticks at injection: {}",
        pre_ticks.trim().trim_matches('"')
    );
    assert!(
        pre_ticks.trim().trim_matches('"') != "0",
        "chain never ticked in the initial about:blank document — probe invalid"
    );

    // Wait until the replacement document is active (the D1→D2 swap happened).
    assert!(
        wait_until(
            &page,
            10_000,
            "String(!!(window.__f.contentWindow && window.__f.contentDocument && \
             window.__f.contentDocument.location.href.indexOf('/frame.html') !== -1))"
        ),
        "iframe never reached /frame.html (ScriptThread may have died on the \
         pending-timer assertion — the E5 reuse-arm panic)"
    );

    // §3 ScriptThread liveness — the primary RED observable. Pre-fix the
    // co-categorized [Load, timer-tasks] batch runs the Load (swap) first
    // and then the pending blank-document timer task against a Window whose
    // `global.pipeline_id()` reports the NEW pipeline:
    //   thread 'Script#1' panicked at .../event_loop/timers.rs:912:
    //   assertion `left == right` failed
    //     left: (2,3)   <- frozen FromWindow(pipeline) of the blank document
    //     right: (2,4)  <- window.Document().pipeline_id() after the swap
    let alive = page
        .evaluate_js_web("40 + 2")
        .unwrap_or_else(|e| {
            panic!(
                "post-swap evaluate failed ({e}) — ScriptThread dead on the \
                 pending-timer assertion (timers.rs reuse-arm panic class)"
            )
        });
    assert_eq!(
        alive.trim().trim_matches('"'),
        "42",
        "ScriptThread did not answer after the reuse-arm navigation"
    );

    // §4 Settle window: any remaining pending blank-document timers fire (or
    // are dropped) across the swap here.
    std::thread::sleep(Duration::from_millis(400));

    // §5 No zombie fire: the superseded blank-document chain froze at the
    // swap (its pending timer event is dropped; the old OneshotTimers are
    // unreachable). Sample, keep pumping ~300ms, sample again.
    let c0 = page
        .evaluate_js_web("String(window.__f.contentWindow.__chain)")
        .expect("chain sample 0 failed");
    std::thread::sleep(Duration::from_millis(150));
    let _ = page.evaluate_js_web("1");
    std::thread::sleep(Duration::from_millis(150));
    let c1 = page
        .evaluate_js_web("String(window.__f.contentWindow.__chain)")
        .expect("chain sample 1 failed");
    eprintln!(
        "[timer-replacement] post-swap chain samples: c0={c0} c1={c1} \
         (a NUMBER means the Window was reused by window_for_replacement; \
         'undefined' means a fresh Window — reuse arm not exercised)"
    );
    let c0v = c0.trim().trim_matches('"').to_string();
    let c1v = c1.trim().trim_matches('"').to_string();
    assert!(
        c0v != "undefined" && c1v != "undefined",
        "iframe Window was NOT reused across the same-origin navigation \
         (window_for_replacement arm not exercised — probe invalid)"
    );
    assert_eq!(
        c0v, c1v,
        "superseded blank-document chain kept firing into the reused window \
         (zombie fire: {c0} → {c1})"
    );

    // §6 The replacement document's own timers still fire — the fix must
    // drop only stale-pipeline events, never silence live timer delivery.
    assert!(
        wait_until(
            &page,
            5_000,
            "(function(){ var w = window.__f.contentWindow; \
               if (w.__new_probe === undefined) { \
                 w.__new_probe = 0; \
                 w.setTimeout(function(){ w.__new_probe = 1; }, 30); \
               } \
               return String(w.__new_probe === 1); })()"
        ),
        "timers in the replacement document never fired — timer delivery over-suppressed"
    );
}
