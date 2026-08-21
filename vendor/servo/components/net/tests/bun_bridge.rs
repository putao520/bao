/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! U2 page-network bridge (bun HTTPThread fetch driver) unit tests.
//!
//! These exercise `fetch_core` directly (no `FetchContext` needed) against
//! local servers:
//!   1. plain GET round trip (buffered-consumer view of the streamed body),
//!   2. `FetchRedirect::Manual` returns the original 3xx (not followed),
//!   3. abort via the Signals `aborted` atomic + HTTPThread shutdown →
//!      `NetworkError::LoadCancelled`,
//!   4. the full bun error → NetworkError mapping table,
//!   and the stage-2 semantics:
//!   5. true streaming delivery (head first, body chunks incremental — raw
//!      slow-chunked server, the 52634b89 pattern at bridge level),
//!   6. ReasonPhrase / response Version surfacing (`to_servo_response`),
//!   7. `BunTlsInfo` → `TlsHandshakeInfo` mapping (TlsSecurityInfo wiring),
//!   8. per-request CA override (right CA verifies, wrong/absent CA fails
//!      closed — servo `CACertificates::Override` semantics),
//!   9. devtools requestWillBeSent-equivalent field parity,
//!   10. SSLConfig h2 fingerprint parity (SETTINGS payload + pseudo-header
//!       order + preface PRIORITY frames + CA list in one config).
//!   11. stage-3 SSLConfig interning: content-equal per-request configs
//!       resolve to ONE registry pointer → keep-alive pool hit (one
//!       connection for two requests; the same key shape the h2 session
//!       matchers use, so this is the coalescing proof at pool level).
//!   12. response-body backpressure (fetch-side W1+W2 template at bridge
//!       level): a blasting h1 origin with a consumer that stops reading —
//!       in-flight channel bytes pin at the high-water mark (park: drain
//!       withheld + transport paused, server TCP-backpressured), then the
//!       drain resumes and every byte arrives with the accounting balanced.
//!
//! The bridge is the only page-network path (the `BAO_PAGE_NET_BUN` flag and
//! the hyper escape hatch were removed).
//! `http_loader.rs` dispatch and the servo-side behaviour are covered by the
//! rest of this test suite.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::unbounded;
use http::StatusCode;
use http::HeaderValue;
use hyper::ext::ReasonPhrase;
use net_traits::NetworkError;

use net::async_runtime::{spawn_blocking_task, spawn_task};
use net::fetch::bun_bridge::{
    BODY_HIGH_WATER_MARK, BridgeRequestBody, BunCancelHandle, BridgeError,
    build_devtools_request_msg, build_ssl_config, bun_tls_info_to_handshake, fetch_core,
    map_bun_error, to_servo_response,
};
use net::test_util::{make_body, make_server, make_ssl_server};
use net_traits::request::Destination;

/// bun's HTTP client resolves through its own resolver; pin the loopback
/// literal (the test server binds 0.0.0.0, not ::1).
fn loopback_url(url: &servo_url::ServoUrl) -> String {
    url.as_str().replace("localhost", "127.0.0.1")
}

/// Wall-clock floor for the streaming-delivery timing proofs. The strict
/// semantic threshold is `2 × inter-chunk delay` (two server delays between
/// first and last delivery), but a zero-tolerance comparison flakes under
/// multi-process test load: at 12-way concurrency the scheduler routinely
/// shaves a few ms off each ~150ms server sleep, and measured spans of
/// 287-299ms against the 300ms line failed ~31-89% of runs (load-dependent).
/// The 0.75× floor keeps the proof meaningful — a buffered one-shot delivery
/// collapses to ~0ms, hundreds of ms below the floor — while load jitter
/// alone can no longer pierce it. Same tolerance principle as
/// tls_info_and_streaming_tests' streaming_mode_delivers_chunks_incrementally
/// (150ms floor for a 200ms strict threshold).
fn timing_floor(strict: Duration) -> Duration {
    strict * 3 / 4
}

/// The connector's wire struct is the bridge's input type; build it from the
/// same bao_stealth derivation the embedder (runtime_bridge) uses. Shared by
/// the fingerprint-parity and intern-coalescing tests.
fn servo_wire(profile: &bao_stealth::StealthProfile) -> net::connector::StealthTlsWireConfig {
    let stc = bao_stealth::StealthTlsWireConfig::from_profile(profile);
    net::connector::StealthTlsWireConfig {
        tls12_cipher_suites: stc.tls12_cipher_suites,
        tls13_cipher_suites: stc.tls13_cipher_suites,
        signature_algorithms: stc.signature_algorithms,
        supported_groups: stc.supported_groups,
        alpn_protocols: stc.alpn_protocols,
        h2_settings_payload: stc.h2_settings_payload,
        h2_initial_stream_size: stc.h2_initial_stream_size,
        h2_initial_connection_window_size: stc.h2_initial_connection_window_size,
        h2_max_frame_size: stc.h2_max_frame_size,
        h2_max_header_list_size: stc.h2_max_header_list_size,
    }
}

/// Test-binary bootstrap for the bun HTTP seam — the same leg bun_http's own
/// tests use: native archives (uws loop, boringssl, lsquic) + the Output
/// sinks the HTTP thread's on_start asserts were initialized.
fn link_native_seam() {
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
}

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by `bun_runtime::dispatch` in product
// binaries (bun_runtime is higher-tier than servo-net and cannot be
// dev-depped from here). No FilePoll sources are registered anywhere in
// these tests, so a no-op satisfies the link-time reference — the same seam
// bun_http's own test binary uses.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

#[test]
fn bridge_get_roundtrip() {
    link_native_seam();
    let (server, url) = make_server(|_request, response| {
        response.headers_mut().insert(
            "content-type",
            HeaderValue::from_static("text/plain"),
        );
        response.headers_mut().insert(
            "x-bao-bridge",
            HeaderValue::from_static("yes"),
        );
        *response.body_mut() = make_body(b"hello from bun bridge".to_vec());
    });

    let url = loopback_url(&url);
    let cancel = BunCancelHandle::new();
    let outcome = spawn_blocking_task::<_, Result<Vec<u8>, BridgeError>>(async {
        let mut response = fetch_core(
            bun_http::Method::GET,
            None,
            url,
            vec![(b"accept".to_vec(), b"*/*".to_vec())],
            BridgeRequestBody::Empty,
            &cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        )
        .await
        .expect("GET through the bun bridge must succeed");
        assert_eq!(response.status_code, 200);
        let header = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(b"x-bao-bridge"))
            .expect("custom response header must pass through");
        assert_eq!(header.1, b"yes");
        response.collect_body().await
    });

    assert_eq!(outcome.expect("streamed body must reassemble"), b"hello from bun bridge");

    let _ = server.close();
}

