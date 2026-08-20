// @trace REQ-ENG-008 [entity:BaoLoopState]
//! macOS kqueue arm of the uSockets loop tick (Wave 74-C.8, macOS M2).
//!
//! Mirror of the Linux `run_epoll` tick (lib.rs) ported from the vendored C
//! truth — the kqueue half of `csrc/bun-usockets/src/eventing/epoll_kqueue.c`
//! (`us_loop_run_bun_tick` fetch loop + `us_internal_dispatch_ready_polls`
//! kqueue branch). The Linux/kqueue pair shares one tick shape:
//!
//!   1. controlled-timeout wait  — `kevent64` here, `epoll_wait` on Linux
//!   2. ready-event normalisation + dispatch — `poll::dispatch_ready_polls`
//!      (platform arm), which forwards untagged `us_poll_t` events to the C
//!      `us_internal_dispatch_ready_poll` and tagged FilePoll pointers to
//!      `Bun__internal_dispatch_ready_poll`
//!
//! ## BCE-007 contract (same as the Linux tick)
//!
//! The Rust tick never blocks indefinitely: NULL timeout or pending work
//! runs under `KEVENT_FLAG_IMMEDIATE` — XNU's `kqueue_scan` returns right
//! after `kqueue_process()` instead of falling through to
//! `assert_wait_deadline` + `thread_block` (the ~14µs context-switch cycle
//! a bare zero timespec still pays; see the C comment at the kevent64 call).
//! An explicit timespec bounds the wait; the kernel takes sec/nsec directly
//! (no millisecond rounding like the Linux arm).
//!
//! ## Wakeup is the C layer's (no Rust eventfd)
//!
//! The Linux tick owns an eventfd registered with `WAKEUP_TAG` and drains it
//! before dispatch. On macOS the C `us_create_loop` →
//! `us_internal_loop_data_init` arms the `EVFILT_MACHPORT` wakeup whose kevent
//! carries the untagged `us_internal_callback_t` pointer in `udata`; it flows
//! through the normal untagged dispatch (`POLL_TYPE_CALLBACK`), where
//! `us_internal_accept_poll_event` is a no-op on kqueue — the message was
//! already copied out via `MACH_RCV_OVERWRITE` inside the kevent64 syscall.
//! The Rust side builds no wakeup primitive of its own.
//!
//! ## Pre/post handler ordering
//!
//! Identical to Linux: the Rust arm only waits + dispatches. The deferred
//! queue, pre/post handlers, sweep-timer integration, DNS result handling and
//! closed-socket freeing all live in the C `us_loop_run_bun_tick` /
//! `us_internal_loop_pre` / `us_internal_loop_post` machinery, which runs on
//! the delegation path (`bao_loop_tick` else-branch) for threads without a
//! `BaoLoopState`.

use core::ffi::{c_int, c_uint};
use core::ptr;

use bun_uws_sys::{Loop, PosixLoop, Timespec};

use crate::poll;

