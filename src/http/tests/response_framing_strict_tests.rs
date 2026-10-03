//! Wire-level regression lock for the response-side framing parsers the
//! upstream bun 83913e746a consolidation reshaped:
//!
//! - `parse_content_length_strict` (RFC 9110 §8.6 `1*DIGIT`): the shared
//!   parser replaces the inline `parse_unsigned` form. Wire-observable
//!   grammar is unchanged (radix-10 parse already rejected hex / sign /
//!   overflow) — the wire cases below pin that grammar so the consolidation
//!   cannot silently widen it.
//! - `fold_transfer_encoding` (RFC 9112 §6.1): THE behavior fix. bao's
//!   previous arm was an exact-match chain, so every valid list form
//!   (`gzip, chunked`, `Chunked`, OWS-padded) was rejected with
//!   UnsupportedTransferEncoding; the fold accepts them, rejects a coding
//!   after `chunked`, and makes the unknown-token rejection explicit.
//!
//! Wire-level harness mirroring `connection_close_guard_tests`: a raw-TCP
//! path-routed origin driven through the real `AsyncHTTP` + `HTTPThread`.
//! No JS runtime involved.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bun_core::MutableString;
use bun_http::signals::Store;
use bun_http::{
    AsyncHTTP, FetchRedirect, HTTPClientResult, HTTPClientResultCallback, Method, async_http,
};

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by bun_runtime::dispatch in product binaries.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

// Link seam for `__bun_crash_handler_out_of_memory` (see
// connection_close_guard_tests for rationale).
#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    eprintln!("bun: out of memory");
    std::process::abort()
}

// ─── Delivery recorder (verbatim shape from connection_close_guard_tests) ──

#[derive(Debug)]
struct Delivery {
    bytes: Vec<u8>,
    has_more: bool,
    fail: Option<bun_core::Error>,
    status: Option<u32>,
}

struct Recorder {
    tx: mpsc::Sender<Delivery>,
}

/// The `HTTPClientResultCallback`. Runs on the HTTP thread from
/// `progress_update` delivery (shape from connection_close_guard_tests).
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
    let status = result.metadata.as_ref().map(|m| m.response.status_code);
    let has_more = result.has_more;
    let fail = result.fail;

    let _ = rec.tx.send(Delivery {
        bytes,
        has_more,
        fail,
        status,
    });

    if !has_more {
        // Terminal delivery: reclaim the caller-thread `AsyncHTTP` box via
        // the `real` backref plus the response buffer — sole dropper,
        // mirroring `on_http_done` in fetch_async.rs.
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
fn run_request(url: String) -> Vec<Delivery> {
    bao_native_stubs::force_link();
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
    // async_http_id and the abort tracker entry.
    options.signals = Some(store.to());

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

// ─── Path-routed framing origin ────────────────────────────────────────────

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

/// Every answer promises `Connection: close` (one response per connection) —
/// framing is the variable under test, connection lifecycle is not.
fn spawn_framing_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            std::thread::spawn(move || {
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
                let answer: Vec<u8> = match path.as_str() {
                    // sanity: the accepted strict form
                    "/cl-ok" => {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
                            .to_vec()
                    }
                    // grammar pin: hex form is not 1*DIGIT (unchanged verdict)
                    "/cl-hex" => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: 0x10\r\nConnection: close\r\n\r\n0123456789abcdef"
                    )
                    .into_bytes(),
                    // decimal digit grammar: sign is not a digit
                    "/cl-plus" => {
                        b"HTTP/1.1 200 OK\r\nContent-Length: +5\r\nConnection: close\r\n\r\nhello"
                            .to_vec()
                    }
                    // overflow past u64: unrepresentable framing
                    "/cl-overflow" => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    // valid list form the exact-match arm rejected
                    "/te-list" => format!(
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\nConnection: close\r\n\r\n7\r\nnr1-nr2\r\n0\r\n\r\n"
                    )
                    .into_bytes(),
                    // case-insensitive token
                    "/te-upper" => format!(
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked\r\nConnection: close\r\n\r\n7\r\nnr1-nr2\r\n0\r\n\r\n"
                    )
                    .into_bytes(),
                    // OWS around the token
                    "/te-ows" => format!(
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: \tchunked \r\nConnection: close\r\n\r\n7\r\nnr1-nr2\r\n0\r\n\r\n"
                    )
                    .into_bytes(),
                    // chunked must be the FINAL coding
                    "/te-repeat" => format!(
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, chunked\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    // unknown coding token rejects (unchanged rule, locked)
                    "/te-unknown" => format!(
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: br2, chunked\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    _ => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                if stream.write_all(&answer).is_err() {
                    return;
                }
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
            });
        }
    });
    port
}

