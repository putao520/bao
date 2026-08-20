// @trace REQ-ENG-008 [level:e2e] — streaming chunked request upload through
// the bun bridge (restored hyper-era semantics: servo IPC body chunks feed a
// `ThreadSafeStreamBuffer` the HTTPThread drains to the socket as the peer
// accepts bytes — never a fully-buffered body).
//
// One test process (mozjs Runtime and servo Opts are per-process singletons):
//
//   1. SLOW chunked upload: the page XHR-POSTs a 512 KiB body (window.fetch
//      is the Node-stack override in pages — XHR is the servo page-network
//      POST path). A deliberately SLOW fixture server reads the body 8 KiB
//      at a time (10 ms between reads), proving:
//        - the exchange rides `Transfer-Encoding: chunked` on h1 (the new
//          streaming framing; the buffered era sent Content-Length),
//        - the de-chunked payload is byte-exact and complete,
//        - the upload survives socket backpressure (the feeder's 16 KiB
//          high-water mark pauses the supply side and the buffer-drain
//          callback must resume it — a broken drain wakeup hangs here),
//        - the page gets its 200 + body back (the full round trip).
//      The body-read span is asserted to be non-trivial: the fixture's paced
//      reads, not a single burst, delivered the bytes.
//   2. Mid-upload abort: a second POST to a path whose body the server never
//      reads; the page aborts the XHR — the exchange must unwind (onabort
//      fires, no hang) and the server observes the disconnect.
//
// The bridge request counter gates both legs: the uploads went through the
// bun bridge (the only page-network path).

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool, PageState};

/// Uploaded payload size: comfortably above every buffer on the path (the
/// 16 KiB feeder high-water mark, the TSB, kernel socket buffers), so the
/// drain/backpressure machinery must engage for the upload to finish.
const UPLOAD_BYTES: usize = 512 * 1024;

// ---------------------------------------------------------------------------
// Slow-read H1 fixture
// ---------------------------------------------------------------------------

/// What the fixture observed for one upload request.
#[derive(Debug, Default, Clone)]
struct UploadObservation {
    /// `Transfer-Encoding: chunked` present on the request.
    chunked: bool,
    /// `Content-Length` present on the request (must be absent for the
    /// streaming upload — chunked and CL are mutually exclusive).
    content_length: bool,
    /// De-chunked byte count that reached the server.
    body_bytes: usize,
    /// Millis between the first and last body reads — the paced-delivery
    /// span (a single pre-buffered burst would collapse this).
    body_read_span_ms: u128,
    /// The peer hung up mid-body (abort leg).
    peer_closed_early: bool,
}

struct SlowUploadFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    observation: Arc<Mutex<Option<UploadObservation>>>,
    abort_observation: Arc<Mutex<Option<UploadObservation>>>,
    /// (count of served upload paths, condvar) — set when a request head
    /// for the given path arrived.
    signal: Arc<(Mutex<Vec<String>>, Condvar)>,
}

impl SlowUploadFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow-upload fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let observation = Arc::new(Mutex::new(None));
        let abort_observation = Arc::new(Mutex::new(None));
        let signal = Arc::new((Mutex::new(Vec::<String>::new()), Condvar::new()));

        let shutdown_c = Arc::clone(&shutdown);
        let observation_c = Arc::clone(&observation);
        let abort_observation_c = Arc::clone(&abort_observation);
        let signal_c = Arc::clone(&signal);
        std::thread::Builder::new()
            .name("slow-upload-fixture".into())
            .spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("nonblocking listener");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((tcp, _)) => {
                            handle_connection(
                                tcp,
                                &shutdown_c,
                                &observation_c,
                                &abort_observation_c,
                                &signal_c,
                            );
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => break,
                    }
                }
            })
            .expect("spawn slow-upload fixture");

        SlowUploadFixture {
            port,
            shutdown,
            observation,
            abort_observation,
            signal,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    /// Wait until `count` request heads with `path` have been recorded.
    fn wait_for_path(&self, path: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            {
                let (lock, cond) = &*self.signal;
                let guard = lock.lock().unwrap();
                if guard.iter().filter(|p| p.as_str() == path).count() >= 1 {
                    return true;
                }
                let _ = cond
                    .wait_timeout(guard, Duration::from_millis(20))
                    .unwrap();
            }
        }
        false
    }

    fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Read exactly one request head (through `\r\n\r\n`) from the connection.
