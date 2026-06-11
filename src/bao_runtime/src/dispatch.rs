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
// FilePoll dispatch (POSIX-only)
// ──────────────────────────────────────────────────────────────

/// FilePoll owner dispatch — routes `on_update` to the correct handler
/// based on the `PollTag` stored in `FilePoll.owner`.
///
/// In upstream Bun this is a ~13-variant match. Bao implements the variants
/// that are actually used: BufferedReader (file I/O), Process (child process
/// waitpid), and falls through silently for unused tags.
///
/// # Safety
/// `poll` must point at a live `FilePoll` per the caller contract,
/// and `owner.ptr` must be a valid pointer of the type indicated by `owner.tag`.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub unsafe extern "Rust" fn __bun_run_file_poll(
    poll: *mut FilePoll,
    size_or_offset: i64,
) {
    use bun_io::posix_event_loop::{PollTag, poll_tag};

    if poll.is_null() {
        return;
    }
    let poll_ref = unsafe { &mut *poll };
    let owner = poll_ref.owner;
    let hup = poll_ref.flags.contains(bun_io::posix_event_loop::Flags::Hup);

    match owner.tag() {
        poll_tag::BUFFERED_READER => {
            let reader = owner.ptr.cast::<bun_io::BufferedReader>();
            unsafe {
                bun_io::BufferedReader::on_poll(&mut *reader, size_or_offset as isize, hup);
            }
        }
        poll_tag::PROCESS => {
            let proc = owner.ptr.cast::<bun_spawn::process::Process>();
            unsafe {
                bun_spawn::process::Process::on_wait_pid_from_event_loop_task(proc);
            }
        }
        // Tags not yet used in Bao — safe no-op (the FilePoll will just
        // not deliver callbacks for these owner types until implemented).
        poll_tag::NULL
        | poll_tag::FILE_SINK
        | poll_tag::STATIC_PIPE_WRITER
        | poll_tag::SHELL_STATIC_PIPE_WRITER
        | poll_tag::SECURITY_SCAN_STATIC_PIPE_WRITER
        | poll_tag::DNS_RESOLVER
        | poll_tag::GET_ADDR_INFO_REQUEST
        | poll_tag::REQUEST
        | poll_tag::SHELL_BUFFERED_WRITER
        | poll_tag::TERMINAL_POLL
        | poll_tag::PARENT_DEATH_WATCHDOG
        | poll_tag::LIFECYCLE_SCRIPT_SUBPROCESS_OUTPUT_READER => {}
    }
}
