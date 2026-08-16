//! `Bun__addrinfo_*` — the usockets DNS seam, backed by the shared per-host
//! cache (`bun_dns::cache`) plus a blocking `getaddrinfo` worker.
//!
//! # Contract (mirrors upstream Bun's `dns_jsc::internal`, Linux path)
//!
//! The C side (packages/bun-usockets/src/context.c, quic.c) drives this ABI:
//!
//! * `Bun__addrinfo_get(loop, host, port, &req)` — consults the shared cache.
//!   Returns **0** when a completed result is ready (`req` valid, result
//!   readable via `getRequestResult`), **1** when a fresh/inflight request
//!   was started (`req` valid; completion arrives via callbacks). It never
//!   returns -1 — context.c stores `req` unconditionally, so a miss here must
//!   still hand back a live request. (The previous stub returned -1 and left
//!   `ai_req` uninitialized in C, which made every hostname connect through
//!   usockets undefined behavior.)
//! * `Bun__addrinfo_set(req, socket)` — registers a `us_connecting_socket_t`.
//!   If the result is already complete the socket is notified immediately
//!   (same-thread `us_internal_dns_callback`).
//! * `Bun__addrinfo_cancel(req, socket)` — 1 if the socket was removed before
//!   completion (caller then owns teardown + `freeRequest`), 0 if the result
//!   is already set (callback fired or queued; socket.c keeps the socket
//!   alive until `us_internal_socket_after_resolve` finishes it).
//! * `Bun__addrinfo_freeRequest(req, err)` — releases one reference.
//! * `Bun__addrinfo_getRequestResult(req)` — `*addrinfo_result` (stable
//!   address inside the request; entries form an `ai_next` chain).
//! * `Bun__addrinfo_registerQuic2(req, pc, notify)` — QUIC variant of `set`
//!   with an explicit completion callback (see `bun_dns::internal`).
//!
//! # Resolution backend + TTL honesty
//!
//! POSIX `getaddrinfo` exposes no TTL, so worker-produced entries enter the
//! shared cache with the engine's cap (`BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS`,
//! default 30 s) — the same lifetime upstream Bun applies to its Linux
//! `getaddrinfo` WorkPool cache. Upstream also retries once without
//! `AI_ADDRCONFIG` on `EAI_NONAME`; both behaviors are mirrored here.
//!
//! Upstream keeps completed `Request`s in its own GlobalCache and invalidates
//! them when a connect fails (`freeRequest(err=1)`). Here the completed-result
//! cache is the shared `bun_dns` layer and a failed *connect* does not evict:
//! a refused/RST connection says nothing about DNS correctness, entries are
//! bounded by the TTL cap either way, and cross-stack invalidation on routine
//! connect failures would thrash the shared cache the other paths rely on.
//!
//! # Reference counting
//!
//! A new request starts with refs = 3: one per `Bun__addrinfo_get` handout
//! (released by `freeRequest`), one for the resolver worker, one for the
//! inflight map. Cache-hit-completed requests start with refs = 1. Inflight
//! dedupe bumps refs per extra handout. When refs reaches 0 the request box
//! is freed — this happens either in `freeRequest` or in `complete`,
//! whichever releases the last reference.

use std::collections::HashMap;
use std::ffi::CStr;
use std::ffi::CString;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::thread;

use bun_dns::cache::{self as dns_cache, IpAddr};

use core::ffi::c_char;
use core::ffi::c_int;
use core::ffi::c_void;

// ─────────────── C-layout result types (internal/internal.h) ───────────────
//
// `struct addrinfo_result_entry { struct addrinfo info; struct sockaddr_storage _storage; }`
// `struct addrinfo_result { struct addrinfo_result_entry* entries; int error; }`

#[repr(C)]
struct AddrInfoResultEntry {
    info: libc::addrinfo,
    storage: libc::sockaddr_storage,
}

#[repr(C)]
struct AddrInfoResult {
    entries: *mut AddrInfoResultEntry,
    error: c_int,
}

/// Who to tell when the request completes.
enum Notify {
    /// `*mut us_connecting_socket_t` (context.c async path).
    Socket(*mut c_void),
    /// QUIC pending-connect handle + its threadsafe completion callback.
    Quic {
        pc: *mut c_void,
        notify: unsafe extern "C" fn(*mut c_void),
    },
}

struct ReqState {
    /// Manual refcount: 1 per `Bun__addrinfo_get` handout (balanced by
    /// `freeRequest`), +1 for the resolver worker, +1 while the request sits
    /// in the inflight map. Zero → the Box is freed.
    refs: usize,
    /// Set (under `state`'s lock) once resolution finishes. `set`/`cancel`
    /// consult this to pick between immediate-notify and queue.
    completed: bool,
    host: CString,
    port: u16,
    notify: Vec<Notify>,
    /// getaddrinfo error code (0 = success).
    error: c_int,
    /// Completed entries; `result_c.entries` points at this Vec's buffer.
    entries_buf: Vec<AddrInfoResultEntry>,
}

