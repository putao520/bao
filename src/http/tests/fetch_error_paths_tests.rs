//! Wire-level error/edge-arm coverage for the h1 fetch path (REQ-ENG-010):
//! connection refusal, malformed status line, chunked encoding + trailers,
//! declared-length truncation, redirect chain + loop cap, TLS certificate
//! rejection (default `reject_unauthorized`), mid-flight abort via the
//! cross-thread shutdown message on both TCP and TLS sockets, and 1xx
//! interim responses.
//!
//! Same real-path discipline as `connection_close_guard_tests`: every case
//! drives the real `HTTPThread` socket/data path end to end against a
//! scripted fixture server. No JS runtime involved.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsServer, generate_self_signed_pem};
use bun_core::MutableString;
use bun_http::signals::Store;
use bun_http::{AsyncHTTP, FetchRedirect, HTTPClientResult, HTTPClientResultCallback, Method,
               async_http};
use common::ServerTlsIo;

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
}

/// The `HTTPClientResultCallback`. Runs on the HTTP thread from
/// `progress_update` delivery.
fn recorder_callback(
    this: *mut Recorder,
    async_http: *mut AsyncHTTP<'static>,
    result: HTTPClientResult<'_>,
) {
    let rec: &Recorder = unsafe { &*this };
    let bytes = result
        .body
        .as_deref()
        .map(|b| b.list.as_slice().to_vec())
        .unwrap_or_default();
    let delivery = Delivery {
        bytes,
        has_more: result.has_more,
        fail: result.fail.clone(),
        status: result.metadata.as_ref().map(|m| m.response.status_code),
        redirected: result.redirected,
    };

    if std::env::var("BAO_TEST_TRACE").is_ok() {
        eprintln!(
            "[delivery] has_more={} fail={:?} status={:?} redirected={} bytes={:?}",
            delivery.has_more,
            delivery.fail,
            delivery.status,
            delivery.redirected,
            String::from_utf8_lossy(&delivery.bytes)
        );
    }

    let _ = rec.tx.send(delivery);

    if !result.has_more {
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

/// Drive one GET through the real HTTPThread. Returns `(async_http_id,
/// deliveries)` — the id is sampled before the request starts so abort
/// tests can address it mid-flight. `configure` installs suite-specific
/// options on top of the wired signal store.
fn run_request_with(
    url: String,
    configure: impl FnOnce(&mut async_http::Options),
) -> (u32, Vec<Delivery>) {
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
    let recorder = Box::into_raw(Box::new(Recorder { tx }));

    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);

    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    // Full signal store (aborted slot wired) so the request gets a real
    // async_http_id and the abort tracker entry cross-thread schedules
    // resolve.
    options.signals = Some(store.to());
    configure(&mut options);

    let ah = AsyncHTTP::init(
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
    let id = ah.async_http_id;

    let ah_ptr = bun_core::heap::into_raw(Box::new(ah));
    let batch = bun_threading::thread_pool::Batch::from(unsafe {
        core::ptr::addr_of_mut!((*ah_ptr).task)
    });
    bun_http::HTTPThread::schedule(batch);

    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
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
    (id, out)
}

fn run_request(url: String) -> Vec<Delivery> {
    run_request_with(url, |_| {}).1
}

// ─── Plain HTTP/1.1 edge-behavior server ───────────────────────────────────

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

/// One accept-loop server; every connection serves sequential requests
/// (keep-alive) until the client goes away. Path → scripted answer:
///
/// - `/ok` — plain 200, Content-Length body, honest keep-alive
/// - `/chunked` — two chunks + a trailer (`X-Check`) + terminal chunk
/// - `/short-close` — `Content-Length: 100` but only 10 bytes, then a TCP
///   half-close: the declared-length truncation error arm
/// - `/badstatus` — malformed status line (`BADSTATUS` is not a code)
/// - `/hop1`, `/hop2` — 302 chain onto `/end`
/// - `/loop` — 302 onto itself forever (redirect budget exhaustion)
/// - `/1xx` — a 103 Early Hints interim block, then the final 200
/// - `/slow` — 20 drip-fed chunks (abort-test window)
fn spawn_edge_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
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
                    "/ok" => b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
                    "/chunked" => b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                                    6\r\nhello \r\n\
                                    5\r\nworld\r\n\
                                    0\r\nX-Check: 1\r\n\r\n"
                        .to_vec(),
                    // Declared 100 bytes, ship 10, half-close: the client must
                    // judge the message truncated, never complete.
                    "/short-close" => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n0123456789",
                        );
                        let _ = stream.flush();
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                        return;
                    }
                    "/badstatus" => b"HTTP/1.1 BADSTATUS nosuch\r\n\r\n".to_vec(),
                    "/hop1" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{host}/hop2\r\nContent-Length: 0\r\n\r\n"
                    )
                    .into_bytes(),
                    "/hop2" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{host}/end\r\nContent-Length: 0\r\n\r\n"
                    )
                    .into_bytes(),
                    "/end" => b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nchain-end".to_vec(),
                    "/loop" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{host}/loop\r\nContent-Length: 0\r\n\r\n"
                    )
                    .into_bytes(),
                    "/1xx" => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 103 Early Hints\r\nLink: </style.css>\r\n\r\n",
                        );
                        let _ = stream.flush();
                        std::thread::sleep(Duration::from_millis(50));
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nfinal".to_vec()
                    }
                    "/slow" => {
                        // 20 chunks, 100ms apart: keeps the request mid-body
                        // for ~2s so the abort test can land its shutdown in
                        // the middle.
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
                        );
                        let _ = stream.flush();
                        for i in 0..20u32 {
                            std::thread::sleep(Duration::from_millis(100));
                            let chunk = format!("{i:08}");
                            let msg = format!("{:x}\r\n{}\r\n", chunk.len(), chunk);
                            if stream.write_all(msg.as_bytes()).is_err() {
                                return;
                            }
                            let _ = stream.flush();
                        }
                        let _ = stream.write_all(b"0\r\n\r\n");
                        let _ = stream.flush();
                        return;
                    }
                    _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
                };
                if stream.write_all(&answer).is_err() {
                    return;
                }
                let _ = stream.flush();
            });
        }
    });
    port
}

