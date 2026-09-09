// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:19] [level:integration]
// Three-axis fetch probe matrix — page/worker realm async fetch settlement.
//
// Provenance (e39/e40 P0 attribution wave, 2026-09-09): a page-realm async
// `fetch()` was reported to never settle while the SAME test family saw
// bun_runtime fetch tests green. This file is the live axis matrix that
// discriminates WHERE the black hole sits:
//
//   A axis — bao_runtime Bun API fetch (bun_runtime fetch_* tests) — green
//            baseline, asserted by that crate's own suite; not re-tested here.
//   B axis — servo PAGE realm async fetch(): `window.fetch` is bao's
//            Node-stack override (runtime_bridge install_fetch_global), so
//            this probes the override's promise-settlement round trip
//            (HTTPThread egress → ConcurrentTask resolve on the calling
//            ScriptThread's lazily-materialized MiniEventLoop).
//   B-rel  — same, with a RELATIVE URL (`fetch('/rel')`) — page-realm
//            conformance: the override receives the literal string (no
//            base-URL resolution is wired anywhere in the override chain).
//   T axis — page realm `setTimeout` settlement — same-class probe: bao's
//            install_timer_globals may shadow servo's native timers, and
//            bao timers ride the same never-ticked per-thread MiniEventLoop.
//   C axis — WORKER realm servo-native fetch: workers get no bao override
//            (the worker-scope callback installs only stealth inheritance),
//            so this probes servo's own fetch_async → FetchThread →
//            resource-thread → bridge → response-task round trip from a
//            worker thread.
//
// Every probe records BOTH sides of the wire: the fixture's server-side hit
// list (egress proof) and the page/worker-side settlement value. A black
// axis = fixture hit + settlement timeout. A green axis = both.
//
// Environment gating: real servo rendering requires DISPLAY (Xvfb).
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(fetch_axis_probe)'

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle};

// ---------------------------------------------------------------------------
// Minimal H1 fixture: per-path responder + server-side hit log
// ---------------------------------------------------------------------------

struct H1ProbeFixture {
    shutdown: Arc<AtomicBool>,
    hits: Arc<Mutex<Vec<String>>>,
    port: u16,
}

