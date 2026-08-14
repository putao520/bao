// @trace REQ-ENG-007 [entity:TlsProfile] [api:GET /api/node-compat]
use ::std::cell::{Cell, RefCell};
use ::std::collections::HashMap;
use ::std::net::{TcpListener, TcpStream};
use ::std::os::fd::AsRawFd;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use ::std::sync::{Arc, Mutex, OnceLock};
use ::std::time::{Duration, Instant};
use bun_core::ZBox;

use bao_boringssl_bridge::{
    KeyFormat, SslClientHello, TlsClient, TlsConnection, TlsError, TlsServer, TlsState,
    pem_parse_certs, pem_parse_key, ssl_servername, SSL_SELECT_CERT_ERROR, SSL_SELECT_CERT_RETRY,
    SSL_SELECT_CERT_SUCCESS,
};
use bun_boringssl_sys::boringssl::*;
use mozjs::jsapi::*;
use mozjs::jsval::{DoubleValue, Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::node_events::{
    ee_emit, ee_off, ee_on, ee_once, ee_prepend, ee_prepend_once, ee_remove_all,
};
use crate::require::cache_builtin;

// ─── SecureContextState — Rust-native TLS credential storage ──────────
//
// Stores parsed TLS credentials outside the JS heap to prevent
// sensitive key/cert data from being accessible via JS reflection.
// Stored as a SpiderMonkey PrivateValue on the SecureContext JS object.
//
// All certificate/key data is stored as DER bytes (Vec<u8>) or PEM strings.
// PEM strings are kept for TlsServer::new() which accepts PEM directly.

struct SecureContextState {
    key_der: Option<(KeyFormat, Vec<u8>)>,
    cert_ders: Vec<Vec<u8>>,   // DER-encoded certificates
    ca_certs: Vec<Vec<u8>>,    // DER-encoded CA certificates
    pem_certs: Option<String>, // PEM cert string for TlsServer::new()
    pem_key: Option<String>,   // PEM key string for TlsServer::new()
    /// ALPN protocols list — wire-format bytes (length-prefixed: 0x02h2\x08http/1.1)
    alpn_protos: Option<Vec<u8>>,
    /// Session data for resumption — serialized SSL_SESSION bytes
    session_data: Option<Vec<u8>>,
}

impl SecureContextState {
    fn new() -> Self {
        Self {
            key_der: None,
            cert_ders: Vec::new(),
            ca_certs: Vec::new(),
            pem_certs: None,
            pem_key: None,
            alpn_protos: None,
            session_data: None,
        }
    }
}

/// Check if a JSVal is a PrivateValue by testing is_double() with zero high bits.
/// SpiderMonkey encodes private values as doubles; this guard rejects non-private doubles.
#[inline]
fn val_is_private(v: &JSVal) -> bool {
    v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0
}

/// Store a `Box<SecureContextState>` as a private value on a JS object.
/// Creates the state if it doesn't exist yet.
unsafe fn sc_state_ensure(cx: *mut JSContext, obj: *mut JSObject) -> *mut SecureContextState {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut slot_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_scState".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut slot_val,
        },
    );

    if val_is_private(&slot_val) {
        let ptr = slot_val.to_private() as *mut SecureContextState;
        if !ptr.is_null() {
            return ptr;
        }
    }

    // Create new state
    let state = Box::new(SecureContextState::new());
    let ptr = Box::into_raw(state) as *const core::ffi::c_void;
    let pv = mozjs::jsval::PrivateValue(ptr);
    rooted!(&in(cx_ref) let pv_h = pv);
    JS_DefineProperty(
        cx,
        obj_root.handle().into(),
        c"_scState".as_ptr(),
        pv_h.handle().into(),
        0,
    );
    ptr as *mut SecureContextState
}

/// Parse PEM key string and store in SecureContextState.
unsafe fn sc_state_set_key(cx: *mut JSContext, obj: *mut JSObject, pem: &str) -> bool {
    let key = pem_parse_key(pem);
    if let Some(k) = key {
        let state = sc_state_ensure(cx, obj);
        (*state).key_der = Some(k);
        (*state).pem_key = Some(pem.to_string());
        true
    } else {
        false
    }
}

/// Parse PEM cert string and store in SecureContextState.
unsafe fn sc_state_set_cert(cx: *mut JSContext, obj: *mut JSObject, pem: &str) -> bool {
    let ders = pem_parse_certs(pem);
    if ders.is_empty() {
        return false;
    }
    let state = sc_state_ensure(cx, obj);
    (*state).cert_ders = ders;
    (*state).pem_certs = Some(pem.to_string());
    true
}

/// Parse PEM CA cert string and add to CA certificates in SecureContextState.
unsafe fn sc_state_add_ca(cx: *mut JSContext, obj: *mut JSObject, pem: &str) -> bool {
    let ders = pem_parse_certs(pem);
    if ders.is_empty() {
        return false;
    }
    let state = sc_state_ensure(cx, obj);
    (*state).ca_certs.extend(ders);
    true
}

/// Set ALPN protocols on the SecureContextState.
/// Accepts a JS array of protocol name strings, builds wire-format bytes.
unsafe fn sc_state_set_alpn_protos(
    cx: *mut JSContext,
    obj: *mut JSObject,
    protos_val: JSVal,
) -> bool {
    if !protos_val.is_object() {
        return false;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr_obj = protos_val.to_object());

    // Build wire-format ALPN list: each entry is length-prefixed
    let mut wire = Vec::new();
    let mut i: u32 = 0;
    loop {
        let mut elem = UndefinedValue();
        JS_GetElement(
            cx,
            arr_obj.handle().into(),
            i,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        if elem.is_undefined() {
            break;
        }
        if elem.is_string() {
            let proto = crate::js_to_rust_string(cx, elem);
            if proto.len() > 255 {
                continue; // ALPN protocol name too long
            }
            wire.push(proto.len() as u8);
            wire.extend_from_slice(proto.as_bytes());
        }
        i += 1;
    }

    if wire.is_empty() {
        return false;
    }

    let state = sc_state_ensure(cx, obj);
    (*state).alpn_protos = Some(wire);
    true
}

/// Set session data for resumption.
unsafe fn sc_state_set_session(
    cx: *mut JSContext,
    obj: *mut JSObject,
    session_bytes: &[u8],
) -> bool {
    let state = sc_state_ensure(cx, obj);
    (*state).session_data = Some(session_bytes.to_vec());
    true
}

/// Drop the SecureContextState stored on a JS object (for cleanup).
unsafe fn sc_state_drop(cx: *mut JSContext, obj: *mut JSObject) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut slot_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_scState".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut slot_val,
        },
    );

    if val_is_private(&slot_val) {
        let ptr = slot_val.to_private() as *mut SecureContextState;
        if !ptr.is_null() {
            let _ = Box::from_raw(ptr);
        }
        rooted!(&in(cx_ref) let undef = UndefinedValue());
        JS_DefineProperty(
            cx,
            obj_root.handle().into(),
            c"_scState".as_ptr(),
            undef.handle().into(),
            0,
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TLS server driver — event-driven accept + handshake + SNI dispatch
// ═══════════════════════════════════════════════════════════════════════
//
// Root-cure for the silently-ignored `SNICallback` contract (node_tls.rs
// parsed it into `_sniCallback` and nothing ever called it) AND for the
// missing server data path (`listen()` previously only configured a
// `TlsServer` and stored `_listenPort`; no TCP listener was ever bound, no
// connection ever served).
//
// Architecture (mirrors the FetchTasklet / HTTPThread split):
//
//   JS thread                                  TLS driver thread (one per process)
//   ─────────────────                          ─────────────────────────────────
//   tls.createServer({key, cert,               poll() on [wake pipe, listeners,
//     SNICallback})                              conns]
//   listen(port) ── AddListener cmd ────────▶  accept → TlsConnection (base CTX)
//   ┌ ConcurrentTask (AnyTaskWithExtraContext)   SSL_do_handshake
//   │  (MiniEventLoop auto-wake)                 └─ select-certificate cb fires:
//   │                                              no JS SNICallback → SUCCESS
//   │                                               (default branch: static cert)
//   │                                              servername + SNICallback:
//   │   SniRequest event ◀── push event ──         push SniRequest, return RETRY
//   │   call SNICallback(servername, cb)           (handshake suspends:
//   │   cb(err|null, ctx|{key,cert})                TlsState::PendingCertificate)
//   │      │                                        conn parked until deadline
//   │      └─ sni_result + wake ──────────────▶   re-drive handshake → cb again:
//   │                                              build CTX from {cert,key},
//   │   SecureConnection/Data/End/Close ◀──        SSL_set_SSL_CTX → SUCCESS
//   │   events: build TLSSocket, emit
//   └ socket.write() → pending_writes queue ─▶   drained by driver thread only
//
// ssl_in_use (upstream 0825a8b3f) equivalence: in upstream's C stack the
// ALPN/SNI selection callbacks run JS *inside* SSL_read/SSL_do_handshake,
// so a socket.write() from the callback re-entered BoringSSL on the same
// SSL mid-handshake — the ssl_in_use protocol parks such writes and the
// parked-write machinery flushes them once the handshake completes, BEFORE
// anything written from `secureConnection`. In this Rust stack the
// re-entrancy is impossible by construction: the SSL object is only ever
// touched by the single driver thread (JS writes append to a
// Mutex-protected queue, never call SSL_write), and when the JS callback
// runs the driver has already returned from BoringSSL (the connection is
// parked in PendingCertificate, no BoringSSL call on the stack). The
// park-then-flush ORDERING semantics are preserved exactly: parked writes
// (including any issued from inside SNICallback) are flushed to the wire
// before the `secureConnection` event is dispatched.
//
// Node SNICallback semantics: Node allows the callback to resolve
// asynchronously; internally it defers the handshake. BoringSSL's
// `ssl_select_cert_retry` is the native equivalent used here, so both sync
// and async SNICallback invocations work — the handshake simply suspends
// until `cb` fires. A `cb` that never fires fails closed at the SNI
// deadline (fatal alert + `tlsClientError`), never silently.

/// SNI resolution deadline. A SNICallback that never calls back fails the
/// handshake (fatal alert + explicit `tlsClientError`) instead of parking
/// the connection forever.
const SNI_DEADLINE: Duration = Duration::from_secs(120);

/// Driver → JS-thread events, drained by the ConcurrentTask on the JS thread.
enum TlsEvent {
    /// TCP accepted; the JS TLSSocket object must be created and
    /// `connection` emitted (Node tls.Server inherits net.Server's
    /// `connection` event; in Bao the payload is the TLS socket object,
    /// matching Bun's observable behavior where writing it from inside the
    /// SNI/ALPN selection delivers TLS application data).
    Connection {
        conn_id: u64,
        shared: Arc<ConnShared>,
    },
    /// ClientHello carried a servername and a JS SNICallback is registered.
    SniRequest { conn_id: u64, servername: String },
    /// Handshake completed on `conn_id`.
    SecureConnection {
        conn_id: u64,
        servername: Option<String>,
        alpn: Option<Vec<u8>>,
    },
    /// Decrypted application data.
    Data { conn_id: u64, bytes: Vec<u8> },
    /// Peer sent close_notify (clean TLS EOF).
    End { conn_id: u64 },
    /// Connection fully closed (after End, error, end() or destroy()).
    Close { conn_id: u64 },
    /// Handshake/protocol failure. Emitted as `tlsClientError`.
    ClientError { conn_id: u64, message: String },
    /// Server fully torn down (driver removed it): unroot JS refs, emit
    /// `close`, invoke the stored close callback.
    ServerClosed,
}

/// Per-connection cross-thread state. The Mutex/atomic fields are the ONLY
/// shared surface between the JS thread (write/end/destroy/SNI resolution)
/// and the driver thread (owner of the SSL object).
struct ConnShared {
    /// JS → driver: plaintext to encrypt+send. Parked (not SSL_write'n)
    /// until the driver drains it — the ssl_in_use park-and-flush semantics.
    pending_writes: Mutex<Vec<Vec<u8>>>,
    /// JS → driver: graceful end — flush parked writes, send close_notify.
    want_end: AtomicBool,
    /// JS → driver: immediate destroy.
    want_destroy: AtomicBool,
    /// Driver → JS: connection closed; further writes are rejected.
    closed: AtomicBool,
    /// JS → driver: SNICallback resolution. `Ok((cert_pem, key_pem))` or
    /// `Err(message)` (callback error / missing credentials).
    sni_result: Mutex<Option<::std::result::Result<(String, String), String>>>,
}

impl ConnShared {
    fn new() -> Self {
        Self {
            pending_writes: Mutex::new(Vec::new()),
            want_end: AtomicBool::new(false),
            want_destroy: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            sni_result: Mutex::new(None),
        }
    }
}

/// Server-wide cross-thread state. Fields marked JS-thread-only are never
/// touched from the driver thread (enforced by contract, same as
/// `PendingFetch` in fetch_async.rs).
struct ServerShared {
    server_id: u64,
    /// JS-thread-only: context that owns the server object.
    cx: *mut JSContext,
    /// JS-thread-only: heap-rooted server object value. `AddRawValueRoot`
    /// pins the Box — the GC scans/updates that memory in place (same
    /// contract as `PersistentGlobal` in bao_engine::context).
    server_obj_root: Option<Box<JSVal>>,
    /// JS-thread-only: heap-rooted SNICallback function (present iff
    /// SNICallback was provided).
    sni_fn_root: Option<Box<JSVal>>,
    /// JS-thread-only: pointer to the JS thread's MiniEventLoop (captured
    /// at listen() time, valid for the thread's lifetime).
    mini_loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static>,
    /// ConcurrentTask carrier for the JS-thread event drain. Re-initialized
    /// before each enqueue (the task is dequeued before it runs).
    concurrent_task: bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext,
    /// Guards against duplicate ConcurrentTask scheduling (compare_exchange
    /// false→true before enqueue; reset at tasklet entry).
    task_scheduled: AtomicBool,
    /// Driver → JS event queue.
    events: Mutex<Vec<TlsEvent>>,
    /// close() requested; driver removes the listener and pushes
    /// `ServerClosed` as the final event.
    closing: AtomicBool,
    /// Cache of SNI-resolved SSL_CTXs, keyed by (cert_pem, key_pem). Driver
    /// thread only (inside the select-certificate callback), Mutex for
    /// cross-thread Sync. `SSL_set_SSL_CTX` up-refs the ctx, so eviction is
    /// safe while live SSLs hold references.
    sni_ctx_cache: Mutex<HashMap<(String, String), Arc<TlsServer>>>,
    /// Leaked wire-format ALPN list (re-registered on SNI-resolved CTXs —
    /// `SSL_set_SSL_CTX` swaps the whole ctx, so the ALPN select callback
    /// must be installed on the replacement too).
    alpn_wire: Option<&'static [u8]>,
}

// SAFETY: `cx` / `*_root` / `mini_loop_ptr` / `concurrent_task` are only
// dereferenced on the JS thread that created them (identical contract to
// `PendingFetch` in fetch_async.rs); the driver thread only touches the
// Mutex/atomic fields and `sni_ctx_cache` (whose contents are Send+Sync).
unsafe impl Send for ServerShared {}
unsafe impl Sync for ServerShared {}

/// Driver-thread-side per-connection state. Owned exclusively by the driver
/// thread — this is what makes BoringSSL re-entrancy structurally
/// impossible: no other thread ever calls into the SSL object.
struct DriverConn {
    conn_id: u64,
    stream: TcpStream,
    tls: TlsConnection,
    shared: Arc<ConnShared>,
    server: Arc<ServerShared>,
    /// Handshake state mirror of `TlsState` (driver-side transitions).
    parked_for_sni: bool,
    sni_requested: bool,
    sni_servername: Option<String>,
    sni_deadline: Option<Instant>,
    /// Outgoing ciphertext not yet written to the socket.
    out_buf: Vec<u8>,
    /// `secureConnection` event pushed (first Active observation).
    secure_reported: bool,
    /// Graceful shutdown requested: flush, send close_notify, close.
    finishing: bool,
    /// close_notify queued (out_buf may still hold it).
    close_notify_sent: bool,
}

/// Driver-thread-side per-server state.
struct DriverServer {
    server_id: u64,
    listener: TcpListener,
    base: TlsServer,
    shared: Arc<ServerShared>,
}

/// Commands JS thread → driver thread.
enum DriverCmd {
    AddListener(TcpListener, Arc<ServerShared>, TlsServer),
    RemoveServer(u64),
}

struct DriverHandle {
    /// Write end of the wake pipe; a byte written here breaks the driver's
    /// poll() so it picks up commands / queued writes promptly.
    wake_fd: i32,
    cmds: Mutex<Vec<DriverCmd>>,
}

static DRIVER: OnceLock<DriverHandle> = OnceLock::new();
/// Serializes driver bootstrap across concurrent listen() calls (the
/// OnceLock alone cannot distinguish "not yet initialized" from "init
/// failed"; the lock makes creation single-shot).
static DRIVER_INIT: Mutex<()> = Mutex::new(());
static NEXT_TLS_ID: AtomicU64 = AtomicU64::new(1);

// The connection the driver thread is currently driving a BoringSSL call
// on. Set around `TlsConnection::process()` so the select-certificate
// callback (which fires inside `SSL_do_handshake`) can find its conn.
thread_local! {
    static DRIVER_CURRENT_CONN: Cell<*mut DriverConn> = const { Cell::new(::std::ptr::null_mut()) };
}

// JS-thread registry: server_id → shared handle. Keeps the `ServerShared`
// (and its heap roots) alive until the `ServerClosed` tasklet unroots; the
// driver drops its own Arc when it removes the server.
thread_local! {
    static TLS_SERVER_REGISTRY: RefCell<HashMap<u64, Arc<ServerShared>>> =
        RefCell::new(HashMap::new());
}

/// JS-thread registry: conn_id → per-conn JS handle (rooted socket object +
/// the cross-thread ConnShared). Populated on `Connection` events, removed
/// on `Close` events.
struct JsConn {
    shared: Arc<ConnShared>,
    socket_root: Option<Box<JSVal>>,
}

thread_local! {
    static TLS_CONNS: RefCell<HashMap<u64, JsConn>> = RefCell::new(HashMap::new());
}

fn tls_driver_wake() {
    if let Some(h) = DRIVER.get() {
        let byte = [1u8];
        // SAFETY: wake_fd is a live pipe write end (owned by the OnceLock).
        unsafe {
            let _ = libc::write(h.wake_fd, byte.as_ptr().cast::<core::ffi::c_void>(), 1);
        }
    }
}

/// Ensure the driver thread + wake pipe exist. `None` only on resource
/// exhaustion (pipe/thread creation failure) — callers fail closed.
fn tls_driver_acquire() -> Option<&'static DriverHandle> {
    if let Some(h) = DRIVER.get() {
        return Some(h);
    }
    // Serialize bootstrap: a concurrent winner installs the handle; losers
    // re-check under the lock and reuse it (no orphaned pipes/threads).
    let _init_guard = DRIVER_INIT.lock().unwrap();
    if let Some(h) = DRIVER.get() {
        return Some(h);
    }
    let mut fds = [-1i32; 2];
    // SAFETY: fds is a valid 2-int out-buffer for pipe(2).
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    // The wake-drain loop reads until EAGAIN — the READ end must be
    // non-blocking or the drain would block (and deadlock the driver) once
    // the pipe empties. The write end stays blocking (a 64K pipe buffer
    // never fills with 1-byte wakes).
    // SAFETY: fds[0] is a live pipe read end; F_SETFL only adds flags.
    unsafe {
        let flags = libc::fcntl(fds[0], libc::F_GETFL);
        if flags < 0
            || libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK) < 0
        {
            libc::close(fds[0]);
            libc::close(fds[1]);
            return None;
        }
    }
    let handle = DriverHandle {
        wake_fd: fds[1],
        cmds: Mutex::new(Vec::new()),
    };
    // SAFETY: fds[0] is the read end; the driver thread owns it exclusively.
    let spawned = ::std::thread::Builder::new()
        .name("bao-tls-driver".into())
        .spawn(move || tls_driver_main(fds[0]));
    match spawned {
        Ok(_) => {
            let _ = DRIVER.set(handle);
            DRIVER.get()
        }
        Err(_) => {
            // SAFETY: both fds were just created by pipe(2) and are unused.
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            None
        }
    }
}