fn terminal_of(mut deliveries: Vec<Delivery>, label: &str) -> Delivery {
    let term = deliveries
        .pop()
        .unwrap_or_else(|| panic!("{label}: no delivery arrived within 20s"));
    assert!(
        !term.has_more,
        "{label}: last delivery is not terminal: {deliveries:?}"
    );
    term
}

// ─── Tests: plain HTTP error/edge arms ─────────────────────────────────────

/// Connecting to a port with no listener fails the delivery — the
/// `on_connect_error` arm surfaces as a terminal `fail`, never a hang and
/// never a fake success.
#[test]
fn connection_refused_fails_the_delivery() {
    // Bind-and-drop: a port that is deterministically not listening — the
    // helper's probe drops before each attempt. E26 ledger class B
    // (probe-release rebind window, inverted carrier): a parallel suite
    // process claiming the port between the drop and the connect turns the
    // expected refusal into a successful delivery — such an attempt retries
    // on a FRESH dropped port; the failed delivery is the winning arm.
    common::port_probe::retry_probed_port(|port| {
        let deliveries = run_request(format!("http://127.0.0.1:{port}/"));
        let term = terminal_of(deliveries, "connection-refused");
        if term.fail.is_none() {
            // Port claimed between drop and connect — fresh dropped port, retry.
            return None;
        }
        assert!(term.fail.is_some(), "refused connect must fail: {term:?}");
        assert_eq!(term.status, None, "no response status can exist");
        Some(())
    });
}

