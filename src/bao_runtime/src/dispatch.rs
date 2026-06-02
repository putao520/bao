// @trace REQ-ENG-004: Timer dispatch for SpiderMonkey
//! Link-time extern implementations for `EventLoopTimer`.

use bun_core::Timespec;
use bun_event_loop::EventLoopTimer::{
    EventLoopTimer, State as TimerState, Tag,
};

use super::timers::BaoTimeoutObject;

/// Fire a timer callback.
///
/// # Safety
/// `t` must be a live `EventLoopTimer` just popped from the heap.
#[unsafe(no_mangle)]
pub unsafe extern "Rust" fn __bun_fire_timer(
    t: *mut EventLoopTimer,
    now: *const Timespec,
    _vm: *mut (),
) {
    if t.is_null() { return; }

    match (*t).tag {
        Tag::TimeoutObject | Tag::ImmediateObject => {
            let timeout = BaoTimeoutObject::from_timer_ptr(t);
            if (*timeout).event_loop_timer.state != TimerState::FIRED {
                // SAFETY: `now` is non-null per dispatch contract (caller
                // passes a live timespec snapshot from the heap pop path).
                let now_ref = unsafe { &*now };
                // P1-A.3c-step4: dispatch JS callback if a JSContext is
                // registered on this thread. Falls back to state-only fire
                // when no cx is available (e.g. during pure-Rust drain
                // before runtime initialization, or unit tests).
                let raw_cx = crate::timers::current_cx();
                if raw_cx.is_null() {
                    (*timeout).fire(now_ref);
                } else {
                    // SAFETY: current_cx() returns a live JSContext* set by
                    // drain_and_check on entry. callback/args are rooted by
                    // the schedule→fire no-GC window (same invariant as
                    // legacy TimerEntry callback dispatch).
                    unsafe { (*timeout).fire_js(raw_cx, now_ref) };
                }
            }
        }
        _ => {}
    }
}

/// Get the JS-timer epoch for heap ordering.
///
/// # Safety
/// `t` must be the `event_loop_timer` field of a `BaoTimeoutObject`.
#[unsafe(no_mangle)]
pub unsafe extern "Rust" fn __bun_js_timer_epoch(
    tag: Tag,
    t: *const EventLoopTimer,
) -> Option<u32> {
    match tag {
        Tag::TimeoutObject | Tag::ImmediateObject => {
            // SAFETY: `t` is `*const` but `from_timer_ptr` takes `*mut`; cast
            // away constness is safe because we only read `epoch` (no write)
            // and the caller contract guarantees the parent object is live.
            let timeout = BaoTimeoutObject::from_timer_ptr(t as *mut EventLoopTimer);
            Some((*timeout).epoch)
        }
        _ => None,
    }
}
