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
    // directly). The <uv.h> face wired into both builds below is the real
    // header tree of the libuv supply (src/libuv_sys/vendor/libuv — oven-sh/
    // libuv `bun` @8023581113, uv 1.51.1-dev): bun_libuv_sys compiles those
    // exact sources into the static `uv` library that supplies every uv_*
    // symbol at link time, so the compile-time face and the linked objects
    // are the same tree by construction.
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
            "windows" => {
                // LIBUS_USE_LIBUV (issue #34 closed): eventing/libuv.c drives
                // the loop over the libuv supply compiled by bun_libuv_sys
                // (vendor/libuv at oven-sh/libuv `bun` @8023581113 with the
                // two upstream win-poll patches). The windows socket shape
                // stays the libusockets.h default (SOCKET / INVALID_SOCKET) —
                // see the platform-matrix note above.
                (
                    "LIBUS_USE_LIBUV",
                    usockets_src.join("eventing").join("libuv.c"),
                    usockets_src.join("crypto").join("root_certs_windows.cpp"),
                )
            }
            other => panic!("unsupported target OS for bun_uws_sys: {other}"),
        };
    let non_windows_socket_shape = target_os != "windows";

    // windows: <uv.h> include face consumed by eventing/libuv.c (C build) and
    // by libuwsockets.cpp via internal/internal.h → internal/eventing/libuv.h
    // (C++ build). See the platform-matrix note above for provenance.
    // Windows <uv.h> face: the real headers of the vendored libuv supply
    // (../libuv_sys/vendor/libuv — the same tree bun_libuv_sys's build script
    // compiles, so the header face and the uv_* objects cannot skew). The old
    // condensed 1.51.0 mirror under csrc/bun-usockets/src/deps/libuv/include
    // is no longer consumed (kept on disk for the FFI-face contract to retire).
    // The path resolves inside the workspace; fail loudly when absent rather
    // than guessing — exporting it via links metadata from bun_libuv_sys is
    // the standalone-published-crate form and rides the publish wave.
    let libuv_include = crate_dir
        .join("..")
        .join("libuv_sys")
        .join("vendor")
        .join("libuv")
        .join("include");
    let is_windows = target_os == "windows";
    if is_windows && !libuv_include.join("uv.h").exists() {
        panic!(
            "bun_uws_sys windows arm: libuv include face not found at {} — \
             build within the bao workspace (the face is vendored in \
             src/libuv_sys/vendor/libuv)",
            libuv_include.display()
        );
    }
    // clang-cl is the only windows shape exercised upstream (bun builds its
    // windows C/C++ with clang-cl) and the flags below are clang-cl literals;
    // CC/CXX env overrides still win for cross hosts.
    let (default_cc, default_cxx) = if is_windows {
        ("clang-cl", "clang-cl")
    } else {
        ("clang", "clang++")
    };

    // ── C compilation: uSockets core ──────────────────────────────────────
    // Fail-closed, narrowed to the true remaining face (issue #34 follow-up):
    // the LIBUS_USE_LIBUV eventing backend above only compiles against the
    // oven-sh/bun 4af1842c8c usockets tree (the pre-absorb mirror's
    // eventing/libuv.c still defines `void us_poll_change`, which the synced
    // tree's own header rejects). The absorb is its own wave — it drags the
    // whole crypto/TLS surface (us_socket_adopt_tls grew
    // is_client/request_cert/reject_unauthorized; SNI resume callbacks went
    // 2→4-arg) and must land with the linux TLS test discipline, not as a
    // side effect of this arm. Until it lands, refuse here with the
    // actionable gap instead of an opaque C compile error.
    if is_windows {
        let libuv_backend = usockets_src.join("eventing").join("libuv.c");
        let backend = std::fs::read_to_string(&libuv_backend).unwrap_or_default();
        if backend.contains("void us_poll_change") {
            panic!(
                "bun_uws_sys windows arm: the vendored usockets eventing/libuv.c \
                 predates the windows backend rewrite (oven-sh/bun 4af1842c8c) and \
                 cannot compile against its own header — absorb the usockets tree \
                 (incl. the crypto/TLS signature evolution: us_socket_adopt_tls, \
                 4-arg SNI resume) first, then this arm compiles as written \
                 (issue #34, platform matrix: docs/platform-support.md)"
            );
        }
    }
    let mut c_build = cc::Build::new();

    // Use clang: the uSockets C sources use __attribute__((always_inline))
    // on static functions, which is incompatible with GCC + -fPIC.
    // Bun's upstream build uses clang exclusively. A per-target CC env (cross
    // toolchain, issue #10 musl wave) must still override it: cc-rs honors
    // .compiler() before its own env chain (cc 1.2.x get_base_compiler
    // early-return), so probe the env here — clang is only the no-cross-env
    // fallback (host builds unchanged).
    c_build.compiler(env_cc("CC").unwrap_or_else(|| default_cc.into()));

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
    // posix/GCC only: the wrapper defines __has_feature as 0, which would
    // break the clang-cl build (clang-cl has real __has_feature).
    if !is_windows && wrapper_h.exists() {
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
        // winsock's setsockopt(int*) call sites are upstream-suppressed
        // (oven-sh/bun scripts/build/flags.ts carries
        // -Wno-incompatible-pointer-types globally for C); WIN32_LEAN_AND_MEAN
        // keeps windows.h from pulling wincrypt (it macro-poisons the
        // BoringSSL X509_* names some faces would otherwise collide with).
        c_build
            .flag("-Wno-incompatible-pointer-types")
            .define("WIN32_LEAN_AND_MEAN", None);
    }

    // C source files (uSockets core — platform-independent)
    let core_sources = [
        "bsd.c",
        "context.c",
        "loop.c",
        "socket.c",
        "udp.c",
        "quic.c",
        // upstream compiles the fault-injection TU unconditionally; the whole
        // body self-guards on LIBUS_SOCKET_FAULT_INJECTION (off here).
        "fault_inject.c",
    ];

    for src in &core_sources {
        // quic.c: excluded on windows until the lsquic/boringssl mirror
        // alignment lands (the pre-absorb pairing above compiles against the
        // vendored faces; the 4af rewrite does not). loop.c's unguarded
        // us_quic_* references keep any real gap a link-stage failure, not a
        // silent one.
        if is_windows && *src == "quic.c" {
            continue;
        }
        let path = usockets_src.join(src);
        // fault_inject.c arrives with the usockets absorb; it is a no-op TU
        // unless LIBUS_SOCKET_FAULT_INJECTION is armed, so a tree without it
        // simply has nothing to compile here.
        if !path.exists() {
            if *src == "fault_inject.c" {
                continue;
            }
            panic!("uSockets source file not found: {:?}", path);
        }
        c_build.file(&path);
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
        // Env-first per-target probe (see the C build note above); clang++ is
        // only the no-cross-env fallback.
        tls_cpp.compiler(env_cc("CXX").unwrap_or_else(|| default_cxx.into()));
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
    // Env-first per-target probe (see the C build note above); clang++ is
    // only the no-cross-env fallback.
    cpp_build.compiler(env_cc("CXX").unwrap_or_else(|| default_cxx.into()));
    cpp_build.cpp(true);
    cpp_build.opt_level(1);
    if is_windows {
        // MSVC driver form; clang-cl does not apply the GNU -std= spelling.
        cpp_build.flag("/std:c++20");
    } else {
        cpp_build.flag("-std=c++20");
    }
    cpp_build
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
    if !is_windows && wrapper_h.exists() {
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
        // PerMessageDeflate.h includes <zlib.h> and <libdeflate.h>: the zlib
        // face is the vendored libz-rs-sys ABI header pair (same bytes as
        // lsquic_sys's — the symbol supply rides lsquic_sys's windows
        // libz-rs-sys dependency at final link), and libdeflate is vendored
        // + compiled below (upstream bun pins ebiggers/libdeflate
        // @92e6a0db).
        cpp_build
            .include(csrc_dir.join("deps").join("zlib"))
            .include(csrc_dir.join("deps").join("libdeflate"));
    }

    cpp_build.file(crate_dir.join("libuwsockets.cpp"));
    cpp_build.compile("uwsockets");

    // ── Link dependencies ─────────────────────────────────────────────────
    if is_windows {
        // libuv's windows system libraries (vendor/libuv CMakeLists.txt
        // `uv_libraries`, MSVC arm) — consumed transitively by the
        // LIBUS_USE_LIBUV eventing backend. zlib symbols resolve from
        // lsquic_sys's windows libz-rs-sys dependency; libdeflate from the
        // vendored static lib compiled below. No pthread on windows.
        for lib in [
            "ws2_32",
            "psapi",
            "user32",
            "advapi32",
            "iphlpapi",
            "userenv",
            "dbghelp",
            "ole32",
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }
        // Vendored libdeflate (upstream bun pin ebiggers/libdeflate
        // @92e6a0db — the 11 direct sources of its `libdeflate.ts`). Same
        // effective link name as the posix system lib (`-ldeflate`).
        let libdeflate_dir = csrc_dir.join("deps").join("libdeflate");
        let mut ld = cc::Build::new();
        ld.compiler(env_cc("CC").unwrap_or_else(|| default_cc.into()))
            .opt_level(1)
            .include(&libdeflate_dir);
        for src in [
            "lib/utils.c",
            "lib/arm/cpu_features.c",
            "lib/x86/cpu_features.c",
            "lib/deflate_compress.c",
            "lib/deflate_decompress.c",
            "lib/adler32.c",
            "lib/zlib_compress.c",
            "lib/zlib_decompress.c",
            "lib/crc32.c",
            "lib/gzip_compress.c",
            "lib/gzip_decompress.c",
        ] {
            ld.file(libdeflate_dir.join(src));
        }
        ld.compile("deflate");
    } else {
        // pthread is needed for bsd.c (pthread_atfork in some code paths)
        println!("cargo:rustc-link-lib=pthread");
        // zlib for HTTP content-encoding (gzip/deflate) in libuwsockets.cpp
        println!("cargo:rustc-link-lib=z");
        // libdeflate for fast compression/decompression in libuwsockets.cpp
        println!("cargo:rustc-link-lib=deflate");
    }

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
        // libuv supply headers (windows-only consumer: eventing/libuv.c and
        // the C++ wrapper TU). Directory path → cargo watches recursively.
        println!("cargo:rerun-if-changed={}", libuv_include.display());
        println!(
            "cargo:rerun-if-changed={}",
            csrc_dir.join("deps").join("zlib").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            csrc_dir.join("deps").join("libdeflate").display()
        );
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

// cc-rs-compatible per-target compiler probe, mirroring cc's own resolution
// order: {key}_{target-hyphen} → {key}_{target-underscore} → TARGET_{key} →
// {key}. An empty value counts as unset; no hit at all means the caller's
// hardcoded fallback applies (host builds keep the historical toolchain).
fn env_cc(key: &str) -> Option<std::ffi::OsString> {
    let target = env::var("TARGET").ok()?;
    let underscored = target.replace('-', "_");
    [
        format!("{key}_{target}"),
        format!("{key}_{underscored}"),
        format!("TARGET_{key}"),
        key.to_string(),
    ]
    .into_iter()
    .find_map(|name| env::var_os(name).filter(|v| !v.is_empty()))
}
