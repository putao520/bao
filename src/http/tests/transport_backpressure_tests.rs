//! W2 transport-backpressure regression tests (h1 socket pause/resume +
//! h2 per-stream WINDOW_UPDATE gate), driven through the real HTTPThread.
//!
//! 1. `h2_paused_stream_does_not_starve_sibling_and_resume_releases` — two
//!    fetches multiplexed on ONE h2 session (the server accepts exactly one
//!    TCP connection, so a second connection would hang and fail the test).
//!    Stream A is paused from the first delivery on; the server honors flow
//!    control and stalls A once its 16 MiB initial grant is exhausted, while
//!    sibling B completes on the same session (connection-level window keeps
//!    flowing). Wire evidence: zero stream-A WINDOW_UPDATEs while paused;
//!    resume pushes the withheld grant and A finishes.
//!
//! 2. `h1_pause_backpressures_server_resume_completes` — a plain-TCP h1
//!    origin blasting an 8 MiB body: after the consumer pauses, the server
//!    blocks in write (TCP backpressure) and client deliveries stop short of
//!    the full body; resume drains the kernel buffers, the server finishes,
//!    and the request completes with every byte accounted.
//!
//! No JS runtime — the recorder callback stands in for the W1 consumer
//! (pause on first chunk from the HTTP thread; resume from the main thread
//! through the cross-thread entry, exercising both hook surfaces).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsConnection, TlsServer, TlsState, generate_self_signed_pem};
use bun_core::MutableString;
use bun_http::http_thread::TransportPauseKind;
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
    body_len: u64,
    has_more: bool,
    failed: bool,
    status: Option<u32>,
}

struct Recorder {
    tx: mpsc::Sender<Delivery>,
    /// Consumer-stand-in behavior: schedule a transport Pause from this
    /// (HTTP-thread) callback on the first streaming delivery — the W1
    /// "staging full, park the transport" decision.
    pause_after_first: bool,
    paused: AtomicBool,
}

/// The `HTTPClientResultCallback`; runs on the HTTP thread.
fn recorder_callback(
    this: *mut Recorder,
    async_http: *mut AsyncHTTP<'static>,
    mut result: HTTPClientResult<'_>,
) {
    let rec: &Recorder = unsafe { &*this };
    let body_len = result
        .body
        .as_deref()
        .map(|b| b.list.len() as u64)
        .unwrap_or(0);
    let status = result.metadata.as_ref().map(|m| m.response.status_code);
    let has_more = result.has_more;
    let failed = result.fail.is_some();

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
    } else if !failed {
        // Streaming consumer contract: drain the shared body buffer so the
        // next delivery contains only newly arrived bytes.
        if let Some(b) = result.body.as_deref_mut() {
            b.list.clear();
        }
        if rec.pause_after_first && !rec.paused.swap(true, Ordering::SeqCst) {
            let id = unsafe { (*async_http).async_http_id };
            bun_http::http_thread_mut().schedule_transport_pause(id, TransportPauseKind::Pause);
        }
    }

    let _ = rec.tx.send(Delivery {
        body_len,
        has_more,
        failed,
        status,
    });
}

/// One streaming fetch through the real HTTPThread. Returns (id, receiver).
fn spawn_streaming_fetch(url: String, https: bool, pause_after_first: bool) -> (u32, mpsc::Receiver<Delivery>) {
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    let (tx, rx) = mpsc::channel();
    // Leaked on purpose: the Signals NonNulls point into this store for the
    // whole request lifetime; a stable heap address avoids any relocation.
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let recorder = Box::into_raw(Box::new(Recorder {
        tx,
        pause_after_first,
        paused: AtomicBool::new(false),
    }));

    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);

    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    // Full signal store (aborted slot wired) so the request gets a real
    // async_http_id and the tracker entry the pause drain resolves.
    options.signals = Some(store.to());
    if https {
        options.reject_unauthorized = Some(false);
    }

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