/// A status line that is not `HTTP/1.x <code>` cannot be parsed into a
/// response — the request fails at the header boundary.
#[test]
fn malformed_status_line_fails() {
    let port = spawn_edge_server();
    std::thread::sleep(Duration::from_millis(50));
    let deliveries = run_request(format!("http://127.0.0.1:{port}/badstatus"));
    let term = terminal_of(deliveries, "badstatus");
    assert!(term.fail.is_some(), "malformed status must fail: {term:?}");
    assert_eq!(term.status, None, "no parsable status code exists");
}

/// Chunked framing with a trailer: the chunk payloads concatenate to the
/// body, the trailer never leaks into it, and the terminal chunk closes the
/// delivery cleanly.
#[test]
fn chunked_encoding_with_trailers_delivers_body() {
    let port = spawn_edge_server();
    std::thread::sleep(Duration::from_millis(50));
    let deliveries = run_request(format!("http://127.0.0.1:{port}/chunked"));
    let term = terminal_of(deliveries, "chunked");
    assert!(term.fail.is_none(), "chunked request must succeed: {term:?}");
    assert_eq!(term.status, Some(200));
    let body = String::from_utf8_lossy(&term.bytes).to_string();
    assert_eq!(body, "hello world", "chunk payloads must concatenate");
    assert!(!body.contains("X-Check"), "trailer must not leak into body");
}

/// `Content-Length: 100` with only 10 bytes shipped and a half-close is a
/// truncation: the terminal delivery must fail, not deliver a short body as
/// success.
#[test]
fn short_body_against_declared_length_fails() {
    let port = spawn_edge_server();
    std::thread::sleep(Duration::from_millis(50));
    let deliveries = run_request(format!("http://127.0.0.1:{port}/short-close"));
    let term = terminal_of(deliveries, "short-close");
    assert!(
        term.fail.is_some(),
        "truncated body must fail, got {term:?}"
    );
}

/// A 302 chain (hop1 → hop2 → end) follows both hops: final delivery is the
/// chain end's 200 with `redirected=true`.
#[test]
fn redirect_chain_follows_two_hops() {
    let port = spawn_edge_server();
    std::thread::sleep(Duration::from_millis(50));
    let deliveries = run_request(format!("http://127.0.0.1:{port}/hop1"));
    let term = terminal_of(deliveries, "redirect-chain");
    assert!(term.fail.is_none(), "chain must complete: {term:?}");
    assert_eq!(term.status, Some(200));
    assert_eq!(String::from_utf8_lossy(&term.bytes), "chain-end");
    assert!(term.redirected, "final delivery must be marked redirected");
}

/// A 302 loop exhausts the redirect budget and fails with TooManyRedirects
/// — never an infinite loop (the budget is explicit: `max_redirects=3`).
#[test]
fn redirect_loop_fails_at_budget() {
    let port = spawn_edge_server();
    std::thread::sleep(Duration::from_millis(50));
    let (_id, deliveries) = run_request_with(format!("http://127.0.0.1:{port}/loop"), |o| {
        o.max_redirects = Some(3);
    });
    let term = terminal_of(deliveries, "redirect-loop");
    assert!(term.fail.is_some(), "loop must exhaust the budget: {term:?}");
    assert_eq!(
        term.fail.as_ref().map(|e| e.name()),
        Some("TooManyRedirects"),
        "loop must fail with the TooManyRedirects error name"
    );
}

/// A 103 Early Hints interim block is consumed as informational headers and
/// the real 200 delivers afterwards — interim responses never surface as
/// the final status nor corrupt the body.
#[test]
fn early_hints_interim_then_final_200() {
    let port = spawn_edge_server();
    std::thread::sleep(Duration::from_millis(50));
    let deliveries = run_request(format!("http://127.0.0.1:{port}/1xx"));
    let term = terminal_of(deliveries, "1xx");
    assert!(term.fail.is_none(), "1xx-then-200 must succeed: {term:?}");
    assert_eq!(term.status, Some(200), "final status must be the 200");
    assert_eq!(String::from_utf8_lossy(&term.bytes), "final");
}

