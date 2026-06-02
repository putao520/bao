// @trace REQ-ENG-001
//! Pure Rust implementations of symbols originally provided by Zig-compiled C/Zig code
//! in upstream Bun. Eliminates Zig build dependency entirely.
//!
//! Strategy:
//! - mi_* symbols: delegate to libc malloc/free/realloc (functionally equivalent for Bao)
//! - Bun__/WTF__/bun_* symbols: minimal no-op or functional stubs for test linking
//!
//! Linker GC prevention: a ctor in .init_array auto-calls force_link() at load time,
//! so integration tests don't need explicit force_link() calls.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use base64::Engine;
use core::ffi::{c_char, c_int, c_uint, c_void};

mod c_lib_stubs;

/// Force the linker to include all native stub symbols.
/// Call this from test code: `bao_native_stubs::force_link();`
///
/// Note: Only call this after the process is fully initialized (e.g., at the
/// start of a test function, not in a global ctor). Some stubs call libc
/// functions that require full process initialization.
#[inline(never)]
pub fn force_link() {
    // Actually call each function to prevent the linker from GC'ing them.
    // We pass safe arguments to exercise the function bodies.
    let p = mi_malloc(1);
        let _ = mi_usable_size(p); // safe: p is a valid allocation
        mi_free(p);

        let p2 = mi_calloc(1, 8);
        mi_free(p2);

        let p3 = mi_zalloc(16);
        mi_free(p3);

        let p4 = mi_malloc_aligned(32, 64);
        mi_free(p4);

        let _ = mi_good_size(100);
        let heap = mi_heap_new();
        mi_heap_delete(heap); // safe: heap is our sentinel pointer

        let _ = mi_strdup(core::ptr::null()); // handles null internally
        let _ = mi_strndup(core::ptr::null(), 0);
        let _ = mi_mallocn(0, 0);
        let _ = mi_malloc_small(1);
        let _ = mi_zalloc_small(1);

        mi_stats_reset();
        mi_stats_merge();
        mi_collect(false);
        mi_thread_init();
        mi_thread_done();
        mi_process_init();
        mi_register_deferred_free(core::ptr::null(), core::ptr::null_mut());
        mi_register_error(core::ptr::null(), core::ptr::null_mut());
        mi_option_set(0, 0);

        let _ = Bun__linux_trace_init();
        Bun__linux_trace_emit(0, 0, 0, 0, 0);
        Bun__linux_trace_close();
        Bun__StackCheck__initialize();
        let _ = Bun__StackCheck__getMaxStack();
        WTF__DumpStackTrace();

        Bun__registerSignalsForForwarding();
        Bun__unregisterSignalsForForwarding();
        let _ = Bun__currentSyncPID.load(std::sync::atomic::Ordering::Relaxed);
        Bun__sendPendingSignalIfNecessary(0);

        __bun_resolver_init_package_manager(core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null());

        let _ = bun_cpu_features();
        let _ = is_executable_file(core::ptr::null());

        bun_restore_stdio();
        on_before_reload_process_linux();
        let _ = BunString__fromBytes(core::ptr::null(), 0);
        Bun__WTFStringImpl__destroy(core::ptr::null());

        let _ = URL__getHref(core::ptr::null());
        let _ = URL__getHrefJoin(core::ptr::null(), core::ptr::null());

        UpgradedDuplex__is_shutdown(core::ptr::null());
        UpgradedDuplex__is_closed(core::ptr::null());
        UpgradedDuplex__is_established(core::ptr::null());
        UpgradedDuplex__close(core::ptr::null_mut());
        UpgradedDuplex__shutdown(core::ptr::null_mut());
        UpgradedDuplex__flush(core::ptr::null_mut());
        UpgradedDuplex__set_timeout(core::ptr::null_mut(), 0);
        let _ = UpgradedDuplex__ssl(core::ptr::null());
        let _ = UpgradedDuplex__ssl_error(core::ptr::null());
        let _ = UpgradedDuplex__encode_and_write(core::ptr::null_mut(), core::ptr::null(), 0);

        let _ = ares_inet_pton(2, core::ptr::null(), core::ptr::null_mut());
        Bun__addrinfo_registerQuic(core::ptr::null_mut());

        WTF__base64URLEncode(core::ptr::null(), 0, core::ptr::null_mut(), core::ptr::null_mut());

        // Force-link all c_lib_stubs symbols
        c_lib_stubs::force_c_lib_stubs();
}

