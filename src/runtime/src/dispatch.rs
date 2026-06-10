// @trace REQ-ENG-004, REQ-ENG-011
//! Link-time extern implementations for `EventLoopTimer` and `FilePoll`.
//!
//! In upstream Bun, `bun_runtime::dispatch` contains the full ~96-variant
//! task dispatcher, the ~13-variant FilePoll dispatcher, and timer dispatch.
//! Bao only uses SpiderMonkey (no JSC), so the full task/poll dispatch tables
//! are unnecessary — we only bridge the timer dispatch and the FilePoll
//! dispatch for variants that exist in Bao.

use bun_core::Timespec;
use bun_event_loop::EventLoopTimer::{
    EventLoopTimer, State as TimerState, Tag,
};

#[cfg(not(windows))]
use bun_io::posix_event_loop::FilePoll;
#[cfg(not(windows))]
use bun_io::posix_event_loop::poll_tag;

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
                let now_ref = unsafe { &*now };
                let raw_cx = crate::timers::current_cx();
                if raw_cx.is_null() {
                    (*timeout).fire(now_ref);
                } else {
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
            let timeout = BaoTimeoutObject::from_timer_ptr(t as *mut EventLoopTimer);
            Some((*timeout).epoch)
        }
        _ => None,
    }
}

// ──────────────────────────────────────────────────────────────
// FilePoll dispatch (POSIX-only)
// ──────────────────────────────────────────────────────────────

/// Dispatch FilePoll events based on the owner tag.
///
/// This is the Bao equivalent of Bun's ~13-variant FilePoll dispatch.
/// Only tags that exist in Bao are handled; others fall through to no-op.
///
/// # Safety
/// `poll` must point at a live `FilePoll` per the caller contract.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub unsafe extern "Rust" fn __bun_run_file_poll(
    poll: *mut FilePoll,
    size_or_offset: i64,
) {
    if poll.is_null() { return; }

    let poll_ref = unsafe { &mut *poll };
    let owner = poll_ref.owner;

    if owner.is_null() { return; }

    use bun_io::posix_event_loop::Flags as PollFlag;
    let hup = poll_ref.flags.contains(PollFlag::Hup);

    match owner.tag() {
        poll_tag::BUFFERED_READER => {
            // SAFETY: tag matched, so `owner.ptr` is a live `*mut PosixBufferedReader`
            // set at FilePoll::init; exclusive for this dispatch.
            let reader = owner.ptr.cast::<bun_io::pipe_reader::PosixBufferedReader>();
            unsafe {
                bun_io::pipe_reader::PosixBufferedReader::on_poll(
                    &mut *reader,
                    size_or_offset as isize,
                    hup,
                )
            };
        }

        poll_tag::PROCESS => {
            // SAFETY: `proc` carries the +1 ref taken at queue time; this drops it.
            let proc = owner.ptr.cast::<bun_spawn::Process>();
            unsafe { bun_spawn::Process::on_wait_pid_from_event_loop_task(proc) };
        }

        poll_tag::PARENT_DEATH_WATCHDOG => {
            // macOS-only in Bun (kqueue EVFILT_PROC); Linux uses prctl(PR_SET_PDEATHSIG).
            #[cfg(target_os = "macos")]
            {
                let wd = owner.ptr.cast::<bun_io::parent_death_watchdog::ParentDeathWatchdog>();
                unsafe { bun_io::parent_death_watchdog::on_parent_exit(&mut *wd) };
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = hup;
            }
        }

        // Tags not yet ported to Bao — no-op dispatch (safe: the owner
        // will remain registered and the event loop will retry on next
        // wakeup, or the fd will be closed by the parent object's Drop).
        poll_tag::FILE_SINK
        | poll_tag::STATIC_PIPE_WRITER
        | poll_tag::SHELL_STATIC_PIPE_WRITER
        | poll_tag::SECURITY_SCAN_STATIC_PIPE_WRITER
        | poll_tag::SHELL_BUFFERED_WRITER
        | poll_tag::DNS_RESOLVER
        | poll_tag::GET_ADDR_INFO_REQUEST
        | poll_tag::REQUEST
        | poll_tag::TERMINAL_POLL
        | poll_tag::LIFECYCLE_SCRIPT_SUBPROCESS_OUTPUT_READER => {
            // No-op: tag variant not yet ported to Bao.
        }

        poll_tag::NULL => {
            let _ = (size_or_offset, hup);
        }
    }
}
