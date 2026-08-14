// @trace REQ-STL-001 [entity:TlsSessionCache] TLS session resumption (stealth parity: real browsers resume)
// @trace REQ-ENG-007 [entity:TlsProfile]
//! Process-wide TLS client session cache (resumption store).
//!
//! Real Firefox/Chrome clients keep a per-origin session cache and resume
//! with a ticket (TLS 1.3 PSK / TLS 1.2 abbreviated handshake) when
//! reconnecting to the same origin. A client that always performs full
//! handshakes is a bot fingerprint signal (the `pre_shared_key` /
//! `psk_key_exchange_modes` ClientHello extensions are absent), and it also
//! pays double the handshake cost. This module gives Bao browser-parity
//! resumption semantics, shared across both TLS consumers:
//!
//! - bun_http (src/http via HTTPThread, uSockets/BoringSSL ctx)
//! - servo net (`connector.rs` → `TlsClient`/`TlsConnection`)
//!
//! # Wire semantics (ground truth: vendor/boringssl/include/openssl/ssl.h)
//!
//! - The client-side internal session cache is never used by BoringSSL;
//!   `SSL_SESS_CACHE_CLIENT` only enables the callbacks. We register a
//!   new-session callback (`SSL_CTX_sess_set_new_cb`) which fires whenever a
//!   session becomes available — for TLS 1.2 at handshake completion, for
//!   TLS 1.3 asynchronously when the post-handshake `NewSessionTicket` is
//!   processed (i.e. during a later `SSL_read`/`SSL_do_handshake` drive).
//! - Offering: `SSL_set_session(ssl, session)` must run before the handshake
//!   starts (same precondition as the SNI/ALPN setters). It up-refs
//!   internally (`ssl_set_session` in ssl/ssl_session.cc), so the caller
//!   keeps and releases its own reference.
//! - Single-use tickets: TLS 1.3 sessions must be offered at most once
//!   (`SSL_SESSION_should_be_single_use`, RFC 8446 appendix C.4) — `take`
//!   removes them from the store. TLS 1.2 sessions are multi-use and stay.
//! - Failure semantics: an expired/mismatched offered session is dropped by
//!   BoringSSL itself (`ssl_session_is_time_valid` check in
//!   ssl/handshake_client.cc:406) and the handshake falls back to full —
//!   protocol-safe, no new failure surface.
//!
//! # Origin key
//!
//! `"{profile_salt:016x}:{host}:{port}"` — host:port of the TLS endpoint
//! (proxy host:port for tunnelled connections), no path. The profile salt
//! segregates caches between TLS-parameter contexts: offering a session
//! short-circuits parameter negotiation (ssl.h warning), so stealth
//! profiles / custom `SSLConfig`s must not resume sessions established
//! under different parameters. Salt 0 = default profile, and default-profile
//! connections from both stacks share entries — the fusion payoff (page
//! fetch and Node fetch to the same site resume each other's sessions).
//!
//! # Threading
//!
//! The store is accessed from the HTTP thread, JS threads and servo's tokio
//! threads — genuine cross-thread concurrency, hence a `Mutex` (the
//! sanctioned exception to the lock-free rule). `SSL_SESSION` refcounts are
//! atomic in BoringSSL, so owned references may move between threads.