enum Expect {
    /// Resolves with exactly this body.
    Body(&'static str),
    /// Fails with exactly this framing error.
    Fail(bun_core::Error),
}

fn run_matrix() -> Result<(), String> {
    let port = spawn_framing_server();
    std::thread::sleep(Duration::from_millis(50));

    // (path, expect): the label doubles as the routed path on the origin.
    let cases: &[(&str, Expect)] = &[
        // sanity: the accepted strict form
        ("/cl-ok", Expect::Body("hello")),
        // hex form — parse_unsigned's auto-radix accepted it (the defect)
        // (hex / sign / overflow reject — grammar pin, verdict unchanged)
        ("/cl-hex", Expect::Fail(bun_core::err!(InvalidContentLength))),
        // sign is not a digit
        ("/cl-plus", Expect::Fail(bun_core::err!(InvalidContentLength))),
        // OWS is not part of the field value        // unrepresentable past u64
        ("/cl-overflow", Expect::Fail(bun_core::err!(InvalidContentLength))),
        // the fold: valid list forms go through chunked decode
        ("/te-list", Expect::Body("nr1-nr2")),
        ("/te-upper", Expect::Body("nr1-nr2")),
        ("/te-ows", Expect::Body("nr1-nr2")),
        // chunked must be the FINAL coding
        (
            "/te-repeat",
            Expect::Fail(bun_core::err!(UnsupportedTransferEncoding)),
        ),
        // unknown coding token (unchanged rule, now explicit)
        (
            "/te-unknown",
            Expect::Fail(bun_core::err!(UnsupportedTransferEncoding)),
        ),
    ];

    for (label, expect) in cases {
        let deliveries = run_request(format!("http://127.0.0.1:{port}{label}"));
        let last = deliveries
            .last()
            .ok_or_else(|| format!("{label}: no delivery arrived within 10s"))?;
        if last.has_more {
            return Err(format!(
                "{label}: last delivery is not terminal (has_more=true), deliveries={deliveries:?}"
            ));
        }
        match expect {
            Expect::Body(want) => {
                if let Some(fail) = &last.fail {
                    return Err(format!(
                        "{label}: terminal delivery failed: {:?} (expected body {want:?})",
                        fail
                    ));
                }
                let got = String::from_utf8_lossy(&last.bytes).to_string();
                if got != *want {
                    return Err(format!("{label}: body={got:?} expected {want:?}"));
                }
            }
            Expect::Fail(want) => {
                let fail = last
                    .fail
                    .as_ref()
                    .ok_or_else(|| format!(
                        "{label}: expected failure {:?}, got status {:?} body {:?}",
                        want.name(),
                        last.status,
                        String::from_utf8_lossy(&last.bytes)
                    ))?;
                if fail != want {
                    return Err(format!(
                        "{label}: error={} expected {}",
                        fail.name(),
                        want.name()
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The response framing receivers honor the RFC shapes: strict
/// `Content-Length` digit grammar (hex/sign/OWS/overflow reject) and the
/// `Transfer-Encoding` fold (valid list + case + OWS accept; chunked-final
/// and unknown-token reject).
#[test]
fn response_framing_parsers_match_rfc_shapes() {
    if let Err(e) = run_matrix() {
        panic!("response framing matrix failed: {e}");
    }
}