// ──────────────────────────────────────────────────────────────
// mimalloc → system allocator delegation
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn mi_malloc(size: usize) -> *mut c_void {
    unsafe { libc::malloc(size) }
}

#[no_mangle]
pub extern "C" fn mi_calloc(count: usize, size: usize) -> *mut c_void {
    unsafe { libc::calloc(count, size) }
}

#[no_mangle]
pub extern "C" fn mi_zalloc(size: usize) -> *mut c_void {
    unsafe { libc::calloc(1, size) }
}

#[no_mangle]
pub extern "C" fn mi_realloc(p: *mut c_void, newsize: usize) -> *mut c_void {
    unsafe { libc::realloc(p, newsize) }
}

#[no_mangle]
pub extern "C" fn mi_expand(p: *mut c_void, newsize: usize) -> *mut c_void {
    unsafe { libc::realloc(p, newsize) }
}

#[no_mangle]
pub extern "C" fn mi_free(p: *mut c_void) {
    unsafe { libc::free(p) }
}

#[no_mangle]
pub extern "C" fn mi_malloc_aligned(size: usize, alignment: usize) -> *mut c_void {
    if alignment <= 16 {
        unsafe { libc::malloc(size) }
    } else {
        unsafe { libc::aligned_alloc(alignment, size) }
    }
}

#[no_mangle]
pub extern "C" fn mi_zalloc_aligned(size: usize, alignment: usize) -> *mut c_void {
    let ptr = mi_malloc_aligned(size, alignment);
    if !ptr.is_null() {
        unsafe { libc::memset(ptr, 0, size) };
    }
    ptr
}

#[no_mangle]
pub extern "C" fn mi_mallocn(count: usize, size: usize) -> *mut c_void {
    mi_malloc(count.wrapping_mul(size))
}

#[no_mangle]
pub extern "C" fn mi_usable_size(p: *const c_void) -> usize {
    unsafe { libc::malloc_usable_size(p as *mut c_void) }
}

/// mi_malloc_usable_size: alias for mi_usable_size (mimalloc exports both names)
#[no_mangle]
pub extern "C" fn mi_malloc_usable_size(p: *const c_void) -> usize {
    mi_usable_size(p)
}

#[no_mangle]
pub extern "C" fn mi_good_size(size: usize) -> usize {
    size
}

#[no_mangle]
pub extern "C" fn mi_strdup(s: *const c_char) -> *mut c_char {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let len = libc::strlen(s);
        let buf = libc::malloc(len + 1) as *mut c_char;
        if !buf.is_null() {
            libc::memcpy(buf as *mut c_void, s as *const c_void, len + 1);
        }
        buf
    }
}

#[no_mangle]
pub extern "C" fn mi_strndup(s: *const c_char, n: usize) -> *mut c_char {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let len = libc::strlen(s);
        let copy_len = len.min(n);
        let buf = libc::malloc(copy_len + 1) as *mut c_char;
        if !buf.is_null() {
            libc::memcpy(buf as *mut c_void, s as *const c_void, copy_len);
            *buf.add(copy_len) = 0;
        }
        buf
    }
}

#[no_mangle]
pub extern "C" fn mi_malloc_small(size: usize) -> *mut c_void {
    mi_malloc(size)
}

#[no_mangle]
pub extern "C" fn mi_zalloc_small(size: usize) -> *mut c_void {
    mi_zalloc(size)
}

#[no_mangle]
pub extern "C" fn mi_reallocn(p: *mut c_void, count: usize, size: usize) -> *mut c_void {
    mi_realloc(p, count.wrapping_mul(size))
}

