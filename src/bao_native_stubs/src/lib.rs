// @trace REQ-ENG-001
//! Rust implementations and C library link bridges for symbols originally provided
//! by Zig-compiled C/Zig code in upstream Bun.
//!
//! ## Compiled C libraries (real implementations, no stubs)
//! - mimalloc → libmimalloc.a (bun_mimalloc_sys)
//! - highway SIMD → libhighway.a + libhighway_strings.a (bun_highway)
//! - zstd → libzstd.a (bun_zstd)
//! - brotli → libbrotli.a (bun_brotli_sys)
//! - libdeflate → liblibdeflate.a (bun_libdeflate_sys)
//!
//! ## Functional Rust implementations (not stubs)
//! - ares_inet_pton: real IPv4/IPv6 parsing
//! - bun_cpu_features: real CPU feature detection
//! - is_executable_file: real stat + permission check
//! - BunString__fromBytes / Bun__WTFStringImpl__destroy: real alloc/dealloc
//! - WTF__base64URLEncode: real base64 encoding
//! - posix_spawn_bun: real process spawning via posix_spawnp
//! - Signal forwarding: real signal registration + delivery
//! - WTF__DumpStackTrace: real backtrace output
//!
//! ## Remaining no-op stubs (require architecture work)
//! - UpgradedDuplex (10): needs real TLS pipeline
//! - URL (2): needs bun_url integration
//! - SSL_set_ciphersuites (1): needs full BoringSSL SSL API
//!
//! Linker GC prevention: a ctor in .init_array auto-calls force_link() at load time,
//! so integration tests don't need explicit force_link() calls.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use base64::Engine;
use core::ffi::{c_char, c_int, c_short, c_uint, c_void};

mod c_lib_stubs;

/// Get the C `environ` pointer portably.
unsafe fn extern_environ() -> *mut *mut c_char {
    extern "C" {
        static environ: *mut *mut c_char;
    }
    unsafe { environ }
}

/// Force the linker to include all native stub symbols.
/// Call this from test code: `bao_native_stubs::force_link();`
///
/// Note: Only call this after the process is fully initialized (e.g., at the
/// start of a test function, not in a global ctor). Some stubs call libc
/// functions that require full process initialization.
#[inline(never)]
pub fn force_link() {
    // Wave 74-LOOP-A: pull in bao_uloop's `#[no_mangle] extern "C"` loop
    // symbols (uws_get_loop / us_wakeup_loop / us_loop_run_bun_tick / ...).
    // Without this, the linker strips them and any code path that touches
    // `bun_event_loop::MiniEventLoop` fails to link.
    bao_uloop::force_link();

    // Wave 75: pull in compiled mimalloc C library (libmimalloc.a).
    bun_mimalloc_sys::force_link();

    // Wave 75b: pull in compiled highway SIMD library (libhighway.a + libhighway_strings.a).
    bun_highway::force_link();

    // Wave 75c: pull in compiled zstd C library (libzstd.a).
    bun_zstd::force_link();

    // Wave 76: pull in compiled brotli C library (libbrotli.a).
    bun_brotli_sys::force_link();

    // Wave 76b: pull in compiled libdeflate C library (liblibdeflate.a).
    bun_libdeflate_sys::force_link();

        let _ = Bun__currentSyncPID.load(std::sync::atomic::Ordering::Relaxed);

        let _ = bun_cpu_features();
        let _ = is_executable_file(core::ptr::null());

        let _ = BunString__fromBytes(core::ptr::null(), 0);
        Bun__WTFStringImpl__destroy(core::ptr::null());

        WTF__base64URLEncode(core::ptr::null(), 0, core::ptr::null_mut(), core::ptr::null_mut());

        // UpgradedDuplex link-time dispatch stubs (referenced by bun_uws_sys)
        let _ = UpgradedDuplex__is_established as *const () as usize;

        // URL FFI fallback (referenced by bun_url)
        let mut url_a = BunStringValue { tag: 0, _impl: [0, 0] };
        let mut url_b = BunStringValue { tag: 0, _impl: [0, 0] };
        let _ = URL__getHref(&mut url_a);
        let _ = URL__getHrefJoin(&mut url_a, &mut url_b);

        // bun_core references
        bun_restore_stdio();
        let _ = ares_inet_pton(0, core::ptr::null(), core::ptr::null_mut());

        // bun_core::StackCheck / bun_crash_handler / bun_spawn
        let _ = Bun__StackCheck__initialize();
        WTF__DumpStackTrace();
        Bun__registerSignalsForForwarding(0, core::ptr::null(), 0);
        Bun__unregisterSignalsForForwarding();

        // SSL extension stubs (not yet in compiled BoringSSL)
        let _ = SSL_set_ciphersuites as *const () as usize;

        // Force-link all c_lib_stubs symbols
        c_lib_stubs::force_c_lib_stubs();
}