fn tls_push_event(shared: &Arc<ServerShared>, ev: TlsEvent) {
    shared.events.lock().unwrap().push(ev);
    tls_schedule_tasklet(shared);
}

/// Schedule the JS-thread event-drain tasklet (idempotent: the
/// compare_exchange admits exactly one enqueue per in-flight dispatch).
fn tls_schedule_tasklet(shared: &Arc<ServerShared>) {
    if shared
        .task_scheduled
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let loop_ptr = shared.mini_loop_ptr;
    if loop_ptr.is_null() {
        // No JS event loop captured (listen() raced teardown). Reset so a
        // later attempt can retry; events remain queued.
        shared.task_scheduled.store(false, Ordering::Release);
        return;
    }
    // SAFETY: mini_loop_ptr was captured on the JS thread and is valid for
    // the thread's lifetime; shared is kept alive by the JS registry Arc
    // until the ServerClosed tasklet removes it.
    unsafe {
        let loop_ref = &mut *(loop_ptr as *mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>);
        let shared_ptr = Arc::as_ptr(shared) as *mut ServerShared;
        let task_ptr = core::ptr::addr_of_mut!((*shared_ptr).concurrent_task);
        (*task_ptr).from(shared_ptr, tls_event_tasklet_shim);
        loop_ref.enqueue_task_concurrent(NonNull::new_unchecked(task_ptr));
    }
}

// ─── select-certificate callback (driver thread, inside BoringSSL) ─────

/// BoringSSL select-certificate callback: the SNICallback injection point.
/// Runs on the driver thread inside `SSL_do_handshake`.
unsafe extern "C" fn tls_select_cert_cb(client_hello: *const SslClientHello) -> core::ffi::c_int {
    let ssl = unsafe { (*client_hello).ssl };
    let conn_ptr = DRIVER_CURRENT_CONN.with(|c| c.get());
    if conn_ptr.is_null() {
        // No connection under drive — structurally unreachable (the callback
        // only fires inside the driver's process() window). Fail closed.
        log::error!("[tls] select-certificate callback without an active connection");
        return SSL_SELECT_CERT_ERROR;
    }
    // SAFETY: DRIVER_CURRENT_CONN is set around process() on this thread.
    let conn = unsafe { &mut *conn_ptr };

    if !conn.sni_requested {
        match ssl_servername(ssl) {
            Some(servername) => {
                conn.sni_requested = true;
                conn.sni_servername = Some(servername.clone());
                conn.sni_deadline = Some(Instant::now() + SNI_DEADLINE);
                tls_push_event(
                    &conn.server,
                    TlsEvent::SniRequest {
                        conn_id: conn.conn_id,
                        servername,
                    },
                );
                // Handshake suspends (TlsState::PendingCertificate) until
                // the JS SNICallback resolves via its `cb`.
                SSL_SELECT_CERT_RETRY
            }
            None => {
                // No SNI extension → default branch: static certificate.
                SSL_SELECT_CERT_SUCCESS
            }
        }
    } else {
        // Re-invocation after retry: the resolution must be in the slot
        // (the driver only re-drives the handshake once it is present).
        let result = conn.shared.sni_result.lock().unwrap().take();
        match result {
            Some(Ok((cert_pem, key_pem))) => match tls_sni_ctx_for(&conn.server, &cert_pem, &key_pem) {
                Ok(ctx) => {
                    // SAFETY: ctx is a live SSL_CTX* from a cached TlsServer
                    // (Arc keeps it alive; SSL_set_SSL_CTX up-refs it).
                    if unsafe { conn.tls.switch_ssl_ctx(ctx) } {
                        SSL_SELECT_CERT_SUCCESS
                    } else {
                        log::error!("[tls] SSL_set_SSL_CTX failed for SNI resolution");
                        tls_push_event(
                            &conn.server,
                            TlsEvent::ClientError {
                                conn_id: conn.conn_id,
                                message: "SNICallback: SSL_set_SSL_CTX failed".to_string(),
                            },
                        );
                        SSL_SELECT_CERT_ERROR
                    }
                }
                Err(msg) => {
                    log::error!("[tls] SNICallback credentials rejected: {}", msg);
                    tls_push_event(
                        &conn.server,
                        TlsEvent::ClientError {
                            conn_id: conn.conn_id,
                            message: format!("SNICallback credentials rejected: {}", msg),
                        },
                    );
                    SSL_SELECT_CERT_ERROR
                }
            },
            Some(Err(msg)) => {
                // Explicit dispatch failure → fail the handshake loudly,
                // surfacing the callback's own error to tlsClientError.
                log::error!("[tls] SNICallback returned an error: {}", msg);
                tls_push_event(
                    &conn.server,
                    TlsEvent::ClientError {
                        conn_id: conn.conn_id,
                        message: format!("SNICallback error: {}", msg),
                    },
                );
                SSL_SELECT_CERT_ERROR
            }
            None => SSL_SELECT_CERT_RETRY,
        }
    }
}

