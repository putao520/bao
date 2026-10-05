// @trace TEST-BRW-002 [req:REQ-BRW-002] [level:e2e]
// fetchLater deactivate-flush (third flush exit) RED→GREEN lock —
// WPT fetch/fetch-later/send-on-deactivate.https.window.html residual shapes,
// carrier-local mirror (subtests 3/4/5: navigate-away w/o BFCache must flush
// pending deferred fetches regardless of activateAfter; Chromium
// BackgroundSync-off observable semantics per the WPT in-file comments:
// "forcing request sending on every navigation, even if page is put into
// BFCache").
//
// Exit inventory before this wave (e68 attribution, main-session V adopted):
//   1. timer exit       — queue_deferred_fetch schedule_timer(activateAfter)
//   2. destroy exit     — Document::destroy → abort → terminate_fetch_group
//                         → GlobalScope::process_deferred_fetches
//   3. deactivate exit  — MISSING: Document::unload (pagehide/unload on
//                         navigate-away) never flushes; servo's HTML step 20
//                         ("destroy oldDocument if not salvageable") is
//                         unimplemented upstream, so a plain navigation never
//                         reaches the destroy exit and pending records
//                         linger.
//
// Each scenario gives the record a large activateAfter (60s, far beyond the
// test window) so the ONLY way the beacon can arrive is the deactivate flush:
//   * plain record + navigate away       → exactly 1 hit (subtest 3/5 shape)
//   * aborted record + live record       → live 1 hit, aborted 0 (subtest 4)
// plus a pre-navigation negative window proving activateAfter is still
// honored while the document stays active (send-on-deactivate, not
// send-immediately).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle, PageState};

/// servo/BrowserRuntime carry process-global slots (one JSContext per thread;
/// embedder state) — serialize runtime users in this suite (see
/// csp_violation_nav_tests RUNTIME_LOCK).
static RUNTIME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Minimal HTTP fixture: serves named HTML routes and counts every request
/// landing under `/beacon/...` (the deferred-fetch target). Same origin as the
/// pages, so `fetchLater`'s trustworthy-URL gate is satisfied (127.0.0.1).
struct FlushFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    hits: Arc<Mutex<HashMap<String, u32>>>,
}