/// Opaque `struct addrinfo_request`. `result_c` must stay at a fixed offset:
/// C reads it through the pointer returned by `getRequestResult`.
struct Request {
    result_c: AddrInfoResult,
    state: Mutex<ReqState>,
}

// SAFETY: the raw pointers inside are usockets handles; Notify::Socket is
// only touched on the loop thread and only through the C notify entry points
// (which are the same call the upstream Zig code makes cross-thread). The
// request's shared mutable state is behind `state`'s mutex.
unsafe impl Send for Request {}
unsafe impl Sync for Request {}

/// Raw request handle wrapped for cross-thread storage/transfer. Sound
/// because requests are hand-refcounted: whoever holds a `RequestPtr` owns
/// one reference (map membership / worker transfer), so the pointee outlives
/// every wrapper.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RequestPtr(*mut Request);
// SAFETY: see type docs.
unsafe impl Send for RequestPtr {}
// SAFETY: see type docs.
unsafe impl Sync for RequestPtr {}

/// Inflight (worker-outstanding) requests, keyed by the shared-cache key.
/// Map membership is one refs count.
static INFLIGHT: OnceLock<Mutex<HashMap<Box<str>, RequestPtr>>> = OnceLock::new();

fn inflight() -> &'static Mutex<HashMap<Box<str>, RequestPtr>> {
    INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe extern "C" {
    /// Same-thread DNS completion: queues `c` on the loop's dns_ready_head.
    /// Provided by libusockets.a (loop.c).
    unsafe fn us_internal_dns_callback(c: *mut c_void, req: *mut c_void);
    /// Cross-thread DNS completion: queues + wakes the loop.
    /// Provided by libusockets.a (loop.c).
    unsafe fn us_internal_dns_callback_threadsafe(c: *mut c_void, req: *mut c_void);
}

// ───────────────────────────── entry points ─────────────────────────────

/// Query the shared cache / start resolution. See module docs for the
/// return-value contract.
///
/// SAFETY: `host` must be NUL-terminated and live for the call; `ptr` must be
/// a valid out-pointer. The returned request must eventually reach
/// `Bun__addrinfo_freeRequest` exactly once per handout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_get(
    _loop: *mut c_void,
    host: *const c_char,
    port: u16,
    ptr: *mut *mut c_void,
) -> c_int {
    // SAFETY: caller contract (context.c/quic.c pass NUL-terminated hosts).
    let host = unsafe { CStr::from_ptr(host) };
    let mut completed_hit = false;
    let req = get_or_start(host, port, &mut completed_hit);
    // SAFETY: `ptr` is a valid out-pointer per the caller contract.
    unsafe { *ptr = req.cast::<c_void>() };
    if completed_hit { 0 } else { 1 }
}

/// Shared-cache lookup → completed request; miss → inflight dedupe or spawn.
fn get_or_start(host: &CStr, port: u16, completed_hit: &mut bool) -> *mut Request {
    if let Some(addrs) = dns_cache::lookup(host.to_bytes()) {
        *completed_hit = true;
        return Request::completed(addrs, port);
    }
    let key = dns_cache_key(host);
    {
        let inflight = inflight().lock().unwrap();
        if let Some(&RequestPtr(req)) = inflight.get(&key) {
            // The inflight map holds a reference, so the request is alive;
            // refs is bumped under its lock (worker completion cannot free
            // it while the map entry exists).
            let mut st = unsafe { (*req).state.lock().unwrap() };
            st.refs += 1;
            return req;
        }
    }
    let req = new_inflight(host, port);
    inflight()
        .lock()
        .unwrap()
        .insert(key.clone(), RequestPtr(req));

    // RequestPtr (not the bare pointer) so the closure captures a Send
    // wrapper; passed whole into resolve_worker to force whole-struct capture.
    let worker_req = RequestPtr(req);
    let spawned = thread::Builder::new()
        .name("bao-dns-resolve".into())
        .spawn(move || resolve_worker(worker_req, key));
    if spawned.is_err() {
        // No thread available: fail the request now. complete() releases the
        // worker+map refs and notifies any waiters.
        inflight().lock().unwrap().remove(&dns_cache_key(host));
        let notify = complete(req, Vec::new(), libc::EAGAIN as c_int);
        notify_all(notify);
    }
    req
}

