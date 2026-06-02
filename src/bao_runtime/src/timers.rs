// @trace REQ-ENG-004
use ::std::cell::{Cell, RefCell};
use ::std::os::unix::io::RawFd;
use ::std::time::{Duration, Instant};

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, Int32Value, ObjectValue};
use mozjs::rust::wrappers2::JS_DefineFunction;

thread_local! {
    static TIMERS: RefCell<TimerHeap> = RefCell::new(TimerHeap::new());
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };
    static EPOLL_FD: Cell<RawFd> = const { Cell::new(-1) };
    static REGISTERED_FDS: RefCell<Vec<RawFd>> = const { RefCell::new(Vec::new()) };

    /// P1-A.3a: per-thread MiniEventLoop holder for the new timer dispatch
    /// path. Lazily initialized on first access via `with_event_loop`.
    /// Parallel to `bao_engine::dispatch_sm::BaoEventLoop` — eventual
    /// consolidation (single source of truth per thread) is a P1-A.4+
    /// concern. Stored as `Option<MiniEventLoop<'static>>` because
    /// `MiniEventLoop::init()` is non-const.
    static BAO_RUNTIME_LOOP: RefCell<::std::option::Option<bun_event_loop::MiniEventLoop::MiniEventLoop<'static>>> =
        const { RefCell::new(::std::option::Option::None) };

    /// P1-A.3b: thread-local pointer to the current `JSContext*`. Registered
    /// by `drain_and_check` (or any JS-bearing entry point) before any
    /// MiniEventLoop timer fire, so that `__bun_fire_timer` (which receives
    /// only `*mut EventLoopTimer` + an opaque `_vm: *mut ()`) can recover
    /// the live cx and dispatch JS callbacks via `fire_js_callback_raw`.
    ///
    /// Stored as `*mut JSContext` (raw pointer) — `Cell` for cheap set/get.
    /// Null when no JS context is active on this thread.
    static CURRENT_CX: Cell<*mut JSContext> = const { Cell::new(::std::ptr::null_mut()) };

    /// P1-A.3c-step3: per-thread BaoTimerRegistry holding the new Bun-style
    /// intrusive-heap timers. Dual-written alongside the legacy TIMERS
    /// TimerHeap during the P1-A.3c migration — register_timer/clear_timeout
    /// push/remove from BOTH registries so P1-A.3d can flip drain_and_check
    /// to read from this one without losing clearTimeout semantics.
    static BAO_REGISTRY: RefCell<BaoTimerRegistry> = RefCell::new(BaoTimerRegistry::new());

    /// P1-A.3c-step3: monotonic epoch counter for stable heap ordering of
    /// equal-deadline BaoTimeoutObjects. Mirrors Bun's
    /// `TimerObjectInternals.flags.epoch`. Bumped on each schedule.
    static NEXT_EPOCH: Cell<u32> = const { Cell::new(1) };
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
        unsafe { register_current_cx(::std::ptr::null_mut()); }
    }
}

/// P1-A.3a: access or lazily materialize the per-thread MiniEventLoop.
///
/// Returns a `RefMut<MiniEventLoop<'static>>` that callers can use to
/// schedule timers / enqueue tasks / tick. The loop survives for the
/// thread's lifetime (intentionally leaked on thread exit to avoid
/// ordering issues with JSContext teardown — same pattern as
/// `bao_engine::BaoEventLoop`).
///
/// Not yet wired into `drain_and_check` — that's P1-A.3c/d.
pub fn with_event_loop<F, R>(f: F) -> R
where
    F: FnOnce(&mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>) -> R,
{
    BAO_RUNTIME_LOOP.with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.is_none() {
            // Ensure bao_uloop's #[no_mangle] extern "C" symbols are
            // referenced — without this the linker may GC uSockets loop
            // entrypoints that MiniEventLoop::init reaches via UwsLoop::get().
            bao_uloop::force_link();
            *guard = ::std::option::Option::Some(bun_event_loop::MiniEventLoop::MiniEventLoop::init());
        }
        let opt = guard.as_mut().expect("just initialized");
        f(opt)
    })
}