#[test]
fn bridge_manual_redirect_returns_original_3xx() {
    link_native_seam();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);
    let (server, url) = make_server(move |_request, response| {
        hits_clone.fetch_add(1, Ordering::SeqCst);
        *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
        response.headers_mut().insert(
            "location",
            HeaderValue::from_static("/final-destination"),
        );
        *response.body_mut() = make_body(b"moved".to_vec());
    });

    let url = loopback_url(&url);
    let cancel = BunCancelHandle::new();
    let outcome = spawn_blocking_task::<_, Result<(u16, Vec<u8>), BridgeError>>(async {
        let mut response = fetch_core(
            bun_http::Method::GET,
            None,
            url,
            Vec::new(),
            BridgeRequestBody::Empty,
            &cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        )
        .await
        .expect("Manual mode must return the 3xx itself");
        let location = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(b"location"))
            .expect("Location header must be surfaced to servo's redirect loop")
            .1
            .clone();
        assert_eq!(location, b"/final-destination");
        let body: Vec<u8> = response.collect_body().await?;
        Ok::<(u16, Vec<u8>), BridgeError>((response.status_code, body))
    });

    let (status, body) = outcome.expect("3xx exchange must succeed");
    assert_eq!(status, 301);
    assert_eq!(body, b"moved");
    // Exactly one request — bun must NOT have followed the redirect.
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let _ = server.close();
}

#[test]
fn bridge_abort_yields_load_cancelled() {
    link_native_seam();
    let (server, url) = make_server(|_request, _response| {
        // Hold the response long enough for the abort to land mid-flight.
        thread::sleep(Duration::from_millis(1000));
    });

    let url = loopback_url(&url);
    let cancel = Arc::new(BunCancelHandle::new());
    let worker_cancel = Arc::clone(&cancel);

    let (sender, receiver) = unbounded();
    spawn_task(async move {
        let outcome = fetch_core(
            bun_http::Method::GET,
            None,
            url,
            Vec::new(),
            BridgeRequestBody::Empty,
            &worker_cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        )
        .await;
        let _ = sender.send(outcome);
    });

    // Let the request reach the server, then abort through the Signals
    // aborted atomic + HTTPThread shutdown path.
    thread::sleep(Duration::from_millis(150));
    cancel.abort();

    let outcome = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("abort must still produce a terminal callback");
    assert_eq!(
        outcome.err(),
        Some(BridgeError::Network(NetworkError::LoadCancelled))
    );

    let _ = server.close();
}

/// Every row of the mapping table plus the default arm. `bun_core::Error`
/// identity is the interned name, so interning the exact wire names bun's
/// HTTP client produces exercises the real dispatch.
#[test]
fn bridge_error_mapping_table() {
    let cases: Vec<(&str, BridgeError)> = vec![
        // Abort family → cancelled fetch.
        ("Aborted", BridgeError::Network(NetworkError::LoadCancelled)),
        ("AbortedBeforeConnecting", BridgeError::Network(NetworkError::LoadCancelled)),
        ("ClientAborted", BridgeError::Network(NetworkError::LoadCancelled)),
        // Connect-phase transport failures.
        ("ConnectionRefused", BridgeError::Network(NetworkError::ConnectionFailure)),
        ("ECONNREFUSED", BridgeError::Network(NetworkError::ConnectionFailure)),
        ("FailedToOpenSocket", BridgeError::Network(NetworkError::ConnectionFailure)),
        // Mid-response transport failures.
        ("ConnectionClosed", BridgeError::Network(NetworkError::ConnectionFailure)),
        ("ECONNRESET", BridgeError::Network(NetworkError::ConnectionFailure)),
        ("EPIPE", BridgeError::Network(NetworkError::ConnectionFailure)),
        ("ECONNABORTED", BridgeError::Network(NetworkError::ConnectionFailure)),
        // DNS: hyper-era parity — message travels inside HttpError.
        ("DNSResolutionFailed", BridgeError::Network(NetworkError::HttpError("DNSResolutionFailed".into()))),
        ("EAI_AGAIN", BridgeError::Network(NetworkError::HttpError("EAI_AGAIN".into()))),
        // Timeouts: hyper-era default client had no total timeout.
        ("Timeout", BridgeError::Network(NetworkError::HttpError("Timeout".into()))),
        ("ETIMEDOUT", BridgeError::Network(NetworkError::HttpError("ETIMEDOUT".into()))),
        // Certificate verification failure → refined by the override manager
        // in the servo wrapper. Stage 2: the whole BoringSSL X509
        // verify-failure family (get_cert_error_from_no tags) maps here, not
        // just the altname error — wrong-CA / expired / self-signed chains
        // are certificate failures, exactly as from_hyper_error classified
        // them via the override manager's failing-verification certificate.
        ("ERR_TLS_CERT_ALTNAME_INVALID", BridgeError::CertificateFailure("ERR_TLS_CERT_ALTNAME_INVALID".into())),
        ("UNABLE_TO_GET_ISSUER_CERT_LOCALLY", BridgeError::CertificateFailure("UNABLE_TO_GET_ISSUER_CERT_LOCALLY".into())),
        ("DEPTH_ZERO_SELF_SIGNED_CERT", BridgeError::CertificateFailure("DEPTH_ZERO_SELF_SIGNED_CERT".into())),
        ("CERT_HAS_EXPIRED", BridgeError::CertificateFailure("CERT_HAS_EXPIRED".into())),
        ("INVALID_CA", BridgeError::CertificateFailure("INVALID_CA".into())),
        ("HOSTNAME_MISMATCH", BridgeError::CertificateFailure("HOSTNAME_MISMATCH".into())),
        // Redirects (only reachable outside Manual mode; table stays total).
        ("TooManyRedirects", BridgeError::Network(NetworkError::TooManyRedirects)),
        ("RedirectURLTooLong", BridgeError::Network(NetworkError::RedirectError)),
        ("RedirectURLInvalid", BridgeError::Network(NetworkError::RedirectError)),
        ("InvalidRedirectURL", BridgeError::Network(NetworkError::RedirectError)),
        ("UnsupportedRedirectProtocol", BridgeError::Network(NetworkError::RedirectError)),
        ("UnexpectedRedirect", BridgeError::Network(NetworkError::RedirectError)),
        // Method / decompression / memory.
        ("InvalidMethod", BridgeError::Network(NetworkError::InvalidMethod)),
        ("DecompressionNotImplemented", BridgeError::Network(NetworkError::DecompressionError)),
        ("OutOfMemory", BridgeError::Network(NetworkError::Crash("OutOfMemory".into()))),
        // Default arm: HTTP/2 & HTTP/3 protocol-error families and anything
        // else keep the hyper-era shape (message inside HttpError).
        ("HTTP2ProtocolError", BridgeError::Network(NetworkError::HttpError("HTTP2ProtocolError".into()))),
        ("HTTP2FrameSizeError", BridgeError::Network(NetworkError::HttpError("HTTP2FrameSizeError".into()))),
        ("HTTP3Unsupported", BridgeError::Network(NetworkError::HttpError("HTTP3Unsupported".into()))),
        ("InvalidHTTPResponse", BridgeError::Network(NetworkError::HttpError("InvalidHTTPResponse".into()))),
        ("SomethingEntirelyNew", BridgeError::Network(NetworkError::HttpError("SomethingEntirelyNew".into()))),
    ];

    for (name, expected) in cases {
        assert_eq!(map_bun_error(bun_core::Error::intern(name)), expected);
    }
}