use std::collections::HashMap;
use std::ffi::{c_int, c_long, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

use bun_boringssl_sys::boringssl::{SSL, SSL_CTX, SSL_get_ex_data, SSL_set_ex_data};

/// `struct ssl_session_st` (`typedef ... SSL_SESSION`) — opaque. Not yet in
/// the hand-rolled `bun_boringssl_sys` bindings; declared here like the
/// trust-store surface in `client.rs`.
#[repr(C)]
pub struct SslSession {
    _private: [u8; 0],
}

// `#define SSL_SESS_CACHE_*` (vendor/boringssl/include/openssl/ssl.h).
pub const SSL_SESS_CACHE_OFF: c_int = 0x0000;
pub const SSL_SESS_CACHE_CLIENT: c_int = 0x0001;
pub const SSL_SESS_CACHE_SERVER: c_int = 0x0002;

// Symbols compiled into the vendored BoringSSL library but not declared in
// the hand-rolled bindings yet (same pattern as client.rs / connection.rs).
// Ground truth: vendor/boringssl/include/openssl/ssl.h.
unsafe extern "C" {
    fn SSL_CTX_set_session_cache_mode(ctx: *mut SSL_CTX, mode: c_int) -> c_int;
    fn SSL_CTX_sess_set_new_cb(
        ctx: *mut SSL_CTX,
        cb: Option<unsafe extern "C" fn(ssl: *mut SSL, session: *mut SslSession) -> c_int>,
    );
    fn SSL_set_session(ssl: *mut SSL, session: *mut SslSession) -> c_int;
    fn SSL_SESSION_up_ref(session: *mut SslSession) -> c_int;
    fn SSL_SESSION_free(session: *mut SslSession);
    fn SSL_SESSION_is_resumable(session: *const SslSession) -> c_int;
    fn SSL_SESSION_should_be_single_use(session: *const SslSession) -> c_int;
    fn SSL_session_reused(ssl: *const SSL) -> c_int;
    fn SSL_get1_session(ssl: *const SSL) -> *mut SslSession;
    // `CRYPTO_EX_unused`/`CRYPTO_EX_dup` params are opaque pointers here —
    // only `free_func` is consumed.
    fn SSL_get_ex_new_index(
        argl: c_long,
        argp: *mut c_void,
        unused: *mut c_void,
        dup_unused: *mut c_void,
        free_func: Option<
            unsafe extern "C" fn(
                parent: *mut c_void,
                ptr: *mut c_void,
                ad: *mut c_void,
                idx: c_int,
                argl: c_long,
                argp: *mut c_void,
            ),
        >,
    ) -> c_int;
}

/// Default per-process origin capacity. Firefox caps its session cache at
/// ~205 entries per origin / similar global bounds; 100 origins covers real
/// browsing sessions while bounding memory (one `SSL_SESSION` each).
pub const DEFAULT_MAX_ORIGINS: usize = 100;

/// LRU tick counter (monotonic; shared by every `ClientSessionCache`).
static TICK: AtomicU64 = AtomicU64::new(1);

fn next_tick() -> u64 {
    TICK.fetch_add(1, Ordering::Relaxed)
}

struct Entry {
    /// Owned `SSL_SESSION` reference (released on eviction/replace).
    session: *mut SslSession,
    /// Last-offer tick for LRU eviction.
    last_used: u64,
}

// SAFETY: the pointer is an owned `SSL_SESSION` reference; reference counts
// are atomic in BoringSSL (CRYPTO_REFCOUNT), so the reference may move
// between threads (`Send`), and `Sync` is fine because every access is
// serialized by the store mutex and only atomic refcount ops occur on it.
unsafe impl Send for Entry {}
unsafe impl Sync for Entry {}

/// Per-origin client session store: at most one live session per origin key
/// (browser semantics — the freshest ticket wins), bounded origin count with
/// LRU eviction.
pub struct ClientSessionCache {
    entries: Mutex<HashMap<String, Entry>>,
    capacity: usize,
}

impl Default for ClientSessionCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientSessionCache {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_ORIGINS)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity: capacity.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        // A poisoned lock means another thread panicked mid-operation; the
        // map invariants (owned refs) still hold, so recover rather than
        // poisoning every future TLS handshake over bookkeeping.
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Store `session` under `key`, taking ownership of the passed
    /// reference. Replaces (and releases) any previous session for the key.
    ///
    /// # Safety
    ///
    /// `session` must be a live `SSL_SESSION*` the caller owns (either from
    /// `SSL_get1_session` or the new-session callback) and must not be
    /// freed by the caller afterwards.
    pub unsafe fn insert(&self, key: String, session: *mut SslSession) {
        if session.is_null() {
            return;
        }
        let mut map = self.lock();
        if let Some(old) = map.insert(
            key,
            Entry {
                session,
                last_used: next_tick(),
            },
        ) {
            unsafe { SSL_SESSION_free(old.session) };
        }
        while map.len() > self.capacity {
            let evict = map
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone())
                .expect("len > capacity implies non-empty");
            if let Some(old) = map.remove(&evict) {
                unsafe { SSL_SESSION_free(old.session) };
            }
        }
    }

    /// Take a session for `key`: removes single-use (TLS 1.3) sessions,
    /// borrows (up-refs) multi-use (TLS 1.2) ones. Stale entries
    /// (unresumable) are dropped eagerly. Returns an owned reference.
    fn take_for_offer(&self, key: &str) -> Option<*mut SslSession> {
        let mut map = self.lock();
        let entry = map.get_mut(key)?;
        entry.last_used = next_tick();
        let session = entry.session;
        // SAFETY: `session` is a live owned reference in the map.
        let (resumable, single_use) = unsafe {
            (
                SSL_SESSION_is_resumable(session) == 1,
                SSL_SESSION_should_be_single_use(session) == 1,
            )
        };
        if !resumable {
            let old = map.remove(key).expect("get_mut matched");
            unsafe { SSL_SESSION_free(old.session) };
            return None;
        }
        if single_use {
            let old = map.remove(key).expect("get_mut matched");
            Some(old.session)
        } else {
            // SAFETY: up-ref before handing out an owned reference; the
            // entry keeps its own reference.
            unsafe { SSL_SESSION_up_ref(session) };
            Some(session)
        }
    }

    /// Number of cached origins (test/observability).
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `key` currently has a cached session (test/observability).
    pub fn contains_key(&self, key: &str) -> bool {
        self.lock().contains_key(key)
    }

    /// Release every cached session (test/observability).
    pub fn clear(&self) {
        let mut map = self.lock();
        for (_, old) in map.drain() {
            // SAFETY: entries hold owned references.
            unsafe { SSL_SESSION_free(old.session) };
        }
    }
}

