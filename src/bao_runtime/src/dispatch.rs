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
                (*timeout).fire(now);
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
            let timeout = BaoTimeoutObject::from_timer_ptr(t);
            Some((*timeout).epoch)
        }
        _ => None,
    }
}