// ──────────────────────────────────────────────────────────────────────────
// Stage 2: true streaming, ReasonPhrase/version, TLS info, CA override,
// devtools parity, h2 fingerprint parity
// ──────────────────────────────────────────────────────────────────────────

/// Raw-TCP h1 fixture helpers (full control over the status line / chunk
/// timing — the hyper `make_server` can't drip chunks or forge phrases).
fn read_request_head(stream: &mut TcpStream) {
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    while got.len() < 64 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                got.extend_from_slice(&buf[..n]);
                if got.windows(4).any(|w| w == b"\r\n\r\n") {
                    return;
                }
            },
            Err(_) => return,
        }
    }
}

/// Plain HTTP/1.1 chunked server that drips `chunks` one TCP write at a
/// time, sleeping `delay` between chunks (the 52634b89 slow-chunked shape).
fn spawn_slow_chunked_server(chunks: &[&str], delay: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let chunks: Vec<String> = chunks.iter().map(|s| s.to_string()).collect();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        read_request_head(&mut stream);
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for chunk in &chunks {
            thread::sleep(delay);
            resp.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            resp.extend_from_slice(chunk.as_bytes());
            resp.extend_from_slice(b"\r\n");
            stream.write_all(&resp).expect("write chunk");
            stream.flush().expect("flush chunk");
            resp.clear();
        }
        thread::sleep(delay / 2);
        stream.write_all(b"0\r\n\r\n").expect("write terminal");
        stream.flush().expect("flush terminal");
    });
    port
}

/// One-shot plain HTTP/1.1 server that replies with the exact raw status
/// line and headers given (no body framing beyond what the caller writes).
fn spawn_raw_status_server(status_line: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        read_request_head(&mut stream);
        let body = b"ok";
        let resp = format!(
            "{status_line}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(resp.as_bytes()).expect("write head");
        stream.write_all(body).expect("write body");
        stream.flush().expect("flush");
    });
    port
}

/// True streaming (stage 2, item 4): against a slow chunked server the
/// response HEAD must arrive while the body is still dripping, and the body
/// chunks must arrive incrementally (spread over the server's delays), not
/// as one buffered final delivery — the bridge-level port of
/// tls_info_and_streaming_tests' streaming_mode_delivers_chunks_incrementally.
#[test]
fn bridge_streaming_delivery_incremental() {
    link_native_seam();
    // Initialize the async runtime (spawn_task silently drops tasks without
    // it; the raw-TCP fixture below doesn't lazily init like make_server).
    let (_runtime_server, _runtime_url) = make_server(|_request, _response| {});
    const CHUNKS: [&str; 3] = ["alpha-", "beta-", "gamma"];
    const FULL_BODY: &[u8] = b"alpha-beta-gamma";
    const DELAY: Duration = Duration::from_millis(150);
    let port = spawn_slow_chunked_server(&CHUNKS, DELAY);

    let cancel = Arc::new(BunCancelHandle::new());
    let worker_cancel = Arc::clone(&cancel);
    let (sender, receiver) = unbounded::<(Instant, Vec<(Instant, Vec<u8>)>)>();
    spawn_task(async move {
        let mut response = fetch_core(
            bun_http::Method::GET,
            None,
            format!("http://127.0.0.1:{}/", port),
            Vec::new(),
            BridgeRequestBody::Empty,
            &worker_cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        )
        .await
        .expect("streaming request must succeed");
        assert_eq!(response.status_code, 200, "headers must arrive first");
        let head_at = Instant::now();
        let mut chunks = Vec::new();
        while let Some(frame) = response.next_chunk().await {
            chunks.push((Instant::now(), frame.expect("streaming frame must be Ok")));
        }
        let _ = sender.send((head_at, chunks));
    });

    let (head_at, chunks) = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("streaming exchange must complete");

    let reassembled: Vec<u8> = chunks.iter().map(|(_, c)| c.as_slice()).collect::<Vec<_>>().concat();
    assert_eq!(reassembled, FULL_BODY, "reassembled stream must equal the body");
    assert!(
        chunks.len() >= 3,
        "expected >=3 incremental body deliveries (one per server chunk), got {}: {:?}",
        chunks.len(),
        chunks.iter().map(|(_, c)| c.clone()).collect::<Vec<_>>()
    );
    // Incremental proof #1 (time): first chunk delivery to last chunk
    // delivery must span at least two server inter-chunk delays — a buffered
    // one-shot delivery would collapse to ~0. `timing_floor` applies the
    // load tolerance (0.75×) so scheduler jitter can't pierce the line.
    let span = chunks[chunks.len() - 1].0 - chunks[0].0;
    assert!(
        span >= timing_floor(DELAY * 2),
        "chunk deliveries must be spread over the server's delays (floor \
         {:?}, 0.75× of strict {:?}), got {span:?}",
        timing_floor(DELAY * 2),
        DELAY * 2
    );
    // Incremental proof #2 (head-first): the head was published before the
    // last chunk arrived — the server still owed ≥ 2 delays of body when
    // the future resolved. Same `timing_floor` tolerance as proof #1.
    let head_lead = chunks[chunks.len() - 1].0 - head_at;
    assert!(
        head_lead >= timing_floor(DELAY * 2),
        "head must resolve well before the terminal chunk (floor {:?}, 0.75× \
         of strict {:?}), lead was {head_lead:?}",
        timing_floor(DELAY * 2),
        DELAY * 2
    );
}