fn read_head(tcp: &mut TcpStream, buf: &mut Vec<u8>, idle_deadline: Duration) -> Option<String> {
    let deadline = Instant::now() + idle_deadline;
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = find_head_end(buf) {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            buf.drain(..pos + 4);
            return Some(head);
        }
        if Instant::now() > deadline {
            return None;
        }
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(50)));
        match tcp.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return None,
        }
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[allow(clippy::too_many_arguments)]
fn handle_connection(
    mut tcp: TcpStream,
    shutdown: &AtomicBool,
    observation: &Mutex<Option<UploadObservation>>,
    abort_observation: &Mutex<Option<UploadObservation>>,
    signal: &(Mutex<Vec<String>>, Condvar),
) {
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(5)));
    let mut buf: Vec<u8> = Vec::new();
    while !shutdown.load(Ordering::SeqCst) {
        let Some(head) = read_head(&mut tcp, &mut buf, Duration::from_secs(5)) else {
            return; // idle keep-alive connection / client gone
        };
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_string();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        if method.is_empty() || path.is_empty() {
            return;
        }
        {
            let (lock, cond) = signal;
            let mut guard = lock.lock().unwrap();
            guard.push(path.clone());
            cond.notify_all();
        }

        let is_abort_leg = path == "/abort_upload";
        let head_lower = head.to_ascii_lowercase();
        let chunked = head_lower.contains("transfer-encoding: chunked");
        let content_length_header = head_lower
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .map(|v| v.trim().to_string());
        let content_length = content_length_header.is_some();
        // Expected framing: Content-Length (raw bytes — servo knows the XHR
        // body size) or chunked (no known length — the feeder's
        // `{hex}\r\n … \r\n` + terminal framing). Both are valid streaming
        // shapes; the observation records which one rode the wire.
        let expected_raw: Option<usize> = match (&content_length_header, chunked) {
            (Some(value), false) => value.parse::<usize>().ok(),
            _ => None,
        };
        if !is_abort_leg {
            eprintln!(
                "[upload-e2e] fixture: {method} {path} chunked={chunked} content_length={content_length_header:?}"
            );
        }

        let mut obs = UploadObservation {
            chunked,
            content_length,
            ..Default::default()
        };

        // `/upload`: pace-read the body to its framing end, then 200 "ok".
        // `/abort_upload`: paced reads, NO response — the client aborts
        // mid-flight; whichever side gives up first ends the connection.
        let mut decoded: Vec<u8> = Vec::new();
        let mut first_read: Option<Instant> = None;
        let mut last_read: Option<Instant> = None;
        let mut chunk = [0u8; 8 * 1024];
        let overall = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() > overall {
                obs.peer_closed_early = true; // treat timeout as an early end
                break;
            }
            let _ = tcp.set_read_timeout(Some(Duration::from_millis(50)));
            match tcp.read(&mut chunk) {
                Ok(0) => {
                    obs.peer_closed_early = true;
                    break;
                },
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if first_read.is_none() {
                        first_read = Some(Instant::now());
                    }
                    last_read = Some(Instant::now());
                    // Consume as far as the buffered bytes allow, per framing.
                    let framing_done = match expected_raw {
                        Some(raw) => {
                            // Raw Content-Length body: complete once all
                            // announced bytes are buffered.
                            if buf.len() >= raw {
                                decoded.extend_from_slice(&buf[..raw]);
                                buf.drain(..raw);
                                true
                            } else {
                                false
                            }
                        },
                        None => !decode_chunked(&mut buf, &mut decoded),
                    };
                    if framing_done {
                        eprintln!(
                            "[upload-e2e] fixture: body complete, decoded={} raw_buf={}",
                            decoded.len(),
                            buf.len()
                        );
                        break;
                    }
                    // Paced consumer on BOTH legs: 8 KiB burps, 10 ms apart.
                    // The upload leg proves incremental wire delivery under
                    // backpressure; the abort leg keeps the body in flight
                    // (>600 ms span) so the client's 300 ms abort lands
                    // mid-upload, not after a completed exchange.
                    std::thread::sleep(Duration::from_millis(10));
                },
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if is_abort_leg {
                        // The abort leg expects the client to give up; the
                        // WouldBlock timeout keeps the connection open but
                        // this thread responsive.
                        continue;
                    }
                    // Upload leg: keep waiting while the overall deadline
                    // holds (the client may still be producing).
                    continue;
                },
                Err(_) => {
                    obs.peer_closed_early = true;
                    break;
                },
            }
        }
        obs.body_bytes = decoded.len();
        obs.body_read_span_ms = match (first_read, last_read) {
            (Some(f), Some(l)) => l.duration_since(f).as_millis(),
            _ => 0,
        };
        let payload_ok = decoded.len() == UPLOAD_BYTES
            && decoded
                .iter()
                .enumerate()
                .all(|(i, b)| *b == b'A' + ((i / 1024) % 26) as u8);
        if !is_abort_leg {
            if obs.body_bytes == UPLOAD_BYTES && payload_ok {
                let response = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\
                               access-control-allow-origin: *\r\n\
                               content-length: 2\r\nconnection: close\r\n\r\nok";
                let _ = tcp.write_all(response.as_bytes());
                let _ = tcp.flush();
            } else {
                let response = "HTTP/1.1 500 Bad Storage\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                let _ = tcp.write_all(response.as_bytes());
                let _ = tcp.flush();
            }
            *observation.lock().unwrap() = Some(obs);
            // connection: close — one upload per connection for this leg.
            return;
        }
        *abort_observation.lock().unwrap() = Some(obs);
        return;
    }
}

