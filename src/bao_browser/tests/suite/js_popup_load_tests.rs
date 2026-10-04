// @trace TEST-BRW-002-JS-POPUP-LOAD [req:REQ-BRW-002] [level:e2e]
// Top-level `javascript:` URL navigation load-completion E2E (REQ-BRW-002,
// e59 D1 attribution): a popup opened with a `javascript:` URL whose script
// does NOT return a string creates no new document, so the popup stays on its
// initial `about:blank` document — and that document's `load` event never
// fired, hanging any opener that awaits it.
//
// Real-path contract under test (no mocks anywhere):
//
//   opener page JS: window.open('javascript: ...')
//     → WindowProxy::create_auxiliary_web_view (initial about:blank document)
//     → navigate() step 20 → navigate_javascript task
//     → ScriptThread::navigate_to_javascript_url
//     → evaluate_a_javascript_url → no string result → newDocument null
//     → load-completion signaling for the unchanged navigable
//     → window `load` event on the popup
//
// The defect this pins (e59, 4/4 sessions): the popup script runs
// (readyState=complete, side effects observable) but `load` never arrives —
// the iframe arm of step 8 signals load completion for framed navigables
// (run_iframe_load_event_steps) while the top-level arm had no counterpart.
// WPT carrier: fetch/fetch-later/new-window.https.window.html blank-window
// variants (2 targets × 2 features) all await `load` on exactly this shape.
//
// Both js: result shapes are covered:
//   1. non-string result (`javascript: void 0`) — the WPT blank-window shape,
//      RED before the fix (load never fires);
//   2. string result (`javascript:"<html><title>t</title>"`) — produces a real
//      document through the LoadUrl pipeline; load must fire AND the document
//      must carry the evaluated HTML (title check).
//
// Fixture semantics: the opener must ride a real origin (http://127.0.0.1),
// because navigate_to_javascript_url rejects cross-origin-domain initiators
// and a data:-URL opener's opaque origin never matches the popup's inherited
// origin — same reason the e59 probes ran a local HTTP server.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle, PagePool};

// ---------------------------------------------------------------------------
// Fixture — one H1 keep-alive server, opener page only
// ---------------------------------------------------------------------------

struct PopupFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
}

impl PopupFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind popup fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_c = Arc::clone(&shutdown);

        std::thread::Builder::new()
            .name("popup-fixture".into())
            .spawn(move || {
                listener.set_nonblocking(true).expect("nonblocking listener");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((tcp, _)) => Self::serve_connection(tcp),
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn popup fixture");

        PopupFixture { port, shutdown }
    }

    /// HTTP/1.1 keep-alive loop with Content-Length framing (the webfont
    /// fixture shape — close-delimited responses would mask wire defects).
    fn serve_connection(mut tcp: TcpStream) {
        let _ = tcp.set_nonblocking(false);
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(30)));
        let mut buf = [0u8; 8192];
        loop {
            let mut head = Vec::new();
            loop {
                match tcp.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => {
                        head.extend_from_slice(&buf[..n]);
                        if head.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    },
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock ||
                            e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        return
                    },
                    Err(_) => return,
                }
            }
            let head_str = String::from_utf8_lossy(&head).to_string();
            let path = head_str
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            let (status, body): (&str, &[u8]) = if path == "/" {
                ("200 OK", OPENER_PAGE.as_bytes())
            } else {
                ("404 Not Found", b"")
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html\r\n\
                 Cache-Control: no-store\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            if tcp.write_all(response.as_bytes()).is_err() {
                return;
            }
            if !body.is_empty() && tcp.write_all(body).is_err() {
                return;
            }
            let _ = tcp.flush();
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for PopupFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

const OPENER_PAGE: &str = "<!DOCTYPE html><html><head></head><body>opener</body></html>";

/// The non-string js: result — the WPT new-window blank-window shape. The
/// popup window must receive a `load` event (bounded window) even though no
/// document was created. The js: expression also stamps the popup document
/// (`document.title = 'ran'`) so a red run distinguishes "script ran, load
/// missing" (the D1 defect) from "navigation never reached the popup"
/// (harness invalidity). The popup handle is kept in a global so the test can
/// read the stamp directly at settle time (poll-time read — an interval
/// sampler would race the load event returning first).
const OPEN_NON_STRING: &str = r#"(() => {
  globalThis.__d1a = { open: 'NULL', load: false, err: null };
  try {
    const w = window.open('javascript: void((document.title = "ran"))', '', '');
    globalThis.__d1a.open = w ? 'OBJ' : 'NULL';
    globalThis.__d1a_w = w;
    if (w) w.addEventListener('load', () => { globalThis.__d1a.load = true; });
  } catch (e) { globalThis.__d1a.err = String(e); }
  return JSON.stringify(globalThis.__d1a);
})()"#;

