// @trace REQ-ENG-004
use ::std::cell::{Cell, RefCell};
use ::std::time::Duration;

use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_DefineFunction;

use crate::gc_store::{gc_store_get_ns, gc_store_insert_ns, gc_store_remove_ns};

thread_local! {
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };

    /// Per-thread MiniEventLoop holder. Lazily materialized on first access
    /// via `with_event_loop` (leaked — never freed; OS reclaims on exit).
    ///
    /// BCE-20260621-001 (root cause, design layer): previously a
    /// `RefCell<Option<ManuallyDrop<MiniEventLoop>>>`. `with_event_loop`
    /// `borrow_mut`-held the cell across the supplied closure, but the
    /// closure (e.g. `drain_and_check` → `tick_without_idle`) schedules
    /// tasks that re-enter `with_event_loop` (e.g. `resolve_tasklet` step 5
    /// `unref_concurrently`). The reentrant `borrow_mut` triggered
    /// `RefCell already borrowed` panic — observed as fetch hang when the
    /// server actually accepted (only with 127.0.0.1; with `localhost` the
    /// URL mis-resolved to 0.0.0.0 hid this layer).
    ///
    /// Fix: store a raw `*mut MiniEventLoop` in a `Cell` (Zig-style aliasable
    /// thread-local — same pattern as `MiniEventLoop::GLOBAL`). `Cell` has no
    /// borrow tracking; reentry from any path is sound as long as the loop
    /// is alive (thread-lifetime singleton). The pointer is leaked
    /// (`Box::into_raw`) on first init — the loop outlives `with_event_loop`
    /// callers by design (matches `event_loop::MiniEventLoop::init_global`
    /// + `bao_engine::NeverDrop`).
    ///
    /// SAFETY invariant: the pointer is null until first `with_event_loop`
    /// call, then non-null for the remainder of the thread's life. Callers
    /// reborrow `&mut *ptr` only for the scope they need (no aliased
    /// `&'static mut` materialized).
    static BAO_RUNTIME_LOOP: Cell<*mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>> =
        const { Cell::new(::std::ptr::null_mut()) };

    /// Thread-local pointer to the current `JSContext*`. Registered by
    /// `drain_and_check` before timer dispatch, so that `__bun_fire_timer`
    /// can recover the live cx and dispatch JS callbacks.
    static CURRENT_CX: Cell<*mut JSContext> = const { Cell::new(::std::ptr::null_mut()) };

    /// Per-thread BaoTimerRegistry — Bun-style intrusive-heap timers.
    /// Single source of truth for all timer registration, cancellation,
    /// and drain operations.
    static BAO_REGISTRY: RefCell<BaoTimerRegistry> = RefCell::new(BaoTimerRegistry::new());

    /// Monotonic epoch counter for stable heap ordering of equal-deadline
    /// timers. Mirrors Bun's `TimerObjectInternals.flags.epoch`.
    static NEXT_EPOCH: Cell<u32> = const { Cell::new(1) };

    /// BCE-20260619-001: ID of the timer whose JS callback is currently
    /// being dispatched by `drain_bao_timers`, or 0 if no timer is firing.
    ///
    /// The drain loop pops the timer from BAO_REGISTRY *before* firing its
    /// callback (pop-before-fire pattern for re-entrant safety). That means
    /// a `clearInterval(i)`/`clearTimeout(t)` invoked *inside* the callback
    /// finds the timer already absent from the registry → `remove(id)`
    /// returns None → cancel is a silent no-op. Without this signal, the
    /// drain loop then unconditionally re-arms interval timers, defeating
    /// the clear and pinning the process in the loop forever.
    ///
    /// Fix: while dispatching a timer's callback, set this slot to the
    /// timer's id; clear_xxx/cancel_raw check it and flip
    /// `CLEARED_DURING_FIRE` to true even when the registry lookup misses.
    /// The drain loop consults the flag after the callback returns and
    /// skips re-arm. Mirrors Bun's `eventLoopTimer().state == .CANCELLED`
    /// check (TimerObjectInternals.zig:133) — same root invariant, local
    /// encoding because bao's drain takes ownership of the Box before
    /// firing.
    static CURRENT_FIRING_TIMER: Cell<u32> = const { Cell::new(0) };

    /// BCE-20260619-001: latched "clear was called during the in-flight
    /// fire" flag. Reset to false by `drain_bao_timers` before each
    /// callback dispatch; set to true by `cancel_raw`/`clear_timeout`/
    /// `clear_interval` when they observe `CURRENT_FIRING_TIMER` matching
    /// the cleared id (covering both the "already popped, callback fired"
    /// and "still in registry, normal cancel" cases).
    static CLEARED_DURING_FIRE: Cell<bool> = const { Cell::new(false) };
}

/// P1-A.3b: register the current thread's `JSContext*` for retrieval by
/// MiniEventLoop-driven dispatch (notably `__bun_fire_timer`).
///
/// Must be called before any MiniEventLoop tick that may fire JS-bearing
/// timers. `drain_and_check` is the natural place — it already holds a
/// `&mut JSContext` and runs the dispatch loop.
///
/// # Safety
/// - `cx` must be a live `JSContext*` on the current thread.
/// - Caller must clear (pass null) before the JSContext is destroyed,
///   or be confident the thread is single-threaded and the cx outlives
///   any deferred timer dispatch.
pub unsafe fn register_current_cx(cx: *mut JSContext) {
    CURRENT_CX.with(|cell| cell.set(cx));
}

/// P1-A.3b: retrieve the currently-registered `JSContext*`, or null if
/// no JS context is active on this thread. Intended for use by FFI
/// dispatch (e.g. `__bun_fire_timer`) that lacks direct cx access.
#[inline]
pub fn current_cx() -> *mut JSContext {
    CURRENT_CX.with(|cell| cell.get())
}

/// P1-A.3c-step4: RAII guard that clears `CURRENT_CX` on drop. Used by
/// `drain_and_check` so that any return path (early-exit, panic unwinding)
/// restores a null cx — defensive against stale cx reads after the borrow
/// on `&mut JSContext` ends.
struct CxGuard;
impl CxGuard {
    fn new() -> Self {
        Self
    }
}
impl Drop for CxGuard {
    fn drop(&mut self) {
        // SAFETY: writing null is always sound — no lifetime concerns.
        unsafe {
            register_current_cx(::std::ptr::null_mut());
        }
    }
}

/// Access or lazily materialize the per-thread MiniEventLoop.
///
/// Materializes the loop on first call (`Box::into_raw` — leaked for the
/// thread's lifetime; OS reclaims on exit, same strategy as
/// `event_loop::MiniEventLoop::GLOBAL` and `bao_engine::NeverDrop`).
///
/// BCE-20260621-001: reentry-safe. The loop pointer is stored in a `Cell`
/// (no borrow tracking), so callers scheduled by `tick_without_idle` (e.g.
/// `resolve_tasklet`) may re-enter `with_event_loop` without panic.
///
/// # Safety (internal)
///
/// The closure receives `&mut MiniEventLoop` reborrowed from a raw pointer
/// that is valid for the thread's lifetime. The reborrow is sound as long
/// as no other `&mut` to the same `MiniEventLoop` is live in the same
/// stack frame — callers must not hold an outer `&mut` across the call
/// (they pass it through this fn instead).
pub fn with_event_loop<F, R>(f: F) -> R
where
    F: FnOnce(&mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>) -> R,
{
    BAO_RUNTIME_LOOP.with(|cell| {
        let mut ptr = cell.get();
        if ptr.is_null() {
            // Ensure bao_uloop's #[no_mangle] extern "C" symbols are
            // referenced — without this the linker may GC uSockets loop
            // entrypoints that MiniEventLoop::init reaches via UwsLoop::get().
            bao_uloop::force_link();
            let boxed =
                ::std::boxed::Box::new(bun_event_loop::MiniEventLoop::MiniEventLoop::init());
            ptr = bun_core::heap::into_raw(boxed);
            cell.set(ptr);
            // BCE-20260814-TLS-DRIVER-UAF: publish this loop as a live
            // cross-thread wakeup target. Registration happens after
            // `MiniEventLoop::init` materialized the C-owned uws loop
            // (UwsLoop::get → C++ thread-local), whose thread-exit
            // destructor therefore registered FIRST — LIFO runs this
            // module's deregister guard before the C/C++ library frees the
            // loop, the ordering the ConcurrentWakeup lock handshake
            // depends on.
            bun_event_loop::ConcurrentWakeup::register_thread_loop(ptr);
        }
        // SAFETY: `ptr` is either the previously-stored non-null pointer
        // (valid for the thread's lifetime per the BAO_RUNTIME_LOOP
        // invariant) or just-set from a fresh `Box::into_raw` above. In
        // both cases the backing `MiniEventLoop` is live for the duration
        // of this closure. The reborrow ends when `f` returns; no outer
        // `&mut` is held across the call (callers route through this fn).
        // Reentry from within `f` is sound: a nested `with_event_loop`
        // re-reads the same `Cell` pointer and constructs its own
        // short-lived `&mut` — the outer `&mut` is not held across the
        // nested call because `f` is the only live borrow at this level.
        let loop_: &mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static> =
            unsafe { &mut *ptr };
        f(loop_)
    })
}

pub fn init() {}

pub fn install_timer_globals(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    init();
    unsafe {
        JS_DefineFunction(
            cx,
            global,
            c"setTimeout".as_ptr(),
            ::std::option::Option::Some(set_timeout),
            2,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            global,
            c"clearTimeout".as_ptr(),
            ::std::option::Option::Some(clear_timeout),
            1,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            global,
            c"setInterval".as_ptr(),
            ::std::option::Option::Some(set_interval),
            2,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            global,
            c"clearInterval".as_ptr(),
            ::std::option::Option::Some(clear_interval),
            1,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            global,
            c"setImmediate".as_ptr(),
            ::std::option::Option::Some(set_immediate),
            1,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            global,
            c"clearImmediate".as_ptr(),
            ::std::option::Option::Some(clear_timeout),
            1,
            JSPROP_ENUMERATE as u32,
        );
    }
}

