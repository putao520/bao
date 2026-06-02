// @trace TEST-ENG-004 [req:REQ-ENG-004] [level:integration]
// P1-A Stage 1 prerequisite validation: prove `bun_event_loop::MiniEventLoop`
// is usable from the bao_runtime crate (same thread, no JS engine). When P1-A
// lands, drain_and_check will use this same API to replace TimerHeap+epoll.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext;
use bun_event_loop::MiniEventLoop::MiniEventLoop;

// Pull in bao_uloop's `#[no_mangle] extern "C"` symbols (uws_get_loop etc.)
// — `MiniEventLoop::init()` reaches them via `UwsLoop::get()` but the linker
// GCs unreferenced no-mangle symbols without an explicit Rust reference.
fn force_uloop_link() {
    bao_uloop::force_link();
    bao_native_stubs::force_link();
}

#[derive(Debug)]
struct CounterCtx {
    fired: AtomicUsize,
}

fn increment_task(ctx: *mut CounterCtx, _extra: *mut std::ffi::c_void) {
    unsafe { (*ctx).fired.fetch_add(1, Ordering::SeqCst); }
}

#[test]
fn test_minimal_event_loop_init() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();
    assert!(!loop_.loop_ptr().is_null(), "MiniEventLoop must produce a non-null uSockets loop pointer");
    assert!(loop_.tasks.readable_length() == 0, "fresh loop must have empty task queue");
    assert!(loop_.pipe_read_buffer().len() > 0, "pipe_read_buffer must be initialized on first access");
}

#[test]
fn test_minimal_event_loop_enqueue_and_drain() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();
    let ctx = Box::new(CounterCtx { fired: AtomicUsize::new(0) });
    let ctx_ptr = Box::into_raw(ctx);

    // from_callback_auto_deinit wraps the ctx pointer and self-frees the wrapper
    // when the callback fires. counter is bumped inside the callback.
    let task_ptr = AnyTaskWithExtraContext::from_callback_auto_deinit(ctx_ptr, increment_task);
    assert!(!task_ptr.is_null());

    // SAFETY: enqueue_task_concurrent expects a NonNull<AnyTaskWithExtraContext>
    // that outlives the queue. The wrapper box is freed inside the callback,
    // so ownership transfers to the loop until tick drains it.
    let task_nn = unsafe { core::ptr::NonNull::new_unchecked(task_ptr) };
    loop_.enqueue_task_concurrent(task_nn);

    assert_eq!(loop_.tasks.readable_length(), 0, "concurrent queue is not yet flushed into tasks");

    // tick_once first flushes concurrent → tasks, then runs all tasks.
    loop_.tick_once(core::ptr::null_mut());

    let fired = unsafe { (*ctx_ptr).fired.load(Ordering::SeqCst) };
    assert_eq!(fired, 1, "callback must fire exactly once after tick_once");

    // The wrapper Box was freed inside `function<T>` (auto-deinit). Reclaim
    // ctx_ptr only — the wrapper memory is already dropped.
    unsafe { drop(Box::from_raw(ctx_ptr)); }
}

#[test]
fn test_minimal_event_loop_tick_until_done() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();
    let done = Arc::new(AtomicBool::new(true));
    let done_clone = done.clone();

    // is_done returns true immediately — tick must return on first iteration.
    // An empty loop with is_done=true must not call into uSockets internals.
    loop_.tick(core::ptr::null_mut(), |_ctx| done_clone.load(Ordering::SeqCst));

    assert!(done.load(Ordering::Relaxed), "done flag unchanged after tick");
}

#[test]
fn test_minimal_event_loop_multiple_ticks() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();

    // Multiple ticks on an empty loop must be safe (no tasks, no timers).
    for _ in 0..10 {
        loop_.tick_once(core::ptr::null_mut());
    }
    assert_eq!(loop_.tasks.readable_length(), 0);
}
