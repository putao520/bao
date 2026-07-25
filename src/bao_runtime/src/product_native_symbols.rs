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
    // URL / WTF / regex RealImpl anchors (STUB-INVENTORY).
    let _ = WTF__parseES5Date as *const () as usize;
    let _ = WTF__parseDouble as *const () as usize;
    let _ = WTF__dtoa as *const () as usize;
    let _ = URL__fromString as *const () as usize;
    let _ = __bun_regex_compile as *const () as usize;
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

/// Real pure-Rust ES5 date parse (owner: `bun_core::wtf::parse_es5_date_raw`).
/// @trace STUB-INVENTORY: WTF__parseES5Date RealImpl
#[unsafe(no_mangle)]
pub extern "C" fn WTF__parseES5Date(bytes: *const u8, length: usize) -> f64 {
    if bytes.is_null() || length == 0 {
        return f64::NAN;
    }
    // SAFETY: caller provides valid Latin-1 `bytes`/`length`.
    let slice = unsafe { core::slice::from_raw_parts(bytes, length) };
    bun_core::wtf::parse_es5_date_raw(slice)
}

/// Real pure-Rust partial double parse (owner: `bun_core::fmt::parse_double_raw`).
/// @trace STUB-INVENTORY: WTF__parseDouble RealImpl
#[unsafe(no_mangle)]
pub extern "C" fn WTF__parseDouble(
    bytes: *const u8,
    length: usize,
    counted: *mut usize,
) -> f64 {
    if bytes.is_null() || length == 0 {
        if !counted.is_null() {
            unsafe {
                *counted = 0;
            }
        }
        return f64::NAN;
    }
    // SAFETY: caller provides valid Latin-1 `bytes`/`length`.
    let slice = unsafe { core::slice::from_raw_parts(bytes, length) };
    let mut count = 0usize;
    let v = bun_core::fmt::parse_double_raw(slice, &mut count);
    if !counted.is_null() {
        unsafe {
            *counted = count;
        }
    }
    v
}

/// ABI matches historical WTF (`buf: &mut [u8; 124], number: f64`).
/// Owner: `bun_core::fmt::dtoa_into`.
/// @trace STUB-INVENTORY: WTF__dtoa RealImpl
#[unsafe(no_mangle)]
pub extern "C" fn WTF__dtoa(buf: &mut [u8; 124], number: f64) -> usize {
    bun_core::fmt::dtoa_into(buf, number)
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

// ── URL FFI residual (pure-Rust WHATWG; mirrors bun_url::whatwg) ──────────
// Primary callers use `bun_url::whatwg` pure path. These `#[no_mangle]` remain
// as the product link-time owner so any residual C/Rust declarers resolve to
// real parse — never dead/identity noops.
//
// @trace STUB-INVENTORY: URL__* RealImpl via bun_url pure parse

use bun_core::String as BunString;
use bun_url::whatwg::URL as WhatwgUrl;

/// ABI-compatible mirror of `bun_core::String` (24 bytes) for C-style declarers.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct BunStringValue {
    tag: u64,
    _impl: [u64; 2],
}

fn as_bun_string(s: &BunStringValue) -> &BunString {
    // SAFETY: BunStringValue is layout-identical to bun_core::String (24 bytes).
    unsafe { &*(s as *const BunStringValue as *const BunString) }
}

fn as_bun_string_mut(s: &mut BunStringValue) -> &mut BunString {
    // SAFETY: see as_bun_string.
    unsafe { &mut *(s as *mut BunStringValue as *mut BunString) }
}

fn to_bun_string_value(s: BunString) -> BunStringValue {
    // SAFETY: layout-identical Copy POD.
    unsafe { core::mem::transmute(s) }
}

fn dead_string_value() -> BunStringValue {
    to_bun_string_value(BunString::dead())
}

#[unsafe(no_mangle)]
pub extern "C" fn URL__getHref(input: &mut BunStringValue) -> BunStringValue {
    let s = as_bun_string(input);
    to_bun_string_value(bun_url::href_from_string(s))
}