/// Collect deliveries from `rx` until the terminal (`has_more == false`) one
/// or the deadline. Returns every delivery seen (terminal last, if any).
fn collect_until_terminal(rx: &mpsc::Receiver<Delivery>, deadline: Duration) -> Vec<Delivery> {
    let mut out = Vec::new();
    let end = Instant::now() + deadline;
    loop {
        let Some(remaining) = end.checked_duration_since(Instant::now()) else {
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

/// Wait for the first streaming delivery and return immediately — a paused
/// request never reaches terminal, so waiting for one here would burn the
/// whole deadline.
fn wait_for_first_delivery(rx: &mpsc::Receiver<Delivery>, deadline: Duration) -> Vec<Delivery> {
    let end = Instant::now() + deadline;
    let mut out = Vec::new();
    while let Some(remaining) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(d) => {
                let terminal = !d.has_more;
                out.push(d);
                if !terminal {
                    break;
                }
            },
            Err(_) => break,
        }
    }
    out
}

// ─── Test 1: h2 multiplex — one paused, one reading ────────────────────────

/// Minimal HTTP/2 framing helpers (mirror of h2_continuation_cap_tests).
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

const FT_DATA: u8 = 0x0;
const FT_HEADERS: u8 = 0x1;
const FT_SETTINGS: u8 = 0x4;
const FT_GOAWAY: u8 = 0x7;
const FT_WINDOW_UPDATE: u8 = 0x8;
const FLAG_ACK: u8 = 0x1;
const FLAG_END_STREAM: u8 = 0x1;
const FLAG_END_HEADERS: u8 = 0x4;

/// HPACK `:status: 200` — fully indexed (static index 8).
const HPACK_200: &[u8] = &[0x88];

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
/// the raw TCP socket (mirror of the sibling h2 test files).
struct ServerTlsIo {
    tcp: TcpStream,
    tls: TlsConnection,
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl ServerTlsIo {
    fn handshake(tcp: &mut TcpStream, tls: &mut TlsConnection) -> std::io::Result<Vec<u8>> {
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
                // The handshake-completing process() may have decrypted
                // application data that piggybacked on the final handshake
                // record (e.g. the client's Finished + first h2 record read
                // as one segment). It must be delivered, not discarded, or
                // the server waits forever for bytes it already consumed.
                let mut piggybacked = Vec::new();
                for chunk in res.plaintext {
                    piggybacked.extend_from_slice(&chunk);
                }
                return Ok(piggybacked);
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

/// Server-side observability shared with the test main thread.
struct H2Evidence {
    /// WINDOW_UPDATE frames received on stream A's id.
    wu_a: AtomicU32,
    /// Stream A exhausted its granted window with body left to send.
    a_stalled: AtomicBool,
}

/// A's body is larger than the client-advertised 16 MiB initial window so a
/// well-behaved server MUST stall before finishing unless the client grants
/// more window.
const H2_A_TOTAL: usize = 18 * 1024 * 1024;
const H2_A_FRAME: usize = 16 * 1024;

/// One h2 connection, two streams: A (big body, flow-control honoring) and
/// B (small body, answered whenever its HEADERS arrive). Exactly one TCP
/// accept — a client that opens a second connection for B hangs instead.
fn spawn_multiplex_h2_server() -> (u16, Arc<H2Evidence>) {
    let (cert, key) = generate_self_signed_pem("127.0.0.1", 365).expect("self-signed cert");
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
    let evidence = Arc::new(H2Evidence {
        wu_a: AtomicU32::new(0),
        a_stalled: AtomicBool::new(false),
    });
    let ev = evidence.clone();
    std::thread::spawn(move || {
        let Ok((mut tcp, _)) = listener.accept() else {
            return;
        };
        // Periodic read timeouts let the pump loop re-run between client
        // frames instead of parking forever on a silent (backpressured)
        // connection.
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
        let Ok(mut tls) = server.accept() else {
            return;
        };
        let piggybacked = match ServerTlsIo::handshake(&mut tcp, &mut tls) {
            Ok(p) => p,
            Err(_) => return,
        };
        if tls.alpn_protocol() != Some(&b"h2"[..]) {
            return;
        }
        let mut io = ServerTlsIo {
            tcp,
            tls,
            pending_plain: piggybacked,
            pending_off: 0,
        };
        serve_multiplex_h2(&mut io, &ev);
    });
    (port, evidence)
}

/// Flow-control-honoring h2 origin: stream A's DATA only moves while both
/// its stream window and the connection window have room; WINDOW_UPDATE on
/// stream 0 is honored automatically (that is the client behavior under
/// test — connection-level replenish must keep flowing while stream A's
/// grant is withheld).
fn serve_multiplex_h2(io: &mut ServerTlsIo, ev: &H2Evidence) {
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut magic = [0u8; 24];
    if read_exact_deadline(io, &mut magic, deadline).is_err() {
        return;
    }
    if &magic != b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" {
        return;
    }

    let mut a_id: u32 = 0;
    let mut a_window: i64 = 65_535;
    let mut a_sent: usize = 0;
    let mut conn_window: i64 = 65_535;
    let chunk = [b'a'; H2_A_FRAME];

    let mut buffer: Vec<u8> = Vec::new();
    loop {
        // Pump A while both windows allow and body remains.
        loop {
            if a_id == 0 || a_sent >= H2_A_TOTAL {
                break;
            }
            let room_stream = a_window.min(H2_A_TOTAL as i64 - a_sent as i64);
            let room_conn = conn_window;
            let room = room_stream.min(room_conn).min(H2_A_FRAME as i64);
            if room <= 0 {
                if a_window <= 0 && a_sent < H2_A_TOTAL {
                    ev.a_stalled.store(true, Ordering::SeqCst);
                }
                break;
            }
            let n = room as usize;
            let last = a_sent + n >= H2_A_TOTAL;
            let flags = if last { FLAG_END_STREAM } else { 0 };
            if io.write_all(&frame(FT_DATA, flags, a_id, &chunk[..n])).is_err() {
                return;
            }
            a_window -= n as i64;
            conn_window -= n as i64;
            a_sent += n;
            if last {
                let _ = io.flush();
            }
        }
        if Instant::now() > deadline {
            return;
        }

        // Pull one client frame (returning early on timeout → pump again).
        while buffer.len() < 9 {
            let mut tmp = [0u8; 4096];
            match io.read(&mut tmp) {
                Ok(0) => return,
                Ok(n) => buffer.extend_from_slice(&tmp[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break
                }
                Err(_) => return,
            }
        }
        if buffer.len() < 9 {
            continue;
        }
        let frame_len =
            ((buffer[0] as usize) << 16) | ((buffer[1] as usize) << 8) | buffer[2] as usize;
        if frame_len > 1_048_576 {
            return;
        }
        while buffer.len() < 9 + frame_len {
            let mut tmp = [0u8; 16_384];
            match io.read(&mut tmp) {
                Ok(0) => return,
                Ok(n) => buffer.extend_from_slice(&tmp[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break
                }
                Err(_) => return,
            }
        }
        if buffer.len() < 9 + frame_len {
            continue;
        }
        let frame_type = buffer[3];
        let flags = buffer[4];
        let stream = u32::from_be_bytes([buffer[5], buffer[6], buffer[7], buffer[8]]) & 0x7fff_ffff;
        let payload = buffer[9..9 + frame_len].to_vec();
        buffer.drain(..9 + frame_len);

        match frame_type {
            FT_SETTINGS if flags & FLAG_ACK == 0 => {
                // Our SETTINGS (empty) + ACK of theirs.
                let _ = io.write_all(&frame(FT_SETTINGS, 0, 0, &[]));
                let _ = io.write_all(&frame(FT_SETTINGS, FLAG_ACK, 0, &[]));
                // Adopt the client's SETTINGS_INITIAL_WINDOW_SIZE (id 0x4)
                // for streams opened from here on.
                let mut off = 0;
                while off + 6 <= payload.len() {
                    let id = u16::from_be_bytes([payload[off], payload[off + 1]]);
                    let value = u32::from_be_bytes([
                        payload[off + 2],
                        payload[off + 3],
                        payload[off + 4],
                        payload[off + 5],
                    ]);
                    if id == 0x4 && a_id == 0 {
                        a_window = value as i64;
                    }
                    off += 6;
                }
                let _ = io.flush();
            },
            FT_HEADERS if a_id == 0 => {
                // Stream A: response headers, body drips under flow control.
                a_id = stream;
                let _ = io.write_all(&frame(FT_HEADERS, FLAG_END_HEADERS, stream, HPACK_200));
                let _ = io.flush();
            },
            FT_HEADERS => {
                // Sibling B: complete immediately inside its fresh window.
                let mut out = frame(FT_HEADERS, FLAG_END_HEADERS, stream, HPACK_200);
                out.extend_from_slice(&frame(FT_DATA, FLAG_END_STREAM, stream, b"ok"));
                let _ = io.write_all(&out);
                let _ = io.flush();
            },
            FT_WINDOW_UPDATE => {
                let inc = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as i64;
                if stream == 0 {
                    conn_window += inc;
                } else if stream == a_id {
                    ev.wu_a.fetch_add(1, Ordering::SeqCst);
                    a_window += inc;
                }
            },
            FT_GOAWAY => return,
            _ => {},
        }
    }
}

fn read_exact_deadline(
    io: &mut ServerTlsIo,
    buf: &mut [u8],
    deadline: Instant,
) -> std::io::Result<()> {
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

/// Same-session multiplexing under backpressure: pausing stream A's
/// transport must (a) actually gate A — the flow-control-honoring server
/// stalls with zero stream-A WINDOW_UPDATEs received — while (b) sibling B
/// on the SAME session completes (connection window keeps replenishing),
/// and (c) resume releases the withheld grant so A finishes.
#[test]
fn h2_paused_stream_does_not_starve_sibling_and_resume_releases() {
    let (port, ev) = spawn_multiplex_h2_server();

    let (a_id, a_rx) = spawn_streaming_fetch(format!("https://127.0.0.1:{}/a", port), true, true);
    assert_ne!(a_id, 0, "signals store must assign a real async_http_id");

    // First delivery arrives (headers + first body bytes), which schedules
    // the pause from the HTTP-thread callback.
    let first = wait_for_first_delivery(&a_rx, Duration::from_secs(15));
    assert!(
        first.iter().any(|d| d.has_more),
        "no streaming delivery before deadline — fetch hung"
    );

    // Wait until the server has exhausted A's granted window. This is the
    // gate's wire-level proof: replenishment was withheld.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ev.a_stalled.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "server never stalled on A — stream-level WINDOW_UPDATE kept flowing while paused"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        ev.wu_a.load(Ordering::SeqCst),
        0,
        "stream-A WINDOW_UPDATE reached the server while the stream was paused"
    );
    // No terminal delivery for A while paused (it cannot finish: the server
    // is out of window). Drain non-destructively-by-value: everything
    // received here still counts toward the final byte total.
    let mut queued_while_stalled = Vec::new();
    while let Ok(d) = a_rx.try_recv() {
        assert!(d.has_more, "A terminated while its transport was paused");
        queued_while_stalled.push(d);
    }

    // Sibling B on the same origin — one accepted connection forces the
    // client to multiplex onto A's session.
    let (_b_id, b_rx) = spawn_streaming_fetch(format!("https://127.0.0.1:{}/b", port), true, false);
    let b = collect_until_terminal(&b_rx, Duration::from_secs(20));
    let Some(b_last) = b.last() else {
        panic!("B produced no delivery — sibling stream starved or session not reused");
    };
    assert!(!b_last.has_more, "B has no terminal delivery");
    assert!(!b_last.failed, "B failed: {:?}", b_last.status);
    assert_eq!(b_last.status, Some(200), "B expected 200");

    // Still gated while B completed.
    assert_eq!(
        ev.wu_a.load(Ordering::SeqCst),
        0,
        "stream-A WINDOW_UPDATE leaked while B was multiplexed and A paused"
    );

    // Resume A from a non-HTTP thread — the W1 consumer's entry surface.
    bun_http::HTTPThread::schedule_transport_pause_from_any_thread(a_id, TransportPauseKind::Resume);

    let a = {
        let mut all = first;
        all.extend(queued_while_stalled);
        all.extend(collect_until_terminal(&a_rx, Duration::from_secs(20)));
        all
    };
    let Some(a_last) = a.last() else {
        panic!("A produced no terminal delivery after resume");
    };
    assert!(!a_last.has_more, "A has no terminal delivery after resume");
    assert!(!a_last.failed, "A failed after resume");
    // Metadata rides the first streaming delivery (cloned_metadata is
    // consumed once); the terminal delivery carries only the body tail.
    assert_eq!(
        a.iter().find(|d| d.status.is_some()).and_then(|d| d.status),
        Some(200),
        "A expected 200 after resume"
    );
    let total: u64 = a.iter().map(|d| d.body_len).sum();
    assert_eq!(
        total as usize, H2_A_TOTAL,
        "A body bytes after resume: got {total}, want {H2_A_TOTAL}"
    );
    assert!(
        ev.wu_a.load(Ordering::SeqCst) >= 1,
        "resume never pushed the withheld stream-A WINDOW_UPDATE"
    );
}

// ─── Test 2: h1 socket pause → TCP backpressure ────────────────────────────

const H1_TOTAL: usize = 8 * 1024 * 1024;

/// Plain-TCP h1 origin: answers one GET with Content-Length: 8 MiB and
/// blasts the body with blocking writes — once the client pauses its read
/// side, the kernel buffers fill and this server blocks in `write_all`,
/// which is the observable TCP backpressure.
fn spawn_blast_h1_server(written: Arc<AtomicUsize>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        // Drain the request head.
        let mut buf = [0u8; 4096];
        let mut head = Vec::new();
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {H1_TOTAL}\r\n\r\n");
        if stream.write_all(header.as_bytes()).is_err() {
            return;
        }
        written.store(header.len(), Ordering::SeqCst);
        let chunk = [b'h'; 64 * 1024];
        let mut sent: usize = 0;
        while sent < H1_TOTAL {
            let n = chunk.len().min(H1_TOTAL - sent);
            if stream.write_all(&chunk[..n]).is_err() {
                return;
            }
            sent += n;
            written.store(header.len() + sent, Ordering::SeqCst);
        }
        let _ = stream.flush();
    });
    port
}

#[test]
fn h1_pause_backpressures_server_resume_completes() {
    let written = Arc::new(AtomicUsize::new(0));
    let port = spawn_blast_h1_server(written.clone());

    let (id, rx) = spawn_streaming_fetch(format!("http://127.0.0.1:{}/", port), false, true);
    assert_ne!(id, 0, "signals store must assign a real async_http_id");

    // First delivery schedules the pause; the server keeps writing until
    // the kernel buffers fill, then blocks.
    let mut deliveries = wait_for_first_delivery(&rx, Duration::from_secs(15));
    assert!(
        deliveries.iter().any(|d| d.has_more),
        "no streaming delivery before deadline — fetch hung"
    );

    // Wait for quiescence: no new deliveries for 400ms AND the server's
    // write counter stable below the total (blocked in write_all).
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "client never went quiet after pause");
        let quiet = rx.recv_timeout(Duration::from_millis(400));
        match quiet {
            Ok(d) => deliveries.push(d),
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let sample_a = written.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(400));
    let sample_b = written.load(Ordering::SeqCst);
    assert_eq!(sample_a, sample_b, "server kept writing while the socket was paused");
    assert!(sample_b < H1_TOTAL, "server finished the body despite the pause");
    let received: u64 = deliveries.iter().map(|d| d.body_len).sum();
    assert!(
        (received as usize) < H1_TOTAL,
        "client received the whole body while paused: {received}"

    );
    assert!(
        deliveries.iter().all(|d| d.has_more),
        "request terminated while its transport was paused"
    );

    // Resume from a non-HTTP thread: kernel-buffered bytes fire, the server
    // unblocks, and the body completes in full.
    bun_http::HTTPThread::schedule_transport_pause_from_any_thread(id, TransportPauseKind::Resume);
    deliveries.extend(collect_until_terminal(&rx, Duration::from_secs(20)));

    let last = deliveries
        .last()
        .expect("no terminal delivery after resume");
    assert!(!last.has_more, "no terminal delivery after resume");
    assert!(!last.failed, "request failed after resume");
    // Metadata rides the first streaming delivery; see the h2 test note.
    assert_eq!(
        deliveries.iter().find(|d| d.status.is_some()).and_then(|d| d.status),
        Some(200)
    );
    let total: u64 = deliveries.iter().map(|d| d.body_len).sum();
    assert_eq!(total as usize, H1_TOTAL, "body bytes after resume: got {total}, want {H1_TOTAL}");
}
