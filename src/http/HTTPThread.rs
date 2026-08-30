use core::ffi::c_void;
use core::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use bun_collections::ArrayHashMap;
use bun_core::{self, Output};
use bun_ptr::RefPtr;

use bun_threading::{Mutex, UnboundedQueue};
use bun_uws as uws;

use crate::async_http::{ACTIVE_REQUESTS_COUNT, MAX_SIMULTANEOUS_REQUESTS};
use crate::http_context::ActiveSocketExt;
use crate::proxy_tunnel::ProxyTunnel;
use crate::ssl_config::{self, SSLConfig};
use crate::{AsyncHttp, HTTPContext, HttpClient, InitError, NewHttpContext, h2, h3};

bun_core::declare_scope!(HTTPThread, hidden); // threadlog
bun_core::declare_scope!(HTTPThread_log, visible); // log
// TODO(port): Zig had two `Output.scoped(.HTTPThread, ...)` with different visibilities (.hidden + .visible).
// Rust scope registry keys on name; pick one visibility or split scope names.

/// SSL context cache keyed by interned SSLConfig pointer.
/// Since configs are interned via SSLConfig.GlobalRegistry, pointer equality
/// is sufficient for lookup. Each entry holds a ref on its SSLConfig.
struct SslContextCacheEntry {
    ctx: RefPtr<NewHttpContext<true>>,
    last_used_ns: u64,
    /// Strong ref held by the cache entry (released on eviction).
    _config_ref: ssl_config::SharedPtr,
}

impl SslContextCacheEntry {
    /// Mutable access to the cached `NewHttpContext`. The map and all callers
    /// are HTTP-thread-only, so the returned `&mut` is the sole live borrow.
    #[inline]
    fn ctx_mut<'a>(&self) -> &'a mut NewHttpContext<true> {
        // SAFETY: see above.
        unsafe { &mut *self.ctx.as_ptr() }
    }
}
const SSL_CONTEXT_CACHE_MAX_SIZE: usize = 60;
const SSL_CONTEXT_CACHE_TTL_NS: u64 = 30 * (60 * 1_000_000_000); // 30 * std.time.ns_per_min

// PORTING.md §Global mutable state: only ever accessed from the single HTTP
// client thread after `on_start`. RacyCell — thread affinity is the contract.
static CUSTOM_SSL_CONTEXT_MAP: bun_core::RacyCell<
    Option<ArrayHashMap<*const SSLConfig, SslContextCacheEntry>>,
> = bun_core::RacyCell::new(None);
/// Borrow the (lazily-initialized) SSL-context cache. PORTING.md §Global
/// mutable state: only ever accessed from the single HTTP client thread after
/// `on_start`, so the `&'static mut` is the unique live borrow at every call
/// site (callers must not hold the result across a call that re-enters this
/// accessor; the prior `*mut` API enforced the same per-statement reborrow
/// shape).
fn custom_ssl_context_map() -> &'static mut ArrayHashMap<*const SSLConfig, SslContextCacheEntry> {
    // SAFETY: HTTP-thread-only; initialized on first call. Every call site is
    // a per-statement reborrow (audited in r3), so no two `&mut` overlap.
    unsafe { (*CUSTOM_SSL_CONTEXT_MAP.get()).get_or_insert_with(ArrayHashMap::new) }
}

use bun_event_loop::MiniEventLoop as mini_event_loop;
use bun_event_loop::MiniEventLoop::MiniEventLoop;

pub struct HttpThread {
    /// Per-thread `MiniEventLoop` singleton — published by
    /// `MiniEventLoop::init_global()` in [`on_start`]; outlives the thread
    /// (Zig: `bun.http.http_thread.loop = bun.jsc.MiniEventLoop.initGlobal(null, null)`,
    /// HTTPThread.zig:228+235).
    pub loop_: *const MiniEventLoop<'static>,
    /// The raw uSockets loop inside `loop_.loop` — split out so HTTPContext
    /// can `SocketGroup::init` without naming MiniEventLoop.
    pub uws_loop: *mut uws::Loop,
    pub http_context: NewHttpContext<false>,
    pub https_context: NewHttpContext<true>,
    /// Stashed `InitOpts` for the default HTTPS context. When the user passed
    /// no explicit CA config, `on_start` defers
    /// `https_context.init_with_thread_opts` (which calls
    /// `us_ssl_ctx_from_options` → `us_get_default_ca_store`, ~0.7 ms CPU +
    /// ~400 KB heap to parse the bundled root certs) until the first SSL
    /// connect actually arrives via [`HttpThread::connect`]`::<true>`. A
    /// fully-cached `bun install` never makes one, so the cost is skipped
    /// entirely. If `--cafile` / `--ca` *was* passed, `on_start` still runs
    /// init eagerly so a bad CA file crashes at thread start (the long-standing
    /// test contract) and this stays `None`. HTTP-thread-only after `on_start`;
    /// `Option::take` is the once-guard (no atomics needed — `connect` is never
    /// reentrant).
    lazy_https_init: Option<InitOpts>,

    pub queued_tasks: Queue,
    /// Tasks popped from `queued_tasks` that couldn't start because
    /// `active_requests_count >= max_simultaneous_requests`. Kept in FIFO order
    /// and processed before `queued_tasks` on the next `drainEvents`. Owned by
    /// the HTTP thread; never accessed concurrently.
    pub deferred_tasks: Vec<NonNull<AsyncHttp<'static>>>,
    /// Set by `drainQueuedShutdowns` when a shutdown's `async_http_id` wasn't in
    /// `socket_async_http_abort_tracker` — the request is either not yet started
    /// (still in `queued_tasks`/`deferred_tasks`) or already done. `drainEvents`
    /// uses this to decide whether it must scan the queued/deferred lists for
    /// aborted tasks when `active >= max`; without it the common at-capacity
    /// path stays O(1). Owned by the HTTP thread.
    pub has_pending_queued_abort: bool,

    pub queued_shutdowns: Vec<ShutdownMessage>,
    pub queued_writes: Vec<WriteMessage>,
    pub queued_response_body_drains: Vec<DrainMessage>,
    pub queued_cert_check_resumes: Vec<CertCheckResumeMessage>,
    pub queued_transport_pauses: Vec<TransportPauseMessage>,

    pub queued_shutdowns_lock: Mutex,
    pub queued_writes_lock: Mutex,
    pub queued_response_body_drains_lock: Mutex,
    pub queued_cert_check_resumes_lock: Mutex,
    pub queued_transport_pauses_lock: Mutex,

    /// Refs released on the next loop tick rather than inside the socket
    /// callback that gave them up.
    pub queued_threadlocal_proxy_derefs: Vec<RefPtr<ProxyTunnel>>,

    pub has_awoken: AtomicBool,
    pub timer: Instant,
    pub lazy_libdeflater: Option<Box<LibdeflateState>>,

    /// Every `ThreadlocalAsyncHTTP` box currently in flight on this thread.
    /// Inserted by [`start_queued_task`] right after `heap::release`; removed
    /// by `AsyncHTTP::on_async_http_callback_raw` immediately before its
    /// `std::alloc::dealloc`. HTTP-thread-only. Exists so
    /// [`shutdown_for_exit`] can reclaim each clone-owned box at process exit
    /// — the request socket never reaches a terminal state once the JS thread
    /// stops driving the world, so the box would otherwise strand.
    pub in_flight: Vec<NonNull<crate::ThreadlocalAsyncHttp<'static>>>,
}

impl HttpThread {
    /// Mirror of Zig `initOnce`'s `bun.http.http_thread = .{ ... }` field-init
    /// list (HTTPThread.zig:195-206). `loop_`/`uws_loop` are filled in by
    /// `on_start` on the spawned thread; `timer` is started on the calling
    /// thread per spec.
    fn new() -> Self {
        Self {
            loop_: core::ptr::null(),
            uws_loop: core::ptr::null_mut(),
            http_context: NewHttpContext::<false> {
                ref_count: Cell::new(1),
                pending_sockets: bun_collections::HiveArray::init(),
                group: uws::SocketGroup::default(),
                secure: None,
                active_h2_sessions: Vec::new(),
                pending_h2_connects: Vec::new(),
            },
            https_context: NewHttpContext::<true> {
                ref_count: Cell::new(1),
                pending_sockets: bun_collections::HiveArray::init(),
                group: uws::SocketGroup::default(),
                secure: None,
                active_h2_sessions: Vec::new(),
                pending_h2_connects: Vec::new(),
            },
            lazy_https_init: None,
            queued_tasks: Queue::new(),
            deferred_tasks: Vec::new(),
            has_pending_queued_abort: false,
            queued_shutdowns: Vec::new(),
            queued_writes: Vec::new(),
            queued_response_body_drains: Vec::new(),
            queued_cert_check_resumes: Vec::new(),
            queued_transport_pauses: Vec::new(),
            queued_shutdowns_lock: Mutex::new(),
            queued_writes_lock: Mutex::new(),
            queued_response_body_drains_lock: Mutex::new(),
            queued_cert_check_resumes_lock: Mutex::new(),
            queued_transport_pauses_lock: Mutex::new(),
            queued_threadlocal_proxy_derefs: Vec::new(),
            has_awoken: AtomicBool::new(false),
            timer: Instant::now(),
            lazy_libdeflater: None,
            in_flight: Vec::new(),
        }
    }
}

/// Initial capacity of the `Vec` the request head (plus as much body as fits) is assembled into.
pub fn request_body_send_buffer_capacity(estimated_size: usize) -> usize {
    const SMALL: usize = 32 * 1024;
    const LARGE: usize = 512 * 1024;
    if estimated_size >= SMALL {
        LARGE
    } else {
        SMALL
    }
}

pub struct WriteMessage {
    pub async_http_id: u32,
    pub kind: WriteMessageType,
}

#[repr(u8)] // Zig: enum(u2)
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum WriteMessageType {
    Data = 0,
    End = 1,
}

pub struct DrainMessage {
    pub async_http_id: u32,
}

/// W2 transport backpressure: gate one request's response-body transport.
/// h1 pauses the socket's read side (kernel receive buffer fills → TCP
/// backpressure stalls the server); h2 withholds that stream's
/// WINDOW_UPDATE (connection window and sibling streams keep flowing).
/// Pure hook: nothing enqueues these unless a consumer opts in.
pub struct TransportPauseMessage {
    pub async_http_id: u32,
    pub kind: TransportPauseKind,
}

#[repr(u8)] // mirrors WriteMessageType's shape
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum TransportPauseKind {
    Pause = 0,
    Resume = 1,
}

