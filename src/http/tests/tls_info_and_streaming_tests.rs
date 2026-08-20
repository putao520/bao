//! Pure-Rust integration tests for the two servo-bridge phase-2 inputs of
//! the page-network unification:
//!
//! 1. `BunTlsInfo` — the TLS security snapshot delivered on
//!    `HTTPClientResult::tls_info`. Drives a real TLS request (bridge
//!    `TlsServer` with a self-signed cert) through `AsyncHTTP` +
//!    `HTTPThread` and asserts every field matches the server's actual
//!    configuration (leaf DER byte-equality, CN identity, negotiated
//!    protocol/cipher).
//!
//! 2. True streaming response delivery — the
//!    `enable_response_body_streaming` signal +
//!    `schedule_response_body_drain` path that the servo bridge will use
//!    for incremental body hand-off. A slow chunked server (3 chunks, 100ms
//!    apart) proves the callback fires per chunk (spread over time, bytes
//!    arriving incrementally) instead of once at the end; the same server
//!    with the signal off proves the buffered contrast (single final body
//!    delivery).
//!
//! No JS runtime is involved — this exercises the HTTP thread's real
//! socket/data path end to end.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsConnection, TlsServer, TlsState, generate_self_signed_pem};
use bun_core::MutableString;
use bun_http::signals::Store;
use bun_http::{AsyncHTTP, BunTlsInfo, FetchRedirect, HTTPClientResult, HTTPClientResultCallback,
               Method, async_http};

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
    failed: bool,
    status: Option<u32>,
    tls: Option<BunTlsInfo>,
    at: Instant,
}

struct Recorder {
    tx: mpsc::Sender<Delivery>,
    /// Streaming mode: mimic the consumer contract (servo bridge /
    /// FetchTasklet) — drain the delivered bytes out of the shared body
    /// buffer and schedule an `HTTPThread` response-body drain per chunk.
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
        // `MutableString.list` is the dual-channel facade vec (api2 mirror
        // on stable); `Delivery.bytes` is a plain std Vec. Copy from the
        // slice — one std allocation, channel-identical bytes (the previous
        // `.list.clone()` only worked on nightly, where the two Vec types
        // are the same).
        .map(|b| b.list.as_slice().to_vec())
        .unwrap_or_default();
    let status = result.metadata.as_ref().map(|m| m.response.status_code);
    let has_more = result.has_more;
    let failed = result.fail.is_some();
    let tls = result.tls_info.clone();

    if std::env::var("BAO_TEST_TRACE").is_ok() {
        eprintln!(
            "[delivery] has_more={} failed={} bytes={:?}",
            has_more,
            failed,
            String::from_utf8_lossy(&bytes)
        );
    }
    if has_more && !failed {
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
        failed,
        status,
        tls,
        at: Instant::now(),
    });

    if !has_more {
        // Terminal delivery: reclaim the caller-thread `AsyncHTTP` box via
        // the `real` backref plus the response buffer — sole dropper,
        // mirroring `on_http_done` step 5 in fetch_async.rs. The HTTP-thread
        // clone is raw-deallocated by `on_async_http_callback_raw`.
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
fn run_request(url: String, https: bool, streaming: bool) -> Vec<Delivery> {
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

// ─── Test servers ──────────────────────────────────────────────────────────

/// Plain HTTP/1.1 chunked server that drips `chunks` one TCP write at a
/// time, sleeping `delay` between chunks — the shape that forces the client
/// into per-chunk `on_data` callbacks.
fn spawn_slow_chunked_server(chunks: &[&str], delay: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let chunks: Vec<String> = chunks.iter().map(|s| s.to_string()).collect();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        read_request_head(&mut stream);
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for chunk in &chunks {
            std::thread::sleep(delay);
            resp.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            resp.extend_from_slice(chunk.as_bytes());
            resp.extend_from_slice(b"\r\n");
            stream.write_all(&resp).expect("write chunk");
            stream.flush().expect("flush chunk");
            resp.clear();
        }
        std::thread::sleep(delay / 2);
        stream.write_all(b"0\r\n\r\n").expect("write terminal");
        stream.flush().expect("flush terminal");
    });
    port
}

/// TLS stream adapter for the test server (mirror of the runtime test
/// helper in web_socket_async_tests.rs): drives the server-side BoringSSL
/// state machine over the raw TCP socket.
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

