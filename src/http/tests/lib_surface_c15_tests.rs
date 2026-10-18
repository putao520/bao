//! C15 coverage wave — offline characterization of `bun_http` lib.rs
//! standalone surfaces (REQ-ENG-010). Complements the C7 wire-level suites:
//! this file covers the pure/global faces that need no HTTP thread, no
//! sockets and no env mutation — enum defaults, the response-metadata
//! ownership contract, the global knobs, the header-hash / extension-method
//! interns, the result accessors, and the callback boxing faces.
//!
//! Process-isolation note: this binary never touches `BUN_CONFIG_HTTP_IDLE_TIMEOUT`
//! or the h3 feature-flag env, so the static-default assertions here hold under
//! both nextest (one process per test) and plain `cargo test --test-threads=1`.
//! The env-dependent knob arm lives in `knobs_env_c15_tests.rs` (own binary).

use std::sync::atomic::{AtomicUsize, Ordering};

use bun_http::picohttp;
use bun_http::{
    ASYNC_HTTP_ID_MONOTONIC, BodySize, HTTPClientResult, HTTPResponseMetadata, HTTPUpgradeState,
    HTTPVerboseLevel, Method, PoolBypass, Protocol, ShouldContinue, cleanup, hash_header_name,
    idle_timeout_seconds, intern_extension_method, max_http_header_size, set_max_http_header_size,
    temp_hostname,
};

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by `bun_runtime::dispatch` in product binaries
// (bun_runtime is higher-tier than bun_http and cannot be dev-depped here).
// Nothing in this file drives the event loop; a no-op satisfies link.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

// Link seam for `__bun_crash_handler_out_of_memory`: bun_alloc resolves it at
// link time against bun_crash_handler (higher-tier than this crate's test
// binary). OOM aborts the process either way — a faithful test stub.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    eprintln!("bun: out of memory");
    std::process::abort()
}

/// Link leg (C7 suites convention): referencing `force_link` keeps
/// bao_native_stubs in the binary; its .init_array ctor drags in the
/// lsquic/boringssl native archives that bun_uws_sys's quic/socket objects
/// reference at link time.
#[test]
fn native_stubs_link_leg_c15() {
    bao_native_stubs::force_link();
}

// ─── enum / struct default contracts ───────────────────────────────────────

/// `Flags::default()` is the request-contract baseline: verify-reject ON,
/// HTTP/1.1, no redirect/tunnel/streaming side state. Every public field
/// pinned — a new default silently changing one of these changes every
/// fetch's wire behavior. (`pool_bypass`/`reused_socket_verification` are
/// `pub(crate)` — covered via `PoolBypass`'s own default assert below.)
#[test]
fn flags_default_field_contract_c15() {
    let f = bun_http::Flags::default();
    assert!(!f.disable_timeout);
    assert!(!f.disable_keepalive);
    assert!(!f.disable_decompression);
    assert!(!f.did_have_handshaking_error);
    assert!(!f.force_last_modified);
    assert!(!f.redirected);
    assert!(!f.proxy_tunneling);
    assert!(f.reject_unauthorized, "reject_unauthorized defaults ON");
    assert!(!f.is_preconnect_only);
    assert!(!f.is_streaming_request_body);
    assert!(!f.defer_fail_until_connecting_is_complete);
    assert!(matches!(f.upgrade_state, HTTPUpgradeState::None));
    assert!(matches!(f.protocol, Protocol::Http1_1));
    assert!(!f.force_http2);
    assert!(!f.force_http1);
    assert!(!f.force_http3);
    assert!(!f.h3_retried);
    assert!(!f.h3_alt_svc_fallback);
    assert!(!f.is_node_http_client);
    assert!(!f.is_page_egress);
    assert!(!f.omit_connection_header);
    assert!(!f.response_transport_paused);
}

/// The small enums: `#[default]` marks and the wire-protocol discriminants.
#[test]
fn small_enum_defaults_and_discriminants_c15() {
    assert!(PoolBypass::default() == PoolBypass::Off);
    assert!(Protocol::default() == Protocol::Http1_1);
    assert!(HTTPUpgradeState::default() == HTTPUpgradeState::None);
    assert_eq!(HTTPUpgradeState::None as u8, 0);
    assert_eq!(HTTPUpgradeState::Pending as u8, 1);
    assert_eq!(HTTPUpgradeState::Upgraded as u8, 2);
    assert!(HTTPVerboseLevel::default() == HTTPVerboseLevel::None);
    assert!(matches!(BodySize::default(), BodySize::Unknown));
    // BodySize carries the two sized variants used by result accounting.
    assert!(matches!(BodySize::ContentLength(7), BodySize::ContentLength(7)));
    assert!(matches!(
        BodySize::TotalReceived(1),
        BodySize::TotalReceived(1)
    ));
    // ShouldContinue is the h2/h3 framing verdict pair (PartialEq, no Debug).
    assert!(ShouldContinue::ContinueStreaming != ShouldContinue::Finished);
}

