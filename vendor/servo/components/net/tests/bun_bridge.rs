/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! U2 page-network bridge (bun HTTPThread fetch driver) unit tests.
//!
//! These exercise `fetch_core` directly (no `FetchContext` needed) against a
//! local hyper h1 server:
//!   1. plain GET round trip,
//!   2. `FetchRedirect::Manual` returns the original 3xx (not followed),
//!   3. abort via the Signals `aborted` atomic + HTTPThread shutdown →
//!      `NetworkError::LoadCancelled`,
//!   4. the full bun error → NetworkError mapping table.
//!
//! The bridge flag itself is default-off; `http_loader.rs` dispatch and the
//! servo-side behaviour are covered by the rest of this test suite running
//! with the flag off (zero behaviour diff).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::unbounded;
use http::StatusCode;
use http::HeaderValue;
use net_traits::NetworkError;

use net::async_runtime::{spawn_blocking_task, spawn_task};
use net::fetch::bun_bridge::{BunCancelHandle, BridgeError, BunHttpResponse, fetch_core, map_bun_error};
use net::test_util::{make_body, make_server};

/// bun's HTTP client resolves through its own resolver; pin the loopback
/// literal (the test server binds 0.0.0.0, not ::1).
fn loopback_url(url: &servo_url::ServoUrl) -> String {
    url.as_str().replace("localhost", "127.0.0.1")
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
    let outcome = spawn_blocking_task::<_, Result<BunHttpResponse, BridgeError>>(async {
        fetch_core(
            bun_http::Method::GET,
            url,
            vec![(b"accept".to_vec(), b"*/*".to_vec())],
            None,
            &cancel,
            None::<fn() -> bool>,
            None,
            true,
        )
        .await
    });

    let response = outcome.expect("GET through the bun bridge must succeed");
    assert_eq!(response.status_code, 200);
    assert_eq!(response.body, b"hello from bun bridge");
    let header = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(b"x-bao-bridge"))
        .expect("custom response header must pass through");
    assert_eq!(header.1, b"yes");

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
    let outcome = spawn_blocking_task::<_, Result<BunHttpResponse, BridgeError>>(async {
        fetch_core(
            bun_http::Method::GET,
            url,
            Vec::new(),
            None,
            &cancel,
            None::<fn() -> bool>,
            None,
            true,
        )
        .await
    });

    let response = outcome.expect("Manual mode must return the 3xx itself");
    assert_eq!(response.status_code, 301);
    let location = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(b"location"))
        .expect("Location header must be surfaced to servo's redirect loop");
    assert_eq!(location.1, b"/final-destination");
    assert_eq!(response.body, b"moved");
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
            url,
            Vec::new(),
            None,
            &worker_cancel,
            None::<fn() -> bool>,
            None,
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
        outcome,
        Err(BridgeError::Network(NetworkError::LoadCancelled))
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
        // in the servo wrapper.
        ("ERR_TLS_CERT_ALTNAME_INVALID", BridgeError::CertificateFailure("ERR_TLS_CERT_ALTNAME_INVALID".into())),
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