/// Fresh unresolved request (refs = 3: C handout + worker + inflight map).
/// Callers must add it to the inflight map themselves; tests use this to
/// exercise the lifecycle without spawning a real resolver thread.
fn new_inflight(host: &CStr, port: u16) -> *mut Request {
    Box::into_raw(Box::new(Request {
        result_c: AddrInfoResult {
            entries: core::ptr::null_mut(),
            error: 0,
        },
        state: Mutex::new(ReqState {
            refs: 3,
            completed: false,
            host: host.to_owned(),
            port,
            notify: Vec::new(),
            error: 0,
            entries_buf: Vec::new(),
        }),
    }))
}

/// SAFETY: `req` must be a live request handed out by `Bun__addrinfo_get`,
/// and `socket` a live `us_connecting_socket_t` on this thread's loop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_set(req: *mut c_void, socket: *mut c_void) -> c_int {
    // SAFETY: live request per caller contract.
    let Some(r) = (unsafe { (req as *const Request).as_ref() }) else {
        return 0;
    };
    let notify_now = {
        let mut st = r.state.lock().unwrap();
        if st.completed {
            Some(Notify::Socket(socket))
        } else {
            st.notify.push(Notify::Socket(socket));
            None
        }
    };
    if notify_now.is_some() {
        // Same thread as the connecting socket (set is called from
        // us_socket_group_connect) — no wakeup needed.
        // SAFETY: `socket` is a live us_connecting_socket_t per the caller.
        unsafe { us_internal_dns_callback(socket, req) };
    }
    0
}

/// SAFETY: `req` must be a live request; `socket` a live connecting socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_cancel(req: *mut c_void, socket: *mut c_void) -> c_int {
    // SAFETY: live request per caller contract.
    let Some(r) = (unsafe { (req as *const Request).as_ref() }) else {
        return 0;
    };
    let mut st = r.state.lock().unwrap();
    // Once completed, the callback has fired or is queued; the socket stays
    // alive until after_resolve drains it (socket.c relies on exactly this
    // two-outcome contract).
    if st.completed {
        return 0;
    }
    let before = st.notify.len();
    st.notify
        .retain(|n| !matches!(n, Notify::Socket(s) if *s == socket));
    (st.notify.len() != before) as c_int
}

/// SAFETY: `req` must be a live request with exactly one outstanding handout
/// being released by this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_freeRequest(req: *mut c_void, _error: c_int) {
    // SAFETY: live request per caller contract.
    let Some(r) = (unsafe { (req as *const Request).as_ref() }) else {
        return;
    };
    let mut st = r.state.lock().unwrap();
    st.refs -= 1;
    if st.refs == 0 {
        drop(st);
        // SAFETY: refs hit zero — no handouts, no worker, no map entry.
        drop(unsafe { Box::from_raw(r as *const Request as *mut Request) });
    }
}

/// SAFETY: `req` must be a live, completed request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_getRequestResult(req: *mut c_void) -> *mut c_void {
    // SAFETY: live request per caller contract.
    let Some(r) = (unsafe { (req as *const Request).as_ref() }) else {
        return core::ptr::null_mut();
    };
    (&raw const r.result_c) as *const AddrInfoResult as *mut AddrInfoResult as *mut c_void
}

/// QUIC registration with explicit threadsafe completion callback.
///
/// SAFETY: `req` must be a live request; `pc` a live h3 PendingConnect that
/// stays valid until `notify` fires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_registerQuic2(
    req: *mut c_void,
    pc: *mut c_void,
    notify: Option<unsafe extern "C" fn(*mut c_void)>,
) {
    let Some(cb) = notify else {
        return;
    };
    // SAFETY: live request per caller contract.
    let Some(r) = (unsafe { (req as *const Request).as_ref() }) else {
        return;
    };
    let notify_now = {
        let mut st = r.state.lock().unwrap();
        if st.completed {
            Some(Notify::Quic { pc, notify: cb })
        } else {
            st.notify.push(Notify::Quic { pc, notify: cb });
            None
        }
    };
    if let Some(Notify::Quic { pc, notify }) = notify_now {
        // registerQuic2 runs on the HTTP thread; the h3 callback is the
        // threadsafe variant, which is also correct same-thread.
        // SAFETY: `pc` is live per the caller contract.
        unsafe { notify(pc) };
    }
}

/// Legacy C-ABI registration kept for the internal.h declaration; no live C
/// caller (the Rust h3 client uses `Bun__addrinfo_registerQuic2` via
/// `bun_dns::internal::register_quic`). Without a callback there is no way
/// to deliver completion, so this is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_registerQuic(_req: *mut c_void, _pc: *mut c_void) {}

// ───────────────────────────── resolution ─────────────────────────────

fn dns_cache_key(host: &CStr) -> Box<str> {
    // Mirrors bun_dns::cache's key normalization (ASCII-lowercase).
    String::from_utf8_lossy(&host.to_bytes().to_ascii_lowercase()).into()
}

