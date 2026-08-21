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
    // W0b publish incorporation: all C/C++ sources are vendored in-package
    // under csrc/ (byte-identical copies of packages/bun-usockets/src,
    // packages/bun-uws/src, vendor/lsquic public + internal headers,
    // vendor/lsqpack lsxpack_header.h, vendor/lshpack headers, and the
    // vendor/boringssl include tree). A crates.io package can only ship
    // in-package files, so the published crate carries its own source set and
    // local builds compile the exact same bytes from csrc/. Keep csrc/ in
    // sync when absorbing upstream changes.
    let csrc_dir = crate_dir.join("csrc");
    let usockets_dir = csrc_dir.join("bun-usockets");
    let usockets_src = usockets_dir.join("src");

    let with_tls = env::var("BAO_UWS_WITH_TLS").as_deref() != Ok("0");

    // ── Platform matrix (TARGET, never host) ─────────────────────────────
    // `#[cfg(target_os = ...)]` in a build script describes the HOST running
    // cargo, not the artifact being produced. Cross builds (linux host →
    // aarch64-apple-darwin) must select the eventing backend, the system
    // root-cert loader TU, and the socket descriptor shape for CARGO_CFG_
    // TARGET_OS, or the compiled C and the Rust FFI disagree.
    //
    // The matrix mirrors the dispatch the vendored C sources already apply
    // to themselves, so there is exactly one truth per platform:
    //   - libusockets.h "default eventing" block: _WIN32 → LIBUS_USE_LIBUV,
    //     __APPLE__ || __FreeBSD__ → LIBUS_USE_KQUEUE, else → LIBUS_USE_EPOLL.
    //     epoll_kqueue.c carries both the epoll and kqueue halves behind
    //     those macros; eventing/libuv.c is the windows-only backend.
    //   - root_certs.cpp call site: __APPLE__ → us_load_system_certificates_
    //     macos (root_certs_darwin.cpp, dlopen'd Security framework — no
    //     link-time framework dependency), _WIN32 → the windows loader
    //     (root_certs_windows.cpp; root_certs.cpp defines the STACK_OF(X509)
    //     wrapper itself and calls ..._windows_raw from that TU), else →
    //     root_certs_linux.cpp.
    //   - libusockets.h / bsd.h already default LIBUS_SOCKET_DESCRIPTOR /
    //     LIBUS_SOCKET_ERROR to (SOCKET, INVALID_SOCKET) under _WIN32 and
    //     (int, -1) elsewhere; the explicit -D flags below are kept only on
    //     non-windows (identical to the header defaults, preserves the
    //     historical flag set) and MUST NOT be forced on windows, where
    //     SOCKET is UINT_PTR and defining int would break the ABI.
    //
    // windows note: eventing/libuv.c pulls <uv.h> via internal/eventing/
    // libuv.h, and so does libuwsockets.cpp (it includes internal/internal.h
    // directly). The libuv public include face is vendored under
    // bun-usockets/src/deps/libuv/include (uv 1.51.0, the _WIN32 closure of
    // uv.h; byte-identical mirror in packages/bun-usockets) and wired into
    // both builds below — headers only, no libuv sources: every uv_* symbol
    // is supplied at link time by bun_libuv_sys (cfg(windows) dependency of
    // this crate), whose #[repr(C)] mirrors target the same 1.51.0.
    let target_os = env::var("CARGO_CFG_TARGET_OS")
        .expect("CARGO_CFG_TARGET_OS must be set by cargo for build scripts");

    let (eventing_macro, eventing_src, root_certs_src): (&str, PathBuf, PathBuf) =
        match target_os.as_str() {
            "linux" => (
                "LIBUS_USE_EPOLL",
                usockets_src.join("eventing/epoll_kqueue.c"),
                usockets_src.join("crypto/root_certs_linux.cpp"),
            ),
            "macos" | "ios" => (
                "LIBUS_USE_KQUEUE",
                usockets_src.join("eventing/epoll_kqueue.c"),
                usockets_src.join("crypto/root_certs_darwin.cpp"),
            ),
            "freebsd" => (
                "LIBUS_USE_KQUEUE",
                usockets_src.join("eventing/epoll_kqueue.c"),
                usockets_src.join("crypto/root_certs_linux.cpp"),
            ),
            "windows" => (
                "LIBUS_USE_LIBUV",
                usockets_src.join("eventing/libuv.c"),
                usockets_src.join("crypto/root_certs_windows.cpp"),
            ),
            other => panic!("unsupported target OS for bun_uws_sys: {other}"),
        };
    let non_windows_socket_shape = target_os != "windows";

    // windows: <uv.h> include face consumed by eventing/libuv.c (C build) and
    // by libuwsockets.cpp via internal/internal.h → internal/eventing/libuv.h
    // (C++ build). See the platform-matrix note above for provenance.
    let libuv_include = usockets_src.join("deps").join("libuv").join("include");
    let is_windows = target_os == "windows";

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
        .flag(format!("-D{}=1", eventing_macro))
        .flag("-DLIBUS_MAX_READY_POLLS=1024")
        .flag("-DLIBUS_EXT_ALIGNMENT=16");
    if non_windows_socket_shape {
        c_build
            .flag("-DLIBUS_SOCKET_DESCRIPTOR=int")
            .flag("-DLIBUS_SOCKET_ERROR=-1");
    }

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
    let lsquic_dir = csrc_dir.join("deps").join("lsquic");
    let lsqpack_dir = csrc_dir.join("deps").join("lsqpack");
    let lshpack_dir = csrc_dir.join("deps").join("lshpack");

    c_build
        .include(&usockets_dir)          // for #include "libusockets.h"
        .include(&usockets_src)          // for #include "internal/internal.h"
        .include(usockets_src.join("internal"))  // for internal/ sub-includes
        .include(usockets_src.join("internal/networking"))  // for bsd.h
        .include(lsquic_dir.join("include"))  // for #include "lsquic.h" (quic.c)
        .include(lsquic_dir.join("src").join("liblsquic"))  // for lsquic internal headers
        .include(&lsqpack_dir)           // for #include "lsxpack_header.h" (quic.c)
        .include(&lshpack_dir);          // for #include "lshpack.h" (quic.c → lsxpack)
    if is_windows {
        c_build.include(&libuv_include); // for #include <uv.h> (eventing/libuv.c)
    }

    // C source files (uSockets core — platform-independent)
    let core_sources = [
        "bsd.c",
        "context.c",
        "loop.c",
        "socket.c",
        "udp.c",
        "quic.c",
    ];

    for src in &core_sources {
        let path = usockets_src.join(src);
        if path.exists() {
            c_build.file(&path);
        } else {
            panic!("uSockets source file not found: {:?}", path);
        }
    }

    // Platform-specific eventing backend (epoll / kqueue halves both live in
    // epoll_kqueue.c; windows uses the libuv backend — see platform matrix)
    c_build.file(&eventing_src);

    // SSL: stubs or real OpenSSL
    if with_tls {
        c_build.file(usockets_src.join("crypto/openssl.c"));
        let boringssl_dir = csrc_dir.join("boringssl");
        c_build.include(boringssl_dir.join("include"));
    } else {
        c_build.file(usockets_src.join("crypto/ssl_stubs.c"));
    }

    // Skip QUIC and UDP for now (not needed for P1-B HTTP server)
    // (eventing backend is per-target — see the platform matrix above)

    c_build.compile("usockets");

    // ── C++ compilation: TLS crypto helpers (sni_tree + root_certs) ────────
    // These are C++ files that openssl.c calls into; compiled separately
    // because the main uSockets build uses the C compiler.
    if with_tls {
        let boringssl_dir = csrc_dir.join("boringssl");
        let mut tls_cpp = cc::Build::new();
        tls_cpp.compiler("clang++");
        tls_cpp.cpp(true);
        tls_cpp.opt_level(1);
        tls_cpp
            .flag("-std=c++17")
            .flag("-fno-exceptions")
            .flag("-fno-rtti")
            .flag("-DBORINGSSL_IMPLEMENTATION=1")
            .include(boringssl_dir.join("include"))
            .include(&usockets_dir)
            .include(&usockets_src)
            .include(usockets_src.join("internal"));
        tls_cpp.file(usockets_src.join("crypto/sni_tree.cpp"));
        tls_cpp.file(usockets_src.join("crypto/root_certs.cpp"));
        // Platform-specific system certificate loading (darwin dlopens the
        // Security framework at runtime, so no link-time framework is needed)
        tls_cpp.file(&root_certs_src);
        tls_cpp.compile("usockets_tls");
    }

    // ── C++ compilation: uWS C-ABI wrapper (libuwsockets.cpp) ────────────
    // Provides uws_app_*, uws_res_*, uws_req_* symbols that Rust FFI calls.
    let uws_dir = csrc_dir.join("bun-uws");
    let uws_src = uws_dir.join("src");

    let mut cpp_build = cc::Build::new();
    cpp_build.compiler("clang++");
    cpp_build.cpp(true);
    cpp_build.opt_level(1);
    cpp_build
        .flag("-std=c++20")
        .flag("-DBUN_DEBUG=1")
        .flag(format!("-D{}=1", eventing_macro))
        .flag("-DLIBUS_MAX_READY_POLLS=1024")
        .flag("-DLIBUS_EXT_ALIGNMENT=16")
        .flag("-fno-exceptions")          // uWS is compiled without exceptions
        .flag("-Wno-deprecated-declarations");
    // Must mirror the C core build: libusockets.h shapes us_loop_t / poll
    // layout by the eventing macro, so a C↔C++ macro mismatch is an ABI
    // break, not a warning. Same rule as above for the socket shape flags.
    if non_windows_socket_shape {
        cpp_build
            .flag("-DLIBUS_SOCKET_DESCRIPTOR=int")
            .flag("-DLIBUS_SOCKET_ERROR=-1");
    }

    // GCC compat wrapper
    if wrapper_h.exists() {
        cpp_build.flag(format!("-include{}", wrapper_h.display()));
    }

    if with_tls {
        cpp_build
            .flag("-DLIBUS_USE_OPENSSL=1")
            .flag("-DLIBUS_USE_BORINGSSL=1")
            .flag("-DWITH_BORINGSSL=1");
        let boringssl_dir = csrc_dir.join("boringssl");
        cpp_build.include(boringssl_dir.join("include"));
    }

    // Include paths for uWS C++ headers + uSockets internals
    cpp_build
        .include(&csrc_dir)              // for #include <bun-uws/src/App.h>
        .include(&uws_dir)               // for #include "App.h" via bun-uws/src/
        .include(&uws_src)               // for #include "App.h" etc.
        .include(&usockets_dir)           // for #include "libusockets.h"
        .include(&usockets_src)           // for #include "internal/internal.h"
        .include(usockets_src.join("internal"))
        .include(usockets_src.join("internal/networking"))
        .include(&crate_dir)             // for #include "_libusockets.h"
        .include(crate_dir.join("src")); // for #include <wtf/Assertions.h>
    if is_windows {
        // internal/internal.h → internal/eventing/libuv.h → <uv.h>
        cpp_build.include(&libuv_include);
    }

    cpp_build.file(crate_dir.join("libuwsockets.cpp"));
    cpp_build.compile("uwsockets");

    // ── Link dependencies ─────────────────────────────────────────────────
    // pthread is needed for bsd.c (pthread_atfork in some code paths)
    println!("cargo:rustc-link-lib=pthread");
    // zlib for HTTP content-encoding (gzip/deflate) in libuwsockets.cpp
    println!("cargo:rustc-link-lib=z");
    // libdeflate for fast compression/decompression in libuwsockets.cpp
    println!("cargo:rustc-link-lib=deflate");

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
    println!("cargo:rerun-if-changed={}", usockets_src.join("udp.c").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("crypto/ssl_stubs.c").display());
    println!("cargo:rerun-if-changed={}", eventing_src.display());
    if with_tls {
        println!("cargo:rerun-if-changed={}", root_certs_src.display());
        // openssl.c is the TLS core TU (compiled only under with_tls);
        // without this entry, touching it silently keeps the stale object.
        println!("cargo:rerun-if-changed={}", usockets_src.join("crypto/openssl.c").display());
        // Same rule for the two C++ TUs linked into usockets_tls.
        println!("cargo:rerun-if-changed={}", usockets_src.join("crypto/sni_tree.cpp").display());
        println!("cargo:rerun-if-changed={}", usockets_src.join("crypto/root_certs.cpp").display());
    }
    println!("cargo:rerun-if-changed={}", usockets_src.join("internal/internal.h").display());
    println!("cargo:rerun-if-changed={}", usockets_src.join("libusockets.h").display());
    if is_windows {
        // Vendored libuv include face (windows-only consumer: eventing/libuv.c
        // and the C++ wrapper TU). Directory path → cargo watches recursively.
        println!("cargo:rerun-if-changed={}", libuv_include.display());
    }

    // ── Rebuild hints: C++ wrapper TU ─────────────────────────────────────
    // libuwsockets.cpp is one translation unit that #includes the uWS headers
    // (App.h → HttpContext.h → HttpParser.h → HttpContextData.h …), so an edit
    // to any of them must rerun this script. `cc` only emits rerun-if-changed
    // for the compiled .cpp file itself, never for headers — without these
    // lines a header-only change (e.g. an absorbed upstream parser fix)
    // silently links the stale archive.
    println!("cargo:rerun-if-changed={}", crate_dir.join("libuwsockets.cpp").display());
    println!("cargo:rerun-if-changed={}", crate_dir.join("libuwsockets_h3.cpp").display());
    println!("cargo:rerun-if-changed={}", crate_dir.join("_libusockets.h").display());
    if let Ok(entries) = std::fs::read_dir(&uws_src) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "h" || ext == "cpp") {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    // wtf/ helpers are included as <wtf/*.h> from the crate's src dir.
    if let Ok(entries) = std::fs::read_dir(crate_dir.join("src").join("wtf")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "h") {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}