/// Response-body backpressure (fetch-side W1+W2 template at bridge level):
/// against an h1 origin blasting a body far larger than the high-water
/// mark, a consumer that stops reading must pin the channel's in-flight
/// bytes AT the mark (the delivery callback parks: drain round-trip
/// withheld + transport read side paused, so the server itself is
/// TCP-backpressured) — not swallow the whole body. Resuming the drain
/// must then deliver every byte with the in-flight accounting balanced
/// back to zero.
///
/// The bound assertion allows one delivery increment of slack past the
/// mark (the park engages on the delivery that crosses it); the parked
/// stability assertion (two samples while the consumer is silent) proves
/// deliveries actually stopped, i.e. the transport pause + withheld drain
/// hold, and rules out an unbounded channel just lagging behind.
#[test]
fn bridge_body_backpressure_bounded_and_resumes() {
    link_native_seam();
    let (_runtime_server, _runtime_url) = make_server(|_request, _response| {});

    const BODY_LEN: usize = 4 * 1024 * 1024;
    const SLICE: usize = 16 * 1024;
    let full_body: Vec<u8> = (0..BODY_LEN).map(|i| (i % 251) as u8).collect();

    // h1 origin that blasts Content-Length-framed body in 16 KiB slices,
    // blocking in write once the parked client's TCP window closes.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server_body = full_body.clone();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        read_request_head(&mut stream);
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {BODY_LEN}\r\n\r\n");
        stream.write_all(head.as_bytes()).expect("write head");
        for slice in server_body.chunks(SLICE) {
            stream.write_all(slice).expect("write slice");
        }
        stream.flush().expect("flush");
    });

    let cancel = Arc::new(BunCancelHandle::new());
    let worker_cancel = Arc::clone(&cancel);
    let expected_body = full_body.clone();
    let (sender, receiver) = unbounded::<(usize, usize, usize, Vec<u8>)>();
    spawn_task(async move {
        let mut response = fetch_core(
            bun_http::Method::GET,
            None,
            format!("http://127.0.0.1:{}/", port),
            Vec::new(),
            BridgeRequestBody::Empty,
            &worker_cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        )
        .await
        .expect("blasting request must deliver its head");
        assert_eq!(response.status_code, 200);

        // Park window: the consumer goes silent. The callback crosses the
        // high-water mark and parks; in-flight bytes pin at the mark.
        thread::sleep(Duration::from_millis(700));
        let parked_sample_1 = response.body_in_flight();
        thread::sleep(Duration::from_millis(250));
        let parked_sample_2 = response.body_in_flight();

        // Resume: drain the whole body through the accounting receiver.
        let mut received = Vec::with_capacity(BODY_LEN);
        while let Some(frame) = response.next_chunk().await {
            received.extend_from_slice(&frame.expect("resumed frame must be Ok"));
        }
        assert_eq!(received, expected_body, "resumed drain must be lossless");
        let end_in_flight = response.body_in_flight();
        let _ = sender.send((parked_sample_1, parked_sample_2, end_in_flight, received));
    });

    let (parked_sample_1, parked_sample_2, end_in_flight, _received) = receiver
        .recv_timeout(Duration::from_secs(20))
        .expect("backpressure exchange must complete without deadlock");

    // Bounded: parked at the mark (+ at most one delivery increment of
    // slack for the crossing delivery), nowhere near the 4 MiB body an
    // unbounded channel would swallow.
    assert!(
        parked_sample_1 >= BODY_HIGH_WATER_MARK,
        "park must have engaged at/above the high-water mark, got {parked_sample_1}"
    );
    assert!(
        parked_sample_1 < BODY_HIGH_WATER_MARK + 256 * 1024,
        "parked in-flight bytes must stay within one increment of the mark \
         ({} + slack), got {parked_sample_1}",
        BODY_HIGH_WATER_MARK
    );
    // Parked stability: a silent consumer sees deliveries STOP — the exact
    // same in-flight reading twice proves the transport pause + withheld
    // drain hold (an unbounded channel would keep growing under the blast).
    assert_eq!(
        parked_sample_1, parked_sample_2,
        "in-flight bytes must be stable while the consumer is silent (parked)"
    );
    // Balanced accounting after the lossless resume.
    assert_eq!(
        end_in_flight, 0,
        "in-flight accounting must balance to zero after draining"
    );
}

/// ReasonPhrase + Version surfacing (stage 2, item 5): a non-canonical
/// status phrase must ride the response as `hyper::ext::ReasonPhrase`
/// (owned bytes — no &'static leak), and a canonical one must NOT set the
/// extension (hyper's own client condition). h1 exchanges surface
/// `Version::HTTP_11`; the h2 flag case needs ALPN and is covered by the
/// e2e matrix.
#[test]
fn bridge_reason_phrase_and_version() {
    link_native_seam();
    // Initialize the async runtime (spawn_blocking_task needs the runtime
    // handle; the raw-TCP fixture below doesn't lazily init like make_server).
    let (_runtime_server, _runtime_url) = make_server(|_request, _response| {});
    // Non-canonical phrase.
    let port = spawn_raw_status_server("HTTP/1.1 200 Sure Thing");
    let cancel = BunCancelHandle::new();
    let response = spawn_blocking_task::<_, Result<(), BridgeError>>(async {
        let response = fetch_core(
            bun_http::Method::GET,
            None,
            format!("http://127.0.0.1:{}/", port),
            Vec::new(),
            BridgeRequestBody::Empty,
            &cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        )
        .await?
        ;
        let phrase = response.status_text.clone();
        let servo_response =
            to_servo_response(response, false).expect("servo response conversion");
        let ext = servo_response
            .extensions()
            .get::<ReasonPhrase>()
            .map(|p| p.as_bytes().to_vec());
        assert_eq!(ext.as_deref(), Some(b"Sure Thing".as_slice()));
        assert_eq!(phrase, b"Sure Thing");
        assert_eq!(
            servo_response.version(),
            http::Version::HTTP_11,
            "h1 exchange surfaces HTTP/1.1"
        );
        Ok::<(), BridgeError>(())
    });
    response.expect("non-canonical phrase exchange must succeed");

    // Canonical phrase: no extension (hyper client parity).
    let port = spawn_raw_status_server("HTTP/1.1 200 OK");
    let cancel = BunCancelHandle::new();
    let response = spawn_blocking_task::<_, Result<(), BridgeError>>(async {
        let response = fetch_core(
            bun_http::Method::GET,
            None,
            format!("http://127.0.0.1:{}/", port),
            Vec::new(),
            BridgeRequestBody::Empty,
            &cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        )
        .await?;
        let servo_response =
            to_servo_response(response, false).expect("servo response conversion");
        assert!(
            servo_response.extensions().get::<ReasonPhrase>().is_none(),
            "canonical phrase must not set the ReasonPhrase extension"
        );
        Ok::<(), BridgeError>(())
    });
    response.expect("canonical phrase exchange must succeed");
}