fn resolve_worker(worker_req: RequestPtr, key: Box<str>) {
    let RequestPtr(req) = worker_req;
    let (host, port) = {
        let st = unsafe { (*req).state.lock().unwrap() };
        (st.host.clone(), st.port)
    };
    let (entries, addrs, error) = resolve_getaddrinfo(&host, port);
    if error == 0 && !addrs.is_empty() {
        dns_cache::insert(host.to_bytes(), addrs, None);
    }
    inflight().lock().unwrap().remove(&key);
    let notify = complete(req, entries, error);
    notify_all(notify);
}

/// Blocking resolution on this worker thread. Returns the C entries (built as
/// an interleaved `ai_next` chain, matching upstream `processResults`), the
/// raw address list (for the shared cache), and a getaddrinfo error code
/// (0 = success; EAI_* otherwise).
fn resolve_getaddrinfo(host: &CStr, port: u16) -> (Vec<AddrInfoResultEntry>, Vec<IpAddr>, c_int) {
    let mut hints: libc::addrinfo = unsafe { core::mem::zeroed() };
    hints.ai_family = libc::AF_UNSPEC;
    hints.ai_socktype = libc::SOCK_STREAM;
    hints.ai_flags = libc::AI_ADDRCONFIG;

    let mut result: *mut libc::addrinfo = core::ptr::null_mut();
    // SAFETY: host is NUL-terminated; hints/result are valid pointers.
    let mut rc =
        unsafe { libc::getaddrinfo(host.as_ptr(), core::ptr::null(), &hints, &mut result) };

    // Upstream retries once without AI_ADDRCONFIG on EAI_NONAME (an
    // IPv6-only box can make ADDRCONFIG suppress the only usable family).
    if rc == libc::EAI_NONAME {
        hints.ai_flags &= !libc::AI_ADDRCONFIG;
        // SAFETY: same as above.
        rc = unsafe { libc::getaddrinfo(host.as_ptr(), core::ptr::null(), &hints, &mut result) };
    }

    if rc != 0 || result.is_null() {
        if !result.is_null() {
            // SAFETY: result was allocated by getaddrinfo.
            unsafe { libc::freeaddrinfo(result) };
        }
        return (Vec::new(), Vec::new(), rc);
    }

    // SAFETY: result is a live chain allocated by getaddrinfo.
    let (mut entries, mut addrs) = unsafe { collect_entries(result, port) };
    // SAFETY: done with the chain.
    unsafe { libc::freeaddrinfo(result) };
    interleave_families(&mut entries, &mut addrs);
    link_chain(&mut entries);
    (entries, addrs, 0)
}

/// Copy a getaddrinfo chain into C-layout entries + raw cache addrs.
///
/// SAFETY: `head` must be a live getaddrinfo result chain.
unsafe fn collect_entries(
    head: *mut libc::addrinfo,
    port: u16,
) -> (Vec<AddrInfoResultEntry>, Vec<IpAddr>) {
    let mut entries: Vec<AddrInfoResultEntry> = Vec::new();
    let mut addrs: Vec<IpAddr> = Vec::new();
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: cur is non-null and points into the result chain.
        let ai = unsafe { &*cur };
        if !ai.ai_addr.is_null() {
            let mut entry = AddrInfoResultEntry {
                // SAFETY: plain-old-data copy of the chain node; the
                // ai_next/ai_addr/ai_canonname pointers it carries are
                // overwritten below / in link_chain before use.
                info: unsafe { core::ptr::read(ai) },
                storage: unsafe { core::mem::zeroed() },
            };
            // SAFETY: ai_addr is non-null and valid for ai_addrlen.
            if let Some(ip) = unsafe { copy_sockaddr(ai, &mut entry, port) } {
                addrs.push(ip);
                entries.push(entry);
            }
        }
        cur = ai.ai_next;
    }
    (entries, addrs)
}

