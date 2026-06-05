// Build script for bun_uws_sys: compiles the uSockets C library (libusockets)
// using the `cc` crate. This provides real us_socket_* / us_socket_group_*
// symbols, replacing the no-op stubs in bao_native_stubs.
//
// Two compilation modes:
//   1. Plain TCP (default): compiles C sources without BoringSSL, links
//      crypto/ssl_stubs.c for us_internal_ssl_* no-ops.
//   2. With TLS (future, Wave 74-TLS): define BAO_UWS_WITH_TLS, compile
//      crypto/openssl.c, link BoringSSL.

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = crate_dir.parent().unwrap().parent().unwrap().to_path_buf();
    let packages_dir = workspace_dir.join("packages");
    let usockets_dir = packages_dir.join("bun-usockets");
    let usockets_src = usockets_dir.join("src");

    let with_tls = env::var("BAO_UWS_WITH_TLS").is_ok();

    // ── C compilation: uSockets core ──────────────────────────────────────
    let mut c_build = cc::Build::new();

    // Use clang: the uSockets C sources use __attribute__((always_inline))
    // on static functions, which is incompatible with GCC + -fPIC.
    // Bun's upstream build uses clang exclusively.
    c_build.compiler("clang");

    // Compiler flags
    c_build
        .opt_level(1)                   // -O1: always_inline requires optimization
        .flag("-DBUN_DEBUG=1")           // makes nonnull_arg/nonnull_fn_decl expand to empty
        .flag("-DLIBUS_USE_EPOLL=1")
        .flag("-DLIBUS_MAX_READY_POLLS=1024")
        .flag("-DLIBUS_SOCKET_DESCRIPTOR=int")
        .flag("-DLIBUS_SOCKET_ERROR=-1")
        .flag("-DLIBUS_EXT_ALIGNMENT=16");

    // GCC compat: __has_feature is Clang-only. Define it as 0 via a wrapper
    // flag. We use a separate .h file to define it as a function-like macro.
    let wrapper_h = crate_dir.join("src").join("_gcc_compat.h");
    if wrapper_h.exists() {
        c_build.flag(format!("-include{}", wrapper_h.display()));
    }

    if with_tls {
        c_build
            .flag("-DLIBUS_USE_OPENSSL=1")
            .flag("-DLIBUS_USE_BORINGSSL=1")
            .flag("-DWITH_BORINGSSL=1");
    }

    // Include paths
    c_build
        .include(&usockets_dir)          // for #include "libusockets.h"
        .include(&usockets_src)          // for #include "internal/internal.h"
        .include(usockets_src.join("internal"))  // for internal/ sub-includes
        .include(usockets_src.join("internal/networking"));  // for bsd.h

    // C source files (uSockets core — platform-independent)
    let core_sources = [
        "bsd.c",
        "context.c",
        "loop.c",
        "socket.c",
    ];

    for src in &core_sources {
        let path = usockets_src.join(src);
        if path.exists() {
            c_build.file(&path);
        } else {
            panic!("uSockets source file not found: {:?}", path);
        }
    }

    // Platform-specific eventing backend
    #[cfg(target_os = "linux")]
    {
        c_build.file(usockets_src.join("eventing/epoll_kqueue.c"));
    }

    #[cfg(target_os = "macos")]
    {
        c_build.file(usockets_src.join("eventing/epoll_kqueue.c"));
    }

    // SSL: stubs or real OpenSSL
    if with_tls {
        c_build.file(usockets_src.join("crypto/openssl.c"));
        // TODO: add BoringSSL include paths and link flags (Wave 74-TLS)
    } else {
        c_build.file(usockets_src.join("crypto/ssl_stubs.c"));
    }

    // Skip QUIC and UDP for now (not needed for P1-B HTTP server)
    // Skip libuv eventing backend (we use epoll/kqueue)

    c_build.compile("usockets");

    // ── C++ compilation: uWS C-ABI wrapper (libuwsockets.cpp) ────────────
    // Provides uws_app_*, uws_res_*, uws_req_* symbols that Rust FFI calls.
    let uws_dir = packages_dir.join("bun-uws");
    let uws_src = uws_dir.join("src");

    let mut cpp_build = cc::Build::new();
    cpp_build.compiler("clang++");
    cpp_build.cpp(true);
    cpp_build.opt_level(1);
    cpp_build
        .flag("-std=c++20")
        .flag("-DBUN_DEBUG=1")
        .flag("-DLIBUS_USE_EPOLL=1")
        .flag("-DLIBUS_MAX_READY_POLLS=1024")
        .flag("-DLIBUS_SOCKET_DESCRIPTOR=int")
        .flag("-DLIBUS_SOCKET_ERROR=-1")
        .flag("-DLIBUS_EXT_ALIGNMENT=16")
        .flag("-fno-exceptions")          // uWS is compiled without exceptions
        .flag("-Wno-deprecated-declarations");

    // GCC compat wrapper
    if wrapper_h.exists() {
        cpp_build.flag(format!("-include{}", wrapper_h.display()));
    }

    if with_tls {
        cpp_build
            .flag("-DLIBUS_USE_OPENSSL=1")
            .flag("-DLIBUS_USE_BORINGSSL=1")
            .flag("-DWITH_BORINGSSL=1");
    }

    // Include paths for uWS C++ headers + uSockets internals
    cpp_build
        .include(&packages_dir)          // for #include <bun-uws/src/App.h>
        .include(&uws_dir)               // for #include "App.h" via bun-uws/src/
        .include(&uws_src)               // for #include "App.h" etc.
        .include(&usockets_dir)           // for #include "libusockets.h"
        .include(&usockets_src)           // for #include "internal/internal.h"
        .include(usockets_src.join("internal"))
        .include(usockets_src.join("internal/networking"))
        .include(&crate_dir)             // for #include "_libusockets.h"
        .include(crate_dir.join("src")); // for #include <wtf/Assertions.h>

    cpp_build.file(crate_dir.join("libuwsockets.cpp"));
    cpp_build.compile("uwsockets");

    // ── Link dependencies ─────────────────────────────────────────────────
    // pthread is needed for bsd.c (pthread_atfork in some code paths)
    println!("cargo:rustc-link-lib=pthread");

    // SPEC (CLAUDE.md L13): libuwsockets.a (C++ wrapper) depends on libusockets.a
    // (C core). For static archives, the linker resolves undefined symbols only
    // from libraries listed AFTER the reference. cc::compile emits
    // `cargo:rustc-link-lib=static=usockets` BEFORE
    // `cargo:rustc-link-lib=static=uwsockets`, which puts usockets first in the
    // link line — but uwsockets (compiled later) references symbols in usockets,
    // so usockets must come AFTER uwsockets. Re-declare usockets LAST to fix the
    // order (Cargo dedupes link libs in dep-graph order, so this becomes the
    // effective position).
    println!("cargo:rustc-link-lib=static=usockets");

    // ── Rebuild hints ─────────────────────────────────────────────────────
    // Rebuild if any C source changes
    println!("cargo:rerun-if-changed={}", usockets_src.join("bsd.c").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("context.c").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("loop.c").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("socket.c").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("crypto/ssl_stubs.c").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("eventing/epoll_kqueue.c").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("internal/internal.h").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("libusockets.h").display());
}
