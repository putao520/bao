//! Wire-level regression lock for the vendored client's `Connection`-close
//! framing (root-cure of the 2xx-only close guard).
//!
//! Defect (reproduced 2026-08-21, buffered delivery): the response-header
//! guard honored `Connection: close` only for `status ∈ [200, 299]`.
//! Two failure shapes fell out of that gate plus `on_close`'s clean-end
//! clause requiring `content_length.is_none()`:
//!
//! 1. **2xx + null-body + close** (204; 200 with `Content-Length: 0`): the
//!    honored close drove the Finished judgment's `!allow_keepalive` clause
//!    into the wait-for-body state, and the promised EOF then fell through
//!    `on_close` (Content-Length forced to `Some(0)` by the 204/null-body
//!    rule) → `fail(ConnectionClosed)` — surfaced to JS as ECONNRESET
//!    "socket connection closed before response".
//! 2. **non-2xx + close** (304, 418, 3xx redirects): the close promise
//!    escaped the guard entirely — `allow_keepalive` stayed true, the
//!    socket was pooled against the server's explicit close (and a
//!    same-origin redirect would try to reuse the dying socket).
//!
//! Fix (RFC 9110 §9.3 — Connection directives apply to the connection, not
//! the status class): the guard honors close/keep-alive for every status,
//! and `on_close`'s close-delimited clean-end clause accepts a
//! fully-received declared length (`Some(0)` included), so EOF after an
//! explicit close is the message end, never a truncation error.
//!
//! Both fetch delivery modes are locked (buffered = the exposing path,
//! streaming = the path upstream bun masks the defect with); a redirect
//! across an honored close must reconnect cleanly. No JS runtime involved —
//! this drives the real HTTPThread socket/data path end to end.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bun_core::MutableString;
use bun_http::signals::Store;
use bun_http::{AsyncHTTP, FetchRedirect, HTTPClientResult, HTTPClientResultCallback, Method,
               async_http};

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by `bun_runtime::dispatch` in product
// binaries (bun_runtime is higher-tier than bun_http and cannot be
// dev-depped from here). No FilePoll sources are registered anywhere in
// these tests, so a no-op satisfies the link-time reference.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

// Link seam for `__bun_crash_handler_out_of_memory`: bun_alloc resolves it
// at link time against bun_crash_handler (higher-tier than this crate's
// test binary). OOM aborts the process either way — a faithful test stub.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    eprintln!("bun: out of memory");
    std::process::abort()
}

// ─── Delivery recorder ─────────────────────────────────────────────────────

#[derive(Debug)]
struct Delivery {
    bytes: Vec<u8>,
    has_more: bool,
    fail: Option<bun_core::Error>,
    status: Option<u32>,
    redirected: bool,
}

struct Recorder {
    tx: mpsc::Sender<Delivery>,
    /// Streaming mode: mimic the consumer contract (drain the delivered
    /// bytes and schedule an `HTTPThread` response-body drain per chunk).
    drain_each_chunk: bool,
}

/// The `HTTPClientResultCallback`. Runs on the HTTP thread from
/// `progress_update` delivery.
fn recorder_callback(
    this: *mut Recorder,
    async_http: *mut AsyncHTTP<'static>,
    mut result: HTTPClientResult<'_>,
) {
    let rec: &Recorder = unsafe { &*this };
    let bytes = result
        .body
        .as_deref()
        .map(|b| b.list.as_slice().to_vec())
        .unwrap_or_default();
    let status = result.metadata.as_ref().map(|m| m.response.status_code);
    let has_more = result.has_more;
    let fail = result.fail;
    let redirected = result.redirected;

    if std::env::var("BAO_TEST_TRACE").is_ok() {
        eprintln!(
            "[delivery] has_more={} fail={:?} status={:?} bytes={:?}",
            has_more,
            fail,
            status,
            String::from_utf8_lossy(&bytes)
        );
    }
    if has_more && fail.is_none() {
        // Consumer drains the shared body buffer so the next streaming
        // delivery contains only newly arrived bytes.
        if let Some(b) = result.body.as_deref_mut() {
            b.list.clear();
        }
        if rec.drain_each_chunk {
            // Same-thread schedule idiom as ProxyTunnel's
            // schedule_proxy_deref call sites: this callback runs on the
            // HTTP thread, which owns the HTTP_THREAD cell.
            let id = unsafe { (*async_http).async_http_id };
            bun_http::http_thread_mut().schedule_response_body_drain(id);
        }
    }

    let _ = rec.tx.send(Delivery {
        bytes,
        has_more,
        fail,
        status,
        redirected,
    });

    if !has_more {
        // Terminal delivery: reclaim the caller-thread `AsyncHTTP` box via
        // the `real` backref plus the response buffer — sole dropper,
        // mirroring `on_http_done` in fetch_async.rs. The HTTP-thread clone
        // is raw-deallocated by `on_async_http_callback_raw`.
        let real = unsafe { (*async_http).real };
        if let Some(r) = real {
            drop(unsafe { Box::from_raw(r.as_ptr()) });
        }
        let buf = unsafe { (*async_http).response_buffer };
        if !buf.is_null() {
            drop(unsafe { Box::from_raw(buf) });
        }
    }
}