pub struct ShutdownMessage {
    pub async_http_id: u32,
}

/// The JS thread's `checkServerIdentity` callback approved the peer
/// certificate; un-park the connection so the request is written.
pub struct CertCheckResumeMessage {
    pub async_http_id: u32,
}

pub struct LibdeflateState {
    pub shared_buffer: [u8; 512 * 1024],
}

// SAFETY: `[u8; N]` is valid at the all-zero bit pattern.
unsafe impl bun_core::Zeroable for LibdeflateState {}

impl LibdeflateState {
    // Pure Rust: no C decompressor handle needed.
    // The shared_buffer is used for intermediate decompression output.
}

// TODO(port): UnboundedQueue is intrusive over `AsyncHttp.next`; encode the field offset.
pub(crate) type Queue = UnboundedQueue<AsyncHttp<'static>>;

// Clone: bitwise OK for the `*const c_void` CA-string pointers — they borrow
// caller-owned config (Zig `[]stringZ`), not heap we free. The `Vec` itself
// deep-clones its slot list.
#[derive(Clone)]
pub struct InitOpts {
    // TODO(port): lifetime — Zig `[]stringZ` borrowed from caller config; copied into spawned thread.
    pub ca: Vec<*const c_void>, // *const [*:0]const u8
    pub abs_ca_file_name: &'static [u8],
    pub for_install: bool,

    pub on_init_error: fn(err: InitError, opts: &InitOpts) -> !,
}

// SAFETY: `ca` holds borrowed `[*:0]const u8` C-string pointers from caller
// config (Zig `[]stringZ`). They are copied into the spawned HTTP thread and
// only read there; no shared mutable state crosses the thread boundary.
unsafe impl Send for InitOpts {}

impl Default for InitOpts {
    fn default() -> Self {
        Self {
            ca: Vec::new(),
            abs_ca_file_name: b"",
            for_install: false,
            on_init_error: on_init_error_noop,
        }
    }
}

fn on_init_error_noop(err: InitError, opts: &InitOpts) -> ! {
    match err {
        InitError::LoadCAFile => {
            // SAFETY: `abs_ca_file_name` is Zig `stringZ` (`[:0]const u8`) by
            // contract — already passed as a C string to BoringSSL via
            // `init_with_thread_opts`, so `ptr[len] == 0` holds.
            let path = unsafe {
                bun_core::ZStr::from_raw(
                    opts.abs_ca_file_name.as_ptr(),
                    opts.abs_ca_file_name.len(),
                )
            };
            if !bun_sys::exists_z(path) {
                Output::err(
                    "HTTPThread",
                    "failed to find CA file: '{}'",
                    (bstr::BStr::new(opts.abs_ca_file_name),),
                );
            } else {
                Output::err(
                    "HTTPThread",
                    "failed to load CA file: '{}'",
                    (bstr::BStr::new(opts.abs_ca_file_name),),
                );
            }
        }
        InitError::InvalidCAFile => {
            Output::err(
                "HTTPThread",
                "the CA file is invalid: '{}'",
                (bstr::BStr::new(opts.abs_ca_file_name),),
            );
        }
        InitError::InvalidCA => {
            Output::err("HTTPThread", "the provided CA is invalid", ());
        }
        InitError::FailedToOpenSocket => {
            bun_core::err_generic!("failed to start HTTP client thread");
        }
    }
    bun_core::Global::crash();
}

impl HttpThread {
    /// Raw uSockets loop for `SocketGroup::init`. Split from `loop_` so
    /// HTTPContext doesn't need to name the higher-tier MiniEventLoop type.
    #[inline]
    pub fn uws_loop(&self) -> *mut uws::Loop {
        self.uws_loop
    }

    /// Mutable access to the live uSockets event loop.
    ///
    /// INVARIANT: `uws_loop` is set once in [`on_start`] (published via the
    /// `has_awoken` Release store) and outlives the HTTP thread. The loop is a
    /// separate C heap allocation disjoint from `self`. HTTP-thread-only at
    /// every caller — `wakeup()` is the sole cross-thread entry and uses the
    /// raw FFI call instead. Centralises the raw `&mut *self.uws_loop`
    /// upgrade repeated in `process_events`.
    #[inline]
    fn uws_loop_mut<'a>(&self) -> &'a mut uws::Loop {
        // SAFETY: see INVARIANT above.
        unsafe { &mut *self.uws_loop }
    }

    /// Zig `timer.read()` returns u64 ns directly; Rust `Instant::elapsed().as_nanos()` is u128.
    /// Checked narrow — overflows only after ~584 years of process uptime.
    #[inline]
    fn timer_read(&self) -> u64 {
        u64::try_from(self.timer.elapsed().as_nanos()).expect("int cast")
    }

    pub fn deflater(&mut self) -> &mut LibdeflateState {
        if self.lazy_libdeflater.is_none() {
            let state: Box<LibdeflateState> = bun_core::boxed_zeroed();
            self.lazy_libdeflater = Some(state);
        }

        self.lazy_libdeflater.as_deref_mut().unwrap()
    }

    pub fn context<const IS_SSL: bool>(&mut self) -> &mut NewHttpContext<IS_SSL> {
        // PORT NOTE: const-generic dispatch over two distinct fields — `NewHttpContext<true>`
        // and `NewHttpContext<IS_SSL>` are the same type when IS_SSL, just spelled
        // differently. Route through a raw-pointer `.cast()` (identity).
        if IS_SSL {
            // SAFETY: identical type when IS_SSL == true; pointer is to a live `&mut self` field.
            unsafe { &mut *(&raw mut self.https_context).cast::<NewHttpContext<IS_SSL>>() }
        } else {
            // SAFETY: identical type when IS_SSL == false; pointer is to a live `&mut self` field.
            unsafe { &mut *(&raw mut self.http_context).cast::<NewHttpContext<IS_SSL>>() }
        }
    }

    /// One-shot lazy init of the default HTTPS context. See
    /// [`HttpThread::lazy_https_init`] for rationale. Called on the HTTP
    /// thread from [`HttpThread::connect`]`::<true>` only; the `Option::take`
    /// is the once-guard. On failure, `on_init_error` diverges (matching the
    /// eager-init crash semantics from Zig `onStart`).
    #[inline]
    fn ensure_https_context_init(&mut self) {
        if let Some(opts) = self.lazy_https_init.take() {
            self.init_https_context_cold(&opts);
        }
    }

    #[cold]
    fn init_https_context_cold(&mut self, opts: &InitOpts) {
        if let Err(err) = self.https_context.init_with_thread_opts(opts) {
            (opts.on_init_error)(err, opts);
        }
    }

