// @trace REQ-ENG-001
//! C library hooks for uWebSockets HTTP layer, BoringSSL TLS, and DNS.
//!
//! Compiled C libraries (all via cc::Build in respective build.rs):
//! - `libusockets.a` (bun_uws_sys): socket I/O, HTTP, WebSocket
//! - `libusockets_tls.a` (bun_uws_sys): TLS via BoringSSL
//! - `libbrotli.a` (bun_brotli_sys): Brotli compression
//! - `libzstd.a` (bun_zstd): Zstandard compression
//! - `liblshpack.a` (bun_lsquic_sys): HPACK header compression
//! - `libmimalloc.a` (bun_mimalloc_sys): memory allocator
//! - `libhighway.a` + `libhighway_strings.a` (bun_highway): SIMD string ops
//!
//! Loop symbols (`us_loop_run_bun_tick`, `us_wakeup_loop`, `uws_get_loop`)
//! are provided by `bao_uloop`.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// ──────────────────────────────────────────────────────────────
// lshpack — HPACK header compression for HTTP/2
// Provided by compiled C library: bun_lsquic_sys (vendor/lshpack, merged)
// ──────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────
// SSL — provided by libusockets.a + libusockets_tls.a when compiled with TLS
// ──────────────────────────────────────────────────────────────

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
// C-library → Rust hooks (all owned elsewhere; do NOT define here)
// ──────────────────────────────────────────────────────────────
//
// Dual-def iron rule (STUB-INVENTORY): every hook below has exactly one
// `#[no_mangle]` definition in its owning crate:
//
//   - Bun__JSC_onBeforeWait       — bun_uws_sys/src/c_hooks.rs
//   - Bun__panic                  — bun_uws_sys/src/c_hooks.rs
//   - sys_epoll_pwait2            — bun_uws_sys/src/c_hooks.rs
//   - Bun__lock__size             — bun_threading::Mutex (real size of ReleaseImpl)
//   - Bun__isEpollPwait2SupportedOnLinuxKernel — bun_analytics (kernel version check)
//   - __bun_crash_handler_out_of_memory — bun_crash_handler
//
// TLS C→Rust hooks (root_certs.cpp / openssl.c us_ctx_cache_ex_idx) moved to
// their named owner `bao_uloop` (chained into every link scope that pulls the
// uSockets TLS archives). Do NOT reintroduce copies here — dual-def with
// bao_uloop / bun_runtime::product_native_symbols breaks consumer links:
//   - Bun__Node__UseSystemCA      — system CA flag (root_certs.cpp)
//   - BUN__warn__extra_ca_load_failed — warning callback (root_certs.cpp)
//   - bun_ssl_ctx_cache_on_free   — BoringSSL EX_free callback (openssl.c)

// ──────────────────────────────────────────────────────────────
// BoringSSL extensions — now provided by compiled C++ library (bun_boringssl_sys)
// ──────────────────────────────────────────────────────────────

/// Force the linker to include all c_lib_stubs symbols.
/// Called from bao_native_stubs::force_link().
#[inline(never)]
pub fn force_c_lib_stubs() {
    // Force lshpack native link dependency propagation (now merged into bun_lsquic_sys).
    let _ = bun_lsquic_sys::force_link_lshpack as *const () as usize;
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
    let _ = bao_uloop::bao_loop_tick as *const () as usize;
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