/// Direct poll-time read of the popup document state (no interval sampler).
const PROBE_POPUP_STATE: &str = r#"(() => {
  try {
    const w = globalThis.__d1a_w;
    return JSON.stringify({
      ran: w.document.title,
      href: w.location.href,
      rs: w.document.readyState,
    });
  } catch (e) { return JSON.stringify({ err: String(e) }); }
})()"#;

/// The string js: result — the evaluated HTML becomes a real document through
/// the LoadUrl pipeline. Load must fire AND the popup must carry the document
/// produced from the evaluated string.
const OPEN_STRING: &str = r#"(() => {
  globalThis.__d1b = { open: 'NULL', load: false, title: '', err: null };
  try {
    const w = window.open('javascript:"<html><title>t</title>"', '', '');
    globalThis.__d1b.open = w ? 'OBJ' : 'NULL';
    if (w) w.addEventListener('load', () => {
      globalThis.__d1b.load = true;
      try { globalThis.__d1b.title = w.document.title; } catch (e) {
        globalThis.__d1b.title = 'ERR:' + String(e);
      }
    });
  } catch (e) { globalThis.__d1b.err = String(e); }
  return JSON.stringify(globalThis.__d1b);
})()"#;

// ---------------------------------------------------------------------------
// Page helpers (webfont_double_load_tests.rs form)
// ---------------------------------------------------------------------------

fn js(page: &PageHandle, expr: &str) -> String {
    page.evaluate_js_web(expr)
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .to_string()
}

/// Pump the servo event loop while polling the opener page until `expr`
/// evaluates to true (bounded). Returns the final flag snapshot for
/// diagnostics. The pump is load-bearing: the popup's navigate_javascript
/// task only runs while the in-process event loop spins.
fn pump_until(
    runtime: &BrowserRuntime,
    opener: &PageHandle,
    cond: &str,
    snapshot: &str,
    max: Duration,
) -> String {
    let start = Instant::now();
    loop {
        if js(opener, cond) == "true" {
            return js(opener, snapshot);
        }
        if start.elapsed() >= max {
            return js(opener, snapshot);
        }
        runtime.pump_cdp(Duration::from_millis(25));
    }
}

fn create_opener(runtime: &BrowserRuntime, url: &str) -> PageHandle {
    let pool: &PagePool = runtime.page_pool();
    let mut page = None;
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(url.to_string()),
            ..Default::default()
        }) {
            Ok(p) => {
                page = Some(p);
                break;
            },
            Err(e) => {
                eprintln!("page creation failed (retrying): {}", e);
                std::thread::sleep(Duration::from_secs(3));
            },
        }
    }
    page.expect("opener page creation failed after retries")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// RED lock for the top-level js: navigation load-completion gap (e59 D1):
/// `window.open('javascript: void 0')` runs the script but creates no
/// document; the popup's `load` event must still fire within a bounded
/// window (WPT new-window blank-window carriers hang awaiting it).
#[test]
fn js_popup_non_string_result_fires_load() {
    bun_core::Output::init_test();

    let fixture = PopupFixture::spawn();
    let runtime =
        bao_browser::BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let opener = create_opener(&runtime, &fixture.url());

    let open_snapshot = js(&opener, OPEN_NON_STRING);

    // Bounded window for the load event. The defect's signature is a
    // permanent hang: the script side effects land, load never does.
    let window = Duration::from_secs(30);
    let start = Instant::now();
    let final_state = pump_until(
        &runtime,
        &opener,
        "globalThis.__d1a && globalThis.__d1a.load",
        "JSON.stringify(globalThis.__d1a)",
        window,
    );
    // Settle read: sample the popup document state directly (no interval
    // sampler — those race the load event returning first).
    runtime.pump_cdp(Duration::from_millis(300));
    let popup_state = js(&opener, PROBE_POPUP_STATE);
    let elapsed = start.elapsed();

    let mut problems = Vec::new();
    if !open_snapshot.contains("\"open\":\"OBJ\"") {
        problems.push(format!(
            "window.open must return a non-null Window (harness validity) — open snapshot: \
             {open_snapshot}"
        ));
    }
    if open_snapshot.contains("\"err\":") && !open_snapshot.contains("\"err\":null") {
        problems.push(format!("window.open threw: {open_snapshot}"));
    }
    if !final_state.contains("\"load\":true") {
        problems.push(format!(
            "popup load event never fired within {}ms (e59 D1 defect shape) — final state: \
             {final_state}; popup state at settle: {popup_state}",
            elapsed.as_millis()
        ));
    }

    if !problems.is_empty() {
        panic!(
            "js: popup (non-string result) load contract violated:\n  - {}\n  diag: \
             open-at-open={open_snapshot} popup-state={popup_state}",
            problems.join("\n  - "),
        );
    }
    if !popup_state.contains("\"ran\":\"ran\"") {
        // Diagnostic only on the green path: the load contract is met; the
        // side-effect stamp is a probe, not the contract. (Pre-fix red runs
        // observed ran:'ran' with load missing — the D1 attribution anchor.)
        eprintln!(
            "e62 diag: js: side-effect stamp not sampled at settle — popup state: \
             {popup_state}"
        );
    }
    eprintln!(
        "e62 green diag: non-string popup load after {}ms state={final_state} popup-state=\
         {popup_state}",
        elapsed.as_millis()
    );

    runtime.page_pool().close_all();
}