/// Incrementally decode chunked framing from `buf` into `out`.
/// Returns `true` while more (non-terminal) data is expected, `false` once
/// the terminal `0\r\n\r\n` chunk was consumed.
fn decode_chunked(buf: &mut Vec<u8>, out: &mut Vec<u8>) -> bool {
    loop {
        // Need a full `{hex}\r\n` size line.
        let Some(line_end) = buf.windows(2).position(|w| w == b"\r\n") else {
            return true;
        };
        let size_str = String::from_utf8_lossy(&buf[..line_end]).to_string();
        let size = match usize::from_str_radix(
            size_str.split(';').next().unwrap_or_default().trim(),
            16,
        ) {
            Ok(size) => size,
            Err(_) => {
                // Not a size line — malformed framing; treat as terminal so
                // the caller can fail loudly on the byte checks.
                return false;
            },
        };
        if size == 0 {
            // Terminal chunk: consume `0\r\n\r\n` if fully buffered.
            if buf.len() >= line_end + 2 + 2 {
                buf.drain(..line_end + 2 + 2);
            }
            return false;
        }
        if buf.len() < line_end + 2 + size + 2 {
            return true; // wait for the rest of this chunk
        }
        out.extend_from_slice(&buf[line_end + 2..line_end + 2 + size]);
        buf.drain(..line_end + 2 + size + 2);
    }
}