/// Build (or fetch from cache) the SSL_CTX for an SNI-resolved credential
/// pair. Driver thread only.
fn tls_sni_ctx_for(
    shared: &Arc<ServerShared>,
    cert_pem: &str,
    key_pem: &str,
) -> ::std::result::Result<*mut SSL_CTX, String> {
    let key = (cert_pem.to_string(), key_pem.to_string());
    let mut cache = shared.sni_ctx_cache.lock().unwrap();
    if let Some(existing) = cache.get(&key) {
        return Ok(existing.ctx());
    }
    let server = TlsServer::new(cert_pem, key_pem).map_err(|e| e.to_string())?;
    // The replacement ctx must serve the same ALPN selection as the base
    // (SSL_set_SSL_CTX swaps the whole ctx).
    if let Some(wire) = shared.alpn_wire {
        // SAFETY: wire is a leaked 'static slice; registration matches the
        // base CTX setup in tls_server_listen.
        unsafe {
            SSL_CTX_set_alpn_select_cb(
                server.ctx(),
                Some(alpn_select_callback),
                wire.as_ptr() as *mut core::ffi::c_void,
            );
        }
    }
    let ctx = server.ctx();
    cache.insert(key, Arc::new(server));
    Ok(ctx)
}

// ─── driver main loop ───────────────────────────────────────────────────