/// Deadline-aware wait for the timer-only branches of `drain_and_check` /
/// `drain_one_pass`. Replaces the former `sleep(1ms)` busy-poll that burned
/// CPU advancing the wall clock toward the next BAO_REGISTRY deadline.
///
/// Model: compute the delta from *now* to the earliest deadline and perform
/// ONE bounded, interruptible wait via the thread's uWS loop
/// (`tick_with_timeout` → `bao_loop_tick` → `epoll_wait(timeout=delta)`):
///
/// - Pure-timer case (loop `active == 0`): epoll blocks the full delta —
///   zero polling; the timer wakes exactly at its deadline.
/// - Interruptible: a ConcurrentTask completion on any thread calls
///   `us_wakeup_loop`, whose eventfd is registered on the same epoll — the
///   wait returns immediately instead of sleeping out the slice.
/// - Busy-loop clamp (`active > 0`): `bao_loop_tick` clamps the timeout to 0
///   (non-blocking drain, same policy as `tick_without_idle`); the residual
///   sleep below then paces the iteration at the old 1ms cadence —
///   behavior-equivalent to the previous poll.
/// - Fake timers: a mocked clock does not advance while blocked in epoll
///   (only JS running between iterations advances it via `mock_time::set`),
///   so keep the legacy 1ms poll when a mock is installed.
fn wait_for_timer_deadline() {
    if bun_core::util::mock_time::get().is_some() {
        // Mocked clock: waiting cannot advance it. Legacy 1ms poll.
        ::std::thread::sleep(Duration::from_millis(1));
        return;
    }
    let now = Timespec::now_allow_mocked_time();
    let Some(deadline) = BAO_REGISTRY.with(|r| r.borrow().next_deadline()) else {
        return;
    };
    if deadline.order(&now) != core::cmp::Ordering::Greater {
        // Already due — let drain_bao_timers fire it without waiting.
        return;
    }
    let delta = deadline.duration(&now);
    let delta_ns = delta.ns() as i64;
    let start = ::std::time::Instant::now();
    with_event_loop(|loop_| {
        // SAFETY: `loop_ptr()` returns the live thread-lifetime uWS loop
        // (see `with_event_loop`'s invariant). `tick_with_timeout` performs
        // a bounded epoll_wait — never the NULL-timeout blocking path that
        // caused BCE-007-R3.
        unsafe { (*loop_.loop_ptr()).tick_with_timeout(Some(&delta)) };
    });
    let elapsed_ns = start.elapsed().as_nanos() as i64;
    if elapsed_ns < delta_ns {
        // Epoll returned before the deadline: either its ms-truncation
        // (<1ms gap), a wakeup/event arrival, or the active>0 zero-clamp.
        // Sleep at most the old 1ms quantum — never the full remainder, an
        // arrived event must not wait out a long timer — so iterations keep
        // advancing the wall clock exactly like the previous poll did.
        let residual = (delta_ns - elapsed_ns).min(1_000_000);
        ::std::thread::sleep(Duration::from_nanos(residual as u64));
    }
}

/// P1-B: main event-loop tick. Uses MiniEventLoop::tick_once for I/O
/// (bao_uloop epoll drives uWS App sockets). Drains BAO_REGISTRY timers
/// and fires their JS callbacks. Returns `true` if the event loop should
/// continue (has pending timers or active HTTP servers).
pub fn drain_and_check(cx: &mut mozjs::context::JSContext) -> bool {
    // If process.exit() / Bun.exit() was called, stop the event loop.
    // The CLI main will pick up the exit code and exit orderly.
    if crate::should_exit() {
        return false;
    }

    // SAFETY: cx is a live &mut JSContext on the current thread; the guard
    // clears it on drop so subsequent code on this thread sees null.
    unsafe {
        register_current_cx(cx.raw_cx());
    }
    let _cx_guard = CxGuard::new();

    // @trace BCE-20260618-003 [level:regression]
    // BCE-007-R3 (root cause): `MiniEventLoop::tick_once` calls
    // `UwsLoop::tick()` → `us_loop_run_bun_tick(loop, NULL)` → blocking
    // `epoll_pwait2` (libusockets epoll_kqueue.c:391, `timeout=NULL` →
    // `will_idle_inside_event_loop=1` → kernel blocks until a ready fd).
    // After the JS-thread Bun.serve dispatches the request (one accept +
    // recv + send cycle), there are no more ready fds; the worker that
    // completes the fetch in the background cannot wake the JS-thread
    // uWS Loop (no `us_wakeup_loop` cross-thread wake), so `tick_once`
    // blocks forever — `drain_pending_fetches` never runs and the fetch
    // Promise hangs.
    //
    // Fix: use `tick_without_idle` (zero timespec) instead of `tick_once`
    // (NULL timespec). With `timeout = {0,0}`, `will_idle_inside_event_loop`
    // is forced false and `epoll_pwait2` returns immediately after draining
    // ready fds. The eval-loop's `std::thread::sleep(1ms)` after this hook
    // yields the JS thread so I/O still progresses without busy-spinning.
    // Mirrors `drain_one_pass` (timer-only branch) which also avoids the
    // blocking tick path.
    let has_http_before_tick = crate::node_http::has_active_servers();
    let has_pending_before_tick = bao_has_pending_timers();
    let has_pending_async_fetch = crate::fetch_async::has_pending();
    let has_pending_async_fs_crypto = crate::node_fs::fs_async_pending() > 0
        || crate::node_crypto::crypto_async_pending() > 0;
    if has_http_before_tick {
        // I/O is active — let uSockets drain ready fds without blocking.
        with_event_loop(|loop_| {
            loop_.tick_without_idle(core::ptr::null_mut());
        });
    } else if has_pending_before_tick {
        // Timer-only case: bao's setTimeout lives in BAO_REGISTRY (wall-clock
        // deadlines), not in uSockets' timer heap. Wait exactly until the
        // earliest deadline (interruptible by ConcurrentTask wakeups) instead
        // of polling — see `wait_for_timer_deadline`.
        wait_for_timer_deadline();
    } else if has_pending_async_fs_crypto {
        // fs/crypto-only case (A' route): with no timers and no HTTP nothing
        // above would tick the MiniEventLoop, so a completed worker's
        // ConcurrentTask would sit in the queue undelivered while the loop
        // kept spinning on the liveness verdict below. One non-blocking tick
        // per pass pops+runs any arrived tasklet (the cross-thread enqueue
        // already woke the uws loop's eventfd).
        with_event_loop(|loop_| {
            loop_.tick_without_idle(core::ptr::null_mut());
        });
    }
    // BCE-20260619-010: fetch-only busy-poll branch removed. FetchTasklet
    // event-driven paradigm uses ConcurrentTask auto-wakeup via MiniEventLoop;
    // no sleep(1ms) polling is needed.

    let has_http = crate::node_http::has_active_servers();
    let raw_cx = unsafe { cx.raw_cx() };
    drain_bao_timers(raw_cx);
    bao_engine::job_queue::JobQueue::drain(cx);

    // Page WebSocket pump: consume completed background connects
    // (onopen/onerror) and drain inbound frames (onmessage/onclose).
    // Non-blocking — sockets are polled in non-blocking mode.
    crate::web_api::ws_pump_all(raw_cx);

    // fs.watch / fs.watchFile pump: dispatch inotify + stat-poll events queued
    // by the fswatch worker thread (same integration pattern as ws_pump_all).
    crate::node_fs::fs_watch_pump_all(raw_cx);
    // cluster IPC pump: primary/worker message + exit polling (replaces the
    // loop-pinning CLUSTER_JS setIntervals — BCE parent-loop stall).
    crate::node_cluster::cluster_pump_all(raw_cx);
    // Bun.spawn exit watchers: dispatch finish (exit/stdio/close events)
    // once the child's pump publishes (reaped + pipes drained). Same native
    // drain-pump pattern as the cluster pump — no JS-timer chain.
    crate::bun_api::spawn_watch_pump_all(raw_cx);

    // BCE (await proc.exited hang): every pump above runs AFTER the
    // JobQueue::drain at the head of this tick, so promise continuations
    // resolved inside a pump (e.g. `exited` resolved by spawn_watch_pump_all)
    // had nobody to run them in this pass — and with no timer/http pending
    // the loop-alive verdict below was false, so the eval loop broke and the
    // parked top-level await never resumed (silent exit 0). Drain the job
    // queue once more after the pumps: pump-resolved continuations run in
    // the SAME pass, and whatever they await next sees a consistent tick.
    bao_engine::job_queue::JobQueue::drain(cx);

    // BCE-20260619-010: drain_pending_fetches removed. FetchTasklet event-driven
    // paradigm resolves promises via ConcurrentTask (resolve_tasklet), not via
    // JS-thread polling. No drain call needed here.

    // Liveness must be evaluated with FRESH state: callbacks drained above
    // (bao timers, SM jobs, the pumps) can START work — a fetch begun inside
    // a timer callback, a server begun inside a promise, …. The entry
    // snapshots (`has_pending_async_fetch`) and the mid-fn `has_http` read
    // (taken before `drain_bao_timers`) predate those registrations and made
    // the tail all-false while a fetch was in flight → the eval loop broke,
    // exit handlers fired, and the in-flight request was torn down (the
    // "timer-context fetch never completes" wedge; keep-alive intervals
    // masked it). Re-read every source here.
    bao_has_pending_timers()
        || crate::node_http::has_active_servers()
        || crate::fetch_async::has_pending()
        || crate::web_api::ws_has_pending()
        // In-flight fs/crypto async ops keep the loop alive across the
        // worker window (A' route): the completion ConcurrentTask arrives on
        // the pump's MiniEventLoop and must be drained before exit.
        || crate::node_fs::fs_async_pending() > 0
        || crate::node_crypto::crypto_async_pending() > 0
        // Persistent fs watchers / live cluster workers keep the loop alive
        // (Node handle semantics; non-persistent entries never pin).
        || crate::node_fs::fs_watch_loop_alive()
        || crate::node_cluster::cluster_loop_alive()
        // Live Bun.spawn watchers pin the loop (Node active-handle
        // semantics) — a pending `await proc.exited` must outlive a child
        // that has not been reaped yet, even with zero timers registered.
        || crate::bun_api::spawn_watch_loop_alive()
}