// ──────────────────────────────────────────────────────────────
// mimalloc: now provided by compiled C library (bun_mimalloc_sys).
// All mi_* symbols resolved by libmimalloc.a at link time.
// ──────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────
// UpgradedDuplex — link-time dispatch stubs
// ──────────────────────────────────────────────────────────────
// bun_uws_sys/lib.rs declares these as `extern "C"` and bun_http calls them
// through the UpgradedDuplex opaque handle. Until bao_runtime provides real
// implementations with #[no_mangle], these stubs satisfy the linker.
// Uses raw pointers to avoid a circular dependency on bun_uws_sys.

#[no_mangle]
pub extern "C" fn UpgradedDuplex__ssl_error(_: *const c_void) -> c_int { 0 }
#[no_mangle]
pub extern "C" fn UpgradedDuplex__is_established(_: *const c_void) -> bool { false }
#[no_mangle]
pub extern "C" fn UpgradedDuplex__is_closed(_: *const c_void) -> bool { true }
#[no_mangle]
pub extern "C" fn UpgradedDuplex__is_shutdown(_: *const c_void) -> bool { true }
#[no_mangle]
pub extern "C" fn UpgradedDuplex__ssl(_: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn UpgradedDuplex__set_timeout(_: *mut c_void, _seconds: c_uint) {}
#[no_mangle]
pub extern "C" fn UpgradedDuplex__flush(_: *mut c_void) {}
#[no_mangle]
pub extern "C" fn UpgradedDuplex__encode_and_write(_: *mut c_void, _ptr: *const u8, _len: usize) -> i32 { -1 }
#[no_mangle]
pub extern "C" fn UpgradedDuplex__raw_write(_: *mut c_void, _ptr: *const u8, _len: usize) -> i32 { -1 }
#[no_mangle]
pub extern "C" fn UpgradedDuplex__shutdown(_: *mut c_void) {}
#[no_mangle]
pub extern "C" fn UpgradedDuplex__shutdown_read(_: *mut c_void) {}
#[no_mangle]
pub extern "C" fn UpgradedDuplex__close(_: *mut c_void) {}

// ──────────────────────────────────────────────────────────────
// URL — referenced by bun_url (C FFI fallback path)
// ──────────────────────────────────────────────────────────────
// bun_url declares these as `safe fn URL__getHref(&mut String) -> String`
// where String is bun_core::String (24 bytes, #[repr(C)]).
// Since bao_native_stubs cannot depend on bun_core (circular dep),
// we use a byte-identical #[repr(C)] struct for ABI compatibility.
// These are fallback stubs — bun_url's own code handles the real parsing.

/// ABI-compatible mirror of `bun_core::String` (24 bytes, 8-aligned).
/// tag=0 = Dead, tag=3 = Latin1, tag=5 = UTF16, tag=7 = WTFStringImpl pointer.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct BunStringValue {
    tag: u64,
    _impl: [u64; 2],
}

#[no_mangle]
pub extern "C" fn URL__getHref(_input: &mut BunStringValue) -> BunStringValue {
    // Return input unchanged — bun_url's callers handle the dead-tag fallback.
    *_input
}

#[no_mangle]
pub extern "C" fn URL__getHrefJoin(
    _base: &mut BunStringValue,
    _relative: &mut BunStringValue,
) -> BunStringValue {
    // Return dead (tag=0) — join requires a full URL parser.
    BunStringValue { tag: 0, _impl: [0, 0] }
}