// ─── Tests: TLS surfaces ───────────────────────────────────────────────────

/// One-shot HTTPS server with a self-signed cert. `slow` picks the answer:
/// a fixed 200, or a slow chunked body (abort window). Serves connections
/// sequentially on one thread (ServerTlsIo is loop-tied; no nested threads).
fn spawn_tls_server(slow: bool) -> u16 {
    let (cert, key) = generate_self_signed_pem("localhost", 365).expect("self-signed cert");
    let server = std::sync::Arc::new(TlsServer::new(&cert, &key).expect("TlsServer"));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            let Ok((mut tcp, _)) = listener.accept() else {
                break;
            };
            let Ok(mut tls) = server.accept() else {
                break;
            };
            // On loopback the GET routinely piggybacks on the final handshake
            // record — it must be replayed to the reader, not dropped.
            let piggybacked = match ServerTlsIo::handshake(&mut tcp, &mut tls) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let mut io = ServerTlsIo::new(tcp, tls, piggybacked);
            let _ = read_request_head(&mut io);
            if slow {
                let _ = io.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
                let _ = io.flush();
                for i in 0..20u32 {
                    std::thread::sleep(Duration::from_millis(100));
                    let chunk = format!("{i:08}");
                    let msg = format!("{:x}\r\n{}\r\n", chunk.len(), chunk);
                    if io.write_all(msg.as_bytes()).is_err() {
                        break;
                    }
                    let _ = io.flush();
                }
                let _ = io.write_all(b"0\r\n\r\n");
                let _ = io.flush();
            } else {
                let _ = io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
                let _ = io.flush();
                let _ = io.tcp.shutdown(std::net::Shutdown::Write);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    port
}

/// Default `reject_unauthorized=true` rejects the self-signed peer: the
/// identity check fails the request at the TLS boundary. The companion
/// suites prove the accept path (`reject_unauthorized=false`); this locks
/// the reject arm.
#[test]
fn self_signed_cert_rejected_by_default() {
    let port = spawn_tls_server(false);
    std::thread::sleep(Duration::from_millis(50));
    // Options stay default: signals wired, reject_unauthorized untouched
    // (defaults to true per Flags::default).
    let deliveries = run_request(format!("https://127.0.0.1:{port}/"));
    let term = terminal_of(deliveries, "cert-reject");
    assert!(term.fail.is_some(), "self-signed peer must fail: {term:?}");
}

/// Cross-thread shutdown mid-response aborts an in-flight h1-over-TLS
/// request: the tracker resolves the id to its TLS socket and the terminal
/// delivery carries the Aborted error (drain_queued_shutdowns SocketTls
/// arm).
#[test]
fn shutdown_message_aborts_inflight_tls_request() {
    let port = spawn_tls_server(true);
    std::thread::sleep(Duration::from_millis(50));
    let (id, rx) = spawn_midflight_fetch(format!("https://127.0.0.1:{port}/"), |o| {
        // Accept the self-signed peer; the surface under test is the abort,
        // not the identity check.
        o.reject_unauthorized = Some(false);
    });

    // First delivery proves the request is mid-flight on the TLS socket.
    let first = wait_first(&rx);
    assert!(first.is_some(), "no streaming delivery — fetch hung");

    bun_http::HTTPThread::schedule_shutdown_from_any_thread(id);

    let term = wait_terminal(&rx, "tls-abort");
    assert_eq!(
        term.fail.as_ref().map(|e| e.name()),
        Some("Aborted"),
        "TLS abort must surface as Aborted: {term:?}"
    );
}

// ─── Tests: cross-thread abort (plain TCP) ─────────────────────────────────

/// Spawn a streaming GET and return `(id, receiver)` without waiting for
/// completion — the abort tests address the request mid-flight by id.
/// `configure` installs transport options (e.g. accept self-signed for the
/// TLS variant).
fn spawn_midflight_fetch(
    url: String,
    configure: impl FnOnce(&mut async_http::Options),
) -> (u32, mpsc::Receiver<Delivery>) {
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    let (tx, rx) = mpsc::channel();
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let recorder = Box::into_raw(Box::new(Recorder { tx }));

    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);
    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    options.signals = Some(store.to());
    configure(&mut options);

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
    ah.enable_response_body_streaming();
    let id = ah.async_http_id;

    let ah_ptr = bun_core::heap::into_raw(Box::new(ah));
    let batch = bun_threading::thread_pool::Batch::from(unsafe {
        core::ptr::addr_of_mut!((*ah_ptr).task)
    });
    bun_http::HTTPThread::schedule(batch);
    (id, rx)
}