struct TimerEntry {
    id: u32,
    deadline: Instant,
    interval: Option<Duration>,
    callback: *mut JSObject,
    args: Vec<JSVal>,
}

struct TimerHeap {
    timers: Vec<TimerEntry>,
}

impl TimerHeap {
    fn new() -> Self {
        TimerHeap { timers: Vec::new() }
    }

    fn insert(&mut self, entry: TimerEntry) {
        self.timers.push(entry);
    }

    fn remove(&mut self, id: u32) {
        self.timers.retain(|t| t.id != id);
    }

    fn drain_ready(&mut self, now: Instant) -> Vec<TimerEntry> {
        let mut ready = Vec::new();
        let mut remaining = Vec::new();

        for t in self.timers.drain(..) {
            if now >= t.deadline {
                ready.push(t);
            } else {
                remaining.push(t);
            }
        }

        for t in &ready {
            if let Some(interval) = t.interval {
                let mut re_entry = TimerEntry {
                    id: t.id,
                    deadline: t.deadline + interval,
                    interval: Some(interval),
                    callback: t.callback,
                    args: t.args.clone(),
                };
                while re_entry.deadline <= now {
                    re_entry.deadline += interval;
                }
                remaining.push(re_entry);
            }
        }

        self.timers = remaining;
        ready
    }

    fn is_empty(&self) -> bool {
        self.timers.is_empty()
    }
}

unsafe impl Send for TimerHeap {}

fn ensure_epoll_fd() -> RawFd {
    EPOLL_FD.with(|cell| {
        let fd = cell.get();
        if fd >= 0 {
            return fd;
        }
        let fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        debug_assert!(fd >= 0, "epoll_create1 failed: {}", fd);
        cell.set(fd);
        fd
    })
}

fn sync_http_listeners() {
    let epfd = ensure_epoll_fd();
    let current_fds: Vec<RawFd> = crate::node_http::listener_fds();
    let registered: Vec<RawFd> = REGISTERED_FDS.with(|r| r.borrow().clone());

    for fd in &current_fds {
        if !registered.contains(fd) {
            let mut ev = libc::epoll_event {
                events: libc::EPOLLIN as u32,
                u64: *fd as u64,
            };
            unsafe {
                libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, *fd, &mut ev);
            }
        }
    }

    for fd in &registered {
        if !current_fds.contains(fd) {
            unsafe {
                libc::epoll_ctl(epfd, libc::EPOLL_CTL_DEL, *fd, ::std::ptr::null_mut());
            }
        }
    }

    REGISTERED_FDS.with(|r| *r.borrow_mut() = current_fds);
}

pub fn init() {
    ensure_epoll_fd();
}

pub fn install_timer_globals(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    init();
    unsafe {
        JS_DefineFunction(
            cx, global, c"setTimeout".as_ptr(),
            ::std::option::Option::Some(set_timeout), 2, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, global, c"clearTimeout".as_ptr(),
            ::std::option::Option::Some(clear_timeout), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, global, c"setInterval".as_ptr(),
            ::std::option::Option::Some(set_interval), 2, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, global, c"clearInterval".as_ptr(),
            ::std::option::Option::Some(clear_interval), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, global, c"setImmediate".as_ptr(),
            ::std::option::Option::Some(set_immediate), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, global, c"clearImmediate".as_ptr(),
            ::std::option::Option::Some(clear_timeout), 1, JSPROP_ENUMERATE as u32,
        );
    }
}