/// `HTTPResponseMetadata::default` shape + the `Drop` reclaim contract: the
/// only non-empty producer (`clone_metadata`) `Box::leak`s the header list
/// and Drop is its sole owner — simulate that exact shape and drop without
/// leaking/crashing, then prove the empty-list arm skips the reclaim.
#[test]
fn response_metadata_default_and_drop_reclaims_leaked_headers_c15() {
    let meta = HTTPResponseMetadata::default();
    assert!(meta.owned_buf.is_empty());
    assert!(meta.response.headers.list.is_empty());
    drop(meta); // empty-list arm: no reclaim

    // clone_metadata shape: leaked header slice handed to Drop for reclamation.
    let leaked: &'static [picohttp::Header] = Box::leak(
        vec![
            picohttp::Header::new(b"Content-Type", b"text/plain"),
            picohttp::Header::new(b"X-C15", b"yes"),
        ]
        .into_boxed_slice(),
    );
    let mut meta = HTTPResponseMetadata::default();
    meta.response.headers = picohttp::HeaderList { list: leaked };
    drop(meta); // must destroy the leaked slice, not double-free

    // Repeat the shape once more to prove the reclaim is per-object (each
    // Drop owns exactly its own leak; no shared static state).
    let leaked2: &'static [picohttp::Header] =
        Box::leak(vec![picohttp::Header::new(b"A", b"b")].into_boxed_slice());
    let mut meta2 = HTTPResponseMetadata::default();
    meta2.response.headers = picohttp::HeaderList { list: leaked2 };
    drop(meta2);
}

// ─── global knobs ──────────────────────────────────────────────────────────

/// The header-size cap defaults to 16 KiB (mirrors Zig
/// `bun.http.max_http_header_size`); the safe setter round-trips and the
/// original value is restored so other binaries/tests see the default.
#[test]
fn max_http_header_size_set_get_roundtrip_c15() {
    assert_eq!(max_http_header_size(), 16 * 1024, "compile-time default");
    set_max_http_header_size(4096);
    assert_eq!(max_http_header_size(), 4096);
    set_max_http_header_size(16 * 1024);
    assert_eq!(max_http_header_size(), 16 * 1024);
}

/// Idle-timeout static default is 300s (5 minutes). The env override + pad
/// normalization is C7's `idle_timeout_tests` (own binary, env-before-init);
/// this pins the untouched default.
#[test]
fn idle_timeout_seconds_default_is_300_c15() {
    assert_eq!(idle_timeout_seconds(), 300);
}

/// `cleanup` is a documented no-op port (MimallocArena bulk-free removed);
/// callable with either polarity.
#[test]
fn cleanup_is_a_noop_c15() {
    cleanup(false);
    cleanup(true);
}

/// The CLI arm of the h3 Alt-Svc gate: the static flips the gate without any
/// env. The env-flag arm is `knobs_env_c15_tests.rs` (process isolation).
#[test]
fn h3_alt_svc_enabled_cli_atomic_arm_c15() {
    // Fresh-process default: no CLI flip, env flag unset in this binary.
    assert!(!bun_http::h3_alt_svc_enabled());
    bun_http::EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI.store(true, Ordering::Release);
    assert!(bun_http::h3_alt_svc_enabled(), "CLI arm must enable");
    bun_http::EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI.store(false, Ordering::Release);
    assert!(!bun_http::h3_alt_svc_enabled());
}

/// Request ids come from a monotonic fetch_add whose first assignment is 1 —
/// 0 is the "no id" sentinel the cross-thread schedules guard on. Strict
/// increase is the contract (ids must never repeat or wrap back to 0).
#[test]
fn async_http_id_monotonic_never_returns_sentinel_c15() {
    let a = ASYNC_HTTP_ID_MONOTONIC.fetch_add(1, Ordering::Relaxed);
    let b = ASYNC_HTTP_ID_MONOTONIC.fetch_add(1, Ordering::Relaxed);
    assert!(b > a, "ids must strictly increase: {a} then {b}");
    assert_ne!(a, 0, "first-ever assignment is 1, never the 0 sentinel");
}

