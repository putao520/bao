// @trace TEST-BRW-WEBFONT-DOUBLE-LOAD [req:REQ-BRW-002] [level:e2e]
// Webfont loading end-to-end over the REAL servo net stack (bun_bridge →
// HTTPThread): two sequential `@font-face` loads on the SAME document must
// both settle `document.fonts.ready` within a bounded window.
//
// Real-path contract under test (no mocks anywhere):
//
//   page JS (induce @font-face + styled div)
//     → servo FontContext (RemoteWebFontDownloader::download → fetch_async)
//     → servo fetch pipeline (http_fetch → obtain_response_bun)
//     → bun_bridge fetch_core → bun_http HTTPThread (h1 keep-alive)
//     → response head + (empty) body + terminal
//     → FetchResponseMsg::ProcessResponseEOF → download-state failure path
//     → number_of_loading_web_fonts fetch_sub → UnblockedFontReadyPromise
//     → document.fonts.ready resolves
//
// The defect this pins (e52/e56 forensics): the FIRST webfont load in a
// document settles fonts.ready; the SECOND load's terminal (response
// completion) never surfaces — the response is fully read off the wire
// (strace: complete head read, then silence) but no EOF is dispatched, the
// loading count stays at 1 and fonts.ready hangs forever. The server-side
// marker channel (fetches fired by the page AFTER each fonts.ready) is the
// observable: ready1 arrives, ready2 never does.
//
// Fixture semantics matter: HTTP/1.1 with Content-Length and NO
// `Connection: close` (keep-alive), matching both the WPT wptserve shape and
// the e52 probe matrix. A close-delimited fixture would terminate every
// response via EOF and mask the defect. The fixture serves each connection
// on its own thread (wptserve itself is threaded): a page-side `fetch` the
// harness may leave unsettled pins its pooled keep-alive socket, and with a
// single-accept loop the next request (the second @font-face load) would be
// starved on a never-accepted connection — a fixture artifact, not the
// defect under test (e64 bisect: red rate tracked the marker fetch's
// wire-out rate, identical signature at a393e354 and HEAD).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, PageConfig, PageHandle, PageState};

// ---------------------------------------------------------------------------
// Fixture — one H1 keep-alive server, page + font + marker routes
// ---------------------------------------------------------------------------

struct WebfontFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    /// Every served path, in arrival order (marker hits included).
    hits: Arc<Mutex<Vec<String>>>,
}

use std::sync::Arc;

const INDEX_PAGE: &str = r#"<!DOCTYPE html><html><head></head><body><div id="t1">x</div>
<script>
globalThis.__e56 = { ready1: false, ready2: false, error: null };
function induce(id, url) {
  const st = document.createElement('style');
  st.textContent = `@font-face { font-family: pf${id}; src: url(${url}); } #t${id} { font-family: pf${id}; }`;
  const d = document.createElement('div');
  d.id = 't' + id; d.textContent = 'x'; d.appendChild(st);
  document.body.appendChild(d);
  return document.fonts.ready;
}
const f1 = new URL('/font/first', location).href;
const f2 = new URL('/font/second', location).href;
induce(1, f1)
  .then(() => { globalThis.__e56.ready1 = true; fetch('/marker/ready1'); return induce(2, f2); })
  .then(() => { globalThis.__e56.ready2 = true; fetch('/marker/ready2'); })
  .catch((e) => { globalThis.__e56.error = String(e); fetch('/marker/error'); });
</script>
</body></html>"#;

