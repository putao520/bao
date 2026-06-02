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

pub fn drain_and_check(cx: &mut mozjs::context::JSContext) -> bool {
    let epfd = ensure_epoll_fd();
    sync_http_listeners();

    crate::node_http::accept_connections();

    let has_http = crate::node_http::has_active_servers();
    let deadline = next_deadline();

    let timeout_ms: i32 = if let Some(d) = deadline {
        let now = Instant::now();
        if d > now {
            ((d - now).as_millis() as i32).min(100)
        } else {
            0
        }
    } else if has_http {
        100
    } else {
        drain_timers(cx);
        bao_engine::job_queue::JobQueue::drain(cx);
        return false;
    };

    let mut events: [libc::epoll_event; 32] = unsafe { ::std::mem::zeroed() };
    unsafe {
        libc::epoll_wait(epfd, events.as_mut_ptr(), 32, timeout_ms);
    }

    crate::node_http::poll_http_requests(cx);
    drain_timers(cx);
    bao_engine::job_queue::JobQueue::drain(cx);

    has_pending_timers() || has_http
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

pub fn drain_timers(cx: &mut mozjs::context::JSContext) -> bool {
    let now = Instant::now();
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
        unsafe { fire_js_callback_raw(raw_cx, entry.callback, &entry.args) };
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

pub fn has_pending_timers() -> bool {
    TIMERS.with(|t| !t.borrow().is_empty())
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
    id
}

pub fn cancel_raw(id: u32) {
    TIMERS.with(|t| t.borrow_mut().remove(id));
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
        args: extra_args,
    };

    TIMERS.with(|t| t.borrow_mut().insert(entry));

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
}

impl BaoTimeoutObject {
    /// Construct a paused (PENDING state) timeout with epoch 0.
    pub fn new_paused() -> Self {
        Self {
            event_loop_timer: EventLoopTimer::init_paused(
                bun_event_loop::EventLoopTimer::Tag::TimeoutObject,
            ),
            epoch: 0,
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

    /// Mark this timer as fired. P1-A.3 will replace this with real JS
    /// callback dispatch (rooted JSObject + JS_CallFunctionValue).
    pub fn fire(&mut self, _now: &Timespec) {
        self.event_loop_timer.state = TimerState::FIRED;
        self.epoch = self.epoch.wrapping_add(1);
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
}
