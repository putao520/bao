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

    for entry in ready {
        unsafe {
            let raw_cx = cx.raw_cx();
            let global = CurrentGlobalOrNull(raw_cx);
            if global.is_null() {
                continue;
            }

            let obj_handle = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &global,
            };
            let fval = ObjectValue(entry.callback);
            let fval_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &fval,
            };

            let args_array = if entry.args.is_empty() {
                HandleValueArray::empty()
            } else {
                HandleValueArray {
                    length_: entry.args.len(),
                    elements_: entry.args.as_ptr(),
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

    true
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
