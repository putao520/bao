//! CVE-2024-28182 regression tests for the fetch() HTTP/2 client
//! (upstream Bun a5c86aec7): a header block is bounded not just by
//! accumulated bytes (`LOCAL_MAX_HEADER_LIST_SIZE`) but by CONTINUATION
//! frame count (`LOCAL_MAX_CONTINUATIONS` = 8, nghttp2
//! `NGHTTP2_DEFAULT_MAX_CONTINUATIONS`). A hostile origin that answers
//! HEADERS without END_HEADERS and then streams zero-length CONTINUATION
//! frames never advances the byte cap — without the frame-count cap the
//! fetch() promise never settles (connection pinned, CPU burned on 9-byte
//! frame headers).
//!
//! Wire-level pure-Rust harness: ALPN-h2 BoringSSL TlsServer speaking
//! hand-rolled h2 framing (static-table + literal HPACK, no huffman) driven
//! through `AsyncHTTP` + `HTTPThread` — the same shape as
//! `tls_info_and_streaming_tests.rs`, no JS runtime involved.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsConnection, TlsServer, TlsState, generate_self_signed_pem};
use bun_core::MutableString;
use bun_http::signals::Store;
use bun_http::{AsyncHTTP, HTTPClientResult, HTTPClientResultCallback, Method, FetchRedirect,
               async_http};

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by bun_runtime::dispatch in product binaries.
// No FilePoll sources are registered anywhere in these tests, so a no-op
// satisfies the link-time reference. (Mirror of tls_info_and_streaming_tests.)
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

// Link seam for `__bun_crash_handler_out_of_memory` (see
// tls_info_and_streaming_tests for rationale).
#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    eprintln!("bun: out of memory");
    std::process::abort()
}

// ─── Adversarial response scripts ──────────────────────────────────────────

/// Which hostile/benign CONTINUATION shape the server emits for the first
/// request stream it sees.
#[derive(Clone, Copy)]
enum Scenario {
    /// HEADERS + exactly 8 CONTINUATIONs (last one END_HEADERS): the full
    /// budget is legal and must deliver a 200.
    FullBudgetAccepted,
    /// HEADERS + 9 CONTINUATIONs: the 9th is rejected even though it carries
    /// END_HEADERS and would complete the block (nghttp2 / node 22 parity).
    NinthFrameRejected,
    /// HEADERS + 10 000 zero-length CONTINUATIONs: the CVE-2024-28182 repro.
    /// Empty payloads never advance the byte cap; only the frame-count cap
    /// ends this.
    ZeroLengthFlood,
    /// A 103 informational block and the final 200 block on one stream each
    /// split across all 8 CONTINUATIONs: the budget is per header block, not
    /// per stream or connection (16 CONTINUATIONs total are fine).
    BudgetResetsPerBlock,
}

/// Minimal HTTP/2 framing (RFC 9113): header + payload.
fn frame(frame_type: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + payload.len());
    out.push((payload.len() >> 16) as u8);
    out.push((payload.len() >> 8) as u8);
    out.push(payload.len() as u8);
    out.push(frame_type);
    out.push(flags);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

const FT_HEADERS: u8 = 0x1;
const FT_SETTINGS: u8 = 0x4;
const FT_GOAWAY: u8 = 0x7;
const FT_CONTINUATION: u8 = 0x9;
const FLAG_ACK: u8 = 0x1;
const FLAG_END_STREAM: u8 = 0x1;
const FLAG_END_HEADERS: u8 = 0x4;

/// HPACK `:status` literal with indexed name (static index 8) — covers 103;
/// 200 uses the fully indexed form `0x88`.
fn hpack_status(code: &str) -> Vec<u8> {
    let mut block = vec![0x08, code.len() as u8];
    block.extend_from_slice(code.as_bytes());
    block
}

/// One header block split over HEADERS + 8 CONTINUATIONs (the last carries
/// END_HEADERS). `headers_flags` distinguishes END_STREAM (final response)
/// from 0 (informational block that keeps the stream open).
fn split_block(headers_flags: u8, block: &[u8], stream: u32) -> Vec<u8> {
    let mut out = frame(FT_HEADERS, headers_flags, stream, block);
    for _ in 0..7 {
        out.extend_from_slice(&frame(FT_CONTINUATION, 0, stream, &[]));
    }
    out.extend_from_slice(&frame(FT_CONTINUATION, FLAG_END_HEADERS, stream, &[]));
    out
}

// ─── ALPN-h2 TLS fixture ────────────────────────────────────────────────────

