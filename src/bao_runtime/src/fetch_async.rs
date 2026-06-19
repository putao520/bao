// @trace REQ-ENG-010 [entity:FetchTasklet] [req:REQ-ENG-010] [level:library]
//! Async fetch/HTTP integration with the event loop (FetchTasklet pattern).
//!
//! ## Architecture (FetchTasklet event-driven paradigm)
//!
//! Every JS-native http/https/tls entry returns a *pending* `Promise` and
//! delegates the actual network I/O to `AsyncHTTP::init + HTTPThread::schedule`.
//! The HTTPThread runs a dedicated epoll loop and calls back `on_http_done`
//! (pure-Rust, zero SM API) when the response is ready. That callback writes
//! the result into the `outcome` slot and enqueues a `ConcurrentTask`
//! (`resolve_tasklet`) on the JS thread's `MiniEventLoop`, which wakes the
//! JS thread via `us_wakeup_loop`. The JS-thread ConcurrentTask callback
//! builds the Response/error JS object and `ResolvePromise`/`RejectPromise`s.
//!
//! This mirrors Bun's `FetchTasklet` design exactly:
//!   - `AsyncHTTP::init+schedule` = Bun's `FetchTasklet::init+schedule`
//!   - `on_http_done` = Bun's `HTTPCallback` (HTTPThread, pure-Rust)
//!   - `resolve_tasklet` = Bun's JS-thread resolve via ConcurrentTask
//!   - `poll_ref::ref/unref_concurrently` = Bun's `refConcurrently` keepalive
//!
//! ## Why this replaced thread::spawn (BCE-20260619-010)
//!
//! The prior `thread::spawn` + `drain_pending` polling model had three flaws:
//!   1. O(N) OS threads for N concurrent fetches (violates REQ-ENG-010:
//!      "N并发fetch占OS线程数=O(1)")
//!   2. JS-thread busy-poll `sleep(1ms)` in the fetch-only case (wasteful)
//!   3. `drain_pending` must be called every tick (fragile coupling)
//!
//! The event-driven model fixes all three: HTTPThread uses a single epoll fd,
//! ConcurrentTask auto-wakes the JS thread, and no polling is needed.
//!
//! ## Scope
//!
//! Shared helper used by the HTTP-sweep entries:
//! - `node_http.rs:http_request` / `http_get`
//! - `node_https.rs:https_request`
//! - `node_tls.rs:tls_connect`
//!
//! `h3_fetch.rs` is excluded — it has no `send_sync` path.

use ::std::cell::RefCell;
use ::std::ffi::c_char;
use ::std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use ::std::sync::{Arc, Mutex};

use bun_core::ZBox;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, StringValue, UndefinedValue};

use crate::stealth_http::{StealthSyncResult, stealth_http_request};

// ──────────────────────────────────────────────────────────────────────────
// Public types
// ──────────────────────────────────────────────────────────────────────────

/// HTTPThread result of a scheduled fetch. Pure data -- no SM handles -- so
/// it can cross the thread boundary freely (INV-5: no SM API on HTTP thread).
type FetchOutcome = ::std::result::Result<StealthSyncResult, String>;

/// How to materialize the result as a JS object on resolve. Different
/// JS-native entries want different shapes: `fetch`/`http.request`/`https.request`
/// want a `Response`; `tls.connect` (a TLS handshake probe) wants a `TLSSocket`.
#[derive(Clone, Copy)]
pub enum ResolveKind {
    /// Build a fetch-style Response object (status/ok/headers/text()).
    Response,
    /// Build a TLSSocket object (authorized/encrypted/servername). `host` is
    /// captured by the probe caller and surfaced as `servername`.
    TlsSocket { host_idx: usize },
}

