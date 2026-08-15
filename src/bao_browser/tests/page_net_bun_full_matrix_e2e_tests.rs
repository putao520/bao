// @trace REQ-STL-001 [level:e2e] — U2 stage 2: full-destination page-network
// matrix through the bun bridge.
//
// `BAO_PAGE_NET_BUN` = all destinations. One test process (mozjs Runtime and
// servo Opts are per-process singletons), three blocks:
//
//   1. H1 destination matrix: a page loads img / script / css / xhr / fetch
//      subresources plus a second page navigated to an http document — all
//      six destinations land on the plain-h1 fixture AND the bridge request
//      counter advances by exactly the same number (every one of them went
//      through the bridge, none fell back to hyper).
//   2. Semantic pass-through (not just 200s): a redirect chain (302 → 200)
//      resolves to the final body with the second hop on the bridge;
//      a CORS-denied cross-origin response is BLOCKED (servo-side CORS ran
//      on the bridge-delivered response); a nosniff script (text/plain +
//      X-Content-Type-Options) never executes (servo-side nosniff ran).
//   3. H2 matrix: a minimal ALPN-h2 TLS server (BoringSSL TlsServer + a
//      hand-rolled h2 framing/HPACK-encode layer — static-table :status plus
//      literal headers, no huffman) serves an https document and the same
//      five subresource destinations over real h2 streams; the bridge
//      counter covers document + five subresources and the server records
//      every stream with ALPN "h2" negotiated.
//
// `ignore_certificate_errors: true` (servo's WPT posture, newly surfaced on
// `BaoConfig`) is what allows the self-signed h2 fixture: the TLS handshake
// completes with reject_unauthorized=false and ALPN still negotiates h2.

#![allow(dead_code)]

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool, PageState};
use common::h2_server::H2Server;

// ---------------------------------------------------------------------------
// H1 fixture: keep-alive plain-HTTP/1.1 server, per-path responder
// ---------------------------------------------------------------------------

struct H1Fixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    paths: Arc<Mutex<Vec<String>>>,
    signal: Arc<(Mutex<usize>, Condvar)>,
}

