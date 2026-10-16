// @trace TEST-ENG-008-ULOOP-EDGE [req:REQ-ENG-006] [level:integration]
//
// Coverage-shortfall tests for the bao_uloop pub face (addrinfo.rs DNS seam,
// lib.rs FFI entry points, poll.rs BaoPoll accessors — SPEC 02-SYSTEM
// bao_uloop loop plane):
//   - Bun__parseIpAddress: the C connect-seam literal fast path — v4, v6,
//     v6 with numeric zone (scope_id), non-numeric zone → resolver, name →
//     resolver, null host/out guards (reached through its `#[no_mangle]` C
//     ABI — the exact face usockets' C callers use, since the Rust item is
//     module-private)
//   - Bun__addrinfo_get contract through the re-exports: cache hit → 0 +
//     readable single-entry result; miss → 1 + worker completion + cache
//     fill + completed-hit re-get → 0; unresolvable name → EAI error;
//     registerQuic2 notify exactly-once + no-op arms; null-request guards
//   - lib.rs entry points: force_link, BUN__warn__extra_ca_load_failed
//     (null + real filename arms), bun_ssl_ctx_cache_on_free,
//     Bun__internal_ensureDateHeaderTimerIsEnabled, us_dispatch_keylog /
//     session / ssl_raw_tap
//   - BaoPoll bitfield accessors: fd sign-extension, poll_type masking,
//     kind decode, events decode to the libus interest bits, ext offset

use core::ffi::{c_char, c_int, c_void};
use std::ffi::CString;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// The C ABI face of the DNS seam. `Bun__parseIpAddress` is `#[no_mangle]` in
// bao_uloop's private `addrinfo` module — its consumers are C callers, so the
// test reaches it through the same extern declaration (the symbol is linked
// into this binary via the bao_runtime → bao_uloop dependency).
unsafe extern "C" {
    fn Bun__parseIpAddress(
        host: *const c_char,
        port: u16,
        out: *mut libc::sockaddr_storage,
    ) -> c_int;
}

/// C-layout view of `addrinfo_result` (entries, error) — the stable shape
/// `Bun__addrinfo_getRequestResult` hands to C.
#[repr(C)]
struct AddrInfoResultView {
    entries: *mut AddrInfoEntryView,
    error: c_int,
}

/// C-layout view of `addrinfo_result_entry` = { addrinfo info; storage }.
#[repr(C)]
struct AddrInfoEntryView {
    info: libc::addrinfo,
    _storage: [u8; 128],
}

/// (entries published?, getaddrinfo error) of a live request.
fn result_state_of(req: *mut c_void) -> Option<(bool, c_int)> {
    if req.is_null() {
        return None;
    }
    // SAFETY: live request per Bun__addrinfo_get handout; result_c stays at a
    // fixed offset (the C contract the pointer exists for).
    let result = unsafe { bao_uloop::Bun__addrinfo_getRequestResult(req) };
    if result.is_null() {
        return None;
    }
    // SAFETY: result points at the request's result_c (C-layout view above).
    let view = unsafe { &*(result as *const AddrInfoResultView) };
    Some((!view.entries.is_null(), view.error))
}

