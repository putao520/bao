// @trace REQ-ENG-006 REQ-STL-002 [level:e2e]
//
// Node-stack window.fetch vs a REAL h2 server: wire-level evidence for the
// two gaps the bridge matrix left open (the matrix covers servo page-network
// egress only; window.fetch is the Node stack and was only ever asserted
// against plain-HTTP fixtures or ClientHello captures that never complete
// TLS):
//
//   1. Full h2 round trip with a DECODING server: request HEADERS are
//      HPACK-decoded by the fixture (lshpack — the same library the h2
//      client uses for response decoding), so method / :path / :authority /
//      :scheme / regular headers are asserted as decoded wire facts, not
//      config echoes. Response is consumed to completion (status + full
//      body bytes) by the fetch harness.
//   2. h2 SETTINGS on the wire: the client's FIRST non-ACK SETTINGS frame
//      payload, captured byte-for-byte at connection establishment, equals
//      the active stealth profile's Http2Fingerprint::settings_frame_payload
//      serialization (id BE16 + value BE32 per entry) — and follows the
//      profile (Firefox ≠ Chrome), proving the bytes are profile-driven,
//      not a constant.
//   Plus PRIORITY-mode wire fallout: the first request stream id is 13 with
//   the Firefox profile (PRIORITY reservations 3/5/7/11 — REQ-STL-002-C3)
//   and 1 with Chrome (no PRIORITY frames).
//
// Harness: window.fetch's exact wire path driven directly —
// `AsyncHTTP::init` + `HTTPThread::schedule` with
// `GlobalRegistry::intern(stealth_profile_to_ssl_config(&profile))`
// tls_props, the byte-identical SSLConfig object `fetch_async::start` builds
// for `window.fetch` (same intern registry → same pool-key semantics). The
// JS Promise/Response layer above it is engine-agnostic and already e2e'd
// over the same AsyncHTTP by fetch_headers/fetch_init tests. A full-JS
// variant against this self-signed fixture is impossible without product
// changes: Node-stack fetch verifies fail-closed (probe:
// `DEPTH_ZERO_SELF_SIGNED_CERT`) and exposes no test-side CA / verification
// override — `reject_unauthorized=false` here is the same test-only posture
// as h2_continuation_cap_tests / proxy_tunnel_resumption_tests.

#![allow(dead_code)]

mod common;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use bao_stealth::{Http2Fingerprint, StealthProfile};
use bun_core::MutableString;
use bun_http::header_builder::HeaderBuilder;
use bun_http::{AsyncHTTP, HTTPClientResult, HTTPClientResultCallback, Method, FetchRedirect,
               async_http, http_thread};
use common::h2_server::H2Server;

/// The fixed body the h2 fixture answers every request with.
const FIXTURE_BODY: &str =
    "<html><head><title>h2</title></head><body><p id=\"t\">h2 doc</p></body></html>";

/// Serialize a profile's SETTINGS exactly as the wire carries them (and as
/// `stealth_http::h2_settings_wire_format` builds the SSLConfig payload):
/// per entry id BE16 + value BE32, profile order.
fn expected_settings_wire(fp: &Http2Fingerprint) -> Vec<u8> {
    let mut wire = Vec::with_capacity(fp.settings_frame_payload().len() * 6);
    for (id, value) in fp.settings_frame_payload() {
        wire.extend_from_slice(&id.to_be_bytes());
        wire.extend_from_slice(&value.to_be_bytes());
    }
    wire
}

// ─── Delivery harness (mirror of h2_continuation_cap_tests' Recorder) ───────

#[derive(Debug)]
struct Delivery {
    status: Option<u32>,
    fail: Option<bun_core::Error>,
    has_more: bool,
    body: String,
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

    // Copy the response body out of the caller-thread buffer before the
    // terminal drop (mirror of on_http_done's sole-dropper contract).
    let mut body = String::new();
    if !has_more {
        let buf = unsafe { (*async_http).response_buffer };
        if !buf.is_null() {
            let ms: &MutableString = unsafe { &*buf };
            body = String::from_utf8_lossy(&ms.list).into_owned();
        }
    }

    if !has_more {
        // Terminal delivery: reclaim the caller-thread `AsyncHTTP` box via
        // the `real` backref plus the response buffer.
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
        body,
    });
}

/// Outcome of one driven fetch: deliveries in arrival order.
struct FetchRun {
    deliveries: Vec<Delivery>,
}

