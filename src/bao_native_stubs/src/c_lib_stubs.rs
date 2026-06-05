// @trace REQ-ENG-001
//! C library stubs for uWebSockets HTTP layer, lshpack, QUIC, and BoringSSL.
//!
//! Socket I/O symbols (`us_socket_*`, `us_socket_group_*`,
//! `us_connecting_socket_*`, `us_listen_socket_*`, `us_loop_*`,
//! `us_internal_ssl_*`) are now provided by the compiled C library
//! `libusockets.a` (see `src/uws_sys/build.rs`). Loop symbols
//! (`us_loop_run_bun_tick`, `us_wakeup_loop`, `uws_get_loop`) are
//! provided by `bao_uloop`.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_char, c_int, c_void};

// ──────────────────────────────────────────────────────────────
// lshpack — HPACK header compression for HTTP/2
// Original: litespeedtech/ls-hpack C library
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn lshpack_wrapper_init(_enc: *mut c_void, _max_capacity: usize) {}

#[no_mangle]
pub extern "C" fn lshpack_wrapper_deinit(_enc: *mut c_void) {}

#[no_mangle]
pub extern "C" fn lshpack_wrapper_encode(
    _enc: *mut c_void,
    _name: *const u8,
    _name_len: usize,
    _value: *const u8,
    _value_len: usize,
    _buf: *mut u8,
    _buf_len: *mut usize,
    _is_indexed: bool,
) -> c_int {
    -1
}

#[no_mangle]
pub extern "C" fn lshpack_wrapper_enc_set_max_capacity(_enc: *mut c_void, _max_capacity: usize) {}