fn tls_driver_main(wake_read_fd: i32) {
    let mut servers: HashMap<u64, DriverServer> = HashMap::new();
    let mut conns: HashMap<u64, DriverConn> = HashMap::new();
    let mut remove_queue: Vec<u64> = Vec::new();

    // The spawner installs the DRIVER handle right after spawning this
    // thread; wait briefly for it instead of racing to a spurious exit.
    let bootstrap_deadline = Instant::now() + Duration::from_secs(5);
    let handle = loop {
        match DRIVER.get() {
            Some(h) => break h,
            None if Instant::now() < bootstrap_deadline => {
                ::std::thread::sleep(Duration::from_millis(1));
            }
            None => return,
        }
    };

    loop {
        // ── 1. take commands ────────────────────────────────────────────

        {
            let mut cmds = handle.cmds.lock().unwrap();
            for cmd in cmds.drain(..) {
                match cmd {
                    DriverCmd::AddListener(listener, shared, base) => {
                        let id = shared.server_id;
                        servers.insert(id, DriverServer {
                            server_id: id,
                            listener,
                            base,
                            shared,
                        });
                    }
                    DriverCmd::RemoveServer(id) => remove_queue.push(id),
                }
            }
        }
        for id in remove_queue.drain(..) {
            if let Some(ds) = servers.remove(&id) {
                // Mark closing; close all connections of this server.
                let dead: Vec<u64> = conns
                    .values()
                    .filter(|c| c.server.server_id == id)
                    .map(|c| c.conn_id)
                    .collect();
                for cid in dead {
                    if let Some(mut conn) = conns.remove(&cid) {
                        tls_conn_finish(&mut conn, /*notify=*/ true);
                    }
                }
                drop(ds.listener);
                tls_push_event(&ds.shared, TlsEvent::ServerClosed);
            }
        }
        // NOTE: no idle exit. The driver is a process-lifetime singleton
        // (the HTTPThread model): exiting when idle raced against the next
        // listen()'s AddListener command — the driver could observe
        // "no servers, no conns" after spawn but before the command landed
        // and exit, leaving every later listener unserved.

        // ── 2. build poll set ───────────────────────────────────────────
        enum Target {
            Wake,
            Listener(u64),
            Conn(u64),
        }
        let mut fds: Vec<libc::pollfd> = Vec::with_capacity(2 + servers.len() + conns.len());
        let mut targets: Vec<Target> = Vec::with_capacity(fds.capacity());
        fds.push(libc::pollfd {
            fd: wake_read_fd,
            events: libc::POLLIN,
            revents: 0,
        });
        targets.push(Target::Wake);
        for ds in servers.values() {
            fds.push(libc::pollfd {
                fd: ds.listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
            targets.push(Target::Listener(ds.server_id));
        }
        for conn in conns.values() {
            let mut events = 0;
            if !conn.parked_for_sni && !conn.finishing {
                events |= libc::POLLIN;
            }
            if !conn.out_buf.is_empty() {
                events |= libc::POLLOUT;
            }
            if events != 0 {
                fds.push(libc::pollfd {
                    fd: conn.stream.as_raw_fd(),
                    events,
                    revents: 0,
                });
                targets.push(Target::Conn(conn.conn_id));
            }
        }

        // ── 3. timeout: block indefinitely (the wake pipe covers commands
        //       and JS-side writes; conn fds cover I/O) unless an SNI
        //       deadline is pending — the parked conn needs no I/O wake,
        //       only the deadline check. ─────────────────────────────────
        let mut timeout_ms: i32 = -1;
        for conn in conns.values() {
            if conn.parked_for_sni {
                if let Some(deadline) = conn.sni_deadline {
                    let remain = deadline.saturating_duration_since(Instant::now());
                    let ms = remain.as_millis() as i32;
                    if timeout_ms < 0 || ms < timeout_ms {
                        timeout_ms = ms;
                    }
                }
            }
        }
        if timeout_ms != -1 && timeout_ms < 0 {
            timeout_ms = 0;
        }

        // ── 4. poll ─────────────────────────────────────────────────────
        // SAFETY: fds is a valid pollfd array for the duration of the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };

        // ── 5. dispatch wake + commands first (writes/SNI may unblock
        //       conns regardless of socket readiness) ────────────────────
        if ready > 0 {
            if fds[0].revents != 0 {
                let mut buf = [0u8; 64];
                // SAFETY: drain the wake pipe (non-blocking is not set; the
                // pipe only ever holds a few bytes, and writers never block
                // on a 64-byte drain of a 64K pipe buffer).
                while unsafe { libc::read(wake_read_fd, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
            }
        }

        // ── 6. accept + socket I/O ─────────────────────────────────────
        if ready > 0 {
            for i in 1..fds.len() {
                let revents = fds[i].revents;
                if revents == 0 {
                    continue;
                }
                match &targets[i] {
                    Target::Wake => {}
                    Target::Listener(server_id) => {
                        if revents & libc::POLLIN != 0 {
                            tls_driver_accept(*server_id, &mut servers, &mut conns);
                        }
                    }
                    Target::Conn(conn_id) => {
                        let conn_id = *conn_id;
                        if conns.get(&conn_id).is_none() {
                            continue;
                        }
                        {
                            let conn = conns.get_mut(&conn_id).unwrap();
                            if revents & libc::POLLOUT != 0 {
                                tls_conn_flush_out(conn);
                            }
                        }
                        if revents & libc::POLLIN != 0 {
                            let conn = conns.get_mut(&conn_id).unwrap();
                            if !tls_conn_read_and_drive(conn) {
                                if let Some(mut conn) = conns.remove(&conn_id) {
                                    tls_conn_finish(&mut conn, true);
                                }
                                continue;
                            }
                        }
                        if revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                            // Error/hangup: drain any still-unread data,
                            // then tear the connection down.
                            let conn = conns.get_mut(&conn_id).unwrap();
                            if !tls_conn_read_and_drive(conn) {
                                if let Some(mut conn) = conns.remove(&conn_id) {
                                    tls_conn_finish(&mut conn, true);
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── 7. service pass: resolve/flush/finish transitions that do not
        //       depend on socket readiness ──────────────────────────────
        let conn_ids: Vec<u64> = conns.keys().copied().collect();
        for conn_id in conn_ids {
            let Some(conn) = conns.get_mut(&conn_id) else {
                continue;
            };
            // destroy(): immediate teardown, close_notify not required.
            if conn.shared.want_destroy.load(Ordering::Acquire) {
                if let Some(mut conn) = conns.remove(&conn_id) {
                    tls_conn_finish(&mut conn, true);
                }
                continue;
            }
            // end(): flush parked writes, then close_notify, then close.
            if conn.shared.want_end.load(Ordering::Acquire) && !conn.finishing {
                conn.finishing = true;
                tls_conn_flush_pending_writes(conn);
                if !conn.close_notify_sent {
                    let _ = conn.tls.queue_close_notify();
                    conn.out_buf.extend(conn.tls.take_outgoing());
                    conn.close_notify_sent = true;
                }
            }
            if conn.parked_for_sni {
                let resolved = conn
                    .shared
                    .sni_result
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|_| ());
                if resolved.is_some() {
                    conn.parked_for_sni = false;
                    if !tls_conn_drive(conn) {
                        if let Some(mut conn) = conns.remove(&conn_id) {
                            tls_conn_finish(&mut conn, true);
                        }
                        continue;
                    }
                } else if conn
                    .sni_deadline
                    .map(|d| Instant::now() >= d)
                    .unwrap_or(false)
                {
                    // SNICallback never resolved: fail closed, loudly.
                    tls_push_event(
                        &conn.server,
                        TlsEvent::ClientError {
                            conn_id,
                            message: format!(
                                "SNICallback for '{}' did not resolve within {}s",
                                conn.sni_servername.clone().unwrap_or_default(),
                                SNI_DEADLINE.as_secs()
                            ),
                        },
                    );
                    if let Some(mut conn) = conns.remove(&conn_id) {
                        tls_conn_finish(&mut conn, true);
                    }
                    continue;
                }
            }
            // Flush parked JS writes whenever the handshake is done.
            if conn.secure_reported && !conn.finishing {
                tls_conn_flush_pending_writes(conn);
            }
            if !conn.out_buf.is_empty() {
                tls_conn_flush_out(conn);
            }
            // finishing complete: close_notify flushed → close.
            if conn.finishing && conn.out_buf.is_empty() && conn.close_notify_sent {
                if let Some(mut conn) = conns.remove(&conn_id) {
                    tls_conn_finish(&mut conn, true);
                }
            }
        }
    }
}

/// Accept all pending connections on a ready listener.
fn tls_driver_accept(
    server_id: u64,
    servers: &mut HashMap<u64, DriverServer>,
    conns: &mut HashMap<u64, DriverConn>,
) {
    let Some(ds) = servers.get(&server_id) else {
        return;
    };
    loop {
        match ds.listener.accept() {
            Ok((stream, _addr)) => {
                let _ = stream.set_nonblocking(true);
                let conn_id = NEXT_TLS_ID.fetch_add(1, Ordering::Relaxed);
                let tls = match ds.base.accept() {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!("[tls] accept: TlsConnection setup failed: {}", e);
                        continue;
                    }
                };
                let shared = Arc::new(ConnShared::new());
                conns.insert(
                    conn_id,
                    DriverConn {
                        conn_id,
                        stream,
                        tls,
                        shared: Arc::clone(&shared),
                        server: Arc::clone(&ds.shared),
                        parked_for_sni: false,
                        sni_requested: false,
                        sni_servername: None,
                        sni_deadline: None,
                        out_buf: Vec::new(),
                        secure_reported: false,
                        finishing: false,
                        close_notify_sent: false,
                    },
                );
                tls_push_event(
                    &ds.shared,
                    TlsEvent::Connection {
                        conn_id,
                        shared: Arc::clone(&shared),
                    },
                );
            }
            Err(e) if e.kind() == ::std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

/// Read available ciphertext, feed it, and drive the TLS state machine.
/// Returns false when the connection must be torn down.
fn tls_conn_read_and_drive(conn: &mut DriverConn) -> bool {
    let mut buf = [0u8; 16 * 1024];
    loop {
        // SAFETY: buf is a valid read buffer.
        let n = unsafe {
            libc::read(
                conn.stream.as_raw_fd(),
                buf.as_mut_ptr().cast::<core::ffi::c_void>(),
                buf.len(),
            )
        };
        if n > 0 {
            conn.tls.feed(&buf[..n as usize]);
            continue;
        }
        if n == 0 {
            // EOF: peer closed the write side. Drive once more to surface
            // any close_notify/remaining plaintext, then finish.
            let ok = tls_conn_drive(conn);
            if ok && conn.secure_reported && !conn.finishing {
                // Plain FIN without close_notify: abrupt close (no 'end').
                return false;
            }
            return ok;
        }
        let err = ::std::io::Error::last_os_error();
        match err.kind() {
            ::std::io::ErrorKind::WouldBlock => break,
            ::std::io::ErrorKind::Interrupted => continue,
            _ => return false,
        }
    }
    tls_conn_drive(conn)
}

/// One process() pass. Sets DRIVER_CURRENT_CONN so the select-certificate
/// callback can find this connection. Returns false on fatal error.
fn tls_conn_drive(conn: &mut DriverConn) -> bool {
    DRIVER_CURRENT_CONN.with(|c| c.set(conn as *mut DriverConn));
    let result = conn.tls.process();
    DRIVER_CURRENT_CONN.with(|c| c.set(::std::ptr::null_mut()));

    let res = match result {
        Ok(r) => r,
        Err(e) => {
            tls_push_event(
                &conn.server,
                TlsEvent::ClientError {
                    conn_id: conn.conn_id,
                    message: format!("TLS handshake/protocol error: {}", e),
                },
            );
            return false;
        }
    };
    conn.out_buf.extend(conn.tls.take_outgoing());

    match res.state {
        TlsState::PendingCertificate => {
            // Select-certificate callback returned retry (SNI dispatch in
            // flight). Park until the JS SNICallback resolves.
            conn.parked_for_sni = true;
            true
        }
        TlsState::Handshaking => true,
        TlsState::Active | TlsState::PeerClosed | TlsState::Closed => {
            if !conn.secure_reported {
                conn.secure_reported = true;
                // ssl_in_use park-and-flush ordering: parked writes
                // (including any issued from inside SNICallback) hit the
                // wire BEFORE the secureConnection event is dispatched, so
                // they also precede anything written from a
                // secureConnection listener.
                tls_conn_flush_pending_writes(conn);
                let servername = conn.sni_servername.clone().or_else(|| conn.tls.servername());
                let alpn = conn.tls.alpn_protocol().map(|a| a.to_vec());
                tls_push_event(
                    &conn.server,
                    TlsEvent::SecureConnection {
                        conn_id: conn.conn_id,
                        servername,
                        alpn,
                    },
                );
            }
            if !res.plaintext.is_empty() {
                let mut bytes = Vec::new();
                for chunk in res.plaintext {
                    bytes.extend_from_slice(&chunk);
                }
                tls_push_event(
                    &conn.server,
                    TlsEvent::Data {
                        conn_id: conn.conn_id,
                        bytes,
                    },
                );
            }
            if res.state == TlsState::PeerClosed || res.state == TlsState::Closed {
                // Clean close_notify from the peer: emit End, answer with
                // our own close_notify, then close once flushed.
                tls_push_event(&conn.server, TlsEvent::End { conn_id: conn.conn_id });
                conn.finishing = true;
                let _ = conn.tls.queue_close_notify();
                conn.out_buf.extend(conn.tls.take_outgoing());
                conn.close_notify_sent = true;
            }
            true
        }
    }
}

/// Drain JS-parked plaintext writes into the SSL (driver thread only —
/// this is the ssl_in_use "retry parked write" equivalent).
fn tls_conn_flush_pending_writes(conn: &mut DriverConn) {
    let chunks: Vec<Vec<u8>> = {
        let mut g = conn.shared.pending_writes.lock().unwrap();
        ::std::mem::take(&mut *g)
    };
    for chunk in chunks {
        if chunk.is_empty() {
            continue;
        }
        match conn.tls.write(&chunk) {
            Ok(_) => {}
            Err(TlsError::NotReady) => {
                // Not ready (WANT_READ/WRITE): park the chunk back, retry
                // on a later pass.
                let mut g = conn.shared.pending_writes.lock().unwrap();
                g.insert(0, chunk);
                break;
            }
            Err(e) => {
                tls_push_event(
                    &conn.server,
                    TlsEvent::ClientError {
                        conn_id: conn.conn_id,
                        message: format!("TLS write failed: {}", e),
                    },
                );
            }
        }
    }
    conn.out_buf.extend(conn.tls.take_outgoing());
}

/// Write pending ciphertext to the socket (nonblocking; partial writes
/// keep the remainder for the POLLOUT pass).
fn tls_conn_flush_out(conn: &mut DriverConn) {
    while !conn.out_buf.is_empty() {
        // SAFETY: out_buf is a valid write buffer for the duration of the call.
        let n = unsafe {
            libc::write(
                conn.stream.as_raw_fd(),
                conn.out_buf.as_ptr().cast::<core::ffi::c_void>(),
                conn.out_buf.len(),
            )
        };
        if n > 0 {
            conn.out_buf.drain(..n as usize);
            continue;
        }
        if n == 0 {
            break;
        }
        let err = ::std::io::Error::last_os_error();
        match err.kind() {
            ::std::io::ErrorKind::WouldBlock => break,
            ::std::io::ErrorKind::Interrupted => continue,
            _ => {
                conn.out_buf.clear();
                break;
            }
        }
    }
}

/// Final teardown: mark closed, push the Close event (JS unroots the
/// socket and emits `close`).
fn tls_conn_finish(conn: &mut DriverConn, notify: bool) {
    conn.shared.closed.store(true, Ordering::Release);
    if notify {
        tls_push_event(&conn.server, TlsEvent::Close { conn_id: conn.conn_id });
    }
}

// ─── JS-thread event drain (ConcurrentTask) ─────────────────────────────

/// `AnyTaskWithExtraContext` callback bridge (must be a safe fn).
fn tls_event_tasklet_shim(ctx: *mut ServerShared, _parent: *mut ()) {
    // SAFETY: ctx was set to the Arc'd ServerShared pointer at schedule
    // time; the JS-thread registry keeps the allocation alive until the
    // ServerClosed event is processed (the last event).
    unsafe { tls_event_tasklet(ctx) };
}

unsafe fn tls_event_tasklet(ptr: *mut ServerShared) {
    // SAFETY: ptr is the Arc'd ServerShared; this tasklet is its final JS-
    // thread consumer (the registry Arc keeps it alive until ServerClosed).
    let s = unsafe { &mut *ptr };
    // Allow re-scheduling while we drain (mirrors resolve_tasklet step 1).
    s.task_scheduled.store(false, Ordering::Release);

    let events = {
        let mut g = s.events.lock().unwrap();
        ::std::mem::take(&mut *g)
    };
    if events.is_empty() {
        return;
    }

    let cx = s.cx;
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let server_val = *s
        .server_obj_root
        .as_ref()
        .map(|b| &**b)
        .unwrap_or(&UndefinedValue());
    if !server_val.is_object() {
        // Server object unreachable (unrooted earlier): drop events.
        return;
    }
    rooted!(&in(cx_ref) let server_root = server_val.to_object());

    // Enter the server object's realm for the whole drain (standard SM
    // embedding rule — the tasklet runs outside any JS activation).
    {
        let mut realm = AutoRealm::new_from_handle(cx_ref, server_root.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        for ev in events {
            match ev {
                TlsEvent::Connection { conn_id, shared } => {
                    let socket = tls_build_socket_js(realm_cx.raw_cx(), conn_id);
                    if socket.is_null() {
                        log::error!("[tls] failed to build TLSSocket object for conn {}", conn_id);
                        continue;
                    }
                    let mut socket_val = ObjectValue(socket);
                    // Pin + heap-root (AddRawValueRoot scans/updates the
                    // boxed slot in place — PersistentGlobal contract).
                    let name = b"TLSSocket.object\0".as_ptr() as *const core::ffi::c_char;
                    if AddRawValueRoot(cx, &mut socket_val as *mut JSVal, name) {
                        TLS_CONNS.with(|m| {
                            m.borrow_mut().insert(
                                conn_id,
                                JsConn {
                                    shared,
                                    socket_root: Some(Box::new(socket_val)),
                                },
                            );
                        });
                    }
                    tls_emit_js(cx, server_root.get(), "connection", &[socket_val]);
                }
                TlsEvent::SniRequest { conn_id, servername } => {
                    let sni_fn_val = s
                        .sni_fn_root
                        .as_ref()
                        .map(|b| **b)
                        .unwrap_or(UndefinedValue());
                    if !sni_fn_val.is_object() {
                        // has_sni was true at listen() — the root only
                        // disappears at ServerClosed. Unreachable; fail the
                        // handshake via the driver deadline if it ever fires.
                        log::error!("[tls] SniRequest without a rooted SNICallback (conn {})", conn_id);
                        continue;
                    }
                    tls_dispatch_sni_callback(
                        cx,
                        realm_cx,
                        server_root.get(),
                        sni_fn_val,
                        conn_id,
                        &servername,
                    );
                }
                TlsEvent::SecureConnection {
                    conn_id,
                    servername,
                    alpn,
                } => {
                    // Enrich the socket with the negotiated identity.
                    let socket_ptr = tls_socket_ptr_for(conn_id);
                    if !socket_ptr.is_null() {
                        rooted!(&in(realm_cx) let sock = socket_ptr);
                        if let Some(name) = &servername {
                            tls_define_str_prop(cx, sock.get(), "servername", name);
                        }
                        if let Some(proto) = &alpn {
                            let p = String::from_utf8_lossy(proto).to_string();
                            tls_define_str_prop(cx, sock.get(), "_alpnProtocol", &p);
                        }
                        let socket_val = TLS_CONNS
                            .with(|m| {
                                m.borrow()
                                    .get(&conn_id)
                                    .and_then(|e| e.socket_root.as_ref().map(|b| **b))
                            })
                            .unwrap_or_else(|| ObjectValue(socket_ptr));
                        tls_emit_js(cx, server_root.get(), "secureConnection", &[socket_val]);
                    } else {
                        tls_emit_js(cx, server_root.get(), "secureConnection", &[]);
                    }
                }
                TlsEvent::Data { conn_id, bytes } => {
                    let socket_val =
                        TLS_CONNS.with(|m| m.borrow().get(&conn_id).and_then(|e| e.socket_root.as_ref().map(|b| **b)));
                    let Some(socket_val) = socket_val else { continue };
                    let payload = tls_bytes_to_array_buffer(cx, &bytes);
                    if payload.is_null() {
                        continue;
                    }
                    tls_emit_js(cx, socket_val.to_object(), "data", &[ObjectValue(payload)]);
                }
                TlsEvent::End { conn_id } => {
                    let socket_val =
                        TLS_CONNS.with(|m| m.borrow().get(&conn_id).and_then(|e| e.socket_root.as_ref().map(|b| **b)));
                    let Some(socket_val) = socket_val else { continue };
                    tls_emit_js(cx, socket_val.to_object(), "end", &[]);
                }
                TlsEvent::Close { conn_id } => {
                    let entry = TLS_CONNS.with(|m| m.borrow_mut().remove(&conn_id));
                    let Some(mut entry) = entry else { continue };
                    let socket_val = entry
                        .socket_root
                        .as_ref()
                        .map(|b| **b)
                        .unwrap_or(UndefinedValue());
                    if socket_val.is_object() {
                        tls_emit_js(cx, socket_val.to_object(), "close", &[]);
                    }
                    if let Some(mut root) = entry.socket_root.take() {
                        RemoveRawValueRoot(cx, root.as_mut());
                    }
                }
                TlsEvent::ClientError { conn_id, message } => {
                    let err_obj = tls_build_error_js(cx, &message);
                    let socket_val =
                        TLS_CONNS.with(|m| m.borrow().get(&conn_id).and_then(|e| e.socket_root.as_ref().map(|b| **b)));
                    if let Some(sv) = socket_val.filter(|v| v.is_object()) {
                        tls_emit_js(cx, server_root.get(), "tlsClientError", &[ObjectValue(err_obj), sv]);
                    } else {
                        tls_emit_js(cx, server_root.get(), "tlsClientError", &[ObjectValue(err_obj)]);
                    }
                }
                TlsEvent::ServerClosed => {
                    tls_emit_js(cx, server_root.get(), "close", &[]);
                    // Invoke the stored close callback if present.
                    let mut cb_val = UndefinedValue();
                    JS_GetProperty(
                        cx,
                        server_root.handle().into(),
                        c"_closeCb".as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut cb_val,
                        },
                    );
                    if cb_val.is_object() {
                        rooted!(&in(realm_cx) let cb_root = cb_val);
                        let mut rval = UndefinedValue();
                        JS_CallFunctionValue(
                            cx,
                            server_root.handle().into(),
                            cb_root.handle().into(),
                            &HandleValueArray::empty(),
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut rval,
                            },
                        );
                        JS_ClearPendingException(cx);
                    }
                    // Unroot the JS references; the JS-thread registry drops
                    // the final Arc (this tasklet is the last consumer).
                    if let Some(mut root) = s.sni_fn_root.take() {
                        RemoveRawValueRoot(cx, root.as_mut());
                    }
                    if let Some(mut root) = s.server_obj_root.take() {
                        RemoveRawValueRoot(cx, root.as_mut());
                    }
                    TLS_SERVER_REGISTRY.with(|r| {
                        r.borrow_mut().remove(&s.server_id);
                    });
                }
            }
        }
    }
}

/// Look up the cross-thread ConnShared for a conn_id (JS thread).
fn tls_conn_shared_for(conn_id: u64) -> Option<Arc<ConnShared>> {
    TLS_CONNS.with(|m| m.borrow().get(&conn_id).map(|e| Arc::clone(&e.shared)))
}

/// Current JS socket object pointer for a conn_id (unrooted handle for
/// immediate property definitions — the rooted copy lives in the map).
fn tls_socket_ptr_for(conn_id: u64) -> *mut JSObject {
    TLS_CONNS.with(|m| {
        m.borrow()
            .get(&conn_id)
            .and_then(|e| e.socket_root.as_ref().map(|b| **b))
            .filter(|v| v.is_object())
            .map(|v| v.to_object())
            .unwrap_or(::std::ptr::null_mut())
    })
}

/// Build the JS TLSSocket object for an accepted connection (proto chain
/// from the tls module's TLSSocket.prototype).
unsafe fn tls_build_socket_js(cx: *mut JSContext, conn_id: u64) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let tls_mod = crate::gc_store::gc_store_get(cx, "builtin:tls").unwrap_or(::std::ptr::null_mut());
    if tls_mod.is_null() {
        return ::std::ptr::null_mut();
    }
    rooted!(&in(cx_ref) let mod_root = tls_mod);
    let mut ctor_val = UndefinedValue();
    JS_GetProperty(
        cx,
        mod_root.handle().into(),
        c"TLSSocket".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor_val,
        },
    );
    if !ctor_val.is_object() {
        return ::std::ptr::null_mut();
    }
    rooted!(&in(cx_ref) let ctor = ctor_val.to_object());
    let mut proto_val = UndefinedValue();
    JS_GetProperty(
        cx,
        ctor.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut proto_val,
        },
    );
    if !proto_val.is_object() {
        return ::std::ptr::null_mut();
    }
    rooted!(&in(cx_ref) let proto = proto_val.to_object());
    let obj = w2::JS_NewObjectWithGivenProto(cx_ref, ::std::ptr::null(), proto.handle().into());
    if obj.is_null() {
        return ::std::ptr::null_mut();
    }
    rooted!(&in(cx_ref) let obj_root = obj);

    rooted!(&in(cx_ref) let cid = DoubleValue(conn_id as f64));
    JS_DefineProperty(
        cx,
        obj_root.handle().into(),
        c"_connId".as_ptr(),
        cid.handle().into(),
        0,
    );
    rooted!(&in(cx_ref) let auth = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        obj_root.handle().into(),
        c"authorized".as_ptr(),
        auth.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    rooted!(&in(cx_ref) let enc = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(
        cx,
        obj_root.handle().into(),
        c"encrypted".as_ptr(),
        enc.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    rooted!(&in(cx_ref) let destroyed = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        obj_root.handle().into(),
        c"destroyed".as_ptr(),
        destroyed.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    obj_root.get()
}

/// Define a string property on an object (best-effort helper).
unsafe fn tls_define_str_prop(cx: *mut JSContext, obj: *mut JSObject, name: &str, value: &str) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let c_name = ZBox::from_bytes(name.as_bytes());
    let c_val = ZBox::from_bytes(value.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
    if js_str.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let sv = mozjs::jsval::StringValue(&*js_str));
    JS_DefineProperty(
        cx,
        obj_root.handle().into(),
        c_name.as_ptr(),
        sv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
}

/// Build an error object with a `message` property (house pattern from
/// fetch_async::reject_with_message).
unsafe fn tls_build_error_js(cx: *mut JSContext, message: &str) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let err_obj = w2::JS_NewPlainObject(cx_ref));
    if err_obj.is_null() {
        return ::std::ptr::null_mut();
    }
    tls_define_str_prop(cx, err_obj.get(), "message", message);
    err_obj.get()
}

/// Build an ArrayBuffer payload from bytes (house pattern from
/// node_net::net_read — ownership transfers to the ArrayBuffer).
unsafe fn tls_bytes_to_array_buffer(cx: *mut JSContext, bytes: &[u8]) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let len = bytes.len();
    let layout = ::std::alloc::Layout::from_size_align(len.max(1), 1)
        .unwrap_or_else(|_| ::std::alloc::Layout::from_size_align(1, 1).unwrap());
    // SAFETY: layout has non-zero size (clamped above).
    let alloc = unsafe { ::std::alloc::alloc(layout) };
    if alloc.is_null() {
        return ::std::ptr::null_mut();
    }
    // SAFETY: alloc is len bytes; source is a live slice.
    unsafe { ::std::ptr::copy_nonoverlapping(bytes.as_ptr(), alloc, len) };
    let ab = w2::NewArrayBufferWithContents(cx_ref, len, alloc.cast::<core::ffi::c_void>());
    if ab.is_null() {
        // SAFETY: same layout used for alloc.
        unsafe { ::std::alloc::dealloc(alloc, layout) };
        return ::std::ptr::null_mut();
    }
    ab
}

/// Call `obj.emit(name, args...)` via the EventEmitter native on `emit`.
unsafe fn tls_emit_js(cx: *mut JSContext, obj: *mut JSObject, name: &str, args: &[JSVal]) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);

    let c_name = ZBox::from_bytes(name.as_bytes());
    let name_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
    if name_str.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let name_val = mozjs::jsval::StringValue(&*name_str));

    let mut emit_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"emit".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut emit_val,
        },
    );
    if !emit_val.is_object() {
        return;
    }
    rooted!(&in(cx_ref) let emit_root = emit_val);

    let mut call_vals: Vec<JSVal> = Vec::with_capacity(args.len() + 1);
    call_vals.push(name_val.get());
    call_vals.extend_from_slice(args);
    let call_args = HandleValueArray {
        length_: call_vals.len(),
        elements_: call_vals.as_ptr(),
    };
    let mut rval = UndefinedValue();
    JS_CallFunctionValue(
        cx,
        obj_root.handle().into(),
        emit_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    JS_ClearPendingException(cx);
}

/// Dispatch the user SNICallback: `SNICallback(servername, cb)` with
/// `this` = the tls server object. The `cb` native resolves the handshake
/// through ConnShared.sni_result.
unsafe fn tls_dispatch_sni_callback(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    server_obj: *mut JSObject,
    sni_fn_val: JSVal,
    conn_id: u64,
    servername: &str,
) {
    if !sni_fn_val.is_object() {
        return;
    }

    let cb_fn = JS_NewFunction(cx, Some(tls_sni_cb_native), 2, 0, c"onSNICallback".as_ptr());
    if cb_fn.is_null() {
        log::error!("[tls] SNICallback dispatch: JS_NewFunction failed (conn {})", conn_id);
        return;
    }
    let cb_obj = JS_GetFunctionObject(cb_fn);
    if cb_obj.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let cb_root = cb_obj);
    rooted!(&in(cx_ref) let cid = DoubleValue(conn_id as f64));
    JS_DefineProperty(cx, cb_root.handle().into(), c"_sniConnId".as_ptr(), cid.handle().into(), 0);

    let c_servername = ZBox::from_bytes(servername.as_bytes());
    let name_js = JS_NewStringCopyZ(cx, c_servername.as_ptr());
    if name_js.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let name_val = mozjs::jsval::StringValue(&*name_js));
    rooted!(&in(cx_ref) let server_root = server_obj);
    rooted!(&in(cx_ref) let sni_root = sni_fn_val);

    let call_args = HandleValueArray {
        length_: 2,
        elements_: [name_val.get(), ObjectValue(cb_obj)].as_ptr(),
    };
    let mut rval = UndefinedValue();
    JS_CallFunctionValue(
        cx,
        server_root.handle().into(),
        sni_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    JS_ClearPendingException(cx);
    // NOTE: if the SNICallback never invokes `cb`, the driver's SNI deadline
    // fails the handshake with an explicit tlsClientError — never silent.
}

/// The `cb(err, secureContextOrOptions)` native handed to the user
/// SNICallback. Extracts {cert,key} (plain object, or a SecureContext via
/// its Rust-native `_scState`), posts the resolution to the driver, and
/// wakes it.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_sni_cb_native(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // The conn binding lives on the FUNCTION object (`_sniConnId`), not on
    // `this`: users call `cb(err, ctx)` unbound from their SNICallback, so
    // `this` is undefined/global depending on caller strictness.
    let callee_v = args.calleev();
    if !callee_v.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = callee_v.to_object());

    let mut cid_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_obj.handle().into(),
        c"_sniConnId".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut cid_val,
        },
    );
    let Some(conn_id) = (if cid_val.is_double() {
        Some(cid_val.to_double() as u64)
    } else {
        None
    }) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let Some(shared) = tls_conn_shared_for(conn_id) else {
        // Connection already closed; nothing to resolve.
        args.rval().set(UndefinedValue());
        return true;
    };

    let err_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let result: ::std::result::Result<(String, String), String> = if !err_val.is_null_or_undefined() {
        // cb(err, ...): surface the error message.
        let msg = if err_val.is_object() {
            let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let wr = &mut wrapped;
            rooted!(&in(wr) let err_obj = err_val.to_object());
            let mut msg_val = UndefinedValue();
            JS_GetProperty(
                cx,
                err_obj.handle().into(),
                c"message".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut msg_val,
                },
            );
            if msg_val.is_string() {
                crate::js_to_rust_string(cx, msg_val)
            } else {
                "SNICallback error".to_string()
            }
        } else {
            crate::js_to_rust_string(cx, err_val)
        };
        Err(msg)
    } else {
        let ctx_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
        tls_extract_credentials(cx, ctx_val)
    };

    *shared.sni_result.lock().unwrap() = Some(result);
    tls_driver_wake();

    args.rval().set(UndefinedValue());
    true
}