    pub fn connect<const IS_SSL: bool>(
        &mut self,
        client: &mut HttpClient,
    ) -> Result<Option<crate::HTTPSocket<IS_SSL>>, bun_core::Error>
// TODO(port): narrow error set
    {
        if IS_SSL {
            // First SSL connect: materialize the default HTTPS `SSL_CTX` +
            // socket group now (deferred from `on_start`). Runs once; every
            // SSL request — including unix-socket and proxy paths below —
            // funnels through here before touching `https_context.{group,secure}`.
            self.ensure_https_context_init();
        }
        // PORT NOTE: borrowck — `slice()` borrows `client`; capture into a
        // `bun_ptr::RawSlice` (encapsulated outlives-holder invariant) so the
        // borrow of `client` ends before we hand `&mut client` to
        // `connect_socket`. Backing storage is `client.unix_socket_path`, which
        // `connect_socket` does not touch.
        let unix_path = bun_ptr::RawSlice::new(client.unix_socket_path.slice());
        if !unix_path.is_empty() {
            return self
                .context::<IS_SSL>()
                .connect_socket(client, unix_path.slice());
        }

        if IS_SSL {
            'custom_ctx: {
                let Some(tls) = client.tls_props.clone() else {
                    break 'custom_ctx;
                };
                if !tls.get().requires_custom_request_ctx {
                    break 'custom_ctx;
                }
                let requested_config: *const SSLConfig = tls.get();

                // Evict stale entries from the cache
                self.evict_stale_ssl_contexts();

                // Look up by pointer equality (configs are interned)
                if let Some(entry) = custom_ssl_context_map().get_mut(&requested_config) {
                    // Cache hit - reuse existing SSL context
                    entry.last_used_ns = self.timer_read();
                    client.set_custom_ssl_ctx(entry.ctx.clone());
                    let ctx = entry.ctx_mut();
                    // Keepalive is now supported for custom SSL contexts
                    return if let Some(url) = client.http_proxy.clone() {
                        ctx.connect(client, url.hostname, url.get_port_auto())
                    } else {
                        let (hn, pt) = (client.url.hostname, client.url.get_port_auto());
                        ctx.connect(client, hn, pt)
                    }
                    // PORT NOTE: NewHttpContext<true> == NewHttpContext<IS_SSL> here (IS_SSL branch).
                    .map(|o| o.map(|s| s.cast_ssl::<IS_SSL>()));
                }

                // Cache miss - create new SSL context
                let ctx = RefPtr::new(NewHttpContext::<true> {
                    ref_count: Cell::new(1),
                    pending_sockets: bun_collections::HiveArray::init(),
                    group: uws::SocketGroup::default(),
                    secure: None,
                    active_h2_sessions: Vec::new(),
                    pending_h2_connects: Vec::new(),
                });
                // SAFETY: fresh allocation; HTTP-thread-only.
                let custom_context = unsafe { &mut *ctx.as_ptr() };
                if let Err(err) = custom_context.init_with_client_config(client) {
                    // `init_with_client_config` fails before `group.init()` runs.
                    // `impl Drop for HTTPContext` tolerates an uninitialized
                    // group (skips close_all/destroy when `group.loop_` is
                    // null), so dropping `ctx` here is safe.
                    return Err(match err {
                        InitError::FailedToOpenSocket
                        | InitError::InvalidCA
                        | InitError::InvalidCAFile
                        | InitError::LoadCAFile => bun_core::err!("FailedToOpenSocket"),
                    });
                }

                let now = self.timer_read();
                client.set_custom_ssl_ctx(ctx.clone());
                let _ = custom_ssl_context_map().put(
                    requested_config,
                    SslContextCacheEntry {
                        ctx,
                        last_used_ns: now,
                        // Strong ref for the cache entry; client.tls_props keeps its own.
                        _config_ref: tls,
                    },
                );

                // Enforce max cache size - evict oldest entry
                if custom_ssl_context_map().count() > SSL_CONTEXT_CACHE_MAX_SIZE {
                    evict_oldest_ssl_context();
                }

                // Keepalive is now supported for custom SSL contexts
                let result = if let Some(url) = client.http_proxy.clone() {
                    if url.protocol.is_empty()
                        || url.protocol == b"https"
                        || url.protocol == b"http"
                    {
                        custom_context.connect(client, url.hostname, url.get_port_auto())
                    } else {
                        return Err(bun_core::err!("UnsupportedProxyProtocol"));
                    }
                } else {
                    let (hn, pt) = (client.url.hostname, client.url.get_port_auto());
                    custom_context.connect(client, hn, pt)
                };
                // PORT NOTE: NewHttpContext<true> == NewHttpContext<IS_SSL> here (IS_SSL branch).
                return result.map(|o| o.map(|s| s.cast_ssl::<IS_SSL>()));
            }
        }
        if let Some(url) = client.http_proxy.clone() {
            if !url.href.is_empty() {
                // https://github.com/oven-sh/bun/issues/11343
                if url.protocol.is_empty() || url.protocol == b"https" || url.protocol == b"http" {
                    return self.context::<IS_SSL>().connect(
                        client,
                        url.hostname,
                        url.get_port_auto(),
                    );
                }
                return Err(bun_core::err!("UnsupportedProxyProtocol"));
            }
        }
        let (hn, pt) = (client.url.hostname, client.url.get_port_auto());
        self.context::<IS_SSL>().connect(client, hn, pt)
    }

    /// Evict SSL context cache entries that haven't been used for ssl_context_cache_ttl_ns.
    fn evict_stale_ssl_contexts(&mut self) {
        let now = self.timer_read();
        let map = custom_ssl_context_map();
        let mut i: usize = 0;
        while i < map.count() {
            let entry_last_used = map.values()[i].last_used_ns;
            if now.saturating_sub(entry_last_used) > SSL_CONTEXT_CACHE_TTL_NS {
                map.swap_remove_at(i);
            } else {
                i += 1;
            }
        }
    }

    fn abort_pending_h2_waiter(&mut self, async_http_id: u32) -> bool {
        if self.https_context.abort_pending_h2_waiter(async_http_id) {
            return true;
        }
        for entry in custom_ssl_context_map().values_mut() {
            if entry.ctx_mut().abort_pending_h2_waiter(async_http_id) {
                return true;
            }
        }
        false
    }

    fn drain_queued_shutdowns(&mut self) {
        loop {
            // socket.close() can potentially be slow
            // Let's not block other threads while this runs.
            let queued_shutdowns = {
                let _guard = self.queued_shutdowns_lock.lock_guard();
                core::mem::take(&mut self.queued_shutdowns)
            };

            for http in &queued_shutdowns {
                let tracker = abort_tracker();
                let found_idx = tracker.keys().iter().position(|&k| k == http.async_http_id);
                if let Some(idx) = found_idx {
                    let (_k, socket_ptr) = tracker.swap_remove_at(idx);
                    match socket_ptr {
                        uws::AnySocket::SocketTls(socket) => {
                            let tagged = HTTPContext::<true>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                // If we only call socket.close(), then it won't
                                // call `onClose` if this happens before `onOpen` is
                                // called.
                                client.close_and_abort::<true>(socket);
                                continue;
                            }
                            if let Some(session) = tagged.session() {
                                h2::ClientSession::abort_by_http_id(session, http.async_http_id);
                                continue;
                            }
                            socket.close(uws::CloseKind::Failure);
                        }
                        uws::AnySocket::SocketTcp(socket) => {
                            let tagged = HTTPContext::<false>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                client.close_and_abort::<false>(socket);
                                continue;
                            }
                            if let Some(session) = tagged.session() {
                                h2::ClientSession::abort_by_http_id(session, http.async_http_id);
                                continue;
                            }
                            socket.close(uws::CloseKind::Failure);
                        }
                    }
                } else {
                    // No socket for this id. It may be a request coalesced onto a
                    // leader's in-flight h2 TLS connect (parked in `pc.waiters`
                    // with no abort-tracker entry); scan those first so the abort
                    // doesn't wait for the leader's connect to resolve.
                    if self.abort_pending_h2_waiter(http.async_http_id) {
                        continue;
                    }
                    // Or it's on an HTTP/3 session, which has no TCP socket to
                    // register in the tracker.
                    if h3::ClientContext::abort_by_http_id(http.async_http_id) {
                        continue;
                    }
                    // Otherwise the request either hasn't started yet (still in
                    // `queued_tasks`/`deferred_tasks`) or has already completed.
                    // Flag it so `drainEvents` knows to scan the queue for
                    // aborted-but-unstarted tasks even when `active >= max`
                    // would otherwise short-circuit.
                    self.has_pending_queued_abort = true;
                }
            }
            let len = queued_shutdowns.len();
            drop(queued_shutdowns);
            if len == 0 {
                break;
            }
            bun_core::scoped_log!(HTTPThread, "drained {} queued shutdowns", len);
        }
    }

    fn drain_queued_writes(&mut self) {
        loop {
            let queued_writes = {
                let _guard = self.queued_writes_lock.lock_guard();
                core::mem::take(&mut self.queued_writes)
            };
            for write in &queued_writes {
                let message = write.kind;
                let ended = message == WriteMessageType::End;

                if let Some(socket_ptr) = abort_tracker().get(&write.async_http_id) {
                    match *socket_ptr {
                        uws::AnySocket::SocketTls(socket) => {
                            if socket.is_closed() || socket.is_shutdown() {
                                continue;
                            }
                            let tagged = HTTPContext::<true>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                if let crate::HTTPRequestBody::Stream(stream) =
                                    &mut client.state.original_request_body
                                {
                                    stream.ended = ended;
                                    client.flush_stream::<true>(socket);
                                }
                            }
                            if let Some(session) = tagged.session() {
                                h2::ClientSession::stream_body_by_http_id(
                                    session,
                                    write.async_http_id,
                                    ended,
                                );
                            }
                        }
                        uws::AnySocket::SocketTcp(socket) => {
                            if socket.is_closed() || socket.is_shutdown() {
                                continue;
                            }
                            let tagged = HTTPContext::<false>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                if let crate::HTTPRequestBody::Stream(stream) =
                                    &mut client.state.original_request_body
                                {
                                    stream.ended = ended;
                                    client.flush_stream::<false>(socket);
                                }
                            }
                            if let Some(session) = tagged.session() {
                                h2::ClientSession::stream_body_by_http_id(
                                    session,
                                    write.async_http_id,
                                    ended,
                                );
                            }
                        }
                    }
                } else {
                    h3::ClientContext::stream_body_by_http_id(write.async_http_id, ended);
                }
            }
            let len = queued_writes.len();
            drop(queued_writes);
            if len == 0 {
                break;
            }
            bun_core::scoped_log!(HTTPThread, "drained {} queued writes", len);
        }
    }

    fn drain_queued_cert_check_resumes(&mut self) {
        loop {
            let queued_cert_check_resumes = {
                let _guard = self.queued_cert_check_resumes_lock.lock_guard();
                core::mem::take(&mut self.queued_cert_check_resumes)
            };
            for resume in &queued_cert_check_resumes {
                // Both arms are required: an HTTPS target behind a plaintext
                // proxy parks behind a SocketTcp tracker entry.
                if let Some(socket_ptr) = abort_tracker().get(&resume.async_http_id) {
                    match *socket_ptr {
                        uws::AnySocket::SocketTls(socket) => {
                            if socket.is_closed() || socket.is_shutdown() {
                                continue;
                            }
                            let tagged = HTTPContext::<true>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                // May synchronously reach close_and_fail →
                                // dispatch_result_and_reset; do not touch
                                // `client` after this call.
                                client.resume_after_cert_check::<true>(socket);
                            }
                        }
                        uws::AnySocket::SocketTcp(socket) => {
                            if socket.is_closed() || socket.is_shutdown() {
                                continue;
                            }
                            let tagged = HTTPContext::<false>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                // See Tls arm.
                                client.resume_after_cert_check::<false>(socket);
                            }
                        }
                    }
                }
            }
            let len = queued_cert_check_resumes.len();
            drop(queued_cert_check_resumes);
            if len == 0 {
                break;
            }
            bun_core::scoped_log!(HTTPThread, "drained {} queued cert check resumes", len);
        }
    }

    fn drain_queued_http_response_body_drains(&mut self) {
        loop {
            // socket.close() can potentially be slow
            // Let's not block other threads while this runs.
            let queued_response_body_drains = {
                let _guard = self.queued_response_body_drains_lock.lock_guard();
                core::mem::take(&mut self.queued_response_body_drains)
            };

            for drain in &queued_response_body_drains {
                if let Some(socket_ptr) = abort_tracker().get(&drain.async_http_id) {
                    match *socket_ptr {
                        uws::AnySocket::SocketTls(socket) => {
                            let tagged = HTTPContext::<true>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                client.drain_response_body::<true>(socket);
                            }
                            if let Some(session) = tagged.session() {
                                h2::ClientSession::drain_response_body_by_http_id(
                                    session,
                                    drain.async_http_id,
                                );
                            }
                        }
                        uws::AnySocket::SocketTcp(socket) => {
                            let tagged = HTTPContext::<false>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                client.drain_response_body::<false>(socket);
                            }
                            if let Some(session) = tagged.session() {
                                h2::ClientSession::drain_response_body_by_http_id(
                                    session,
                                    drain.async_http_id,
                                );
                            }
                        }
                    }
                }
            }
            let len = queued_response_body_drains.len();
            drop(queued_response_body_drains);
            if len == 0 {
                break;
            }
            bun_core::scoped_log!(HTTPThread, "drained {} queued drains", len);
        }
    }

    fn drain_queued_transport_pauses(&mut self) {
        loop {
            let queued_transport_pauses = {
                let _guard = self.queued_transport_pauses_lock.lock_guard();
                core::mem::take(&mut self.queued_transport_pauses)
            };
            for msg in &queued_transport_pauses {
                if let Some(socket_ptr) = abort_tracker().get(&msg.async_http_id) {
                    match *socket_ptr {
                        uws::AnySocket::SocketTls(socket) => {
                            let tagged = HTTPContext::<true>::get_tagged_from_socket(socket);
                            // h1: the client owns the socket — gate its read side.
                            if let Some(client) = tagged.client_mut() {
                                match msg.kind {
                                    TransportPauseKind::Pause => {
                                        client.pause_response_transport::<true>(socket)
                                    }
                                    TransportPauseKind::Resume => {
                                        client.resume_response_transport::<true>(socket)
                                    }
                                }
                                continue;
                            }
                            // h2: never pause the session socket — that would
                            // starve every sibling stream. Gate the one
                            // stream's WINDOW_UPDATE instead.
                            if let Some(session) = tagged.session() {
                                match msg.kind {
                                    TransportPauseKind::Pause => h2::ClientSession::pause_stream_by_http_id(
                                        session,
                                        msg.async_http_id,
                                    ),
                                    TransportPauseKind::Resume => h2::ClientSession::resume_stream_by_http_id(
                                        session,
                                        msg.async_http_id,
                                    ),
                                }
                            }
                        }
                        uws::AnySocket::SocketTcp(socket) => {
                            let tagged = HTTPContext::<false>::get_tagged_from_socket(socket);
                            if let Some(client) = tagged.client_mut() {
                                match msg.kind {
                                    TransportPauseKind::Pause => {
                                        client.pause_response_transport::<false>(socket)
                                    }
                                    TransportPauseKind::Resume => {
                                        client.resume_response_transport::<false>(socket)
                                    }
                                }
                                continue;
                            }
                            if let Some(session) = tagged.session() {
                                match msg.kind {
                                    TransportPauseKind::Pause => h2::ClientSession::pause_stream_by_http_id(
                                        session,
                                        msg.async_http_id,
                                    ),
                                    TransportPauseKind::Resume => h2::ClientSession::resume_stream_by_http_id(
                                        session,
                                        msg.async_http_id,
                                    ),
                                }
                            }
                        }
                    }
                } else {
                    // No tracker entry: h3/QUIC requests never enter the abort
                    // tracker (no us_socket to track — the h3 session registry
                    // is their index), so route them there. Non-h3 ids (request
                    // not yet connected or already terminal) miss the h3
                    // registry too and no-op — same contract as the drain queue.
                    match msg.kind {
                        TransportPauseKind::Pause => {
                            h3::ClientContext::pause_stream_by_http_id(msg.async_http_id)
                        }
                        TransportPauseKind::Resume => {
                            h3::ClientContext::resume_stream_by_http_id(msg.async_http_id)
                        }
                    }
                }
            }
            let len = queued_transport_pauses.len();
            drop(queued_transport_pauses);
            if len == 0 {
                break;
            }
            bun_core::scoped_log!(HTTPThread, "drained {} transport pauses", len);
        }
    }

    pub fn drain_events(&mut self) {
        // Process any pending writes **before** aborting.
        self.drain_queued_http_response_body_drains();
        // After staged bytes are drained forward: park the transport of any
        // consumer that asked, and lift parks whose consumer came back — both
        // land before the next tick, so a pause stops reads at the next
        // epoll wait and a resume re-arms them for it.
        self.drain_queued_transport_pauses();
        self.drain_queued_writes();
        self.drain_queued_shutdowns();
        // After shutdowns: an abort or cert-rejection scheduled in the same JS
        // turn removes the abort-tracker entry first, so the resume becomes a
        // no-op and the request is never transmitted after a same-tick abort.
        self.drain_queued_cert_check_resumes();
        h3::PendingConnect::drain_resolved();

        self.queued_threadlocal_proxy_derefs.clear();

        let mut count: usize = 0;
        let mut active = ACTIVE_REQUESTS_COUNT.load(Ordering::Relaxed);
        let max = MAX_SIMULTANEOUS_REQUESTS.load(Ordering::Relaxed);

        // Fast path: at capacity and no queued/deferred task could possibly be
        // aborted. A queued task can only become aborted via `scheduleShutdown`,
        // which we just drained — `drainQueuedShutdowns` sets
        // `has_pending_queued_abort` for any id it couldn't find in the socket
        // tracker. If that's clear, there's nothing to fail-fast and nothing can
        // start, so don't walk the lists.
        if active >= max && !self.has_pending_queued_abort {
            return;
        }

        // Deferred tasks are ones we previously popped from the MPSC queue but
        // couldn't start because we were at max. They stay in FIFO order ahead of
        // anything still in `queued_tasks`.
        //
        // Already-aborted tasks are started regardless of `max`: `start_()` will
        // observe the `aborted` signal and fail immediately with
        // `error.AbortedBeforeConnecting`, and `onAsyncHTTPCallback` decrements
        // `active_requests_count` in the same turn — so they never hold a slot.
        // Without this, an aborted fetch that was queued behind `max` would sit
        // there until some unrelated request completed; if every active request
        // is itself hung, the aborted one never settles and its promise hangs
        // forever even though the user called `controller.abort()`.
        //
        // `startQueuedTask` can re-enter `onAsyncHTTPCallback` synchronously (for
        // aborted tasks, or when connect() fails immediately), which reads both
        // `active_requests_count` and `deferred_tasks.items.len` to decide whether
        // to wake the loop. To keep those reads accurate we swap the deferred list
        // out before iterating so the field reflects only tasks still waiting, and
        // reload `active` from the atomic after every start rather than tracking
        // it locally.
        self.has_pending_queued_abort = false;
        {
            let pending = core::mem::take(&mut self.deferred_tasks);
            for http in pending {
                // AsyncHttp is heap-owned by the caller and alive until its
                // completion callback; while parked in `deferred_tasks` no other
                // borrow exists, so a transient `ParentRef` shared deref is sound.
                let aborted = bun_ptr::ParentRef::from(http)
                    .client
                    .signals
                    .get(crate::signals::Field::Aborted);
                if aborted || active < max {
                    start_queued_task(http.as_ptr(), &mut self.in_flight);
                    if cfg!(debug_assertions) {
                        count += 1;
                    }
                    active = ACTIVE_REQUESTS_COUNT.load(Ordering::Relaxed);
                } else {
                    self.deferred_tasks.push(http);
                }
            }
        }

        loop {
            let Some(http) = NonNull::new(self.queued_tasks.pop()) else {
                break;
            };
            // AsyncHttp is heap-owned by the caller and alive until its
            // completion callback; the MPSC pop hands sole access to this
            // thread, so a transient `ParentRef` shared deref is sound.
            let aborted = bun_ptr::ParentRef::from(http)
                .client
                .signals
                .get(crate::signals::Field::Aborted);
            if !aborted && active >= max {
                // Can't start this one yet. Defer it (preserves FIFO relative to
                // later pops) and keep draining — there may be aborted tasks
                // behind it that we can fail-fast right now.
                self.deferred_tasks.push(http);
                continue;
            }
            start_queued_task(http.as_ptr(), &mut self.in_flight);
            if cfg!(debug_assertions) {
                count += 1;
            }
            active = ACTIVE_REQUESTS_COUNT.load(Ordering::Relaxed);
        }

        if cfg!(debug_assertions) && count > 0 {
            bun_core::scoped_log!(HTTPThread_log, "Processed {} tasks\n", count);
        }
    }

    pub fn schedule_response_body_drain(&mut self, async_http_id: u32) {
        {
            let _guard = self.queued_response_body_drains_lock.lock_guard();
            self.queued_response_body_drains
                .push(DrainMessage { async_http_id });
        }
        self.wakeup();
    }

    /// Cross-thread response-body drain round-trip for one request: the
    /// consumer-side half of the streaming park/unpark contract (a consumer
    /// that withheld the drain while parked re-arms it on unpark, alongside
    /// [`Self::schedule_transport_pause_from_any_thread`] with `Resume`).
    /// Mirrors that entry point exactly: push under the drain-queue lock
    /// (shared with the HTTP-thread drain pass) + wakeup. A message for an
    /// id with no registered socket (h3, not yet connected, already
    /// terminal) is dropped by the tick — the same contract as every other
    /// cross-thread schedule.
    pub fn schedule_response_body_drain_from_any_thread(async_http_id: u32) {
        if async_http_id == 0 {
            return;
        }
        assert!(
            crate::HTTP_THREAD_INIT.load(Ordering::Acquire),
            "HTTPThread::schedule_response_body_drain_from_any_thread() called before HTTPThread::init()"
        );
        // SAFETY: `HTTP_THREAD_INIT == true` ⇒ `HTTP_THREAD` is fully
        // written. Raw field access (not `ParentRef`): the push is guarded
        // by `queued_response_body_drains_lock`, which serializes access
        // with the HTTP-thread drain pass — no aliasing is observable.
        let this: *mut Self = unsafe {
            (*crate::HTTP_THREAD.get_unchecked()).as_mut_ptr()
        };
        unsafe {
            {
                let _guard = (*this).queued_response_body_drains_lock.lock_guard();
                (*this).queued_response_body_drains.push(DrainMessage { async_http_id });
            }
        }
        // Same has_awoken handshake as `schedule()`.
        // SAFETY: `this` points at the initialized HttpThread; both fields
        // below are designed for cross-thread shared access.
        let has_awoken = unsafe { &*::std::ptr::addr_of!((*this).has_awoken) };
        if !has_awoken.load(Ordering::Acquire) {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(100);
            while !has_awoken.load(Ordering::Acquire) {
                if std::time::Instant::now() > deadline {
                    panic!(
                        "HTTPThread::schedule_response_body_drain_from_any_thread() timed out waiting for HTTPThread::init()"
                    );
                }
                ::std::hint::spin_loop();
            }
        }
        // SAFETY: wakeup only touches atomics + the uws loop pointer, both
        // valid after on_start() (guaranteed by the has_awoken check above).
        unsafe { (&*this).wakeup() };
    }

    /// HTTP-thread-local enqueue of a transport pause/resume for one request
    /// (e.g. from a delivery callback running on this thread). Cross-thread
    /// callers (the JS-thread consumer) use
    /// [`Self::schedule_transport_pause_from_any_thread`].
    pub fn schedule_transport_pause(&mut self, async_http_id: u32, kind: TransportPauseKind) {
        {
            let _guard = self.queued_transport_pauses_lock.lock_guard();
            self.queued_transport_pauses.push(TransportPauseMessage {
                async_http_id,
                kind,
            });
        }
        self.wakeup();
    }

    pub fn schedule_shutdown(&mut self, http: &AsyncHttp) {
        bun_core::scoped_log!(HTTPThread, "scheduleShutdown {}", http.async_http_id);
        {
            let _guard = self.queued_shutdowns_lock.lock_guard();
            self.queued_shutdowns.push(ShutdownMessage {
                async_http_id: http.async_http_id,
            });
        }
        self.wakeup();
    }

    pub fn schedule_cert_check_resume(&mut self, http: &AsyncHttp) {
        bun_core::scoped_log!(HTTPThread, "scheduleCertCheckResume {}", http.async_http_id);
        {
            let _guard = self.queued_cert_check_resumes_lock.lock_guard();
            self.queued_cert_check_resumes.push(CertCheckResumeMessage {
                async_http_id: http.async_http_id,
            });
        }
        self.wakeup();
    }

    pub fn schedule_request_write(&mut self, http: &AsyncHttp, kind: WriteMessageType) {
        {
            let _guard = self.queued_writes_lock.lock_guard();
            self.queued_writes.push(WriteMessage {
                async_http_id: http.async_http_id,
                kind,
            });
        }
        self.wakeup();
    }

    pub fn schedule_proxy_deref(&mut self, proxy: RefPtr<ProxyTunnel>) {
        // this is always called on the http thread,
        self.queued_threadlocal_proxy_derefs.push(proxy);
        self.wakeup();
    }

    /// Called from [`crate::shutdown_for_exit`] on the HTTP thread once
    /// `SHUTDOWN_REQUESTED` is observed. Reclaims every clone-owned
    /// `ThreadlocalAsyncHTTP` box by mirroring the teardown
    /// `on_async_http_callback_raw` performs (drop the clone-only fields,
    /// then raw-dealloc the storage — NOT `Box::drop`, since the remaining
    /// fields are bitwise-shared with the JS-thread original). The full
    /// result callback is not invoked (the JS thread is parked in
    /// `global_exit()` waiting on us and will not process the completion);
    /// only `release_at_shutdown` runs so the owner can drop the ref it
    /// took for the in-flight callback — without it the `ctx` ⇄
    /// `Box<AsyncHTTP>` cycle is unreachable from any root and LSan reports
    /// the whole chain as indirect leaks.
    fn dealloc_in_flight_for_exit(&mut self) {
        bun_core::scoped_log!(
            HTTPThread,
            "dealloc_in_flight_for_exit: in_flight={} deferred={}",
            self.in_flight.len(),
            self.deferred_tasks.len()
        );
        for nn in core::mem::take(&mut self.in_flight) {
            // SAFETY: every entry is the `heap::release` allocation pushed by
            // `start_queued_task`; HTTP-thread-only and removed at the
            // callback dealloc site, so each is still live and uniquely
            // accessed here. The connecting socket's ext may still alias
            // `client`, but the loop below never ticks again (we park
            // forever after this returns).
            unsafe {
                // Snapshot before tearing down `client` — `release_at_shutdown`
                // must run after the clone-only fields drop (it may park `ctx`
                // for JS-thread `deinit`, which frees the original
                // `Box<AsyncHTTP>` whose bitwise-shared fields those clone
                // fields alias) but before the raw `dealloc` (so `ctx` is
                // observed once per in-flight entry).
                let release = (*nn.as_ptr()).async_http.result_callback;
                let client = &mut (*nn.as_ptr()).async_http.client;
                drop(core::mem::take(&mut client.redirect));
                drop(core::mem::take(&mut client.prev_redirect));
                client.close_proxy_tunnel(false);
                drop(core::mem::take(&mut client.custom_ssl_ctx));
                drop(core::mem::take(&mut client.state));
                if let Some(f) = release.release_at_shutdown {
                    f(release.ctx);
                }
                std::alloc::dealloc(
                    nn.as_ptr().cast::<u8>(),
                    std::alloc::Layout::new::<crate::ThreadlocalAsyncHttp<'static>>(),
                );
            }
        }
    }

    pub fn wakeup(&self) {
        // Acquire (not Relaxed): pairs with the Release store in `on_start`
        // so the read of `self.uws_loop` (a non-atomic field set there)
        // observes the published value. This is the canonical "Relaxed gives
        // no happens-before for the init it guards" case.
        if self.has_awoken.load(Ordering::Acquire) {
            // SAFETY: uws_loop is the live HTTP-thread loop set in on_start.
            // Call the raw extern (not `Loop::wakeup(&mut self)`) — this runs
            // cross-thread while the HTTP thread owns the loop, so forming
            // `&mut Loop` here would alias.
            unsafe { uws::us_wakeup_loop(self.uws_loop) };
        }
    }

    /// Enqueue a batch of `AsyncHttp` tasks for the HTTP thread. Safe to
    /// call from any thread: only touches the lock-free `queued_tasks` MPSC
    /// queue and `wakeup()` (atomic load + raw FFI call). This is the
    /// **only** cross-thread entry point — every other `HttpThread` method
    /// is HTTP-thread-only via [`http_thread()`](crate::http_thread).
    pub fn schedule(batch: bun_threading::thread_pool::Batch) {
        if batch.len == 0 {
            return;
        }
        // Release-mode guard: `HttpThread` has niche-bearing fields, so
        // dereffing `as_mut_ptr()` below on an uninitialized static is UB.
        // The "every caller goes through `init`" invariant was unenforced
        // (e.g. `async_http::preconnect` did not), so check it here. The
        // `Acquire` load pairs with `init_once`'s `Release` store to publish
        // the `HTTP_THREAD.write(..)` to this thread.
        assert!(
            crate::HTTP_THREAD_INIT.load(Ordering::Acquire),
            "HTTPThread::schedule() called before HTTPThread::init()"
        );
        // SAFETY: `HTTP_THREAD_INIT == true` (checked above) ⇒ `HTTP_THREAD`
        // is fully written. `get_unchecked` (no owner assert) so the
        // `ThreadCell` debug-owner check is skipped on this cross-thread
        // caller. Wrap the result in a `ParentRef` (process-lifetime backref)
        // so the `&self`-only calls below — `queued_tasks.push` (lock-free
        // MPSC) and `wakeup` (atomics + raw uws ptr) — go through the safe
        // `Deref` impl instead of open-coded `(*this_p)` raw derefs. Only a
        // shared `&HttpThread` is ever materialised; the HTTP thread itself
        // never holds a long-lived `&mut HttpThread` across the points these
        // touch (both fields are designed for cross-thread shared access).
        let this = unsafe {
            bun_ptr::ParentRef::<Self>::from_raw((*crate::HTTP_THREAD.get_unchecked()).as_mut_ptr())
        };
        {
            let mut batch_ = batch;
            while let Some(task) = batch_.pop() {
                // SAFETY: task points to AsyncHttp.task; recover parent via field offset.
                let http: *mut AsyncHttp =
                    unsafe { bun_core::from_field_ptr!(AsyncHttp, task, task.as_ptr()) };
                // SAFETY: `http` recovered from a live batch node (non-null); valid until popped.
                let http = unsafe { core::ptr::NonNull::new_unchecked(http) };
                this.queued_tasks.push(http);
            }
        }
        // BUG-ENG-369 fix: wait for the HTTP thread to finish `on_start()` before
        // calling `wakeup()`. `on_start()` sets `has_awoken = true` after
        // configuring `uws_loop`; if we wakeup before that, `us_wakeup_loop`
        // either dereferences a null loop pointer or misses the event entirely.
        //
        // Primary guarantee: `init()` blocks via Condvar until `on_start()`
        // completes, so `has_awoken == true` is already expected here. The
        // spin-wait is a defensive guard against any future regression where
        // `schedule()` is called before `init()` finishes (e.g. if the Condvar
        // wait is removed or a new call path bypasses `init()`).
        if !this.has_awoken.load(Ordering::Acquire) {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(100);
            while !this.has_awoken.load(Ordering::Acquire) {
                if std::time::Instant::now() > deadline {
                    panic!(
                        "HTTPThread::schedule() timed out waiting for HTTPThread::init()"
                    );
                }
                std::hint::spin_loop();
            }
        }
        this.wakeup();
    }

    /// Cross-thread abort entry: enqueue a shutdown for the request owning
    /// `async_http_id`. Port of Zig `HTTPThread.scheduleShutdown` called from
    /// a non-HTTP thread on AbortSignal; the HTTP-thread tick
    /// (`drain_queued_shutdowns`) looks the id up in the abort tracker and
    /// force-closes the socket via `close_and_abort` (`err!(Aborted)`).
    ///
    /// Safe to call from any thread, mirroring [`Self::schedule`]: the push
    /// is guarded by `queued_shutdowns_lock` (shared with
    /// `drain_queued_shutdowns` on the HTTP thread) and `wakeup()` is
    /// atomics + raw FFI.
    pub fn schedule_shutdown_from_any_thread(async_http_id: u32) {
        if async_http_id == 0 {
            return;
        }
        // Release guard + assertion: same pairing with `init_once` as
        // `schedule()` above.
        assert!(
            crate::HTTP_THREAD_INIT.load(Ordering::Acquire),
            "HTTPThread::schedule_shutdown_from_any_thread() called before HTTPThread::init()"
        );
        // SAFETY: `HTTP_THREAD_INIT == true` ⇒ `HTTP_THREAD` is fully
        // written. Raw field access (not `ParentRef`): `queued_shutdowns` is
        // a plain `Vec` needing `&mut` for `push`, but every read/write —
        // here and in `drain_queued_shutdowns` — happens under
        // `queued_shutdowns_lock`, so the lock serializes all access and no
        // aliasing is observable.
        let this: *mut Self = unsafe {
            (*crate::HTTP_THREAD.get_unchecked()).as_mut_ptr()
        };
        unsafe {
            let shutdowns = ::std::ptr::addr_of_mut!((*this).queued_shutdowns);
            let lock = ::std::ptr::addr_of!((*this).queued_shutdowns_lock);
            {
                let _guard = (*lock).lock_guard();
                (*shutdowns).push(ShutdownMessage { async_http_id });
            }
        }
        // Same has_awoken handshake as `schedule()`.
        // SAFETY: `this` points at the initialized HttpThread; both fields
        // below are designed for cross-thread shared access.
        let has_awoken = unsafe { &*::std::ptr::addr_of!((*this).has_awoken) };
        if !has_awoken.load(Ordering::Acquire) {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(100);
            while !has_awoken.load(Ordering::Acquire) {
                if std::time::Instant::now() > deadline {
                    panic!(
                        "HTTPThread::schedule_shutdown_from_any_thread() timed out waiting for HTTPThread::init()"
                    );
                }
                ::std::hint::spin_loop();
            }
        }
        // SAFETY: wakeup only touches atomics + the uws loop pointer, both
        // valid after on_start() (guaranteed by the has_awoken check above).
        unsafe { (&*this).wakeup() };
    }

    /// Cross-thread request-body write entry: announce that the producer of a
    /// streaming request body wrote bytes into the request's
    /// `ThreadSafeStreamBuffer` (`Data`), or finished it (`End`). Port of Zig
    /// `HTTPThread.scheduleRequestWrite` called from a non-HTTP thread
    /// (FetchTasklet.writeRequestData / writeEndRequest); the HTTP-thread tick
    /// (`drain_queued_writes`) looks the id up in the abort tracker, marks the
    /// stream `ended` for `End`, and flushes the buffered bytes to the socket
    /// (h1) or the h2 session (`ClientSession::stream_body_by_http_id`).
    ///
    /// Safe to call from any thread, mirroring
    /// [`Self::schedule_shutdown_from_any_thread`]: the push is guarded by
    /// `queued_writes_lock` (shared with `drain_queued_writes` on the HTTP
    /// thread) and `wakeup()` is atomics + raw FFI. A write for an id with no
    /// registered socket (request not yet connected, or already terminal) is
    /// dropped by the tick — the bytes stay in the buffer until the socket
    /// exists, or are simply never sent for a finished request.
    pub fn schedule_request_write_from_any_thread(async_http_id: u32, kind: WriteMessageType) {
        if async_http_id == 0 {
            return;
        }
        // Release guard + assertion: same pairing with `init_once` as
        // `schedule()` above.
        assert!(
            crate::HTTP_THREAD_INIT.load(Ordering::Acquire),
            "HTTPThread::schedule_request_write_from_any_thread() called before HTTPThread::init()"
        );
        // SAFETY: `HTTP_THREAD_INIT == true` ⇒ `HTTP_THREAD` is fully
        // written. Raw field access (not `ParentRef`): `queued_writes` is a
        // plain `Vec` needing `&mut` for `push`, but every read/write — here
        // and in `drain_queued_writes` — happens under `queued_writes_lock`,
        // so the lock serializes all access and no aliasing is observable.
        let this: *mut Self = unsafe {
            (*crate::HTTP_THREAD.get_unchecked()).as_mut_ptr()
        };
        unsafe {
            let writes = ::std::ptr::addr_of_mut!((*this).queued_writes);
            let lock = ::std::ptr::addr_of!((*this).queued_writes_lock);
            {
                let _guard = (*lock).lock_guard();
                (*writes).push(WriteMessage {
                    async_http_id,
                    kind,
                });
            }
        }
        // Same has_awoken handshake as `schedule()`.
        // SAFETY: `this` points at the initialized HttpThread; both fields
        // below are designed for cross-thread shared access.
        let has_awoken = unsafe { &*::std::ptr::addr_of!((*this).has_awoken) };
        if !has_awoken.load(Ordering::Acquire) {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(100);
            while !has_awoken.load(Ordering::Acquire) {
                if std::time::Instant::now() > deadline {
                    panic!(
                        "HTTPThread::schedule_request_write_from_any_thread() timed out waiting for HTTPThread::init()"
                    );
                }
                ::std::hint::spin_loop();
            }
        }
        // SAFETY: wakeup only touches atomics + the uws loop pointer, both
        // valid after on_start() (guaranteed by the has_awoken check above).
        unsafe { (&*this).wakeup() };
    }

    /// Cross-thread transport-backpressure entry: gate (`Pause`) or release
    /// (`Resume`) the response-body transport of the request owning
    /// `async_http_id` — the JS-thread consumer's hook (fetch streaming
    /// staging). The HTTP-thread tick (`drain_queued_transport_pauses`)
    /// resolves the id to its socket: h1 pauses the socket read side
    /// (TCP backpressure to the server), h2 withholds that stream's
    /// WINDOW_UPDATE (sibling streams unaffected).
    ///
    /// Safe to call from any thread, mirroring
    /// [`Self::schedule_request_write_from_any_thread`]: the push is guarded
    /// by `queued_transport_pauses_lock` (shared with the HTTP-thread drain)
    /// and `wakeup()` is atomics + raw FFI. A message for an id with no
    /// registered socket (h3, not yet connected, already terminal) is
    /// dropped by the tick — the request keeps today's unthrottled delivery.
    pub fn schedule_transport_pause_from_any_thread(async_http_id: u32, kind: TransportPauseKind) {
        if async_http_id == 0 {
            return;
        }
        // Release guard + assertion: same pairing with `init_once` as
        // `schedule()` above.
        assert!(
            crate::HTTP_THREAD_INIT.load(Ordering::Acquire),
            "HTTPThread::schedule_transport_pause_from_any_thread() called before HTTPThread::init()"
        );
        // SAFETY: `HTTP_THREAD_INIT == true` ⇒ `HTTP_THREAD` is fully
        // written. Raw field access (not `ParentRef`): `queued_transport_pauses`
        // is a plain `Vec` needing `&mut` for `push`, but every read/write —
        // here and in `drain_queued_transport_pauses` — happens under
        // `queued_transport_pauses_lock`, so the lock serializes all access
        // and no aliasing is observable.
        let this: *mut Self = unsafe {
            (*crate::HTTP_THREAD.get_unchecked()).as_mut_ptr()
        };
        unsafe {
            let pauses = ::std::ptr::addr_of_mut!((*this).queued_transport_pauses);
            let lock = ::std::ptr::addr_of!((*this).queued_transport_pauses_lock);
            {
                let _guard = (*lock).lock_guard();
                (*pauses).push(TransportPauseMessage {
                    async_http_id,
                    kind,
                });
            }
        }
        // Same has_awoken handshake as `schedule()`.
        // SAFETY: `this` points at the initialized HttpThread; both fields
        // below are designed for cross-thread shared access.
        let has_awoken = unsafe { &*::std::ptr::addr_of!((*this).has_awoken) };
        if !has_awoken.load(Ordering::Acquire) {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(100);
            while !has_awoken.load(Ordering::Acquire) {
                if std::time::Instant::now() > deadline {
                    panic!(
                        "HTTPThread::schedule_transport_pause_from_any_thread() timed out waiting for HTTPThread::init()"
                    );
                }
                ::std::hint::spin_loop();
            }
        }
        // SAFETY: wakeup only touches atomics + the uws loop pointer, both
        // valid after on_start() (guaranteed by the has_awoken check above).
        unsafe { (&*this).wakeup() };
    }
}