/// Length-prefixed ALPN wire entry for "h2".
const ALPN_H2: &[u8] = b"\x02h2";

unsafe extern "C" fn alpn_select_h2(
    _ssl: *mut bun_boringssl_sys::SSL,
    out: *mut *const u8,
    out_len: *mut u8,
    in_: *const u8,
    in_len: core::ffi::c_uint,
    _arg: *mut core::ffi::c_void,
) -> core::ffi::c_int {
    let list = unsafe { std::slice::from_raw_parts(in_, in_len as usize) };
    let mut offset = 0usize;
    while offset < list.len() {
        let len = list[offset] as usize;
        offset += 1;
        if offset + len > list.len() {
            break;
        }
        if &list[offset..offset + len] == b"h2" {
            unsafe {
                *out = ALPN_H2.as_ptr().add(1); // past the length byte
                *out_len = 2;
            }
            return bun_boringssl_sys::SSL_TLSEXT_ERR_OK;
        }
        offset += len;
    }
    bun_boringssl_sys::SSL_TLSEXT_ERR_NOACK
}

/// TLS stream adapter driving the server-side BoringSSL state machine over
/// the raw TCP socket (mirror of tls_info_and_streaming_tests' ServerTlsIo).
struct ServerTlsIo {
    tcp: TcpStream,
    tls: TlsConnection,
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl ServerTlsIo {
    fn handshake(tcp: &mut TcpStream, tls: &mut TlsConnection) -> std::io::Result<()> {
        loop {
            let res = tls
                .process()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            loop {
                let outgoing = tls.take_outgoing();
                if outgoing.is_empty() {
                    break;
                }
                tcp.write_all(&outgoing)?;
            }
            if res.state == TlsState::Active || res.state == TlsState::PeerClosed {
                return Ok(());
            }
            let mut buf = [0u8; 16_384];
            match tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "peer closed during tls handshake",
                    ))
                }
                Ok(n) => tls.feed(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    }

    fn read_plaintext(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            let outgoing = self.tls.take_outgoing();
            if !outgoing.is_empty() {
                self.tcp.write_all(&outgoing)?;
            }
            let res = self
                .tls
                .process()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            if !res.plaintext.is_empty() {
                let mut joined = Vec::new();
                for chunk in res.plaintext {
                    joined.extend_from_slice(&chunk);
                }
                return Ok(joined);
            }
            let mut buf = [0u8; 16_384];
            match self.tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "tls peer closed",
                    ))
                }
                Ok(n) => self.tls.feed(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    }
}

impl Read for ServerTlsIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending_off >= self.pending_plain.len() {
            self.pending_plain = self.read_plaintext()?;
            self.pending_off = 0;
        }
        let avail = &self.pending_plain[self.pending_off..];
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.pending_off += n;
        Ok(n)
    }
}

impl Write for ServerTlsIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self
            .tls
            .write(buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let outgoing = self.tls.take_outgoing();
        if !outgoing.is_empty() {
            self.tcp.write_all(&outgoing)?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

/// Spawn the adversarial h2 origin. One connection is served: the h2
/// handshake (preface + SETTINGS exchange) completes normally, then the
/// scripted CONTINUATION shape is written for the first request HEADERS the
/// client sends. Write errors after the client GOAWAYs are ignored — the
/// point is what the client does with the frames it already parsed.
fn spawn_adversarial_h2_server(scenario: Scenario) -> u16 {
    let (cert, key) =
        generate_self_signed_pem("127.0.0.1", 365).expect("self-signed cert");
    let server = std::sync::Arc::new(TlsServer::new(&cert, &key).expect("TlsServer"));
    // SAFETY: TlsServer::ctx returns its live SSL_CTX; installing the ALPN
    // select callback before any accept is thread-free.
    unsafe {
        bun_boringssl_sys::SSL_CTX_set_alpn_select_cb(
            server.ctx(),
            Some(alpn_select_h2),
            core::ptr::null_mut(),
        );
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut tcp, _)) = listener.accept() else {
            return;
        };
        let Ok(mut tls) = server.accept() else {
            return;
        };
        if ServerTlsIo::handshake(&mut tcp, &mut tls).is_err() {
            return;
        }
        if tls.alpn_protocol() != Some(&b"h2"[..]) {
            return;
        }
        let mut io = ServerTlsIo {
            tcp,
            tls,
            pending_plain: Vec::new(),
            pending_off: 0,
        };
        serve_adversarial_h2(&mut io, scenario);
    });
    port
}