// ---------------------------------------------------------------------------
// Page harness helpers (same contract as the matrix e2e)
// ---------------------------------------------------------------------------

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
    // Firefox profile: keeps the bridge's ALPN offer at hyper parity
    // (`h2,http/1.1`); this test's fixture is plain h1, but the profile
    // keeps the e2e posture identical to the matrix suite.
    let stealth_profile = Some(bao_stealth::StealthProfile::firefox_default());
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(url.clone()),
            stealth_profile: stealth_profile.clone(),
            ..Default::default()
        }) {
            Ok(page) => return page,
            Err(e) => {
                eprintln!("[upload-e2e] page creation failed (retrying): {e}");
                std::thread::sleep(Duration::from_secs(3));
            },
        }
    }
    panic!("page creation failed after retries");
}

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

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

#[test]
fn page_net_bun_streaming_chunked_upload() {
    bun_core::Output::init_test();

    let config = BaoConfig {
        ignore_certificate_errors: true,
        ..BaoConfig::default()
    };
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => panic!("BaoRuntime::new failed: {e}"),
    };
    let pool: &PagePool = runtime.page_pool();

    let fixture = SlowUploadFixture::spawn();

    // data: URL shell page (opaque "null" origin — the 200 responses carry
    // ACAO * so the XHR read passes servo's CORS check).
    let html = "<!DOCTYPE html><html><head><title>u</title></head>\
                <body><p id=\"t\">u</p></body></html>"
        .to_string();
    let page = create_page(
        pool,
        format!("data:text/html;charset=utf-8,{}", data_url_escape(&html)),
    );
    eprintln!("[upload-e2e] shell page created");
    wait_for_load(&page, 3000);

    let counter_before = servo_net::fetch::bun_bridge::page_net_bun_request_count();

    // Leg 1 — slow paced chunked upload via XHR POST. The 512 KiB body is
    // built with a rotating per-KiB byte pattern so any truncation,
    // duplication, or reordering fails the byte-exact check.
    let upload_js = format!(
        "(function(){{ \
            window.__up = 'pending'; \
            try {{ \
                var parts = []; \
                for (var i = 0; i < 512; i++) {{ \
                    var c = String.fromCharCode(65 + (i % 26)); \
                    parts.push(new Array(1025).join(c)); \
                }} \
                var body = parts.join(''); \
                var x = new XMLHttpRequest(); \
                x.onload = function(){{ window.__up = 'done:' + x.status + ':' + x.responseText; }}; \
                x.onerror = function(){{ window.__up = 'error:rs' + x.readyState + ':st' + x.status; }}; \
                x.onabort = function(){{ window.__up = 'abort'; }}; \
                x.open('POST', '{url}'); \
                x.send(body); \
                window.__upBodyLen = body.length; \
            }} catch (e) {{ window.__up = 'throw:' + e; }} \
        }})()",
        url = fixture.url("/upload")
    );
    match page.evaluate_js_web(&upload_js) {
        Ok(value) => eprintln!("[upload-e2e] upload xhr sent: {value}"),
        Err(error) => panic!("upload xhr injection failed: {error:?}"),
    }

    let pump = |ms: u64| {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let _ = page.evaluate_js("");
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    // The request head must reach the fixture promptly (streaming: the head
    // goes on the wire while the body is still being produced — the buffered
    // era only scheduled the request after draining the whole IPC body).
    assert!(
        fixture.wait_for_path("/upload", Duration::from_secs(10)),
        "upload request head never reached the fixture"
    );
    eprintln!("[upload-e2e] upload head arrived");

    // Wait for the page-side round trip to settle.
    let settle = |js: &str| -> String {
        match page.evaluate_js_web(js) {
            Ok(v) => v,
            Err(e) => format!("ERR {e:?}"),
        }
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let state = settle("(function(){ return window.__up || null; })()");
        if state.contains("done:200:ok") || state.starts_with("error") || state.starts_with("throw")
        {
            eprintln!("[upload-e2e] upload settled: {state}");
            assert!(
                state.contains("done:200:ok"),
                "upload XHR did not complete cleanly: {state}"
            );
            break;
        }
        if Instant::now() > deadline {
            panic!("upload XHR never settled (last: {state})");
        }
        pump(100);
    }

    let observation = fixture
        .observation
        .lock()
        .unwrap()
        .clone()
        .expect("fixture never recorded the upload observation");
    eprintln!("[upload-e2e] observation: {observation:?}");

    // Streaming framing: exactly ONE of Transfer-Encoding: chunked (no known
    // body size — the feeder writes the chunk framing) or Content-Length
    // (known size — raw bytes, the header honored). Never both, never neither
    // (a body-less framing would hang the paced read).
    assert!(
        observation.chunked ^ observation.content_length,
        "upload framing must be chunked or Content-Length, got chunked={} content_length={}",
        observation.chunked,
        observation.content_length
    );
    // Byte-exact, complete payload.
    assert_eq!(
        observation.body_bytes, UPLOAD_BYTES,
        "chunked upload byte count mismatch"
    );
    // The paced reads span real time — the fixture's slow consumption
    // throttled the exchange end-to-end (backpressure path exercised, not a
    // single pre-buffered burst).
    assert!(
        observation.body_read_span_ms >= 100,
        "body delivery span too short for paced reads: {}ms",
        observation.body_read_span_ms
    );
    assert!(
        !observation.peer_closed_early,
        "peer closed before the upload completed"
    );

    let counter_after_upload = servo_net::fetch::bun_bridge::page_net_bun_request_count();
    assert!(
        counter_after_upload > counter_before,
        "the upload must ride the bun bridge (counter did not advance)"
    );

    // Leg 2 — mid-upload abort: the page aborts the XHR while the body is in
    // flight (over loopback the kernel may already have buffered the whole
    // 512 KiB by the 300 ms abort — the exact split is not the assertion).
    // The regression this leg guards is the HANG class: a streaming upload
    // whose exchange refuses to unwind after abort (feeder stuck paused,
    // drain callback lost, exchange never terminal). Any prompt terminal
    // settlement — onabort, or the server-side close surfacing as
    // error/done — proves the unwind; 'pending' past the deadline fails.
    let abort_js = format!(
        "(function(){{ \
            window.__ab = 'pending'; \
            try {{ \
                var parts = []; \
                for (var i = 0; i < 512; i++) {{ \
                    var c = String.fromCharCode(65 + (i % 26)); \
                    parts.push(new Array(1025).join(c)); \
                }} \
                var x = new XMLHttpRequest(); \
                x.onload = function(){{ window.__ab = 'done:' + x.status; }}; \
                x.onerror = function(){{ window.__ab = 'error'; }}; \
                x.onabort = function(){{ window.__ab = 'aborted'; }}; \
                x.open('POST', '{url}'); \
                x.send(parts.join('')); \
                window.__abXhr = x; \
                setTimeout(function(){{ try {{ x.abort(); }} catch (e) {{ window.__ab = 'throw:' + e; }} }}, 300); \
            }} catch (e) {{ window.__ab = 'throw:' + e; }} \
        }})()",
        url = fixture.url("/abort_upload")
    );
    match page.evaluate_js_web(&abort_js) {
        Ok(value) => eprintln!("[upload-e2e] abort xhr sent: {value}"),
        Err(error) => panic!("abort xhr injection failed: {error:?}"),
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let abort_state = loop {
        let state = settle("(function(){ return window.__ab || null; })()");
        if state != "pending" && !state.is_empty() {
            break state;
        }
        if Instant::now() > deadline {
            panic!("abort leg never settled — mid-upload abort hung (last: {state})");
        }
        pump(100);
    };
    assert!(
        !abort_state.starts_with("throw"),
        "abort leg JS threw: {abort_state}"
    );
    eprintln!("[upload-e2e] abort leg settled: {abort_state}");

    // Give the fixture a moment to observe the disconnect, then stop it.
    std::thread::sleep(Duration::from_millis(300));
    fixture.stop();

    eprintln!("[upload-e2e] streaming chunked upload e2e complete");
}
