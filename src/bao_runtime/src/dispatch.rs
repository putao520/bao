// @trace REQ-ENG-004: Timer dispatch for SpiderMonkey
//! Link-time extern implementations for `EventLoopTimer` and `FilePoll`.
//!
//! In upstream Bun, `bun_runtime::dispatch` contains the full ~96-variant
//! task dispatcher, the ~13-variant FilePoll dispatcher, and timer dispatch.
//! Bao only uses SpiderMonkey (no JSC), so the full task/poll dispatch tables
//! are unnecessary — we only bridge the timer dispatch. The FilePoll dispatch
//! (`__bun_run_file_poll`) is provided as a no-op stub here because our
//! runtime does not implement the Bun-specific poll-tag dispatch vtable;
//! `bun_io::posix_event_loop` declares it `extern "Rust"` and expects it at
//! link time.

use bun_core::Timespec;
use bun_event_loop::EventLoopTimer::{
    EventLoopTimer, State as TimerState, Tag,
};

#[cfg(not(windows))]
use bun_io::posix_event_loop::FilePoll;

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

// ──────────────────────────────────────────────────────────────
// FilePoll dispatch stub (POSIX-only)
// ──────────────────────────────────────────────────────────────

/// No-op stub for `bun_io::FilePoll::on_update` dispatch.
///
/// In upstream Bun, `bun_runtime::dispatch::__bun_run_file_poll` contains the
/// full ~13-variant poll-tag match (BufferedReader, Process, FileSink, DNS,
/// etc.). Bao does not use these Bun-specific poll owners — our I/O goes
/// through SpiderMonkey + bao_uloop. This stub satisfies the `extern "Rust"`
/// link-time reference from `bun_io::posix_event_loop::FilePoll::on_update`.
///
/// # Safety
/// `poll` must point at a live `FilePoll` per the caller contract.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub unsafe extern "Rust" fn __bun_run_file_poll(
    _poll: *mut FilePoll,
    _size_or_offset: i64,
) {
    // No-op: Bao does not implement Bun's poll-tag dispatch vtable.
    // FilePoll::on_update calls this when the owner tag is non-NULL;
    // with a no-op, the poll is effectively disconnected after the
    // first event, which is safe for our use case (we don't use
    // Bun's FilePoll for I/O).
}