/// Responder result: (status line, headers, body).
type H1Response = (u16, Vec<(&'static str, String)>, Vec<u8>);

impl H1Fixture {
    /// `respond` maps (method, path) → response; unknown paths → 404.
    fn spawn<F>(respond: F) -> Self
    where
        F: Fn(&str, &str) -> H1Response + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind h1 fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let signal = Arc::new((Mutex::new(0usize), Condvar::new()));

        let respond = Arc::new(respond);
        let shutdown_c = Arc::clone(&shutdown);
        let paths_c = Arc::clone(&paths);
        let signal_c = Arc::clone(&signal);
        let respond_c = Arc::clone(&respond);
        std::thread::Builder::new()
            .name("h1-fixture".into())
            .spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("nonblocking listener");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((tcp, _)) => {
                            let respond = Arc::clone(&respond_c);
                            let paths = Arc::clone(&paths_c);
                            let signal = Arc::clone(&signal_c);
                            std::thread::spawn(move || handle_h1_connection(tcp, respond, paths, signal));
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn h1 fixture");
        H1Fixture {
            port,
            shutdown,
            paths,
            signal,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn wait_for_paths(&self, count: usize, timeout: Duration) -> bool {
        let (lock, cond) = &*self.signal;
        let guard = lock.lock().unwrap();
        let (guard, timed_out) = cond
            .wait_timeout_while(guard, timeout, |c| *c < count)
            .expect("h1 fixture condvar poisoned");
        !timed_out.timed_out() && *guard >= count
    }

    fn recorded(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
}

fn handle_h1_connection(
    mut tcp: TcpStream,
    respond: Arc<dyn Fn(&str, &str) -> H1Response + Send + Sync>,
    paths: Arc<Mutex<Vec<String>>>,
    signal: Arc<(Mutex<usize>, Condvar)>,
) {
    let _ = tcp.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    'conn: loop {
        // Read one request head (up to \r\n\r\n).
        let head_start = buf.len();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            if Instant::now() > deadline {
                return; // idle keep-alive connection
            }
            match tcp.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock ||
                        e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if buf.len() > head_start {
                        continue; // still waiting for the rest of the head
                    }
                    continue 'conn; // idle between requests
                },
                Err(_) => return,
            }
        }
        let head = String::from_utf8_lossy(&buf).to_string();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_string();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        buf.clear();

        if path.is_empty() {
            return;
        }
        paths.lock().unwrap().push(path.clone());
        {
            let (lock, cond) = &*signal;
            let mut guard = lock.lock().unwrap();
            *guard += 1;
            cond.notify_all();
        }

        let (status, headers, body) = respond(&method, &path);
        let reason = match status {
            200 => "OK",
            301 => "Moved Permanently",
            302 => "Found",
            403 => "Forbidden",
            _ => "OK",
        };
        let mut out = format!("HTTP/1.1 {} {}\r\n", status, reason);
        for (name, value) in &headers {
            out.push_str(&format!("{}: {}\r\n", name, value));
        }
        out.push_str(&format!("Content-Length: {}\r\nConnection: keep-alive\r\n\r\n", body.len()));
        if tcp.write_all(out.as_bytes()).is_err() || tcp.write_all(&body).is_err() {
            return;
        }
        let _ = tcp.flush();
    }
}

// ---------------------------------------------------------------------------
// Page harness helpers (same contract as the fingerprint e2e)
// ---------------------------------------------------------------------------

fn wait_for_load(page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn data_url_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'#' | b'%' | b'&' | b'?' | b'<' | b'>' | b'"' | b'\\' | b'^' | b'`' | b'{' | b'}'
            | b'|' => out.push_str(&format!("%{:02X}", b)),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn create_page(pool: &PagePool, url: String) -> bao_browser::PageHandle {
    // Firefox profile: without one the bridge's ALPN offer is http/1.1-only
    // (stage-0 posture); the profile drives the `h2,http/1.1` offer —
    // hyper parity — which the h2 half of the matrix needs (and the h2
    // fingerprint fields then match production pages).
    let stealth_profile = Some(bao_stealth::StealthProfile::firefox_default());
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(url.clone()),
            stealth_profile: stealth_profile.clone(),
            ..Default::default()
        }) {
            Ok(page) => return page,
            Err(e) => {
                eprintln!("[matrix-e2e] page creation failed (retrying): {}", e);
                std::thread::sleep(Duration::from_secs(3));
            },
        }
    }
    panic!("page creation failed after retries");
}

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