// ─── hashing / interning ───────────────────────────────────────────────────

/// Header-name hashing is ASCII-lowercase-folded and deterministic — the
/// same name in any case must hash identically (the case-insensitive header
/// table key contract), different names must differ.
#[test]
fn hash_header_name_folds_case_and_is_deterministic_c15() {
    let lower = hash_header_name(b"content-type");
    let upper = hash_header_name(b"Content-Type");
    let mixed = hash_header_name(b"CoNtEnT-TyPe");
    assert_eq!(lower, upper, "case-folded hashing");
    assert_eq!(lower, mixed, "case-folded hashing");
    assert_eq!(lower, hash_header_name(b"content-type"), "deterministic");
    assert_ne!(lower, hash_header_name(b"content-length"));
    assert_ne!(hash_header_name(b""), hash_header_name(b"a"));
}

/// Extension-method interning: the same verb returns the same `&'static`
/// slice (pointer-stable — callers store it in `Options::extension_method`
/// and the wire writer reads it later), distinct verbs get distinct tokens.
#[test]
fn intern_extension_method_is_stable_and_distinct_c15() {
    let foo1 = intern_extension_method(b"FOO");
    let foo2 = intern_extension_method(b"FOO");
    assert!(std::ptr::eq(foo1, foo2), "same verb must intern to one token");
    assert_eq!(foo1, b"FOO", "wire form is the verb bytes verbatim");

    let bar = intern_extension_method(b"BAR");
    assert!(!std::ptr::eq(foo1, bar));
    assert_eq!(bar, b"BAR");
    // Interning a closed-registry verb is legal and equally stable.
    let get = intern_extension_method(b"GET");
    assert!(std::ptr::eq(get, intern_extension_method(b"GET")));
}

/// The HTTP-thread scratch hostname buffer is process-stable storage: a
/// write through the accessor reads back until overwritten (the NUL-terminated
/// SNI builder's contract).
#[test]
fn temp_hostname_buffer_write_read_c15() {
    let buf: &'static mut [u8; 8192] = temp_hostname();
    buf[..9].copy_from_slice(b"c15-host\0");
    assert_eq!(&buf[..8], b"c15-host");
    assert_eq!(buf[8], 0, "NUL terminator slot writable");
    let again: &'static mut [u8; 8192] = temp_hostname();
    assert_eq!(&again[..8], b"c15-host", "same process-stable storage");
    again[..4].copy_from_slice(b"over");
    assert_eq!(&buf[..4], b"over", "both hands alias one buffer");
}

// ─── HTTPClientResult accessor matrix ──────────────────────────────────────

/// The fail-shape accessors: Timeout → abort_reason Timeout; Aborted /
/// AbortedBeforeConnecting → UserAbort; any other failure → neither; no
/// failure → is_success. The terminal-state vocabulary the JS layer maps
/// `AbortSignal`/timeout onto.
#[test]
fn http_client_result_fail_shape_matrix_c15() {
    use bun_http_types::FetchRedirect::CommonAbortReason;

    let r = HTTPClientResult::default();
    assert!(r.is_success());
    assert!(!r.is_timeout());
    assert!(!r.is_abort());
    assert_eq!(r.abort_reason(), None);
    assert_eq!(r.connect_errno, 0);
    assert!(r.metadata.is_none());
    assert!(!r.has_more && !r.redirected && !r.can_stream && !r.is_http2);

    let mut timeout = HTTPClientResult::default();
    timeout.fail = Some(bun_core::err!("Timeout"));
    assert!(timeout.is_timeout());
    assert!(!timeout.is_abort());
    assert!(matches!(timeout.abort_reason(), Some(CommonAbortReason::Timeout)));

    let mut aborted = HTTPClientResult::default();
    aborted.fail = Some(bun_core::err!("Aborted"));
    assert!(!aborted.is_timeout());
    assert!(aborted.is_abort());
    assert!(matches!(aborted.abort_reason(), Some(CommonAbortReason::UserAbort)));

    let mut pre_connect = HTTPClientResult::default();
    pre_connect.fail = Some(bun_core::err!("AbortedBeforeConnecting"));
    assert!(pre_connect.is_abort(), "pre-connect abort is still a user abort");
    assert!(matches!(pre_connect.abort_reason(), Some(CommonAbortReason::UserAbort)));

    let mut refused = HTTPClientResult::default();
    refused.fail = Some(bun_core::err!("ConnectionRefused"));
    assert!(!refused.is_timeout());
    assert!(!refused.is_abort());
    assert_eq!(refused.abort_reason(), None, "connect failures are not aborts");
}