/// Drive one request through the real HTTPThread over ALPN-negotiated h2 —
/// window.fetch's wire path with the given profile's interned SSLConfig.
fn run_node_fetch_h2(
    port: u16,
    profile: &StealthProfile,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> FetchRun {
    bun_core::output::init_test();
    http_thread::init(&Default::default());

    // The exact tls_props object window.fetch builds in fetch_async::start:
    // stealth_profile_to_ssl_config → GlobalRegistry::intern.
    let ssl_config = bun_runtime::stealth_http::stealth_profile_to_ssl_config(&Some(profile.clone()));
    let tls_props = bun_http::ssl_config::GlobalRegistry::intern(ssl_config);

    let (tx, rx) = mpsc::channel();
    let recorder = Box::into_raw(Box::new(Recorder { tx }));

    let url = format!("https://127.0.0.1:{}{}", port, path);
    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);

    let mut hb = HeaderBuilder::default();
    for (name, value) in headers {
        hb.count(name.as_bytes(), value.as_bytes());
    }
    hb.allocate().expect("header allocation");
    for (name, value) in headers {
        hb.append(name.as_bytes(), value.as_bytes());
    }
    let entry_list = hb.entries;
    let headers_buf: &'static [u8] = if hb.content.len > 0 {
        let ptr = hb.content.ptr.expect("allocated content ptr");
        // SAFETY: HeaderBuilder allocated `len` initialized bytes at `ptr`;
        // leaked for the heap AsyncHTTP to borrow (same contract as
        // fetch_async / the bun bridge).
        unsafe { std::slice::from_raw_parts(ptr.as_ptr(), hb.content.len) }
    } else {
        b""
    };

    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let body_static: &'static [u8] = if body.is_empty() {
        b""
    } else {
        Box::leak(body.to_vec().into_boxed_slice())
    };

    let mut options = async_http::Options::default();
    options.tls_props = Some(tls_props);
    // Self-signed fixture cert — same test-only posture as
    // h2_continuation_cap_tests (product fetch is fail-closed here).
    options.reject_unauthorized = Some(false);

    let ah = AsyncHTTP::init(
        method,
        parsed_url,
        entry_list,
        headers_buf,
        response_buffer,
        body_static,
        HTTPClientResultCallback::new(recorder, recorder_callback),
        FetchRedirect::Follow,
        options,
    );

    let ah_ptr = bun_core::heap::into_raw(Box::new(ah));
    let batch = bun_threading::thread_pool::Batch::from(unsafe {
        core::ptr::addr_of_mut!((*ah_ptr).task)
    });
    bun_http::HTTPThread::schedule(batch);

    let mut deliveries = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let Ok(d) = rx.recv_timeout(remaining) else {
            break;
        };
        let terminal = !d.has_more;
        deliveries.push(d);
        if terminal {
            break;
        }
    }
    FetchRun { deliveries }
}

/// Terminal delivery: not failed, status 200, full fixture body consumed.
fn assert_ok_200_full_body(run: &FetchRun, ctx: &str) {
    let Some(last) = run.deliveries.last() else {
        panic!("{}: no delivery before deadline (fetch hung?)", ctx);
    };
    assert!(!last.has_more, "{}: no terminal delivery", ctx);
    assert!(
        last.fail.is_none(),
        "{}: expected success, got fail {:?}",
        ctx,
        last.fail.as_ref().map(|e| e.name())
    );
    assert_eq!(last.status, Some(200), "{}: expected 200", ctx);
    assert_eq!(
        last.body, FIXTURE_BODY,
        "{}: response body must be the fixture's full HTML",
        ctx
    );
}