/// One-shot HTTPS server: self-signed cert (CN=localhost), serves a fixed
/// Content-Length response. Returns (port, leaf cert DER) so the test can
/// byte-compare what the client snapshotted against what the server holds.
fn spawn_tls_http_server(body: &'static str) -> (u16, Vec<u8>) {
    let (cert, key) = generate_self_signed_pem("localhost", 365).expect("self-signed cert");
    let server = std::sync::Arc::new(TlsServer::new(&cert, &key).expect("TlsServer"));
    let leaf_der = bao_boringssl_bridge::pem_parse_certs(&cert)
        .into_iter()
        .next()
        .expect("cert PEM parses to DER");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    std::thread::spawn(move || {
        let Ok((mut tcp, _)) = listener.accept() else {
            return;
        };
        let Ok(mut tls) = server.accept() else {
            return;
        };
        let piggybacked = match ServerTlsIo::handshake(&mut tcp, &mut tls) {
            Ok(p) => p,
            Err(_) => return,
        };
        let mut io = ServerTlsIo {
            tcp,
            tls,
            pending_plain: piggybacked,
            pending_off: 0,
        };
        read_request_head(&mut io);
        let _ = io.write_all(response.as_bytes());
        let _ = io.flush();
        let _ = io.tcp.shutdown(std::net::Shutdown::Write);
        // Give the client a moment to read before the TLS state is dropped.
        std::thread::sleep(Duration::from_millis(200));
    });
    (port, leaf_der)
}

fn read_request_head(stream: &mut impl Read) {
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    while got.len() < 64 * 1024 {
        let Ok(n) = stream.read(&mut buf) else {
            return;
        };
        if n == 0 {
            return;
        }
        got.extend_from_slice(&buf[..n]);
        if got.windows(4).any(|w| w == b"\r\n\r\n") {
            return;
        }
    }
}

// ─── Component 2: true streaming delivery ──────────────────────────────────

const CHUNKS: [&str; 3] = ["alpha-", "beta-", "gamma"];
const FULL_BODY: &str = "alpha-beta-gamma";
const CHUNK_DELAY: Duration = Duration::from_millis(100);

/// Wall-clock floor for the streaming-delivery timing proof: the strict
/// semantic threshold is `2 × CHUNK_DELAY`, but a zero-tolerance comparison
/// flakes under multi-process test load (scheduler shaves a few ms off each
/// server sleep; bun_bridge's copy of this test measured 287-299ms spans
/// against a 300ms line at 12-way concurrency). The 0.75× floor keeps the
/// proof meaningful — a buffered one-shot delivery collapses to ~0ms — while
/// load jitter alone can no longer pierce it. Same tolerance principle as
/// bun_bridge's `timing_floor`.
fn timing_floor(strict: Duration) -> Duration {
    strict * 3 / 4
}

/// Streaming mode: the result callback must arrive per chunk (incremental,
/// spread over the server's delays), not as one buffered final delivery.
/// Exercises `enable_response_body_streaming` → per-`on_data`
/// `process_body_buffer` → `progress_update`, plus the consumer-side
/// `schedule_response_body_drain` round-trip per chunk.
#[test]
fn streaming_mode_delivers_chunks_incrementally() {
    let port = spawn_slow_chunked_server(&CHUNKS, CHUNK_DELAY);
    let deliveries = run_request(format!("http://127.0.0.1:{}/", port), false, true);

    let terminal = deliveries
        .last()
        .expect("at least one delivery arrived");
    assert!(
        !terminal.failed,
        "request failed; deliveries: {:?}",
        deliveries.iter().map(|d| &d.bytes).collect::<Vec<_>>()
    );
    assert!(!terminal.has_more, "last delivery must be terminal");

    let first_status = deliveries[0].status;
    assert_eq!(first_status, Some(200), "headers must arrive first");

    // Every non-terminal delivery must carry has_more (stream still open).
    for d in &deliveries[..deliveries.len() - 1] {
        assert!(d.has_more, "intermediate delivery must have has_more=true");
    }

    let body_chunks: Vec<&Delivery> = deliveries
        .iter()
        .filter(|d| !d.bytes.is_empty() && d.has_more)
        .collect();
    assert!(
        body_chunks.len() >= 3,
        "expected >=3 incremental body deliveries (one per server chunk), got {}: \
         {:?}",
        body_chunks.len(),
        deliveries.iter().map(|d| d.bytes.clone()).collect::<Vec<_>>()
    );

    // Incremental proof #1 (time): first chunk delivery to last chunk
    // delivery must span at least two server inter-chunk delays; a buffered
    // one-shot delivery would collapse to ~0. `timing_floor` applies the
    // load tolerance (0.75× of the strict 2×CHUNK_DELAY = 150ms floor).
    let span = body_chunks[body_chunks.len() - 1].at - body_chunks[0].at;
    assert!(
        span >= timing_floor(CHUNK_DELAY * 2),
        "chunk deliveries must be spread over the server's delays (floor \
         {:?}, 0.75× of strict {:?}), got {:?}",
        timing_floor(CHUNK_DELAY * 2),
        CHUNK_DELAY * 2,
        span
    );

    // Incremental proof #2 (bytes): concatenated deliveries reproduce the
    // body exactly — no loss, no duplication from the drain round-trips.
    let reassembled: Vec<u8> = deliveries.iter().flat_map(|d| d.bytes.iter().copied()).collect();
    assert_eq!(
        reassembled,
        FULL_BODY.as_bytes(),
        "reassembled stream must equal the full body"
    );
}