// ─── HTTPClientResultCallback boxing ───────────────────────────────────────

/// Shutdown-time release counter — bumped by the `new_with_release` release
/// fn. Release must NOT run at `run()` time (it is exit-path only).
static RELEASE_CALLS: AtomicUsize = AtomicUsize::new(0);
unsafe fn release_counter(_ctx: *mut ()) {
    RELEASE_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// Delivery recorder shared with the `run()` callback (module-static because
/// the release fn above shares the file's statics discipline).
static LAST_DELIVERY: std::sync::Mutex<Option<(u32, bool)>> = std::sync::Mutex::new(None);

fn delivery_cb(_this: *mut (), _ah: *mut bun_http::AsyncHTTP<'static>, result: HTTPClientResult<'_>) {
    *LAST_DELIVERY.lock().unwrap() = Some((42, result.has_more));
}

/// `HTTPClientResultCallback::new` leaves the shutdown release unset,
/// `new_with_release` attaches it, and `run` delivers the result to the
/// wrapped callback with the ctx round-tripped — driven with a real (never
/// scheduled, no network) `AsyncHTTP`.
#[test]
fn result_callback_release_and_run_delivery_c15() {
    use bun_core::MutableString;
    use bun_http::async_http;
    use bun_http::signals::Store;

    bun_core::Output::init_test();

    let plain = bun_http::HTTPClientResultCallback::new(std::ptr::null_mut::<()>(), delivery_cb);
    assert!(plain.release_at_shutdown.is_none(), "new: no release");
    let with_release = bun_http::HTTPClientResultCallback::new_with_release(
        std::ptr::null_mut::<()>(),
        delivery_cb,
        release_counter,
    );
    assert!(with_release.release_at_shutdown.is_some(), "new_with_release attaches");

    // Build a real (never-scheduled) AsyncHTTP so `run` gets a valid pointer.
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let url_bytes: &'static [u8] =
        Box::leak(b"http://127.0.0.1:9/not-scheduled".to_vec().into_boxed_slice());
    let parsed = bun_url::URL::parse(url_bytes);
    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    options.signals = Some(store.to());
    let ah = bun_http::AsyncHTTP::init(
        Method::GET,
        parsed,
        Default::default(),
        b"",
        response_buffer,
        b"",
        with_release,
        bun_http::FetchRedirect::Follow,
        options,
    );
    let ah_ptr: *mut bun_http::AsyncHTTP<'static> = bun_core::heap::into_raw(Box::new(ah));
    let id = unsafe { (*ah_ptr).async_http_id };
    assert_ne!(id, 0, "init assigns a real request id");

    let result = HTTPClientResult {
        has_more: false,
        ..Default::default()
    };
    // SAFETY: ah_ptr is a live heap AsyncHTTP; the callback reads only its
    // own statics.
    with_release.run(ah_ptr, result);
    let got = LAST_DELIVERY.lock().unwrap().take().expect("run must deliver");
    assert_eq!(got, (42, false));
    assert_eq!(
        RELEASE_CALLS.load(Ordering::Relaxed),
        0,
        "release is shutdown-time only, never at run()"
    );

    // Reclaim the unscheduled box the way a terminal delivery would.
    unsafe { drop(Box::from_raw(ah_ptr)) };
    unsafe { drop(Box::from_raw(response_buffer)) };
}

/// `ThreadlocalAsyncHTTP::new` boxes the request handle (the heap-stats
/// wrapper the in-flight queue stores); the id carries through.
#[test]
fn threadlocal_async_http_boxes_c15() {
    use bun_core::MutableString;
    use bun_http::async_http;
    use bun_http::signals::Store;

    bun_core::Output::init_test();
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let url_bytes: &'static [u8] =
        Box::leak(b"http://127.0.0.1:9/boxed".to_vec().into_boxed_slice());
    let parsed = bun_url::URL::parse(url_bytes);
    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    options.signals = Some(store.to());
    let ah = bun_http::AsyncHTTP::init(
        Method::GET,
        parsed,
        Default::default(),
        b"",
        response_buffer,
        b"",
        bun_http::HTTPClientResultCallback::new(std::ptr::null_mut::<()>(), |_this, _ah, _r| {}),
        bun_http::FetchRedirect::Follow,
        options,
    );
    let boxed = bun_http::ThreadlocalAsyncHTTP::new(ah);
    assert_ne!(boxed.async_http.async_http_id, 0, "handle carried into the box");
    // Reclaim the never-scheduled box + buffer (AsyncHTTP Drop is exercised
    // by the C7 recorders too).
    let boxed = *boxed;
    drop(boxed);
    unsafe { drop(Box::from_raw(response_buffer)) };
}

// ─── print_request ─────────────────────────────────────────────────────────

/// `print_request` covers both formatters and all three protocol labels. The
/// output face is `Output::pretty_errorln` (initialized by `init_test`);
/// the contract under test is the no-panic formatting over a real request
/// shape, including the curl rebuild with `path = url`.
#[test]
fn print_request_formats_all_protocols_and_curl_form_c15() {
    bun_core::Output::init_test();
    let hdrs = [
        picohttp::Header::new(b"Host", b"example.com"),
        picohttp::Header::new(b"User-Agent", b"c15"),
    ];
    let request = picohttp::Request {
        method: b"GET",
        path: b"/idx",
        minor_version: 1,
        headers: &hdrs,
        bytes_read: 0,
    };
    for protocol in [Protocol::Http1_1, Protocol::Http2, Protocol::Http3] {
        bun_http::print_request(protocol, &request, b"http://example.com/idx", true, b"", false);
        bun_http::print_request(protocol, &request, b"http://example.com/idx", false, b"body", true);
    }
}

// ─── misc re-exports / helpers ─────────────────────────────────────────────

/// `strip_ipv6_brackets` (re-exported as the single SNI/NO_PROXY chokepoint):
/// strips only well-formed brackets around a real IPv6 literal.
#[test]
fn strip_ipv6_brackets_shapes_c15() {
    assert_eq!(bun_http::strip_ipv6_brackets(b"[::1]"), b"::1");
    assert_eq!(bun_http::strip_ipv6_brackets(b"[2001:db8::1]"), b"2001:db8::1");
    // Not bracket-wrapped: identity.
    assert_eq!(bun_http::strip_ipv6_brackets(b"127.0.0.1"), &b"127.0.0.1"[..]);
    assert_eq!(bun_http::strip_ipv6_brackets(b"::1"), &b"::1"[..]);
    // Brackets but not an IPv6 literal: unchanged (hostname, not address).
    assert_eq!(bun_http::strip_ipv6_brackets(b"[example.com]"), &b"[example.com]"[..]);
    // Degenerate shapes: too short / unbalanced.
    assert_eq!(bun_http::strip_ipv6_brackets(b"["), &b"["[..]);
    assert_eq!(bun_http::strip_ipv6_brackets(b"[]"), &b"[]"[..]);
}

/// `basic_authorization` (pub, used to auto-fill `Proxy-Authorization` from
/// the proxy URL's userinfo): user+pass → `Basic <b64("user:pass")>`;
/// user without password folds to just the username (`auth = username`, no
/// colon — AsyncHTTP.rs `basic_authorization` else-arm); credential-less URL
/// → None. Base64 literals below are RFC 4648 oracle values.
#[test]
fn basic_authorization_from_proxy_url_userinfo_c15() {
    use bun_http::async_http::basic_authorization;

    let parse = |s: &'static str| {
        let bytes: &'static [u8] = Box::leak(s.as_bytes().to_vec().into_boxed_slice());
        bun_url::URL::parse(bytes)
    };

    let both = parse("http://user:pass@proxy.local:8080");
    assert_eq!(
        basic_authorization(&both).as_deref(),
        Some(&b"Basic dXNlcjpwYXNz"[..]),
        "user:pass → base64(\"user:pass\")"
    );

    let user_only = parse("http://user@proxy.local");
    assert_eq!(
        basic_authorization(&user_only).as_deref(),
        Some(&b"Basic dXNlcg"[..]),
        "user without password → base64(\"user\") with NO padding — the impl          truncates to `encode_url_safe`'s emitted length, so unpadded output          is the wire contract here (the 9-byte \"user:pass\" case needs no          padding either, which is why it reads standard)"
    );

    let none = parse("http://proxy.local:8080");
    assert_eq!(basic_authorization(&none), None, "no userinfo → no auth header");
}

/// `h3::PendingConnect::drain_resolved` on an empty queue is a safe no-op —
/// the only offline-callable face of the DNS-pending QUIC connect lifecycle
/// (the resolved/cancel arms need a live lsquic session; registered as a
/// coverage gap in the C15 wave report).
#[test]
fn h3_pending_connect_drain_empty_queue_is_noop_c15() {
    bun_http::h3::pending_connect::PendingConnect::drain_resolved();
    bun_http::h3::pending_connect::PendingConnect::drain_resolved();
}