/// Evict the least-recently-used SSL context cache entry.
fn evict_oldest_ssl_context() {
    let map = custom_ssl_context_map();
    if map.count() == 0 {
        return;
    }
    let mut oldest_idx: usize = 0;
    let mut oldest_time: u64 = u64::MAX;
    for (i, entry) in map.values().iter().enumerate() {
        if entry.last_used_ns < oldest_time {
            oldest_time = entry.last_used_ns;
            oldest_idx = i;
        }
    }
    map.swap_remove_at(oldest_idx);
}

fn start_queued_task(
    http: *mut AsyncHttp,
    in_flight: &mut Vec<NonNull<crate::ThreadlocalAsyncHttp<'static>>>,
) {
    // SAFETY: http points to a live AsyncHttp queued by the caller thread.
    let cloned = crate::ThreadlocalAsyncHttp::new(unsafe { core::ptr::read(http) });
    // PORT NOTE: Zig used struct copy `http.*`; AsyncHttp is byte-copied here
    // since the original stays valid (real owner is `http`, copy is the
    // HTTP-thread working set).
    let cloned = bun_core::heap::release(cloned);
    in_flight.push(NonNull::from(&*cloned).cast::<crate::ThreadlocalAsyncHttp<'static>>());
    cloned.async_http.real = NonNull::new(http);
    // Clear stale queue pointers - the clone inherited http.next and http.task.node.next
    // which may point to other AsyncHTTP structs that could be freed before the callback
    // copies data back to the original. If not cleared, retrying a failed request would
    // re-queue with stale pointers causing use-after-free.
    cloned.async_http.next.clear();
    cloned.async_http.task.node.next = core::ptr::null_mut();
    cloned.async_http.on_start();
}

/// Borrow the HTTP-thread abort tracker. PORTING.md §Global mutable state:
/// HTTP-thread-only, per-statement reborrow.
#[inline]
fn abort_tracker() -> &'static mut ArrayHashMap<u32, uws::AnySocket> {
    crate::abort_tracker()
}