/// Copy `ai.ai_addr` into `entry.storage`, point `entry.info.ai_addr` at it,
/// apply `port`, and return the raw address.
///
/// SAFETY: `ai.ai_addr` must be non-null and valid for `ai.ai_addrlen`, and
/// `ai.ai_family` must accurately describe it.
unsafe fn copy_sockaddr(
    ai: &libc::addrinfo,
    entry: &mut AddrInfoResultEntry,
    port: u16,
) -> Option<IpAddr> {
    match ai.ai_family {
        libc::AF_INET => {
            // SAFETY: family-checked; sockaddr_in fits sockaddr_storage.
            let src = unsafe { &*(ai.ai_addr as *const libc::sockaddr_in) };
            // SAFETY: storage is zeroed and at least sockaddr_in-sized.
            let dst = unsafe { &mut *((&raw mut entry.storage).cast::<libc::sockaddr_in>()) };
            dst.sin_family = libc::AF_INET as libc::sa_family_t;
            dst.sin_port = port.to_be();
            dst.sin_addr = src.sin_addr;
            entry.info.ai_family = libc::AF_INET;
            entry.info.ai_addrlen = core::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
            // ai_addr is NOT set here: this entry is a stack local that gets
            // moved into the Vec by value — a self-pointer assigned now would
            // dangle with the frame. link_chain re-points it at the entry's
            // own storage inside the final Vec (see its BCE note).
            Some(IpAddr::V4(src.sin_addr.s_addr.to_ne_bytes()))
        }
        libc::AF_INET6 => {
            // SAFETY: family-checked; sockaddr_in6 fits sockaddr_storage.
            let src = unsafe { &*(ai.ai_addr as *const libc::sockaddr_in6) };
            // SAFETY: storage is zeroed and at least sockaddr_in6-sized.
            let dst = unsafe { &mut *((&raw mut entry.storage).cast::<libc::sockaddr_in6>()) };
            dst.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            dst.sin6_port = port.to_be();
            dst.sin6_addr = src.sin6_addr;
            entry.info.ai_family = libc::AF_INET6;
            entry.info.ai_addrlen = core::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;
            // ai_addr intentionally not set — see the AF_INET arm's note.
            Some(IpAddr::V6(src.sin6_addr.s6_addr))
        }
        _ => None,
    }
}

/// Interleave v6/v4 starting with IPv6, in lockstep across both parallel
/// vectors (upstream `processResults` ordering — `start_connections` then
/// opens family-balanced candidate sockets).
fn interleave_families(entries: &mut [AddrInfoResultEntry], addrs: &mut [IpAddr]) {
    debug_assert_eq!(entries.len(), addrs.len());
    let mut want = libc::AF_INET6;
    for idx in 0..entries.len() {
        if entries[idx].info.ai_family == want {
            want = other_family(want);
            continue;
        }
        for j in idx + 1..entries.len() {
            if entries[j].info.ai_family == want {
                entries.swap(idx, j);
                addrs.swap(idx, j);
                want = other_family(want);
                break;
            }
        }
    }
}

fn other_family(f: c_int) -> c_int {
    if f == libc::AF_INET6 {
        libc::AF_INET
    } else {
        libc::AF_INET6
    }
}

/// Point each entry's `ai_next` at the following entry and drop the stale
/// canonname pointer copied from the source chain.
///
/// BCE (hostname-connect hang): every producer (`collect_entries`,
/// `entry_from_ip`) builds an entry as a stack local and only then moves it
/// (by value) into the Vec. The `ai_addr` stored there points at the
/// *stack-frame* storage — dangling once the frame dies — so C's
/// `init_addr_with_port` memcpy'd 16 bytes of stack residue into the connect
/// address (`0.0.0.0`, `AF_UNSPEC`, …) and every hostname connect through
/// usockets hung or misdialed. IP literals never see this seam (C
/// `try_parse_ip` short-circuits before `Bun__addrinfo_get`), which is why
/// only hostname URLs hung. Re-point `ai_addr` at each entry's *own* storage
/// in the final Vec, here, the single mandatory tail of both producer paths —
/// the authoritative redirect, exactly like the `ai_next` fixups below.
fn link_chain(entries: &mut [AddrInfoResultEntry]) {
    let len = entries.len();
    let base = entries.as_mut_ptr();
    for idx in 0..len {
        // SAFETY: idx < len; single mutable access per iteration, and the
        // pointers stored into ai_next/ai_addr are raw (no borrow carried
        // across).
        let entry = unsafe { &mut *base.add(idx) };
        entry.info.ai_canonname = core::ptr::null_mut();
        entry.info.ai_addr = core::ptr::addr_of_mut!(entry.storage).cast();
        if idx + 1 < len {
            entry.info.ai_next = core::ptr::addr_of_mut!(entry.info);
            // SAFETY: idx + 1 < len.
            entry.info.ai_next = unsafe { core::ptr::addr_of_mut!((*base.add(idx + 1)).info) };
        } else {
            entry.info.ai_next = core::ptr::null_mut();
        }
    }
}