/// `BunTlsInfo` → `TlsHandshakeInfo` mapping (stage 2, item 2): field-for-field
/// pass-through with the connector's documented BoringSSL limitations
/// (`kea_group_name` / `signature_scheme_name` stay `None` — no public API),
/// ALPN bytes → String, chain DER copied, ECH off.
#[test]
fn bridge_tls_info_to_handshake_mapping() {
    let info = bun_http::BunTlsInfo {
        protocol_version: Some("TLSv1.3".into()),
        cipher_suite: Some("TLS_AES_256_GCM_SHA384".into()),
        cipher_version: Some("TLSv1/SSLv3".into()),
        cipher_bits: Some(256),
        cipher_alg_bits: Some(256),
        // AEAD: authentication is integral to the cipher — the separate-MAC
        // field is None by design and TlsHandshakeInfo has no mac field.
        mac: None,
        alpn: Some(b"h2".to_vec()),
        peer_certificate: None,
        peer_certificates_der: vec![vec![0x30, 0x03, 0x02, 0x01, 0x01]],
    };
    let handshake = bun_tls_info_to_handshake(&info);
    assert_eq!(handshake.protocol_version.as_deref(), Some("TLSv1.3"));
    assert_eq!(
        handshake.cipher_suite.as_deref(),
        Some("TLS_AES_256_GCM_SHA384")
    );
    assert_eq!(handshake.kea_group_name, None, "connector.rs parity: no BoringSSL API");
    assert_eq!(
        handshake.signature_scheme_name, None,
        "connector.rs parity: no BoringSSL API"
    );
    assert_eq!(handshake.alpn_protocol.as_deref(), Some("h2"));
    assert_eq!(
        handshake.certificate_chain_der,
        vec![vec![0x30, 0x03, 0x02, 0x01, 0x01]]
    );
    assert!(!handshake.used_ech);
}

/// Per-request CA override (stage 2, item 3), against the rustls
/// self-signed test server (CN=localhost, no SAN):
/// - `ca_override = [server cert DER]` + reject_unauthorized=true → verifies
///   (Override semantics: the listed cert IS the trust list) and the head
///   carries the connection's `BunTlsInfo` (leaf DER byte-equality with the
///   server's certificate — the TLS snapshot rides the bridge result).
/// - `ca_override = []` (Override with an empty/unrelated list) +
///   reject_unauthorized=true → fails closed as a certificate failure
///   (the store replaces the system roots; nothing trusts a self-signed
///   leaf) — the class servo refines via the override manager.
/// - no override (`Default`) + reject_unauthorized=true against the same
///   self-signed server → also a certificate failure (not in system roots).
/// One self-signed rustls server + its https URL (host kept as `localhost` —
/// the certificate's CN) + its DER certificate list. Each probe gets its OWN
/// server: bun pools the first probe's keep-alive socket, which would park
/// the shared rustls server inside hyper's keep-alive read loop and starve
/// the next probe's TCP connect into the 300s idle timeout.
fn spawn_ca_probe_server() -> (u16, Vec<Vec<u8>>) {
    let (server, mut url) = make_ssl_server(|_request, response| {
        *response.body_mut() = make_body(b"secure hello".to_vec());
    });
    url.as_mut_url().set_scheme("https").unwrap();
    assert!(url.as_str().starts_with("https://localhost:"));
    let port = url.as_url().port().expect("test server binds an explicit port");
    let certificates = server
        .certificates
        .as_ref()
        .expect("make_ssl_server must expose its certificate list")
        .iter()
        .map(|cert| cert.as_ref().to_vec())
        .collect::<Vec<Vec<u8>>>();
    assert!(!certificates.is_empty(), "test server certificate available");
    // Keep the server alive for the whole probe (dropping `Server` closes it).
    std::mem::forget(server);
    (port, certificates)
}

#[test]
fn bridge_ca_override_trust_store() {
    link_native_seam();
    let (port, certificates) = spawn_ca_probe_server();
    let url = format!("https://localhost:{}/", port);

    // Right CA: verifies; TLS snapshot rides the head.
    let ca = certificates.clone();
    let leaf_der = certificates[0].clone();
    let cancel = BunCancelHandle::new();
    let probe_url = url.clone();
    let outcome = spawn_blocking_task::<_, Result<(), BridgeError>>(async {
        let ssl = net::fetch::bun_bridge::build_ssl_config(None, None, Some(&ca));
        let tls_props = Some(bun_http::ssl_config::SharedPtr::new(ssl));
        let mut response = fetch_core(
            bun_http::Method::GET,
            None,
            probe_url,
            Vec::new(),
            BridgeRequestBody::Empty,
            &cancel,
            None::<fn() -> bool>,
            tls_props,
            false,
            true,
        )
        .await?;
        assert_eq!(response.status_code, 200);
        let body = response.collect_body().await?;
        assert_eq!(body, b"secure hello");
        let tls = response
            .tls_info
            .as_ref()
            .expect("TLS exchange must carry a BunTlsInfo snapshot");
        assert_eq!(
            tls.peer_certificates_der.first(),
            Some(&leaf_der),
            "leaf DER must be byte-identical to the server's certificate"
        );
        assert!(tls.protocol_version.is_some(), "negotiated protocol recorded");
        Ok::<(), BridgeError>(())
    });
    outcome.expect("right CA override must verify the self-signed server");

    // Wrong (empty) CA list: fails closed as a certificate failure. Own
    // server: the right-CA probe's socket is pooled (keep-alive), which
    // parks the shared rustls server in its h1 keep-alive read loop.
    let (port, _certs) = spawn_ca_probe_server();
    let cancel = BunCancelHandle::new();
    let probe_url = format!("https://localhost:{}/", port);
    let empty_ca: Vec<Vec<u8>> = Vec::new();
    let outcome = spawn_blocking_task::<_, Result<(), BridgeError>>(async {
        let ssl = net::fetch::bun_bridge::build_ssl_config(None, None, Some(&empty_ca));
        let tls_props = Some(bun_http::ssl_config::SharedPtr::new(ssl));
        fetch_core(
            bun_http::Method::GET,
            None,
            probe_url,
            Vec::new(),
            BridgeRequestBody::Empty,
            &cancel,
            None::<fn() -> bool>,
            tls_props,
            false,
            true,
        )
        .await
        .map(|_| ())
    });
    match outcome.expect_err("empty Override must fail closed") {
        BridgeError::CertificateFailure(message) => {
            assert!(!message.is_empty(), "certificate failure carries its message");
        },
        other => panic!("expected CertificateFailure, got {other:?}"),
    }

    // Default roots: same self-signed server is equally untrusted. Own
    // server again (same pooling reason).
    let (port, _certs) = spawn_ca_probe_server();
    let cancel = BunCancelHandle::new();
    let outcome = spawn_blocking_task::<_, Result<(), BridgeError>>(async {
        fetch_core(
            bun_http::Method::GET,
            None,
            format!("https://localhost:{}/", port),
            Vec::new(),
            BridgeRequestBody::Empty,
            &cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        )
        .await
        .map(|_| ())
    });
    match outcome.expect_err("self-signed server must fail against system roots") {
        BridgeError::CertificateFailure(_) => (),
        other => panic!("expected CertificateFailure, got {other:?}"),
    }
}