use core::cell::Cell;

// ═══════════════════════════════════════════════════════════════════════════
// init / on_start / process_events — depends on `bun_event_loop::MiniEventLoop`
// (higher-tier) for `loop_.loop_.{tick,inc,dec,num_polls}`. The wakeup path
// above uses the raw `*mut uws::Loop` directly so the rest of the thread
// machinery compiles; the actual event-loop drive stays gated until the tier
// boundary is resolved.
// TODO(port): MiniEventLoop is in bun_event_loop (not in bun_http deps).
// ═══════════════════════════════════════════════════════════════════════════

mod _event_loop_draft {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Condvar, Mutex, Once, OnceLock, PoisonError};

    static INIT_ONCE: Once = Once::new();

    /// Result of the one-shot `init_once` run. `call_once` blocks late
    /// callers until the first finishes, so every `init()` caller can read
    /// the same outcome here.
    static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

    /// Startup-handshake state shared between `init_once()` (caller thread)
    /// and `on_start()` (HTTP thread). One-way `Pending → Ready | Failed`;
    /// the first terminal write wins (a panic *after* `on_start` signaled
    /// ready — inside `process_events` — must not downgrade `Ready`).
    enum ThreadReady {
        Pending,
        Ready,
        /// The HTTP thread panicked before signaling readiness; carries the
        /// panic message so `init_once` can fail fast instead of parking on
        /// the condvar forever.
        Failed(String),
    }

    type ReadyPair = (Mutex<ThreadReady>, Condvar);

    /// Condvar pair shared between `init_once()` (caller thread) and
    /// `on_start()` (HTTP thread). The caller blocks until the HTTP thread
    /// publishes a terminal state — this eliminates the has_awoken race
    /// entirely, and the `Failed` arm turns a startup panic into a fast
    /// `Err` instead of a permanent block.
    static THREAD_READY: OnceLock<Arc<ReadyPair>> = OnceLock::new();

    fn thread_ready_pair() -> Arc<ReadyPair> {
        THREAD_READY
            .get_or_init(|| Arc::new((Mutex::new(ThreadReady::Pending), Condvar::new())))
            .clone()
    }

    #[cfg(test)]
    fn fresh_ready_pair() -> Arc<ReadyPair> {
        Arc::new((Mutex::new(ThreadReady::Pending), Condvar::new()))
    }

    /// Poison-tolerant lock: this module exists to survive the HTTP thread
    /// panicking, so a poisoned mutex must not turn the handshake itself
    /// into a second panic.
    fn lock_ready(pair: &ReadyPair) -> std::sync::MutexGuard<'_, ThreadReady> {
        pair.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `on_start`'s success path: publish `Ready` and wake `init_once`.
    fn signal_ready(ready: &ReadyPair) {
        let mut guard = lock_ready(ready);
        *guard = ThreadReady::Ready;
        ready.1.notify_all();
    }

    /// `init_once`'s side of the handshake: park until the HTTP thread
    /// publishes a terminal state. `Ok(())` on `Ready`; `Err(panic message)`
    /// if the thread died before signaling — the caller must fail loudly
    /// instead of letting every subsequent fetch hang on a dead thread.
    fn wait_for_ready(ready: &ReadyPair) -> Result<(), String> {
        let mut guard = lock_ready(ready);
        loop {
            match &*guard {
                ThreadReady::Ready => return Ok(()),
                ThreadReady::Failed(msg) => return Err(msg.clone()),
                ThreadReady::Pending => {
                    guard = ready.1.wait(guard).unwrap_or_else(PoisonError::into_inner)
                }
            }
        }
    }

    fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
        if let Some(msg) = payload.downcast_ref::<&str>() {
            (*msg).to_string()
        } else if let Some(msg) = payload.downcast_ref::<String>() {
            msg.clone()
        } else {
            "non-string panic payload".to_string()
        }
    }

    /// Panic guard for the HTTP-thread startup body. `on_start` signals
    /// `Ready` itself on success; if it panics first (Output bring-up, loop
    /// init, CA load — or any unknown future path), publish `Failed` with
    /// the panic message so `init_once` returns `Err` instead of blocking on
    /// the condvar forever (historically: an embedding that never
    /// initialized bun's Output subsystem panicked before the signal and
    /// every scheduled fetch hung — the U2 scenario_1 "pipeline not ready"
    /// skip). The default panic hook still prints the panic + location to
    /// stderr; this guard only adds the failure channel to the handshake.
    fn run_startup_guarded(ready: Arc<ReadyPair>, body: impl FnOnce()) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(body)) {
            let msg = panic_payload_message(payload);
            let mut guard = lock_ready(&ready);
            // First terminal state wins: `on_start` signals ready *before*
            // entering `process_events`, so a `Pending` state here means the
            // panic struck during startup — the case `init_once` is parked
            // on. A `Ready` state means the loop body died later; leave it
            // (shutdown goes through `has_awoken`, not this pair).
            if matches!(*guard, ThreadReady::Pending) {
                *guard = ThreadReady::Failed(msg);
                ready.1.notify_all();
            }
        }
    }

    // PORT NOTE: Zig `std.Thread.spawn` + `.detach()` allocates nothing on the
    // heap. Rust's `Builder::spawn` allocates an `Arc<thread::Inner>` (48 B)
    // shared between the `JoinHandle` and the new thread's TLS `current()`.
    // Dropping the handle leaves the only strong ref inside the spawned
    // thread's TLS, which LSAN does not scan as a root — so when the main
    // thread reaches `Global::exit` *before* the HTTP thread has installed
    // that TLS slot, LSAN reports the Arc as a direct leak and (with CI's
    // `abort_on_error=1`) the process SIGABRTs (exit 134). Park the handle in
    // a process-lifetime static so the Arc is always reachable from a global
    // root, matching the Zig semantics of "detach" without the false positive.
    static HTTP_THREAD_HANDLE: std::sync::OnceLock<std::thread::JoinHandle<()>> =
        std::sync::OnceLock::new();

    pub(super) fn init(opts: &InitOpts) {
        INIT_ONCE.call_once(|| {
            let _ = INIT_RESULT.set(init_once(opts));
        });
        if let Some(Err(msg)) = INIT_RESULT.get() {
            Output::panic(format_args!("HTTP client thread failed to start: {}", msg));
        }
    }

    fn init_once(opts: &InitOpts) -> Result<(), String> {
        // Spec HTTPThread.zig:195-206 — initialize the global (with timer
        // started on the calling thread) BEFORE spawning, so `on_start`'s
        // `crate::http_thread_mut()` finds `Some(..)` and can fill in
        // `loop_`/`uws_loop`/contexts.
        // SAFETY: `init_once` runs under `Once`; no other thread reads
        // `HTTP_THREAD` until `has_awoken` is set in `on_start`.
        unsafe {
            (*crate::HTTP_THREAD.get()).write(HttpThread::new());
        }
        crate::HTTP_THREAD_INIT.store(true, core::sync::atomic::Ordering::Release);
        // libdeflate::load() no longer needed — pure Rust flate2 + miniz_oxide backend.
        let ready = thread_ready_pair();
        let opts_copy = opts.clone();
        let ready_guard = ready.clone();
        let ready_body = ready.clone();
        let thread = std::thread::Builder::new()
            .stack_size(bun_threading::thread_pool::DEFAULT_THREAD_STACK_SIZE as usize)
            .spawn(move || {
                run_startup_guarded(ready_guard, move || on_start(opts_copy, ready_body))
            });
        match thread {
            // detach — see HTTP_THREAD_HANDLE note above re: LSAN reachability
            Ok(t) => {
                let _ = HTTP_THREAD_HANDLE.set(t);
            }
            Err(err) => return Err(format!("Failed to start HTTP Client thread: {}", err)),
        }
        // Block until the HTTP thread has finished on_start() and is ready to
        // accept tasks. This guarantees that every subsequent `schedule()` call
        // observes `has_awoken == true`, eliminating the wakeup race entirely.
        // The Condvar wait is the architecturally correct synchronization
        // primitive for this thread-startup handshake (replaces the former
        // yield-and-skip hack which relied on probabilistic timing); the
        // `Failed` arm (a startup panic on the HTTP thread, caught by
        // `run_startup_guarded`) turns what used to be a permanent block into
        // a fast `Err` for `init` to report.
        wait_for_ready(&ready)
    }

    fn on_start(opts: InitOpts, ready: Arc<ReadyPair>) {
        // Late bring-up instead of `configure_named_thread`: an embedding that
        // never initializes bun's Output subsystem (e.g. the servo-embedding
        // `bao_browser::BaoRuntime`) used to die here on
        // `configure_thread`'s `STDOUT_STREAM_SET` debug_assert — the HTTP
        // thread panicked before signaling `ready`, so `init_once` blocked
        // forever on the condvar and every scheduled fetch hung (the U2
        // realworld scenario_1 "pipeline not ready" skip). That symptom class
        // is closed by `run_startup_guarded` (any pre-signal panic now
        // surfaces as a fast `Err` from `init`); the late bring-up below
        // removes this particular panic's cause. The HTTP thread never
        // executes JS, so the no-JS source (no StackCheck FFI — see
        // `configure_thread_no_js`'s doc, which names the HTTP client thread
        // as its intended user) is the correct shape, adopting the real stdio
        // fds when no CLI-side init ran.
        bun_core::Global::set_thread_name(bun_core::zstr!("HTTP Client"));
        Output::Source::ensure_thread_source();
        // PERF(port): was MimallocArena bulk-free for bun.http.default_allocator.

        // upstream 24944aad41: normalise an idle timeout for uSockets' timer
        // wheels. The sweep phase is unrelated to when a socket arms its timer,
        // so a timer armed for N ticks can fire up to one period (4s short
        // wheel, 60s long wheel) BEFORE the requested duration and abort an
        // in-flight request (#39952). Pad by one period so it never fires
        // early, and clamp so the padded value stays below the long wheel's
        // `% 240` minutes wrap (see `us_socket_long_timeout` in
        // packages/bun-usockets/src/socket.c). 0 = disabled. Normalising once
        // here keeps the h1 (`HTTPClient::set_timeout`) and h2
        // (`ClientSession::rearm_timeout`) paths identical without duplicating
        // the math at each call site.
        /// `LIBUS_TIMEOUT_GRANULARITY` (packages/bun-usockets/src/libusockets.h).
        const SHORT_WHEEL_PERIOD_SECONDS: u64 = 4;
        const LONG_WHEEL_PERIOD_SECONDS: u64 = 60;
        /// `SocketTimeout::set_timeout` routes values above this to the long wheel.
        const SHORT_WHEEL_MAX_SECONDS: u64 = 240;
        /// The long counter wraps `% 240` minutes; one minute of pad stays below.
        const MAX_RAW_SECONDS: u64 = 238 * LONG_WHEEL_PERIOD_SECONDS;
        let raw: u64 = bun_core::env_var::BUN_CONFIG_HTTP_IDLE_TIMEOUT
            .get()
            .unwrap_or(300);
        let normalized = if raw == 0 {
            0
        } else {
            let raw = raw.min(MAX_RAW_SECONDS);
            if raw + SHORT_WHEEL_PERIOD_SECONDS > SHORT_WHEEL_MAX_SECONDS {
                (raw.div_ceil(LONG_WHEEL_PERIOD_SECONDS) + 1) * LONG_WHEEL_PERIOD_SECONDS
            } else {
                raw + SHORT_WHEEL_PERIOD_SECONDS
            }
        };
        crate::IDLE_TIMEOUT_SECONDS.store(
            normalized as core::ffi::c_uint,
            core::sync::atomic::Ordering::Relaxed,
        );

        // Zig: `const loop = bun.jsc.MiniEventLoop.initGlobal(null, null)`
        // (HTTPThread.zig:228). Critical side effect: `init_global` calls
        // `internal_loop_data.set_parent_raw(2 /* mini */, mini_ptr)` on this
        // thread's uSockets loop. Without it, the macOS DNS cache-miss path
        // (`dns::getaddrinfo` → `(*loop).internal_loop_data.get_parent()`)
        // panics with `Parent loop not set - pointer is null`, which aborts
        // the process — `bun install` SIGABRT on the first uncached lookup.
        let loop_ = mini_event_loop::init_global(None, None);
        // `init_global` returns the heap-allocated thread-local singleton (never
        // null); this thread owns it for the thread lifetime. `loop_ptr()` reads
        // a stable field via `&self`, so a `ParentRef` shared deref suffices.
        let uws_loop = bun_ptr::ParentRef::from(
            NonNull::new(loop_).expect("init_global returns the thread-local singleton"),
        )
        .loop_ptr();

        #[cfg(windows)]
        {
            // `getenv_w` forwards `name.as_ptr()` directly to Win32
            // `GetEnvironmentVariableW`, which expects a NUL-terminated LPCWSTR.
            // `bun_core::w!` does NOT append a sentinel on its own (see
            // src/sys/windows/mod.rs WATCHER_CHILD_ENV_Z note), so embed `\0`
            // in the literal — Zig's `bun.strings.w("SystemRoot")` returns
            // `[:0]const u16`, this matches that spec (HTTPThread.zig:231).
            if bun_sys::windows::getenv_w(bun_core::w!("SystemRoot\0")).is_none() {
                Output::err_generic(
                    "The %SystemRoot% environment variable is not set. Bun needs this set in order for network requests to work.",
                    (),
                );
                bun_core::Global::crash();
            }
        }

        let thread = crate::http_thread_mut();
        thread.loop_ = loop_;
        thread.uws_loop = uws_loop;
        thread.http_context.init();
        // `https_context.init_with_thread_opts` eagerly builds the BoringSSL
        // `SSL_CTX` and parses the bundled root-CA store
        // (`us_get_default_ca_store`, root_certs.cpp:210), costing ~0.7 ms CPU
        // and ~400 KB heap whether or not an HTTPS request ever happens. When
        // there is no user-supplied CA config we stash `opts` and let the first
        // `connect::<true>` call run it (see `HttpThread::lazy_https_init`) — a
        // fully-cached `bun install` (which makes zero network requests) then
        // skips the cost entirely.
        if !opts.abs_ca_file_name.is_empty() || !opts.ca.is_empty() {
            // User passed --cafile / --ca: validate now so a bad CA file fails
            // the process at thread start (test contract:
            // bun-install-registry.test.ts "non-existent --cafile" /
            // "invalid cafile"), even if the registry is plain HTTP and no SSL
            // connect would ever happen.
            if let Err(err) = thread.https_context.init_with_thread_opts(&opts) {
                (opts.on_init_error)(err, &opts);
            }
        } else {
            // No CA config — safe to defer the ~0.7 ms / ~400 KB root-cert
            // parse to the first SSL connect (warm-cache `bun install` makes
            // none).
            thread.lazy_https_init = Some(opts);
        }
        // Release: publishes `uws_loop`/`loop_` to cross-thread `wakeup()`
        // readers (which Acquire-load `has_awoken`).
        thread.has_awoken.store(true, Ordering::Release);
        // Signal the caller thread that we are ready. The Condvar notify
        // unblocks `init_once()` which is waiting in `wait_for_ready()`,
        // guaranteeing that every subsequent `schedule()` call observes
        // `has_awoken == true`.
        signal_ready(&ready);
        thread.process_events();
    }

    impl HttpThread {
        fn process_events(&mut self) -> ! {
            let uws_loop = self.uws_loop_mut();
            #[cfg(unix)]
            {
                uws_loop.num_polls = uws_loop.num_polls.max(2);
            }
            #[cfg(windows)]
            {
                uws_loop.inc();
            }

            loop {
                if SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
                    self.dealloc_in_flight_for_exit();
                    {
                        let mut done = SHUTDOWN_DONE.0.lock();
                        *done = true;
                        SHUTDOWN_DONE.1.notify_all();
                    }
                    // The JS thread is in `global_exit()` and will call
                    // `Global::exit()` after we ack. Park forever so the loop
                    // never ticks the (now partially-freed) sockets again.
                    loop {
                        std::thread::park();
                    }
                }
                self.drain_events();
                Output::flush();

                let uws_loop = self.uws_loop_mut();
                uws_loop.inc();
                // BCE-20260618-007-R3 extension: the HTTPThread's tick used the
                // NULL-timeout us_loop_run_bun_tick, which makes epoll_pwait2
                // block indefinitely when no fd is ready. R3 fixed this for the
                // JS thread (timers.rs) but missed the HTTPThread. Use the
                // zero-timeout tick_without_idle so the loop returns after
                // draining ready fds — the HTTPThread's process_events loop
                // re-enters immediately, no CPU waste (it only does HTTP I/O).
                uws_loop.tick_without_idle();
                uws_loop.dec();

                if cfg!(debug_assertions) {
                    Output::flush();
                }
            }
        }
    }

    #[cfg(test)]
    mod startup_guard_tests {
        use super::*;

        /// The exact wrapper the real spawn in `init_once` puts around
        /// `on_start`, driven with an injected panicking body: the handshake
        /// must surface `Err` with the panic message quickly instead of
        /// parking `init_once` on the condvar forever.
        #[test]
        fn startup_panic_before_ready_surfaces_err_not_hang() {
            let ready = fresh_ready_pair();
            let guard_ready = ready.clone();
            let spawned = std::thread::spawn(move || {
                run_startup_guarded(guard_ready, || panic!("injected startup failure"))
            });
            // Watchdog: run the wait side on its own thread and fail on
            // timeout — a regression to the condvar hang shows up as a test
            // failure, not a dead test binary.
            let waiter_ready = ready.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(wait_for_ready(&waiter_ready));
            });
            let result = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("wait_for_ready did not return — handshake hung");
            spawned.join().unwrap();
            let Err(msg) = result else {
                panic!("startup panic must surface as Err, got Ok");
            };
            assert!(
                msg.contains("injected startup failure"),
                "panic message not propagated: {msg}"
            );
        }

        /// Success path: the body signals ready (as `on_start` does) and the
        /// handshake returns `Ok`.
        #[test]
        fn startup_ready_signal_returns_ok() {
            let ready = fresh_ready_pair();
            let guard_ready = ready.clone();
            let body_ready = ready.clone();
            std::thread::spawn(move || {
                run_startup_guarded(guard_ready, move || signal_ready(&body_ready))
            });
            wait_for_ready(&ready).expect("ready signal must return Ok");
        }

        /// `on_start` signals ready *before* entering `process_events`; a
        /// panic after the signal (in the loop body) must not downgrade
        /// `Ready` to `Failed`.
        #[test]
        fn panic_after_ready_keeps_ready() {
            let ready = fresh_ready_pair();
            let guard_ready = ready.clone();
            let body_ready = ready.clone();
            let spawned = std::thread::spawn(move || {
                run_startup_guarded(guard_ready, move || {
                    signal_ready(&body_ready);
                    panic!("post-ready crash (process_events analog)");
                })
            });
            wait_for_ready(&ready).expect("ready already signaled");
            spawned.join().unwrap();
            assert!(matches!(&*lock_ready(&ready), ThreadReady::Ready));
        }
    }
}

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_DONE: (bun_threading::Guarded<bool>, bun_threading::Condvar) = (
    bun_threading::Guarded::new(false),
    bun_threading::Condvar::new(),
);