#[unsafe(no_mangle)]
pub extern "C" fn URL__getHrefJoin(
    base: &mut BunStringValue,
    relative: &mut BunStringValue,
) -> BunStringValue {
    to_bun_string_value(bun_url::join(as_bun_string(base), as_bun_string(relative)))
}

#[unsafe(no_mangle)]
pub extern "C" fn URL__fromString(
    str: &mut BunStringValue,
) -> Option<core::ptr::NonNull<WhatwgUrl>> {
    WhatwgUrl::from_string(as_bun_string(str))
}

#[unsafe(no_mangle)]
pub extern "C" fn URL__pathname(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.pathname())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__protocol(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.protocol())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__hostname(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.hostname())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__hash(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.hash())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__host(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.host())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__password(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.password())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__username(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.username())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__search(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.search())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__fragmentIdentifier(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.fragment_identifier())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__href(url: &WhatwgUrl) -> BunStringValue {
    to_bun_string_value(url.href())
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__getFileURLString(input: &mut BunStringValue) -> BunStringValue {
    to_bun_string_value(bun_url::file_url_from_string(as_bun_string(input)))
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__pathFromFileURL(input: &mut BunStringValue) -> BunStringValue {
    to_bun_string_value(bun_url::path_from_file_url(as_bun_string(input)))
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__port(url: &WhatwgUrl) -> u32 {
    url.port()
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__deinit(url: &mut WhatwgUrl) {
    url.deinit();
}
#[unsafe(no_mangle)]
pub extern "C" fn URL__originLength(latin1_slice: *const u8, len: usize) -> u32 {
    if latin1_slice.is_null() || len == 0 {
        return 0;
    }
    // SAFETY: caller provides valid slice.
    let slice = unsafe { core::slice::from_raw_parts(latin1_slice, len) };
    match bun_url::origin_from_slice(slice) {
        Some(o) => o.len() as u32,
        None => 0,
    }
}

// Silence unused mut helper if only as_bun_string is hot-path used.
#[allow(dead_code)]
fn _touch_mut(s: &mut BunStringValue) {
    let _ = as_bun_string_mut(s);
}
#[allow(dead_code)]
fn _touch_dead() {
    let _ = dead_string_value();
}

// ── regex (regex crate) / linux_trace residual ────────────────────────────
// install_types::NodeLinker declares `extern "Rust"` — use Rust ABI (no
// `extern "C"`). Pattern is compiled with the `regex` crate.
//
// @trace STUB-INVENTORY: __bun_regex_* RealImpl via regex crate

/// Compile `pattern` with no flags. `None` ⇔ invalid regex.
/// Rust ABI matches `bun_install_types::NodeLinker` declarer.
#[unsafe(no_mangle)]
pub fn __bun_regex_compile(pattern: BunString) -> Option<core::ptr::NonNull<()>> {
    let utf8 = pattern.to_utf8_without_ref();
    let bytes = utf8.slice();
    // SAFETY: package-name regex patterns are ASCII (escape_reg_exp output).
    let pat = core::str::from_utf8(bytes).ok()?;
    let re = regex::Regex::new(pat).ok()?;
    let boxed = Box::new(re);
    // SAFETY: freshly allocated; unique owner until __bun_regex_drop.
    Some(unsafe { core::ptr::NonNull::new_unchecked(Box::into_raw(boxed) as *mut ()) })
}

#[unsafe(no_mangle)]
pub fn __bun_regex_matches(regex: core::ptr::NonNull<()>, input: &BunString) -> bool {
    // SAFETY: regex was produced by __bun_regex_compile.
    let re = unsafe { &*(regex.as_ptr() as *const regex::Regex) };
    let utf8 = input.to_utf8_without_ref();
    let Ok(s) = core::str::from_utf8(utf8.slice()) else {
        return false;
    };
    re.is_match(s)
}

#[unsafe(no_mangle)]
pub fn __bun_regex_drop(regex: core::ptr::NonNull<()>) {
    // SAFETY: unique owner from __bun_regex_compile.
    unsafe {
        drop(Box::from_raw(regex.as_ptr() as *mut regex::Regex));
    }
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