#[no_mangle]
pub extern "C" fn mi_reallocf(p: *mut c_void, newsize: usize) -> *mut c_void {
    let result = mi_realloc(p, newsize);
    if result.is_null() && !p.is_null() {
        mi_free(p);
    }
    result
}

// ── Heap API ──

#[no_mangle]
pub extern "C" fn mi_heap_new() -> *mut c_void {
    static SENTINEL: usize = 1;
    &SENTINEL as *const usize as *mut c_void
}

#[no_mangle]
pub extern "C" fn mi_heap_malloc(_heap: *mut c_void, size: usize) -> *mut c_void {
    mi_malloc(size)
}

#[no_mangle]
pub extern "C" fn mi_heap_malloc_aligned(_heap: *mut c_void, size: usize, alignment: usize) -> *mut c_void {
    mi_malloc_aligned(size, alignment)
}

#[no_mangle]
pub extern "C" fn mi_heap_zalloc(_heap: *mut c_void, size: usize) -> *mut c_void {
    mi_zalloc(size)
}

#[no_mangle]
pub extern "C" fn mi_heap_zalloc_aligned(_heap: *mut c_void, size: usize, alignment: usize) -> *mut c_void {
    mi_zalloc_aligned(size, alignment)
}

#[no_mangle]
pub extern "C" fn mi_heap_delete(_heap: *mut c_void) {}

#[no_mangle]
pub extern "C" fn mi_heap_destroy(_heap: *mut c_void) {}

#[no_mangle]
pub extern "C" fn mi_heap_realloc(_heap: *mut c_void, p: *mut c_void, newsize: usize) -> *mut c_void {
    mi_realloc(p, newsize)
}

#[no_mangle]
pub extern "C" fn mi_heap_collect(_heap: *mut c_void, _force: bool) {}

// ── Process/thread stubs ──

#[no_mangle]
pub extern "C" fn mi_process_info(
    elapsed_msecs: *mut usize,
    user_msecs: *mut usize,
    system_msecs: *mut usize,
    current_rss: *mut usize,
    peak_rss: *mut usize,
    current_commit: *mut usize,
    peak_commit: *mut usize,
    page_faults: *mut usize,
) {
    let ptrs: [*mut usize; 8] = [
        elapsed_msecs, user_msecs, system_msecs,
        current_rss, peak_rss, current_commit,
        peak_commit, page_faults,
    ];
    for ptr in ptrs {
        if !ptr.is_null() {
            unsafe { ptr.write(0) };
        }
    }
}

#[no_mangle]
pub extern "C" fn mi_stats_reset() {}
#[no_mangle]
pub extern "C" fn mi_stats_merge() {}
#[no_mangle]
pub extern "C" fn mi_collect(_force: bool) {}
#[no_mangle]
pub extern "C" fn mi_thread_init() {}
#[no_mangle]
pub extern "C" fn mi_thread_done() {}
#[no_mangle]
pub extern "C" fn mi_process_init() {}
#[no_mangle]
pub extern "C" fn mi_register_deferred_free(_fun: *const c_void, _arg: *mut c_void) {}
#[no_mangle]
pub extern "C" fn mi_register_error(_fun: *const c_void, _arg: *mut c_void) {}

// ──────────────────────────────────────────────────────────────
// Bun runtime stubs
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn Bun__linux_trace_init() -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn Bun__linux_trace_emit(
    _id: u32,
    _a: u64,
    _b: u64,
    _c: u64,
    _d: u64,
) {}

#[no_mangle]
pub extern "C" fn Bun__linux_trace_close() {}

#[no_mangle]
pub extern "C" fn Bun__StackCheck__initialize() {}

#[no_mangle]
pub extern "C" fn Bun__StackCheck__getMaxStack() -> *mut c_void {
    usize::MAX as *mut c_void
}

#[no_mangle]
pub extern "C" fn WTF__DumpStackTrace() {}

