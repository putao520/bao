// @trace REQ-ENG-001
//! C library hooks for uWebSockets HTTP layer, BoringSSL TLS, and DNS.
//!
//! Compiled C libraries (all via cc::Build in respective build.rs):
//! - `libusockets.a` (bun_uws_sys): socket I/O, HTTP, WebSocket
//! - `libusockets_tls.a` (bun_uws_sys): TLS via BoringSSL
//! - `libbrotli.a` (bun_brotli_sys): Brotli compression
//! - `libzstd.a` (bun_zstd): Zstandard compression
//! - `liblibdeflate.a` (bun_libdeflate_sys): deflate/gzip/zlib
//! - `liblshpack.a` (bun_lshpack_sys): HPACK header compression
//! - `libmimalloc.a` (bun_mimalloc_sys): memory allocator
//! - `libhighway.a` + `libhighway_strings.a` (bun_highway): SIMD string ops
//!
//! Loop symbols (`us_loop_run_bun_tick`, `us_wakeup_loop`, `uws_get_loop`)
//! are provided by `bao_uloop`.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_char, c_int, c_void};

// ──────────────────────────────────────────────────────────────
// lshpack — HPACK header compression for HTTP/2
// Provided by compiled C library: bun_lshpack_sys (vendor/lshpack)
// ──────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────
// SSL — provided by libusockets.a + libusockets_tls.a when compiled with TLS
// ──────────────────────────────────────────────────────────────

/// BoringSSL CRYPTO_EX_free callback. Tombstones the SSLContextCache entry
/// when the last SSL_CTX ref drops. The real implementation in Bun's
/// SSLContextCache clears `entry.ctx = null`; this no-op is safe when the
/// cache is not yet wired — every TLS handshake creates a fresh SSL_CTX.
#[no_mangle]
pub extern "C" fn bun_ssl_ctx_cache_on_free(
    _parent: *mut c_void,
    _ptr: *mut c_void,
    _ad: *mut c_void,
    _index: c_int,
    _argl: i64,
    _argp: *mut c_void,
) {}

// us_get_default_ca_store / us_get_shared_default_ca_store — provided by
// compiled C++ code (root_certs.cpp in libusockets_tls.a).

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
// TLS C→Rust callbacks (root_certs.cpp)
// ──────────────────────────────────────────────────────────────

/// Global flag: whether to load system CA certificates.
/// In Bun, this is set by `--use-system-ca` CLI flag or `NODE_USE_SYSTEM_CA=1`.
/// Default `true` — always load system CAs for TLS verification.
#[no_mangle]
pub static mut Bun__Node__UseSystemCA: bool = true;

/// Warning callback when loading extra CA files fails.
/// Called by `root_certs.cpp` when a certificate file in the system CA
/// directory cannot be parsed.
#[no_mangle]
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
    eprintln!("warn: ignoring extra certs from {}, load failed: {}", filename_str, error_str);
}

// ──────────────────────────────────────────────────────────────
// C-library → Rust hooks (non-duplicate subset)
// ──────────────────────────────────────────────────────────────
//
// The following hooks live in `bun_uws_sys/src/c_hooks.rs` (co-located
// with the C code that references them) and must NOT be duplicated here:
//
//   - Bun__JSC_onBeforeWait       — JSC VM pre-wait hook
//   - Bun__panic                  — fatal panic from C
//   - sys_epoll_pwait2            — Linux syscall wrapper
//   - Bun__lock__size             — mutex size validation
//   - Bun__isEpollPwait2SupportedOnLinuxKernel — epoll_pwait2 check
//
// Remaining hooks that are specific to bao_native_stubs' link scope:
//   - Bun__Node__UseSystemCA      — system CA flag (root_certs.cpp)
//   - BUN__warn__extra_ca_load_failed — warning callback (root_certs.cpp)
//   - bun_ssl_ctx_cache_on_free   — BoringSSL EX_free callback

// ──────────────────────────────────────────────────────────────
// BoringSSL extensions — now provided by compiled C++ library (bun_boringssl_sys)
// ──────────────────────────────────────────────────────────────

/// Force the linker to include all c_lib_stubs symbols.
/// Called from bao_native_stubs::force_link().
#[inline(never)]
pub fn force_c_lib_stubs() {
    // Force bun_lshpack_sys native link dependency propagation.
    let _ = bun_lshpack_sys::force_link as *const () as usize;
    // Force bun_boringssl_sys native link dependency propagation.
    let _ = bun_boringssl_sys::force_link as *const () as usize;
    // Force bun_lsquic_sys native link dependency propagation.
    let _ = bun_lsquic_sys::force_link as *const () as usize;
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

    // SSL symbols now come from libusockets.a (compiled with TLS).

    // QUIC symbols now come from libusockets.a (quic.c) + liblsquic.a (bun_lsquic_sys).
    // No need to force_link individual us_quic_* functions — the compiled C code
    // provides all of them and the linker resolves references automatically.

    // SPEC (CLAUDE.md L13/L26): uws_* / us_socket_get_fd / us_socket_sendfile_needs_more
    // 由 libuwsockets.a (bun_uws_sys) 提供。这里不再 force_link，让真实 C++ 符号
    // 在链接器解析中胜出。
    let _ = bao_uloop::uws_get_loop();

    // BoringSSL symbols now come from compiled C++ library (bun_boringssl_sys).
}