// @trace REQ-ENG-001
//! C→Rust callback hooks required by the compiled uSockets C library.
//!
//! These symbols are referenced by libusockets.a (loop.c, epoll_kqueue.c)
//! and must be available to any binary that links against bun_uws_sys.
//! Placing them here ensures they're co-located with the C code that needs them,
//! avoiding link-order issues.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_int, c_void};

// Dual-def rule (full product path always co-links higher crates):
//   - `Bun__lock__size`              → real export in `bun_threading::Mutex`
//   - `Bun__isEpollPwait2SupportedOnLinuxKernel` → real export in `bun_analytics`
// Do NOT redefine them here. libusockets.a resolves them from those crates when
// the product graph (bun_runtime → bun_install / bun_threading / analytics) is
// linked. Tier-0-only consumers that need a stand-in must dep those crates or
// provide their own single definition — dual-def breaks gsc-frog-tools lib test.

/// Fatal panic from C. Called by uSockets on unrecoverable errors.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__panic(msg: *const u8, len: usize) -> ! {
    let msg_str = if msg.is_null() || len == 0 {
        "(no message)".to_string()
    } else {
        let slice = unsafe { core::slice::from_raw_parts(msg, len) };
        String::from_utf8_lossy(slice).into_owned()
    };
    eprintln!("Bun__panic from C: {}", msg_str);
    std::process::abort()
}

/// Reports "Bun ran out of memory" through the crash handler and aborts.
/// Absorbed from oven-sh/bun 4af1842c8c: usockets calls this for allocations
/// it cannot fail gracefully from (loop receive/send buffers). Mirrors the
/// `Bun__panic` shape above — bao's crash-handler surface is the process
/// abort path.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__outOfMemory() -> ! {
    eprintln!("Bun ran out of memory (usockets allocation)");
    std::process::abort()
}

/// Linux epoll_pwait2 syscall wrapper. Used by libusockets.a's epoll_kqueue.c
/// (epoll half only — the kqueue half compiles under LIBUS_USE_KQUEUE and never
/// references this symbol, so it is gated out on non-linux where libc has no
/// epoll_event / SYS_epoll_pwait2).
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "C" fn sys_epoll_pwait2(
    epfd: c_int,
    events: *mut libc::epoll_event,
    maxevents: c_int,
    timeout: *const libc::timespec,
    sigmask: *const libc::sigset_t,
) -> isize {
    unsafe {
        libc::syscall(
            libc::SYS_epoll_pwait2,
            epfd as isize as usize,
            events as usize,
            maxevents as isize as usize,
            timeout as usize,
            sigmask as usize,
            8usize,
        ) as isize
    }
}

/// JSC VM pre-wait hook. No-op for SpiderMonkey integration.
/// Absorbed from oven-sh/bun 4af1842c8c: the callback grew a `now_ns`
/// argument (the loop's absolute monotonic now, shared so the VM's own
/// timers need no extra clock read). Ignored here for the same reason.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__JSC_onBeforeWait(_jsc_vm: *mut c_void, _now_ns: u64) {}

// Absorbed from oven-sh/bun 4af1842c8c: upstream's mimalloc fork exposes a
// thread-idle scavenger hand-off that the event loop offers its kernel wait
// to (epoll_kqueue.c's `mi_on_thread_idle_start/idle/idle_end`). Bao's
// vendored mimalloc (vendor/mimalloc) predates that API — supply the no-op
// shape so the loop takes its own inline rate-limited sweep path
// (`mi_on_thread_idle_start` returning 0 is exactly upstream's "scavenger
// declined" outcome). mimalloc's own deferred-free pacing is untouched.

/// Offer the loop's kernel wait to mimalloc's scavenger. Returns 1 when the
/// sweep was handed off (and `mi_on_thread_idle_end` must be called after the
/// wait), 0 when the caller should keep ownership.
#[unsafe(no_mangle)]
pub extern "C" fn mi_on_thread_idle_start() -> c_int {
    0
}

/// Inline sweep fallback for a thread that really parked in the loop.
#[unsafe(no_mangle)]
pub extern "C" fn mi_on_thread_idle() {}

/// Matching hand-off end for a `mi_on_thread_idle_start` that returned 1.
#[unsafe(no_mangle)]
pub extern "C" fn mi_on_thread_idle_end() {}

// Absorbed from oven-sh/bun 4af1842c8c: upstream's node:quic loop driver
// (node_quic_shim.c — not compiled here, bao's frozen lsquic mirror predates
// its lsquic face) registers endpoints on the loop via `nq_head`; loop.c
// flushes pending driver work unconditionally around each poll. Nothing
// registers without the shim TU, so the list is always empty and the flush
// is a no-op.

/// Walk the loop's node:quic driver list, flushing endpoints with pending work.
#[unsafe(no_mangle)]
pub extern "C" fn us_nq_loop_flush_if_pending(_loop: *mut c_void) {}
