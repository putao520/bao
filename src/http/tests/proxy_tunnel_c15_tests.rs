//! C15 coverage wave — proxy faces of the h1 fetch path (REQ-ENG-010).
//! Complements the C7 suites (which never set `Options::http_proxy`): the
//! absolute-URI proxying wire form for plain-HTTP targets, the Basic auth
//! auto-fill from proxy-URL userinfo, the CONNECT refusal arm
//! (`ProxyConnectFailed` + the proxy's reply riding `proxy_connect_response`),
//! and the CONNECT-200 tunnel that relays TLS end to end to a real origin.
//!
//! Same real-path discipline as the C7 suites: every case drives the real
//! `HTTPThread` socket path against scripted loopback fixtures. No JS runtime.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsServer, generate_self_signed_pem};
use bun_core::MutableString;
use bun_http::signals::Store;
use bun_http::{AsyncHTTP, FetchRedirect, HTTPClientResult, HTTPClientResultCallback, Method,
               async_http};
use common::ServerTlsIo;

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by `bun_runtime::dispatch` in product binaries.
// No FilePoll sources are registered anywhere in these tests, so a no-op
// satisfies the link-time reference.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

// Link seam for `__bun_crash_handler_out_of_memory`: bun_alloc resolves it
// at link time against bun_crash_handler (higher-tier than this crate's test
// binary). OOM aborts the process either way — a faithful test stub.
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
    /// The proxy's CONNECT reply status when the failure is
    /// `ProxyConnectFailed` (kept apart from `metadata` by contract).
    proxy_status: Option<u32>,
}

struct Recorder {
    tx: mpsc::Sender<Delivery>,
}

fn recorder_callback(
    this: *mut Recorder,
    async_http: *mut AsyncHTTP<'static>,
    result: HTTPClientResult<'_>,
) {
    let rec: &Recorder = unsafe { &*this };
    let delivery = Delivery {
        bytes: result
            .body
            .as_deref()
            .map(|b| b.list.as_slice().to_vec())
            .unwrap_or_default(),
        has_more: result.has_more,
        fail: result.fail.clone(),
        status: result.metadata.as_ref().map(|m| m.response.status_code),
        proxy_status: result
            .proxy_connect_response
            .as_ref()
            .map(|m| m.response.status_code),
    };

    if !result.has_more {
        // Terminal delivery reclamation, mirroring fetch_async's on_http_done.
        let real = unsafe { (*async_http).real };
        if let Some(r) = real {
            drop(unsafe { Box::from_raw(r.as_ptr()) });
        }
        let buf = unsafe { (*async_http).response_buffer };
        if !buf.is_null() {
            drop(unsafe { Box::from_raw(buf) });
        }
    }

    let _ = rec.tx.send(delivery);
}

/// Drive one GET through the real HTTPThread with suite-specific options.
fn run_request_with(
    url: String,
    configure: impl FnOnce(&mut async_http::Options),
) -> Vec<Delivery> {
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

    let ah_ptr = bun_core::heap::into_raw(Box::new(ah));
    let batch = bun_threading::thread_pool::Batch::from(unsafe {
        core::ptr::addr_of_mut!((*ah_ptr).task)
    });
    bun_http::HTTPThread::schedule(batch);

    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
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

// ─── Fixtures ──────────────────────────────────────────────────────────────

/// A proxy fixture. For plain-HTTP targets it captures the request head
/// (wire-shape oracle) and answers with a canned 200; for `CONNECT` it
/// replies with `connect_status` and — on 2xx — blind-relays raw bytes to
/// `origin_port` (the TLS origin answers through the tunnel).
struct ProxyCapture {
    head: Mutex<Vec<u8>>,
}

fn spawn_proxy(origin_port: u16, connect_status: u16) -> (u16, Arc<ProxyCapture>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("proxy bind");
    let port = listener.local_addr().unwrap().port();
    let capture = Arc::new(ProxyCapture {
        head: Mutex::new(Vec::new()),
    });
    let capture_clone = Arc::clone(&capture);
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
            let head = read_request_head(&mut stream);
            *capture_clone.head.lock().unwrap() = head.clone();
            let first_line = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_string();
            if first_line.starts_with("CONNECT") {
                if (200..300).contains(&connect_status) {
                    let _ = stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
                    let _ = stream.flush();
                    relay_to_origin(stream, origin_port);
                } else {
                    let _ = stream.write_all(
                        format!("HTTP/1.1 {connect_status} Forbidden\r\nContent-Length: 6\r\n\r\ndenied")
                            .as_bytes(),
                    );
                    let _ = stream.flush();
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                }
            } else {
                // Plain-HTTP target: the proxy answers on the target's behalf
                // (the wire shape the client sent IS the contract under test).
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\nproxied-body");
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
        }
    });
    (port, capture)
}