/// Poll a request to completion (entries published or error set).
fn wait_completion(req: *mut c_void, what: &str) -> (bool, c_int) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((has_entries, error)) = result_state_of(req) {
            if has_entries || error != 0 {
                return (has_entries, error);
            }
        }
        assert!(
            Instant::now() < deadline,
            "resolver worker did not complete: {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

extern "C" fn quic_notify(pc: *mut c_void) {
    // Records only the payload it was handed — never dereferences it.
    QUIC_NOTIFIED.fetch_add(1, Ordering::SeqCst);
    QUIC_PC.store(pc as usize, Ordering::SeqCst);
}

static QUIC_NOTIFIED: AtomicUsize = AtomicUsize::new(0);
static QUIC_PC: AtomicUsize = AtomicUsize::new(0);

#[test]
fn parse_ip_address_literal_fast_path() {
    // ── guards ──
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let host = CString::new("192.168.4.5").unwrap();
    assert_eq!(
        unsafe { Bun__parseIpAddress(std::ptr::null(), 80, &mut storage) },
        0,
        "null host must fall through to the resolver"
    );
    assert_eq!(
        unsafe { Bun__parseIpAddress(host.as_ptr(), 80, std::ptr::null_mut()) },
        0,
        "null out must fall through to the resolver"
    );

    // ── IPv4 literal → filled sockaddr_in, port network order ──
    let filled = unsafe { Bun__parseIpAddress(host.as_ptr(), 0x1F90, &mut storage) };
    assert_eq!(filled, 1, "v4 literal must be parsed");
    // SAFETY: family-checked storage written by the v4 arm.
    let sin = unsafe { &*std::ptr::addr_of!(storage).cast::<libc::sockaddr_in>() };
    assert_eq!(sin.sin_family, libc::AF_INET as u16);
    assert_eq!(sin.sin_port, 0x1F90u16.to_be());
    assert_eq!(sin.sin_addr.s_addr, u32::from_ne_bytes([192, 168, 4, 5]));

    // ── IPv6 literal ──
    let v6 = CString::new("::1").unwrap();
    let filled = unsafe { Bun__parseIpAddress(v6.as_ptr(), 443, &mut storage) };
    assert_eq!(filled, 1, "v6 literal must be parsed");
    // SAFETY: family-checked storage written by the v6 arm.
    let sin6 = unsafe { &*std::ptr::addr_of!(storage).cast::<libc::sockaddr_in6>() };
    assert_eq!(sin6.sin6_family, libc::AF_INET6 as u16);
    assert_eq!(sin6.sin6_port, 443u16.to_be());
    assert_eq!(sin6.sin6_addr.s6_addr, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

    // ── v6 with numeric zone → scope_id kept (RFC 6874 numeric interface) ──
    let zoned = CString::new("fe80::1%3").unwrap();
    let filled = unsafe { Bun__parseIpAddress(zoned.as_ptr(), 80, &mut storage) };
    assert_eq!(filled, 1, "numeric-zone v6 must be parsed");
    // SAFETY: family-checked storage written by the v6 arm.
    let sin6 = unsafe { &*std::ptr::addr_of!(storage).cast::<libc::sockaddr_in6>() };
    assert_eq!(sin6.sin6_family, libc::AF_INET6 as u16);
    assert_eq!(sin6.sin6_scope_id, 3);
    assert_eq!(sin6.sin6_addr.s6_addr[15], 1);

    // ── resolver fall-throughs ──
    let named_zone = CString::new("fe80::1%eth0").unwrap();
    assert_eq!(
        unsafe { Bun__parseIpAddress(named_zone.as_ptr(), 80, &mut storage) },
        0,
        "name-form zone must go to getaddrinfo"
    );
    let name = CString::new("example.internal").unwrap();
    assert_eq!(
        unsafe { Bun__parseIpAddress(name.as_ptr(), 80, &mut storage) },
        0,
        "hostnames must go to getaddrinfo"
    );
    let not_ip = CString::new("999.999.1.1").unwrap();
    assert_eq!(unsafe { Bun__parseIpAddress(not_ip.as_ptr(), 80, &mut storage) }, 0);
    let empty = CString::new("").unwrap();
    assert_eq!(unsafe { Bun__parseIpAddress(empty.as_ptr(), 80, &mut storage) }, 0);
}

#[test]
fn addrinfo_cache_hit_miss_and_quic_registration() {
    use bun_dns::cache::IpAddr;

    // ── completed hit through the shared cache: get returns 0 ──
    bun_dns::cache::insert(b"uloop-edge-hit.test", vec![IpAddr::V4([127, 0, 0, 2])], Some(60));
    let host = CString::new("uloop-edge-hit.test").unwrap();
    let mut req: *mut c_void = std::ptr::null_mut();
    let rc =
        unsafe { bao_uloop::Bun__addrinfo_get(std::ptr::null_mut(), host.as_ptr(), 8080, &mut req) };
    assert_eq!(rc, 0, "cache hit must report completed (0)");
    assert!(!req.is_null(), "cache hit must still hand back a live request");
    let (has_entries, error) = result_state_of(req).expect("readable result");
    assert!(has_entries && error == 0, "hit carries entries, no error");
    // SAFETY: entries points at the request-owned single-entry chain; the
    // C-layout view above matches addrinfo_result_entry on this platform.
    let result = unsafe { bao_uloop::Bun__addrinfo_getRequestResult(req) };
    let view = unsafe { &*(result as *const AddrInfoResultView) };
    let first = unsafe { &*view.entries };
    assert_eq!(first.info.ai_family, libc::AF_INET);
    assert!(first.info.ai_next.is_null());
    // SAFETY: release the single completed-hit handout.
    unsafe { bao_uloop::Bun__addrinfo_freeRequest(req, 0) };

    // ── miss → 1, real worker resolves, cache fills, re-get hits ──
    let localhost = CString::new("localhost").unwrap();
    let mut req2: *mut c_void = std::ptr::null_mut();
    let rc = unsafe {
        bao_uloop::Bun__addrinfo_get(std::ptr::null_mut(), localhost.as_ptr(), 80, &mut req2)
    };
    assert_eq!(rc, 1, "miss must report in-flight (1)");
    assert!(!req2.is_null());
    let (has_entries, error) = wait_completion(req2, "localhost");
    assert!(has_entries && error == 0, "localhost must resolve: {error}");
    // SAFETY: release the miss handout (worker refs are the worker's own).
    unsafe { bao_uloop::Bun__addrinfo_freeRequest(req2, 0) };
    assert!(
        bun_dns::cache::lookup(b"localhost").is_some(),
        "worker results must land in the shared cache"
    );
    let mut req3: *mut c_void = std::ptr::null_mut();
    let rc = unsafe {
        bao_uloop::Bun__addrinfo_get(std::ptr::null_mut(), localhost.as_ptr(), 80, &mut req3)
    };
    assert_eq!(rc, 0, "post-resolve re-get must be a completed hit");
    // SAFETY: release the second hit handout.
    unsafe { bao_uloop::Bun__addrinfo_freeRequest(req3, 0) };

    // ── miss with an unresolvable name → worker completes with an error ──
    let bad = CString::new("..").unwrap();
    let mut req4: *mut c_void = std::ptr::null_mut();
    let rc =
        unsafe { bao_uloop::Bun__addrinfo_get(std::ptr::null_mut(), bad.as_ptr(), 80, &mut req4) };
    assert_eq!(rc, 1);
    let (_, error) = wait_completion(req4, "invalid name");
    assert_ne!(error, 0, "unresolvable name must complete with an EAI error");
    // SAFETY: release the failed-request handout.
    unsafe { bao_uloop::Bun__addrinfo_freeRequest(req4, 0) };

    // ── registerQuic2: notify fires exactly once (pending queue or
    //    already-completed immediate delivery); no-notify arms stay silent ──
    QUIC_NOTIFIED.store(0, Ordering::SeqCst);
    let hit_host = CString::new("uloop-edge-hit.test").unwrap();
    let mut req5: *mut c_void = std::ptr::null_mut();
    let rc = unsafe {
        bao_uloop::Bun__addrinfo_get(std::ptr::null_mut(), hit_host.as_ptr(), 80, &mut req5)
    };
    assert_eq!(rc, 0);
    let pc = 0x5150usize as *mut c_void;
    unsafe {
        bao_uloop::Bun__addrinfo_registerQuic2(req5, pc, None); // no notify → no-op
        bao_uloop::Bun__addrinfo_registerQuic(req5, pc); // legacy C-ABI → no-op
        bao_uloop::Bun__addrinfo_registerQuic2(req5, pc, Some(quic_notify));
    }
    unsafe { bao_uloop::Bun__addrinfo_freeRequest(req5, 0) };
    assert_eq!(QUIC_NOTIFIED.load(Ordering::SeqCst), 1, "notify exactly once");
    assert_eq!(QUIC_PC.load(Ordering::SeqCst), pc as usize, "payload passed through");

    // ── null-request guards: every entry point tolerates a null req ──
    let mut out: *mut c_void = std::ptr::null_mut();
    let rc =
        unsafe { bao_uloop::Bun__addrinfo_get(std::ptr::null_mut(), host.as_ptr(), 80, &mut out) };
    assert_eq!(rc, 0, "guard-block get is a cache hit");
    assert_eq!(unsafe { bao_uloop::Bun__addrinfo_set(std::ptr::null_mut(), pc) }, 0);
    assert_eq!(unsafe { bao_uloop::Bun__addrinfo_cancel(std::ptr::null_mut(), pc) }, 0);
    assert!(unsafe { bao_uloop::Bun__addrinfo_getRequestResult(std::ptr::null_mut()) }.is_null());
    unsafe {
        bao_uloop::Bun__addrinfo_freeRequest(std::ptr::null_mut(), 0);
        bao_uloop::Bun__addrinfo_registerQuic2(std::ptr::null_mut(), pc, Some(quic_notify));
    }
    assert_eq!(QUIC_NOTIFIED.load(Ordering::SeqCst), 1, "null req must not notify");
    // SAFETY: out holds the live completed-hit request from the guard get.
    unsafe { bao_uloop::Bun__addrinfo_freeRequest(out, 0) };
}

#[test]
fn uloop_lib_entry_points_and_poll_accessors() {
    use bao_uloop::poll::{
        BaoPoll, POLL_TYPE_CALLBACK, POLL_TYPE_POLLING_IN, POLL_TYPE_POLLING_OUT, POLL_TYPE_SOCKET,
    };

    // ── lib.rs entry points: cover both null and real-argument arms ──
    bao_uloop::force_link();
    bao_uloop::poll::force_link_poll();
    unsafe {
        bao_uloop::Bun__internal_ensureDateHeaderTimerIsEnabled(std::ptr::null_mut());
        bao_uloop::BUN__warn__extra_ca_load_failed(std::ptr::null(), std::ptr::null());
        let name = CString::new("extra.pem").unwrap();
        let err = CString::new("no such file").unwrap();
        bao_uloop::BUN__warn__extra_ca_load_failed(name.as_ptr(), err.as_ptr());
        bao_uloop::bun_ssl_ctx_cache_on_free(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            0,
            std::ptr::null_mut(),
        );
        bao_uloop::us_dispatch_keylog(std::ptr::null_mut(), std::ptr::null(), 0);
        bao_uloop::us_dispatch_session(std::ptr::null_mut(), std::ptr::null(), 0);
        let probe = 0x1000usize as *mut c_void;
        assert_eq!(
            bao_uloop::us_dispatch_ssl_raw_tap(probe, std::ptr::null_mut(), 0),
            probe,
            "ssl_raw_tap returns the socket unchanged"
        );
    }

    // ── BaoPoll bitfield accessors (zeroed is a valid fd-0/untyped poll) ──
    // SAFETY: BaoPoll is a plain u32 bitfield holder — all-zero is valid.
    let mut poll: BaoPoll = unsafe { std::mem::zeroed() };
    assert_eq!(poll.fd(), 0);
    assert_eq!(poll.poll_type(), 0);

    poll.set_fd(5);
    assert_eq!(poll.fd(), 5);
    poll.set_fd(-3);
    assert_eq!(poll.fd(), -3, "fd is a signed 27-bit field — must sign-extend");

    poll.set_poll_type(POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN);
    assert_eq!(poll.poll_type(), POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN);
    assert_eq!(poll.kind(), POLL_TYPE_CALLBACK, "kind masks the direction bits");
    assert_eq!(
        poll.events(),
        libc::EPOLLIN,
        "POLLING_IN decodes to the libus readable bit"
    );

    poll.set_poll_type(POLL_TYPE_POLLING_IN | POLL_TYPE_POLLING_OUT);
    assert_eq!(poll.events(), libc::EPOLLIN | libc::EPOLLOUT);
    poll.set_poll_type(POLL_TYPE_POLLING_OUT);
    assert_eq!(poll.events(), libc::EPOLLOUT);
    poll.set_poll_type(POLL_TYPE_SOCKET);
    assert_eq!(poll.events(), 0, "unarmed poll decodes to no interest bits");

    // ext() points at the trailing extension area (one 16-byte header past).
    let base = std::ptr::addr_of!(poll);
    assert_eq!(
        poll.ext() as usize,
        base as usize + std::mem::size_of::<BaoPoll>(),
        "ext must be the trailing bytes after the 16-byte header"
    );
    // SAFETY: us_poll_ext is the same offset arithmetic over a live poll.
    let ext = unsafe { bao_uloop::poll::us_poll_ext(std::ptr::addr_of_mut!(poll)) };
    assert_eq!(ext as usize, base as usize + std::mem::size_of::<BaoPoll>());
}