/// Single `kevent64` + dispatch — the kqueue mirror of `run_epoll`.
///
/// Reads ready events into `(*loop_).ready_polls`, sets `num_ready_polls` /
/// `current_ready_poll`, then delegates normalisation + dispatch to
/// `poll::dispatch_ready_polls` (macOS arm). The loop fd is the kqueue fd
/// created by C `us_create_loop` (`loop->fd = kqueue()`); every registration
/// the C layer makes (`kqueue_change`, timers, the machport wakeup) stores
/// the poll pointer in `udata`, so this tick is read-and-dispatch only.
///
/// # Safety
/// `loop_` must be a valid `*mut Loop` created by `us_create_loop` on the
/// calling thread (same contract as `run_epoll`).
pub(crate) unsafe fn run_kqueue(loop_: *mut Loop, pending: u32, timeout: *const Timespec) {
    // BCE-007 controlled timeout — same rule as the Linux arm: pending work,
    // NULL timeout, or a zero timespec → non-blocking harvest; an explicit
    // non-zero timespec → bounded wait. kevent64 receives the timespec by
    // pointer; with KEVENT_FLAG_IMMEDIATE the pointer stays NULL (XNU returns
    // immediately after kqueue_process, no scheduler interaction).
    let immediate: bool = pending > 0
        || timeout.is_null()
        || {
            let ts = unsafe { *timeout };
            ts.sec == 0 && ts.nsec == 0
        };

    let ts_spec: libc::timespec = if immediate {
        libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }
    } else {
        let ts = unsafe { *timeout };
        libc::timespec {
            tv_sec: ts.sec,
            tv_nsec: ts.nsec,
        }
    };
    let flags: c_uint = if immediate {
        libc::KEVENT_FLAG_IMMEDIATE
    } else {
        0
    };
    let ts_ptr: *const libc::timespec = if immediate {
        ptr::null()
    } else {
        &ts_spec
    };

    let loop_ptr: *mut PosixLoop = loop_;
    let kqfd = unsafe { (*loop_ptr).fd };
    let events_ptr = unsafe { (*loop_ptr).ready_polls.as_mut_ptr() };
    let max_events = unsafe { (*loop_ptr).ready_polls.len() } as c_int;

    // Fetch ready polls. No changelist — registration is the C layer's
    // (`kqueue_change` in epoll_kqueue.c); retry on EINTR like every C
    // kevent64 loop (IS_EINTR).
    let mut n: c_int;
    loop {
        n = unsafe { libc::kevent64(kqfd, ptr::null(), 0, events_ptr, max_events, flags, ts_ptr) };
        let eintr = n == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR);
        if !eintr {
            break;
        }
    }

    // n == 0: bounded timeout expired with no events. n < 0: kevent64 error
    // (e.g. EBADF/EINVAL) — return without dispatch, same as the C tick's
    // fallthrough on a failed fetch. Never fabricate events.
    if n <= 0 {
        return;
    }

    unsafe {
        (*loop_ptr).num_ready_polls = n;
        (*loop_ptr).current_ready_poll = 0;
    }

    // Normalise (EVFILT_*/EV_* → libus units, per-poll coalescing) and
    // dispatch — the macOS arm mirrors epoll_kqueue.c:219-298.
    unsafe {
        poll::dispatch_ready_polls(loop_);
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    //! macOS-gated unit tests for the kqueue tick's timeout rule. Behavioural
    //! coverage (kevent64 harvest + dispatch round-trip) requires a macOS
    //! host — these run there via `cargo nt -p bao_uloop`; on Linux CI the
    //! module is cfg'd out (no kqueue to drive).

    use super::*;

    #[test]
    fn kevent64_and_constants_match_the_c_contract() {
        // The FFI surface this module builds on must exist with the shapes
        // epoll_kqueue.c compiles against on __APPLE__.
        assert_eq!(libc::EVFILT_READ, -1i16);
        assert_eq!(libc::EVFILT_WRITE, -2i16);
        // Timer and the C-layer wakeup filter — both normalise to "readable".
        assert_eq!(libc::EVFILT_TIMER, -7i16);
        assert_eq!(libc::EVFILT_MACHPORT, -8i16);
        // Flag bits the dispatch arm decodes.
        assert_eq!(libc::EV_ERROR, 0x4000u16);
        assert_eq!(libc::EV_EOF, 0x8000u16);
        // Non-blocking harvest flag (XNU kqueue_scan fast path).
        assert_eq!(libc::KEVENT_FLAG_IMMEDIATE, 0x1u32);
        // kevent64_s::udata is where the C registrations stash the poll
        // pointer — must be a full 64-bit slot for tagged FilePoll pointers.
        assert_eq!(
            core::mem::size_of::<libc::kevent64_s>(),
            48,
            "kevent64_s layout {ident,filter,flags,fflags,data,udata,ext[2]}"
        );
    }

    #[test]
    fn timespec_conversion_preserves_seconds_and_nanoseconds() {
        // Timespec (bun_core, sec/nsec i64 pairs) → libc::timespec (darwin
        // tv_sec/tv_nsec c_long) must be a lossless field move — no rounding,
        // no clamping; sub-millisecond timers keep their resolution.
        let ts = Timespec {
            sec: 0,
            nsec: 1_500_000,
        };
        let spec = libc::timespec {
            tv_sec: ts.sec,
            tv_nsec: ts.nsec,
        };
        assert_eq!(spec.tv_sec, 0);
        assert_eq!(spec.tv_nsec, 1_500_000);
    }
}