impl H1ProbeFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fetch probe fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (s2, h2) = (Arc::clone(&shutdown), Arc::clone(&hits));
        std::thread::Builder::new()
            .name("fetch-probe-fixture".into())
            .spawn(move || {
                while !s2.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut buf = Vec::new();
                            let mut tmp = [0u8; 2048];
                            let deadline = Instant::now() + Duration::from_secs(2);
                            while buf.windows(4).position(|w| w == b"\r\n\r\n").is_none()
                                && Instant::now() < deadline
                            {
                                match tcp.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                    Err(_) => break,
                                }
                            }
                            let head = String::from_utf8_lossy(&buf).to_string();
                            let path = head
                                .lines()
                                .next()
                                .and_then(|l| l.split_whitespace().nth(1))
                                .unwrap_or("")
                                .to_string();
                            h2.lock().unwrap().push(path.clone());
                            let (ct, body): (&str, &str) = if path.starts_with("/sw_probe") {
                                ("application/javascript", "")
                            } else {
                                ("text/plain", "probe-body")
                            };
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
                                ct = ct,
                                len = body.len(),
                                body = body,
                            );
                            let _ = tcp.write_all(resp.as_bytes());
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn fetch probe fixture thread");
        H1ProbeFixture {
            shutdown,
            hits,
            port,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn saw(&self, path: &str) -> bool {
        self.hits.lock().unwrap().iter().any(|p| p == path)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

impl Drop for H1ProbeFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Shared probe helpers
// ---------------------------------------------------------------------------

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

/// Drive servo's ScriptThread with no-op evaluates while polling a page-side
/// sink variable (the same pump pattern every live harness here uses).
fn poll_sink(page: &PageHandle, sink: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(v) = page.evaluate_js_web(&format!("window.{sink} === null ? '' : String(window.{sink})")) {
            let t = v.trim().trim_matches('"').to_string();
            if !t.is_empty() && t != "null" && t != "undefined" {
                return Some(t);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// B axis — page realm ABSOLUTE-URL async fetch settlement (e2e).
///
/// Asserts BOTH sides: the fixture serves /abs_probe (egress) AND the page's
/// `fetch().then()` chain settles with the response body. Before the
/// ScriptThread MiniEventLoop pump fix the settle side timed out while the
/// fixture still recorded the hit — the exact e39 black-hole signature.
#[test]
fn fetch_axis_probe_b_absolute() {
    if !common::run_isolated("fetch_axis_probe_tests::fetch_axis_probe_b_absolute") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = H1ProbeFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    let js = format!(
        "(function() {{ window.__fr = null; \
         fetch('{url}').then(function(r) {{ return r.text(); }}) \
         .then(function(t) {{ window.__fr = 'OK:' + t; }}) \
         .catch(function(e) {{ window.__fr = 'ERR:' + ((e && e.message) || String(e)); }}); \
         return 'sent'; }})()",
        url = fixture.url("/abs_probe")
    );
    let sent = page.evaluate_js_web(&js).expect("probe dispatch");
    assert!(sent.contains("sent"), "probe dispatch failed: {sent:?}");

    let settled = poll_sink(&page, "__fr", Duration::from_secs(15));
    eprintln!(
        "[b-abs] settled={settled:?} fixture_hits={:?}",
        fixture.hits()
    );
    assert!(
        fixture.saw("/abs_probe"),
        "B axis: fixture never saw /abs_probe (egress broken): {:?}",
        fixture.hits()
    );
    assert_eq!(
        settled.as_deref(),
        Some("OK:probe-body"),
        "B axis BLACK: page-realm async fetch never settled (fixture hit but promise hung) — \
         ScriptThread MiniEventLoop ConcurrentTask has no pumper. fixture: {:?}",
        fixture.hits()
    );
}

/// B axis variant — page realm RELATIVE-URL async fetch (`fetch('/rel')`).
///
/// The Node-stack override receives the literal '/rel_probe' string (no
/// base-URL resolution exists in the override chain). This probe pins what a
/// conformant page expects: resolution against the document base → same
/// settle shape as the absolute probe.
#[test]
fn fetch_axis_probe_b_relative() {
    if !common::run_isolated("fetch_axis_probe_tests::fetch_axis_probe_b_relative") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = H1ProbeFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    let js = "(function() { window.__fr = null; \
         fetch('/rel_probe').then(function(r) { return r.text(); }) \
         .then(function(t) { window.__fr = 'OK:' + t; }) \
         .catch(function(e) { window.__fr = 'ERR:' + ((e && e.message) || String(e)); }); \
         return 'sent'; })()";
    let sent = page.evaluate_js_web(js).expect("probe dispatch");
    assert!(sent.contains("sent"), "probe dispatch failed: {sent:?}");

    let settled = poll_sink(&page, "__fr", Duration::from_secs(15));
    eprintln!(
        "[b-rel] settled={settled:?} fixture_hits={:?}",
        fixture.hits()
    );
    assert!(
        fixture.saw("/rel_probe"),
        "B-rel axis: fixture never saw /rel_probe (relative URL never egressed): {:?}",
        fixture.hits()
    );
    assert_eq!(
        settled.as_deref(),
        Some("OK:probe-body"),
        "B-rel axis BLACK: relative fetch('/rel_probe') never settled (fixture hit but promise \
         hung, or base-URL resolution missing): {:?}",
        fixture.hits()
    );
}

/// T axis — page realm `setTimeout` settlement (same-class probe for the
/// per-thread MiniEventLoop: bao's install_timer_globals runs on page realms
/// and its callbacks dispatch through the same loop the fetch resolve uses).
#[test]
fn fetch_axis_probe_t_settimeout() {
    if !common::run_isolated("fetch_axis_probe_tests::fetch_axis_probe_t_settimeout") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = H1ProbeFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    let js = "(function() { window.__tr = null; \
         try { setTimeout(function() { window.__tr = 'tick'; }, 50); return 'armed'; } \
         catch (e) { return 'THREW:' + e; } })()";
    let armed = page.evaluate_js_web(js).expect("timer dispatch");
    assert!(armed.contains("armed"), "timer dispatch failed: {armed:?}");

    let settled = poll_sink(&page, "__tr", Duration::from_secs(10));
    eprintln!("[t-settimeout] settled={settled:?}");
    assert_eq!(
        settled.as_deref(),
        Some("tick"),
        "T axis BLACK: page-realm setTimeout callback never fired (same never-ticked \
         MiniEventLoop class as the fetch resolve)"
    );
}

/// URL-shape control probe — double-slash path (`//dbl`) from the PAGE realm.
///
/// Discriminates whether an `origin//path` URL (the fetchevent probe's
/// `__ORIGIN__` + "/sw-probe" concatenation shape) egresses at all: the SW
/// probe's publish fetch "completed" with a 200-shaped response while the
/// fixture never saw a connection. If the same URL shape misbehaves from a
/// plain page realm too, the defect is in the shared egress (bridge/bun URL
/// handling), not the SW thread.
#[test]
fn fetch_axis_probe_urlshape_double_slash() {
    if !common::run_isolated("fetch_axis_probe_tests::fetch_axis_probe_urlshape_double_slash") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = H1ProbeFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    let js = format!(
        "(function() {{ window.__fr = null; \
         fetch('{origin}//dbl_probe').then(function(r) {{ return r.text(); }}) \
         .then(function(t) {{ window.__fr = 'OK:' + t; }}) \
         .catch(function(e) {{ window.__fr = 'ERR:' + ((e && e.message) || String(e)); }}); \
         return 'sent'; }})()",
        origin = fixture.url("/"),
    );
    let sent = page.evaluate_js_web(&js).expect("probe dispatch");
    assert!(sent.contains("sent"), "probe dispatch failed: {sent:?}");

    let settled = poll_sink(&page, "__fr", Duration::from_secs(15));
    eprintln!(
        "[urlshape-dbl] settled={settled:?} fixture_hits={:?}",
        fixture.hits()
    );
    // The double slash is normalized to a single slash somewhere in the
    // egress chain (observed live: the fixture records "/dbl_probe").
    assert!(
        fixture.hits().iter().any(|p| p.starts_with("/dbl_probe")),
        "url-shape: fixture never saw //dbl_probe (or its normalized form): {:?}",
        fixture.hits()
    );
    assert_eq!(
        settled.as_deref(),
        Some("OK:probe-body"),
        "url-shape: double-slash fetch never settled with the fixture body: {:?}",
        fixture.hits()
    );
}

/// Query-string control probe — page-realm SYNC XHR (pure servo bridge path,
/// no node-stack involvement) with and without a query string.
///
/// The fetchevent SW probe's publish XHR (`/sw-probe?result=…`) entered the
/// bridge and "completed" with the PREVIOUS exchange's response bytes
/// (Content-Length 48 = the "/" document) while the fixture never saw a
/// connection. Every historically-green bridge test URL carries no query
/// string — this probe discriminates query-vs-not on the bridge path.
#[test]
fn fetch_axis_probe_query_string_xhr() {
    if !common::run_isolated("fetch_axis_probe_tests::fetch_axis_probe_query_string_xhr") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = H1ProbeFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    // One sync XHR WITHOUT a query (control), one WITH (the suspect shape).
    let js = "(function() { window.__x1 = null; window.__x2 = null; \
         try { var a = new XMLHttpRequest(); a.open('GET', '/plain', false); a.send(null); \
               window.__x1 = 'st=' + a.status + 'body=' + a.responseText; } \
         catch (e) { window.__x1 = 'THREW:' + e; } \
         try { var b = new XMLHttpRequest(); b.open('GET', '/withq?result=xyz', false); b.send(null); \
               window.__x2 = 'st=' + b.status + 'body=' + b.responseText; } \
         catch (e) { window.__x2 = 'THREW:' + e; } \
         return 'sent'; })()";
    let sent = page.evaluate_js_web(js).expect("probe dispatch");
    assert!(sent.contains("sent"), "probe dispatch failed: {sent:?}");

    let x1 = poll_sink(&page, "__x1", Duration::from_secs(10));
    let x2 = poll_sink(&page, "__x2", Duration::from_secs(10));
    eprintln!(
        "[query-xhr] plain={x1:?} withq={x2:?} fixture_hits={:?}",
        fixture.hits()
    );
    assert!(
        fixture.saw("/plain"),
        "control: fixture never saw /plain: {:?}",
        fixture.hits()
    );
    assert!(
        fixture.hits().iter().any(|p| p.starts_with("/withq")),
        "QUERY DEFECT: fixture never saw /withq (silently answered elsewhere): {:?}",
        fixture.hits()
    );
    assert!(
        x1.as_deref().is_some_and(|v| v.contains("st=200")),
        "control XHR must be a real 200: {x1:?}"
    );
    assert!(
        x2.as_deref().is_some_and(|v| v.contains("st=200body=probe-body")),
        "query XHR must return the fixture body, got: {x2:?}"
    );
}

/// C axis — WORKER realm servo-native fetch (workers carry no bao fetch
/// override; this exercises servo's own fetch_async → FetchThread →
/// resource-thread → bun bridge → response-task round trip from a worker
/// thread, with the result posted back via postMessage).
#[test]
fn fetch_axis_probe_c_worker_realm() {
    if !common::run_isolated("fetch_axis_probe_tests::fetch_axis_probe_c_worker_realm") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = H1ProbeFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    // Worker body: fetch the absolute URL (workers resolve against their
    // script URL for relative refs, but absolute keeps this probe purely
    // about the transport), post the settlement verdict back.
    let worker_body = format!(
        "var __p = fetch('{url}').then(function(r) {{ return r.text(); }}) \
         .then(function(t) {{ self.postMessage('OK:' + t); }}) \
         .catch(function(e) {{ self.postMessage('ERR:' + ((e && e.message) || String(e))); }});",
        url = fixture.url("/worker_probe")
    );
    let mut encoded = String::new();
    for b in worker_body.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char)
            },
            _ => encoded.push_str(&format!("%{b:02X}")),
        }
    }
    let driver = format!(
        "(function() {{ window.__wr = null; \
         try {{ var w = new Worker(\"data:text/javascript,{encoded}\"); \
         w.onmessage = function(e) {{ window.__wr = String(e.data); }}; \
         w.onerror = function(ev) {{ window.__wr = 'WORKER-ERROR:' + ((ev && ev.message) || 'unknown'); return true; }}; \
         return 'worker-created'; }} catch (e) {{ window.__wr = 'CREATE-ERROR:' + e; return 'failed'; }} }})()",
        encoded = encoded
    );
    let created = page.evaluate_js_web(&driver).expect("worker dispatch");
    assert!(
        created.contains("worker-created"),
        "worker creation failed: {created:?}"
    );

    let settled = poll_sink(&page, "__wr", Duration::from_secs(15));
    eprintln!(
        "[c-worker] settled={settled:?} fixture_hits={:?}",
        fixture.hits()
    );
    assert!(
        fixture.saw("/worker_probe"),
        "C axis: fixture never saw /worker_probe (worker fetch egress broken): {:?}",
        fixture.hits()
    );
    assert_eq!(
        settled.as_deref(),
        Some("OK:probe-body"),
        "C axis BLACK: worker-realm servo fetch never settled (fixture hit but postMessage \
         verdict never arrived): {:?}",
        fixture.hits()
    );
}