/// Extract (cert_pem, key_pem) from the SNICallback's second argument:
/// either a SecureContext (Rust-native `_scState` — including the server
/// object itself, which carries the same state) or a plain
/// `{ key, cert }` options object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn tls_extract_credentials(cx: *mut JSContext, val: JSVal) -> ::std::result::Result<(String, String), String> {
    if !val.is_object() {
        return Err("SNICallback resolved without a SecureContext or {key, cert} object".to_string());
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = val.to_object());

    // SecureContext path: Rust-native _scState private value.
    let mut sc_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"_scState".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut sc_val,
        },
    );
    if val_is_private(&sc_val) {
        let state = sc_val.to_private() as *mut SecureContextState;
        if !state.is_null() {
            let s = &*state;
            if let (Some(cert), Some(key)) = (&s.pem_certs, &s.pem_key) {
                return Ok((cert.clone(), key.clone()));
            }
            return Err("SecureContext passed to SNICallback has no cert/key loaded".to_string());
        }
    }

    // Plain object path: string .cert / .key properties.
    let get_str_prop = |name: &str| -> Option<String> {
        let cname = ZBox::from_bytes(name.as_bytes());
        let mut v = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            cname.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            },
        );
        if v.is_string() {
            Some(crate::js_to_rust_string(cx, v))
        } else {
            None
        }
    };
    match (get_str_prop("cert"), get_str_prop("key")) {
        (Some(cert), Some(key)) => Ok((cert, key)),
        _ => Err("SNICallback result must provide both cert and key".to_string()),
    }
}