/// P1-A.3d: main event-loop tick. Drains BAO_REGISTRY timers via the new
/// Bun-style intrusive heap and fires their JS callbacks. Retains epoll for
/// HTTP I/O (P1-B will replace this with bun_uws).
///
/// Returns `true` if the event loop should continue (has pending timers or
/// active HTTP servers).
pub fn drain_and_check(cx: &mut mozjs::context::JSContext) -> bool {
    // SAFETY: cx is a live &mut JSContext on the current thread; the guard
    // clears it on drop so subsequent code on this thread sees null.
    unsafe {
        register_current_cx(cx.raw_cx());
    }
    let _cx_guard = CxGuard::new();

    let epfd = ensure_epoll_fd();
    sync_http_listeners();

    crate::node_http::accept_connections();

    let has_http = crate::node_http::has_active_servers();
    let deadline_ms = bao_next_deadline_ms();

    let timeout_ms: i32 = if let Some(ms) = deadline_ms {
        ms.min(100)
    } else if has_http {
        100
    } else {
        // No timers and no HTTP — drain any already-expired timers and exit.
        let raw_cx = unsafe { cx.raw_cx() };
        drain_bao_timers(raw_cx);
        bao_engine::job_queue::JobQueue::drain(cx);
        return false;
    };

    let mut events: [libc::epoll_event; 32] = unsafe { ::std::mem::zeroed() };
    unsafe {
        libc::epoll_wait(epfd, events.as_mut_ptr(), 32, timeout_ms);
    }

    crate::node_http::poll_http_requests(cx);
    let raw_cx = unsafe { cx.raw_cx() };
    drain_bao_timers(raw_cx);
    bao_engine::job_queue::JobQueue::drain(cx);

    bao_has_pending_timers() || has_http
}

pub fn next_deadline() -> ::std::option::Option<Instant> {
    TIMERS.with(|t| {
        let heap = t.borrow();
        if heap.is_empty() {
            return None;
        }
        heap.timers.iter().map(|e| e.deadline).min()
    })
}

/// Drain ready timers from TIMERS and fire their JS callbacks.
/// Synchronises BAO_REGISTRY so the dual-write invariant is maintained:
/// - One-shot timers: removed from BAO_REGISTRY after fire (prevent leak).
/// - Interval timers: deadline updated in BAO_REGISTRY to match TIMERS re-arm.
///
/// Must NOT hold a borrow on BAO_REGISTRY across JS callback dispatch
/// (callbacks may call setTimeout/clearTimeout → re-entrant borrow).
pub fn drain_timers(cx: &mut mozjs::context::JSContext) -> bool {
    let now = Instant::now();
    let now_ts = bun_core::Timespec::now_allow_mocked_time();
    let ready = TIMERS.with(|t| t.borrow_mut().drain_ready(now));

    if ready.is_empty() {
        return false;
    }

    // SAFETY: `cx.raw_cx()` returns the live JSContext* for the current
    // thread; `fire_js_callback_raw` requires a live cx and rooted callback/
    // args. Callbacks come from TimerEntry which stores raw `*mut JSObject`
    // scheduled from JS host fns — bao_runtime keeps them alive via the
    // implicit no-GC window between schedule and fire (drain_and_check runs
    // to completion before yielding back to the JS engine).
    let raw_cx = unsafe { cx.raw_cx() };
    for entry in ready {
        // Fire the JS callback first (no BAO_REGISTRY borrow held).
        unsafe { fire_js_callback_raw(raw_cx, entry.callback, &entry.args) };

        // Post-fire BAO_REGISTRY synchronisation — separate borrow scope
        // so re-entrant setTimeout/clearTimeout from the callback above
        // does not conflict with this borrow.
        BAO_REGISTRY.with(|r| {
            if let Some(interval) = entry.interval {
                // Interval: re-arm the BaoTimeoutObject to match the
                // new deadline that drain_ready computed for TIMERS.
                // Re-arm strategy: update state back to PENDING, bump
                // epoch, compute next deadline via Timespec arithmetic.
                let mut reg = r.borrow_mut();
                if let ::std::option::Option::Some(obj) = reg.owned.get_mut(&entry.id) {
                    let interval_ms = interval.as_millis() as i64;
                    // Advance next deadline past `now` (same while-loop as
                    // drain_ready uses for Instant — but in Timespec domain).
                    let mut next_ts = obj.event_loop_timer.next;
                    while next_ts.order(&now_ts) != core::cmp::Ordering::Greater {
                        next_ts = next_ts.add_ms(interval_ms);
                    }
                    obj.event_loop_timer.next = next_ts;
                    obj.event_loop_timer.state = TimerState::PENDING;
                    obj.epoch = obj.epoch.wrapping_add(1);
                }
            } else {
                // One-shot: remove from BAO_REGISTRY to prevent leak.
                r.borrow_mut().remove(entry.id);
            }
        });
    }

    true
}