/// Mark the request completed and release the worker+map references.
/// Returns the notify list (moved out under the same lock — once `completed`
/// is visible, cancel can no longer remove anyone) and frees the request if
/// that released the last reference.
fn complete(req: *mut Request, entries: Vec<AddrInfoResultEntry>, error: c_int) -> Vec<Notify> {
    let mut st = unsafe { (*req).state.lock().unwrap() };
    st.completed = true;
    st.error = error;
    st.entries_buf = entries;
    // Single writer (the resolver side) publishes the C-facing result before
    // completion is delivered; the loop thread reads it only after the
    // callback/wakeup handshake, which orders the writes in practice (same
    // discipline as upstream's afterResult under the global cache lock).
    let entries_ptr = if st.entries_buf.is_empty() {
        core::ptr::null_mut()
    } else {
        st.entries_buf.as_mut_ptr()
    };
    unsafe {
        (*req).result_c.entries = entries_ptr;
        (*req).result_c.error = error;
    }
    let notify = core::mem::take(&mut st.notify);
    st.refs -= 2;
    if st.refs == 0 {
        drop(st);
        // SAFETY: refs hit zero — no handouts remain, worker+map released.
        drop(unsafe { Box::from_raw(req) });
    }
    notify
}

impl Request {
    /// Build an already-completed request from a shared-cache hit (refs = 1:
    /// the C handout). Not placed in the inflight map — there is no work.
    fn completed(addrs: Vec<IpAddr>, port: u16) -> *mut Request {
        let mut entries: Vec<AddrInfoResultEntry> =
            addrs.iter().map(|ip| entry_from_ip(ip, port)).collect();
        link_chain(&mut entries);
        let entries_ptr = if entries.is_empty() {
            core::ptr::null_mut()
        } else {
            entries.as_mut_ptr()
        };
        Box::into_raw(Box::new(Request {
            result_c: AddrInfoResult {
                entries: entries_ptr,
                error: 0,
            },
            state: Mutex::new(ReqState {
                refs: 1,
                completed: true,
                host: CString::default(),
                port,
                notify: Vec::new(),
                error: 0,
                entries_buf: entries,
            }),
        }))
    }
}

/// Deliver completions. Called from the resolver worker (threadsafe Cb) or,
/// for already-completed registrations, from the registering thread.
fn notify_all(notify: Vec<Notify>) {
    for owner in notify {
        match owner {
            Notify::Socket(socket) => {
                // SAFETY: the usockets invariant — a registered connecting
                // socket stays alive until its callback drains (cancel either
                // removed it before we took the list, or socket.c left it
                // alive for after_resolve). The req argument is unused by
                // loop.c.
                unsafe { us_internal_dns_callback_threadsafe(socket, core::ptr::null_mut()) };
            }
            Notify::Quic { pc, notify } => {
                // SAFETY: h3's PendingConnect stays alive until this fires.
                unsafe { notify(pc) };
            }
        }
    }
}