// Host strings captured at JS-native-call time, indexed by `ResolveKind`-
// carried indices. The HTTPThread must not touch JS state, so we pass plain
// Rust strings across and let the JS-thread resolver look them up by index.
thread_local! {
    static HOST_STRINGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A fetch tasklet: pending Promise + event-driven HTTP integration.
///
/// Invariants (FetchTaskletLifecycle SM):
/// - `rooted == true` while the Promise is outstanding (heap root held).
/// - Every terminal transition (resolve/reject) must `RemoveRawValueRoot`.
/// - `has_schedule_callback` prevents duplicate ConcurrentTask scheduling.
/// - `outcome` is written by `on_http_done` (HTTPThread) and consumed by
///   `resolve_tasklet` (JS thread via ConcurrentTask).
pub struct PendingFetch {
    /// SpiderMonkey context that owns the Promise. Only touched on the JS thread.
    pub cx: *mut JSContext,
    /// Heap-rooted pending Promise *value*. Rooted while `outcome.is_none()`.
    pub promise_val: JSVal,
    /// `true` while `AddRawValueRoot` is in effect.
    pub rooted: bool,
    /// HTTPThread result slot. `None` until `on_http_done` writes the outcome.
    pub outcome: Arc<Mutex<Option<FetchOutcome>>>,
    /// How to materialize the result on the JS thread.
    pub kind: ResolveKind,
    /// Pointer to the JS thread's `MiniEventLoop<'static>`. Used by
    /// `on_http_done` to enqueue `resolve_tasklet` and wake the JS thread.
    mini_loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static>,
    /// ConcurrentTask carrier: embedded `AnyTaskWithExtraContext` that
    /// `resolve_tasklet` uses. Initialized by `start_with_kind`, consumed
    /// by the MiniEventLoop concurrent-task dispatcher.
    concurrent_task: bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext,
    /// Prevents duplicate ConcurrentTask scheduling: `on_http_done` does a
    /// `compare_exchange(false → true)` before enqueuing; `resolve_tasklet`
    /// stores `false` after consuming.
    has_schedule_callback: AtomicBool,
}

// SAFETY: `cx`/`promise_val` are only ever dereferenced on the JS thread that
// created them; the HTTPThread only touches `outcome` and
// `has_schedule_callback` (pure Rust / atomic). Sending the struct across
// threads is sound as long as no SM API is called off the JS thread --
// enforced by keeping all SM access behind `resolve_tasklet` (JS-thread only).
unsafe impl Send for PendingFetch {}

// ──────────────────────────────────────────────────────────────────────────
// Pending-fetch registry (JS-thread local)
//
// PENDING is still needed as a GC root collection (prevents SM from collecting
// the pending Promise while the HTTPThread is in flight). But it is NOT
// polled anymore -- ConcurrentTask auto-dispatches resolve_tasklet.
// ──────────────────────────────────────────────────────────────────────────

thread_local! {
    static PENDING: RefCell<Vec<*mut PendingFetch>> = const { RefCell::new(Vec::new()) };
}

/// JS-thread poll: are there any outstanding async fetches on this thread?
pub fn has_pending() -> bool {
    PENDING.with(|p| !p.borrow().is_empty())
}

// ──────────────────────────────────────────────────────────────────────────
// start() — JS-thread: register a pending fetch + schedule AsyncHTTP
// ──────────────────────────────────────────────────────────────────────────

/// Schedule an async fetch via `AsyncHTTP::init + HTTPThread::schedule`.
/// The caller must have already created the pending Promise via
/// `JS::NewPromiseObject(cx, null)`, pass it here as `promise_val`
/// (an Object JSVal), and then set `args.rval()` to the same value before
/// returning from the extern-C trampoline.
///
/// This function:
///   1. Heap-roots the Promise value (GUARD-A: SM GC safety across ticks).
///   2. Creates `AsyncHTTP::init` with TLS fingerprint injection.
///   3. Schedules via `HTTPThread::schedule` (single epoll thread, O(1) OS threads).
///   4. `ref_concurrently` on the event loop (keepalive while fetch is in flight).
///   5. Pushes a `PendingFetch` onto the JS-thread registry.
///
/// # Safety
///
/// - `cx` must be a live `JSContext*` on the current thread.
/// - `promise_val` must be an Object JSVal holding a *pending* Promise.
pub unsafe fn start(
    cx: *mut JSContext,
    promise_val: JSVal,
    profile: Option<crate::stealth_http::StealthProfile>,
    method: bun_http::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
) {
    // SAFETY: delegate to the kind-aware start with the default Response form.
    unsafe { start_with_kind(cx, promise_val, profile, method, url, headers, body, ResolveKind::Response) }
}

/// Schedule a TLS handshake probe: a single stealth HTTPS HEAD against
/// `host:port`. The Promise resolves to a TLSSocket-shaped object
/// (`authorized`/`encrypted`/`servername`) on success, or rejects on error.
/// `host` is captured so the resolver can surface it as `servername`.
///
/// # Safety
///
/// - `cx` must be a live `JSContext*` on the current thread.
/// - `promise_val` must be an Object JSVal holding a *pending* Promise.
pub unsafe fn start_tls_probe(
    cx: *mut JSContext,
    promise_val: JSVal,
    host: String,
    port: u16,
) {
    let test_url = format!("https://{}:{}", host, port);
    // Capture the host string on the JS thread; the resolver looks it up by
    // index so the HTTPThread never touches JS state.
    let host_idx = HOST_STRINGS.with(|h| {
        let mut g = h.borrow_mut();
        let idx = g.len();
        g.push(host);
        idx
    });
    // SAFETY: cx live on this thread; promise_val is the pending Promise.
    unsafe {
        start_with_kind(
            cx,
            promise_val,
            None,
            bun_http::Method::HEAD,
            test_url,
            Vec::new(),
            None,
            ResolveKind::TlsSocket { host_idx },
        )
    }
}

/// Kind-aware scheduler. Creates `AsyncHTTP::init`, schedules on the
/// HTTPThread, and registers the PendingFetch for GC root protection.
///
/// # Safety
///
/// - `cx` must be a live `JSContext*` on the current thread.
/// - `promise_val` must be an Object JSVal holding a *pending* Promise.
unsafe fn start_with_kind(
    cx: *mut JSContext,
    promise_val: JSVal,
    profile: Option<crate::stealth_http::StealthProfile>,
    method: bun_http::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    kind: ResolveKind,
) {
    // GUARD-A (GC root): heap-root the pending Promise value across the async
    // window. The async window spans ticks, so the stack-rooted!() macro
    // (whose roots die with the frame) is unsound here -- we use the runtime's
    // raw root table instead.
    let mut pv = promise_val;
    let rooted = unsafe {
        let name = b"FetchTasklet.promise\0".as_ptr() as *const c_char;
        AddRawValueRoot(cx, &mut pv, name)
    };
    let rooted_val = if rooted { pv } else { promise_val };

    let outcome: Arc<Mutex<Option<FetchOutcome>>> = Arc::new(Mutex::new(None));

    // Allocate the PendingFetch on the heap. The pointer is shared between
    // the HTTPThread callback (on_http_done) and the JS-thread ConcurrentTask
    // callback (resolve_tasklet). It is freed by resolve_tasklet after
    // resolving/rejecting the promise (single-consumer: resolve_tasklet owns
    // the deallocation).
    let pending = Box::new(PendingFetch {
        cx,
        promise_val: rooted_val,
        rooted,
        outcome: Arc::clone(&outcome),
        kind,
        mini_loop_ptr: ::std::ptr::null(), // filled below
        concurrent_task: bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext::default(),
        has_schedule_callback: AtomicBool::new(false),
    });
    let pending_ptr = Box::into_raw(pending);

    // ── Schedule the actual HTTP request ──────────────────────────────────
    // Use `AsyncHTTP::init` (event-driven) instead of `thread::spawn`.
    // The HTTPThread's epoll loop drives the request; `on_http_done`
    // is called back on the HTTPThread when the response is ready.
    let _on_done_outcome = Arc::clone(&outcome);

    // Build the HTTPClientResultCallback that will fire on the HTTPThread.
    // INV-5: on_http_done must never call SM API (only touches pure Rust).
    let callback =
        bun_http::HTTPClientResultCallback::new(pending_ptr, on_http_done);

    // Parse URL and build header entries.
    let parsed_url = bun_url::URL::parse(url.as_bytes());

    // Build headers via HeaderBuilder (same pattern as http_client.rs).
    let mut hb = bun_http::HeaderBuilder::default();
    for (name, value) in &headers {
        hb.count(name.as_bytes(), value.as_bytes());
    }
    if hb.allocate().is_err() {
        // Header allocation failure -- reject the promise.
        let mut outcome_guard = outcome.lock().unwrap();
        *outcome_guard = Some(Err("fetch: header allocation failed".into()));
        drop(outcome_guard);
        // Schedule resolve immediately on JS thread.
        schedule_resolve_on_js_thread(pending_ptr);
        return;
    }
    for (name, value) in &headers {
        hb.append(name.as_bytes(), value.as_bytes());
    }
    let entry_list = hb.entries;
    let headers_buf: &[u8] = unsafe {
        if let Some(ptr) = hb.content.ptr {
            ::std::slice::from_raw_parts(ptr.as_ptr(), hb.content.len)
        } else {
            &[]
        }
    };

    // Response buffer (heap-allocated, owned by AsyncHTTP).
    let response_buffer =
        Box::into_raw(Box::new(bun_core::string::MutableString::default()));

    // Request body slice.
    let body_slice: &[u8] = body.as_deref().unwrap_or_default();

    // TLS fingerprint: StealthProfile → SSLConfig → SSLConfigSharedPtr.
    let tls_props = {
        let ssl_config =
            crate::stealth_http::stealth_profile_to_ssl_config(&profile);
        Some(bun_http::ssl_config::SharedPtr::new(ssl_config))
    };

    // Build AsyncHTTP::Options with TLS props.
    let options = bun_http::async_http::Options {
        tls_props,
        ..Default::default()
    };

    // Initialize AsyncHTTP (event-driven, no blocking).
    let mut async_http = bun_http::AsyncHTTP::init(
        method,
        parsed_url,
        entry_list,
        headers_buf,
        response_buffer,
        body_slice,
        callback,
        bun_http::FetchRedirect::Follow,
        options,
    );

    // Capture the MiniEventLoop pointer for concurrent-task scheduling.
    // SAFETY: with_event_loop borrows the MiniEventLoop on the current thread;
    // the pointer remains valid for the thread's lifetime (intentionally
    // leaked on thread exit, same as BaoEventLoop).
    let loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static> =
        crate::timers::with_event_loop(|loop_| loop_ as *const _);
    // Write the pointer and initialize the ConcurrentTask into the PendingFetch.
    // SAFETY: pending_ptr is a live heap allocation we just created.
    unsafe {
        (*pending_ptr).mini_loop_ptr = loop_ptr;

        // Initialize the ConcurrentTask embedded in PendingFetch.
        // `resolve_tasklet` is the callback that fires on the JS thread.
        // The field_offset tells AnyTaskWithExtraContext where it lives
        // inside the parent struct.
        let _field_offset = ::std::mem::offset_of!(PendingFetch, concurrent_task);
        (*pending_ptr)
            .concurrent_task
            .from(pending_ptr, resolve_tasklet_shim);
    }

    // Ensure HTTPThread is initialized before scheduling. `init` is idempotent
    // (backed by `Once`); this mirrors Bun's `AsyncHTTP.rs:414` guard for the
    // case where fetch is the process's first HTTP operation.
    bun_http::http_thread::init(&Default::default());

    // Schedule the AsyncHTTP task on the HTTPThread (single epoll thread).
    let batch = bun_threading::thread_pool::Batch::from(
        core::ptr::addr_of_mut!(async_http.task)
    );
    bun_http::HTTPThread::schedule(batch);

    // Leak the AsyncHTTP -- its memory is managed by the HTTPThread task system.
    // When the task completes, on_http_done fires and the HTTPThread frees the
    // task node. The AsyncHTTP data itself lives until the callback fires.
    ::std::mem::forget(async_http);

    // ref_concurrently: keep the event loop alive while this fetch is in flight.
    // Mirrors Bun's `FetchTasklet.refConcurrently()`.
    // Only valid when the EventLoopCtx is JS-VM-backed (MiniEventLoop's
    // ref_concurrently is unreachable). In test/embedded contexts without a
    // full JS-VM loop, the fetch still works — the test just won't keep the
    // process alive on its own (which is correct: tests control their own exit).
    {
        let ctx = crate::timers::with_event_loop(|loop_| {
            bun_event_loop::MiniEventLoop::MiniEventLoop::as_event_loop_ctx(loop_)
        });
        if ctx.is_js() {
            ctx.ref_concurrently();
        }
    }

    // Register the PendingFetch pointer in the GC root collection.
    PENDING.with(|p| {
        p.borrow_mut().push(pending_ptr);
    });
}

// ──────────────────────────────────────────────────────────────────────────
// on_http_done — HTTPThread callback (pure-Rust, zero SM API, INV-5)
// ──────────────────────────────────────────────────────────────────────────

/// HTTPThread completion callback. Called by `AsyncHTTP` when the HTTP
/// response is ready (or an error occurred). This runs on the HTTPThread,
/// NOT the JS thread, so it must never call SM API (INV-5).
///
/// It:
///   1. Copies the `HTTPClientResult` into a `FetchOutcome` (pure Rust).
///   2. Writes the outcome into the shared slot.
///   3. Atomically claims the scheduling slot (`has_schedule_callback`).
///   4. If claimed, enqueues `resolve_tasklet` on the JS thread's
///      `MiniEventLoop` via `enqueue_task_concurrent_with_extra_ctx`,
///      which auto-wakes the JS thread.
fn on_http_done(
    this: *mut PendingFetch,
    _async_http: *mut bun_http::AsyncHTTP<'static>,
    result: bun_http::HTTPClientResult<'_>,
) {
    // 1. Convert HTTPClientResult → FetchOutcome (pure Rust).
    let outcome: FetchOutcome = if let Some(fail) = result.fail {
        Err(format!("{:?}", fail))
    } else {
        // Extract status code, status text, headers, and body from the result.
        let status_code = result
            .metadata
            .as_ref()
            .map(|m| m.response.status_code)
            .unwrap_or(0);
        let status_text: compact_str::CompactString = result
            .metadata
            .as_ref()
            .map(|m| {
                ::std::str::from_utf8(m.response.status)
                    .unwrap_or("")
                    .into()
            })
            .unwrap_or_default();

        // Headers from picohttp Response.
        let headers: smallvec::SmallVec<[(compact_str::CompactString, compact_str::CompactString); 8]> = result
            .metadata
            .as_ref()
            .map(|m| {
                m.response
                    .headers
                    .list
                    .iter()
                    .map(|h| {
                        (
                            compact_str::CompactString::from(
                                ::std::str::from_utf8(h.name()).unwrap_or(""),
                            ),
                            compact_str::CompactString::from(
                                ::std::str::from_utf8(h.value()).unwrap_or(""),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Body from the MutableString response buffer.
        let body_bytes: bytes::Bytes = result
            .body
            .map(|ms| bytes::Bytes::copy_from_slice(ms.list.as_slice()))
            .unwrap_or_default();

        Ok(StealthSyncResult {
            status_code,
            status_text,
            headers,
            body: body_bytes,
        })
    };

    // 2. Write outcome into the shared slot.
    // SAFETY: this is a shared Arc<Mutex<>>, safe to lock from any thread.
    if let Ok(mut guard) = unsafe { &*this }.outcome.lock() {
        *guard = Some(outcome);
    }

    // 3. Atomically claim the scheduling slot.
    // compare_exchange: if false → true, we are the first to schedule.
    // If already true, resolve_tasklet is already scheduled or running.
    if unsafe { &*this }
        .has_schedule_callback
        .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
        .is_ok()
    {
        // 4. Enqueue resolve_tasklet on the JS thread's MiniEventLoop.
        // SAFETY: mini_loop_ptr was set by start_with_kind on the JS thread;
        // it remains valid for the thread's lifetime.
        let loop_ptr = unsafe { &*this }.mini_loop_ptr;
        if !loop_ptr.is_null() {
            // SAFETY: the MiniEventLoop is alive for the thread's lifetime.
            let loop_ref = unsafe { &mut *(
                loop_ptr as *mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>
            ) };
            let concurrent_task_ptr =
                unsafe { core::ptr::addr_of_mut!((*this).concurrent_task) };
            // SAFETY: concurrent_task_ptr is a valid pointer to the
            // AnyTaskWithExtraContext embedded in PendingFetch.
            loop_ref.enqueue_task_concurrent(unsafe {
                core::ptr::NonNull::new_unchecked(concurrent_task_ptr)
            });
            // enqueue_task_concurrent internally calls wakeup(), so the
            // JS thread will be woken from any blocking epoll_wait.
        }
    }
}

/// Shim that bridges `AnyTaskWithExtraContext` callback signature to
/// `resolve_tasklet`. The `ctx` parameter is the `*mut PendingFetch`.
/// This must be a safe fn because `AnyTaskWithExtraContext::from` expects
/// `fn(*mut T, *mut ())`, not `unsafe fn`.
fn resolve_tasklet_shim(ctx: *mut PendingFetch, _parent: *mut ()) {
    // SAFETY: ctx was set to pending_ptr in start_with_kind; it is a valid
    // heap-allocated PendingFetch that has not been freed yet.
    unsafe { resolve_tasklet(ctx) };
}

/// JS-thread ConcurrentTask callback. Fires when `on_http_done` enqueues
/// this task on the MiniEventLoop. Runs on the JS thread (safe to call SM API).
///
/// It:
///   1. Resets `has_schedule_callback` (allows future scheduling if needed).
///   2. Takes the outcome from the shared slot.
///   3. Builds the Response/error JS object and resolves/rejects the Promise.
///   4. `RemoveRawValueRoot` (terminal cleanup).
///   5. `unref_concurrently` (keepalive decrement).
///   6. Deallocates the `PendingFetch` Box.
///   7. Removes from PENDING registry.
unsafe fn resolve_tasklet(this: *mut PendingFetch) {
    // 1. Reset scheduling flag.
    unsafe { &*this }
        .has_schedule_callback
        .store(false, AtomicOrdering::Release);

    // 2. Take the outcome from the shared slot.
    let outcome = unsafe { &*this }
        .outcome
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .unwrap_or_else(|| Err("fetch: result slot was empty".into()));

    let cx = unsafe { &*this }.cx;
    let kind = unsafe { &*this }.kind;

    // 3. Build Response/error JS object and resolve/reject the Promise.
    // We reconstruct the PendingFetch fields needed by resolve_tasklet_inner.
    let promise_val = unsafe { &*this }.promise_val;
    let rooted = unsafe { &*this }.rooted;

    let promise_obj = promise_val.to_object();
    let promise_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &promise_obj,
    };

    match (outcome, kind) {
        (Ok(resp), ResolveKind::Response) => {
            let resp_obj = build_response_js(cx, &resp);
            if !resp_obj.is_null() {
                let resp_val = ObjectValue(resp_obj);
                let resp_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &resp_val,
                };
                JS::ResolvePromise(cx, promise_h, resp_handle);
            } else {
                reject_with_message(cx, promise_h, "http: failed to build Response");
            }
        }
        (Ok(_resp), ResolveKind::TlsSocket { host_idx }) => {
            let host = HOST_STRINGS
                .with(|h| h.borrow().get(host_idx).cloned())
                .unwrap_or_default();
            let tls_obj = build_tls_socket_js(cx, &host);
            if !tls_obj.is_null() {
                let tls_val = ObjectValue(tls_obj);
                let tls_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &tls_val,
                };
                JS::ResolvePromise(cx, promise_h, tls_handle);
            } else {
                reject_with_message(cx, promise_h, "tls: failed to build socket object");
            }
            HOST_STRINGS.with(|h| {
                if host_idx < h.borrow().len() {
                    h.borrow_mut()[host_idx].clear();
                }
            });
        }
        (Err(msg), _) => {
            reject_with_message(cx, promise_h, &msg);
        }
    }

    // 4. Terminal cleanup: unroot (every exit path).
    if rooted {
        let mut pv = promise_val;
        RemoveRawValueRoot(cx, &mut pv);
    }

    // 5. unref_concurrently: decrement keepalive (must balance ref_concurrently
    //    in start_with_kind). Only valid for JS-VM-backed loops.
    {
        let ctx = crate::timers::with_event_loop(|loop_| {
            bun_event_loop::MiniEventLoop::MiniEventLoop::as_event_loop_ctx(loop_)
        });
        if ctx.is_js() {
            ctx.unref_concurrently();
        }
    }

    // 6. Remove from PENDING registry.
    PENDING.with(|p| {
        let mut guard = p.borrow_mut();
        if let Some(pos) = guard.iter().position(|&ptr| ptr == this) {
            guard.swap_remove(pos);
        }
    });

    // 7. Deallocate the PendingFetch Box.
    // SAFETY: this pointer was allocated by Box::into_raw in start_with_kind.
    // We are the sole consumer (ConcurrentTask runs once); no other code
    // accesses the Box after this point.
    unsafe {
        drop(Box::from_raw(this));
    }

    // Flush microtasks queued by ResolvePromise/RejectPromise.
    mozjs_sys::jsapi::js::RunJobs(cx);
}

/// Helper: schedule resolve_tasklet on the JS thread immediately (used when
/// the request fails synchronously before HTTPThread scheduling, e.g. header
/// allocation failure).
fn schedule_resolve_on_js_thread(pending_ptr: *mut PendingFetch) {
    // Write a sentinel outcome so resolve_tasklet has something to consume.
    // (Already written by caller before calling this function.)

    // Try to claim the scheduling slot.
    if unsafe { &*pending_ptr }
        .has_schedule_callback
        .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
        .is_ok()
    {
        let loop_ptr = unsafe { &*pending_ptr }.mini_loop_ptr;
        if !loop_ptr.is_null() {
            let loop_ref = unsafe { &mut *(
                loop_ptr as *mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>
            ) };
            let concurrent_task_ptr =
                unsafe { core::ptr::addr_of_mut!((*pending_ptr).concurrent_task) };
            loop_ref.enqueue_task_concurrent(unsafe {
                core::ptr::NonNull::new_unchecked(concurrent_task_ptr)
            });
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// JS Response / TLSSocket / rejection builders
// ──────────────────────────────────────────────────────────────────────────

/// Build a TLSSocket-shaped JS object: `{ authorized: true, encrypted: true,
/// servername: host }`. Mirrors the legacy synchronous `tls.connect` return
/// shape so consumers are unaffected.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn build_tls_socket_js(cx: *mut JSContext, host: &str) -> *mut JSObject {
    let obj = JS_NewPlainObject(cx);
    if obj.is_null() {
        return obj;
    }
    let obj_handle = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };

    let auth_val = mozjs::jsval::BooleanValue(true);
    let auth_h = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &auth_val,
    };
    JS_DefineProperty(cx, obj_handle, c"authorized".as_ptr(), auth_h, JSPROP_ENUMERATE as u32);

    let enc_val = mozjs::jsval::BooleanValue(true);
    let enc_h = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &enc_val,
    };
    JS_DefineProperty(cx, obj_handle, c"encrypted".as_ptr(), enc_h, JSPROP_ENUMERATE as u32);

    if !host.is_empty() {
        let c_host = ZBox::from_bytes(host.as_bytes());
        let host_js = JS_NewStringCopyZ(cx, c_host.as_ptr());
        if !host_js.is_null() {
            let hv = StringValue(&*host_js);
            let hv_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &hv,
            };
            JS_DefineProperty(cx, obj_handle, c"servername".as_ptr(), hv_h, JSPROP_ENUMERATE as u32);
        }
    }

    obj
}

/// Construct the JS Response object from a `StealthSyncResult`. Shape mirrors
/// fetch_api's Response: `status`/`ok`/`statusText`/`headers`/`_bodyText`/
/// `text()`.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn build_response_js(cx: *mut JSContext, resp: &StealthSyncResult) -> *mut JSObject {
    let obj = JS_NewPlainObject(cx);
    if obj.is_null() {
        return obj;
    }
    let obj_handle = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };

    // status: int32
    let status_val = mozjs::jsval::Int32Value(resp.status_code as i32);
    let s_handle = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &status_val,
    };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    // ok: boolean (2xx)
    let ok_val = mozjs::jsval::BooleanValue((200..300).contains(&resp.status_code));
    let ok_handle = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &ok_val,
    };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    // statusText
    {
        let c_st = ZBox::from_bytes(resp.status_text.as_bytes());
        let st_js = JS_NewStringCopyZ(cx, c_st.as_ptr());
        if !st_js.is_null() {
            let st_val = StringValue(&*st_js);
            let st_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &st_val,
            };
            JS_DefineProperty(
                cx,
                obj_handle,
                c"statusText".as_ptr(),
                st_handle,
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // headers (flattened to a plain enumerable object)
    {
        let headers_obj = JS_NewPlainObject(cx);
        if !headers_obj.is_null() {
            let hdr_handle = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &headers_obj,
            };
            for (k, v) in resp.headers.iter() {
                let c_k = ZBox::from_bytes(k.as_bytes());
                let k_js = JS_NewStringCopyZ(cx, c_k.as_ptr());
                if k_js.is_null() {
                    continue;
                }
                let kv = StringValue(&*k_js);
                let kv_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &kv,
                };
                let c_v = ZBox::from_bytes(v.as_bytes());
                let v_js = JS_NewStringCopyZ(cx, c_v.as_ptr());
                if v_js.is_null() {
                    continue;
                }
                let vv = StringValue(&*v_js);
                let vv_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &vv,
                };
                let c_key = ZBox::from_bytes(k.as_bytes());
                JS_DefineProperty(
                    cx,
                    hdr_handle,
                    c_key.as_ptr(),
                    vv_handle,
                    JSPROP_ENUMERATE as u32,
                );
                let _ = kv_handle; // suppress unused-assign
            }
            let hv = ObjectValue(headers_obj);
            let hv_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &hv,
            };
            JS_DefineProperty(
                cx,
                obj_handle,
                c"headers".as_ptr(),
                hv_handle,
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // body -- stored as `_bodyText` (lossy UTF-8), surfaced via `.text()`.
    {
        let body_lossy = String::from_utf8_lossy(&resp.body);
        let c_body = ZBox::from_bytes(body_lossy.as_bytes());
        let body_js = JS_NewStringCopyZ(cx, c_body.as_ptr());
        if !body_js.is_null() {
            let bv = StringValue(&*body_js);
            let b_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &bv,
            };
            JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle, 0);
        }
    }

    // text() -- no-arg JS function returning `_bodyText`.
    {
        let text_fn = JS_NewFunction(cx, Some(response_text_fn), 0, 0, c"text".as_ptr());
        if !text_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(text_fn);
            if !fn_obj.is_null() {
                let fv = ObjectValue(fn_obj);
                let fv_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &fv,
                };
                JS_DefineProperty(
                    cx,
                    obj_handle,
                    c"text".as_ptr(),
                    fv_handle,
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }

    obj
}

