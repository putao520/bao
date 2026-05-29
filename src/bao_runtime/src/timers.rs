use ::std::cell::RefCell;
use ::std::time::{Duration, Instant};

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, Int32Value};
use mozjs::rust::wrappers2::JS_DefineFunction;

thread_local! {
    static TIMERS: RefCell<TimerHeap> = RefCell::new(TimerHeap::new());
    static NEXT_ID: RefCell<u32> = RefCell::new(1);
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

pub fn install_timer_globals(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
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

pub fn drain_and_check(cx: &mut mozjs::context::JSContext) -> bool {
    crate::node_http::accept_connections();
    crate::node_http::poll_http_requests(cx);
    drain_timers(cx);
    if has_pending_timers() {
        wait_for_next_timer();
        true
    } else {
        crate::node_http::has_active_servers()
    }
}

pub fn next_deadline() -> ::std::option::Option<Instant> {
    TIMERS.with(|t| {
        let heap = t.borrow();
        if heap.is_empty() {
            return None;
        }
        let earliest = heap.timers.iter().map(|e| e.deadline).min();
        earliest
    })
}

pub fn wait_for_next_timer() {
    let deadline = next_deadline();
    let Some(deadline) = deadline else { return };
    let now = Instant::now();
    if deadline > now {
        let wait = deadline - now;
        if wait > Duration::from_millis(1) {
            ::std::thread::sleep(wait.min(Duration::from_millis(100)));
        }
    }
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
            let fval = mozjs::jsval::ObjectValue(entry.callback);
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
        let val = *n.borrow();
        *n.borrow_mut() += 1;
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
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(mozjs::jsval::Int32Value(0));
        return true;
    }
    let cb = (*args.get(0).ptr).to_object();
    let cb_args = if argc > 1 {
        let mut a = Vec::new();
        for i in 1..argc {
            a.push(*args.get(i).ptr);
        }
        a
    } else {
        Vec::new()
    };
    let id = schedule_raw(cb, 0, false, &cb_args);
    args.rval().set(mozjs::jsval::Int32Value(id as i32));
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
        let val = *n.borrow();
        *n.borrow_mut() += 1;
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