/// Blind byte relay client↔origin (the CONNECT tunnel's proxy half).
fn relay_to_origin(client: TcpStream, origin_port: u16) {
    let Ok(origin) = TcpStream::connect(("127.0.0.1", origin_port)) else {
        return;
    };
    origin.set_read_timeout(Some(Duration::from_secs(60))).ok();
    origin.set_write_timeout(Some(Duration::from_secs(60))).ok();
    let client_read = match client.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let origin_read = match origin.try_clone() {
        Ok(o) => o,
        Err(_) => return,
    };
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut &client_read, &mut &origin);
        let _ = origin.shutdown(std::net::Shutdown::Write);
    });
    let down = std::thread::spawn(move || {
        let _ = std::io::copy(&mut &origin_read, &mut &client);
        let _ = client.shutdown(std::net::Shutdown::Write);
    });
    let _ = up.join();
    let _ = down.join();
}

/// Loopback plain-HTTP origin (the proxy's upstream for the absolute-URI
/// arm's canned answer is self-served, so this origin only needs to exist
/// for URL construction — the client must never reach it directly).
fn spawn_plain_origin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("origin bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            // Drain the head; if a request lands here the client bypassed the
            // proxy — the absolute-URI assertion on the capture catches that.
            let _ = read_request_head(&mut stream);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\norigin-body");
            let _ = stream.flush();
        }
    });
    port
}

/// One-shot TLS origin, C7 `spawn_tls_server` recipe (ServerTlsIo is
/// loop-tied; served sequentially on its own thread).
fn spawn_tls_origin() -> u16 {
    let (cert, key) = generate_self_signed_pem("localhost", 365).expect("self-signed cert");
    let server = std::sync::Arc::new(TlsServer::new(&cert, &key).expect("TlsServer"));
    let listener = TcpListener::bind("127.0.0.1:0").expect("tls origin bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            let Ok((mut tcp, _)) = listener.accept() else {
                break;
            };
            tcp.set_read_timeout(Some(Duration::from_secs(30))).ok();
            tcp.set_write_timeout(Some(Duration::from_secs(30))).ok();
            let Ok(mut tls) = server.accept() else {
                break;
            };
            let piggybacked = match ServerTlsIo::handshake(&mut tcp, &mut tls) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let mut io = ServerTlsIo::new(tcp, tls, piggybacked);
            let _ = read_request_head(&mut io);
            let _ = io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\ntunneled");
            let _ = io.flush();
        }
    });
    port
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|l| {
            l.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix(':'))
                .map(|v| v.trim())
        })
}

// ─── Tests ─────────────────────────────────────────────────────────────────