/// The process-wide store shared by both TLS consumer stacks.
pub fn global() -> &'static ClientSessionCache {
    static GLOBAL: LazyLock<ClientSessionCache> = LazyLock::new(ClientSessionCache::new);
    &GLOBAL
}

/// Origin cache key: `"{profile_salt:016x}:{host}:{port}"`. The salt
/// segregates TLS-parameter contexts (stealth profiles, custom SSLConfigs);
/// 0 = default profile shared across stacks.
pub fn origin_key(host: &str, port: u16, profile_salt: u64) -> String {
    format!("{profile_salt:016x}:{host}:{port}")
}

/// Enable client-side session caching on `ctx`: turns on the client cache
/// bit (callbacks only — BoringSSL's client never uses the internal cache)
/// and registers the global new-session callback. Idempotent.
///
/// Only call on client (or client-capable) contexts. On dual-use contexts
/// the `SSL_SESS_CACHE_CLIENT` bit alone has no effect for accepted server
/// connections (ssl/ssl_session.cc:769 picks the bit by `SSL_is_server`).
pub fn enable_client(ctx: *mut SSL_CTX) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is a live SSL_CTX; both calls only mutate ctx state.
    unsafe {
        SSL_CTX_set_session_cache_mode(ctx, SSL_SESS_CACHE_CLIENT);
        SSL_CTX_sess_set_new_cb(ctx, Some(on_new_session));
    }
}

/// Set the raw session-cache mode bits (server-side test helper; thin FFI
/// over `SSL_CTX_set_session_cache_mode`).
///
/// # Safety
///
/// `ctx` must be a live `SSL_CTX*`.
pub unsafe fn set_session_cache_mode(ctx: *mut SSL_CTX, mode: c_int) {
    unsafe { SSL_CTX_set_session_cache_mode(ctx, mode) };
}

