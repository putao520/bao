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
    // URL / WTF / regex RealImpl anchors (STUB-INVENTORY).
    let _ = WTF__parseES5Date as *const () as usize;
    let _ = WTF__parseDouble as *const () as usize;
    let _ = WTF__dtoa as *const () as usize;
    let _ = URL__fromString as *const () as usize;
    let _ = __bun_regex_compile as *const () as usize;
    // Signal-forwarding quartet (this module is the single owner).
    let _ = Bun__registerSignalsForForwarding as *const () as usize;
    let _ = Bun__currentSyncPID.load(Ordering::Relaxed);
    // linux_trace RealImpl lives in `linux_trace` module (STUB-INVENTORY).
    crate::linux_trace::force_link_linux_trace();
}

// ── process reload / stdio / StackCheck / crash dump / cpu / exec probe ───
// All migrated to their named owner `bun_core::native_seam` /
// `bun_core::util::spawn_ffi` (the lowest crate on the dep graph that
// declares them), so test binaries linking bao_native_stubs and the product
// binary resolve the SAME single definition. Do NOT reintroduce here
// (STUB-INVENTORY dual-def iron rule):
//   bun_initialize_process, bun_restore_stdio, on_before_reload_process_linux,
//   Bun__StackCheck__{initialize,getMaxStack}, WTF__DumpStackTrace,
//   bun_cpu_features, is_executable_file, ares_inet_pton,
//   BunString__fromBytes, Bun__WTFStringImpl__destroy, posix_spawn_bun.
// TLS C→Rust hooks (Bun__Node__UseSystemCA / BUN__warn__extra_ca_load_failed /
// bun_ssl_ctx_cache_on_free) moved to `bao_uloop`.

// ── signal forwarding / sync PID (spawn) ──────────────────────────────────
// ABI SSOT: `bun_spawn_sys::ffi` — zero-arg register/unregister/sendPending.
// Semantics match `spawn/process.rs` SignalForwarding:
//   register → install handlers; currentSyncPID=0 → spawn → store ±pid →
//   sendPending → drop → unregister.
// @trace STUB-INVENTORY: Bun__*Signals* / Bun__currentSyncPID RealImpl

/// Pending signal received while `Bun__currentSyncPID` was 0 / -1 (pre-spawn).
static PENDING_SIGNAL: AtomicI32 = AtomicI32::new(0);

#[unsafe(no_mangle)]
pub static Bun__currentSyncPID: AtomicI64 = AtomicI64::new(-1);

/// Forward `sig` to the current sync child (or process group if pid < -1),
/// else stash as pending for `Bun__sendPendingSignalIfNecessary`.
/// Negative pid = process group (libc::kill convention, same as Bun).
#[cfg(unix)]
extern "C" fn bun_forward_signal_handler(sig: c_int) {
    let pid = Bun__currentSyncPID.load(Ordering::Relaxed);
    // 0 = pre-spawn / cleared; -1 = default unset — stash for later.
    if pid != 0 && pid != -1 {
        // SAFETY: kill with pid/pgroup is async-signal-safe.
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
    } else {
        PENDING_SIGNAL.store(sig, Ordering::SeqCst);
    }
}

/// Install SIGINT/SIGTERM/SIGHUP/SIGQUIT handlers that forward to
/// [`Bun__currentSyncPID`]. Zero-arg ABI (call-site SSOT).
#[unsafe(no_mangle)]
pub extern "C" fn Bun__registerSignalsForForwarding() {
    #[cfg(unix)]
    {
        PENDING_SIGNAL.store(0, Ordering::SeqCst);
        unsafe {
            let mut sa: libc::sigaction = core::mem::zeroed();
            sa.sa_sigaction = bun_forward_signal_handler as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = libc::SA_RESTART;
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                libc::sigaction(sig, &sa, core::ptr::null_mut());
            }
        }
    }
    // Windows: no-op same ABI (spawn signal forwarding is Unix-only).
}

/// Restore default dispositions and clear any pending signal.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__unregisterSignalsForForwarding() {
    #[cfg(unix)]
    {
        unsafe {
            let mut sa: libc::sigaction = core::mem::zeroed();
            sa.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = 0;
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                libc::sigaction(sig, &sa, core::ptr::null_mut());
            }
        }
        PENDING_SIGNAL.store(0, Ordering::SeqCst);
    }
}

/// Deliver a signal that arrived before the child PID was known.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__sendPendingSignalIfNecessary() {
    let sig = PENDING_SIGNAL.swap(0, Ordering::SeqCst);
    if sig == 0 {
        return;
    }
    let pid = Bun__currentSyncPID.load(Ordering::Relaxed);
    if pid != 0 && pid != -1 {
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
}

// ── ares / CPU / executable / crash dump ─────────────────────────────────
// Migrated to `bun_core::native_seam` (named owner):
//   ares_inet_pton, bun_cpu_features, is_executable_file, WTF__DumpStackTrace.
// Do NOT reintroduce here (STUB-INVENTORY dual-def iron rule).

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
pub extern "C" fn WTF__parseDouble(bytes: *const u8, length: usize, counted: *mut usize) -> f64 {
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

// `WTF__releaseFastMallocFreeMemoryForThisThread` — real owner: `bun_alloc`
// (`mi_collect(false)`). Do NOT reintroduce empty noop (dual-def iron rule).

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

// ── regex (regex crate) residual ──────────────────────────────────────────
// install_types::NodeLinker declares `extern "Rust"` — use Rust ABI (no
// `extern "C"`). Pattern is compiled with the `regex` crate.
//
// @trace STUB-INVENTORY: __bun_regex_* RealImpl via regex crate
//
// Bun__linux_trace_* → `crate::linux_trace` (RealImpl, correct ABI).
// Do NOT reintroduce the old 8-arg Chrome-trace noop here (dual-def + ABI
// mismatch with bun_core::perf::sys / perf.zig).

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

// ── TLS residual (root_certs / BoringSSL EX_free) ─────────────────────────
// Migrated to `bao_uloop` (named owner — chained into every link scope that
// pulls the uSockets TLS archives, including test binaries). Do NOT
// reintroduce here (STUB-INVENTORY dual-def iron rule):
//   Bun__Node__UseSystemCA, BUN__warn__extra_ca_load_failed,
//   bun_ssl_ctx_cache_on_free.