/// Devtools requestWillBeSent-equivalent (stage 2, item 1): the message is
/// built with hyper-path field parity, gated on the same triple-Option
/// (request_id / pipeline_id / browsing_context_id).
#[test]
fn bridge_devtools_msg_field_parity() {
    use devtools_traits::NetworkEvent;
    use http::Method;
    use servo_base::id::{TEST_BROWSING_CONTEXT_ID, TEST_PIPELINE_ID};
    let mut headers = http::HeaderMap::new();
    headers.insert("accept", HeaderValue::from_static("*/*"));
    let msg = build_devtools_request_msg(
        Some("req-42"),
        &servo_url::ServoUrl::parse("http://example.org/x").unwrap(),
        &Method::GET,
        &headers,
        b"payload".to_vec(),
        Some(TEST_PIPELINE_ID),
        Duration::from_millis(12),
        Duration::from_millis(34),
        Destination::Image,
        true,
        Some(TEST_BROWSING_CONTEXT_ID),
    )
    .expect("complete ids must produce the devtools message");
    let devtools_traits::ChromeToDevtoolsControlMsg::NetworkEvent(
        request_id,
        NetworkEvent::HttpRequestUpdate(request),
    ) = msg
    else {
        panic!("expected NetworkEvent::HttpRequestUpdate");
    };
    assert_eq!(request_id, "req-42");
    assert_eq!(request.url.as_str(), "http://example.org/x");
    assert_eq!(request.method, Method::GET);
    assert_eq!(request.headers, headers);
    assert_eq!(request.body.as_deref(), Some(&b"payload".to_vec()));
    assert_eq!(request.pipeline_id, TEST_PIPELINE_ID);
    assert_eq!(request.connect_time, Duration::from_millis(12));
    assert_eq!(request.send_time, Duration::from_millis(34));
    assert_eq!(request.destination, Destination::Image);
    assert!(request.is_xhr);
    assert_eq!(request.browsing_context_id, TEST_BROWSING_CONTEXT_ID);

    // Triple-Option gate: any missing id → None (hyper-path gate).
    assert!(build_devtools_request_msg(
        None,
        &servo_url::ServoUrl::parse("http://example.org/x").unwrap(),
        &Method::GET,
        &headers,
        Vec::new(),
        Some(TEST_PIPELINE_ID),
        Duration::ZERO,
        Duration::ZERO,
        Destination::Image,
        false,
        Some(TEST_BROWSING_CONTEXT_ID),
    )
    .is_none());
    assert!(build_devtools_request_msg(
        Some("req-42"),
        &servo_url::ServoUrl::parse("http://example.org/x").unwrap(),
        &Method::GET,
        &headers,
        Vec::new(),
        None,
        Duration::ZERO,
        Duration::ZERO,
        Destination::Image,
        false,
        Some(TEST_BROWSING_CONTEXT_ID),
    )
    .is_none());
    assert!(build_devtools_request_msg(
        Some("req-42"),
        &servo_url::ServoUrl::parse("http://example.org/x").unwrap(),
        &Method::GET,
        &headers,
        Vec::new(),
        Some(TEST_PIPELINE_ID),
        Duration::ZERO,
        Duration::ZERO,
        Destination::Image,
        false,
        None,
    )
    .is_none());
}

/// SSLConfig parity (stage 2, items 3 + 6): from the stealth wire config +
/// the profile's Http2Fingerprint snapshot + a CA override, the built config
/// carries the exact SETTINGS payload (byte-equal to the profile's wire
/// bytes), the profile's pseudo-header wire order and preface PRIORITY
/// frames (REQ-STL-002 / REQ-STL-002-C3), and the DER trust list — the
/// inputs `h2_client::encode::write_preface` / `encode_request_headers`
/// and `configure_http_client_with_alpn` consume.
#[test]
fn bridge_ssl_config_h2_fingerprint_and_ca() {
    // The connector's wire struct is the bridge's input type; build it from
    // the same bao_stealth derivation the embedder (runtime_bridge) uses.
    let profile = bao_stealth::StealthProfile::firefox_default();
    let wire = bao_stealth::StealthTlsWireConfig::from_profile(&profile);
    let ca: Vec<Vec<u8>> = vec![vec![0x30, 0x00]];
    let config = build_ssl_config(
        Some(&servo_wire(&profile)),
        Some(&profile.http2),
        Some(&ca),
    );
    // h2 SETTINGS payload: byte-equal to the wire config's (which
    // stealth_wire derives from the same profile).
    assert_eq!(
        config.h2_settings_payload.as_deref(),
        Some(wire.h2_settings_payload.as_slice()),
        "bridge h2 SETTINGS payload must equal the profile's"
    );
    assert_eq!(config.h2_initial_window_size, wire.h2_initial_stream_size);
    // Pseudo-header wire order: Firefox's method/path/authority/scheme.
    let order = config
        .h2_pseudo_header_order
        .as_ref()
        .expect("fingerprint snapshot must set the pseudo-header order");
    let names: Vec<&str> = order.iter().map(|s| &**s).collect();
    assert_eq!(names, vec![":method", ":path", ":authority", ":scheme"]);
    // Preface PRIORITY frames: Firefox reserves streams 3/5/7/11.
    let frames = config
        .h2_priority_frames
        .as_ref()
        .expect("fingerprint snapshot must set the preface PRIORITY frames");
    let ids: Vec<u32> = frames.iter().map(|f| f.stream_id).collect();
    assert_eq!(ids, vec![3, 5, 7, 11]);
    // CA override rides the config.
    assert_eq!(
        config.ca_certs_der.as_deref(),
        Some(
            ca.iter()
                .map(|der| der.as_slice().into())
                .collect::<Vec<Box<[u8]>>>()
                .as_slice()
        )
    );

    // No fingerprint / no CA: defaults stay clean (None), no phantom config.
    let bare = build_ssl_config(None, None, None);
    assert!(bare.h2_settings_payload.is_none());
    assert!(bare.h2_pseudo_header_order.is_none());
    assert!(bare.h2_priority_frames.is_none());
    assert!(bare.ca_certs_der.is_none());

    // Chrome snapshot: pseudo-header order flips, PRIORITY frames drop.
    let chrome = bao_stealth::StealthProfile::chrome_default();
    let config = build_ssl_config(Some(&servo_wire(&chrome)), Some(&chrome.http2), None);
    let order = config.h2_pseudo_header_order.as_ref().unwrap();
    let names: Vec<&str> = order.iter().map(|s| &**s).collect();
    assert_eq!(names, vec![":method", ":authority", ":scheme", ":path"]);
    assert!(
        config.h2_priority_frames.as_ref().is_some_and(|f| f.is_empty()),
        "Chrome v106+ sends no PRIORITY frames"
    );
}

