// @trace REQ-ENG-001
//! C library stubs for uSockets, uWebSockets, lshpack, and BoringSSL extensions.
//!
//! These symbols are provided by C/C++ libraries compiled by Zig in upstream Bun.
//! Bao provides no-op stubs for test linking — actual HTTP/WS functionality
//! uses bun_http/bun_uws which link against the real C libraries at runtime.

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
// uSockets — event-driven socket abstraction
// Original: uNetworking/uSockets C library
// ──────────────────────────────────────────────────────────────

// Loop
#[no_mangle]
pub extern "C" fn us_loop_run_bun_tick(_loop: *mut c_void) {}

#[no_mangle]
pub extern "C" fn us_wakeup_loop(_loop: *mut c_void) {}

// Socket
#[no_mangle]
pub extern "C" fn us_socket_ext(_socket: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_socket_is_shut_down(_socket: *const c_void) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn us_socket_is_established(_socket: *const c_void) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn us_socket_timeout(_socket: *const c_void) -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn us_socket_long_timeout(_socket: *const c_void) -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn us_socket_keepalive(_socket: *const c_void) -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn us_socket_get_native_handle(_socket: *const c_void) -> c_int {
    -1
}

#[no_mangle]
pub extern "C" fn us_socket_close(_socket: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_socket_flush(_socket: *mut c_void) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn us_socket_write(
    _socket: *mut c_void,
    _data: *const u8,
    _len: usize,
    _msg_more: bool,
) -> usize {
    0
}

#[no_mangle]
pub extern "C" fn us_socket_sendfile_needs_more(_socket: *mut c_void) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn us_socket_get_fd(_socket: *const c_void) -> c_int {
    -1
}

#[no_mangle]
pub extern "C" fn us_socket_shutdown(_socket: *mut c_void) {}

#[no_mangle]
pub extern "C" fn us_socket_get_error(_socket: *const c_void) -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn us_socket_is_closed(_socket: *const c_void) -> bool {
    true
}

// Socket group
#[no_mangle]
pub extern "C" fn us_socket_group_init(
    _loop: *mut c_void,
    _options: *const c_void,
    _ssl_options: *const c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_socket_group_deinit(_group: *mut c_void) {}

#[no_mangle]
pub extern "C" fn us_socket_group_close_all(_group: *mut c_void) {}

#[no_mangle]
pub extern "C" fn us_socket_group_connect(
    _group: *mut c_void,
    _host: *const c_char,
    _port: c_int,
    _options: *const c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_socket_group_connect_unix(
    _group: *mut c_void,
    _path: *const c_char,
    _options: *const c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

// SSL — see "BoringSSL extensions" block below for the Wave 74-A audit
// context. These 2 us_ssl_* entry points are likewise called via FFI
// indirection (bun_uws_sys / bun_boringssl_sys extern blocks), so the stubs
// must stay until the Phase-level rustls migration replaces the whole TLS
// stack. Keeping them as safe no-ops (return null / 0).

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

// Connecting socket
#[no_mangle]
pub extern "C" fn us_connecting_socket_ext(_socket: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn us_connecting_socket_is_shut_down(_socket: *const c_void) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn us_connecting_socket_is_closed(_socket: *const c_void) -> bool {
    true
}

#[no_mangle]
pub extern "C" fn us_connecting_socket_get_native_handle(_socket: *const c_void) -> c_int {
    -1
}

#[no_mangle]
pub extern "C" fn us_connecting_socket_get_error(_socket: *const c_void) -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn us_connecting_socket_timeout(_socket: *const c_void) -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn us_connecting_socket_long_timeout(_socket: *const c_void) -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn us_connecting_socket_close(_socket: *mut c_void) {}

#[no_mangle]
pub extern "C" fn us_connecting_socket_shutdown(_socket: *mut c_void) {}

// QUIC
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
pub extern "C" fn us_quic_socket_status(_socket: *const c_void) -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn us_quic_socket_streams_avail(_socket: *const c_void) -> u32 {
    0
}

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
) {
}

#[no_mangle]
pub extern "C" fn us_quic_stream_header_count(_stream: *const c_void) -> usize {
    0
}

#[no_mangle]
pub extern "C" fn us_quic_stream_send_headers(
    _stream: *mut c_void,
    _headers: *const c_void,
    _header_len: usize,
) {
}

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
pub extern "C" fn uws_get_loop(_app: *const c_void) -> *mut c_void {
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
) {
}

#[no_mangle]
pub extern "C" fn uws_app_listen(
    _app: *mut c_void,
    _host: *const c_char,
    _host_len: usize,
    _port: c_int,
    _cb: *const c_void,
    _user_data: *mut c_void,
) {
}

// Request
#[no_mangle]
pub extern "C" fn uws_req_get_method(_req: *const c_void) -> *const c_char {
    b"GET\0".as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn uws_req_get_url(_req: *const c_void, _len: *mut usize) -> *const c_char {
    unsafe { _len.write(1) };
    b"/\0".as_ptr() as *const c_char
}

// Response
#[no_mangle]
pub extern "C" fn uws_res_write_status(_res: *mut c_void, _status: *const c_char, _len: usize) {}

#[no_mangle]
pub extern "C" fn uws_res_write_header(
    _res: *mut c_void,
    _key: *const c_char,
    _key_len: usize,
    _value: *const c_char,
    _value_len: usize,
) {
}

#[no_mangle]
pub extern "C" fn uws_res_write_header_int(
    _res: *mut c_void,
    _key: *const c_char,
    _key_len: usize,
    _value: u64,
) {
}

#[no_mangle]
pub extern "C" fn uws_res_end(_res: *mut c_void, _data: *const c_char, _len: usize) {}

// ──────────────────────────────────────────────────────────────
// BoringSSL extensions (not in system OpenSSL)
// ──────────────────────────────────────────────────────────────
//
// Wave 74-A audit (2026-06-02): these stubs ARE used — `bun_boringssl_sys`
// declares them via `extern "C"` blocks, and `bun_http::configure_http_client_with_alpn`
// (plus other consumers) calls them through `bun_boringssl::c::*`. There is
// no native BoringSSL C library linked in this build, so removing the stubs
// breaks the linker (`undefined symbol: SSL_enable_signed_cert_timestamps`).
//
// The architect's "0 caller" audit was wrong — it counted only direct
// `bao_native_stubs::SSL_*` calls and missed the FFI indirection through
// `bun_boringssl_sys`. Keeping these stubs is required until the Phase-level
// rustls migration replaces both the stubs AND the `bun_boringssl_sys` extern
// declarations with a single Rust TLS backend.

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
pub extern "C" fn SSL_enable_ocsp_stapling(_ssl: *mut c_void) -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn SSL_enable_signed_cert_timestamps(_ssl: *mut c_void) -> c_int {
    0
}

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

        us_loop_run_bun_tick(core::ptr::null_mut());
        us_wakeup_loop(core::ptr::null_mut());
        let _ = us_socket_ext(core::ptr::null());
        let _ = us_socket_is_shut_down(core::ptr::null());
        let _ = us_socket_is_established(core::ptr::null());
        let _ = us_socket_timeout(core::ptr::null());
        let _ = us_socket_long_timeout(core::ptr::null());
        let _ = us_socket_keepalive(core::ptr::null());
        let _ = us_socket_get_native_handle(core::ptr::null());
        us_socket_close(core::ptr::null_mut());
        us_socket_flush(core::ptr::null_mut());
        us_socket_write(core::ptr::null_mut(), core::ptr::null(), 0, false);
        let _ = us_socket_sendfile_needs_more(core::ptr::null_mut());
        let _ = us_socket_get_fd(core::ptr::null());
        us_socket_shutdown(core::ptr::null_mut());
        let _ = us_socket_get_error(core::ptr::null());
        let _ = us_socket_is_closed(core::ptr::null());

        us_socket_group_init(core::ptr::null_mut(), core::ptr::null(), core::ptr::null());
        us_socket_group_deinit(core::ptr::null_mut());
        us_socket_group_close_all(core::ptr::null_mut());
        us_socket_group_connect(core::ptr::null_mut(), core::ptr::null(), 0, core::ptr::null());
        us_socket_group_connect_unix(core::ptr::null_mut(), core::ptr::null(), core::ptr::null());

        let _ = us_ssl_ctx_from_options(core::ptr::null(), core::ptr::null());
        let _ = us_ssl_socket_verify_error_from_ssl(core::ptr::null());

        let _ = us_connecting_socket_ext(core::ptr::null());
        let _ = us_connecting_socket_is_shut_down(core::ptr::null());
        let _ = us_connecting_socket_is_closed(core::ptr::null());
        let _ = us_connecting_socket_get_native_handle(core::ptr::null());
        let _ = us_connecting_socket_get_error(core::ptr::null());
        let _ = us_connecting_socket_timeout(core::ptr::null());
        let _ = us_connecting_socket_long_timeout(core::ptr::null());
        us_connecting_socket_close(core::ptr::null_mut());
        us_connecting_socket_shutdown(core::ptr::null_mut());

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
        us_quic_stream_write(core::ptr::null_mut(), core::ptr::null(), 0);
        us_quic_stream_want_write(core::ptr::null_mut());
        let _ = us_quic_stream_socket(core::ptr::null());
        us_quic_stream_header(core::ptr::null(), 0, core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut());
        let _ = us_quic_stream_header_count(core::ptr::null());
        us_quic_stream_send_headers(core::ptr::null_mut(), core::ptr::null(), 0);
        let _ = us_quic_pending_connect_addrinfo(core::ptr::null_mut());
        us_quic_pending_connect_cancel(core::ptr::null_mut());
        let _ = us_quic_pending_connect_resolved(core::ptr::null_mut(), core::ptr::null());

        let _ = uws_create_app(core::ptr::null_mut(), core::ptr::null(), false);
        let _ = uws_get_loop(core::ptr::null());
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
}
