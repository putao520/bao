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
    #[cfg(not(windows))]
    unsafe {
        libc::sync();
    }
    // Windows has no process-global fs-buffer sync: `sync(2)` has no Win32
    // equivalent (`FlushFileBuffers` is per-handle and this best-effort seam
    // carries no handles). The closest CRT primitive — flushing every open
    // stream — stands in for the best-effort contract; the kernel write-back
    // portion is deferred to the NT cache manager by platform design.
    #[cfg(windows)]
    {
        unsafe extern "C" {
            safe fn _flushall() -> c_int;
        }
        let _ = _flushall();
    }
}

// ── stack check (declarer: `util::StackCheck`) ────────────────────────────

/// Thread stack bounds are resolved lazily via `getMaxStack`.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__StackCheck__initialize() {}

/// Return the stack-end pointer for the current thread (pthread_attr path,
/// 8 MiB-below-here fallback).
///
/// "End" is WTF `StackBounds::end()` — the LOW bound (the limit the stack
/// grows down toward), NOT the high `origin()`. `StackCheck::
/// is_safe_to_recurse()` measures `rsp - end`, so returning the high address
/// inverts the check and makes every recursion probe fail at depth 0
/// (surfaces as `error.StackOverflow` from the JSON parser before it reads
/// a single token).
#[unsafe(no_mangle)]
pub extern "C" fn Bun__StackCheck__getMaxStack() -> *mut c_void {
    // macOS: `pthread_getattr_np`/`pthread_attr_getstack` are glibc-only;
    // the Apple equivalents are `pthread_get_stackaddr_np` (the ORIGIN, high
    // end) minus `pthread_get_stacksize_np` — the same bounds formula std
    // (`std::sys::pal::unix::stack_overflow::get_stack_start`) and WTF
    // `StackBounds` use on Darwin.
    #[cfg(target_os = "macos")]
    unsafe {
        let th = libc::pthread_self();
        let origin = libc::pthread_get_stackaddr_np(th) as usize;
        let size = libc::pthread_get_stacksize_np(th);
        (origin.wrapping_sub(size)) as *mut c_void
    }
    // Windows: `GetCurrentThreadStackLimits` (kernel32, Win8+) hands back the
    // calling thread's stack region [low, high]; the low limit is the same
    // down-growing "end" the pthread paths return. The API has no failure
    // mode (void return, always writes both outs), so the marker fallback
    // below has no Windows arm.
    #[cfg(windows)]
    {
        // Local extern so bun_core stays leaf (no windows-sys dep); kernel32
        // is in every Rust windows target's default link set.
        unsafe extern "system" {
            safe fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
        }
        let mut low: usize = 0;
        let mut high: usize = 0;
        GetCurrentThreadStackLimits(&mut low, &mut high);
        low as *mut c_void
    }
    // Linux (glibc): pthread_attr path; `stack_addr` is already the low
    // bound on down-growing stacks — do NOT add `stack_size` (the origin).
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    unsafe {
        let mut attr: libc::pthread_attr_t = core::mem::zeroed();
        if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) == 0 {
            let mut stack_addr: *mut c_void = core::ptr::null_mut();
            let mut stack_size: usize = 0;
            if libc::pthread_attr_getstack(&attr, &mut stack_addr, &mut stack_size) == 0 {
                libc::pthread_attr_destroy(&mut attr);
                return stack_addr;
            }
            libc::pthread_attr_destroy(&mut attr);
        }
        let marker: usize = 0;
        (marker as *const usize as usize - 8 * 1024 * 1024) as *mut c_void
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
        // Windows has no mode bits; MSVCRT `_stat` sets `S_IEXEC` from the
        // file extension — the platform's own executable predicate — so the
        // query is semantics-equal to the POSIX owner-bit check.
        #[cfg(windows)]
        let exec_bit = libc::S_IEXEC as u32;
        #[cfg(not(windows))]
        let exec_bit = libc::S_IXUSR as u32;
        ((st.st_mode as u32) & exec_bit) != 0
    }
}

// ── IP literal parse (declarers: `strings`, `string::immutable`) ──────────
// Pure-Rust mirror of c-ares' `ares_inet_pton` (spec: immutable.zig:1984).
// Defined here (not linked from libcares.a) so targets that do not chain the
// c-ares archive still resolve the seam.
//
// windows: the c-ares archive EXISTS (bun_cares_sys compiles the vendored
// c-ares into cares.lib) — a no_mangle mirror here would be a duplicate
// symbol at the final link, so the seam name resolves to the real c-ares
// export through a wrapper instead (same safe face for every caller).
#[cfg(windows)]
pub fn ares_inet_pton(af: c_int, src: *const c_char, dst: *mut c_void) -> c_int {
    unsafe extern "C" {
        #[link_name = "ares_inet_pton"]
        fn c_ares_inet_pton(af: c_int, src: *const c_char, dst: *mut c_void) -> c_int;
    }
    // SAFETY: pure C address parser, no preconditions.
    unsafe { c_ares_inet_pton(af, src, dst) }
}

#[cfg(not(windows))]
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