/// True streaming on a KEEP-ALIVE Content-Length exchange (stage 2): the
/// terminal delivery must fire from the byte count alone — no connection
/// close — and a SECOND request must reuse the pooled socket and complete
/// the same way (the page-level posture: subresource bursts on one pool).
#[test]
fn bridge_streaming_keepalive_content_length_and_reuse() {
    use std::io::{Read, Write};
    link_native_seam();
    // Initialize the async runtime (spawn_task silently drops tasks without
    // it; the raw-TCP fixture below doesn't lazily init like make_server).
    let (_runtime_server, _runtime_url) = make_server(|_request, _response| {});
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        // Keep-alive server: serves TWO sequential requests on one connection.
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        for request_no in 0..2 {
            let mut got = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 { return; }
                got.extend_from_slice(&buf[..n]);
                if got.windows(4).any(|w| w == b"\r\n\r\n") { break; }
            }
            let body = format!("hello-cl-{}", request_no);
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            stream.write_all(resp.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
        // KEEP the connection open — do NOT close for 8s.
        std::thread::sleep(std::time::Duration::from_secs(8));
    });

    let (sender, receiver) = unbounded::<Result<(Instant, Vec<Vec<u8>>), String>>();
    let cancel = Arc::new(BunCancelHandle::new());
    let worker_cancel = Arc::clone(&cancel);
    let port_for_task = port;
    spawn_task(async move {
        let url = format!("http://127.0.0.1:{}/", port_for_task);
        let mut response = match fetch_core(
            bun_http::Method::GET,
            None,
            url,
            Vec::new(),
            BridgeRequestBody::Empty,
            &worker_cancel,
            None::<fn() -> bool>,
            None,
            false,
            true,
        ).await {
            Ok(response) => response,
            Err(error) => { let _ = sender.send(Err(format!("head failed: {error:?}"))); return; },
        };
        let head_at = Instant::now();
        let mut chunks = Vec::new();
        while let Some(frame) = response.next_chunk().await {
            match frame {
                Ok(bytes) => chunks.push(bytes),
                Err(error) => { let _ = sender.send(Err(format!("frame failed: {error:?}"))); return; },
            }
        }
        let _ = sender.send(Ok((head_at, chunks)));
    });
    match receiver.recv_timeout(Duration::from_secs(6)) {
        Ok(Ok((head_at, chunks))) => {
            eprintln!("keepalive #1 chunks={:?} (head {:?} ago)", chunks, head_at.elapsed());
            assert_eq!(chunks.concat(), b"hello-cl-0");
        },
        Ok(Err(message)) => panic!("stream error: {message}"),
        Err(_) => panic!("stream never completed on keep-alive Content-Length"),
    }

    // Second request: MUST reuse the pooled socket (same host:port, same
    // null SSLConfig) and still complete.
    let (sender2, receiver2) = unbounded::<Result<Vec<u8>, String>>();
    let cancel2 = Arc::new(BunCancelHandle::new());
    let worker_cancel2 = Arc::clone(&cancel2);
    let port_for_task2 = port;
    spawn_task(async move {
        let url = format!("http://127.0.0.1:{}/second", port_for_task2);
        let mut response = match fetch_core(
            bun_http::Method::GET,
            None,
            url,
            Vec::new(),
            BridgeRequestBody::Empty,
            &worker_cancel2,
            None::<fn() -> bool>,
            None,
            false,
            true,
        ).await {
            Ok(response) => response,
            Err(error) => { let _ = sender2.send(Err(format!("head failed: {error:?}"))); return; },
        };
        match response.collect_body().await {
            Ok(body) => { let _ = sender2.send(Ok(body)); },
            Err(error) => { let _ = sender2.send(Err(format!("body failed: {error:?}"))); },
        }
    });
    match receiver2.recv_timeout(Duration::from_secs(6)) {
        Ok(Ok(body)) => {
            eprintln!("keepalive #2 (reused socket): body={:?}", body);
            assert_eq!(body, b"hello-cl-1");
        },
        Ok(Err(message)) => panic!("PROBE ERROR #2: {message}"),
        Err(_) => panic!("second request on the pooled keep-alive socket never completed"),
    }
}