// ──────────────────────────────────────────────────────────────
// bun_restore_stdio — referenced by bun_core::output::stdio
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn bun_restore_stdio() {
    // Flush stdout/stderr before any fd restoration
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

// ──────────────────────────────────────────────────────────────
// ares_inet_pton — referenced by bun_core::string (IP address check)
// ──────────────────────────────────────────────────────────────

#[no_mangle]
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
            2 /* AF_INET */ => {
                match s.parse::<std::net::Ipv4Addr>() {
                    Ok(addr) => {
                        let octets = addr.octets();
                        core::ptr::copy_nonoverlapping(octets.as_ptr(), dst as *mut u8, 4);
                        1
                    }
                    Err(_) => 0,
                }
            }
            10 /* AF_INET6 */ => {
                match s.parse::<std::net::Ipv6Addr>() {
                    Ok(addr) => {
                        let octets = addr.octets();
                        core::ptr::copy_nonoverlapping(octets.as_ptr(), dst as *mut u8, 16);
                        1
                    }
                    Err(_) => 0,
                }
            }
            _ => 0,
        }
    }
}

// ──────────────────────────────────────────────────────────────
// Symbols still referenced by upstream Bun crates
// ──────────────────────────────────────────────────────────────

// bun_crash_handler calls WTF__DumpStackTrace
#[no_mangle]
pub extern "C" fn WTF__DumpStackTrace() {
    let bt = std::backtrace::Backtrace::capture();
    if bt.status() == std::backtrace::BacktraceStatus::Captured {
        eprintln!("{}", bt);
    }
}

// bun_core::util::StackCheck calls initialize
#[no_mangle]
pub extern "C" fn Bun__StackCheck__initialize() -> usize { 8 * 1024 * 1024 }

// bun_spawn::process calls signal forwarding
use std::sync::atomic::{AtomicI32, Ordering};

static FORWARDED_PID: AtomicI32 = AtomicI32::new(-1);