/// Drive one GET through the real HTTPThread and collect every result
/// callback until the terminal (`has_more == false`) delivery.
fn run_request(url: String, streaming: bool) -> Vec<Delivery> {
    // Test-binary link leg: provides the C-seam symbols (StackCheck,
    // addrinfo, native archives) the product binary gets elsewhere. Its
    // .init_array ctor also force-links the native archives.
    bao_native_stubs::force_link();
    // Output sinks: the HTTP thread's on_start configures its thread-local
    // writer, which asserts the process-level streams were initialized.
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    let (tx, rx) = mpsc::channel();
    // Leaked on purpose: the Signals NonNulls point into this store for the
    // whole request lifetime; a stable heap address avoids any relocation.
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let recorder = Box::into_raw(Box::new(Recorder {
        tx,
        drain_each_chunk: streaming,
    }));

    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);

    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    // Full signal store (aborted slot wired) so the request gets a real
    // async_http_id and the abort tracker entry that
    // schedule_response_body_drain resolves.
    options.signals = Some(store.to());

    let mut ah = AsyncHTTP::init(
        Method::GET,
        parsed_url,
        Default::default(),
        b"",
        response_buffer,
        b"",
        HTTPClientResultCallback::new(recorder, recorder_callback),
        FetchRedirect::Follow,
        options,
    );
    if streaming {
        ah.enable_response_body_streaming();
    }

    let ah_ptr = bun_core::heap::into_raw(Box::new(ah));
    let batch = bun_threading::thread_pool::Batch::from(unsafe {
        core::ptr::addr_of_mut!((*ah_ptr).task)
    });
    bun_http::HTTPThread::schedule(batch);

    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let Ok(d) = rx.recv_timeout(remaining) else {
            break;
        };
        let terminal = !d.has_more;
        out.push(d);
        if terminal {
            break;
        }
    }
    out
}

// ─── Test server ───────────────────────────────────────────────────────────

fn read_request_head(stream: &mut impl Read) -> Vec<u8> {
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    while got.len() < 64 * 1024 {
        let Ok(n) = stream.read(&mut buf) else {
            return got;
        };
        if n == 0 {
            return got;
        }
        got.extend_from_slice(&buf[..n]);
        if got.windows(4).any(|w| w == b"\r\n\r\n") {
            return got;
        }
    }
    got
}