/// Second axis of the WPT new-window blank ×4 carriers: `features='popup'`.
/// (target='' and target='_blank' are the same path — window-open step 5
/// normalizes the empty target to '_blank' — so the 2×2 matrix collapses to
/// the two features values, each covered once here.)
#[test]
fn js_popup_non_string_result_fires_load_popup_features() {
    bun_core::Output::init_test();

    let fixture = PopupFixture::spawn();
    let runtime =
        bao_browser::BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let opener = create_opener(&runtime, &fixture.url());

    let open_expr = r#"(() => {
  globalThis.__d1c = { open: 'NULL', load: false, err: null };
  try {
    const w = window.open('javascript: void 0', '_blank', 'popup');
    globalThis.__d1c.open = w ? 'OBJ' : 'NULL';
    if (w) w.addEventListener('load', () => { globalThis.__d1c.load = true; });
  } catch (e) { globalThis.__d1c.err = String(e); }
  return JSON.stringify(globalThis.__d1c);
})()"#;
    let open_snapshot = js(&opener, open_expr);

    let window = Duration::from_secs(30);
    let final_state = pump_until(
        &runtime,
        &opener,
        "globalThis.__d1c && globalThis.__d1c.load",
        "JSON.stringify(globalThis.__d1c)",
        window,
    );

    let mut problems = Vec::new();
    if !open_snapshot.contains("\"open\":\"OBJ\"") {
        problems.push(format!(
            "window.open must return a non-null Window (harness validity) — open snapshot: \
             {open_snapshot}"
        ));
    }
    if !final_state.contains("\"load\":true") {
        problems.push(format!(
            "popup (features='popup') load event never fired within {}ms — final state: \
             {final_state}",
            window.as_millis()
        ));
    }

    if !problems.is_empty() {
        panic!(
            "js: popup (features='popup') load contract violated:\n  - {}",
            problems.join("\n  - ")
        );
    }

    runtime.page_pool().close_all();
}

/// String-result shape: `window.open('javascript:"<html>…</title>"')` creates
/// a real document through the LoadUrl pipeline. Load must fire AND the popup
/// document must carry the evaluated HTML (title check pins the document
/// replacement, not just the event).
#[test]
fn js_popup_string_result_fires_load_with_document() {
    bun_core::Output::init_test();

    let fixture = PopupFixture::spawn();
    let runtime =
        bao_browser::BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let opener = create_opener(&runtime, &fixture.url());

    let open_snapshot = js(&opener, OPEN_STRING);

    let window = Duration::from_secs(30);
    let start = Instant::now();
    let final_state = pump_until(
        &runtime,
        &opener,
        "globalThis.__d1b && globalThis.__d1b.load",
        "JSON.stringify(globalThis.__d1b)",
        window,
    );
    let elapsed = start.elapsed();

    let mut problems = Vec::new();
    if !open_snapshot.contains("\"open\":\"OBJ\"") {
        problems.push(format!(
            "window.open must return a non-null Window (harness validity) — open snapshot: \
             {open_snapshot}"
        ));
    }
    if !final_state.contains("\"load\":true") {
        problems.push(format!(
            "popup load event never fired within {}ms — final state: {final_state}",
            elapsed.as_millis()
        ));
    }
    if !final_state.contains("\"title\":\"t\"") {
        problems.push(format!(
            "popup document must carry the evaluated HTML (title 't') — final state: \
             {final_state}"
        ));
    }

    if !problems.is_empty() {
        panic!(
            "js: popup (string result) load contract violated:\n  - {}",
            problems.join("\n  - ")
        );
    }
    eprintln!(
        "e62 green diag: string popup load after {}ms state={final_state}",
        elapsed.as_millis()
    );

    runtime.page_pool().close_all();
}