// ──────────────────────────────────────────────────────────────
// SSL — entry points not covered by libusockets.a
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn us_ssl_ctx_from_options(
    _options: *const c_void,
    _domain: *const c_char,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_ssl_socket_verify_error_from_ssl(_socket: *const c_void) -> c_int {
    0
}

// ──────────────────────────────────────────────────────────────
// QUIC — not compiled into libusockets.a (future Wave)
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn us_quic_global_init() {}

#[no_mangle]
pub extern "C" fn us_create_quic_client_context(
    _loop: *mut c_void,
    _options: *const c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_quic_socket_context_connect(
    _ctx: *mut c_void,
    _host: *const c_char,
    _port: c_int,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_quic_socket_context_loop(_ctx: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_quic_socket_context_on_close(_ctx: *mut c_void, _cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_socket_context_on_goaway(_ctx: *mut c_void, _cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_socket_context_on_hsk_done(_ctx: *mut c_void, _cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_socket_context_on_stream_close(_ctx: *mut c_void, _cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_socket_context_on_stream_data(_ctx: *mut c_void, _cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_socket_context_on_stream_headers(_ctx: *mut c_void, _cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_socket_context_on_stream_open(_ctx: *mut c_void, _cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_socket_context_on_stream_writable(_ctx: *mut c_void, _cb: *const c_void) {}

#[no_mangle]
pub extern "C" fn us_quic_socket_ext(_socket: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_quic_socket_make_stream(
    _socket: *mut c_void,
    _headers: *const c_void,
    _header_len: usize,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_quic_socket_status(_socket: *const c_void) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn us_quic_socket_streams_avail(_socket: *const c_void) -> u32 { 0 }

#[no_mangle]
pub extern "C" fn us_quic_stream_ext(_stream: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_quic_stream_close(_stream: *mut c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_stream_shutdown(_stream: *mut c_void) {}
#[no_mangle]
pub extern "C" fn us_quic_stream_reset(_stream: *mut c_void) {}

#[no_mangle]
pub extern "C" fn us_quic_stream_write(
    _stream: *mut c_void,
    _data: *const u8,
    _len: usize,
) -> usize {
    0
}

#[no_mangle]
pub extern "C" fn us_quic_stream_want_write(_stream: *mut c_void) {}

#[no_mangle]
pub extern "C" fn us_quic_stream_socket(_stream: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_quic_stream_header(
    _stream: *const c_void,
    _idx: usize,
    _name: *mut *const u8,
    _name_len: *mut usize,
    _value: *mut *const u8,
    _value_len: *mut usize,
) {}

#[no_mangle]
pub extern "C" fn us_quic_stream_header_count(_stream: *const c_void) -> usize { 0 }

#[no_mangle]
pub extern "C" fn us_quic_stream_send_headers(
    _stream: *mut c_void,
    _headers: *const c_void,
    _header_len: usize,
) {}

#[no_mangle]
pub extern "C" fn us_quic_pending_connect_addrinfo(_socket: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_quic_pending_connect_cancel(_socket: *mut c_void) {}

#[no_mangle]
pub extern "C" fn us_quic_pending_connect_resolved(
    _socket: *mut c_void,
    _addr: *const c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

// ──────────────────────────────────────────────────────────────
// uWebSockets — HTTP/WebSocket server C API
// Original: uNetworking/uWebSockets C++ wrapper (libuwsockets.a)
//
// SPEC (CLAUDE.md L13/L26) 禁止手写 C++ 已实现的符号的 Rust 翻译。
// `bun_uws_sys` 编译产出 libuwsockets.a，导出真实 uws_create_app /
// uws_app_any / uws_app_listen / uws_req_* / uws_res_* / us_socket_get_fd /
// us_socket_sendfile_needs_more 等符号。这里不再保留 stub —— 让 C++ 二进制
// 符号在链接器解析中胜出。
// ──────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────
// BoringSSL extensions (not in system OpenSSL)
// ──────────────────────────────────────────────────────────────
//
// These stubs ARE used — `bun_boringssl_sys` declares them via
// `extern "C"` blocks, and `bun_http::configure_http_client_with_alpn`
// calls them through `bun_boringssl::c::*`. Keeping them until
// the Phase-level rustls migration replaces the whole TLS stack.

// ──────────────────────────────────────────────────────────────
// C-library → Rust hooks (BUG-353 fallout)
// ──────────────────────────────────────────────────────────────
//
// After BUG-353 fix removed bao_uloop's 11 conflicting #[no_mangle]
// Rust symbols, the C library libusockets.a is now actually linked.
// This exposes 5 hooks the C side calls into Bun:
//
//   1. Bun__JSC_onBeforeWait  — JSC VM pre-wait hook (no-op for SM)
//   2. Bun__panic             — fatal panic from C
//   3. sys_epoll_pwait2       — Linux syscall wrapper
//   4. us_udp_socket_close    — UDP socket close (we don't compile UDP)
//   5. us_quic_loop_process   — QUIC loop tick (we don't compile QUIC)
//
// CLAUDE.md L13/L26 allows these: they are NOT C-implemented socket
// I/O (which we MUST link). They are Rust-side hooks the C library
// needs to call back. JSC→SM bridge replaces (1); the rest are
// minimal stubs matching Bun's contract.

/// JSC VM pre-wait hook. In upstream Bun this drains JSC's GC etc.
/// SpiderMonkey integration is handled via bao_engine's JobQueue, so
/// this is a no-op here.
#[no_mangle]
pub extern "C" fn Bun__JSC_onBeforeWait(_jsc_vm: *mut c_void) {}

/// Fatal panic from C. Mirrors bun_bin/phase_c_exports.rs semantics
/// but lives here so bao_bin (which doesn't depend on bun_bin) gets
/// the symbol resolved at link time.
#[no_mangle]
pub extern "C" fn Bun__panic(msg: *const u8, len: usize) -> ! {
    let msg_str = if msg.is_null() || len == 0 {
        "(no message)".to_string()
    } else {
        let slice = unsafe { core::slice::from_raw_parts(msg, len) };
        String::from_utf8_lossy(slice).into_owned()
    };
    eprintln!("Bun__panic from C: {}", msg_str);
    std::process::abort();
}

/// Linux epoll_pwait2 syscall wrapper. Mirrors bun_platform/linux.rs
/// semantics. Used by libusockets.a's epoll_kqueue.c:bun_epoll_pwait2.
#[no_mangle]
pub extern "C" fn sys_epoll_pwait2(
    epfd: c_int,
    events: *mut libc::epoll_event,
    maxevents: c_int,
    timeout: *const libc::timespec,
    sigmask: *const libc::sigset_t,
) -> isize {
    // SAFETY: direct syscall; arguments mirror the kernel ABI for epoll_pwait2(2).
    unsafe {
        libc::syscall(
            libc::SYS_epoll_pwait2,
            epfd as isize as usize,
            events as usize,
            maxevents as isize as usize,
            timeout as usize,
            sigmask as usize,
            // glibc passes 8 here (not sizeof(sigset_t)=128) — what kernel expects.
            8usize,
        ) as isize
    }
}

/// UDP socket close. No-op in plain TCP mode (libusockets.a's UDP and
/// QUIC files are not compiled in by bun_uws_sys/build.rs).
#[no_mangle]
pub extern "C" fn us_udp_socket_close(_socket: *mut c_void) {}

/// QUIC loop processing. No-op — QUIC is not compiled in.
#[no_mangle]
pub extern "C" fn us_quic_loop_process(_loop: *mut c_void) {}

// ──────────────────────────────────────────────────────────────
// BoringSSL extensions — original section continues below
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn SSL_CTX_set0_buffer_pool(
    _ctx: *mut c_void,
    _pool: *mut c_void,
) -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn CRYPTO_BUFFER_POOL_new() -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn SSL_enable_ocsp_stapling(_ssl: *mut c_void) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn SSL_enable_signed_cert_timestamps(_ssl: *mut c_void) -> c_int { 0 }

#[no_mangle]
pub extern "C" fn SSL_set_tlsext_host_name(_ssl: *mut c_void, _name: *const c_char) -> c_int {
    0
}

/// Force the linker to include all c_lib_stubs symbols.
/// Called from bao_native_stubs::force_link().
#[inline(never)]
pub fn force_c_lib_stubs() {
    lshpack_wrapper_init(core::ptr::null_mut(), 0);
    lshpack_wrapper_deinit(core::ptr::null_mut());
    let _ = lshpack_wrapper_encode(
        core::ptr::null_mut(),
        core::ptr::null(), 0,
        core::ptr::null(), 0,
        core::ptr::null_mut(), core::ptr::null_mut(),
        false,
    );
    lshpack_wrapper_enc_set_max_capacity(core::ptr::null_mut(), 0);

    // Socket/group/connecting symbols now come from libusockets.a
    // (via bun_uws_sys build.rs). No need to touch them here.

    // Loop symbols come from bao_uloop. Keep the call chain alive.
    // NOTE: Do NOT call these with null pointers — they dereference the loop
    // struct immediately (e.g. loop->num_polls). Reference the function
    // directly so the linker keeps the symbol without triggering a SIGSEGV
    // from a null deref. The `as usize` cast forces a symbol reference
    // without invoking the function body.
    let _ = bao_uloop::us_loop_run_bun_tick as *const () as usize;
    let _ = bao_uloop::us_wakeup_loop as *const () as usize;

    let _ = us_ssl_ctx_from_options(core::ptr::null(), core::ptr::null());
    let _ = us_ssl_socket_verify_error_from_ssl(core::ptr::null());

    us_quic_global_init();
    let _ = us_create_quic_client_context(core::ptr::null_mut(), core::ptr::null());
    let _ = us_quic_socket_context_connect(core::ptr::null_mut(), core::ptr::null(), 0);
    let _ = us_quic_socket_context_loop(core::ptr::null_mut());
    us_quic_socket_context_on_close(core::ptr::null_mut(), core::ptr::null());
    us_quic_socket_context_on_goaway(core::ptr::null_mut(), core::ptr::null());
    us_quic_socket_context_on_hsk_done(core::ptr::null_mut(), core::ptr::null());
    us_quic_socket_context_on_stream_close(core::ptr::null_mut(), core::ptr::null());
    us_quic_socket_context_on_stream_data(core::ptr::null_mut(), core::ptr::null());
    us_quic_socket_context_on_stream_headers(core::ptr::null_mut(), core::ptr::null());
    us_quic_socket_context_on_stream_open(core::ptr::null_mut(), core::ptr::null());
    us_quic_socket_context_on_stream_writable(core::ptr::null_mut(), core::ptr::null());
    let _ = us_quic_socket_ext(core::ptr::null());
    let _ = us_quic_socket_make_stream(core::ptr::null_mut(), core::ptr::null(), 0);
    let _ = us_quic_socket_status(core::ptr::null());
    let _ = us_quic_socket_streams_avail(core::ptr::null());

    let _ = us_quic_stream_ext(core::ptr::null());
    us_quic_stream_close(core::ptr::null_mut());
    us_quic_stream_shutdown(core::ptr::null_mut());
    us_quic_stream_reset(core::ptr::null_mut());
    let _ = us_quic_stream_write(core::ptr::null_mut(), core::ptr::null(), 0);
    us_quic_stream_want_write(core::ptr::null_mut());
    let _ = us_quic_stream_socket(core::ptr::null());
    us_quic_stream_header(
        core::ptr::null(), 0, core::ptr::null_mut(), core::ptr::null_mut(),
        core::ptr::null_mut(), core::ptr::null_mut(),
    );
    let _ = us_quic_stream_header_count(core::ptr::null());
    us_quic_stream_send_headers(core::ptr::null_mut(), core::ptr::null(), 0);
    let _ = us_quic_pending_connect_addrinfo(core::ptr::null_mut());
    us_quic_pending_connect_cancel(core::ptr::null_mut());
    let _ = us_quic_pending_connect_resolved(core::ptr::null_mut(), core::ptr::null());

    // SPEC (CLAUDE.md L13/L26): uws_* / us_socket_get_fd / us_socket_sendfile_needs_more
    // 由 libuwsockets.a (bun_uws_sys) 提供。这里不再 force_link，让真实 C++ 符号
    // 在链接器解析中胜出。
    let _ = bao_uloop::uws_get_loop();

    let _ = SSL_CTX_set0_buffer_pool(core::ptr::null_mut(), core::ptr::null_mut());
    let _ = CRYPTO_BUFFER_POOL_new();
    let _ = SSL_enable_ocsp_stapling(core::ptr::null_mut());
    let _ = SSL_enable_signed_cert_timestamps(core::ptr::null_mut());
    let _ = SSL_set_tlsext_host_name(core::ptr::null_mut(), core::ptr::null());
}