/// Honest close semantics raw-wire server: every close-marked answer is
/// followed by a TCP half-close + full close (the explicit `Connection:
/// close` promise). `/control` is the keep-alive sanity answer. One accept
/// loop; each connection may carry sequential requests (the client pools).
fn spawn_close_guard_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .ok();
            std::thread::spawn(move || loop {
                let req = read_request_head(&mut stream);
                if req.is_empty() {
                    return;
                }
                let head = String::from_utf8_lossy(&req);
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("")
                    .to_string();
                let host = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Host:").map(|h| h.trim().to_string()))
                    .unwrap_or_default();
                let answer: Vec<u8> = match path.as_str() {
                    // control: honest keep-alive, no Connection header
                    "/control" => b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec(),
                    // close answers — every status class must honor the promise
                    "/200cl0" => {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                    "/204" => b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec(),
                    "/304" => {
                        b"HTTP/1.1 304 Not Modified\r\nETag: \"x\"\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                    "/418" => {
                        b"HTTP/1.1 418 I'm a teapot\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                    // redirect whose FIRST hop carries Connection: close —
                    // the follow-up must reconnect, not reuse the dying socket
                    "/302" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{host}/control\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec(),
                };
                let keep_alive = path == "/control";
                if stream.write_all(&answer).is_err() {
                    return;
                }
                let _ = stream.flush();
                if !keep_alive {
                    // Connection: close — the server keeps its promise.
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    return;
                }
                // keep-alive: serve the next request on this connection
                // until EOF or the read timeout.
            });
        }
    });
    port
}

// ─── The matrix ────────────────────────────────────────────────────────────

/// The full probe matrix shared by both delivery modes. Delivery mode must
/// not change `Connection: close` semantics.
fn run_close_guard_matrix(streaming: bool) -> Result<(), String> {
    let port = spawn_close_guard_server();
    std::thread::sleep(Duration::from_millis(50));

    // (label, path, expect_status, expect_body, expect_redirected)
    let cases: &[(&str, &str, u32, &str, bool)] = &[
        // sanity: honest keep-alive origin, no Connection header
        ("control-200-keepalive", "/control", 200, "hello", false),
        // 2xx null-body + close — the exposing shape (was ECONNRESET)
        ("200-cl0-close", "/200cl0", 200, "", false),
        ("204-close", "/204", 204, "", false),
        // non-2xx + close — the guard-escape shape; must stay clean now
        // that the close IS honored (would break if the guard alone were
        // fixed without the on_close clean-end widening)
        ("304-close", "/304", 304, "", false),
        ("418-close", "/418", 418, "", false),
        // redirect whose first hop promised close — follow-up reconnects
        ("302-close-follow", "/302", 200, "hello", true),
    ];

    for (label, path, status, body, redirected) in cases {
        let deliveries = run_request(format!("http://127.0.0.1:{port}{path}"), streaming);
        let terminal = deliveries
            .last()
            .ok_or_else(|| format!("{label}: no delivery arrived within 10s"))?;
        if terminal.has_more {
            return Err(format!(
                "{label}: last delivery is not terminal (has_more=true), deliveries={:?}",
                deliveries
            ));
        }
        if let Some(fail) = &terminal.fail {
            return Err(format!(
                "{label}: terminal delivery failed: {:?} (expected clean {})",
                fail, status
            ));
        }
        let got_status = terminal
            .status
            .ok_or_else(|| format!("{label}: terminal delivery carries no status"))?;
        if got_status != *status {
            return Err(format!(
                "{label}: status={} expected {}",
                got_status, status
            ));
        }
        let got_body = String::from_utf8_lossy(&terminal.bytes).to_string();
        if got_body != *body {
            return Err(format!(
                "{label}: body={:?} expected {:?}",
                got_body, body
            ));
        }
        if terminal.redirected != *redirected {
            return Err(format!(
                "{label}: redirected={} expected {}",
                terminal.redirected, redirected
            ));
        }
    }
    Ok(())
}

/// Buffered delivery (the legacy single-outcome flow): the mode that
/// exposed the defect. Every close-promised answer — 2xx null-body,
/// non-2xx, redirect first hop — must resolve as a clean terminal
/// delivery, never `ConnectionClosed`.
#[test]
fn connection_close_is_clean_end_in_buffered_delivery() {
    if let Err(e) = run_close_guard_matrix(false) {
        panic!("buffered-mode Connection: close matrix failed: {e}");
    }
}

/// Streaming delivery (resolve-at-head): the same matrix must agree — the
/// mode upstream bun masks this defect with is not allowed to diverge.
#[test]
fn connection_close_is_clean_end_in_streaming_delivery() {
    if let Err(e) = run_close_guard_matrix(true) {
        panic!("streaming-mode Connection: close matrix failed: {e}");
    }
}