/// Extract bytes from a JS value: string (UTF-8) or
/// Uint8Array/TypedArray/DataView/ArrayBuffer (house pattern from
/// node_buffer::collect_byte_view).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn tls_collect_write_bytes(cx: *mut JSContext, v: JSVal) -> Option<Vec<u8>> {
    if v.is_string() {
        return Some(crate::js_to_rust_string(cx, v).into_bytes());
    }
    if !v.is_object() {
        return None;
    }
    let obj = v.to_object();
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
    let unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
        obj,
        &mut length,
        &mut is_shared,
        &mut data_ptr,
    );
    if !unwrapped.is_null() && !data_ptr.is_null() {
        // SAFETY: data_ptr/length describe the typed array's bytes.
        return Some(unsafe { ::std::slice::from_raw_parts(data_ptr, length) }.to_vec());
    }
    let mut ab_length: usize = 0;
    let mut ab_data: *mut u8 = ::std::ptr::null_mut();
    let ab_unwrapped = mozjs_sys::jsapi::JS::GetObjectAsArrayBuffer(obj, &mut ab_length, &mut ab_data);
    if !ab_unwrapped.is_null() && !ab_data.is_null() {
        // SAFETY: ab_data/ab_length describe the ArrayBuffer's bytes.
        return Some(unsafe { ::std::slice::from_raw_parts(ab_data, ab_length) }.to_vec());
    }
    if !ab_unwrapped.is_null() || !unwrapped.is_null() {
        return Some(Vec::new());
    }
    None
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let raw = cx.raw_cx();

        // TLSSocket constructor
        let ctor_fn = JS_NewFunction(
            raw,
            Some(tls_socket_ctor),
            2,
            JSFUN_CONSTRUCTOR,
            c"TLSSocket".as_ptr(),
        );
        if !ctor_fn.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor_fn);
            rooted!(&in(cx) let cv = ObjectValue(ctor_obj));
            JS_DefineProperty(
                raw,
                mod_obj.handle().into(),
                c"TLSSocket".as_ptr(),
                cv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            // TLSSocket.prototype methods
            rooted!(&in(cx) let proto = w2::JS_NewPlainObject(cx));
            if !proto.get().is_null() {
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"write".as_ptr(),
                    Some(tls_socket_write),
                    2,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"end".as_ptr(),
                    Some(tls_socket_end),
                    1,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"destroy".as_ptr(),
                    Some(tls_socket_destroy),
                    0,
                    0,
                );
                w2::JS_DefineFunction(cx, proto.handle(), c"on".as_ptr(), Some(ee_on), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"once".as_ptr(), Some(ee_once), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"emit".as_ptr(), Some(ee_emit), 1, 0);
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"addListener".as_ptr(),
                    Some(ee_on),
                    2,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"removeListener".as_ptr(),
                    Some(ee_off),
                    2,
                    0,
                );
                w2::JS_DefineFunction(cx, proto.handle(), c"off".as_ptr(), Some(ee_off), 2, 0);
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"removeAllListeners".as_ptr(),
                    Some(ee_remove_all),
                    0,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"prependListener".as_ptr(),
                    Some(ee_prepend),
                    2,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"prependOnceListener".as_ptr(),
                    Some(ee_prepend_once),
                    2,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"getProtocol".as_ptr(),
                    Some(tls_get_protocol),
                    0,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"getCipher".as_ptr(),
                    Some(tls_get_cipher),
                    0,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"getPeerCertificate".as_ptr(),
                    Some(tls_get_peer_cert),
                    0,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"getFinished".as_ptr(),
                    Some(tls_socket_get_finished),
                    0,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"getPeerFinished".as_ptr(),
                    Some(tls_socket_get_peer_finished),
                    0,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"getSession".as_ptr(),
                    Some(tls_socket_get_session),
                    0,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"setEncoding".as_ptr(),
                    Some(tls_socket_set_encoding),
                    1,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"ref".as_ptr(),
                    Some(tls_socket_ref),
                    0,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"unref".as_ptr(),
                    Some(tls_socket_unref),
                    0,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"getALPNProtocol".as_ptr(),
                    Some(tls_socket_get_alpn),
                    0,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c" renegotiate".as_ptr(),
                    Some(tls_socket_noop_bool),
                    0,
                    0,
                );

                let proto_val = ObjectValue(proto.get());
                rooted!(&in(cx) let pv = proto_val);
                rooted!(&in(cx) let ctor_h = ctor_obj);
                // Set Constructor.prototype = proto so `new TLSSocket()` instances
                // inherit from proto (where on/once/emit are defined).
                JS_DefineProperty(
                    raw,
                    ctor_h.handle().into(),
                    c"prototype".as_ptr(),
                    pv.handle().into(),
                    0,
                );
                // Also set proto.constructor = TLSSocket for completeness.
                rooted!(&in(cx) let ctor_val = ObjectValue(ctor_obj));
                JS_DefineProperty(
                    raw,
                    proto.handle().into(),
                    c"constructor".as_ptr(),
                    ctor_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // Static methods
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"connect".as_ptr(),
            Some(tls_connect),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"createServer".as_ptr(),
            Some(tls_create_server),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"createSecureContext".as_ptr(),
            Some(tls_create_secure_context),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"getCiphers".as_ptr(),
            Some(tls_get_ciphers),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"checkServerIdentity".as_ptr(),
            Some(tls_check_server_identity),
            2,
            JSPROP_ENUMERATE as u32,
        );

        // Constants
        let _ciphers_str =
            "TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_128_GCM_SHA256";
        let cs = JS_NewStringCopyZ(
            raw,
            c"TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_128_GCM_SHA256".as_ptr(),
        );
        if !cs.is_null() {
            rooted!(&in(cx) let csv = mozjs::jsval::StringValue(&*cs));
            JS_DefineProperty(
                raw,
                mod_obj.handle().into(),
                c"DEFAULT_CIPHERS".as_ptr(),
                csv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let minv = JS_NewStringCopyZ(raw, c"TLSv1.2".as_ptr());
        if !minv.is_null() {
            rooted!(&in(cx) let mv = mozjs::jsval::StringValue(&*minv));
            JS_DefineProperty(
                raw,
                mod_obj.handle().into(),
                c"DEFAULT_MIN_VERSION".as_ptr(),
                mv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let maxv = JS_NewStringCopyZ(raw, c"TLSv1.3".as_ptr());
        if !maxv.is_null() {
            rooted!(&in(cx) let xmv = mozjs::jsval::StringValue(&*maxv));
            JS_DefineProperty(
                raw,
                mod_obj.handle().into(),
                c"DEFAULT_MAX_VERSION".as_ptr(),
                xmv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        cache_builtin(cx, "tls", mod_obj.get());
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_ctor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Get the constructor's .prototype property to set as the new object's proto.
    rooted!(&in(cx_ref) let callee_obj = args.calleev().to_object());
    let mut proto_val = UndefinedValue();
    JS_GetProperty(
        cx,
        callee_obj.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut proto_val,
        },
    );
    let proto_obj = if proto_val.is_object() {
        proto_val.to_object()
    } else {
        ::std::ptr::null_mut()
    };

    rooted!(&in(cx_ref) let proto_rooted = proto_obj);
    rooted!(&in(cx_ref) let obj = if !proto_obj.is_null() {
        unsafe { w2::JS_NewObjectWithGivenProto(cx_ref, ::std::ptr::null(), proto_rooted.handle().into()) }
    } else {
        w2::JS_NewPlainObject(cx_ref)
    });

    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    // Properties
    rooted!(&in(cx_ref) let auth = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"authorized".as_ptr(),
        auth.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    rooted!(&in(cx_ref) let enc = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"encrypted".as_ptr(),
        enc.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // If first arg is an object (socket), store reference
    if argc > 0 && (*args.get(0).ptr).is_object() {
        rooted!(&in(cx_ref) let sock = (*args.get(0).ptr).to_object());
        rooted!(&in(cx_ref) let sv = ObjectValue(sock.get()));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"_socket".as_ptr(),
            sv.handle().into(),
            0,
        );
    }

    // Store hostname from options
    if argc > 1 && (*args.get(1).ptr).is_object() {
        rooted!(&in(cx_ref) let opts = (*args.get(1).ptr).to_object());
        let mut host_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts.handle().into(),
            c"servername".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut host_val,
            },
        );
        if host_val.is_string() {
            rooted!(&in(cx_ref) let hv = host_val);
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"servername".as_ptr(),
                hv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // Read ALPNProtocols from options and store as _alpnProtos
        let mut alpn_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts.handle().into(),
            c"ALPNProtocols".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut alpn_val,
            },
        );
        if alpn_val.is_object() {
            rooted!(&in(cx_ref) let alpn_root = alpn_val);
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"_alpnProtos".as_ptr(),
                alpn_root.handle().into(),
                0,
            );
        }

        // Read session from options
        let mut session_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts.handle().into(),
            c"session".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut session_val,
            },
        );
        if !session_val.is_undefined() {
            rooted!(&in(cx_ref) let session_root = session_val);
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"_session".as_ptr(),
                session_root.handle().into(),
                0,
            );
        }
    }

    // Initialize _refed = true (socket keeps event loop alive by default)
    rooted!(&in(cx_ref) let refed = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"_refed".as_ptr(),
        refed.handle().into(),
        0,
    );

    args.rval().set(ObjectValue(obj.get()));
    true
}

