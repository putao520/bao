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

/// Linux epoll_pwait2 syscall wrapper. Used by libusockets.a's epoll_kqueue.c.
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
#[unsafe(no_mangle)]
pub extern "C" fn Bun__JSC_onBeforeWait(_jsc_vm: *mut c_void) {}