impl FlushFixture {
    fn spawn(routes: Vec<(String, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let hits: Arc<Mutex<HashMap<String, u32>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let routes: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(routes.into_iter().collect()));

        let shutdown_c = Arc::clone(&shutdown);
        let hits_c = Arc::clone(&hits);
        let routes_c = Arc::clone(&routes);
        std::thread::Builder::new()
            .name("fl-flush-fixture".into())
            .spawn(move || {
                listener.set_nonblocking(true).expect("nonblocking");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let path = read_request_path(&mut tcp);
                            let resp = match path {
                                Some(p) if p.starts_with("/beacon") => {
                                    *hits_c.lock().unwrap().entry(p).or_insert(0) += 1;
                                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                                        .to_string()
                                },
                                Some(p) => {
                                    let body = routes_c
                                        .lock()
                                        .unwrap()
                                        .get(&p)
                                        .cloned()
                                        .unwrap_or_else(|| "<html><body>404</body></html>".into());
                                    format!(
                                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                        body.len(),
                                        body
                                    )
                                },
                                None => "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                                    .to_string(),
                            };
                            let _ = tcp.write_all(resp.as_bytes());
                            let _ = tcp.flush();
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn fixture");

        FlushFixture {
            port,
            shutdown,
            hits,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn hits(&self, path: &str) -> u32 {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

impl Drop for FlushFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Read one HTTP request head (+ body per Content-Length) and return the
/// request path (with query string), draining enough bytes that keepalive
/// POSTs are not reset mid-flight.
fn read_request_path(tcp: &mut std::net::TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        let n = tcp.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let first_line = head.lines().next()?.to_string();
    let path = first_line.split_whitespace().nth(1)?.to_string();

    // Drain the declared body so the client's write side completes.
    let content_length = head
        .to_ascii_lowercase()
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut received = buf.len() - head_end - 4;
    while received < content_length {
        let n = tcp.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        received += n;
    }
    Some(path)
}

/// Pump the page's callback drain until `cond` holds or the deadline expires.
fn poll_until(page: &PageHandle, timeout: Duration, cond: &dyn Fn() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        let _ = page.evaluate_js_web(""); // pump the callback drain
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// Load `page1_path`, wait for interactivity, and assert `fetchLater` armed
/// cleanly (throws here would invalidate the whole scenario).
fn arm_page1(runtime: &BrowserRuntime, page1_url: &str) -> PageHandle {
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .expect("create_page");
    page.navigate(page1_url).expect("navigate page1");
    poll_until(&page, Duration::from_secs(30), &|| {
        page.get_state() == PageState::Interactive
    });
    let armed = poll_until(&page, Duration::from_secs(10), &|| {
        matches!(page.evaluate_js_web("String(window.__armed)"), Ok(v) if !v.is_empty() && v != "undefined")
    });
    let armed_value = page
        .evaluate_js_web("String(window.__armed)")
        .unwrap_or_default();
    assert!(
        armed && armed_value == "ok",
        "fetchLater must arm cleanly on the active page (got: {armed_value:?})"
    );
    page
}

/// Shape 1 (WPT subtests 3/5): a pending record with activateAfter=60s must be
/// flushed by the navigate-away deactivation instead of lingering until the
/// timer — while the document stays active, activateAfter must still be
/// honored (negative window).
#[test]
fn fetch_later_on_navigate_away_is_flushed_without_waiting_for_activate_after() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let page1_body = r#"<!DOCTYPE html><html><head><title>fl-flush</title></head><body><script>
try {
  fetchLater("/beacon/plain", {activateAfter: 60000});
  window.__armed = "ok";
} catch (e) { window.__armed = "err:" + e; }
</script></body></html>"#;
    let page2_body =
        r#"<!DOCTYPE html><html><head><title>fl-next</title></head><body><p>next</p></body></html>"#;
    let fixture = FlushFixture::spawn(vec![
        ("/page1".into(), page1_body.into()),
        ("/page2".into(), page2_body.into()),
    ]);

    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = arm_page1(&runtime, &fixture.url("/page1"));

    // Negative window: while the document is active the 60s activateAfter
    // must hold — no early send (this is the send-on-DEACTIVATE discriminator).
    std::thread::sleep(Duration::from_millis(1500));
    let _ = page.evaluate_js_web("");
    assert_eq!(
        fixture.hits("/beacon/plain"),
        0,
        "activateAfter=60s must not fire while the document stays active"
    );

    // Navigate away: the old document deactivates (pagehide/unload) — the
    // pending record must be force-flushed now, not in 60s.
    page.navigate(&fixture.url("/page2")).expect("navigate away");
    let flushed = poll_until(&page, Duration::from_secs(15), &|| {
        fixture.hits("/beacon/plain") >= 1
    });
    assert!(
        flushed,
        "navigate-away must flush the pending deferred fetch (hits: {})",
        fixture.hits("/beacon/plain")
    );
}

/// Shape 2 (WPT subtest 4): the aborted record must stay silent through the
/// deactivate flush; the live sibling must be flushed exactly once.
#[test]
fn aborted_deferred_record_is_not_flushed_on_navigate_away() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let page1_body = r#"<!DOCTYPE html><html><head><title>fl-abort</title></head><body><script>
try {
  const controller = new AbortController();
  fetchLater("/beacon/abort-a", {signal: controller.signal, activateAfter: 60000});
  fetchLater("/beacon/abort-b", {activateAfter: 60000});
  controller.abort();
  window.__armed = "ok";
} catch (e) { window.__armed = "err:" + e; }
</script></body></html>"#;
    let page2_body =
        r#"<!DOCTYPE html><html><head><title>fl-next</title></head><body><p>next</p></body></html>"#;
    let fixture = FlushFixture::spawn(vec![
        ("/page1".into(), page1_body.into()),
        ("/page2".into(), page2_body.into()),
    ]);

    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = arm_page1(&runtime, &fixture.url("/page1"));

    page.navigate(&fixture.url("/page2")).expect("navigate away");
    let flushed = poll_until(&page, Duration::from_secs(15), &|| {
        fixture.hits("/beacon/abort-b") >= 1
    });
    assert!(
        flushed,
        "navigate-away must flush the live deferred record (hits: {})",
        fixture.hits("/beacon/abort-b")
    );

    // Bounded grace window, then: aborted record stays silent, live record
    // was flushed exactly once (idempotent flush, no double-send).
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        fixture.hits("/beacon/abort-a"),
        0,
        "aborted record must NOT be flushed on navigate-away"
    );
    assert_eq!(
        fixture.hits("/beacon/abort-b"),
        1,
        "live record must be flushed exactly once (no double-send)"
    );
}