/// Offer the cached session for `origin` on `ssl` and stash the origin key
/// so the new-session callback can (re)populate the store as fresh tickets
/// arrive. Must be called before the handshake starts — the same site and
/// precondition as the SNI/ALPN configuration calls.
///
/// Returns `true` when a session was actually offered (`SSL_set_session`
/// accepted). A miss is not an error: the first connection to an origin
/// performs a full handshake by design.
pub fn offer_session(ssl: *mut SSL, host: &str, port: u16, profile_salt: u64) -> bool {
    if ssl.is_null() || host.is_empty() {
        return false;
    }
    let key = origin_key(host, port, profile_salt);
    // Re-offer on the same SSL replaces the stashed key; free the old box
    // instead of leaking it (the ex-data destructor only runs at SSL_free).
    // SAFETY: idx is a registered ex-data slot; data is a live Box<String>.
    unsafe {
        let prev = SSL_get_ex_data(ssl, origin_ex_idx());
        if !prev.is_null() {
            drop(Box::from_raw(prev.cast::<String>()));
        }
        SSL_set_ex_data(ssl, origin_ex_idx(), Box::into_raw(Box::new(key.clone())).cast());
    }

    let Some(session) = global().take_for_offer(&key) else {
        return false;
    };
    // SAFETY: `session` is an owned live reference (SSL_set_session up-refs
    // internally — ssl_set_session in ssl/ssl_session.cc — so release our
    // own reference immediately).
    unsafe {
        let ok = SSL_set_session(ssl, session);
        SSL_SESSION_free(session);
        ok == 1
    }
}

/// Whether `ssl` resumed a cached session (abbreviated handshake / TLS 1.3
/// PSK). Observability seam for tests and future stealth telemetry.
pub fn session_reused(ssl: *const SSL) -> bool {
    // SAFETY: read-only query on a live SSL.
    unsafe { SSL_session_reused(ssl) == 1 }
}

/// The current session of `ssl` as an owned reference (`SSL_get1_session`).
/// Returns `None` before the handshake produces a session. Note: on a
/// TLS 1.3 client the resumable ticket sessions only reach the new-session
/// callback — this returns the (non-resumable) initial session.
///
/// # Safety
///
/// `ssl` must be a live `SSL*`; the caller owns the returned reference.
pub unsafe fn get1_session(ssl: *const SSL) -> Option<*mut SslSession> {
    // SAFETY: see fn doc.
    let session = unsafe { SSL_get1_session(ssl) };
    (!session.is_null()).then_some(session)
}

/// Ex-data slot stashing the per-connection origin key (`Box<String>`) set
/// by `offer_session`, consumed by `on_new_session` and freed by
/// `origin_key_free` at `SSL_free`.
fn origin_ex_idx() -> c_int {
    static IDX: OnceLock<c_int> = OnceLock::new();
    *IDX.get_or_init(|| {
        // SAFETY: registers a new ex-data class once; the free callback
        // matches the Box<String> stored by offer_session.
        unsafe {
            SSL_get_ex_new_index(
                0,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                Some(origin_key_free),
            )
        }
    })
}

/// `CRYPTO_EX_free` for the stashed origin key.
unsafe extern "C" fn origin_key_free(
    _parent: *mut c_void,
    ptr: *mut c_void,
    _ad: *mut c_void,
    _idx: c_int,
    _argl: c_long,
    _argp: *mut c_void,
) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(ptr.cast::<String>()) });
    }
}