/// Fire a JS callback via `JS_CallFunctionValue`, swallowing any pending
/// exception. Extracted from `drain_timers` so `BaoTimeoutObject::fire`
/// (P1-A.3) can reuse the same dispatch path without duplication.
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
        if global.is_null() {
            return;
        }

        let obj_handle = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &global,
        };
        let fval = ObjectValue(callback);
        let fval_handle = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &fval,
        };

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

        JS_CallFunctionValue(raw_cx, obj_handle, fval_handle, &args_array, rval_handle);
        JS_ClearPendingException(raw_cx);
    }
}

/// P1-A.3d: check if BAO_REGISTRY has any pending timers.
/// This is the new source of truth (replaces TIMERS-based check).
pub fn has_pending_timers() -> bool {
    BAO_REGISTRY.with(|r| !r.borrow().is_empty())
}

/// P1-A.3d: compute milliseconds until the next BAO_REGISTRY timer deadline.
/// Returns `None` if no timers are pending. Result capped at 100ms (epoll max
/// sleep to keep HTTP polling responsive).
fn bao_next_deadline_ms() -> ::std::option::Option<i32> {
    BAO_REGISTRY.with(|r| {
        let reg = r.borrow();
        match reg.next_deadline() {
            None => None,
            Some(dl) => {
                let now = Timespec::now_allow_mocked_time();
                if dl.order(&now) != core::cmp::Ordering::Greater {
                    ::std::option::Option::Some(0) // already expired
                } else if dl.sec > now.sec {
                    // ≥1 second remaining → definitely > 100ms
                    ::std::option::Option::Some(100)
                } else {
                    // Same second — nanosecond diff fits in i64 easily
                    let diff_ns = dl.nsec - now.nsec;
                    ::std::option::Option::Some(((diff_ns / 1_000_000) as i32).min(100))
                }
            }
        }
    })
}

/// P1-A.3d: alias for has_pending_timers (used by drain_and_check).
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
        if !should_fire { break; }

        // Pop the timer from BAO_REGISTRY — takes Box ownership.
        // No borrow is held after this scope closes.
        let obj_box = BAO_REGISTRY.with(|r| {
            let mut reg = r.borrow_mut();
            let peeked = reg.heap.peek();
            if peeked.is_null() { return None; }
            // SAFETY: peeked is non-null and points to a BaoTimeoutObject's
            // event_loop_timer field (the only type we insert into this heap).
            let timeout = unsafe { BaoTimeoutObject::from_timer_ptr(peeked) };
            let id = unsafe { (*timeout).timer_id };
            reg.remove(id)
        });

        let Some(mut obj) = obj_box else { break; };
        fired = true;

        // Fire JS callback — no BAO_REGISTRY borrow held.
        // SAFETY: raw_cx is a live JSContext* registered by drain_and_check.
        unsafe { obj.fire_js(raw_cx, &now_ts); }

        // If interval, re-arm with updated deadline and re-insert.
        if let Some(interval) = obj.interval {
            let interval_ms = interval.as_millis() as i64;
            let mut next_ts = obj.event_loop_timer.next;
            while next_ts.order(&now_ts) != core::cmp::Ordering::Greater {
                next_ts = next_ts.add_ms(interval_ms);
            }
            obj.event_loop_timer.next = next_ts;
            obj.event_loop_timer.state = TimerState::PENDING;
            obj.epoch = obj.epoch.wrapping_add(1);
            BAO_REGISTRY.with(|r| r.borrow_mut().insert(obj));
        }
        // One-shot: Box<BaoTimeoutObject> dropped here.
    }

    fired
}