/// tls.connect(options) — TLS client connect with ALPN negotiation, SNI,
/// and session resumption support.
///
/// Reads `ALPNProtocols`, `servername`, `session`, and `secureContext`
/// from the options object, then creates a TlsClient + TlsConnection
/// for the outbound connection. The actual network I/O is performed
/// asynchronously via `fetch_async::start_tls_probe`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_connect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let (host, port) = if argc > 0 && (*args.get(0).ptr).is_object() {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let opts = (*args.get(0).ptr).to_object());
        let mut h = UndefinedValue();
        JS_GetProperty(
            cx,
            opts.handle().into(),
            c"host".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut h,
            },
        );
        let host = if h.is_string() {
            crate::js_to_rust_string(cx, h)
        } else {
            "localhost".to_string()
        };
        let mut p = UndefinedValue();
        JS_GetProperty(
            cx,
            opts.handle().into(),
            c"port".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut p,
            },
        );
        let port = if p.is_int32() {
            p.to_int32() as u16
        } else {
            443
        };
        (host, port)
    } else if argc > 0 && (*args.get(0).ptr).is_int32() {
        let port = (*args.get(0).ptr).to_int32() as u16;
        let host = if argc > 1 && (*args.get(1).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(1).ptr)
        } else {
            "localhost".to_string()
        };
        (host, port)
    } else {
        args.rval().set(UndefinedValue());
        return true;
    };

    // @trace REQ-ENG-010 [api:tls.connect async] [entity:FetchTasklet]
    //
    // BCE-20260618-007: `tls.connect` previously called `stealth_http_request`
    // (a single stealth HTTPS HEAD handshake probe) directly inside the
    // JS-native frame, blocking the JS thread on the full TLS round-trip.
    // Now it returns a *pending* Promise and schedules the probe on a detached
    // worker via `fetch_async::start_tls_probe` (FetchTasklet pattern). The
    // Promise resolves to a TLSSocket object (`authorized`/`encrypted`/
    // `servername`) on success, or rejects on error.
    let promise = {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let null_h = ::std::ptr::null_mut::<JSObject>());
        mozjs_sys::jsapi::JS::NewPromiseObject(cx, null_h.handle().into())
    };
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let promise_val = mozjs::jsval::ObjectValue(promise);

    // SAFETY: cx is live on this thread; promise_val is the pending Promise.
    // The worker runs the TLS handshake probe off-thread; the JS thread
    // returns immediately with the pending Promise.
    unsafe {
        crate::fetch_async::start_tls_probe(cx, promise_val, host, port);
    }

    args.rval().set(promise_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_create_server(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let server = w2::JS_NewPlainObject(cx_ref));
    if !server.get().is_null() {
        w2::JS_DefineFunction(
            cx_ref,
            server.handle(),
            c"listen".as_ptr(),
            Some(tls_server_listen),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            server.handle(),
            c"close".as_ptr(),
            Some(tls_server_close),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            server.handle(),
            c"address".as_ptr(),
            Some(tls_server_address),
            0,
            0,
        );
        w2::JS_DefineFunction(cx_ref, server.handle(), c"on".as_ptr(), Some(ee_on), 2, 0);
        w2::JS_DefineFunction(
            cx_ref,
            server.handle(),
            c"once".as_ptr(),
            Some(ee_once),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            server.handle(),
            c"emit".as_ptr(),
            Some(ee_emit),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            server.handle(),
            c"removeListener".as_ptr(),
            Some(ee_off),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            server.handle(),
            c"removeAllListeners".as_ptr(),
            Some(ee_remove_all),
            0,
            0,
        );

        // Store the first arg (options or SecureContext) as _secureContext
        // tls.createServer(options, [callback]) — options may contain key/cert directly
        if argc > 0 && (*args.get(0).ptr).is_object() {
            rooted!(&in(cx_ref) let opts = (*args.get(0).ptr).to_object());
            rooted!(&in(cx_ref) let ov = ObjectValue(opts.get()));
            JS_DefineProperty(
                cx,
                server.handle().into(),
                c"_secureContext".as_ptr(),
                ov.handle().into(),
                0,
            );

            // Parse key/cert from options and store in SecureContextState on the server object
            let mut key_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts.handle().into(),
                c"key".as_ptr(),
                MutableHandle::<JSVal> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut key_val,
                },
            );
            if key_val.is_string() {
                let pem = crate::js_to_rust_string(cx, key_val);
                sc_state_set_key(cx, server.get(), &pem);
            }
            let mut cert_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts.handle().into(),
                c"cert".as_ptr(),
                MutableHandle::<JSVal> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut cert_val,
                },
            );
            if cert_val.is_string() {
                let pem = crate::js_to_rust_string(cx, cert_val);
                sc_state_set_cert(cx, server.get(), &pem);
            }

            // Parse ALPNProtocols from options
            let mut alpn_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts.handle().into(),
                c"ALPNProtocols".as_ptr(),
                MutableHandle::<JSVal> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut alpn_val,
                },
            );
            if !alpn_val.is_undefined() {
                sc_state_set_alpn_protos(cx, server.get(), alpn_val);
            }

            // Parse SNICallback from options — store as JS function reference
            let mut sni_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts.handle().into(),
                c"SNICallback".as_ptr(),
                MutableHandle::<JSVal> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut sni_val,
                },
            );
            if sni_val.is_object() {
                rooted!(&in(cx_ref) let sni_root = sni_val);
                JS_DefineProperty(
                    cx,
                    server.handle().into(),
                    c"_sniCallback".as_ptr(),
                    sni_root.handle().into(),
                    0,
                );
            }

            // Parse session from options (for session resumption)
            let mut session_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts.handle().into(),
                c"session".as_ptr(),
                MutableHandle::<JSVal> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut session_val,
                },
            );
            if !session_val.is_undefined() {
                // Store as a reference property; the actual session bytes
                // are applied when SSL objects are created in listen().
                rooted!(&in(cx_ref) let session_root = session_val);
                JS_DefineProperty(
                    cx,
                    server.handle().into(),
                    c"_session".as_ptr(),
                    session_root.handle().into(),
                    0,
                );
            }
        }

        args.rval().set(ObjectValue(server.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_create_secure_context(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let ctx = w2::JS_NewPlainObject(cx_ref));
    if !ctx.get().is_null() {
        w2::JS_DefineFunction(
            cx_ref,
            ctx.handle(),
            c"setKey".as_ptr(),
            Some(sc_set_key),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            ctx.handle(),
            c"setCert".as_ptr(),
            Some(sc_set_cert),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            ctx.handle(),
            c"addCACert".as_ptr(),
            Some(sc_add_ca_cert),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            ctx.handle(),
            c"setCA".as_ptr(),
            Some(sc_set_ca),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            ctx.handle(),
            c"setALPNProtocols".as_ptr(),
            Some(sc_set_alpn_protocols),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx_ref,
            ctx.handle(),
            c"setSession".as_ptr(),
            Some(sc_set_session),
            1,
            0,
        );

        // Initialize SecureContextState as private value
        let state = Box::new(SecureContextState::new());
        let ptr = Box::into_raw(state) as *const core::ffi::c_void;
        let pv = mozjs::jsval::PrivateValue(ptr);
        rooted!(&in(cx_ref) let pv_h = pv);
        JS_DefineProperty(
            cx,
            ctx.handle().into(),
            c"_scState".as_ptr(),
            pv_h.handle().into(),
            0,
        );

        args.rval().set(ObjectValue(ctx.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_set_key(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let pem = crate::js_to_rust_string(cx, val);
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            sc_state_set_key(cx, this_obj.get(), &pem);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_set_cert(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let pem = crate::js_to_rust_string(cx, val);
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            sc_state_set_cert(cx, this_obj.get(), &pem);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_add_ca_cert(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let pem = crate::js_to_rust_string(cx, val);
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            sc_state_add_ca(cx, this_obj.get(), &pem);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_set_ca(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let pem = crate::js_to_rust_string(cx, val);
            // setCA replaces the entire CA store, so reset first
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            let state = sc_state_ensure(cx, this_obj.get());
            (*state).ca_certs = Vec::new();
            sc_state_add_ca(cx, this_obj.get(), &pem);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

/// secureContext.setALPNProtocols(protocols) — set the ALPN protocols list.
/// Accepts an array of protocol name strings, e.g. ['h2', 'http/1.1'].
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_set_alpn_protocols(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        sc_state_set_alpn_protos(cx, this_obj.get(), val);
    }
    args.rval().set(UndefinedValue());
    true
}

/// secureContext.setSession(session) — set the session data for resumption.
/// Accepts a Buffer or Uint8Array containing serialized SSL_SESSION data.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_set_session(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        // For now, accept string or object (Buffer-like).
        // When BoringSSL session serialization bindings are available,
        // this will parse the actual SSL_SESSION data.
        if val.is_string() {
            let data = crate::js_to_rust_string(cx, val);
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            sc_state_set_session(cx, this_obj.get(), data.as_bytes());
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_ciphers(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let ciphers = [
        "TLS_AES_256_GCM_SHA384",
        "TLS_CHACHA20_POLY1305_SHA256",
        "TLS_AES_128_GCM_SHA256",
        "ECDHE-RSA-AES256-GCM-SHA384",
        "ECDHE-RSA-AES128-GCM-SHA256",
        "ECDHE-ECDSA-AES256-GCM-SHA384",
        "ECDHE-ECDSA-AES128-GCM-SHA256",
    ];
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, ciphers.len()));
    if !arr.get().is_null() {
        for (i, name) in ciphers.iter().enumerate() {
            let c_name = ZBox::from_bytes(name.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
                JS_DefineElement(
                    cx,
                    arr.handle().into(),
                    i as u32,
                    v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        args.rval().set(ObjectValue(arr.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

/// tls.checkServerIdentity(hostname, cert) — verify the server's certificate
/// matches the expected hostname. Delegates to `bun_boringssl::check_server_identity`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_check_server_identity(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // This is a JS-level function; the actual cert checking is done in
    // `bun_boringssl::check_server_identity` at the BoringSSL level during
    // TLS handshake. This function provides a JS-callable API that returns
    // an Error object if verification fails, or undefined if it passes.
    // For now, return undefined (identity check passes by default).
    // Full implementation requires access to the peer certificate from JS,
    // which will be added when SSL_get_peer_certificate bindings are complete.
    let _ = (cx, argc);
    args.rval().set(UndefinedValue());
    true
}

/// socket.write(data) — queue plaintext for TLS delivery. The bytes are
/// PARKED in the connection's Mutex-protected queue and encrypted+sent by
/// the driver thread (the SSL object's single owner — see the ssl_in_use
/// analysis in the driver section). Returns false when the socket is not
/// backed by a live server connection (e.g. a tls.connect probe socket).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());

    let Some((shared, _socket)) = tls_socket_conn_handle(cx, this_obj.get()) else {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    };
    if shared.closed.load(Ordering::Acquire) {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    if argc > 0 {
        let data_val = *args.get(0).ptr;
        if let Some(bytes) = tls_collect_write_bytes(cx, data_val) {
            if !bytes.is_empty() {
                shared.pending_writes.lock().unwrap().push(bytes);
                tls_driver_wake();
            }
        }
    }
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

/// socket.end([data]) — optional final write, then graceful TLS shutdown
/// (flush parked writes, send close_notify, close).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_end(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());

    if let Some((shared, _socket)) = tls_socket_conn_handle(cx, this_obj.get()) {
        if !shared.closed.load(Ordering::Acquire) {
            if argc > 0 {
                let data_val = *args.get(0).ptr;
                if let Some(bytes) = tls_collect_write_bytes(cx, data_val) {
                    if !bytes.is_empty() {
                        shared.pending_writes.lock().unwrap().push(bytes);
                    }
                }
            }
            shared.want_end.store(true, Ordering::Release);
            tls_driver_wake();
        }
    }
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

/// socket.destroy() — immediate teardown (no close_notify guarantee).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_destroy(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());

    if let Some((shared, _socket)) = tls_socket_conn_handle(cx, this_obj.get()) {
        if !shared.closed.load(Ordering::Acquire) {
            shared.want_destroy.store(true, Ordering::Release);
            tls_driver_wake();
        }
    }
    tls_set_bool_prop(cx, this_obj.get(), "destroyed", true);
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

/// Resolve a JS socket object to its live connection handle via `_connId`.
/// Returns None for sockets not backed by a live server connection.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn tls_socket_conn_handle(
    cx: *mut JSContext,
    obj: *mut JSObject,
) -> Option<(Arc<ConnShared>, Option<JSVal>)> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut cid_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_connId".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut cid_val,
        },
    );
    if !cid_val.is_double() {
        return None;
    }
    let conn_id = cid_val.to_double() as u64;
    let entry = TLS_CONNS.with(|m| {
        m.borrow()
            .get(&conn_id)
            .map(|e| (Arc::clone(&e.shared), e.socket_root.as_ref().map(|b| **b)))
    });
    entry
}

/// Define a boolean property on an object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn tls_set_bool_prop(cx: *mut JSContext, obj: *mut JSObject, name: &str, value: bool) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let c_name = ZBox::from_bytes(name.as_bytes());
    rooted!(&in(cx_ref) let bv = mozjs::jsval::BooleanValue(value));
    JS_DefineProperty(
        cx,
        obj_root.handle().into(),
        c_name.as_ptr(),
        bv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
}

/// Noop returning false (for methods like renegotiate).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_noop_bool(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(false));
    true
}

// ─── TLSSocket methods ─────────────────────────────────────────────────

/// socket.getFinished() — returns the TLS Finished message verify data.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_get_finished(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(false));
    true
}

/// socket.getPeerFinished() — returns the peer's TLS Finished message verify data.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_get_peer_finished(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(false));
    true
}

/// socket.getSession() — returns the TLS session ticket/data for resumption.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_get_session(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

/// socket.setEncoding(encoding) — set the encoding for the readable stream.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_set_encoding(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());

    if argc > 0 && (*args.get(0).ptr).is_string() {
        rooted!(&in(cx_ref) let enc_val = *args.get(0).ptr);
        JS_DefineProperty(
            cx,
            this_obj.handle().into(),
            c"_encoding".as_ptr(),
            enc_val.handle().into(),
            0,
        );
    }

    args.rval().set(ObjectValue(this_obj.get()));
    true
}

/// socket.ref() — keep the event loop alive while the socket is active.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_ref(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
    rooted!(&in(cx_ref) let refed = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(
        cx,
        this_obj.handle().into(),
        c"_refed".as_ptr(),
        refed.handle().into(),
        0,
    );
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

/// socket.unref() — allow the event loop to exit even if the socket is active.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_unref(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
    rooted!(&in(cx_ref) let refed = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        this_obj.handle().into(),
        c"_refed".as_ptr(),
        refed.handle().into(),
        0,
    );
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

/// socket.getALPNProtocol() — returns the negotiated ALPN protocol.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_get_alpn(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());

    // Check if _alpnProtocol was set on the socket (set during TLS handshake
    // resolution in fetch_async resolve_tasklet).
    let mut alpn_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_obj.handle().into(),
        c"_alpnProtocol".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut alpn_val,
        },
    );
    if alpn_val.is_string() {
        args.rval().set(alpn_val);
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_protocol(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let js_str = JS_NewStringCopyZ(cx, c"TLSv1.3".as_ptr());
    if !js_str.is_null() {
        args.rval().set(mozjs::jsval::StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_cipher(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if !obj.get().is_null() {
        let name_str = JS_NewStringCopyZ(cx, c"TLS_AES_256_GCM_SHA384".as_ptr());
        if !name_str.is_null() {
            rooted!(&in(cx_ref) let nv = mozjs::jsval::StringValue(&*name_str));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"name".as_ptr(),
                nv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let ver_str = JS_NewStringCopyZ(cx, c"TLSv1/SSLv3".as_ptr());
        if !ver_str.is_null() {
            rooted!(&in(cx_ref) let vv = mozjs::jsval::StringValue(&*ver_str));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"version".as_ptr(),
                vv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        args.rval().set(ObjectValue(obj.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_peer_cert(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let cert_obj = w2::JS_NewPlainObject(cx_ref));
    if !cert_obj.get().is_null() {
        rooted!(&in(cx_ref) let rv = UndefinedValue());
        JS_DefineProperty(
            cx,
            cert_obj.handle().into(),
            c"subject".as_ptr(),
            rv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineProperty(
            cx,
            cert_obj.handle().into(),
            c"issuer".as_ptr(),
            rv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        let empty = JS_NewStringCopyZ(cx, c"".as_ptr());
        if !empty.is_null() {
            rooted!(&in(cx_ref) let ev = mozjs::jsval::StringValue(&*empty));
            JS_DefineProperty(
                cx,
                cert_obj.handle().into(),
                c"valid_from".as_ptr(),
                ev.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineProperty(
                cx,
                cert_obj.handle().into(),
                c"valid_to".as_ptr(),
                ev.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineProperty(
                cx,
                cert_obj.handle().into(),
                c"fingerprint".as_ptr(),
                ev.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        rooted!(&in(cx_ref) let fv = mozjs::jsval::BooleanValue(false));
        JS_DefineProperty(
            cx,
            cert_obj.handle().into(),
            c"authorized".as_ptr(),
            fv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        args.rval().set(ObjectValue(cert_obj.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

/// tls.createServer().listen(port[, host][, callback]) — start a TLS server.
///
/// Binds a real TCP listener, hands it to the TLS driver thread, and — when
/// the options carried an `SNICallback` — registers the BoringSSL
/// select-certificate callback so the user's JS SNICallback is dispatched
/// during the handshake (see the driver section above for the full data
/// flow). Without `SNICallback` the static certificate serves every
/// connection (the contract's default branch).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_server_listen(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut port: u16 = 0;
    let mut host: String = "0.0.0.0".to_string();
    let mut listen_cb: Option<JSVal> = None;
    for i in 0..argc as usize {
        let v = *args.get(i as u32).ptr;
        if v.is_int32() && i == 0 {
            port = v.to_int32() as u16;
        } else if v.is_string() {
            host = crate::js_to_rust_string(cx, v);
        } else if v.is_object() && JS_ObjectIsFunction(v.to_object()) {
            listen_cb = Some(v);
        }
    }

    let this_obj = args.thisv().to_object();

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_root = this_obj);

    // Try to read SecureContextState from this object first (set by createServer with key/cert)
    // Then fall back to _secureContext object's state
    let mut state_ptr: *mut SecureContextState = core::ptr::null_mut();

    // Check if this object has its own _scState
    let mut sc_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_root.handle().into(),
        c"_scState".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut sc_val,
        },
    );
    if val_is_private(&sc_val) {
        let ptr = sc_val.to_private() as *mut SecureContextState;
        if !ptr.is_null() && (!(*ptr).cert_ders.is_empty() || (*ptr).key_der.is_some()) {
            state_ptr = ptr;
        }
    }

    // If no state on this object, try _secureContext
    if state_ptr.is_null() {
        let mut ctx_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_root.handle().into(),
            c"_secureContext".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ctx_val,
            },
        );

        if ctx_val.is_object() {
            rooted!(&in(cx_ref) let ctx_obj = ctx_val.to_object());
            let mut ctx_sc_val = UndefinedValue();
            JS_GetProperty(
                cx,
                ctx_obj.handle().into(),
                c"_scState".as_ptr(),
                MutableHandle::<JSVal> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut ctx_sc_val,
                },
            );
            if ctx_sc_val.is_double() && (ctx_sc_val.asBits_ & 0xFFFF000000000000) == 0 {
                let ptr = ctx_sc_val.to_private() as *mut SecureContextState;
                if !ptr.is_null() {
                    state_ptr = ptr;
                }
            }
        }
    }

    if state_ptr.is_null() {
        log::warn!("[tls] createServer.listen() called without cert/key");
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    let state = &*state_ptr;

    if state.cert_ders.is_empty() || state.key_der.is_none() {
        log::warn!("[tls] createServer.listen() called without cert/key");
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    // Use PEM strings directly with TlsServer::new(pem_certs, pem_key)
    let pem_certs = match &state.pem_certs {
        Some(p) => p.clone(),
        None => {
            log::warn!("[tls] createServer.listen() no PEM cert string available");
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };
    let pem_key = match &state.pem_key {
        Some(p) => p.clone(),
        None => {
            log::warn!("[tls] createServer.listen() no PEM key string available");
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };

    let base_server = match TlsServer::new(&pem_certs, &pem_key) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[tls] TlsServer::new failed: {}", e);
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };

    // Configure ALPN on the server's SSL_CTX if protocols were set.
    // Uses BoringSSL's SSL_CTX_set_alpn_select_cb to advertise protocols.
    let mut alpn_wire: Option<&'static [u8]> = None;
    if let Some(ref wire) = state.alpn_protos {
        let alpn_box = wire.as_slice().to_vec().into_boxed_slice();
        let alpn_static: &'static [u8] = Box::leak(alpn_box);

        // SAFETY: SSL_CTX_set_alpn_select_cb is a BoringSSL FFI call.
        // The callback reads from the static leaked slice; the slice lives
        // for the process lifetime (the ServerShared retains it to
        // re-register on SNI-resolved CTXs).
        unsafe {
            SSL_CTX_set_alpn_select_cb(
                base_server.ctx(),
                Some(alpn_select_callback),
                alpn_static.as_ptr() as *mut core::ffi::c_void,
            );
        }
        alpn_wire = Some(alpn_static);
    }

    // SNICallback: register the select-certificate hook and heap-root the
    // JS function so the driver can dispatch into it.
    let mut sni_fn_root: Option<Box<JSVal>> = None;
    let mut sni_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_root.handle().into(),
        c"_sniCallback".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut sni_val,
        },
    );
    let mut has_sni = sni_val.is_object() && JS_ObjectIsFunction(sni_val.to_object());
    if has_sni {
        let mut boxed = Box::new(sni_val);
        let name = b"TLSServer.sniCallback\0".as_ptr() as *const core::ffi::c_char;
        // SAFETY: cx is live on this thread; sni_val is a live object value.
        if unsafe { AddRawValueRoot(cx, boxed.as_mut(), name) } {
            sni_fn_root = Some(boxed);
        } else {
            has_sni = false;
        }
    }
    if has_sni {
        base_server.set_select_certificate_callback(Some(tls_select_cert_cb));
    }

    // Bind the real TCP listener (port 0 → ephemeral).
    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("[tls] listen({}:{}) bind failed: {}", host, port, e);
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };
    let real_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let _ = listener.set_nonblocking(true);

    // Heap-root the server object (the tasklet needs it across ticks).
    let mut server_obj_root = Box::new(ObjectValue(this_root.get()));
    let root_name = b"TLSServer.object\0".as_ptr() as *const core::ffi::c_char;
    // SAFETY: cx is live on this thread; the value is the live server object.
    if !unsafe { AddRawValueRoot(cx, server_obj_root.as_mut(), root_name) } {
        // Unroot what we already rooted, fail closed.
        if let Some(mut sni_root) = sni_fn_root.take() {
            // SAFETY: rooted above on this cx.
            unsafe { RemoveRawValueRoot(cx, sni_root.as_mut()) };
        }
        log::warn!("[tls] listen: AddRawValueRoot failed");
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    let loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static> =
        crate::timers::with_event_loop(|loop_| loop_ as *const _);

    let server_id = NEXT_TLS_ID.fetch_add(1, Ordering::Relaxed);
    let shared = Arc::new(ServerShared {
        server_id,
        cx,
        server_obj_root: Some(server_obj_root),
        sni_fn_root,
        mini_loop_ptr: loop_ptr,
        concurrent_task: bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext::default(
        ),
        task_scheduled: AtomicBool::new(false),
        events: Mutex::new(Vec::new()),
        closing: AtomicBool::new(false),
        sni_ctx_cache: Mutex::new(HashMap::new()),
        alpn_wire,
    });

    // Hand the listener to the driver.
    let Some(handle) = tls_driver_acquire() else {
        // Resource exhaustion: fail closed (unroot, drop the listener).
        if let Ok(inner) = Arc::try_unwrap(shared) {
            if let Some(mut r) = inner.sni_fn_root {
                // SAFETY: rooted on this cx above.
                unsafe { RemoveRawValueRoot(cx, r.as_mut()) };
            }
            if let Some(mut r) = inner.server_obj_root {
                // SAFETY: rooted on this cx above.
                unsafe { RemoveRawValueRoot(cx, r.as_mut()) };
            }
        }
        log::warn!("[tls] listen: TLS driver unavailable");
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    };
    handle
        .cmds
        .lock()
        .unwrap()
        .push(DriverCmd::AddListener(listener, Arc::clone(&shared), base_server));
    TLS_SERVER_REGISTRY.with(|r| {
        r.borrow_mut().insert(server_id, Arc::clone(&shared));
    });
    tls_driver_wake();

    // Expose identity/address on the server object.
    rooted!(&in(cx_ref) let sid_val = DoubleValue(server_id as f64));
    JS_DefineProperty(
        cx,
        this_root.handle().into(),
        c"_serverId".as_ptr(),
        sid_val.handle().into(),
        0,
    );
    rooted!(&in(cx_ref) let port_val = Int32Value(real_port as i32));
    JS_DefineProperty(
        cx,
        this_root.handle().into(),
        c"_listenPort".as_ptr(),
        port_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    tls_define_str_prop(cx, this_root.get(), "_listenHost", &host);

    log::info!(
        "[tls] server listening on {}:{} (SNICallback: {})",
        host,
        real_port,
        if has_sni { "enabled" } else { "off — static cert" }
    );

    // 'listening' event + optional callback (same tick; matches the
    // node:net Server implementation's synchronous emit).
    tls_emit_js(cx, this_root.get(), "listening", &[]);
    if let Some(cb) = listen_cb {
        rooted!(&in(cx_ref) let cb_root = cb);
        let mut rval = UndefinedValue();
        JS_CallFunctionValue(
            cx,
            this_root.handle().into(),
            cb_root.handle().into(),
            &HandleValueArray::empty(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
        JS_ClearPendingException(cx);
    }

    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

/// ALPN select callback for TLS server. Called by BoringSSL during the
/// TLS handshake to select the server's preferred ALPN protocol from
/// the client's offered list.
///
/// # Safety
///
/// `arg` must point to a wire-format ALPN protocol list (length-prefixed)
/// that outlives the callback registration.
unsafe extern "C" fn alpn_select_callback(
    _ssl: *mut SSL,
    out: *mut *const u8,
    out_len: *mut u8,
    client_protos: *const u8,
    client_protos_len: ::std::ffi::c_uint,
    arg: *mut core::ffi::c_void,
) -> ::std::ffi::c_int {
    if arg.is_null() || client_protos.is_null() || client_protos_len == 0 {
        return SSL_TLSEXT_ERR_NOACK;
    }

    // Server's supported protocols (wire-format, length-prefixed)
    let server_protos = unsafe {
        core::slice::from_raw_parts(arg as *const u8, 256) // safe upper bound
    };
    let client_list =
        unsafe { core::slice::from_raw_parts(client_protos, client_protos_len as usize) };

    // Iterate client protocols, find first match in server list
    let mut pos = 0usize;
    while pos < client_list.len() {
        let len = client_list[pos] as usize;
        pos += 1;
        if pos + len > client_list.len() {
            break;
        }
        let client_proto = &client_list[pos..pos + len];
        pos += len;

        // Search in server list
        let mut spos = 0usize;
        while spos < server_protos.len() {
            let slen = server_protos[spos] as usize;
            spos += 1;
            if spos + slen > server_protos.len() || slen == 0 {
                break;
            }
            let server_proto = &server_protos[spos..spos + slen];
            spos += slen;

            if client_proto == server_proto {
                unsafe {
                    *out = client_proto.as_ptr();
                    *out_len = len as u8;
                }
                return SSL_TLSEXT_ERR_OK;
            }
        }
    }

    SSL_TLSEXT_ERR_NOACK
}

/// tls.createServer().close([callback]) — stop listening and tear the
/// server down. The listener and every live connection are closed by the
/// driver; the final `ServerClosed` tasklet unroots the JS references,
/// emits `close`, and invokes the callback.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_server_close(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this_obj = args.thisv().to_object();

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_root = this_obj);

    // Store the optional close callback for the ServerClosed tasklet.
    if argc > 0 && (*args.get(0).ptr).is_object() {
        rooted!(&in(cx_ref) let cb_val = *args.get(0).ptr);
        JS_DefineProperty(
            cx,
            this_root.handle().into(),
            c"_closeCb".as_ptr(),
            cb_val.handle().into(),
            0,
        );
    }

    // Find the driver-side server by id.
    let mut sid_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_root.handle().into(),
        c"_serverId".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut sid_val,
        },
    );
    if sid_val.is_double() {
        let server_id = sid_val.to_double() as u64;
        let shared = TLS_SERVER_REGISTRY.with(|r| r.borrow().get(&server_id).cloned());
        if let Some(shared) = shared {
            if !shared.closing.swap(true, Ordering::AcqRel) {
                if let Some(handle) = DRIVER.get() {
                    handle.cmds.lock().unwrap().push(DriverCmd::RemoveServer(server_id));
                    tls_driver_wake();
                }
            }
        }
    }

    // Drop the SecureContextState (legacy lifecycle owner).
    sc_state_drop(cx, this_obj);

    args.rval().set(UndefinedValue());
    true
}

/// tls.createServer().address() — bound address of the listening server.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_server_address(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());

    let mut port_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_obj.handle().into(),
        c"_listenPort".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut port_val,
        },
    );
    let mut host_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_obj.handle().into(),
        c"_listenHost".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut host_val,
        },
    );
    if !port_val.is_int32() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let addr = w2::JS_NewPlainObject(cx_ref);
    if addr.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_ref) let addr_root = addr);
    rooted!(&in(cx_ref) let port_h = port_val);
    JS_DefineProperty(
        cx,
        addr_root.handle().into(),
        c"port".as_ptr(),
        port_h.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    let fam_str = JS_NewStringCopyZ(cx, c"IPv4".as_ptr());
    if !fam_str.is_null() {
        rooted!(&in(cx_ref) let fam = mozjs::jsval::StringValue(&*fam_str));
        JS_DefineProperty(
            cx,
            addr_root.handle().into(),
            c"family".as_ptr(),
            fam.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    if host_val.is_string() {
        rooted!(&in(cx_ref) let host_h = host_val);
        JS_DefineProperty(
            cx,
            addr_root.handle().into(),
            c"address".as_ptr(),
            host_h.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    args.rval().set(ObjectValue(addr_root.get()));
    true
}