/// Drive a single "wait for promise / timer" iteration in test-runner mode.
///
/// Fires all due `setTimeout`/`setInterval` callbacks, then drains SM's
/// job queue (microtasks + queued promise jobs). Unlike `drain_and_check`,
/// this entry point takes a raw `JSContext*` — it's used by the bun:test
/// runner's async-await support, where the JSContext is held as a raw
/// pointer (see `bun_test::drain_until_done`).
///
/// Test-runner semantics:
///   - When there are active HTTP servers / I/O, `tick_once` is invoked so
///     uSockets-driven I/O (fetch, http.createServer) makes progress.
///   - When only timers are pending (typical setTimeout/async test), we
///     wait until the earliest deadline instead. bao's `setTimeout`
///     registers in `BAO_REGISTRY` (not in uSockets' timer heap), so an
///     empty `tick_once` would block forever on `epoll_wait` with nothing
///     to wake it. `drain_bao_timers` uses wall-clock deadlines;
///     `wait_for_timer_deadline` performs one bounded, wakeup-interruptible
///     `epoll_wait(delta-to-deadline)` — no polling.
///
/// Returns true if any timer fired this pass.
///
/// # Safety
/// - `raw_cx` must be a live `JSContext*` on the current thread with an
///   active request.
/// - Caller must hold a root on the global object (standard globals are
///   rooted by the runtime).
pub unsafe fn drain_one_pass(raw_cx: *mut JSContext) -> bool {
    if crate::should_exit() {
        return false;
    }
    // SAFETY: caller guarantees raw_cx is live and on this thread.
    unsafe {
        register_current_cx(raw_cx);
    }
    let _cx_guard = CxGuard::new();

    let has_http = crate::node_http::has_active_servers();
    if has_http {
        // BCE-007-R3: use the zero-timespec non-blocking tick so the test
        // runner (which has no eval-loop sleep to yield) makes progress
        // without hanging on epoll_pwait2 when no fds are ready. Mirrors
        // `drain_and_check`'s has_http branch.
        with_event_loop(|loop_| {
            loop_.tick_without_idle(core::ptr::null_mut());
        });
    } else if bao_has_pending_timers() {
        // Timer-only case: wait exactly until the earliest deadline
        // (interruptible by ConcurrentTask wakeups) instead of the former
        // 1ms poll — see `wait_for_timer_deadline`.
        wait_for_timer_deadline();
    } else if crate::node_fs::fs_async_pending() > 0
        || crate::node_crypto::crypto_async_pending() > 0
    {
        // fs/crypto-only case (A' route): drain any arrived completion
        // tasklet — without this branch nothing in this pass ticks the
        // MiniEventLoop. Mirrors `drain_and_check`'s same-named branch.
        with_event_loop(|loop_| {
            loop_.tick_without_idle(core::ptr::null_mut());
        });
    }
    // BCE-20260619-010: fetch-only busy-poll branch removed.
    // FetchTasklet uses ConcurrentTask auto-wakeup; no sleep polling needed.
    let fired = drain_bao_timers(raw_cx);
    // Drain microtasks + queued promise jobs.
    mozjs_sys::jsapi::js::RunJobs(raw_cx);
    // BCE-20260619-010: drain_pending_fetches removed.
    // FetchTasklet resolves via ConcurrentTask; no drain call needed.
    // Page WebSocket pump (connect completion + inbound frames) — same
    // non-blocking poll as drain_and_check.
    crate::web_api::ws_pump_all(raw_cx);
    // fs.watch / cluster pumps — same dispatch points as drain_and_check so
    // test-runner async tests see watch/IPC events with identical semantics.
    crate::node_fs::fs_watch_pump_all(raw_cx);
    crate::node_cluster::cluster_pump_all(raw_cx);
    crate::bun_api::spawn_watch_pump_all(raw_cx);
    // BCE (await proc.exited hang): the RunJobs above ran BEFORE the pumps —
    // pump-resolved promise continuations (e.g. spawn's `exited`) would wait
    // a full extra pass. Drain once more so async tests resume in-pass,
    // mirroring drain_and_check's post-pump job drain.
    mozjs_sys::jsapi::js::RunJobs(raw_cx);
    fired
}

/// Check if there are any pending timers, HTTP servers, or async fetches.
/// Used by the test runner to know whether the loop should keep spinning.
// @trace REQ-ENG-010 [entity:FetchTasklet] — fetch_async::has_pending keeps the loop alive
pub fn has_pending_work() -> bool {
    bao_has_pending_timers()
        || crate::node_http::has_active_servers()
        || crate::fetch_async::has_pending()
        || crate::node_fs::fs_async_pending() > 0
        || crate::node_crypto::crypto_async_pending() > 0
        || crate::node_fs::fs_watch_loop_alive()
}

/// Fire a JS callback via `JS_CallFunctionValue`, routing any thrown
/// exception to the unified uncaught-exception router (handler dispatch or
/// stderr + exit 1). Used by `drain_bao_timers` and `BaoTimeoutObject::fire_js`
/// for JS callback dispatch.
///
/// # Safety
/// - `raw_cx` must be a live `JSContext*` on the current thread.
/// - `callback` must be a non-null live `JSObject*` (function object) rooted
///   by the caller for the duration of this call.
/// - `args` slice must point to `JSVal`s rooted by the caller.
pub unsafe fn fire_js_callback_raw(
    raw_cx: *mut JSContext,
    callback: *mut JSObject,
    args: &[JSVal],
) {
    unsafe {
        let global = CurrentGlobalOrNull(raw_cx);
        // Drain-time dispatch may run outside any entered realm (manual pump
        // callers, embedder drain hooks). Fall back to the thread's persistent
        // realm global (realm-per-context model) instead of silently dropping
        // the timer callback — same convention as node_http route dispatch.
        let global = if global.is_null() {
            match bao_engine::context::thread_realm_global() {
                ::std::option::Option::Some(g) if !g.is_null() => g,
                _ => return,
            }
        } else {
            global
        };

        let cx_ref =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx));
        rooted!(&in(cx_ref) let global_root = global);
        rooted!(&in(cx_ref) let fval_root = ObjectValue(callback));

        let args_array = if args.is_empty() {
            HandleValueArray::empty()
        } else {
            HandleValueArray {
                length_: args.len(),
                elements_: args.as_ptr(),
            }
        };

        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };

        let ok = JS_CallFunctionValue(
            raw_cx,
            global_root.handle().into(),
            fval_root.handle().into(),
            &args_array,
            rval_handle,
        );
        if !ok {
            // The timer callback threw. Capture the pending exception, clear
            // it, and route it (process.on('uncaughtException') or stderr +
            // exit 1) — Node semantics; NOT silently swallowed.
            let mut exn = UndefinedValue();
            JS_GetPendingException(
                raw_cx,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut exn,
                },
            );
            JS_ClearPendingException(raw_cx);
            rooted!(&in(cx_ref) let reason_root = exn);
            if !exn.is_undefined() {
                crate::uncaught::route_uncaught_exception(raw_cx, exn);
            }
        }
    }
}

/// P1-A.3d: check if BAO_REGISTRY has any pending timers.
/// This is the new source of truth (replaces TIMERS-based check).
pub fn has_pending_timers() -> bool {
    BAO_REGISTRY.with(|r| !r.borrow().is_empty())
}

/// Alias for has_pending_timers (used by drain_and_check).
fn bao_has_pending_timers() -> bool {
    has_pending_timers()
}

/// P1-A.3d: drain ready timers from BAO_REGISTRY (new source of truth).
///
/// Pops expired timers from the intrusive heap, fires their JS callbacks,
/// and re-arms interval timers. Uses "pop-before-fire" pattern to ensure
/// re-entrant setTimeout/clearTimeout safety: no RefCell borrow is held
/// across JS callback dispatch.
fn drain_bao_timers(raw_cx: *mut JSContext) -> bool {
    let now_ts = Timespec::now_allow_mocked_time();
    let mut fired = false;

    loop {
        // Check if earliest deadline has expired (separate borrow scope).
        let should_fire = BAO_REGISTRY.with(|r| {
            let reg = r.borrow();
            match reg.next_deadline() {
                Some(dl) => dl.order(&now_ts) != core::cmp::Ordering::Greater,
                None => false,
            }
        });
        if !should_fire {
            break;
        }

        // Pop the timer from BAO_REGISTRY — takes Box ownership.
        // No borrow is held after this scope closes.
        let obj_box = BAO_REGISTRY.with(|r| {
            let mut reg = r.borrow_mut();
            let peeked = reg.heap.peek();
            if peeked.is_null() {
                return None;
            }
            // SAFETY: peeked is non-null and points to a BaoTimeoutObject's
            // event_loop_timer field (the only type we insert into this heap).
            let timeout = unsafe { BaoTimeoutObject::from_timer_ptr(peeked) };
            let id = unsafe { (*timeout).timer_id };
            reg.remove(id)
        });

        let Some(mut obj) = obj_box else {
            break;
        };
        fired = true;

        // BCE-20260619-001: advertise the firing id + reset the clear flag
        // before invoking JS so a re-entrant clearInterval/clearTimeout can
        // signal cancellation even though the timer is no longer in
        // BAO_REGISTRY (we popped it above). Cleared on the next loop iter.
        let firing_id = obj.timer_id;
        CURRENT_FIRING_TIMER.with(|c| c.set(firing_id));
        CLEARED_DURING_FIRE.with(|c| c.set(false));

        // Fire JS callback — no BAO_REGISTRY borrow held.
        // SAFETY: raw_cx is a live JSContext* registered by drain_and_check.
        unsafe {
            obj.fire_js(raw_cx, &now_ts);
        }

        // BCE-20260619-001: stop advertising the firing id before reading
        // the flag — any later cancel on a different id must not see this id.
        CURRENT_FIRING_TIMER.with(|c| c.set(0));
        let cleared_during_fire = CLEARED_DURING_FIRE.with(|c| c.get());

        // If interval and the callback did NOT call clearInterval on us,
        // re-arm with updated deadline and re-insert. Matching Bun's
        // `TimerObjectInternals.fire` (TimerObjectInternals.zig:204-225):
        // state==FIRED → reschedule; state==CANCELLED → drop (is_timer_done).
        if obj.interval.is_some() && !cleared_during_fire {
            let interval = obj.interval.expect("checked Some above");
            let interval_ms = interval.as_millis() as i64;
            let mut next_ts = obj.event_loop_timer.next;
            while next_ts.order(&now_ts) != core::cmp::Ordering::Greater {
                next_ts = next_ts.add_ms(interval_ms);
            }
            obj.event_loop_timer.next = next_ts;
            obj.event_loop_timer.state = TimerState::PENDING;
            obj.epoch = obj.epoch.wrapping_add(1);
            BAO_REGISTRY.with(|r| r.borrow_mut().insert(obj));
        } else {
            // One-shot OR interval cleared during fire: cleanup GcStore
            // before Box is dropped. Mirrors Bun's is_timer_done branch
            // which calls setEnableKeepingEventLoopAlive(vm, false).
            obj.cleanup_callback(raw_cx);
            // Also latch the CANCELLED state for parity with Bun's state
            // machine — defensive: any later re-entrant cancel on this id
            // observes CANCELLED rather than FIRED.
            obj.event_loop_timer.state = TimerState::CANCELLED;
        }
    }

    // Defensive: clear the firing slot in case we broke out of the loop
    // without going through the post-fire reset (e.g. peek-then-miss).
    CURRENT_FIRING_TIMER.with(|c| c.set(0));

    fired
}