/// Plain-HTTP target behind a proxy: the client must dial the PROXY and send
/// the absolute-URI request form with `Host` naming the TARGET and
/// `Proxy-Authorization: Basic …` auto-filled from the proxy URL's userinfo
/// (RFC 9110 §7.2 absolute form; WHATWG fetch proxy semantics).
#[test]
fn http_target_via_proxy_sends_absolute_uri_and_basic_auth_c15() {
    let origin_port = spawn_plain_origin();
    let (proxy_port, capture) = spawn_proxy(origin_port, 403);

    let proxy_url: &'static [u8] = Box::leak(
        format!("http://user:pass@127.0.0.1:{proxy_port}")
            .into_bytes()
            .into_boxed_slice(),
    );

    let deliveries = run_request_with(
        format!("http://127.0.0.1:{origin_port}/"),
        |options| {
            options.http_proxy = Some(bun_url::URL::parse(proxy_url));
        },
    );

    let terminal = deliveries
        .iter()
        .find(|d| !d.has_more)
        .expect("proxied request must settle");
    assert_eq!(
        terminal.fail, None,
        "proxied request must succeed: {terminal:?}"
    );
    assert_eq!(terminal.status, Some(200));
    assert_eq!(terminal.bytes, b"proxied-body", "the proxy's answer is delivered");

    // Wire-shape oracle: what the client actually put on the wire to the proxy.
    let head = String::from_utf8(capture.head.lock().unwrap().clone()).expect("utf8 head");
    let first = head.lines().next().unwrap_or("");
    assert!(
        first.starts_with(&format!("GET http://127.0.0.1:{origin_port}/ HTTP/1.1")),
        "plain-HTTP targets must use the absolute-URI request form, got: {first:?}"
    );
    assert_eq!(
        header_value(&head, "Host"),
        Some(&format!("127.0.0.1:{origin_port}")[..]),
        "Host must name the target origin, not the proxy"
    );
    assert_eq!(
        header_value(&head, "Proxy-Authorization"),
        Some("Basic dXNlcjpwYXNz"),
        "auto Basic auth from proxy-URL userinfo (user:pass)"
    );
}

/// CONNECT refusal: the proxy's non-2xx reply must surface as
/// `ProxyConnectFailed` with the proxy's reply head carried on
/// `proxy_connect_response` (status 403), never as metadata or a hang.
#[test]
fn connect_refusal_yields_proxy_connect_failed_with_reply_c15() {
    let (origin_port, _guard) = {
        // The tunnel is refused before any origin traffic; the origin port is
        // only needed to form the target URL.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        (l.local_addr().unwrap().port(), l)
    };
    let (proxy_port, _capture) = spawn_proxy(origin_port, 403);
    let proxy_url: &'static [u8] = Box::leak(
        format!("http://127.0.0.1:{proxy_port}").into_bytes().into_boxed_slice(),
    );

    let deliveries = run_request_with(
        format!("https://127.0.0.1:{origin_port}/"),
        |options| {
            options.http_proxy = Some(bun_url::URL::parse(proxy_url));
        },
    );

    let terminal = deliveries
        .iter()
        .find(|d| !d.has_more)
        .expect("refused CONNECT must settle, not hang");
    let fail_name = terminal
        .fail
        .as_ref()
        .map(|e| e.name().to_string())
        .unwrap_or_default();
    assert_eq!(
        fail_name, "ProxyConnectFailed",
        "non-2xx CONNECT must fail with ProxyConnectFailed: {terminal:?}"
    );
    assert_eq!(
        terminal.proxy_status, Some(403),
        "the proxy's reply must ride proxy_connect_response"
    );
    assert_eq!(terminal.status, None, "no origin response exists");
}

/// CONNECT 200: the client must switch to tunnel mode (RFC 9110 §9.3.6) and
/// complete the TLS handshake + request through the relayed tunnel to the
/// real origin.
#[test]
fn connect_200_tunnels_tls_to_origin_c15() {
    let origin_port = spawn_tls_origin();
    let (proxy_port, capture) = spawn_proxy(origin_port, 200);
    let proxy_url: &'static [u8] = Box::leak(
        format!("http://127.0.0.1:{proxy_port}").into_bytes().into_boxed_slice(),
    );

    let deliveries = run_request_with(
        format!("https://127.0.0.1:{origin_port}/"),
        |options| {
            options.http_proxy = Some(bun_url::URL::parse(proxy_url));
            // Self-signed fixture cert.
            options.reject_unauthorized = Some(false);
        },
    );

    let terminal = deliveries
        .iter()
        .find(|d| !d.has_more)
        .expect("tunneled request must settle");
    assert_eq!(
        terminal.fail, None,
        "CONNECT 200 must tunnel to a completed exchange: {terminal:?}"
    );
    assert_eq!(terminal.status, Some(200));
    assert_eq!(terminal.bytes, b"tunneled", "origin's body delivered through the tunnel");

    let head = String::from_utf8(capture.head.lock().unwrap().clone()).expect("utf8 head");
    let first = head.lines().next().unwrap_or("");
    assert!(
        first.starts_with("CONNECT 127.0.0.1:") && first.ends_with(" HTTP/1.1"),
        "https targets must CONNECT-tunnel through the proxy, got: {first:?}"
    );
}