pub fn schedule_raw(callback: *mut JSObject, delay_ms: u64, repeating: bool, _args: &[JSVal]) -> u32 {
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

    let entry = TimerEntry {
        id,
        deadline: Instant::now() + Duration::from_millis(if delay_ms == 0 && repeating { 1 } else { delay_ms }),
        interval,
        callback,
        args: _args.to_vec(),
    };

    TIMERS.with(|t| t.borrow_mut().insert(entry));

    // P1-A.3c-step3: dual-write — also push a BaoTimeoutObject so the new
    // BAO_REGISTRY stays in sync with the legacy TIMERS heap.
    let mut bao_obj = Box::new(BaoTimeoutObject::new_paused());
    bao_obj.timer_id = id;
    bao_obj.event_loop_timer.next = bun_core::Timespec::now_allow_mocked_time()
        .add_ms((if delay_ms == 0 && repeating { 1 } else { delay_ms }) as i64);
    bao_obj.interval = interval;
    bao_obj.callback = ::std::option::Option::Some(callback);
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
    TIMERS.with(|t| t.borrow_mut().remove(id));
    // P1-A.3c-step3: dual-write cancel — also remove from BAO_REGISTRY.
    BAO_REGISTRY.with(|r| {
        r.borrow_mut().remove(id);
    });
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn set_timeout(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    register_timer(cx, argc, vp, false)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn set_interval(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    register_timer(cx, argc, vp, true)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn clear_timeout(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            let id = v.to_int32() as u32;
            TIMERS.with(|t| t.borrow_mut().remove(id));
            // P1-A.3c-step3: dual-write cancel from JS host_fn.
            BAO_REGISTRY.with(|r| {
                r.borrow_mut().remove(id);
            });
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn clear_interval(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    clear_timeout(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn set_immediate(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }
    let cb = (*args.get(0).ptr).to_object();
    let cb_args = if argc > 1 {
        (1..argc).map(|i| *args.get(i).ptr).collect()
    } else {
        Vec::new()
    };
    let id = schedule_raw(cb, 0, false, &cb_args);
    args.rval().set(Int32Value(id as i32));
    true
}

/// Parse JS args from setTimeout/setInterval and delegate to `schedule_raw`
/// for dual-write registration. Single source of truth for TimerEntry +
/// BaoTimeoutObject construction, deadline calculation, and epoch bump.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn register_timer(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
    repeating: bool,
) -> bool {
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

    let callback = first.to_object();

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

    let id = schedule_raw(callback, delay_ms, repeating, &extra_args);
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
    /// JS callback to invoke when the timer fires. None = virgin state.
    /// Stored as raw `*mut JSObject`; rooted by the implicit no-GC window
    /// between schedule (host_fn set_timeout/set_interval) and fire (drain
    /// runs to completion before yielding to the SM engine). Same pattern as
    /// the legacy TimerHeap — see `TimerEntry.callback` doc.
    pub callback: ::std::option::Option<*mut JSObject>,
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
            callback: ::std::option::Option::None,
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
    /// No-op if no callback is attached (defensive — schedule path always
    /// sets one). Swallows any JS exception per drain_timers convention.
    ///
    /// # Safety
    /// - `raw_cx` must be a live `JSContext*` on the current thread.
    /// - `self.callback` (if Some) must point to a live function object
    ///   rooted by the caller for the duration of this call.
    /// - `self.args` slice must point to `JSVal`s rooted by the caller.
    pub unsafe fn fire_js(&mut self, raw_cx: *mut JSContext, now: &Timespec) {
        self.fire(now);
        if let ::std::option::Option::Some(cb) = self.callback
            && !cb.is_null()
        {
            unsafe { fire_js_callback_raw(raw_cx, cb, &self.args) };
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
        assert!(!self.owned.contains_key(&id), "duplicate timer_id {id} in BaoTimerRegistry");
        // SAFETY: Box owns a heap allocation; we never move the Box while
        // it's in the heap. Pointer stays valid until remove() pulls it out.
        let timer_ptr: *mut EventLoopTimer = &mut obj.event_loop_timer;
        unsafe { self.heap.insert(timer_ptr); }
        self.owned.insert(id, obj);
        id
    }

    /// Remove timer `id` from both the heap and the owned map. Returns
    /// ownership of the removed `BaoTimeoutObject`, or `None` if not found.
    pub fn remove(&mut self, id: u32) -> ::std::option::Option<::std::boxed::Box<BaoTimeoutObject>> {
        let mut obj = self.owned.remove(&id)?;
        let timer_ptr: *mut EventLoopTimer = &mut obj.event_loop_timer;
        // SAFETY: timer_ptr was inserted into self.heap by `insert`; the
        // node is still live (we hold the Box). remove() detaches it from
        // the heap without touching any other node.
        unsafe { self.heap.remove(timer_ptr); }
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
        unsafe { drop(Box::from_raw(obj_ptr)); }
    }

    #[test]
    fn bao_timeout_object_fire_transitions_state() {
        let mut obj = BaoTimeoutObject::new_paused();
        assert!(obj.event_loop_timer.state == TimerState::PENDING, "initial state is PENDING");
        assert_eq!(obj.epoch, 0);

        let now = Timespec { sec: 1_700_000_000, nsec: 0 };
        obj.fire(&now);

        assert!(obj.event_loop_timer.state == TimerState::FIRED, "fire transitions to FIRED");
        assert_eq!(obj.epoch, 1, "fire bumps epoch for stable re-queue ordering");
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
        assert!(obj.callback.is_none(), "new_paused must start with no callback");
        assert!(obj.args.is_empty(), "new_paused must start with empty args");
    }

    #[test]
    fn bao_timeout_object_fire_js_null_callback_is_noop() {
        // fire_js with null callback must still transition state but skip
        // JS dispatch (defensive guard). No JSContext is touched.
        let mut obj = BaoTimeoutObject::new_paused();
        let now = Timespec { sec: 1_700_000_000, nsec: 0 };

        // SAFETY: passing null raw_cx is sound because the null callback
        // guard returns before any JSAPI call. We deliberately test the
        // defensive path — the production path requires a live cx.
        unsafe { obj.fire_js(::std::ptr::null_mut(), &now); }

        assert!(obj.event_loop_timer.state == TimerState::FIRED, "fire_js transitions state even with null callback");
        assert_eq!(obj.epoch, 1, "fire_js bumps epoch");
    }

    #[test]
    fn bao_timeout_object_callback_field_roundtrip() {
        // Verify callback/args fields can be set and read back. Uses a
        // sentinel pointer (non-null but never dereferenced) to validate
        // storage without engaging JSAPI.
        let mut obj = BaoTimeoutObject::new_paused();
        let sentinel: *mut JSObject = 0xdeadbeef as *mut JSObject;
        obj.callback = ::std::option::Option::Some(sentinel);
        obj.args = vec![mozjs::jsval::Int32Value(42)];

        assert_eq!(obj.callback, ::std::option::Option::Some(sentinel), "callback field stores raw pointer");
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
        assert_eq!(ptr1, ptr2, "with_event_loop must return the same loop on repeated calls");
    }

    #[test]
    fn current_cx_roundtrip_via_thread_local() {
        // P1-A.3b: register_current_cx writes; current_cx reads back.
        // Sentinel pointer is never dereferenced — only validates storage.
        let sentinel: *mut JSContext = 0x12345678 as *mut JSContext;
        // SAFETY: sentinel is never dereferenced; we only validate the
        // thread_local round-trip. Cleared at end so subsequent tests see
        // null.
        unsafe { register_current_cx(sentinel); }
        assert_eq!(current_cx(), sentinel, "register_current_cx stores cx in thread_local");

        // Clear back to null to avoid leaking sentinel into other tests.
        unsafe { register_current_cx(::std::ptr::null_mut()); }
        assert!(current_cx().is_null(), "register_current_cx(null) clears the slot");
    }

    #[test]
    fn bao_timeout_object_new_paused_has_no_interval() {
        // P1-A.3c: new_paused initial state for interval + timer_id fields.
        let obj = BaoTimeoutObject::new_paused();
        assert!(obj.interval.is_none(), "new_paused must start with no interval (one-shot)");
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
        assert_eq!(peeked, earlier_ptr, "heap.peek must return earliest deadline (Bun's less ordering)");

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
        assert!(removed.is_some(), "remove returns Some(Box<..>) for known id");
        assert_eq!(reg.len(), 0);
        assert!(reg.is_empty());
        assert!(reg.next_deadline().is_none(), "heap must be empty after remove");

        // Removing unknown id returns None.
        let again = reg.remove(42);
        assert!(again.is_none(), "remove returns None for unknown id");
    }
}