/// Wait until the fixture recorded at least one client SETTINGS payload.
fn wait_for_client_settings(server: &H2Server) -> Vec<Vec<u8>> {
    let ok = common::wait_for_condition(Duration::from_secs(5), || {
        !server.client_settings.lock().unwrap().is_empty()
    });
    assert!(
        ok,
        "fixture never saw a client SETTINGS frame (connections: h2={}, non-h2={})",
        server.alpn_h2_count.load(std::sync::atomic::Ordering::SeqCst),
        server.non_h2_count.load(std::sync::atomic::Ordering::SeqCst),
    );
    server.client_settings.lock().unwrap().clone()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Full round trip + decoded request facts + Firefox SETTINGS bytes on the
/// wire, one connection: the primary window.fetch(Node stack) h2 evidence.
#[test]
fn window_fetch_wire_h2_get_roundtrip_firefox() {
    let server = H2Server::spawn();
    std::thread::sleep(Duration::from_millis(50));

    // fetch's default profile (globals::install_all → ensure_default…).
    let profile = StealthProfile::firefox_default();
    let run = run_node_fetch_h2(
        server.port,
        &profile,
        Method::GET,
        "/window-fetch-h2-e2e",
        &[("x-bao-probe", "node-stack-h2")],
        b"",
    );
    assert_ok_200_full_body(&run, "firefox GET round trip");

    // Decoded request facts (HPACK-decoded by the fixture, lshpack).
    let ok = common::wait_for_condition(Duration::from_secs(5), || {
        !server.requests.lock().unwrap().is_empty()
    });
    assert!(ok, "fixture never decoded a request HEADERS block");
    let requests = server.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "one request stream: {requests:?}");
    let req = &requests[0];
    assert!(!req.decode_error, "HPACK decode failed on the fixture");
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/window-fetch-h2-e2e");
    assert_eq!(req.authority, format!("127.0.0.1:{}", server.port));
    assert_eq!(req.scheme, "https");
    let probe = req
        .headers
        .iter()
        .find(|(name, _)| name == "x-bao-probe")
        .expect("custom request header survived HPACK encode→decode");
    assert_eq!(probe.1, "node-stack-h2");
    // REQ-STL-002-C3 on the Node stack's wire: Firefox PRIORITY frames
    // reserve streams 3/5/7/11 before the first request (→ 13).
    assert_eq!(
        req.stream_id, 13,
        "Firefox profile first request stream (PRIORITY reservations 3/5/7/11)"
    );

    // SETTINGS wire bytes == profile payload, byte for byte.
    let settings = wait_for_client_settings(&server);
    assert_eq!(
        settings.len(),
        1,
        "one connection → one captured SETTINGS: {settings:?}"
    );
    let expected = expected_settings_wire(&profile.http2);
    assert_eq!(
        settings[0],
        expected,
        "client's first SETTINGS payload must equal the Firefox profile's wire bytes"
    );
    assert_eq!(settings[0].len(), 36, "6 settings × 6 bytes");

    // Sanity: the payload's first entry is HEADER_TABLE_SIZE 65536 (BE).
    assert_eq!(&settings[0][0..6], &[0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);

    server.shutdown();
}

/// The SETTINGS wire bytes FOLLOW the profile: Chrome's serialization on a
/// fresh origin, byte-for-byte, and different from Firefox's. Chrome also
/// sends no PRIORITY frames → first request stream id 1.
#[test]
fn window_fetch_wire_h2_settings_follow_chrome_profile() {
    let server = H2Server::spawn();
    std::thread::sleep(Duration::from_millis(50));

    let profile = StealthProfile::chrome_default();
    let run = run_node_fetch_h2(
        server.port,
        &profile,
        Method::GET,
        "/chrome-profile",
        &[],
        b"",
    );
    assert_ok_200_full_body(&run, "chrome GET round trip");

    let settings = wait_for_client_settings(&server);
    assert_eq!(settings.len(), 1, "one connection → one SETTINGS: {settings:?}");
    let expected = expected_settings_wire(&profile.http2);
    assert_eq!(
        settings[0],
        expected,
        "client's first SETTINGS payload must equal the Chrome profile's wire bytes"
    );
    let firefox_expected = expected_settings_wire(&Http2Fingerprint::firefox());
    assert_ne!(
        settings[0],
        firefox_expected,
        "Chrome and Firefox SETTINGS wire bytes must differ (profile-driven, not constant)"
    );

    let ok = common::wait_for_condition(Duration::from_secs(5), || {
        !server.requests.lock().unwrap().is_empty()
    });
    assert!(ok, "fixture never decoded a request HEADERS block");
    let requests = server.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].stream_id,
        1,
        "Chrome profile sends no PRIORITY reservations (first stream 1, not 13)"
    );
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/chrome-profile");

    server.shutdown();
}

/// POST with a body: the fixture decodes method POST and receives the exact
/// DATA frame payload (END_STREAM completes the body).
#[test]
fn window_fetch_wire_h2_post_body_roundtrip() {
    let server = H2Server::spawn();
    std::thread::sleep(Duration::from_millis(50));

    let profile = StealthProfile::firefox_default();
    let body = b"bao-h2-e2e-request-body";
    let run = run_node_fetch_h2(
        server.port,
        &profile,
        Method::POST,
        "/upload",
        &[("content-type", "application/octet-stream")],
        body,
    );
    assert_ok_200_full_body(&run, "POST round trip");

    let ok = common::wait_for_condition(Duration::from_secs(5), || {
        server
            .requests
            .lock()
            .unwrap()
            .first()
            .is_some_and(|r| r.body_done)
    });
    assert!(ok, "request body never completed (END_STREAM DATA missing)");
    let requests = server.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert!(!req.decode_error);
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/upload");
    assert_eq!(req.body, body, "DATA frame payload must match the request body");
    let ct = req
        .headers
        .iter()
        .find(|(name, _)| name == "content-type")
        .expect("content-type header decoded");
    assert_eq!(ct.1, "application/octet-stream");

    // Same connection carried the SETTINGS evidence too (profile-driven).
    let settings = wait_for_client_settings(&server);
    assert_eq!(
        settings[0],
        expected_settings_wire(&profile.http2),
        "POST connection's SETTINGS must equal the Firefox profile bytes"
    );

    server.shutdown();
}
