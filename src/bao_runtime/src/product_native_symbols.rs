// @trace STUB-INVENTORY: product residual RealImpl rehomed out of bao_native_stubs
//! Residual `#[no_mangle]` symbols formerly provided only via the
//! `bao_native_stubs` hard-dep. Living here lets the **default product path**
//! drop that hard-dep / force_link anchor.
//!
//! Prefer migrating each symbol to its named owner (spawn_sys / bun_url /
//! bun_core / TLS) and deleting from this module. Do not reintroduce a product
//! hard-dep on `bao_native_stubs`.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_char, c_int, c_short, c_void};
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

/// Keep this compilation unit on the product link line.
#[inline(never)]
pub fn force_link_product_native_symbols() {
    let _ = force_link_product_native_symbols as *const () as usize;
    let _ = on_before_reload_process_linux as *const () as usize;
    let _ = bun_restore_stdio as *const () as usize;
    let _ = Bun__StackCheck__initialize as *const () as usize;
}

// ── process reload / stdio ────────────────────────────────────────────────

/// Best-effort `sync()` before `exec` on Linux reload (bun_core::util).
#[unsafe(no_mangle)]
pub extern "C" fn on_before_reload_process_linux() {
    unsafe { libc::sync() };
}

#[unsafe(no_mangle)]
pub extern "C" fn bun_restore_stdio() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

/// Minimal process stdio init (product path without CLI).
#[unsafe(no_mangle)]
pub extern "C" fn bun_initialize_process() {
    // Product path: nothing to set up beyond what std already provides.
}

// ── stack check (bun_core::util::StackCheck) ──────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn Bun__StackCheck__initialize() {
    // Thread stack bounds are resolved lazily via getMaxStack.
}

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

// ── signal forwarding / sync PID (spawn) ──────────────────────────────────

static FORWARDED_PID: AtomicI32 = AtomicI32::new(-1);