/// `SSL_new_session_cb`: fires when a fresh session is available (TLS 1.2
/// handshake completion; TLS 1.3 NewSessionTicket processing — possibly
/// after the handshake, on ticket renewal too, which refreshes the entry).
///
/// Returning 1 takes ownership of the passed reference (ssl.h).
unsafe extern "C" fn on_new_session(_ssl: *mut SSL, session: *mut SslSession) -> c_int {
    if session.is_null() {
        return 0;
    }
    // SAFETY: ex-data reads are plain pointer reads on the live SSL.
    let key_ptr = unsafe { SSL_get_ex_data(_ssl, origin_ex_idx()) };
    if key_ptr.is_null() {
        // Connection never went through offer_session (not a tracked
        // client path) — leave ownership with BoringSSL.
        return 0;
    }
    let key = unsafe { &*key_ptr.cast::<String>() };
    // SAFETY: session is a live owned reference per the callback contract.
    if unsafe { SSL_SESSION_is_resumable(session) } != 1 {
        return 0;
    }
    unsafe { global().insert(key.clone(), session) };
    1
}

// ─── Tests ──────────────────────────────────────────────────────────────
//
// Store semantics (LRU, single-use take, thread safety) are exercised with
// real `SSL_SESSION`s from real in-process memory-BIO handshakes — no
// fabricated pointers (they would be dereferenced by the resumability
// probes and are meaningless as stand-ins).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TlsClient, TlsConnection, TlsServer, generate_self_signed_pem};
    use bun_boringssl_sys::boringssl::{ERR_clear_error, ERR_error_string, ERR_get_error};

    /// The lib tests share the process-wide GLOBAL store (that is the layer
    /// under test), so they must not run concurrently against it.
    static GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn err_queue() -> String {
        let mut out = String::new();
        loop {
            let packed = ERR_get_error();
            if packed == 0 {
                break;
            }
            let mut buf = [0i8; 256];
            unsafe { ERR_error_string(packed, buf.as_mut_ptr()) };
            let bytes: Vec<u8> = buf.iter().map(|b| *b as u8).take_while(|b| *b != 0).collect();
            out.push_str(&String::from_utf8_lossy(&bytes));
            out.push_str("; ");
        }
        out
    }

    fn drive(c: &mut TlsConnection, s: &mut TlsConnection) {
        ERR_clear_error();
        c.process().unwrap_or_else(|e| panic!("client: {e} | {}", err_queue()));
        s.feed(&c.take_outgoing());
        ERR_clear_error();
        s.process().unwrap_or_else(|e| panic!("server: {e} | {}", err_queue()));
        c.feed(&s.take_outgoing());
    }

    /// Drive one full handshake (plus enough post-handshake passes for the
    /// TLS 1.3 NewSessionTicket round-trip) and return the client
    /// connection. The connection goes through the production offer path:
    /// `offer_session` stashes the origin key before the handshake and the
    /// new-session callback populates the GLOBAL store when the ticket is
    /// processed (on TLS 1.3 the ticket session only reaches the callback —
    /// `SSL_get1_session` still returns the non-resumable initial session).
    fn handshake(server: &TlsServer, client: &TlsClient, host: &str, port: u16) -> TlsConnection {
        let mut s = server.accept().expect("server accept");
        let mut c = TlsConnection::new_client(client, host).expect("client conn");
        offer_session(c.ssl_ptr(), host, port, 0);
        for _ in 0..64 {
            drive(&mut c, &mut s);
            if !c.is_handshaking() && !s.is_handshaking() {
                break;
            }
        }
        assert!(!c.is_handshaking(), "client handshake must complete");
        assert!(!s.is_handshaking(), "server handshake must complete");
        // TLS over TCP: the server defers sending NewSessionTickets until
        // its first write (tls13_server.cc do_send_new_session_ticket), so
        // flush with a one-byte marker and drive the tickets to the client
        // (processing them fires the new-session callback).
        s.write(b"\x00").expect("ticket-flush marker write");
        for _ in 0..8 {
            drive(&mut c, &mut s);
        }
        assert!(
            global().contains_key(&origin_key(host, port, 0)),
            "new-session callback must populate the store (queue: {})",
            err_queue()
        );
        c
    }

    fn server_client() -> (TlsServer, TlsClient, String) {
        let (cert, key) = generate_self_signed_pem("session.local", 365).expect("cert");
        let der = crate::pem_parse_certs(&cert).into_iter().next().unwrap();
        let server = TlsServer::new(&cert, &key).expect("server");
        let client = TlsClient::new().expect("client");
        assert!(client.add_trusted_der(&der), "trust anchor");
        (server, client, "session.local".to_string())
    }

    #[test]
    fn lru_eviction() {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (server, client, host) = server_client();
        global().clear();
        let cache = ClientSessionCache::with_capacity(2);

        for i in 1..=3 {
            let port = 440 + i;
            let _c = handshake(&server, &client, &host, port);
            // Move the freshly issued ticket session (production insert
            // path put it in the GLOBAL store) into the small local store.
            // SAFETY: take_for_offer returns an owned reference that insert
            // takes over.
            let sess = global()
                .take_for_offer(&origin_key(&host, port, 0))
                .expect("fresh ticket session in global store");
            unsafe { cache.insert(format!("{host}:44{i}"), sess) };
        }
        assert_eq!(cache.len(), 2, "capacity 2 must cap origins");
        assert!(!cache.contains_key(&format!("{host}:441")), "LRU must evict the oldest key");
        assert!(cache.contains_key(&format!("{host}:442")));
        assert!(cache.contains_key(&format!("{host}:443")));
        cache.clear();
        assert!(cache.is_empty());
        global().clear();
    }

    #[test]
    fn take_removes_single_use_sessions() {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (server, client, host) = server_client();
        global().clear();
        let _c = handshake(&server, &client, &host, 443);
        let key = origin_key(&host, 443, 0);

        let first = global().take_for_offer(&key).expect("first take hits");
        // SAFETY: release the take-returned owned reference.
        unsafe { SSL_SESSION_free(first) };
        let second = global().take_for_offer(&key);
        if global().contains_key(&key) {
            // TLS 1.2 multi-use session stays in the store.
            let sess = second.expect("multi-use session must stay takeable");
            // SAFETY: release the take-returned owned reference.
            unsafe { SSL_SESSION_free(sess) };
        } else {
            // TLS 1.3 single-use ticket was removed by the first take.
            assert!(
                second.is_none(),
                "removed single-use session must not be offered twice"
            );
        }
        global().clear();
    }

    #[test]
    fn concurrent_access() {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (server, client, host) = server_client();
        global().clear();
        let cache = std::sync::Arc::new(ClientSessionCache::with_capacity(4));
        // SAFETY: TlsServer/TlsClient are Send+Sync (SSL_CTX is thread-safe
        // after creation); connections run on their own threads.
        let server = std::sync::Arc::new(server);
        let client = std::sync::Arc::new(client);
        let host = std::sync::Arc::new(host);

        let mut joins = Vec::new();
        for t in 0..4u16 {
            let (server, client, host, cache) =
                (server.clone(), client.clone(), host.clone(), cache.clone());
            joins.push(std::thread::spawn(move || {
                for i in 0..8u16 {
                    let port = 5000 + t * 100 + i;
                    let _c = handshake(&server, &client, &host, port);
                    // Move the fresh ticket into the contended local store,
                    // then take it back out — insert/take churn from four
                    // threads exercises the mutex-serialized paths.
                    // SAFETY: take returns an owned ref that insert takes.
                    if let Some(sess) = global().take_for_offer(&origin_key(&host, port, 0)) {
                        let key = format!("{host}:{port}");
                        unsafe { cache.insert(key.clone(), sess) };
                        if let Some(sess) = cache.take_for_offer(&key) {
                            // SAFETY: release the take-returned owned ref.
                            unsafe { SSL_SESSION_free(sess) };
                        }
                    }
                }
            }));
        }
        for j in joins {
            j.join().expect("no thread panicked / deadlocked");
        }
        assert!(cache.len() <= 4, "capacity respected under concurrency");
        cache.clear();
        global().clear();
    }
}