/// `.text()` method: reads `_bodyText` off the Response and returns it.
#[allow(non_snake_case)]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_text_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let this_obj = this.to_object();
    let this_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &this_obj,
    };
    let mut bt = UndefinedValue();
    let bt_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut bt,
    };
    JS_GetProperty(cx, this_h, c"_bodyText".as_ptr(), bt_h);
    if bt.is_string() {
        args.rval().set(bt);
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

/// Reject a Promise with a plain Error-like object carrying `.message`.
///
/// # Safety
///
/// `cx` must be live on the current thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_with_message(
    cx: *mut JSContext,
    promise_h: Handle<*mut JSObject>,
    msg: &str,
) {
    let err_obj = JS_NewPlainObject(cx);
    if !err_obj.is_null() {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
        if !js_str.is_null() {
            let msg_val = StringValue(&*js_str);
            let msg_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &msg_val,
            };
            let err_h = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &err_obj,
            };
            JS_DefineProperty(
                cx,
                err_h,
                c"message".as_ptr(),
                msg_handle,
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    let ev = if err_obj.is_null() {
        UndefinedValue()
    } else {
        ObjectValue(err_obj)
    };
    let ev_handle = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &ev,
    };
    JS::RejectPromise(cx, promise_h, ev_handle);
}

// ──────────────────────────────────────────────────────────────────────────
// Unit tests -- pure logic (no live JSContext)
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_pending_false_initially() {
        // Each test thread has its own thread_local; should start empty.
        assert!(!has_pending());
    }

    #[test]
    fn pending_fetch_is_send() {
        // Compile-time check: PendingFetch must be Send to live in the
        // thread_local registry that the HTTPThread writes back into.
        fn assert_send<T: Send>() {}
        assert_send::<PendingFetch>();
    }

    #[test]
    fn outcome_slot_roundtrip() {
        let slot: Arc<Mutex<Option<FetchOutcome>>> = Arc::new(Mutex::new(None));
        {
            let mut g = slot.lock().unwrap();
            *g = Some(Ok(stealth_result_for_test()));
        }
        let taken = slot.lock().unwrap().take();
        assert!(taken.is_some());
        match taken.unwrap() {
            Ok(r) => assert_eq!(r.status_code, 200),
            Err(_) => panic!("expected Ok"),
        }
    }

    #[test]
    fn has_schedule_callback_atomic_roundtrip() {
        let pf = PendingFetch {
            cx: ::std::ptr::null_mut(),
            promise_val: UndefinedValue(),
            rooted: false,
            outcome: Arc::new(Mutex::new(None)),
            kind: ResolveKind::Response,
            mini_loop_ptr: ::std::ptr::null(),
            concurrent_task: Default::default(),
            has_schedule_callback: AtomicBool::new(false),
        };
        assert!(!pf.has_schedule_callback.load(AtomicOrdering::Relaxed));
        assert!(
            pf.has_schedule_callback
                .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
                .is_ok()
        );
        assert!(pf.has_schedule_callback.load(AtomicOrdering::Relaxed));
        pf.has_schedule_callback.store(false, AtomicOrdering::Release);
        assert!(!pf.has_schedule_callback.load(AtomicOrdering::Relaxed));
    }

    fn stealth_result_for_test() -> StealthSyncResult {
        use compact_str::CompactString;
        use smallvec::smallvec;
        StealthSyncResult {
            status_code: 200,
            status_text: CompactString::new("OK"),
            headers: smallvec![],
            body: bytes::Bytes::from_static(b"hello"),
        }
    }
}