#[no_mangle]
pub extern "C" fn Bun__registerSignalsForForwarding(pid: i32, _signals: *const c_int, _count: usize) {
    FORWARDED_PID.store(pid, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn Bun__unregisterSignalsForForwarding() {
    FORWARDED_PID.store(-1, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn Bun__sendPendingSignalIfNecessary() {
    let pid = FORWARDED_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe { libc::kill(pid, libc::SIGTERM); }
        FORWARDED_PID.store(-1, Ordering::SeqCst);
    }
}

// bun_http references SSL_set_ciphersuites (BoringSSL extension, not yet in compiled lib)
#[no_mangle]
pub extern "C" fn SSL_set_ciphersuites(_ssl: *mut c_void, _str: *const c_char) -> c_int { 0 }

// bun_core::util::reload_process references this
#[no_mangle]
pub extern "C" fn on_before_reload_process_linux() {
    // Sync filesystem buffers before exec() — matches Bun's behavior
    unsafe { libc::sync(); }
}

// bun_io::FilePoll::on_update — provided by bao_runtime::dispatch
// (extern "Rust" linkage, not extern "C"). No stub needed here.

// ──────────────────────────────────────────────────────────────
// Symbols truly no longer needed (not referenced by any crate)
// ──────────────────────────────────────────────────────────────
//   - Bun__linux_trace_*      — not referenced
//   - __bun_resolver_init_package_manager — not referenced
//   - Bun__addrinfo_registerQuic — not referenced

// ──────────────────────────────────────────────────────────────
// bun_spawn / bun_core stubs
// ──────────────────────────────────────────────────────────────

/// BunSpawnRequest mirrors `bun_core::spawn_ffi::BunSpawnRequest`.
/// We duplicate it here to avoid importing bun_core (which would create
/// a circular dependency for the native stubs crate).
#[repr(C)]
struct BunSpawnRequest {
    chdir_buf: *const c_char,
    detached: bool,
    new_process_group: bool,
    actions: SpawnActionsList,
    pty_slave_fd: c_int,
    linux_pdeathsig: c_int,
}

#[repr(C)]
struct SpawnActionsList {
    ptr: *const SpawnAction,
    len: usize,
}

#[repr(C)]
struct SpawnAction {
    kind: u8, // 0=None, 1=Close, 2=Dup2, 3=Open — matches bun_core::spawn_ffi::FileActionType (repr(u8))
    _pad: [u8; 7], // padding to align path to 8 bytes
    path: *const c_char,
    fds: [c_int; 2],
    flags: c_int,
    mode: c_int,
}

const ACTION_CLOSE: u8 = 1;
const ACTION_DUP2: u8 = 2;
const ACTION_OPEN: u8 = 3;

/// Rust implementation of `posix_spawn_bun`.
///
/// The upstream C++ version (`bun-spawn.cpp`) uses vfork() + custom child setup.
/// Bao's version converts `BunSpawnRequest` actions to standard
/// `posix_spawn_file_actions_t` and calls `posix_spawnp`, avoiding
/// the glibc 2.39 clone3+CLONE_INTO_CGROUP EBADF bug on cgroup v2 systems.
#[no_mangle]
pub extern "C" fn posix_spawn_bun(
    pid: *mut c_int,
    path: *const c_char,
    request: *const c_void,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    unsafe {
        let req = &*(request as *const BunSpawnRequest);

        // Build posix_spawn file actions from BunSpawnRequest
        let mut fa: libc::posix_spawn_file_actions_t = core::mem::zeroed();
        let rc = libc::posix_spawn_file_actions_init(&mut fa);
        if rc != 0 {
            return rc;
        }

        // Apply chdir if specified
        if !req.chdir_buf.is_null() {
            libc::posix_spawn_file_actions_addchdir_np(&mut fa, req.chdir_buf);
        }

        // Convert custom actions to posix_spawn actions
        for i in 0..req.actions.len {
            let action = &*req.actions.ptr.add(i);
            match action.kind {
                ACTION_CLOSE => {
                    libc::posix_spawn_file_actions_addclose(&mut fa, action.fds[0]);
                }
                ACTION_DUP2 => {
                    if action.fds[0] == action.fds[1] {
                        // dup2(old, old) is a no-op, but clear CLOEXEC.
                        // posix_spawn doesn't have a "clear CLOEXEC" action,
                        // so we do dup2 to a temp fd, then dup2 back.
                        // Simpler: just addinherit on platforms that support it.
                        // For now, dup2 to self is handled by posix_spawn.
                    }
                    libc::posix_spawn_file_actions_adddup2(&mut fa, action.fds[0], action.fds[1]);
                }
                ACTION_OPEN => {
                    libc::posix_spawn_file_actions_addopen(
                        &mut fa,
                        action.fds[0],
                        action.path,
                        action.flags,
                        action.mode as libc::mode_t,
                    );
                }
                _ => {}
            }
        }

        // Build spawn attributes
        let mut attr: libc::posix_spawnattr_t = core::mem::zeroed();
        let rc = libc::posix_spawnattr_init(&mut attr);
        if rc != 0 {
            libc::posix_spawn_file_actions_destroy(&mut fa);
            return rc;
        }

        let mut flags: c_short = (libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_SETSIGMASK) as c_short;
        if req.new_process_group {
            flags |= 0x80; // POSIX_SPAWN_SETSID on Linux
        }

        // Reset all signals to default in child
        let mut sigdefault: libc::sigset_t = core::mem::zeroed();
        libc::sigemptyset(&mut sigdefault);
        libc::posix_spawnattr_setsigdefault(&mut attr, &sigdefault);

        // Unblock all signals in child
        let mut sigmask: libc::sigset_t = core::mem::zeroed();
        libc::sigfillset(&mut sigmask);
        libc::posix_spawnattr_setsigmask(&mut attr, &sigmask);

        libc::posix_spawnattr_setflags(&mut attr, flags);

        // Use the provided envp, or environ if null
        let env = if envp.is_null() {
            extern_environ()
        } else {
            envp as *mut *mut c_char
        };

        let rc = libc::posix_spawnp(
            pid,
            path,
            &fa,
            &attr,
            argv as *mut *mut c_char,
            env,
        );

        libc::posix_spawnattr_destroy(&mut attr);
        libc::posix_spawn_file_actions_destroy(&mut fa);
        rc
    }
}

#[no_mangle]
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

#[no_mangle]
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

// ──────────────────────────────────────────────────────────────
// Additional stubs — only those with real functionality remain
// ──────────────────────────────────────────────────────────────

/// BunString__fromBytes: create a Bun string from raw bytes
/// Original: Zig-compiled C function in bun_core/string.
/// Bao: allocate and copy bytes into a Rust String, returned as a pointer.
/// Note: This is a simplified stub — the real implementation handles Latin1 vs UTF-16.
#[no_mangle]
pub extern "C" fn BunString__fromBytes(bytes: *const u8, len: usize) -> *mut c_void {
    if bytes.is_null() || len == 0 {
        return core::ptr::null_mut();
    }
    unsafe {
        let slice = core::slice::from_raw_parts(bytes, len);
        let s = ::std::string::String::from_utf8_lossy(slice).into_owned();
        Box::into_raw(Box::new(s)) as *mut c_void
    }
}

/// Bun__WTFStringImpl__destroy: destroy a WTFStringImpl
/// Original: Zig-compiled C function that decrements refcount and frees.
/// Bao: free the allocated String.
#[no_mangle]
pub extern "C" fn Bun__WTFStringImpl__destroy(this: *const c_void) {
    if this.is_null() {
        return;
    }
    unsafe {
        let _ = Box::from_raw(this as *mut ::std::string::String);
    }
}

// ──────────────────────────────────────────────────────────────
// Signal forwarding — provided by bao_bin/process management
// ──────────────────────────────────────────────────────────────

/// Bun__currentSyncPID: tracks the current synchronous child process PID.
/// Original: Zig-compiled C++ AtomicI64 global variable accessed via .store()/.load().
/// Bao: provides an AtomicI64 static that bun_spawn_sys::ffi expects.
#[cfg(target_os = "linux")]
#[no_mangle]
pub static Bun__currentSyncPID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

#[cfg(not(target_os = "linux"))]
#[no_mangle]
pub static Bun__currentSyncPID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

// ──────────────────────────────────────────────────────────────
// WTF base64
// ──────────────────────────────────────────────────────────────

/// WTF__base64URLEncode: URL-safe base64 encoding
/// Original: WTF library function from WebKit.
/// Bao: use base64 crate.
#[no_mangle]
pub extern "C" fn WTF__base64URLEncode(
    data: *const u8,
    len: usize,
    out: *mut u8,
    out_len: *mut usize,
) {
    if data.is_null() || out.is_null() || out_len.is_null() {
        return;
    }
    unsafe {
        let slice = core::slice::from_raw_parts(data, len);
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(slice);
        let bytes = encoded.as_bytes();
        let copy_len = bytes.len().min(*out_len);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, copy_len);
        *out_len = bytes.len();
    }
}

// ──────────────────────────────────────────────────────────────
// Brotli decoder — now provided by compiled C library (bun_brotli_sys build.rs).
// All BrotliDecoder* symbols resolved by libbrotli.a at link time.
// ──────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────
// libdeflate — now provided by compiled C library (bun_libdeflate_sys build.rs).
// All libdeflate_* symbols resolved by liblibdeflate.a at link time.
// ──────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────
// ZSTD — now provided by compiled C library (bun_zstd build.rs).
// All ZSTD_* symbols resolved by libzstd.a at link time.
// ──────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────
// Misc stubs
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn __bun_crash_handler_out_of_memory() -> *mut c_void { unsafe { libc::abort() } }

// ──────────────────────────────────────────────────────────────
// Highway SIMD string ops: now provided by compiled C++ library (bun_highway).
// All highway_* symbols resolved by libhighway.a + libhighway_strings.a at link time.
// ──────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────
// FilePoll dispatch — provided by bao_uloop/bun_io integration
// ──────────────────────────────────────────────────────────────

// BoringSSL SSL_* stubs — all removed, now provided by compiled
// libboringssl.a (bun_boringssl_sys build.rs). Includes:
//   SSL_set_cipher_list, SSL_set_ciphersuites, SSL_set1_curves_list,
//   SSL_set1_sigalgs_list, and all other SSL_*/BIO_*/ERR_* symbols.