pub fn schedule_raw(
    cx: *mut JSContext,
    callback: *mut JSObject,
    delay_ms: u64,
    repeating: bool,
    _args: &[JSVal],
) -> u32 {
    let id = NEXT_ID.with(|n| {
        let val = n.get();
        n.set(val + 1);
        val
    });

    let interval = if repeating {
        Some(Duration::from_millis(delay_ms.max(1)))
    } else {
        None
    };

    let effective_delay = if delay_ms == 0 && repeating {
        1
    } else {
        delay_ms
    };

    // GC-safe: store callback in GcStore, keep only the string key.
    // Guard: test code may pass null cx; skip GC registration in that case.
    let callback_key = format!("cb_{}", id);
    if !cx.is_null() {
        gc_store_insert_ns(cx, "timer", &callback_key, callback);
    }

    let mut bao_obj = Box::new(BaoTimeoutObject::new_paused());
    bao_obj.timer_id = id;
    bao_obj.event_loop_timer.next =
        bun_core::Timespec::now_allow_mocked_time().add_ms(effective_delay as i64);
    bao_obj.interval = interval;
    bao_obj.callback_key = ::std::option::Option::Some(callback_key);
    bao_obj.args = _args.to_vec();
    bao_obj.epoch = NEXT_EPOCH.with(|c| {
        let v = c.get();
        c.set(v.wrapping_add(1));
        v
    });
    BAO_REGISTRY.with(|r| r.borrow_mut().insert(bao_obj));

    id
}

pub fn cancel_raw(id: u32) {
    BAO_REGISTRY.with(|r| {
        if let ::std::option::Option::Some(obj) = r.borrow_mut().remove(id) {
            // GC-safe: remove callback from GcStore if cx is available.
            let cx = current_cx();
            if !cx.is_null() {
                obj.cleanup_callback(cx);
            }
            return;
        }
    });
    // BCE-20260619-001: timer is not in BAO_REGISTRY. If it's the one
    // currently firing (we popped it before the JS callback in
    // `drain_bao_timers`), latch the "clear was called" signal so the
    // post-callback re-arm check sees it and drops the timer instead of
    // resurrecting it. Without this, clearInterval inside an interval
    // callback is a silent no-op and the process never exits.
    let firing = CURRENT_FIRING_TIMER.with(|c| c.get());
    if firing != 0 && firing == id {
        CLEARED_DURING_FIRE.with(|c| c.set(true));
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn set_timeout(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    register_timer(cx, argc, vp, false)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn set_interval(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    register_timer(cx, argc, vp, true)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn clear_timeout(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            let id = v.to_int32() as u32;
            let removed = BAO_REGISTRY.with(|r| {
                r.borrow_mut().remove(id).map(|obj| {
                    obj.cleanup_callback(cx);
                })
            });
            // BCE-20260619-001: if the lookup missed, this may be the
            // currently-firing timer (popped before its JS callback ran).
            // Latch the clear-signal so drain_bao_timers skips re-arm.
            if removed.is_none() {
                let firing = CURRENT_FIRING_TIMER.with(|c| c.get());
                if firing != 0 && firing == id {
                    CLEARED_DURING_FIRE.with(|c| c.set(true));
                }
            }
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn clear_interval(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    clear_timeout(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn set_immediate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }
    let cb_val = *args.get(0).ptr;
    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let cb = cb_val.to_object());
    let cb_args = if argc > 1 {
        (1..argc).map(|i| *args.get(i).ptr).collect()
    } else {
        Vec::new()
    };
    let id = schedule_raw(cx, cb.get(), 0, false, &cb_args);
    args.rval().set(Int32Value(id as i32));
    true
}

/// Parse JS args from setTimeout/setInterval and delegate to `schedule_raw`
/// for dual-write registration. Single source of truth for TimerEntry +
/// BaoTimeoutObject construction, deadline calculation, and epoch bump.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn register_timer(_cx: *mut JSContext, argc: u32, vp: *mut JSVal, repeating: bool) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if argc == 0 {
        args.rval().set(Int32Value(0));
        return true;
    }

    let first = *args.get(0).ptr;
    if !first.is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }

    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(_cx));
    rooted!(&in(cx_ref) let callback = first.to_object());

    let delay_ms = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            v.to_int32().max(0) as u64
        } else if v.is_double() {
            v.to_double().max(0.0) as u64
        } else {
            0
        }
    } else {
        0
    };

    let extra_args: Vec<JSVal> = if argc > 2 {
        (2..argc).map(|i| *args.get(i).ptr).collect()
    } else {
        Vec::new()
    };

    let id = schedule_raw(_cx, callback.get(), delay_ms, repeating, &extra_args);
    args.rval().set(Int32Value(id as i32));
    true
}

// ──────────────────────────────────────────────────────────────────────────
// P1-A.2: BaoTimeoutObject — SpiderMonkey equivalent of Bun's TimeoutObject.
//
// Carries an `EventLoopTimer` (the timer heap node) plus the JS-timer epoch
// used for stable ordering of equal-deadline timers.
//
// `from_timer_ptr` is the `container_of` recovery — given a pointer to the
// `event_loop_timer` field (what bun_event_loop's heap stores), recover the
// parent `BaoTimeoutObject`. This is the FFI contract that
// `dispatch::__bun_fire_timer` / `__bun_js_timer_epoch` fulfill.
//
// Until P1-A.3 wires drain_and_check through MiniEventLoop's timer heap,
// nothing in bao_runtime actually allocates BaoTimeoutObject — but the type
// + dispatch module must be linkable so bun_event_loop's EventLoopTimer
// externs resolve. The unit test below validates the container_of roundtrip.
// ──────────────────────────────────────────────────────────────────────────

use bun_core::Timespec;
use bun_event_loop::EventLoopTimer::{EventLoopTimer, State as TimerState};

#[repr(C)]
pub struct BaoTimeoutObject {
    /// MUST be at offset 0 — `dispatch::__bun_fire_timer`/`__bun_js_timer_epoch`
    /// recover `*mut BaoTimeoutObject` from this field's address via
    /// `from_timer_ptr` (container_of via `offset_of!`).
    pub event_loop_timer: EventLoopTimer,
    /// Monotonic epoch for stable heap ordering of equal-deadline JS timers.
    /// Mirrors Bun's `TimerObjectInternals.flags.epoch` (u25 in Zig).
    pub epoch: u32,
    /// GC-safe: GcStore key for the JS callback. None = virgin state.
    /// The actual `*mut JSObject` is stored as a property on the JS global
    /// via `gc_store_insert_ns(cx, "timer", &key, ..)`; this field only
    /// holds the string key. Retrieval via
    /// `gc_store_get_ns(cx, "timer", key)` is GC-safe (the property is
    /// rooted on the global object, managed by SpiderMonkey's GC).
    /// (Doc-link residue of the BUG-ENG-360 migration: these lines still
    /// named the un-namespaced pre-migration accessors.)
    pub callback_key: ::std::option::Option<String>,
    /// Marshalled JS arguments preserved across the schedule→fire window.
    pub args: ::std::vec::Vec<JSVal>,
    /// P1-A.3c: re-arm interval for setInterval. None = one-shot setTimeout.
    /// Set when scheduled via `setInterval`; consumed by drain logic which
    /// re-inserts the timer after firing. Mirrors Bun's
    /// `TimerObjectInternals.flags.repeat` (Duration in ms).
    pub interval: ::std::option::Option<Duration>,
    /// P1-A.3c: unique timer id used for clearTimeout/clearInterval lookups.
    /// The JS-visible id (setTimeout return value) — same value as `id` in
    /// the legacy `TimerEntry`. Stored on the object so cancel_raw can
    /// identify the right BaoTimeoutObject when clearing.
    pub timer_id: u32,
}

impl BaoTimeoutObject {
    /// Construct a paused (PENDING state) timeout with epoch 0, no callback.
    pub fn new_paused() -> Self {
        Self {
            event_loop_timer: EventLoopTimer::init_paused(
                bun_event_loop::EventLoopTimer::Tag::TimeoutObject,
            ),
            epoch: 0,
            callback_key: ::std::option::Option::None,
            args: ::std::vec::Vec::new(),
            interval: ::std::option::Option::None,
            timer_id: 0,
        }
    }

    /// Container-of: recover the parent `BaoTimeoutObject` from a pointer to
    /// its `event_loop_timer` field. This is the inverse of
    /// `&obj.event_loop_timer as *mut _`.
    ///
    /// # Safety
    /// `t` must be a non-null pointer to the `event_loop_timer` field of a
    /// live `BaoTimeoutObject`. Caller must not hold a `&mut` to the parent
    /// across this call (re-entrant JS callbacks can re-derive aliasing
    /// `&mut`, same as Bun's TimeoutObject::fire pattern).
    pub unsafe fn from_timer_ptr(t: *mut EventLoopTimer) -> *mut Self {
        let offset = core::mem::offset_of!(Self, event_loop_timer);
        (t as *mut u8).wrapping_sub(offset) as *mut Self
    }