/// h2 connection loop: read the client preface, ACK every non-ACK SETTINGS,
/// and answer the first request HEADERS with the scenario script.
fn serve_adversarial_h2(io: &mut ServerTlsIo, scenario: Scenario) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut magic = [0u8; 24];
    if read_exact_deadline(io, &mut magic, deadline).is_err() {
        return;
    }
    if &magic != b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" {
        return;
    }
    let mut buffer: Vec<u8> = Vec::new();
    let mut answered = false;
    loop {
        // Frame header.
        while buffer.len() < 9 {
            let mut chunk = [0u8; 4096];
            match io.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if Instant::now() > deadline {
                        return;
                    }
                    continue;
                }
                Err(_) => return,
            }
        }
        let frame_len = ((buffer[0] as usize) << 16) | ((buffer[1] as usize) << 8) | buffer[2] as usize;
        let frame_type = buffer[3];
        let flags = buffer[4];
        let stream = u32::from_be_bytes([buffer[5], buffer[6], buffer[7], buffer[8]]) & 0x7fff_ffff;
        while buffer.len() < 9 + frame_len {
            let mut chunk = [0u8; 16384];
            match io.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if Instant::now() > deadline {
                        return;
                    }
                    continue;
                }
                Err(_) => return,
            }
        }
        buffer.drain(..9 + frame_len);

        match frame_type {
            FT_SETTINGS if flags & FLAG_ACK == 0 => {
                // Server SETTINGS (empty) + ACK of the client's.
                let _ = io.write_all(&frame(FT_SETTINGS, 0, 0, &[]));
                let _ = io.write_all(&frame(FT_SETTINGS, FLAG_ACK, 0, &[]));
                let _ = io.flush();
            },
            FT_HEADERS if !answered => {
                answered = true;
                let script = adversarial_script(scenario, stream);
                // The client GOAWAYs and closes once the cap fires mid-script
                // (NinthFrame / ZeroLengthFlood); a write error there is the
                // expected outcome, not a failure of the fixture.
                let _ = io.write_all(&script);
                let _ = io.flush();
            },
            FT_GOAWAY => return,
            _ => {},
        }
        if Instant::now() > deadline {
            return;
        }
    }
}

/// The hostile/benign byte sequence for one request stream.
fn adversarial_script(scenario: Scenario, stream: u32) -> Vec<u8> {
    match scenario {
        Scenario::FullBudgetAccepted => split_block(FLAG_END_STREAM, &[0x88], stream),
        Scenario::NinthFrameRejected => {
            let mut out = frame(FT_HEADERS, FLAG_END_STREAM, stream, &[0x88]);
            for _ in 0..8 {
                out.extend_from_slice(&frame(FT_CONTINUATION, 0, stream, &[]));
            }
            out.extend_from_slice(&frame(FT_CONTINUATION, FLAG_END_HEADERS, stream, &[]));
            out
        },
        Scenario::ZeroLengthFlood => {
            let mut out = frame(FT_HEADERS, FLAG_END_STREAM, stream, &[0x88]);
            // 10 000 empty CONTINUATION frames = 90 000 bytes of 9-byte
            // headers with zero payload; the accumulated header block stays
            // at 1 byte forever, so only the frame-count cap can end this.
            for _ in 0..10_000 {
                out.extend_from_slice(&frame(FT_CONTINUATION, 0, stream, &[]));
            }
            out
        },
        Scenario::BudgetResetsPerBlock => {
            let mut out = split_block(0, &hpack_status("103"), stream);
            out.extend_from_slice(&split_block(FLAG_END_STREAM, &[0x88], stream));
            out
        },
    }
}

fn read_exact_deadline(io: &mut ServerTlsIo, buf: &mut [u8], deadline: Instant) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        if Instant::now() > deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "h2 preface timeout",
            ));
        }
        match io.read(&mut buf[filled..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof during preface",
                ))
            },
            Ok(n) => filled += n,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {},
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ─── Client harness (mirror of tls_info_and_streaming_tests) ───────────────

#[derive(Debug)]
struct Delivery {
    status: Option<u32>,
    fail: Option<bun_core::Error>,
    has_more: bool,
}

struct Recorder {
    tx: mpsc::Sender<Delivery>,
}

/// The `HTTPClientResultCallback`; runs on the HTTP thread.
fn recorder_callback(
    this: *mut Recorder,
    async_http: *mut AsyncHTTP<'static>,
    result: HTTPClientResult<'_>,
) {
    let rec: &Recorder = unsafe { &*this };
    let status = result.metadata.as_ref().map(|m| m.response.status_code);
    let fail = result.fail.clone();
    let has_more = result.has_more;

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

    let _ = rec.tx.send(Delivery {
        status,
        fail,
        has_more,
    });
}