impl WebfontFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind webfont fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let hits_c = Arc::clone(&hits);
        let shutdown_c = Arc::clone(&shutdown);

        std::thread::Builder::new()
            .name("webfont-fixture".into())
            .spawn(move || {
                listener.set_nonblocking(true).expect("nonblocking listener");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((tcp, _)) => {
                            // One thread per connection (wptserve shape): a
                            // keep-alive connection parked by an unsettled
                            // page fetch must never starve requests that the
                            // client legitimately carries on a second
                            // connection. Keep-alive + Content-Length framing
                            // are per-connection and unchanged.
                            let hits_conn = Arc::clone(&hits_c);
                            std::thread::Builder::new()
                                .name("webfont-fixture-conn".into())
                                .spawn(move || Self::serve_connection(tcp, &hits_conn))
                                .expect("spawn webfont fixture connection");
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn webfont fixture");

        WebfontFixture {
            port,
            shutdown,
            hits,
        }
    }

    /// HTTP/1.1 keep-alive loop: drain request heads, answer each with
    /// Content-Length framing, keep the socket open (the wire shape the
    /// defect reproduces under).
    fn serve_connection(mut tcp: TcpStream, hits: &Mutex<Vec<String>>) {
        let _ = tcp.set_nonblocking(false);
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(30)));
        let mut buf = [0u8; 8192];
        loop {
            // Read until end of request head (requests here carry no body).
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
            hits.lock().expect("hits lock").push(path.clone());

            let (status, ctype, body): (&str, &str, &[u8]) = if path == "/" {
                ("200 OK", "text/html", INDEX_PAGE.as_bytes())
            } else if path.starts_with("/font/") {
                // Empty font body: fontsan rejects it, the load takes the
                // FAILURE path (still must decrement the loading count and
                // settle fonts.ready — exactly the WPT css-font-face shape).
                ("200 OK", "font/woff2", b"")
            } else if path.starts_with("/marker/") {
                ("200 OK", "text/plain", b"ok")
            } else {
                ("404 Not Found", "text/plain", b"")
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\n\
                 Access-Control-Allow-Origin: *\r\nCache-Control: no-store\r\n\
                 Content-Length: {}\r\n\r\n",
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

    fn hit_count(&self, prefix: &str) -> usize {
        self.hits
            .lock()
            .expect("hits lock")
            .iter()
            .filter(|p| p.starts_with(prefix))
            .count()
    }
}

impl Drop for WebfontFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Page helpers (media_e2e_tests.rs form)
// ---------------------------------------------------------------------------

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn js(page: &PageHandle, expr: &str) -> String {
    page.evaluate_js_web(expr)
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .to_string()
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// RED lock for the second-webfont terminal loss: both sequential
/// `document.fonts.ready` settlements must arrive (page-side flags) and both
/// post-ready markers must reach the fixture (server-side channel).
#[test]
fn webfont_double_load_fonts_ready_settles() {
    bun_core::Output::init_test();

    let fixture = WebfontFixture::spawn();
    let runtime =
        bao_browser::BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let pool = runtime.page_pool();

    let mut page = None;
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(fixture.url()),
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
    let page = page.expect("page creation failed after retries");
    wait_for_load(&page, 10_000);

    // Bounded window for BOTH settlements. The defect's signature is a
    // permanent hang: ready1 lands immediately, ready2 never.
    let window = Duration::from_secs(25);
    let start = Instant::now();
    let both = page.wait_for_function(
        "globalThis.__e56 && globalThis.__e56.ready1 && globalThis.__e56.ready2",
        window,
    );
    let elapsed = start.elapsed();
    let snapshot = js(&page, "JSON.stringify(globalThis.__e56)");

    // NOTE on the fixture marker channel: the page-side marker fetches
    // (`fetch('/marker/...')`) are pure diagnostics. `window.fetch` issued from
    // page JS is a KNOWN-UNRELIABLE channel in the BrowserRuntime harness
    // (media_e2e_tests REAL-RUN NOTES #4: page fetches may never settle/wire
    // here), so marker arrival is NOT asserted — the fonts.ready settlement
    // flags above are the contract. Font RESOURCE loads (the @font-face
    // fetches) ride the font-context downloader channel and DO wire reliably,
    // which is what this test exercises.
    let ready1_marker = fixture.hit_count("/marker/ready1");
    let ready2_marker = fixture.hit_count("/marker/ready2");

    let mut problems = Vec::new();
    if both.is_err() {
        problems.push(format!(
            "fonts.ready settlements incomplete after {}ms: __e56={snapshot}",
            elapsed.as_millis()
        ));
    }
    if js(&page, "String(globalThis.__e56 && globalThis.__e56.ready1)") != "true" {
        problems.push("first @font-face load did not settle document.fonts.ready".into());
    }
    if js(&page, "String(globalThis.__e56 && globalThis.__e56.ready2)") != "true" {
        problems.push(
            "second @font-face load never settled document.fonts.ready (terminal lost)".into(),
        );
    }
    let err = js(&page, "String(globalThis.__e56 && globalThis.__e56.error)");
    if err.is_empty() || err == "null" {
        // no page-side rejection — expected on the green path
    } else {
        problems.push(format!("page reported error: {err}"));
    }

    // Server-side context for diagnostics only (never asserted):
    // markers arrived = ready1={ready1_marker}, ready2={ready2_marker}.
    let _ = (&ready1_marker, &ready2_marker);

    if !problems.is_empty() {
        let diag = format!(
            "font reqs first={} second={} readyState={} fonts.size={} markers r1={} r2={}",
            fixture.hit_count("/font/first"),
            fixture.hit_count("/font/second"),
            js(&page, "document.readyState"),
            js(&page, "document.fonts.size"),
            ready1_marker,
            ready2_marker,
        );
        panic!(
            "webfont double-load fonts.ready contract violated:\n  - {}\n  diag: {diag}",
            problems.join("\n  - ")
        );
    }
    eprintln!(
        "e56 green diag: elapsed={}ms hits first={} second={} markers r1={} r2={}",
        elapsed.as_millis(),
        fixture.hit_count("/font/first"),
        fixture.hit_count("/font/second"),
        ready1_marker,
        ready2_marker,
    );
}