#[unsafe(no_mangle)]
pub extern "C" fn Bun__registerSignalsForForwarding(
    pid: i32,
    _signals: *const c_int,
    _count: usize,
) {
    FORWARDED_PID.store(pid, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn Bun__unregisterSignalsForForwarding() {
    FORWARDED_PID.store(-1, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn Bun__sendPendingSignalIfNecessary() {
    let pid = FORWARDED_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        FORWARDED_PID.store(-1, Ordering::SeqCst);
    }
}

#[unsafe(no_mangle)]
pub static Bun__currentSyncPID: AtomicI64 = AtomicI64::new(-1);

// ── ares / CPU / executable ───────────────────────────────────────────────

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

// ── WTF helpers (bun_core::wtf / fmt / crash_handler) ─────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn WTF__DumpStackTrace() {
    let bt = std::backtrace::Backtrace::capture();
    if bt.status() == std::backtrace::BacktraceStatus::Captured {
        eprintln!("{bt}");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn WTF__parseES5Date(_bytes: *const u8, _length: usize) -> f64 {
    f64::NAN
}

#[unsafe(no_mangle)]
pub extern "C" fn WTF__parseDouble(
    _bytes: *const u8,
    _length: usize,
    counted: *mut usize,
) -> f64 {
    if !counted.is_null() {
        unsafe {
            *counted = 0;
        }
    }
    f64::NAN
}

/// ABI matches `bun_core::fmt` (`buf: &mut [u8; 124], number: f64`).
#[unsafe(no_mangle)]
pub extern "C" fn WTF__dtoa(buf: &mut [u8; 124], number: f64) -> usize {
    // Prefer std formatting until real WTF dtoa lands in bun_core.
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(&mut buf[..]);
    match write!(cursor, "{number}") {
        Ok(()) => cursor.position() as usize,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn WTF__releaseFastMallocFreeMemoryForThisThread() {}

// ── BunString / WTFString (simplified product residual) ───────────────────

#[unsafe(no_mangle)]
pub extern "C" fn BunString__fromBytes(bytes: *const u8, len: usize) -> bun_core::String {
    if bytes.is_null() || len == 0 {
        return bun_core::String::empty();
    }
    // SAFETY: caller provides valid `bytes`/`len` for the duration of this call.
    let slice = unsafe { core::slice::from_raw_parts(bytes, len) };
    bun_core::String::from_bytes(slice)
}

#[unsafe(no_mangle)]
pub extern "C" fn Bun__WTFStringImpl__destroy(this: *const c_void) {
    if this.is_null() {
        return;
    }
    // Product residual: refcount/free is incomplete until full WTF owner lands.
    // Do not free arbitrary pointers — only no-op for now to avoid double-free
    // when callers use ZigString/DEAD tags rather than WTF heap strings.
    let _ = this;
}

// ── posix_spawn_bun (real posix_spawnp path) ──────────────────────────────

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
    kind: u8,
    _pad: [u8; 7],
    path: *const c_char,
    fds: [c_int; 2],
    flags: c_int,
    mode: c_int,
}

const ACTION_CLOSE: u8 = 1;
const ACTION_DUP2: u8 = 2;
const ACTION_OPEN: u8 = 3;

unsafe fn extern_environ() -> *mut *mut c_char {
    unsafe extern "C" {
        static environ: *mut *mut c_char;
    }
    unsafe { environ }
}

#[unsafe(no_mangle)]
pub extern "C" fn posix_spawn_bun(
    pid: *mut c_int,
    path: *const c_char,
    request: *const c_void,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    unsafe {
        let req = &*(request as *const BunSpawnRequest);

        let mut fa: libc::posix_spawn_file_actions_t = core::mem::zeroed();
        let rc = libc::posix_spawn_file_actions_init(&mut fa);
        if rc != 0 {
            return rc;
        }

        if !req.chdir_buf.is_null() {
            libc::posix_spawn_file_actions_addchdir_np(&mut fa, req.chdir_buf);
        }

        for i in 0..req.actions.len {
            let action = &*req.actions.ptr.add(i);
            match action.kind {
                ACTION_CLOSE => {
                    libc::posix_spawn_file_actions_addclose(&mut fa, action.fds[0]);
                }
                ACTION_DUP2 => {
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

        let mut attr: libc::posix_spawnattr_t = core::mem::zeroed();
        let rc = libc::posix_spawnattr_init(&mut attr);
        if rc != 0 {
            libc::posix_spawn_file_actions_destroy(&mut fa);
            return rc;
        }

        let mut flags: c_short =
            (libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_SETSIGMASK) as c_short;
        if req.new_process_group {
            flags |= 0x80; // POSIX_SPAWN_SETSID on Linux
        }

        let mut sigdefault: libc::sigset_t = core::mem::zeroed();
        libc::sigemptyset(&mut sigdefault);
        libc::posix_spawnattr_setsigdefault(&mut attr, &sigdefault);

        let mut sigmask: libc::sigset_t = core::mem::zeroed();
        libc::sigfillset(&mut sigmask);
        libc::posix_spawnattr_setsigmask(&mut attr, &sigmask);

        libc::posix_spawnattr_setflags(&mut attr, flags);

        let env = if envp.is_null() {
            extern_environ()
        } else {
            envp as *mut *mut c_char
        };

        let rc = libc::posix_spawnp(pid, path, &fa, &attr, argv as *mut *mut c_char, env);

        libc::posix_spawnattr_destroy(&mut attr);
        libc::posix_spawn_file_actions_destroy(&mut fa);
        rc
    }
}

// ── URL FFI residual (dead-tag until bun_url pure path finishes) ──────────

/// ABI-compatible mirror of `bun_core::String` (24 bytes).
#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct BunStringValue {
    tag: u64,
    _impl: [u64; 2],
}

#[repr(C)]
struct OpaqueURL {
    _opaque: [u8; 0],
}

fn dead_string() -> BunStringValue {
    BunStringValue {
        tag: 0,
        _impl: [0, 0],
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn URL__getHref(input: &mut BunStringValue) -> BunStringValue {
    *input
}

#[unsafe(no_mangle)]
pub extern "C" fn URL__getHrefJoin(
    _base: &mut BunStringValue,
    _relative: &mut BunStringValue,
) -> BunStringValue {
    dead_string()
}

#[unsafe(no_mangle)]
pub extern "C" fn URL__fromString(
    _str: &mut BunStringValue,
) -> Option<core::ptr::NonNull<OpaqueURL>> {
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn URL__pathname(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__protocol(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__hostname(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__hash(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__host(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__password(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__username(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__search(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__fragmentIdentifier(_url: &OpaqueURL) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__getFileURLString(_input: &mut BunStringValue) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__pathFromFileURL(_input: &mut BunStringValue) -> BunStringValue {
    dead_string()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__port(_url: &OpaqueURL) -> u32 {
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__deinit(_url: &mut OpaqueURL) {}

// ── regex / linux_trace residual ──────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __bun_regex_drop(_regex: core::ptr::NonNull<()>) {}

#[unsafe(no_mangle)]
pub extern "C" fn __bun_regex_compile(
    _pattern: BunStringValue,
) -> Option<core::ptr::NonNull<()>> {
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn __bun_regex_matches(
    _regex: core::ptr::NonNull<()>,
    _input: &BunStringValue,
) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn Bun__linux_trace_init() -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn Bun__linux_trace_emit(
    _id: u32,
    _name: *const c_char,
    _cat: *const c_char,
    _phase: u8,
    _ts: u64,
    _pid: u32,
    _tid: u32,
    _extra: *const c_char,
) {
}

// ── TLS residual (root_certs / BoringSSL EX_free) ─────────────────────────

#[unsafe(no_mangle)]
pub static mut Bun__Node__UseSystemCA: bool = true;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn BUN__warn__extra_ca_load_failed(
    filename: *const c_char,
    error_msg: *const c_char,
) {
    let filename_str = if filename.is_null() {
        "(unknown)".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(filename) }
            .to_string_lossy()
            .into_owned()
    };
    let error_str = if error_msg.is_null() {
        "(unknown)".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(error_msg) }
            .to_string_lossy()
            .into_owned()
    };
    eprintln!("warn: ignoring extra certs from {filename_str}, load failed: {error_str}");
}

#[unsafe(no_mangle)]
pub extern "C" fn bun_ssl_ctx_cache_on_free(
    _parent: *mut c_void,
    _ptr: *mut c_void,
    _ad: *mut c_void,
    _index: c_int,
    _argl: i64,
    _argp: *mut c_void,
) {
}
