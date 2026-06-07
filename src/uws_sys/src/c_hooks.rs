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

/// Mutex size exported for uSockets loop.c validation.
/// `loop.c` checks `Bun__lock__size == sizeof(loop->data.mutex)` at init.
/// On Linux, `zig_mutex_t = uint32_t` (4 bytes) — a userspace spinlock, not
/// pthread_mutex_t. The C struct `us_internal_loop_data_t.mutex` is `zig_mutex_t`.
#[unsafe(no_mangle)]
pub static Bun__lock__size: usize = core::mem::size_of::<u32>();

/// epoll_pwait2 kernel support check. Returns 1 (Linux 5.11+ always supports it).
#[unsafe(no_mangle)]
pub extern "C" fn Bun__isEpollPwait2SupportedOnLinuxKernel() -> c_int {
    1
}

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