/// Buffered contrast: the identical slow server WITHOUT the streaming
/// signal must deliver the body exactly once, at the end. Proves the
/// per-chunk deliveries above come from the streaming signal path, not from
/// incidental TCP fragmentation.
#[test]
fn buffered_mode_contrast_delivers_body_once() {
    let port = spawn_slow_chunked_server(&CHUNKS, CHUNK_DELAY);
    let deliveries = run_request(format!("http://127.0.0.1:{}/", port), false, false);

    let terminal = deliveries.last().expect("delivery arrived");
    assert!(!terminal.failed, "buffered request must succeed");
    assert!(!terminal.has_more);

    let body_chunks: Vec<&Delivery> = deliveries
        .iter()
        .filter(|d| !d.bytes.is_empty() && d.has_more)
        .collect();
    assert_eq!(
        body_chunks.len(),
        0,
        "buffered mode must not deliver intermediate body chunks"
    );
    assert_eq!(terminal.bytes, FULL_BODY.as_bytes());
}

// ─── Component 1: TLS security info ────────────────────────────────────────

/// A real TLS request must snapshot the connection's security facts onto
/// `HTTPClientResult::tls_info`, matching the server's actual
/// configuration: leaf certificate DER byte-identical to the server's cert,
/// subject/issuer CN = "localhost", TLSv1.3 with a named AEAD cipher.
#[test]
fn tls_info_snapshot_matches_server_configuration() {
    let (port, leaf_der) = spawn_tls_http_server("hello");
    let deliveries = run_request(format!("https://127.0.0.1:{}/", port), true, false);

    let terminal = deliveries.last().expect("delivery arrived");
    assert!(
        !terminal.failed,
        "TLS request must succeed (reject_unauthorized=false), got failure"
    );
    assert_eq!(terminal.status, Some(200));
    assert_eq!(terminal.bytes, b"hello");

    let tls = terminal
        .tls
        .as_ref()
        .expect("terminal delivery must carry tls_info for a TLS connection");

    // Protocol/cipher: negotiated with the bridge TlsServer (modern
    // BoringSSL on both ends ⇒ TLS 1.3 AEAD suite).
    assert_eq!(
        tls.protocol_version.as_deref(),
        Some("TLSv1.3"),
        "negotiated protocol must be TLSv1.3"
    );
    let cipher = tls.cipher_suite.as_deref().expect("cipher suite name");
    assert!(
        cipher.starts_with("TLS_") && cipher.contains("_SHA"),
        "expected an AEAD cipher suite name, got {cipher}"
    );
    let bits = tls.cipher_bits.expect("cipher strength bits");
    assert!(bits >= 128, "AEAD symmetric strength must be >= 128 bits");
    assert_eq!(
        tls.cipher_alg_bits,
        Some(bits),
        "non-export cipher: alg_bits == bits"
    );
    assert!(
        tls.cipher_version.is_some(),
        "cipher version string must be present"
    );
    assert!(
        tls.mac.is_none(),
        "AEAD suites authenticate within the cipher — no separate MAC"

    );

    // Peer certificate: the parsed leaf must identify the server's actual
    // self-signed cert, and the DER chain must be byte-identical.
    let peer = tls
        .peer_certificate
        .as_ref()
        .expect("server presented a certificate");
    let subject_cn = peer
        .subject
        .iter()
        .find(|e| e.key == "CN")
        .map(|e| e.value.as_str());
    assert_eq!(subject_cn, Some("localhost"), "subject CN must match server");
    let issuer_cn = peer
        .issuer
        .iter()
        .find(|e| e.key == "CN")
        .map(|e| e.value.as_str());
    assert_eq!(
        issuer_cn,
        Some("localhost"),
        "self-signed: issuer CN must equal subject CN"
    );
    assert!(peer.valid_from.is_some(), "valid_from must be present");
    assert!(peer.valid_to.is_some(), "valid_to must be present");
    let fp = peer.fingerprint256.as_deref().expect("fingerprint256");
    assert_eq!(
        fp.len(),
        95,
        "SHA-256 fingerprint is 32 colon-separated hex bytes"
    );
    assert!(
        fp.bytes().all(|b| b.is_ascii_hexdigit() || b == b':'),
        "fingerprint must be hex pairs"
    );
    assert!(
        peer.serial_number.as_deref().is_some_and(|s| !s.is_empty()),
        "serial number must be present"
    );

    assert!(
        !tls.peer_certificates_der.is_empty(),
        "peer chain must contain the leaf"
    );
    assert_eq!(
        tls.peer_certificates_der[0],
        leaf_der,
        "leaf DER must be byte-identical to the server's certificate"
    );

    // The bridge TlsServer selects no ALPN, so none is negotiated.
    assert!(
        tls.alpn.is_none(),
        "no ALPN expected from the plain TlsServer, got {:?}",
        tls.alpn
    );
}