/// Build one C entry from a raw cache address (port applied; authoritative
/// for display only — context.c re-applies the real port via
/// `init_addr_with_port` before connecting).
fn entry_from_ip(ip: &IpAddr, port: u16) -> AddrInfoResultEntry {
    let mut entry = AddrInfoResultEntry {
        info: unsafe { core::mem::zeroed() },
        storage: unsafe { core::mem::zeroed() },
    };
    match ip {
        IpAddr::V4(octets) => {
            // SAFETY: storage is zeroed and at least sockaddr_in-sized.
            let dst = unsafe { &mut *((&raw mut entry.storage).cast::<libc::sockaddr_in>()) };
            dst.sin_family = libc::AF_INET as libc::sa_family_t;
            dst.sin_port = port.to_be();
            dst.sin_addr.s_addr = u32::from_ne_bytes(*octets);
            entry.info.ai_family = libc::AF_INET;
            entry.info.ai_socktype = libc::SOCK_STREAM;
            entry.info.ai_protocol = libc::IPPROTO_TCP;
            entry.info.ai_addrlen = core::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        }
        IpAddr::V6(octets) => {
            // SAFETY: storage is zeroed and at least sockaddr_in6-sized.
            let dst = unsafe { &mut *((&raw mut entry.storage).cast::<libc::sockaddr_in6>()) };
            dst.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            dst.sin6_port = port.to_be();
            dst.sin6_addr.s6_addr = *octets;
            entry.info.ai_family = libc::AF_INET6;
            entry.info.ai_socktype = libc::SOCK_STREAM;
            entry.info.ai_protocol = libc::IPPROTO_TCP;
            entry.info.ai_addrlen = core::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;
        }
    }
    // ai_addr intentionally not set here (stack-local entry moved by value):
    // link_chain re-points it at the entry's own storage inside the Vec.
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cache-flood isolation: the shared bun_dns cache is process-global.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// completed-from-cache requests must expose a readable C result with a
    /// correct single-entry chain (context.c's immediate-connect fast path).
    #[test]
    fn completed_request_result_layout() {
        let _guard = TEST_LOCK.lock().unwrap();
        dns_cache::insert(b"layout.test", vec![IpAddr::V4([127, 0, 0, 1])], Some(60));
        let mut hit = false;
        let req = get_or_start(
            CString::new("layout.test").unwrap().as_c_str(),
            8080,
            &mut hit,
        );
        assert!(hit);
        // SAFETY: req is a live handout.
        let result = unsafe { Bun__addrinfo_getRequestResult(req as *mut c_void) };
        assert!(!result.is_null());
        // SAFETY: result points at the request's result_c.
        let result = unsafe { &*(result as *const AddrInfoResult) };
        assert_eq!(result.error, 0);
        assert!(!result.entries.is_null());
        // SAFETY: entries points at entries_buf[0] (chain of one).
        let entry = unsafe { &*result.entries };
        assert!(entry.info.ai_next.is_null());
        assert_eq!(entry.info.ai_family, libc::AF_INET);
        // BCE (hostname-connect hang) regression lock: ai_addr must point at
        // THIS entry's storage inside the request-owned Vec — not at a dead
        // producer stack frame. Pointer identity, plus the content it yields.
        assert!(core::ptr::eq(
            entry.info.ai_addr.cast::<u8>(),
            (&raw const entry.storage).cast::<u8>()
        ));
        // SAFETY: ai_addr points at entry.storage (identity-checked above).
        let sa = unsafe { &*(entry.info.ai_addr as *const libc::sockaddr_in) };
        assert_eq!(sa.sin_addr.s_addr, u32::from_ne_bytes([127, 0, 0, 1]));
        assert_eq!(sa.sin_port, 8080u16.to_be());
        // SAFETY: release the handout (single ref → freed).
        unsafe { Bun__addrinfo_freeRequest(req as *mut c_void, 0) };
    }

    /// BCE (hostname-connect hang) regression lock, multi-entry worker path:
    /// after collect→interleave→link_chain every entry's ai_addr must point
    /// at its own storage in the final Vec and dereference to the matching
    /// address — the invariant C's `init_addr_with_port` memcpy relies on.
    #[test]
    fn link_chain_points_ai_addr_at_own_storage() {
        let mut entries = vec![
            entry_from_ip(&IpAddr::V4([10, 0, 0, 1]), 80),
            entry_from_ip(&IpAddr::V6([0x20; 16]), 443),
        ];
        link_chain(&mut entries);
        for entry in &mut entries {
            assert!(
                core::ptr::eq(
                    entry.info.ai_addr.cast::<u8>(),
                    (&raw const entry.storage).cast::<u8>()
                ),
                "ai_addr must point at the entry's own Vec storage, not a producer stack frame"
            );
        }
        // Content survives through the published pointers: v4 first.
        // SAFETY: identity-checked above; family-checked here.
        let sa4 = unsafe { &*(entries[0].info.ai_addr as *const libc::sockaddr_in) };
        assert_eq!(sa4.sin_addr.s_addr, u32::from_ne_bytes([10, 0, 0, 1]));
        assert_eq!(sa4.sin_port, 80u16.to_be());
        // SAFETY: same for the v6 entry.
        let sa6 = unsafe { &*(entries[1].info.ai_addr as *const libc::sockaddr_in6) };
        assert_eq!(sa6.sin6_addr.s6_addr, [0x20; 16]);
        assert_eq!(sa6.sin6_port, 443u16.to_be());
    }

    /// A miss must hand back a live request (never null / never leave the C
    /// out-pointer untouched) and register it inflight so a concurrent get
    /// dedupes onto the same request. Uses `new_inflight` directly so no real
    /// resolver thread spawns; the dedupe branch of `get_or_start` is then
    /// exercised through the map entry.
    #[test]
    fn miss_creates_inflight_request() {
        let _guard = TEST_LOCK.lock().unwrap();
        let host = CString::new("definitely-miss-1.test").unwrap();
        let req = new_inflight(host.as_c_str(), 443);
        assert!(!req.is_null());
        let key = dns_cache_key(&host);
        inflight()
            .lock()
            .unwrap()
            .insert(key.clone(), RequestPtr(req));
        {
            let inflight = inflight().lock().unwrap();
            assert!(inflight.contains_key(&key));
        }
        // get while inflight → same request via the dedupe branch, refs
        // bumped, no worker spawned (map entry exists). Callers initialize
        // `completed_hit` to false (Bun__addrinfo_get does); the dedupe
        // branch leaves it untouched.
        let mut hit = false;
        let req2 = get_or_start(host.as_c_str(), 443, &mut hit);
        assert!(!hit);
        assert_eq!(req, req2);
        // Simulate worker completion (releases worker+map refs) and drop the
        // map entry just like resolve_worker does.
        let notify = complete(req, Vec::new(), libc::EAI_NONAME);
        assert!(notify.is_empty());
        inflight().lock().unwrap().remove(&key);
        // SAFETY: two handouts out → two frees (refs 3 + 1 dedupe - 2
        // complete - 1 - 1 = 0 → freed by the second one).
        unsafe {
            Bun__addrinfo_freeRequest(req as *mut c_void, 0);
            Bun__addrinfo_freeRequest(req2 as *mut c_void, 0);
        }
    }

    /// set() on an uncompleted request registers the socket; cancel removes
    /// it and reports 1; a second cancel reports 0.
    #[test]
    fn set_and_cancel_lifecycle() {
        let _guard = TEST_LOCK.lock().unwrap();
        let host = CString::new("definitely-miss-2.test").unwrap();
        let req = new_inflight(host.as_c_str(), 443);
        let key = dns_cache_key(&host);
        inflight()
            .lock()
            .unwrap()
            .insert(key.clone(), RequestPtr(req));
        let fake_socket = 0x1000usize as *mut c_void;
        // SAFETY: req is live; the fake socket is only stored/compared, never
        // dereferenced while the request is uncompleted.
        assert_eq!(
            unsafe { Bun__addrinfo_set(req as *mut c_void, fake_socket) },
            0
        );
        assert_eq!(
            unsafe { Bun__addrinfo_cancel(req as *mut c_void, fake_socket) },
            1
        );
        assert_eq!(
            unsafe { Bun__addrinfo_cancel(req as *mut c_void, fake_socket) },
            0
        );
        // SAFETY: release handout; then simulate worker completion (frees:
        // 3 refs - 1 handout free - 2 worker/map = 0).
        unsafe { Bun__addrinfo_freeRequest(req as *mut c_void, 0) };
        let notify = complete(req, Vec::new(), libc::EAI_NONAME);
        assert!(notify.is_empty());
        inflight().lock().unwrap().remove(&key);
    }

    /// End-to-end: a real worker resolves `localhost`, the result lands in
    /// the shared cache, and a second `get` returns a completed hit without
    /// spawning another worker — the usockets path's fusion guarantee.
    #[test]
    fn end_to_end_resolve_then_cache_hit() {
        let _guard = TEST_LOCK.lock().unwrap();
        let host = CString::new("localhost").unwrap();
        // First get: miss → inflight request + real worker thread.
        let mut hit = false;
        let req = get_or_start(host.as_c_str(), 80, &mut hit);
        assert!(!hit);
        // The worker holds its refs, so `req` stays alive; wait for it.
        let mut completed = false;
        for _ in 0..1000 {
            {
                let st = unsafe { (*req).state.lock().unwrap() };
                if st.completed {
                    completed = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(completed, "resolver worker did not complete in time");
        {
            // SAFETY: completed — result_c published before completion.
            let st = unsafe { (*req).state.lock().unwrap() };
            assert_eq!(st.error, 0);
            assert!(!st.entries_buf.is_empty());
        }
        // SAFETY: release the first handout.
        unsafe { Bun__addrinfo_freeRequest(req as *mut c_void, 0) };
        // The lookup path must now hit the shared cache the worker filled.
        assert!(dns_cache::lookup(b"localhost").is_some());
        // Second get: completed hit (return code 0 contract), no new worker.
        let req2 = get_or_start(host.as_c_str(), 80, &mut hit);
        assert!(hit);
        // SAFETY: release the second handout.
        unsafe { Bun__addrinfo_freeRequest(req2 as *mut c_void, 0) };
    }

    /// interleave_families must alternate v6-first across both parallel
    /// vectors so the cache sees the same order the entries chain gets.
    #[test]
    fn interleave_starts_v6() {
        let mut entries = vec![
            entry_from_ip(&IpAddr::V4([1, 1, 1, 1]), 0),
            entry_from_ip(&IpAddr::V4([2, 2, 2, 2]), 0),
            entry_from_ip(&IpAddr::V6([0x20; 16]), 0),
        ];
        let mut addrs = vec![
            IpAddr::V4([1, 1, 1, 1]),
            IpAddr::V4([2, 2, 2, 2]),
            IpAddr::V6([0x20; 16]),
        ];
        interleave_families(&mut entries, &mut addrs);
        assert_eq!(entries[0].info.ai_family, libc::AF_INET6);
        assert_eq!(entries[1].info.ai_family, libc::AF_INET);
        assert_eq!(entries[2].info.ai_family, libc::AF_INET);
        assert!(matches!(addrs[0], IpAddr::V6(_)));
        link_chain(&mut entries);
        // SAFETY: chain links point inside the vec.
        unsafe {
            let next1 = (&raw mut entries[1].info) as *mut libc::addrinfo;
            assert_eq!(entries[0].info.ai_next, next1);
            assert!(entries[2].info.ai_next.is_null());
        }
    }
}