/// Drive one GET through the real HTTPThread over ALPN-negotiated h2 and
/// collect deliveries until the terminal (`has_more == false`) one.
fn run_h2_fetch(port: u16) -> Vec<Delivery> {
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    let (tx, rx) = mpsc::channel();
    // Leaked on purpose: the Signals NonNulls point into this store for the
    // whole request lifetime; a stable heap address avoids any relocation.
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let recorder = Box::into_raw(Box::new(Recorder { tx }));

    let url = format!("https://127.0.0.1:{}/", port);
    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);

    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    options.signals = Some(store.to());
    // Self-signed fixture cert.
    options.reject_unauthorized = Some(false);

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
    out
}

/// Terminal delivery of a successful fetch: not failed, status 200.
fn assert_ok_200(deliveries: &[Delivery], ctx: &str) {
    let Some(last) = deliveries.last() else {
        panic!("{}: no delivery before deadline (fetch hung — the CONTINUATION cap regressed?)", ctx);
    };
    assert!(!last.has_more, "{}: no terminal delivery", ctx);
    assert!(
        last.fail.is_none(),
        "{}: expected success, got fail {:?}",
        ctx,
        last.fail.map(|e| e.name())
    );
    assert_eq!(last.status, Some(200), "{}: expected 200", ctx);
}

/// Terminal delivery fails with HTTP2EnhanceYourCalm.
fn assert_enhance_your_calm(deliveries: &[Delivery], ctx: &str) {
    let Some(last) = deliveries.last() else {
        panic!("{}: no delivery before deadline (fetch hung—CVE-2024-28182 repro)", ctx);
    };
    assert!(!last.has_more, "{}: no terminal delivery", ctx);
    let fail = last
        .fail
        .unwrap_or_else(|| panic!("{}: expected a failure, got status {:?}", ctx, last.status));
    assert_eq!(
        fail,
        bun_core::err!(HTTP2EnhanceYourCalm),
        "{}: expected HTTP2EnhanceYourCalm, got {}",
        ctx,
        fail.name()
    );
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// HEADERS + exactly 8 CONTINUATIONs (END_HEADERS on the 8th) is inside the
/// budget and must complete the response (nghttp2 parity: the cap is `> 8`,
/// not `>= 8`).
#[test]
fn h2_continuation_full_budget_is_accepted() {
    let port = spawn_adversarial_h2_server(Scenario::FullBudgetAccepted);
    let deliveries = run_h2_fetch(port);
    assert_ok_200(&deliveries, "full 8-frame budget");
}

/// The 9th CONTINUATION is rejected even when it carries END_HEADERS and
/// would have completed the block (pins the cap at exactly 8 and the
/// comparison at `>`).
#[test]
fn h2_continuation_ninth_frame_is_rejected() {
    let port = spawn_adversarial_h2_server(Scenario::NinthFrameRejected);
    let deliveries = run_h2_fetch(port);
    assert_enhance_your_calm(&deliveries, "9th CONTINUATION");
}

/// CVE-2024-28182 repro: zero-length CONTINUATION flood. Empty payloads
/// never advance `LOCAL_MAX_HEADER_LIST_SIZE`, so without the frame-count
/// cap the fetch pins forever; with it the connection fails
/// `HTTP2EnhanceYourCalm` promptly.
#[test]
fn h2_continuation_zero_length_flood_is_rejected() {
    let start = Instant::now();
    let port = spawn_adversarial_h2_server(Scenario::ZeroLengthFlood);
    let deliveries = run_h2_fetch(port);
    assert_enhance_your_calm(&deliveries, "zero-length flood");
    // The 9th frame of the flood is already over budget, so the failure must
    // arrive long before the fixture's 10 000 frames / 15 s deadline.
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "flood rejection took {:?} — cap fired too late",
        start.elapsed()
    );
}

/// The budget is per header block, not per stream or connection: a 103
/// informational block and the final 200 block on one stream may each use
/// all 8 CONTINUATIONs (16 total). Fails if the counter is not reset at
/// HEADERS.
#[test]
fn h2_continuation_budget_resets_per_block() {
    let port = spawn_adversarial_h2_server(Scenario::BudgetResetsPerBlock);
    let deliveries = run_h2_fetch(port);
    assert_ok_200(&deliveries, "budget resets per block");
}