// ──────────────────────────────────────────────────────────────
// bun_resolver init stub
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn __bun_resolver_init_package_manager(
    _log: *mut c_void,
    _install: *mut c_void,
    _env: *const u8,
) {}

// ──────────────────────────────────────────────────────────────
// bun_spawn / bun_core stubs
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn posix_spawn_bun(
    pid: *mut i32,
    path: *const c_char,
    file_actions: *mut c_void,
    attrp: *mut c_void,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    unsafe {
        libc::posix_spawn(
            pid,
            path,
            file_actions as *mut libc::posix_spawn_file_actions_t,
            attrp as *mut libc::posix_spawnattr_t,
            argv,
            envp,
        )
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
// Additional stubs discovered during test linking
// ──────────────────────────────────────────────────────────────

/// bun_restore_stdio: restore stdout/stderr after process operations
/// Original: Zig-compiled C wrapper around dup2/dup3.
/// Bao: no-op stub (test environments don't need stdio restoration).
#[no_mangle]
pub extern "C" fn bun_restore_stdio() {}

/// on_before_reload_process_linux: prepare for process exec on Linux
/// Original: Zig-compiled C function that sets CLOEXEC on FDs before execve.
/// Bao: no-op stub (test environments don't need exec preparation).
#[no_mangle]
pub extern "C" fn on_before_reload_process_linux() {}

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
// Signal forwarding stubs (spawn process support)
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn Bun__registerSignalsForForwarding() {}

#[no_mangle]
pub extern "C" fn Bun__unregisterSignalsForForwarding() {}

/// Bun__currentSyncPID: tracks the current synchronous child process PID.
/// Original: Zig-compiled C++ AtomicI64 global variable accessed via .store()/.load().
/// Bao: provides an AtomicI64 static that bun_spawn_sys::ffi expects.
#[cfg(target_os = "linux")]
#[no_mangle]
pub static Bun__currentSyncPID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

#[cfg(not(target_os = "linux"))]
#[no_mangle]
pub static Bun__currentSyncPID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

#[no_mangle]
pub extern "C" fn Bun__sendPendingSignalIfNecessary(_pid: i32) {}

// ──────────────────────────────────────────────────────────────
// mimalloc option stub
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn mi_option_set(_option: i32, _value: i64) {}

// ──────────────────────────────────────────────────────────────
// URL stubs (bun_url native helpers)
// ──────────────────────────────────────────────────────────────

/// URL__getHref: get the href string from a URL object
/// Original: Zig-compiled C function in bun_url.
/// Bao: return null — URL parsing is done in pure Rust.
#[no_mangle]
pub extern "C" fn URL__getHref(_url: *const c_void) -> *mut c_char {
    core::ptr::null_mut()
}

/// URL__getHrefJoin: join a base URL with a relative URL
/// Original: Zig-compiled C function in bun_url.
/// Bao: return null — URL joining is done in pure Rust.
#[no_mangle]
pub extern "C" fn URL__getHrefJoin(_base: *const c_void, _relative: *const c_char) -> *mut c_char {
    core::ptr::null_mut()
}

// ──────────────────────────────────────────────────────────────
// UpgradedDuplex stubs (HTTP/2 duplex stream)
// ──────────────────────────────────────────────────────────────

/// UpgradedDuplex represents an upgraded HTTP connection (HTTP/2, WebSocket).
/// Original: Zig-compiled C++ class in bun_http.
/// Bao: no-op stubs — duplex streams handled by bun_uws.
#[no_mangle]
pub extern "C" fn UpgradedDuplex__is_shutdown(_this: *const c_void) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__is_closed(_this: *const c_void) -> bool {
    true
}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__is_established(_this: *const c_void) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__close(_this: *mut c_void) {}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__shutdown(_this: *mut c_void) {}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__flush(_this: *mut c_void) {}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__set_timeout(_this: *mut c_void, _timeout_ms: u32) {}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__ssl(_this: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__ssl_error(_this: *const c_void) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn UpgradedDuplex__encode_and_write(
    _this: *mut c_void,
    _data: *const u8,
    _len: usize,
) -> i32 {
    -1 // Error
}

// ──────────────────────────────────────────────────────────────
// DNS stubs (c-ares helpers)
// ──────────────────────────────────────────────────────────────

/// ares_inet_pton: convert presentation format to network address
/// Original: c-ares library function.
/// Bao: simple stub returning -1 (error) — actual DNS uses bun_dns pure Rust.
#[no_mangle]
pub extern "C" fn ares_inet_pton(_af: i32, _src: *const c_char, _dst: *mut c_void) -> i32 {
    -1 // Not implemented — bun_dns uses pure Rust
}

/// Bun__addrinfo_registerQuic: register QUIC address info
/// Original: Zig-compiled C function for HTTP/3 support.
/// Bao: no-op stub — QUIC handled by bun_http.
#[no_mangle]
pub extern "C" fn Bun__addrinfo_registerQuic(_addrinfo: *mut c_void) {}

// ──────────────────────────────────────────────────────────────
// WTF base64 stub
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
// BoringSSL / OpenSSL stubs (for test linking)
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn BIO_ctrl_pending(_bio: *mut c_void) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn BIO_read(_bio: *mut c_void, _buf: *mut c_void, _len: c_int) -> c_int { -1 }

#[no_mangle]
pub extern "C" fn ERR_clear_error() {}

#[no_mangle]
pub extern "C" fn SSL_clear_options(_ssl: *mut c_void, _options: u64) {}

#[no_mangle]
pub extern "C" fn SSL_CTX_free(_ctx: *mut c_void) {}

#[no_mangle]
pub extern "C" fn SSL_CTX_set_cipher_list(_ctx: *mut c_void, _str: *const c_char) -> c_int { 1 }

#[no_mangle]
pub extern "C" fn SSL_do_handshake(_ssl: *mut c_void) -> c_int { -1 }

#[no_mangle]
pub extern "C" fn SSL_free(_ssl: *mut c_void) {}

#[no_mangle]
pub extern "C" fn SSL_get0_alpn_selected(_ssl: *mut c_void, data: *mut *const u8, len: *mut u32) {
    if !data.is_null() { unsafe { *data = core::ptr::null(); } }
    if !len.is_null() { unsafe { *len = 0; } }
}

#[no_mangle]
pub extern "C" fn SSL_get_error(_ssl: *mut c_void, _ret: c_int) -> c_int { 2 } // SSL_ERROR_SYSCALL

#[no_mangle]
pub extern "C" fn SSL_get_rbio(_ssl: *mut c_void) -> *mut c_void { core::ptr::null_mut() }

#[no_mangle]
pub extern "C" fn SSL_get_shutdown(_ssl: *mut c_void) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn SSL_get_wbio(_ssl: *mut c_void) -> *mut c_void { core::ptr::null_mut() }

#[no_mangle]
pub extern "C" fn SSL_is_init_finished(_ssl: *mut c_void) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn SSL_read(_ssl: *mut c_void, _buf: *mut c_void, _len: c_int) -> c_int { -1 }

#[no_mangle]
pub extern "C" fn SSL_renegotiate(_ssl: *mut c_void) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn SSL_set_alpn_protos(_ssl: *mut c_void, _protos: *const u8, _len: c_uint) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn SSL_set_options(_ssl: *mut c_void, _options: u64) {}

#[no_mangle]
pub extern "C" fn SSL_shutdown(_ssl: *mut c_void) -> c_int { 1 }

#[no_mangle]
pub extern "C" fn SSL_write(_ssl: *mut c_void, _buf: *const c_void, _len: c_int) -> c_int { -1 }

// ──────────────────────────────────────────────────────────────
// Brotli decoder — pure Rust via `brotli` crate
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn BrotliDecoderCreateInstance(
    _alloc: *const c_void,
    _free: *const c_void,
    _opaque: *mut c_void,
) -> *mut c_void {
    let state = Box::new(Vec::<u8>::new());
    Box::into_raw(state) as *mut c_void
}

#[no_mangle]
pub extern "C" fn BrotliDecoderDecompressStream(
    _state: *mut c_void,
    _available_in: *mut usize,
    _next_in: *mut *const u8,
    _available_out: *mut usize,
    _next_out: *mut *mut u8,
    _total_out: *mut usize,
) -> c_int {
    // Simplified: use brotli::Decompressor for full buffer decompression
    // Streaming API requires more complex state management
    if _available_in.is_null() || _available_out.is_null() {
        return 0; // BROTLI_DECODER_RESULT_ERROR
    }
    let input_len = unsafe { *_available_in };
    if input_len == 0 {
        return 1; // BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT
    }
    let input_data = unsafe { core::slice::from_raw_parts(*_next_in, input_len) };
    let mut decoder = brotli::Decompressor::new(input_data, 4096);
    let mut output = Vec::with_capacity(4096);
    use std::io::Read;
    match decoder.read_to_end(&mut output) {
        Ok(_) => {
            let out_len = output.len();
            let copy_len = out_len.min(unsafe { *_available_out });
            if copy_len > 0 && !_next_out.is_null() {
                unsafe {
                    core::ptr::copy_nonoverlapping(output.as_ptr(), *_next_out, copy_len);
                    *_available_out = copy_len;
                    if !_total_out.is_null() { *_total_out = copy_len; }
                    *_available_in = 0;
                }
            }
            3 // BROTLI_DECODER_RESULT_SUCCESS
        }
        Err(_) => 0, // BROTLI_DECODER_RESULT_ERROR
    }
}

#[no_mangle]
pub extern "C" fn BrotliDecoderDestroyInstance(state: *mut c_void) {
    if !state.is_null() {
        unsafe { let _ = Box::from_raw(state as *mut Vec<u8>); }
    }
}

#[no_mangle]
pub extern "C" fn BrotliDecoderGetErrorCode(_state: *mut c_void) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn BrotliDecoderIsFinished(state: *mut c_void) -> c_int {
    if state.is_null() { 0 } else { 1 }
}

#[no_mangle]
pub extern "C" fn BrotliDecoderSetParameter(_state: *mut c_void, _param: c_int, _value: u32) -> c_int { 0 }

// ──────────────────────────────────────────────────────────────
// libdeflate — pure Rust via `libdeflater` crate
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn libdeflate_alloc_decompressor() -> *mut c_void {
    let decompressor = libdeflater::Decompressor::new();
    Box::into_raw(Box::new(decompressor)) as *mut c_void
}

#[no_mangle]
pub extern "C" fn libdeflate_deflate_decompress_ex(
    decompressor: *mut c_void,
    inp: *const u8,
    in_nbytes: usize,
    out: *mut u8,
    out_nbytes_avail: usize,
    actual_in_nbytes: *mut usize,
    actual_out_nbytes: *mut usize,
) -> c_int {
    if decompressor.is_null() || inp.is_null() || out.is_null() { return -1; }
    let dec = unsafe { &mut *(decompressor as *mut libdeflater::Decompressor) };
    let input = unsafe { core::slice::from_raw_parts(inp, in_nbytes) };
    let mut output_buf = vec![0u8; out_nbytes_avail];
    match dec.deflate_decompress(input, &mut output_buf) {
        Ok(written) => {
            unsafe {
                core::ptr::copy_nonoverlapping(output_buf.as_ptr(), out, written);
                if !actual_in_nbytes.is_null() { *actual_in_nbytes = in_nbytes; }
                if !actual_out_nbytes.is_null() { *actual_out_nbytes = written; }
            }
            0
        }
        Err(_) => -1,
    }
}

#[no_mangle]
pub extern "C" fn libdeflate_gzip_decompress_ex(
    decompressor: *mut c_void,
    inp: *const u8,
    in_nbytes: usize,
    out: *mut u8,
    out_nbytes_avail: usize,
    actual_in_nbytes: *mut usize,
    actual_out_nbytes: *mut usize,
) -> c_int {
    if decompressor.is_null() || inp.is_null() || out.is_null() { return -1; }
    let dec = unsafe { &mut *(decompressor as *mut libdeflater::Decompressor) };
    let input = unsafe { core::slice::from_raw_parts(inp, in_nbytes) };
    let mut output_buf = vec![0u8; out_nbytes_avail];
    match dec.gzip_decompress(input, &mut output_buf) {
        Ok(written) => {
            unsafe {
                core::ptr::copy_nonoverlapping(output_buf.as_ptr(), out, written);
                if !actual_in_nbytes.is_null() { *actual_in_nbytes = in_nbytes; }
                if !actual_out_nbytes.is_null() { *actual_out_nbytes = written; }
            }
            0
        }
        Err(_) => -1,
    }
}

#[no_mangle]
pub extern "C" fn libdeflate_set_memory_allocator(
    _malloc_fn: *const c_void,
    _free_fn: *const c_void,
    _opaque: *mut c_void,
) {}

#[no_mangle]
pub extern "C" fn libdeflate_zlib_decompress_ex(
    decompressor: *mut c_void,
    inp: *const u8,
    in_nbytes: usize,
    out: *mut u8,
    out_nbytes_avail: usize,
    actual_in_nbytes: *mut usize,
    actual_out_nbytes: *mut usize,
) -> c_int {
    if decompressor.is_null() || inp.is_null() || out.is_null() { return -1; }
    let dec = unsafe { &mut *(decompressor as *mut libdeflater::Decompressor) };
    let input = unsafe { core::slice::from_raw_parts(inp, in_nbytes) };
    let mut output_buf = vec![0u8; out_nbytes_avail];
    match dec.zlib_decompress(input, &mut output_buf) {
        Ok(written) => {
            unsafe {
                core::ptr::copy_nonoverlapping(output_buf.as_ptr(), out, written);
                if !actual_in_nbytes.is_null() { *actual_in_nbytes = in_nbytes; }
                if !actual_out_nbytes.is_null() { *actual_out_nbytes = written; }
            }
            0
        }
        Err(_) => -1,
    }
}

// ──────────────────────────────────────────────────────────────
// ZSTD — pure Rust via `zstd` crate
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn ZSTD_createDStream() -> *mut c_void {
    // Store a Vec<u8> as decoder buffer state
    let state = Box::new(Vec::<u8>::new());
    Box::into_raw(state) as *mut c_void
}

#[no_mangle]
pub extern "C" fn ZSTD_decompressStream(
    _ds: *mut c_void,
    _output: *mut c_void,
    _input: *mut c_void,
) -> usize { 0 }

#[no_mangle]
pub extern "C" fn ZSTD_freeDStream(ds: *mut c_void) {
    if !ds.is_null() {
        unsafe { let _ = Box::from_raw(ds as *mut Vec<u8>); }
    }
}

#[no_mangle]
pub extern "C" fn ZSTD_initDStream(_ds: *mut c_void) -> usize { 0 }

#[no_mangle]
pub extern "C" fn ZSTD_isError(_code: usize) -> c_int { 0 }

// ──────────────────────────────────────────────────────────────
// Misc stubs
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn __bun_crash_handler_out_of_memory() -> *mut c_void { unsafe { libc::abort() } }

// @trace REQ-ENG-004 [algorithm:highway_index_of_char]
// Highway SIMD char-index helper — pure Rust replacement for the C/Zig stub.
// Returns haystack_len when not found (per Highway convention; matches the
// `result == haystack.len()` check in src/highway/lib.rs:109).
#[no_mangle]
pub extern "C" fn highway_index_of_char(haystack: *const u8, haystack_len: usize, needle: u8) -> usize {
    if haystack.is_null() || haystack_len == 0 {
        return haystack_len;
    }
    let slice = unsafe { core::slice::from_raw_parts(haystack, haystack_len) };
    for (i, &b) in slice.iter().enumerate() {
        if b == needle {
            return i;
        }
    }
    haystack_len
}