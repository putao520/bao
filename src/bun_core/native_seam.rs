// @trace STUB-INVENTORY: C-seam RealImpl rehomed to bun_core (named owner)
//! `#[no_mangle]` C-seam symbols whose Rust declarers live in this crate
//! (`output` / `util` / `string`) or in crates above it (`bun_sys`,
//! `bun_crash_handler`, `bun_alloc`). bun_core is the lowest crate on the
//! dep graph that touches these seams, so each symbol has exactly ONE
//! definition here, valid for every link scope:
//!
//! - `bun_core --lib` tests: self-provided, so the crate no longer needs a
//!   `bao_native_stubs` dev-dependency — that dev-dep cycled back to the
//!   bun_core rlib and dual-defined the crate's own `#[no_mangle]` symbols
//!   (`Bun__atexit`, `Bun__onExit`, …) in the test binary.
//! - test binaries linking `bao_native_stubs`: former def site, now deleted
//!   (STUB-INVENTORY dual-def iron rule).
//! - the product binary: former def site
//!   `bun_runtime::product_native_symbols`, now deleted.
//!
//! Do NOT reintroduce copies in `bao_native_stubs` or
//! `bun_runtime::product_native_symbols`.
//!
//! `posix_spawn_bun` lives in `util::spawn_ffi` next to its request structs.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_char, c_int, c_void};

// ── process stdio (declarer: `output`) ────────────────────────────────────

/// Minimal process stdio init; nothing to set up beyond what std provides.
#[unsafe(no_mangle)]
pub extern "C" fn bun_initialize_process() {}

/// Best-effort flush of stdout/stderr before any fd restoration.
#[unsafe(no_mangle)]
pub extern "C" fn bun_restore_stdio() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

/// Best-effort `sync()` before `exec` on Linux reload (declarer: `util`).
#[unsafe(no_mangle)]
pub extern "C" fn on_before_reload_process_linux() {
    // SAFETY: `sync()` only flushes filesystem buffers; no preconditions.
    unsafe {
        libc::sync();
    }
}

// ── stack check (declarer: `util::StackCheck`) ────────────────────────────

/// Thread stack bounds are resolved lazily via `getMaxStack`.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__StackCheck__initialize() {}

/// Return the stack-end pointer for the current thread (pthread_attr path,
/// 8 MiB-from-here fallback).
#[unsafe(no_mangle)]
pub extern "C" fn Bun__StackCheck__getMaxStack() -> *mut c_void {
    unsafe {
        let mut attr: libc::pthread_attr_t = core::mem::zeroed();
        if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) == 0 {
            let mut stack_addr: *mut c_void = core::ptr::null_mut();
            let mut stack_size: usize = 0;
            if libc::pthread_attr_getstack(&attr, &mut stack_addr, &mut stack_size) == 0 {
                libc::pthread_attr_destroy(&mut attr);
                return (stack_addr as usize + stack_size) as *mut c_void;
            }
            libc::pthread_attr_destroy(&mut attr);
        }
        let marker: usize = 0;
        (marker as *const usize as usize + 8 * 1024 * 1024) as *mut c_void
    }
}

// ── crash / stack dump (declarer: `bun_crash_handler`) ────────────────────

/// ABI SSOT: `bun_crash_handler` — `(ptr, count)` instruction addresses.
/// When frames are provided, print them; otherwise capture a live backtrace.
/// @trace STUB-INVENTORY: WTF__DumpStackTrace RealImpl
#[unsafe(no_mangle)]
pub extern "C" fn WTF__DumpStackTrace(ptr: *const usize, count: usize) {
    if !ptr.is_null() && count > 0 {
        // SAFETY: caller provides `count` valid instruction addresses.
        let frames = unsafe { core::slice::from_raw_parts(ptr, count) };
        for (i, addr) in frames.iter().enumerate() {
            eprintln!("  #{i:2} {addr:#x}");
        }
    } else {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("{bt}");
    }
}

// ── CPU features (declarer: `bun_crash_handler::CPUFeatures`) ─────────────

#[unsafe(no_mangle)]
pub extern "C" fn bun_cpu_features() -> u64 {
    let mut flags: u64 = 0;
    flags |= 1 << 1; // SSE2 (guaranteed on x86_64)
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            flags |= 1 << 5;
        }
        if is_x86_feature_detected!("sse4.2") {
            flags |= 1 << 3;
        }
    }
    flags
}

// ── executable probe (declarer: `bun_sys`) ────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn is_executable_file(path: *const c_char) -> bool {
    if path.is_null() {
        return false;
    }
    unsafe {
        let mut st: libc::stat = core::mem::zeroed();
        if libc::stat(path, &mut st) != 0 {
            return false;
        }
        (st.st_mode & libc::S_IXUSR) != 0
    }
}

// ── IP literal parse (declarers: `strings`, `string::immutable`) ──────────
// Pure-Rust mirror of c-ares' `ares_inet_pton` (spec: immutable.zig:1984).
// Defined here (not linked from libcares.a) so targets that do not chain the
// c-ares archive still resolve the seam.

#[unsafe(no_mangle)]
pub extern "C" fn ares_inet_pton(af: c_int, src: *const c_char, dst: *mut c_void) -> c_int {
    if src.is_null() || dst.is_null() {
        return 0;
    }
    unsafe {
        let cstr = core::ffi::CStr::from_ptr(src);
        let s = match cstr.to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };
        match af {
            2 /* AF_INET */ => match s.parse::<std::net::Ipv4Addr>() {
                Ok(addr) => {
                    let octets = addr.octets();
                    core::ptr::copy_nonoverlapping(octets.as_ptr(), dst as *mut u8, 4);
                    1
                }
                Err(_) => 0,
            },
            10 /* AF_INET6 */ => match s.parse::<std::net::Ipv6Addr>() {
                Ok(addr) => {
                    let octets = addr.octets();
                    core::ptr::copy_nonoverlapping(octets.as_ptr(), dst as *mut u8, 16);
                    1
                }
                Err(_) => 0,
            },
            _ => 0,
        }
    }
}

// ── BunString construction (declarer: `string`) ───────────────────────────

/// RealImpl via `String::from_bytes` (Latin1/UTF-8 detection).
/// @trace STUB-INVENTORY: BunString__fromBytes RealImpl
#[unsafe(no_mangle)]
pub extern "C" fn BunString__fromBytes(bytes: *const u8, len: usize) -> crate::String {
    if bytes.is_null() || len == 0 {
        return crate::String::EMPTY;
    }
    // SAFETY: caller provides valid `bytes`/`len` for the duration of this call.
    let slice = unsafe { core::slice::from_raw_parts(bytes, len) };
    crate::String::from_bytes(slice)
}

/// Dead / safe-noop-by-design: cannot free arbitrary WTF heap without the
/// full refcount owner; callers mostly use ZigString/DEAD tags. A fake free
/// would double-free. Not a product Partial residual.
/// @trace STUB-INVENTORY: Bun__WTFStringImpl__destroy Dead/safe-noop-by-design
#[unsafe(no_mangle)]
pub extern "C" fn Bun__WTFStringImpl__destroy(this: *const c_void) {
    if this.is_null() {
        return;
    }
    let _ = this;
}