/// Wait for the first delivery (bounded).
fn wait_first(rx: &mpsc::Receiver<Delivery>) -> Option<Delivery> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(d) => return Some(d),
            Err(mpsc::RecvTimeoutError::Timeout) => return None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
    None
}

/// Collect until the terminal delivery or a 15s deadline; panics on
/// deadline — an aborted request must settle, never hang.
fn wait_terminal(rx: &mpsc::Receiver<Delivery>, label: &str) -> Delivery {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            panic!("{label}: aborted request never settled within 15s");
        };
        match rx.recv_timeout(remaining) {
            Ok(d) if !d.has_more => return d,
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("{label}: aborted request never settled within 15s")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("{label}: recorder channel dropped before terminal delivery")
            }
        }
    }
}

/// Cross-thread shutdown mid-response aborts an in-flight plain-TCP request:
/// the tracker resolves the id to its TCP socket and the terminal delivery
/// carries the Aborted error (drain_queued_shutdowns SocketTcp arm).
#[test]
fn shutdown_message_aborts_inflight_tcp_request() {
    let port = spawn_edge_server();
    std::thread::sleep(Duration::from_millis(50));
    let (id, rx) = spawn_midflight_fetch(format!("http://127.0.0.1:{port}/slow"), |_| {});

    let first = wait_first(&rx);
    assert!(first.is_some(), "no streaming delivery — fetch hung");

    bun_http::HTTPThread::schedule_shutdown_from_any_thread(id);

    let term = wait_terminal(&rx, "tcp-abort");
    assert_eq!(
        term.fail.as_ref().map(|e| e.name()),
        Some("Aborted"),
        "TCP abort must surface as Aborted: {term:?}"
    );
}

/// Zero ids are the "no id" sentinel: every cross-thread schedule entry must
/// no-op (guard-first early return) instead of touching the HTTP thread
/// state. Runs on a fully initialized thread (the fetch above it), so the
/// guard is the exercised branch, not a pre-init accident.
#[test]
fn zero_id_cross_thread_schedules_are_noops() {
    // Initialize the HTTP thread for real (same bring-up as every fetch).
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    // No assertion of observable effect — the contract is "returns without
    // panicking and without requiring a socket"; a panic or the init-assert
    // firing would fail the test.
    bun_http::HTTPThread::schedule_response_body_drain_from_any_thread(0);
    bun_http::HTTPThread::schedule_shutdown_from_any_thread(0);
    bun_http::HTTPThread::schedule_request_write_from_any_thread(
        0,
        bun_http::http_thread::WriteMessageType::End,
    );
    bun_http::HTTPThread::schedule_transport_pause_from_any_thread(
        0,
        bun_http::http_thread::TransportPauseKind::Pause,
    );
}

/// The h3 DNS-resolution handoff queue drains safely when empty (the
/// per-`drain_events` tick calls this unconditionally): take an empty batch
/// and iterate nothing. The populated path requires a live QUIC engine +
/// registered addrinfo (FFI-deep) and stays out of scope here; this locks
/// the always-ticking no-op arm.
#[test]
fn h3_pending_connect_drain_on_empty_queue_is_noop() {
    bun_http::h3_client::PendingConnect::drain_resolved();
}