    /// Mark this timer as fired and bump epoch for stable re-queue ordering.
    /// Pure state transition — does NOT dispatch the JS callback.
    /// Callers that need JS dispatch must use `fire_js` after retrieving the
    /// current `JSContext*`.
    pub fn fire(&mut self, _now: &Timespec) {
        self.event_loop_timer.state = TimerState::FIRED;
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Mark fired AND dispatch the JS callback via `fire_js_callback_raw`.
    /// This is the SpiderMonkey equivalent of Bun's `TimeoutObject::fire`.
    /// No-op if no callback key is attached (defensive — schedule path always
    /// sets one). Callback throws route through the uncaught-exception
    /// router (handler dispatch or stderr + exit 1) — never silently
    /// swallowed.
    ///
    /// # Safety
    /// - `raw_cx` must be a live `JSContext*` on the current thread.
    /// - The callback retrieved from GcStore (if Some) must be a live function
    ///   object rooted on the JS global for the duration of this call.
    /// - `self.args` slice must point to `JSVal`s rooted by the caller.
    pub unsafe fn fire_js(&mut self, raw_cx: *mut JSContext, now: &Timespec) {
        self.fire(now);
        if let ::std::option::Option::Some(ref key) = self.callback_key {
            if let ::std::option::Option::Some(cb) = gc_store_get_ns(raw_cx, "timer", key) {
                if !cb.is_null() {
                    unsafe { fire_js_callback_raw(raw_cx, cb, &self.args) };
                }
            }
        }
    }

    /// Remove the callback from GcStore. Call on cancel/remove.
    fn cleanup_callback(&self, cx: *mut JSContext) {
        if let ::std::option::Option::Some(ref key) = self.callback_key {
            gc_store_remove_ns(cx, "timer", key);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// P1-A.3c: BaoTimerRegistry — SpiderMonkey equivalent of Bun's `timer::All`.
//
// Reuses `bun_io::heap::Intrusive<EventLoopTimer, BaoTimerHeapCtx>` for the
// pairing-heap algorithm. Owns `Box<BaoTimeoutObject>` in a HashMap keyed by
// `timer_id` so clearInterval/clearTimeout can locate the node and the
// intrusive heap's `remove` can recover the parent via `HeapNode::heap`.
//
// Per CLAUDE.md「去锁化」 — single-thread JS model → RefCell, no Mutex.
// ──────────────────────────────────────────────────────────────────────────

/// ZST heap context that delegates to `EventLoopTimer::less` (Bun's
/// comparator). Same comparator as Bun's `TimerHeapCtx` in
/// `runtime/timer/mod.rs:314`.
#[derive(::std::default::Default)]
pub struct BaoTimerHeapCtx;

impl bun_io::heap::HeapContext<EventLoopTimer> for BaoTimerHeapCtx {
    /// # Safety
    /// `a`/`b` must be live intrusive heap nodes (caller invariant per
    /// `HeapContext` contract).
    unsafe fn less(&self, a: *mut EventLoopTimer, b: *mut EventLoopTimer) -> bool {
        // SAFETY: caller guarantees both pointers are live and aligned.
        // EventLoopTimer::less is a pure comparison — it reads `next.sec`,
        // `next.nsec`, dispatches `__bun_js_timer_epoch` for stable
        // tie-breaking. No mutation, no JS recursion risk.
        unsafe { EventLoopTimer::less((), &*a, &*b) }
    }
}

/// Type alias for the Bun-derived intrusive heap of EventLoopTimer nodes.
pub type BaoTimerHeap = bun_io::heap::Intrusive<EventLoopTimer, BaoTimerHeapCtx>;

/// P1-A.3c-step2: BaoTimerRegistry — the SpiderMonkey-side equivalent of
/// Bun's `runtime::timer::All`. Owns `Box<BaoTimeoutObject>` nodes keyed by
/// `timer_id` and tracks them in an intrusive pairing-heap of their
/// `EventLoopTimer` slots. Single-thread (JS model) → no Mutex.
///
/// Lifetime note: stored in a thread_local `RefCell`, so any re-entrant
/// schedule/cancel/drain must avoid holding `&mut` across JS callback
/// dispatch — same pattern as Bun's `drain_timers` (drops the borrow before
/// firing).
pub struct BaoTimerRegistry {
    heap: BaoTimerHeap,
    /// Owns the BaoTimeoutObject memory; the intrusive heap reaches into
    /// each box's `event_loop_timer` field via raw pointer.
    owned: ::std::collections::HashMap<u32, ::std::boxed::Box<BaoTimeoutObject>>,
}

#[allow(clippy::derivable_impls)]
impl ::std::default::Default for BaoTimerRegistry {
    fn default() -> Self {
        Self {
            heap: ::std::default::Default::default(),
            owned: ::std::default::Default::default(),
        }
    }
}

impl BaoTimerRegistry {
    pub fn new() -> Self {
        ::std::default::Default::default()
    }

    /// Number of timers currently held (oneshot + interval combined).
    pub fn len(&self) -> usize {
        self.owned.len()
    }

    pub fn is_empty(&self) -> bool {
        self.owned.is_empty()
    }

    /// Take ownership of `obj`, register it under `obj.timer_id`, and push
    /// its `event_loop_timer` into the intrusive heap. Returns the timer_id.
    ///
    /// # Panics
    /// Panics if `timer_id` is already registered (caller must cancel first).
    pub fn insert(&mut self, mut obj: ::std::boxed::Box<BaoTimeoutObject>) -> u32 {
        let id = obj.timer_id;
        assert!(
            !self.owned.contains_key(&id),
            "duplicate timer_id {id} in BaoTimerRegistry"
        );
        // SAFETY: Box owns a heap allocation; we never move the Box while
        // it's in the heap. Pointer stays valid until remove() pulls it out.
        let timer_ptr: *mut EventLoopTimer = &mut obj.event_loop_timer;
        unsafe {
            self.heap.insert(timer_ptr);
        }
        self.owned.insert(id, obj);
        id
    }

    /// Remove timer `id` from both the heap and the owned map. Returns
    /// ownership of the removed `BaoTimeoutObject`, or `None` if not found.
    pub fn remove(
        &mut self,
        id: u32,
    ) -> ::std::option::Option<::std::boxed::Box<BaoTimeoutObject>> {
        let mut obj = self.owned.remove(&id)?;
        let timer_ptr: *mut EventLoopTimer = &mut obj.event_loop_timer;
        // SAFETY: timer_ptr was inserted into self.heap by `insert`; the
        // node is still live (we hold the Box). remove() detaches it from
        // the heap without touching any other node.
        unsafe {
            self.heap.remove(timer_ptr);
        }
        ::std::option::Option::Some(obj)
    }

    /// Peek the earliest deadline in the heap (Bun's `get_timeout` analog).
    pub fn next_deadline(&self) -> ::std::option::Option<bun_core::Timespec> {
        let ptr = self.heap.peek();
        if ptr.is_null() {
            return ::std::option::Option::None;
        }
        // SAFETY: peek returns null or a valid timer pointer that's still
        // owned by some box in self.owned; we only read `next`.
        ::std::option::Option::Some(unsafe { (*ptr).next })
    }
}

#[cfg(test)]
mod bao_timeout_tests {
    use super::*;

    #[test]
    fn bao_timeout_object_offset_zero() {
        // `event_loop_timer` MUST be at offset 0 — dispatch.rs's container_of
        // depends on this invariant. Break this test if the layout changes.
        let obj = BaoTimeoutObject::new_paused();
        let base = &obj as *const _ as usize;
        let timer = &obj.event_loop_timer as *const _ as usize;
        assert_eq!(timer - base, 0, "event_loop_timer must be at offset 0");
    }

    #[test]
    fn bao_timeout_object_from_timer_ptr_roundtrip() {
        let obj = Box::new(BaoTimeoutObject::new_paused());
        let obj_ptr = Box::into_raw(obj);
        // SAFETY: obj_ptr is a live Box-derived pointer; addr_of_mut avoids
        // creating a `&mut` that would alias with the raw pointer we hand to
        // `from_timer_ptr`.
        let timer_ptr = unsafe { core::ptr::addr_of_mut!((*obj_ptr).event_loop_timer) };

        // SAFETY: timer_ptr is the event_loop_timer field of a live BaoTimeoutObject.
        let recovered = unsafe { BaoTimeoutObject::from_timer_ptr(timer_ptr) };
        assert_eq!(recovered, obj_ptr, "from_timer_ptr must recover the parent");

        // SAFETY: reclaim the Box to avoid leaking. from_timer_ptr did not
        // take ownership; we still own it via obj_ptr.
        unsafe {
            drop(Box::from_raw(obj_ptr));
        }
    }

    #[test]
    fn bao_timeout_object_fire_transitions_state() {
        let mut obj = BaoTimeoutObject::new_paused();
        assert!(
            obj.event_loop_timer.state == TimerState::PENDING,
            "initial state is PENDING"
        );
        assert_eq!(obj.epoch, 0);

        let now = Timespec {
            sec: 1_700_000_000,
            nsec: 0,
        };
        obj.fire(&now);

        assert!(
            obj.event_loop_timer.state == TimerState::FIRED,
            "fire transitions to FIRED"
        );
        assert_eq!(
            obj.epoch, 1,
            "fire bumps epoch for stable re-queue ordering"
        );
    }

    #[test]
    fn bao_timeout_object_tag_is_timeout_object() {
        let obj = BaoTimeoutObject::new_paused();
        assert!(
            obj.event_loop_timer.tag == bun_event_loop::EventLoopTimer::Tag::TimeoutObject,
            "tag must be TimeoutObject for FFI dispatch",
        );
    }

    #[test]
    fn bao_timeout_object_new_paused_has_no_callback() {
        let obj = BaoTimeoutObject::new_paused();
        assert!(
            obj.callback_key.is_none(),
            "new_paused must start with no callback_key"
        );
        assert!(obj.args.is_empty(), "new_paused must start with empty args");
    }

    #[test]
    fn bao_timeout_object_fire_js_no_callback_key_is_noop() {
        // fire_js with no callback_key must still transition state but skip
        // JS dispatch (defensive guard). No JSContext is touched.
        let mut obj = BaoTimeoutObject::new_paused();
        let now = Timespec {
            sec: 1_700_000_000,
            nsec: 0,
        };

        // SAFETY: passing null raw_cx is sound because the None callback_key
        // guard returns before any JSAPI call. We deliberately test the
        // defensive path — the production path requires a live cx.
        unsafe {
            obj.fire_js(::std::ptr::null_mut(), &now);
        }

        assert!(
            obj.event_loop_timer.state == TimerState::FIRED,
            "fire_js transitions state even with no callback_key"
        );
        assert_eq!(obj.epoch, 1, "fire_js bumps epoch");
    }

    #[test]
    fn bao_timeout_object_callback_key_field_roundtrip() {
        // Verify callback_key/args fields can be set and read back.
        let mut obj = BaoTimeoutObject::new_paused();
        obj.callback_key = ::std::option::Option::Some("cb_42".to_string());
        obj.args = vec![mozjs::jsval::Int32Value(42)];

        assert_eq!(
            obj.callback_key,
            ::std::option::Option::Some("cb_42".to_string()),
            "callback_key stores string"
        );
        assert_eq!(obj.args.len(), 1, "args field stores JSVal vec");
        assert_eq!(obj.args[0].to_int32(), 42, "JSVal roundtrips intact");
    }

    #[test]
    fn with_event_loop_lazily_materializes_mini_event_loop() {
        // P1-A.3a: verify the per-thread MiniEventLoop holder works.
        // Calling with_event_loop twice must yield the same underlying
        // loop pointer (lazy-init contract).
        let ptr1 = with_event_loop(|loop_| loop_.loop_ptr() as usize);
        let ptr2 = with_event_loop(|loop_| loop_.loop_ptr() as usize);
        assert!(ptr1 != 0, "MiniEventLoop loop_ptr must be non-null");
        assert_eq!(
            ptr1, ptr2,
            "with_event_loop must return the same loop on repeated calls"
        );
    }

    #[test]
    fn current_cx_roundtrip_via_thread_local() {
        // P1-A.3b: register_current_cx writes; current_cx reads back.
        // Sentinel pointer is never dereferenced — only validates storage.
        let sentinel: *mut JSContext = 0x12345678 as *mut JSContext;
        // SAFETY: sentinel is never dereferenced; we only validate the
        // thread_local round-trip. Cleared at end so subsequent tests see
        // null.
        unsafe {
            register_current_cx(sentinel);
        }
        assert_eq!(
            current_cx(),
            sentinel,
            "register_current_cx stores cx in thread_local"
        );

        // Clear back to null to avoid leaking sentinel into other tests.
        unsafe {
            register_current_cx(::std::ptr::null_mut());
        }
        assert!(
            current_cx().is_null(),
            "register_current_cx(null) clears the slot"
        );
    }

    #[test]
    fn bao_timeout_object_new_paused_has_no_interval() {
        // P1-A.3c: new_paused initial state for interval + timer_id fields.
        let obj = BaoTimeoutObject::new_paused();
        assert!(
            obj.interval.is_none(),
            "new_paused must start with no interval (one-shot)"
        );
        assert_eq!(obj.timer_id, 0, "new_paused must start with timer_id 0");
    }

    #[test]
    fn bao_timer_heap_ctx_default_compiles() {
        // P1-A.3c: BaoTimerHeapCtx is a ZST, must be default-constructible.
        let _ctx = BaoTimerHeapCtx::default();
        // Heap with default context — Intrusive::default requires Context: Default.
        let _heap: BaoTimerHeap = ::std::default::Default::default();
    }

    #[test]
    fn bao_timer_heap_insert_then_peek_orders_by_deadline() {
        // P1-A.3c: validate that BaoTimerHeap + BaoTimerHeapCtx orders
        // EventLoopTimers by their `next` Timespec using Bun's `less`.
        let mut earlier = Box::new(BaoTimeoutObject::new_paused());
        earlier.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        let mut later = Box::new(BaoTimeoutObject::new_paused());
        later.event_loop_timer.next = bun_core::Timespec { sec: 200, nsec: 0 };

        let earlier_ptr = (&mut earlier.event_loop_timer) as *mut EventLoopTimer;
        let later_ptr = (&mut later.event_loop_timer) as *mut EventLoopTimer;

        let mut heap: BaoTimerHeap = ::std::default::Default::default();
        // SAFETY: both pointers are live heap-allocated EventLoopTimer nodes
        // not currently in any other heap. They stay alive via the Box
        // guards until the end of the test.
        unsafe {
            heap.insert(later_ptr);
            heap.insert(earlier_ptr);
        }

        // peek returns the minimum (earliest deadline).
        let peeked = heap.peek();
        assert_eq!(
            peeked, earlier_ptr,
            "heap.peek must return earliest deadline (Bun's less ordering)"
        );

        // Cleanup: remove both nodes from the heap before dropping Boxes
        // so the intrusive field is not in a heap state when dropped.
        unsafe {
            let _ = heap.delete_min();
            let _ = heap.delete_min();
        }
        drop(earlier);
        drop(later);
    }

    #[test]
    fn bao_timer_registry_insert_and_len() {
        // P1-A.3c-step2: insert keeps ownership + heap membership consistent.
        let mut reg = BaoTimerRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);

        let mut obj = Box::new(BaoTimeoutObject::new_paused());
        obj.timer_id = 1;
        obj.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        let id = reg.insert(obj);
        assert_eq!(id, 1);
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
    }

    #[test]
    fn bao_timer_registry_next_deadline_returns_min() {
        // Insert 3 timers out of order; next_deadline returns earliest.
        let mut reg = BaoTimerRegistry::new();
        for (id, sec) in [(1, 300), (2, 100), (3, 200)] {
            let mut obj = Box::new(BaoTimeoutObject::new_paused());
            obj.timer_id = id;
            obj.event_loop_timer.next = bun_core::Timespec { sec, nsec: 0 };
            reg.insert(obj);
        }
        let dl = reg.next_deadline().expect("heap non-empty");
        assert_eq!(dl.sec, 100, "next_deadline must return earliest (sec=100)");
    }

    #[test]
    fn bao_timer_registry_remove_clears_ownership() {
        let mut reg = BaoTimerRegistry::new();
        let mut obj = Box::new(BaoTimeoutObject::new_paused());
        obj.timer_id = 42;
        obj.event_loop_timer.next = bun_core::Timespec { sec: 500, nsec: 0 };
        reg.insert(obj);
        assert_eq!(reg.len(), 1);

        let removed = reg.remove(42);
        assert!(
            removed.is_some(),
            "remove returns Some(Box<..>) for known id"
        );
        assert_eq!(reg.len(), 0);
        assert!(reg.is_empty());
        assert!(
            reg.next_deadline().is_none(),
            "heap must be empty after remove"
        );

        // Removing unknown id returns None.
        let again = reg.remove(42);
        assert!(again.is_none(), "remove returns None for unknown id");
    }

    // ──────────────────────────────────────────────────────────────────────────
    // Additional unit tests covering edge cases (20+ new tests)
    // ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn bao_timer_registry_insert_multiple_orders_by_deadline() {
        // Insert 5 timers with varying deadlines; verify heap ordering.
        let mut reg = BaoTimerRegistry::new();
        let deadlines = [(1, 500), (2, 100), (3, 300), (4, 50), (5, 200)];
        for (id, sec) in deadlines {
            let mut obj = Box::new(BaoTimeoutObject::new_paused());
            obj.timer_id = id;
            obj.event_loop_timer.next = bun_core::Timespec { sec, nsec: 0 };
            reg.insert(obj);
        }
        assert_eq!(reg.len(), 5);
        // Earliest deadline should be sec=50 (id=4)
        let dl = reg.next_deadline().expect("heap non-empty");
        assert_eq!(dl.sec, 50, "next_deadline must return earliest deadline");
    }

    #[test]
    fn bao_timer_registry_remove_middle_preserves_heap_order() {
        // Remove a middle timer; verify remaining heap still ordered.
        let mut reg = BaoTimerRegistry::new();
        for (id, sec) in [(1, 100), (2, 200), (3, 300), (4, 400)] {
            let mut obj = Box::new(BaoTimeoutObject::new_paused());
            obj.timer_id = id;
            obj.event_loop_timer.next = bun_core::Timespec { sec, nsec: 0 };
            reg.insert(obj);
        }
        // Remove id=2 (sec=200, middle of heap)
        let removed = reg.remove(2);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().timer_id, 2);
        assert_eq!(reg.len(), 3);
        // Earliest should still be sec=100
        let dl = reg.next_deadline().expect("heap non-empty");
        assert_eq!(dl.sec, 100);
    }

    #[test]
    fn bao_timer_registry_remove_earliest_updates_next_deadline() {
        // Remove the earliest timer; next_deadline should return the new earliest.
        let mut reg = BaoTimerRegistry::new();
        for (id, sec) in [(10, 1000), (20, 100), (30, 500)] {
            let mut obj = Box::new(BaoTimeoutObject::new_paused());
            obj.timer_id = id;
            obj.event_loop_timer.next = bun_core::Timespec { sec, nsec: 0 };
            reg.insert(obj);
        }
        // Remove id=20 (sec=100, the earliest)
        reg.remove(20);
        // New earliest should be sec=500 (id=30)
        let dl = reg.next_deadline().expect("heap non-empty");
        assert_eq!(dl.sec, 500);
    }

    #[test]
    fn bao_timer_registry_remove_all_makes_empty() {
        // Insert then remove all timers; registry should be empty.
        let mut reg = BaoTimerRegistry::new();
        let ids: Vec<u32> = (1..=10).collect();
        for &id in &ids {
            let mut obj = Box::new(BaoTimeoutObject::new_paused());
            obj.timer_id = id;
            obj.event_loop_timer.next = bun_core::Timespec {
                sec: id as i64 * 100,
                nsec: 0,
            };
            reg.insert(obj);
        }
        assert_eq!(reg.len(), 10);
        for &id in &ids {
            let removed = reg.remove(id);
            assert!(removed.is_some(), "remove({id}) should succeed");
        }
        assert!(reg.is_empty());
        assert!(reg.next_deadline().is_none());
    }

    #[test]
    fn bao_timer_registry_insert_duplicate_panics() {
        // Inserting a timer with duplicate timer_id should panic.
        let mut reg = BaoTimerRegistry::new();
        let mut obj1 = Box::new(BaoTimeoutObject::new_paused());
        obj1.timer_id = 123;
        obj1.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        reg.insert(obj1);

        let mut obj2 = Box::new(BaoTimeoutObject::new_paused());
        obj2.timer_id = 123; // duplicate
        obj2.event_loop_timer.next = bun_core::Timespec { sec: 200, nsec: 0 };
        let result = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
            reg.insert(obj2);
        }));
        assert!(result.is_err(), "insert with duplicate timer_id must panic");
    }

    #[test]
    fn bao_timer_registry_next_deadline_equal_deadlines_uses_epoch() {
        // Two timers with same deadline but different epochs; heap should order by epoch.
        let mut reg = BaoTimerRegistry::new();
        let mut obj1 = Box::new(BaoTimeoutObject::new_paused());
        obj1.timer_id = 1;
        obj1.epoch = 10; // earlier epoch
        obj1.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        reg.insert(obj1);

        let mut obj2 = Box::new(BaoTimeoutObject::new_paused());
        obj2.timer_id = 2;
        obj2.epoch = 20; // later epoch
        obj2.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        reg.insert(obj2);

        // Peek should return the one with earlier epoch (obj1)
        let peeked = reg.heap.peek();
        let recovered = unsafe { BaoTimeoutObject::from_timer_ptr(peeked) };
        assert_eq!(
            unsafe { (*recovered).timer_id },
            1,
            "earlier epoch should be at heap root"
        );
    }

    #[test]
    fn bao_timeout_object_epoch_wrapping_add() {
        // Epoch is u32; verify wrapping_add works correctly near overflow.
        let mut obj = BaoTimeoutObject::new_paused();
        obj.epoch = u32::MAX - 1;
        obj.epoch = obj.epoch.wrapping_add(1);
        assert_eq!(obj.epoch, u32::MAX);
        obj.epoch = obj.epoch.wrapping_add(1);
        assert_eq!(obj.epoch, 0, "epoch should wrap to 0 on overflow");
    }

    #[test]
    fn bao_timeout_object_fire_bumps_epoch_multiple_times() {
        // Calling fire multiple times should bump epoch each time.
        let mut obj = BaoTimeoutObject::new_paused();
        let now = Timespec {
            sec: 1_700_000_000,
            nsec: 0,
        };
        for i in 1..=5 {
            obj.fire(&now);
            assert_eq!(obj.epoch, i, "epoch should increment each fire");
        }
    }

    #[test]
    fn bao_timeout_object_interval_field_roundtrip() {
        // Verify interval field can be set and read back.
        let mut obj = BaoTimeoutObject::new_paused();
        assert!(obj.interval.is_none());
        obj.interval = Some(Duration::from_millis(250));
        assert_eq!(obj.interval, Some(Duration::from_millis(250)));
        obj.interval = None;
        assert!(obj.interval.is_none());
    }

    #[test]
    fn bao_timeout_object_timer_id_field_roundtrip() {
        // Verify timer_id field can be set and read back.
        let mut obj = BaoTimeoutObject::new_paused();
        assert_eq!(obj.timer_id, 0);
        obj.timer_id = 999_999;
        assert_eq!(obj.timer_id, 999_999);
    }

    #[test]
    fn bao_timeout_object_args_field_multiple_values() {
        // Verify args field can store multiple JSVal values.
        let mut obj = BaoTimeoutObject::new_paused();
        obj.args = vec![
            mozjs::jsval::Int32Value(1),
            mozjs::jsval::Int32Value(2),
            mozjs::jsval::Int32Value(3),
        ];
        assert_eq!(obj.args.len(), 3);
        assert_eq!(obj.args[0].to_int32(), 1);
        assert_eq!(obj.args[1].to_int32(), 2);
        assert_eq!(obj.args[2].to_int32(), 3);
    }

    #[test]
    fn bao_timer_heap_ctx_is_zst() {
        // BaoTimerHeapCtx should be a zero-sized type.
        assert_eq!(
            ::std::mem::size_of::<BaoTimerHeapCtx>(),
            0,
            "BaoTimerHeapCtx must be ZST"
        );
    }

    #[test]
    fn bao_timer_heap_default_is_empty() {
        // Default-constructed heap should have null root.
        let heap: BaoTimerHeap = ::std::default::Default::default();
        assert!(heap.peek().is_null(), "default heap peek must return null");
    }

    #[test]
    fn bao_timer_heap_insert_single_peek_returns_same() {
        // Insert one timer; peek should return that same pointer.
        let mut obj = Box::new(BaoTimeoutObject::new_paused());
        obj.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        let obj_ptr = Box::into_raw(obj);
        let timer_ptr = unsafe { core::ptr::addr_of_mut!((*obj_ptr).event_loop_timer) };

        let mut heap: BaoTimerHeap = ::std::default::Default::default();
        unsafe {
            heap.insert(timer_ptr);
        }
        assert_eq!(
            heap.peek(),
            timer_ptr,
            "peek must return the only inserted node"
        );

        // Cleanup: remove from heap before freeing
        unsafe {
            let _ = heap.delete_min();
            drop(Box::from_raw(obj_ptr));
        }
    }

    #[test]
    fn bao_timer_heap_delete_min_returns_null_when_empty() {
        // delete_min on empty heap should return null.
        let mut heap: BaoTimerHeap = ::std::default::Default::default();
        let result = unsafe { heap.delete_min() };
        assert!(
            result.is_null(),
            "delete_min on empty heap must return null"
        );
    }

    #[test]
    fn bao_timer_heap_count_empty_is_zero() {
        // count on empty heap should return 0.
        let heap: BaoTimerHeap = ::std::default::Default::default();
        assert_eq!(unsafe { heap.count() }, 0);
    }

    #[test]
    fn bao_timer_heap_count_single_is_one() {
        // count on heap with one element should return 1.
        let mut obj = Box::new(BaoTimeoutObject::new_paused());
        obj.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        let obj_ptr = Box::into_raw(obj);
        let timer_ptr = unsafe { core::ptr::addr_of_mut!((*obj_ptr).event_loop_timer) };

        let mut heap: BaoTimerHeap = ::std::default::Default::default();
        unsafe {
            heap.insert(timer_ptr);
        }
        assert_eq!(unsafe { heap.count() }, 1);

        // Cleanup
        unsafe {
            let _ = heap.delete_min();
            drop(Box::from_raw(obj_ptr));
        }
    }

    #[test]
    fn bao_timer_heap_remove_middle_node() {
        // Remove a non-root node from the heap.
        let mut objs: Vec<Box<BaoTimeoutObject>> = (0..3)
            .map(|i| {
                let mut obj = Box::new(BaoTimeoutObject::new_paused());
                obj.event_loop_timer.next = bun_core::Timespec {
                    sec: (i + 1) as i64 * 100,
                    nsec: 0,
                };
                obj
            })
            .collect();

        let ptrs: Vec<*mut EventLoopTimer> = objs
            .iter_mut()
            .map(|obj| &mut obj.event_loop_timer as *mut _)
            .collect();

        let mut heap: BaoTimerHeap = ::std::default::Default::default();
        unsafe {
            for &p in &ptrs {
                heap.insert(p);
            }
            // Remove the middle one (not the root)
            heap.remove(ptrs[1]);
            // Count should be 2
            assert_eq!(heap.count(), 2);
            // Peek should still be the earliest
            assert_eq!(heap.peek(), ptrs[0]);
            // Cleanup
            let _ = heap.delete_min();
            let _ = heap.delete_min();
        }
        drop(objs);
    }

    #[test]
    fn schedule_raw_returns_monotonic_ids() {
        // schedule_raw should return monotonically increasing IDs.
        // Use sentinel callback pointer (never dereferenced).
        // Null cx is safe — gc_store_insert skips when cx/global is null.
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        let null_cx: *mut JSContext = ::std::ptr::null_mut();
        let id1 = schedule_raw(null_cx, sentinel, 100, false, &[]);
        let id2 = schedule_raw(null_cx, sentinel, 200, false, &[]);
        let id3 = schedule_raw(null_cx, sentinel, 300, false, &[]);
        assert!(id2 > id1, "schedule_raw IDs must be monotonic");
        assert!(id3 > id2, "schedule_raw IDs must be monotonic");
        // Cleanup: cancel all scheduled timers
        cancel_raw(id1);
        cancel_raw(id2);
        cancel_raw(id3);
    }

    #[test]
    fn schedule_raw_one_shot_has_no_interval() {
        // schedule_raw with repeating=false should create one-shot timer (no interval).
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        let null_cx: *mut JSContext = ::std::ptr::null_mut();
        let id = schedule_raw(null_cx, sentinel, 100, false, &[]);
        // Retrieve the timer from BAO_REGISTRY to check interval
        let interval = BAO_REGISTRY.with(|r| r.borrow().owned.get(&id).map(|obj| obj.interval));
        assert_eq!(
            interval,
            Some(None),
            "one-shot timer must have interval=None"
        );
        cancel_raw(id);
    }

    #[test]
    fn schedule_raw_interval_has_interval_set() {
        // schedule_raw with repeating=true should create interval timer.
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        let null_cx: *mut JSContext = ::std::ptr::null_mut();
        let id = schedule_raw(null_cx, sentinel, 100, true, &[]);
        let interval = BAO_REGISTRY.with(|r| r.borrow().owned.get(&id).map(|obj| obj.interval));
        assert_eq!(
            interval,
            Some(Some(Duration::from_millis(100))),
            "interval timer must have interval set"
        );
        cancel_raw(id);
    }

    #[test]
    fn schedule_raw_zero_delay_interval_uses_minimum_one_ms() {
        // schedule_raw with delay=0 and repeating=true should use 1ms minimum.
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        let null_cx: *mut JSContext = ::std::ptr::null_mut();
        let id = schedule_raw(null_cx, sentinel, 0, true, &[]);
        let interval = BAO_REGISTRY.with(|r| r.borrow().owned.get(&id).map(|obj| obj.interval));
        assert_eq!(
            interval,
            Some(Some(Duration::from_millis(1))),
            "interval with delay=0 must use 1ms minimum"
        );
        cancel_raw(id);
    }

    #[test]
    fn cancel_raw_removes_timer_from_registry() {
        // cancel_raw should remove the timer from BAO_REGISTRY.
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        let null_cx: *mut JSContext = ::std::ptr::null_mut();
        let id = schedule_raw(null_cx, sentinel, 1000, false, &[]);
        assert!(
            BAO_REGISTRY.with(|r| r.borrow().owned.contains_key(&id)),
            "timer should be in registry"
        );
        cancel_raw(id);
        assert!(
            !BAO_REGISTRY.with(|r| r.borrow().owned.contains_key(&id)),
            "timer should be removed after cancel_raw"
        );
    }

    #[test]
    fn cancel_raw_unknown_id_is_noop() {
        // cancel_raw with unknown ID should be a no-op (no panic).
        cancel_raw(999999); // Should not panic
    }

    #[test]
    fn next_id_thread_local_isolation() {
        // NEXT_ID should be per-thread; verify we can read/write it.
        let initial = NEXT_ID.with(|n| n.get());
        NEXT_ID.with(|n| n.set(initial + 100));
        let updated = NEXT_ID.with(|n| n.get());
        assert_eq!(updated, initial + 100);
        // Reset for other tests
        NEXT_ID.with(|n| n.set(initial));
    }

    #[test]
    fn next_epoch_thread_local_isolation() {
        // NEXT_EPOCH should be per-thread; verify we can read/write it.
        let initial = NEXT_EPOCH.with(|n| n.get());
        NEXT_EPOCH.with(|n| n.set(initial + 50));
        let updated = NEXT_EPOCH.with(|n| n.get());
        assert_eq!(updated, initial + 50);
        // Reset for other tests
        NEXT_EPOCH.with(|n| n.set(initial));
    }

    #[test]
    fn next_epoch_wrapping_behavior() {
        // NEXT_EPOCH uses wrapping_add; verify it wraps correctly.
        let initial = NEXT_EPOCH.with(|n| n.get());
        NEXT_EPOCH.with(|n| n.set(u32::MAX));
        let next = NEXT_EPOCH.with(|n| {
            let v = n.get();
            n.set(v.wrapping_add(1));
            n.get()
        });
        assert_eq!(next, 0, "NEXT_EPOCH should wrap to 0");
        // Reset
        NEXT_EPOCH.with(|n| n.set(initial));
    }

    #[test]
    fn with_event_loop_returns_same_instance_on_multiple_calls() {
        // with_event_loop should return the same MiniEventLoop on repeated calls.
        let ptr1 = with_event_loop(|loop_| loop_.loop_ptr() as usize);
        let ptr2 = with_event_loop(|loop_| loop_.loop_ptr() as usize);
        let ptr3 = with_event_loop(|loop_| loop_.loop_ptr() as usize);
        assert_eq!(ptr1, ptr2);
        assert_eq!(ptr2, ptr3);
    }

    #[test]
    fn current_cx_initially_null() {
        // current_cx should return null before any register_current_cx call.
        // Note: this test may see non-null if a previous test set it, so we
        // explicitly clear it first.
        unsafe {
            register_current_cx(::std::ptr::null_mut());
        }
        assert!(
            current_cx().is_null(),
            "current_cx should be null after clearing"
        );
    }

    #[test]
    fn register_current_cx_overwrites_previous() {
        // register_current_cx should overwrite the previous value.
        let sentinel1: *mut JSContext = 0x11111111 as *mut JSContext;
        let sentinel2: *mut JSContext = 0x22222222 as *mut JSContext;
        unsafe {
            register_current_cx(sentinel1);
            assert_eq!(current_cx(), sentinel1);
            register_current_cx(sentinel2);
            assert_eq!(current_cx(), sentinel2);
            // Cleanup
            register_current_cx(::std::ptr::null_mut());
        }
    }

    #[test]
    fn bao_timer_registry_default_is_empty() {
        // BaoTimerRegistry::default() should be empty.
        let reg = BaoTimerRegistry::default();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.next_deadline().is_none());
    }

    #[test]
    fn bao_timeout_object_event_loop_timer_state_initial() {
        // new_paused should have state PENDING.
        let obj = BaoTimeoutObject::new_paused();
        assert!(
            obj.event_loop_timer.state == TimerState::PENDING,
            "initial state should be PENDING"
        );
    }

    #[test]
    fn bao_timeout_object_event_loop_timer_next_initial() {
        // new_paused should have next set to EPOCH (0, 0).
        let obj = BaoTimeoutObject::new_paused();
        assert_eq!(obj.event_loop_timer.next.sec, 0);
        assert_eq!(obj.event_loop_timer.next.nsec, 0);
    }

    #[test]
    fn bao_timeout_object_from_timer_ptr_null_is_unsound() {
        // from_timer_ptr with null should produce a valid-looking but invalid pointer.
        // This test documents that the caller must ensure non-null input.
        // We don't dereference the result; just verify the offset math.
        let result = unsafe { BaoTimeoutObject::from_timer_ptr(::std::ptr::null_mut()) };
        // The result is offset_of!(BaoTimeoutObject, event_loop_timer) bytes before null.
        // On most platforms this will be 0 (since event_loop_timer is at offset 0).
        assert_eq!(
            result as usize, 0,
            "from_timer_ptr(null) should return null when offset is 0"
        );
    }

    #[test]
    fn bao_timer_registry_insert_returns_timer_id() {
        // insert should return the timer_id that was set on the object.
        let mut reg = BaoTimerRegistry::new();
        let mut obj = Box::new(BaoTimeoutObject::new_paused());
        obj.timer_id = 42;
        obj.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        let id = reg.insert(obj);
        assert_eq!(id, 42);
    }

    #[test]
    fn bao_timer_registry_remove_returns_correct_object() {
        // remove should return the exact Box that was inserted.
        let mut reg = BaoTimerRegistry::new();
        let mut obj = Box::new(BaoTimeoutObject::new_paused());
        obj.timer_id = 777;
        obj.epoch = 12345;
        obj.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        reg.insert(obj);
        let removed = reg.remove(777).expect("should remove");
        assert_eq!(removed.timer_id, 777);
        assert_eq!(removed.epoch, 12345);
    }

    #[test]
    fn bao_timer_heap_insert_out_of_order_still_orders() {
        // Insert timers in reverse deadline order; heap should still order correctly.
        let mut reg = BaoTimerRegistry::new();
        // Insert in reverse: 500, 400, 300, 200, 100
        for (id, sec) in [(1, 500), (2, 400), (3, 300), (4, 200), (5, 100)] {
            let mut obj = Box::new(BaoTimeoutObject::new_paused());
            obj.timer_id = id;
            obj.event_loop_timer.next = bun_core::Timespec { sec, nsec: 0 };
            reg.insert(obj);
        }
        // next_deadline should be sec=100 (id=5)
        let dl = reg.next_deadline().expect("non-empty");
        assert_eq!(dl.sec, 100);
    }

    #[test]
    fn bao_timer_registry_insert_zero_timer_id() {
        // timer_id=0 is valid (though new_paused defaults to it).
        let mut reg = BaoTimerRegistry::new();
        let mut obj = Box::new(BaoTimeoutObject::new_paused());
        obj.timer_id = 0;
        obj.event_loop_timer.next = bun_core::Timespec { sec: 100, nsec: 0 };
        let id = reg.insert(obj);
        assert_eq!(id, 0);
        assert!(reg.remove(0).is_some());
    }

    #[test]
    fn bao_timeout_object_fire_does_not_change_deadline() {
        // fire should not modify the `next` deadline field.
        let mut obj = BaoTimeoutObject::new_paused();
        obj.event_loop_timer.next = bun_core::Timespec {
            sec: 12345,
            nsec: 67890,
        };
        let now = Timespec {
            sec: 1_700_000_000,
            nsec: 0,
        };
        obj.fire(&now);
        assert_eq!(obj.event_loop_timer.next.sec, 12345);
        assert_eq!(obj.event_loop_timer.next.nsec, 67890);
    }

    #[test]
    fn bao_timeout_object_fire_js_none_callback_key_skips_dispatch_with_null_cx() {
        // fire_js with callback_key=None and null cx should safely transition state.
        // The None callback_key guard in fire_js returns before reaching
        // fire_js_callback_raw, so null cx is safe.
        let mut obj = BaoTimeoutObject::new_paused();
        let now = Timespec {
            sec: 1_700_000_000,
            nsec: 0,
        };
        unsafe {
            obj.fire_js(::std::ptr::null_mut(), &now);
        }
        assert!(
            obj.event_loop_timer.state == TimerState::FIRED,
            "fire_js transitions state with None callback_key"
        );
        assert_eq!(obj.epoch, 1);
    }

    #[test]
    fn has_pending_timers_reflects_registry_state() {
        // has_pending_timers should reflect BAO_REGISTRY state.
        // Clear registry first
        let ids: Vec<u32> = BAO_REGISTRY.with(|r| r.borrow().owned.keys().copied().collect());
        for id in ids {
            cancel_raw(id);
        }
        assert!(!has_pending_timers(), "should be empty after clearing");
        // Add a timer
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        let null_cx: *mut JSContext = ::std::ptr::null_mut();
        let id = schedule_raw(null_cx, sentinel, 1000, false, &[]);
        assert!(has_pending_timers(), "should have pending timer");
        cancel_raw(id);
        assert!(!has_pending_timers(), "should be empty after cancel");
    }

    // ──────────────────────────────────────────────────────────────────────────
    // BCE-20260619-001 regression: clearInterval / clearTimeout invoked inside
    // the timer's own JS callback must mark the timer as cleared so the
    // drain loop skips the post-fire re-arm. Without the firing-slot signal,
    // the registry lookup misses (drain pops before firing) and the interval
    // would resurrect forever, pinning the process in the event loop.
    // ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn cancel_raw_during_fire_latches_cleared_flag_for_matching_id() {
        // Simulate drain_bao_timers' "popped before fire" state: advertise
        // the firing id, then cancel_raw(id) must set CLEARED_DURING_FIRE
        // even though the timer is no longer in BAO_REGISTRY.
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        let null_cx: *mut JSContext = ::std::ptr::null_mut();
        let id = schedule_raw(null_cx, sentinel, 1000, true, &[]); // interval
                                                                   // Pop it the way drain_bao_timers does.
        let _obj = BAO_REGISTRY.with(|r| r.borrow_mut().remove(id));
        assert!(!BAO_REGISTRY.with(|r| r.borrow().owned.contains_key(&id)));

        // Advertise firing + reset flag (drain pattern).
        CURRENT_FIRING_TIMER.with(|c| c.set(id));
        CLEARED_DURING_FIRE.with(|c| c.set(false));

        // cancel_raw on the popped id must latch the flag.
        cancel_raw(id);
        assert!(
            CLEARED_DURING_FIRE.with(|c| c.get()),
            "cancel_raw during fire of matching id must set CLEARED_DURING_FIRE"
        );

        // Cleanup.
        CURRENT_FIRING_TIMER.with(|c| c.set(0));
        CLEARED_DURING_FIRE.with(|c| c.set(false));
    }

    #[test]
    fn cancel_raw_during_fire_ignores_non_matching_id() {
        // If a different id is being fired, cancel_raw(other_id) must NOT
        // latch the flag — that would wrongly signal clear-of-firing-timer.
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        let null_cx: *mut JSContext = ::std::ptr::null_mut();
        let firing_id = schedule_raw(null_cx, sentinel, 1000, true, &[]);
        let other_id = schedule_raw(null_cx, sentinel, 1000, true, &[]);
        let _o = BAO_REGISTRY.with(|r| r.borrow_mut().remove(firing_id));

        CURRENT_FIRING_TIMER.with(|c| c.set(firing_id));
        CLEARED_DURING_FIRE.with(|c| c.set(false));

        // Cancel the *other* id — it's in registry, normal path. Must not
        // touch the firing flag.
        cancel_raw(other_id);
        assert!(
            !CLEARED_DURING_FIRE.with(|c| c.get()),
            "cancel of non-firing id must NOT latch CLEARED_DURING_FIRE"
        );

        CURRENT_FIRING_TIMER.with(|c| c.set(0));
    }

    #[test]
    fn cancel_raw_during_fire_with_no_firing_timer_is_noop() {
        // When CURRENT_FIRING_TIMER is 0 (no timer firing), cancel_raw of
        // an unknown id must be a plain no-op — flag stays false.
        CLEARED_DURING_FIRE.with(|c| c.set(false));
        CURRENT_FIRING_TIMER.with(|c| c.set(0));
        cancel_raw(95123); // unknown, no firing
        assert!(
            !CLEARED_DURING_FIRE.with(|c| c.get()),
            "cancel with no firing timer must not latch flag"
        );
    }
}