struct ShutdownReclaim {
    ctx: *mut c_void,
    drop_fn: unsafe fn(*mut c_void),
}
// SAFETY: pushed from the HTTP thread, drained from the JS thread once the
// HTTP thread is parked; `ctx` is an exclusive heap allocation handed off
// between the two.
unsafe impl Send for ShutdownReclaim {}

static SHUTDOWN_RECLAIMS: bun_threading::Guarded<Vec<ShutdownReclaim>> =
    bun_threading::Guarded::new(Vec::new());

/// Park `(ctx, drop_fn)` until [`shutdown_for_exit`] has waited the HTTP
/// thread out of its loop. The drop is applied on the JS thread once the
/// daemon is parked, so callers can hand off allocations whose teardown is
/// not safe while a `tick()` is still on the HTTP-thread stack.
pub fn defer_shutdown_reclaim(ctx: *mut c_void, drop_fn: unsafe fn(*mut c_void)) {
    SHUTDOWN_RECLAIMS
        .lock()
        .push(ShutdownReclaim { ctx, drop_fn });
}

/// Called from `bun_jsc::VirtualMachine::global_exit()` on the JS thread,
/// before `~VM`. Asks the HTTP daemon thread to reclaim every in-flight
/// `ThreadlocalAsyncHTTP` box and waits (with a short timeout) for it to ack.
/// No-op if the HTTP thread was never started.
pub fn shutdown_for_exit() {
    if !crate::HTTP_THREAD_INIT.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: `HTTP_THREAD_INIT == true` ⇒ `HTTP_THREAD` is fully written.
    // `get_unchecked` so the `ThreadCell` owner assert is skipped on this
    // cross-thread caller; `ParentRef` so only a shared `&HttpThread` is
    // materialised — `process_events(&mut self)` is live on the HTTP thread,
    // so a `&mut` here would alias. Same shape as `schedule()` above.
    let thread = unsafe {
        bun_ptr::ParentRef::<HttpThread>::from_raw(
            (*crate::HTTP_THREAD.get_unchecked()).as_mut_ptr(),
        )
    };
    if !thread.has_awoken.load(Ordering::Acquire) {
        // `on_start` hasn't published the loop yet — no `start_queued_task`
        // can have run, so no boxes exist.
        return;
    }
    SHUTDOWN_REQUESTED.store(true, Ordering::Release);
    thread.wakeup();
    let mut done = SHUTDOWN_DONE.0.lock();
    // 1s upper bound: a stuck HTTP thread shouldn't deadlock process exit.
    let deadline = Instant::now() + std::time::Duration::from_secs(1);
    while !*done {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        if SHUTDOWN_DONE
            .1
            .timed_wait_guarded(&mut done, remaining.as_nanos() as u64)
            .is_err()
        {
            break;
        }
    }
    let acked = *done;
    drop(done);
    if !acked {
        // Timed out without an ack: the HTTP thread may still be inside
        // `tick()` and could touch parked allocations. Leak them — the
        // process is exiting and a leak beats a use-after-free.
        return;
    }

    // The daemon is parked; no further callbacks will fire. Reclaim boxes
    // that result-callback handlers parked here while the calling stack
    // still aliased their contents.
    for r in core::mem::take(&mut *SHUTDOWN_RECLAIMS.lock()) {
        // SAFETY: `drop_fn` is paired with `ctx` by `defer_shutdown_reclaim`;
        // each entry is pushed exactly once and drained exactly once here.
        unsafe { (r.drop_fn)(r.ctx) };
    }
}

// dispatch_deps bridge removed — real impls now live in
// h3_client/ClientContext.rs (abort_by_http_id / stream_body_by_http_id).

/// Module-level bridge for `HTTPThread::init`. The real body lives in
/// `_event_loop_draft` below (depends on `bun_event_loop::MiniEventLoop`,
/// which is outside this crate's dep set). Call sites in AsyncHTTP.rs hit
/// this until that tier boundary is resolved.
pub fn init(opts: &InitOpts) {
    _event_loop_draft::init(opts)
}

// ported from: src/http/HTTPThread.zig
