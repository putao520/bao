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
// Original: uNetworking/uWebSockets C++ wrapper
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn uws_create_app(
    _loop: *mut c_void,
    _options: *const c_void,
    _is_ssl: bool,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn uws_app_any(
    _app: *mut c_void,
    _method: c_int,
    _pattern: *const c_char,
    _pattern_len: usize,
    _handler: *const c_void,
    _user_data: *mut c_void,
) {}

#[no_mangle]
pub extern "C" fn uws_app_listen(
    _app: *mut c_void,
    _host: *const c_char,
    _host_len: usize,
    _port: c_int,
    _cb: *const c_void,
    _user_data: *mut c_void,
) {}

#[no_mangle]
pub extern "C" fn uws_req_get_method(_req: *const c_void) -> *const c_char {
    c"GET".as_ptr()
}

#[no_mangle]
pub extern "C" fn uws_req_get_url(_req: *const c_void, _len: *mut usize) -> *const c_char {
    unsafe { _len.write(1) };
    c"/".as_ptr()
}

#[no_mangle]
pub extern "C" fn uws_res_write_status(_res: *mut c_void, _status: *const c_char, _len: usize) {}

#[no_mangle]
pub extern "C" fn uws_res_write_header(
    _res: *mut c_void,
    _key: *const c_char,
    _key_len: usize,
    _value: *const c_char,
    _value_len: usize,
) {}

#[no_mangle]
pub extern "C" fn uws_res_write_header_int(
    _res: *mut c_void,
    _key: *const c_char,
    _key_len: usize,
    _value: u64,
) {}

#[no_mangle]
pub extern "C" fn uws_res_end(_res: *mut c_void, _data: *const c_char, _len: usize) {}

// ──────────────────────────────────────────────────────────────
// Socket utility functions (not in libusockets.a — Bun C++ layer)
// ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn us_socket_get_fd(_s: *const c_void) -> c_int { -1 }

#[no_mangle]
pub extern "C" fn us_socket_sendfile_needs_more(_s: *mut c_void) -> c_int { 0 }

// ──────────────────────────────────────────────────────────────
// BoringSSL extensions (not in system OpenSSL)
// ──────────────────────────────────────────────────────────────
//
// These stubs ARE used — `bun_boringssl_sys` declares them via
// `extern "C"` blocks, and `bun_http::configure_http_client_with_alpn`
// calls them through `bun_boringssl::c::*`. Keeping them until
// the Phase-level rustls migration replaces the whole TLS stack.

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
    unsafe {
        bao_uloop::us_loop_run_bun_tick(core::ptr::null_mut(), core::ptr::null());
        bao_uloop::us_wakeup_loop(core::ptr::null_mut());
    }

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

    let _ = uws_create_app(core::ptr::null_mut(), core::ptr::null(), false);
    let _ = bao_uloop::uws_get_loop();
    uws_app_any(core::ptr::null_mut(), 0, core::ptr::null(), 0, core::ptr::null(), core::ptr::null_mut());
    uws_app_listen(core::ptr::null_mut(), core::ptr::null(), 0, 0, core::ptr::null(), core::ptr::null_mut());
    let _ = uws_req_get_method(core::ptr::null());
    let mut url_len = 0usize;
    let _ = uws_req_get_url(core::ptr::null(), &mut url_len);
    uws_res_write_status(core::ptr::null_mut(), core::ptr::null(), 0);
    uws_res_write_header(core::ptr::null_mut(), core::ptr::null(), 0, core::ptr::null(), 0);
    uws_res_write_header_int(core::ptr::null_mut(), core::ptr::null(), 0, 0);
    uws_res_end(core::ptr::null_mut(), core::ptr::null(), 0);

    let _ = SSL_CTX_set0_buffer_pool(core::ptr::null_mut(), core::ptr::null_mut());
    let _ = CRYPTO_BUFFER_POOL_new();
    let _ = SSL_enable_ocsp_stapling(core::ptr::null_mut());
    let _ = SSL_enable_signed_cert_timestamps(core::ptr::null_mut());
    let _ = SSL_set_tlsext_host_name(core::ptr::null_mut(), core::ptr::null());

    // Socket utility stubs
    let _ = us_socket_get_fd(core::ptr::null());
    let _ = us_socket_sendfile_needs_more(core::ptr::null_mut());
}