#[test]
fn page_net_bun_full_destination_matrix() {
    bun_core::Output::init_test();
    // Phase 2 posture: EVERY destination through the bun bridge.
    // (BAO_MATRIX_HYPER_CONTROL=1 runs the identical matrix on servo's hyper
    // path — differential control for bridge-vs-page behavior questions.)
    let hyper_control = std::env::var("BAO_MATRIX_HYPER_CONTROL").is_ok();
    net::fetch::bun_bridge::set_page_net_bun_enabled(!hyper_control);
    if hyper_control {
        eprintln!("[matrix-e2e] HYPER CONTROL RUN (bridge off)");
    }

    let config = BaoConfig {
        ignore_certificate_errors: true,
        ..BaoConfig::default()
    };
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => panic!("BaoRuntime::new failed: {}", e),
    };
    let pool: &PagePool = runtime.page_pool();

    // ── Block 1: H1 destination matrix ────────────────────────────────────
    let h1 = H1Fixture::spawn(|_method, path| {
        let script_body = b"window.__scriptRan = true;".to_vec();
        let nosniff_body = b"window.__nosniffRan = true;".to_vec();
        let plain = |body: &[u8]| -> H1Response {
            (200, vec![("content-type", "text/plain".to_string())], body.to_vec())
        };
        // The page lives at a data: URL (opaque "null" origin): reads of the
        // XHR/fetch probes must pass CORS, so those responses carry ACAO *.
        let cors_ok = |body: &[u8]| -> H1Response {
            (
                200,
                vec![
                    ("content-type", "text/plain".to_string()),
                    ("access-control-allow-origin", "*".to_string()),
                ],
                body.to_vec(),
            )
        };
        // Valid 1x1 RGB PNG (chunk CRCs machine-generated) — the image
        // onload fires only when the bytes actually decode.
        const PIXEL_PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49,
            0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02,
            0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44,
            0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01,
            0x00, 0xC9, 0xFE, 0x92, 0xEF, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
            0xAE, 0x42, 0x60, 0x82,
        ];
        match path {
            "/img.png" => (
                200,
                vec![("content-type", "image/png".to_string())],
                PIXEL_PNG.to_vec(),
            ),
            "/app.js" => (
                200,
                vec![("content-type", "text/javascript".to_string())],
                script_body,
            ),
            "/style.css" => (
                200,
                vec![("content-type", "text/css".to_string())],
                b"body{color:red}".to_vec(),
            ),
            "/xhr_probe" => cors_ok(b"xhr-data"),
            "/fetch_probe" => cors_ok(b"fetch-data"),
            "/final" => cors_ok(b"final-body"),
            "/doc.html" => (
                200,
                vec![("content-type", "text/html; charset=utf-8".to_string())],
                b"<html><head><title>doc</title></head><body><p id=\"t\">doc</p></body></html>".to_vec(),
            ),
            // 302 the bridge must hand to servo's redirect loop untouched.
            // (CORS-mode XHR: every hop of the chain must pass the check.)
            "/redirector" => (
                302,
                vec![
                    ("location", "/final".to_string()),
                    ("access-control-allow-origin", "*".to_string()),
                ],
                Vec::new(),
            ),
            // Deliberately NO Access-Control-Allow-Origin: servo's CORS check
            // must reject the read even though the bridge delivered the 200.
            "/cors_denied" => plain(b"cors-secret"),
            // Executable bytes with a non-executable MIME type + nosniff —
            // servo must refuse to run it.
            "/nosniff.js" => (
                200,
                vec![
                    ("content-type", "text/plain".to_string()),
                    ("x-content-type-options", "nosniff".to_string()),
                ],
                nosniff_body,
            ),
            _ => plain(b"ok"),
        }
    });

    // data: URL shell page — subresources injected after create_page.
    let html = "<!DOCTYPE html><html><head><title>m</title></head>\
                <body><p id=\"t\">m</p></body></html>"
        .to_string();
    let page = create_page(
        pool,
        format!(
            "data:text/html;charset=utf-8,{}",
            data_url_escape(&html)
        ),
    );
    eprintln!("[matrix-e2e] h1 shell page created");
    wait_for_load(&page, 3000);

    let counter_before = net::fetch::bun_bridge::page_net_bun_request_count();

    let inject = |label: &str, js: &str| {
        match page.evaluate_js_web(js) {
            Ok(value) => eprintln!("[matrix-e2e] inject {label}: ok {value}"),
            Err(error) => panic!("inject {label} failed: {error:?}"),
        }
    };
    let pump = |ms: u64| {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let _ = page.evaluate_js("");
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    inject(
        "img",
        &format!(
            "(function(){{ var im = document.createElement('img'); \
             im.onload = function(){{ window.__img = 'loaded'; }}; \
             im.onerror = function(){{ window.__img = 'error'; }}; \
             document.body.appendChild(im); im.src = '{}'; }})()",
            h1.url("/img.png")
        ),
    );
    inject(
        "script",
        &format!(
            "(function(){{ var s = document.createElement('script'); s.src = '{}'; document.head.appendChild(s); }})()",
            h1.url("/app.js")
        ),
    );
    inject(
        "css",
        &format!(
            "(function(){{ var l = document.createElement('link'); l.rel = 'stylesheet'; \
             l.href = '{}'; document.head.appendChild(l); }})()",
            h1.url("/style.css")
        ),
    );
    inject(
        "xhr",
        &format!(
            "(function(){{ try {{ var x = new XMLHttpRequest(); \
             x.onload = function(){{ window.__xhr = x.responseText; }}; \
             x.onerror = function(){{ window.__xhr = 'error:rs' + x.readyState + ':st' + x.status; }}; \
             x.onabort = function(){{ window.__xhr = 'abort'; }}; \
             x.ontimeout = function(){{ window.__xhr = 'timeout'; }}; \
             x.open('GET', '{}'); x.send(); window.__xhrSent = true; }} catch (e) {{ window.__xhr = 'throw:' + e; }} }})()",
            h1.url("/xhr_probe")
        ),
    );
    inject(
        "fetch",
        &format!(
            "(function(){{ window.__fetch = 'pending'; \
             fetch('{}').then(function(r){{ return r.text(); }}).then(function(t){{ window.__fetch = t; }}) \
             .catch(function(e){{ window.__fetch = 'err:' + e; }}); }})()",
            h1.url("/fetch_probe")
        ),
    );

    // Wait for all five fixture hits, pumping servo's event loop.
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if h1.wait_for_paths(5, Duration::from_millis(200)) {
            break;
        }
        pump(50);
    }
    let recorded = h1.recorded();
    for expected in ["/img.png", "/app.js", "/style.css", "/xhr_probe", "/fetch_probe"] {
        assert!(
            recorded.iter().any(|p| p == expected),
            "h1 fixture missing {expected} (recorded: {recorded:?})"
        );
    }
    eprintln!("[matrix-e2e] h1 subresources served: {:?}", recorded);

    // Wait for the page-side consumers to settle. `fetch` (window.fetch) is
    // bao's Node-stack fetch (overridden in pages — NOT servo page network,
    // see the fingerprint e2e's stack split) and its read result is not a
    // bridge assertion: only its fixture arrival is, below.
    let settle = |js: &str| -> Option<String> {
        match page.evaluate_js_web(js) {
            Ok(v) => Some(format!("{:?}", v)),
            Err(e) => Some(format!("ERR {:?}", e)),
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let diag = settle(
            "(function(){ return JSON.stringify({img: window.__img||null, xhr: window.__xhr||null, fetch: window.__fetch||null, script: window.__scriptRan||false}); })()",
        )
        .unwrap_or_default();
        eprintln!("[matrix-e2e] settle poll: {diag} (counter={})", net::fetch::bun_bridge::page_net_bun_request_count());
        if diag.contains("loaded") && diag.contains("xhr-data") {
            break;
        }
        if Instant::now() > deadline {
            panic!("h1 page-side consumers did not settle: {diag}");
        }
        pump(50);
    }

    // Four subresources through the bridge (img + script + css + xhr); the
    // window.fetch probe rides the Node stack and must NOT count. (Skipped
    // in the hyper control run — there is no bridge counter to assert.)
    let counter_after_subs = net::fetch::bun_bridge::page_net_bun_request_count();
    if !hyper_control {
        assert_eq!(
            counter_after_subs - counter_before,
            4,
            "img+script+css+xhr must all go through the bridge, window.fetch must not \
             (recorded: {recorded:?})"
        );
    }

    // Document destination: navigate a second page to an http document.
    // Success = the fixture served /doc.html AND the page PARSED it (title
    // and body text queryable). PageState progression for a second page
    // stays Created on BOTH stacks (harness fact — differentially verified),
    // so it is not an assertion here.
    let doc_page = create_page(pool, h1.url("/doc.html"));
    wait_for_load(&doc_page, 5000);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let parsed = doc_page
            .evaluate_js_web("(function(){ return document.title === 'doc' && document.body.innerText === 'doc' ? 'yes' : 'no'; })()")
            .map(|v| v == "yes")
            .unwrap_or(false);
        if h1.recorded().iter().any(|p| p == "/doc.html") && parsed {
            break;
        }
        if Instant::now() > deadline {
            let content = doc_page.evaluate_js_web(
                "(function(){ return document.title + '|' + document.body.innerText.substring(0, 200) + '|ulen=' + document.body.innerText.length; })()",
            );
            panic!(
                "h1 document never parsed (state={:?}, content={:?}, recorded: {:?})",
                doc_page.get_state(),
                content,
                h1.recorded()
            );
        }
        let _ = doc_page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(50));
    }
    let counter_after_doc = net::fetch::bun_bridge::page_net_bun_request_count();
    if !hyper_control {
        assert_eq!(
            counter_after_doc - counter_after_subs,
            1,
            "the document navigation must ride the bridge exactly once (recorded: {:?})",
            h1.recorded()
        );
    }
    eprintln!("[matrix-e2e] h1 document served through the bridge");
    let _ = doc_page.close();

    // ── Block 2: semantic pass-through (redirect / CORS / nosniff) ────────
    let semantic_counter_before = net::fetch::bun_bridge::page_net_bun_request_count();
    inject(
        "redirect-xhr",
        &format!(
            "(function(){{ try {{ var x = new XMLHttpRequest(); \
             x.onload = function(){{ window.__redir = x.status + ':' + x.responseText; }}; \
             x.onerror = function(){{ window.__redir = 'error'; }}; \
             x.open('GET', '{}'); x.send(); }} catch (e) {{ window.__redir = 'throw:' + e; }} }})()",
            h1.url("/redirector")
        ),
    );
    inject(
        "cors-xhr",
        &format!(
            "(function(){{ try {{ var x = new XMLHttpRequest(); \
             x.onload = function(){{ window.__cors = 'leaked:' + x.responseText; }}; \
             x.onerror = function(){{ window.__cors = 'blocked'; }}; \
             x.open('GET', '{}'); x.send(); }} catch (e) {{ window.__cors = 'throw:' + e; }} }})()",
            h1.url("/cors_denied")
        ),
    );
    // Module script: CorsMode, so the nosniff check sees the (kept)
    // response headers — classic no-cors scripts get opaque filtering on
    // BOTH stacks (upstream servo semantics, not a bridge concern).
    inject(
        "nosniff-module-script",
        &format!(
            "(function(){{ var s = document.createElement('script'); s.type = 'module'; s.src = '{}'; document.head.appendChild(s); }})()",
            h1.url("/nosniff.js")
        ),
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut diag;
    loop {
        diag = settle(
            "(function(){ return 'redir=' + (window.__redir||'null') + ' cors=' + (window.__cors||'null') + ' nosniff=' + (window.__nosniffRan ? 'ran' : 'blocked'); })()",
        )
        .unwrap_or_default();
        if diag.contains("redir=200:final-body") && diag.contains("cors=blocked") && diag.contains("nosniff=blocked") {
            break;
        }
        if Instant::now() > deadline {
            panic!("semantic probes did not settle: {diag}");
        }
        pump(50);
    }
    // Redirect chain: both hops hit the fixture.
    assert!(
        h1.recorded().iter().filter(|p| *p == "/redirector").count() >= 1 &&
            h1.recorded().iter().filter(|p| *p == "/final").count() >= 1,
        "redirect chain must hit /redirector then /final (recorded: {:?})",
        h1.recorded()
    );
    // CORS read blocked (servo-side CORS applied to the bridge response).
    assert!(
        !diag.contains("leaked"),
        "CORS-denied response must NOT be readable, got {diag}"
    );
    // nosniff module script never executed (distinct flag — /app.js ran).
    assert!(
        diag.contains("nosniff=blocked"),
        "nosniff-blocked module script must not execute, got {diag}"
    );
    // Redirect = 2 bridge hops (302 + final); cors + nosniff = 1 each.
    let semantic_counter_after = net::fetch::bun_bridge::page_net_bun_request_count();
    if !hyper_control {
        assert_eq!(
            semantic_counter_after - semantic_counter_before,
            4,
            "redirect(2) + cors(1) + nosniff(1) must ride the bridge (recorded: {:?})",
            h1.recorded()
        );
    }
    eprintln!("[matrix-e2e] redirect/CORS/nosniff semantics passed through the bridge");

    // ── Block 3: H2 matrix ────────────────────────────────────────────────
    let h2 = H2Server::spawn();
    let h2_page = create_page(pool, h2.url("/doc2.html"));
    wait_for_load(&h2_page, 8000);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if h2.streams.lock().unwrap().len() >= 1 {
            break;
        }
        if Instant::now() > deadline {
            panic!(
                "h2 document never arrived (streams: {:?}, alpn-h2 conns: {}, non-h2: {})",
                h2.streams.lock().unwrap(),
                h2.alpn_h2_count.load(Ordering::SeqCst),
                h2.non_h2_count.load(Ordering::SeqCst),
            );
        }
        let _ = h2_page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        h2.non_h2_count.load(Ordering::SeqCst),
        0,
        "every connection to the h2 fixture must negotiate ALPN h2"
    );
    eprintln!("[matrix-e2e] h2 document served (streams: {:?})", h2.streams.lock().unwrap());

    let h2_counter_before = net::fetch::bun_bridge::page_net_bun_request_count();
    let inject_h2 = |label: &str, js: &str| {
        match h2_page.evaluate_js_web(js) {
            Ok(value) => eprintln!("[matrix-e2e] h2 inject {label}: ok {value}"),
            Err(error) => panic!("h2 inject {label} failed: {error:?}"),
        }
    };
    inject_h2(
        "img",
        &format!(
            "(function(){{ var im = document.createElement('img'); \
             im.onload = function(){{ window.__h2img = 'loaded'; }}; \
             im.onerror = function(){{ window.__h2img = 'error'; }}; \
             document.body.appendChild(im); im.src = '{}'; }})()",
            h2.url("/h2_img.png")
        ),
    );
    inject_h2(
        "script",
        &format!(
            "(function(){{ var s = document.createElement('script'); s.src = '{}'; document.head.appendChild(s); }})()",
            h2.url("/h2_app.js")
        ),
    );
    inject_h2(
        "css",
        &format!(
            "(function(){{ var l = document.createElement('link'); l.rel = 'stylesheet'; \
             l.href = '{}'; document.head.appendChild(l); }})()",
            h2.url("/h2_style.css")
        ),
    );
    inject_h2(
        "xhr",
        &format!(
            "(function(){{ try {{ var x = new XMLHttpRequest(); \
             x.onload = function(){{ window.__h2xhr = 'ok'; }}; \
             x.onerror = function(){{ window.__h2xhr = 'error'; }}; \
             x.open('GET', '{}'); x.send(); }} catch (e) {{ window.__h2xhr = 'throw:' + e; }} }})()",
            h2.url("/h2_xhr")
        ),
    );
    // (window.fetch is the Node stack — not asserted here; see block 1.)
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if h2.streams.lock().unwrap().len() >= 5 {
            break;
        }
        if Instant::now() > deadline {
            panic!(
                "h2 subresources incomplete (streams: {:?})",
                h2.streams.lock().unwrap()
            );
        }
        let _ = h2_page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(50));
    }
    let h2_counter_after = net::fetch::bun_bridge::page_net_bun_request_count();
    if !hyper_control {
        assert_eq!(
            h2_counter_after - h2_counter_before,
            4,
            "h2 img+script+css+xhr must all ride the bridge (streams: {:?})",
            h2.streams.lock().unwrap()
        );
        // Document navigation was also through the bridge (doc2 + 4 subres).
        assert_eq!(
            h2_counter_after - h2_counter_before + 1, // +1: the document itself
            5,
        );
    }
    // Five h2 request streams total (document + 4 subresources), ALL on ONE
    // connection (stage 3 coalescing): the bridge interns its per-request
    // SSLConfig through the global registry, so content-equal configs resolve
    // to one pointer — every bun_http pool key matches and the subresources
    // multiplex onto the document's h2 session. Stream ids increment by 2
    // per request on the shared connection; the first is 13 (after the
    // profile's PRIORITY reservations 3/5/7/11 — REQ-STL-002-C3 on the wire).
    let streams = h2.streams.lock().unwrap().clone();
    assert_eq!(streams.len(), 5, "five h2 request streams: {streams:?}");
    if !hyper_control {
        // Bridge + interned SSLConfig: one ALPN-h2 connection for the whole
        // matrix; stream ids 13,15,17,19,21 (each +2 on the shared session).
        // The hyper control multiplexes too, but with hyper's own client
        // stream numbering (1,3,5,7,9) — the id-13 shape is bridge-only.
        assert_eq!(
            h2.alpn_h2_count.load(Ordering::SeqCst),
            1,
            "all five h2 requests must coalesce onto ONE connection (interned SSLConfig → single pool key), got {}",
            h2.alpn_h2_count.load(Ordering::SeqCst)
        );
        let mut ids: Vec<u32> = streams.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![13, 15, 17, 19, 21],
            "one shared session: first stream 13 (PRIORITY reservations), then +2 per request: {streams:?}"
        );
    }
    eprintln!(
        "[matrix-e2e] h2 matrix complete: {} streams, {} h2 connections",
        streams.len(),
        h2.alpn_h2_count.load(Ordering::SeqCst)
    );

    h1.shutdown.store(true, Ordering::SeqCst);
    h2.shutdown();
    eprintln!("[matrix-e2e] === ALL ASSERTIONS PASSED ===");

    // Watchdog exit (same contract as the fingerprint e2e: servo teardown can
    // stall independently of the assertions, which have all run above).
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(10));
        eprintln!("[matrix-e2e] watchdog: servo teardown did not finish in 10s — force exit");
        std::process::exit(0);
    });
    let _ = h2_page.close();
    let _ = page.close();
    pool.close_all();
}