/// Stage 3 (h2 coalescing enabler): the bridge's per-request SSLConfig must
/// be interned through `GlobalRegistry` — every bun_http pool key (keep-alive
/// pool AND the h2 session matchers) compares `*const SSLConfig`, so
/// content-equal configs built independently per request must resolve to ONE
/// pointer. Proves both halves:
///   1. intern identity — two separately built content-equal configs upgrade
///      to the same registry entry while alive;
///   2. pool effect — two fetch_core requests that each build+intern their
///      own config (mirroring `obtain_response_bun`) reuse ONE keep-alive
///      connection (server-observed connection count == 1). Pre-intern this
///      was one connection per request (distinct pool keys).
#[test]
fn bridge_ssl_config_intern_pool_coalescing() {
    use std::io::{Read, Write};
    link_native_seam();
    // Initialize the async runtime (spawn_task silently drops tasks without
    // it; the raw-TCP fixture below doesn't lazily init like make_server).
    let (_runtime_server, _runtime_url) = make_server(|_request, _response| {});

    let profile = bao_stealth::StealthProfile::firefox_default();
    let wire = servo_wire(&profile);
    let fingerprint = profile.http2.clone();

    // 1. Intern identity: independently built content-equal configs → one
    //    pointer (both strong refs held, so the registry weak-upgrades).
    let interned_a = bun_http::ssl_config::GlobalRegistry::intern(build_ssl_config(
        Some(&wire),
        Some(&fingerprint),
        None,
    ));
    let interned_b = bun_http::ssl_config::GlobalRegistry::intern(build_ssl_config(
        Some(&wire),
        Some(&fingerprint),
        None,
    ));
    assert_eq!(
        bun_http::ssl_config::SSLConfig::raw_ptr(Some(&interned_a)),
        bun_http::ssl_config::SSLConfig::raw_ptr(Some(&interned_b)),
        "content-equal configs must intern to one registry entry (pool-key shape)"
    );
    drop(interned_a);
    drop(interned_b);

    // 2. Pool effect: keep-alive h1 server that counts connections. Two
    //    requests, each building+interning its own config exactly like
    //    `obtain_response_bun` does → ONE connection total.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let connections = Arc::new(AtomicUsize::new(0));
    let connections_server = Arc::clone(&connections);
    std::thread::spawn(move || {
        // Serve until 2s of silence: per connection, answer every request
        // head with a Content-Length body (keep-alive).
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            connections_server.fetch_add(1, Ordering::SeqCst);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(idx) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..idx]).to_string();
                            let path = head.split(' ').nth(1).unwrap_or("/").to_string();
                            buf.drain(..idx + 4);
                            let body = format!("interned:{path}");
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            if stream.write_all(resp.as_bytes()).is_err() {
                                return;
                            }
                        }
                    },
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock ||
                            e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break; // idle keep-alive connection — stop serving it
                    },
                    Err(_) => return,
                }
            }
        }
    });

    for (label, path) in [("/first", 0usize), ("/second", 1)] {
        let _ = label;
        let (sender, receiver) = unbounded::<Result<Vec<u8>, String>>();
        let cancel = Arc::new(BunCancelHandle::new());
        let worker_cancel = Arc::clone(&cancel);
        let wire_clone = wire.clone();
        let fingerprint_clone = fingerprint.clone();
        let port_for_task = port;
        spawn_task(async move {
            // Exactly obtain_response_bun's shape: build a FRESH config per
            // request, intern it, hand the shared pointer to fetch_core.
            let ssl_config = build_ssl_config(
                Some(&wire_clone),
                Some(&fingerprint_clone),
                None,
            );
            let tls_props = Some(bun_http::ssl_config::GlobalRegistry::intern(ssl_config));
            let url = format!("http://127.0.0.1:{}/first-or-second-{}", port_for_task, path);
            match fetch_core(
            bun_http::Method::GET,
            None,
                url,
                Vec::new(),
                BridgeRequestBody::Empty,
                &worker_cancel,
                None::<fn() -> bool>,
                tls_props,
                false,
                true,
            )
            .await
            {
                Ok(mut response) => match response.collect_body().await {
                    Ok(body) => {
                        let _ = sender.send(Ok(body));
                    },
                    Err(error) => {
                        let _ = sender.send(Err(format!("body failed: {error:?}")));
                    },
                },
                Err(error) => {
                    let _ = sender.send(Err(format!("head failed: {error:?}")));
                },
            }
        });
        match receiver.recv_timeout(Duration::from_secs(6)) {
            Ok(Ok(body)) => assert!(
                body.starts_with(b"interned:/first-or-second-"),
                "request {path} body wrong: {body:?}"
            ),
            Ok(Err(message)) => panic!("PROBE ERROR interned #{path}: {message}"),
            Err(_) => panic!("interned request #{path} never completed"),
        }
    }

    // Give the server thread a moment to observe a (wrong) extra connection.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "two requests with interned content-equal SSLConfigs must share ONE keep-alive connection (got {})",
        connections.load(Ordering::SeqCst)
    );
}

/// Full servo fetch pipeline with the bridge flag ON (stage 2 regression):
/// GET through http_fetch's own consumption loop — status, headers and body
/// must all arrive (this is the level that caught the missing-header-map
/// bug: bridge-level assertions never observe response headers).
#[test]
fn bridge_servo_pipeline_document_fetch() {
    use net_traits::request::{Referrer, RequestBuilder, RequestMode};
    use net_traits::response::ResponseBody;
    use servo_base::id::{TEST_PIPELINE_ID, TEST_WEBVIEW_ID};

    link_native_seam();

    let (server, url) = make_server(|_request, response| {
        response.headers_mut().insert(
            "content-type",
            HeaderValue::from_static("text/plain"),
        );
        *response.body_mut() = make_body(b"pipeline hello".to_vec());
    });
    let mut context = crate::new_fetch_context(None, None);
    let request = RequestBuilder::new(
        Some(TEST_WEBVIEW_ID),
        url.clone(),
        Referrer::NoReferrer,
    )
    .method(http::Method::GET)
    .body(None)
    .destination(Destination::Document)
    .origin(url.clone().origin())
    .pipeline_id(Some(TEST_PIPELINE_ID))
    .mode(RequestMode::NoCors)
    .policy_container(Default::default())
    .build();

    let response = crate::fetch_with_context(request, &mut context);
    let _ = server.close();
    let status = response.status.code();
    eprintln!("pipeline: status={status:?} internal={:?}", response.internal_response.is_some());
    match &*response.body.lock() {
        ResponseBody::Done(bytes) => {
            eprintln!("pipeline body: {:?}", String::from_utf8_lossy(bytes));
            assert_eq!(bytes.as_slice(), b"pipeline hello");
        },
        other => panic!("PROBE2: body not Done: {other:?}"),
    }
}

/// CORS-mode GET from an OPAQUE origin through the full servo pipeline on
/// the bridge (stage 2 regression): mirrors the e2e matrix's data:-page XHR
/// — the ACAO * response must pass servo's CORS check (this exact shape
/// surfaced as `Error(CORS check failed)` when response headers were lost).
#[test]
fn bridge_servo_pipeline_cors_opaque_origin() {
    use net_traits::request::{Referrer, RequestBuilder, RequestMode};
    use net_traits::response::ResponseBody;
    use servo_base::id::{TEST_PIPELINE_ID, TEST_WEBVIEW_ID};

    link_native_seam();

    let (server, url) = make_server(|_request, response| {
        response.headers_mut().insert(
            "access-control-allow-origin",
            HeaderValue::from_static("*"),
        );
        *response.body_mut() = make_body(b"cors-data".to_vec());
    });
    let mut context = crate::new_fetch_context(None, None);
    let request = RequestBuilder::new(
        Some(TEST_WEBVIEW_ID),
        url.clone(),
        Referrer::NoReferrer,
    )
    .method(http::Method::GET)
    .body(None)
    .destination(Destination::None)
    // Opaque origin — mirrors the e2e matrix (data: URL page XHR).
    .origin(servo_url::ImmutableOrigin::new_opaque())
    .pipeline_id(Some(TEST_PIPELINE_ID))
    .mode(RequestMode::CorsMode)
    .policy_container(Default::default())
    .build();

    let response = crate::fetch_with_context(request, &mut context);
    let _ = server.close();
    eprintln!(
        "cors: status={:?} termination={:?} internal={:?} rtype={:?}",
        response.status, response.termination_reason, response.internal_response.is_some(), response.response_type
    );
    if let Some(internal) = response.internal_response.as_ref() {
        eprintln!("cors internal: status={:?} headers={:?}", internal.status, internal.headers);
    }
    match &*response.body.lock() {
        ResponseBody::Done(bytes) => {
            eprintln!("cors body: {:?}", String::from_utf8_lossy(bytes));
            assert_eq!(bytes.as_slice(), b"cors-data");
        },
        other => panic!("cors body not Done: {other:?}"),
    }
}
