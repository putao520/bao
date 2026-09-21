// Build script for bun_lsquic_sys: compiles lsquic (LiteSpeed QUIC/HTTP3)
// + lsqpack (QPACK header compression) using the `cc` crate.
//
// Mirrors Bun's scripts/build/deps/lsquic.ts exactly:
//   - ~70 C files from src/liblsquic/
//   - lsqpack.c compiled inline (lsquic feeds it a non-FILE* logger context)
//   - Links against BoringSSL + lshpack + zlib
//
// Windows arm (W2.5, issue #18; upstream lsquic.ts `cfg.windows` branches):
//   - includes += wincompat (lsquic) + lsqpack/wincompat + lshpack
//     compat/windows — the vendored header shims (sys/queue.h, vc_compat.h,
//     sys/uio.h) that upstream adds for win32
//   - defines += WIN32 / WIN32_LEAN_AND_MEAN
//   - compiler defaults to clang-cl (the shape upstream exercises; CC_<triple>
//     env overrides win, same probe discipline as bun_libuv_sys/build.rs)
//   - links: ws2_32 replaces pthread/m per lsquic's own CMakeLists.txt
//     (`IF (NOT MSVC) ... pthread m ELSE ... ws2_32`); no `-z`.
//     zlib: upstream compiles zlib-ng into every build, but this compile set
//     references no zlib symbols — the only zlib.h includes are under
//     `#if LOG_PACKET_CHECKSUM` (0) or in files outside the IETF-only source
//     array (crt_compress/handshake/crypto). Cross-probe evidence: 72/72 TUs
//     compile and the llvm-nm undefined table of the archived lsquic.lib has
//     zero deflate*/inflate* entries — so no zlib supply (and no libz-rs-sys
//     dep) is needed on any platform for this crate's compile face; the
//     non-windows `-z` line is kept byte-identical to before anyway.

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // The platform matrix keys on TARGET (CARGO_CFG_*), never the host: a
    // linux host cross-building x86_64-pc-windows-msvc must still compile the
    // C with the windows arm. Same discipline as bun_libuv_sys/build.rs.
    let is_windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    // W0a/W0b publish incorporation: all C sources are vendored in-package
    // under csrc/ (byte-identical copies of vendor/lsquic include + liblsquic
    // trees, the lsqpack/lshpack compile set incl. deps/xxhash and generated
    // huff-tables.h, and the vendor/boringssl include tree). A crates.io
    // package can only ship in-package files, so local builds and published
    // builds compile the exact same bytes from csrc/. Keep csrc/ in sync when
    // absorbing upstream lsquic/lsqpack/lshpack/boringssl.
    let csrc_dir = crate_dir.join("csrc");

    let lsquic_dir = csrc_dir.join("lsquic");
    let lsquic_src = lsquic_dir.join("liblsquic");
    let lsqpack_dir = csrc_dir.join("lsqpack");
    let lshpack_dir = csrc_dir.join("lshpack");
    let boringssl_dir = csrc_dir.join("boringssl");

    // ── lshpack compilation (merged from bun_lshpack_sys) ────────────────
    let mut lshpack_build = cc::Build::new();
    // Env-first per-target probe: cc-rs honors .compiler() before its own
    // CC_<target> env chain (cc 1.2.x get_base_compiler early-return), so
    // hardcoding clang silently bypasses per-target cross toolchains (issue
    // #10 musl wave). clang / clang-cl are only the no-cross-env fallbacks —
    // host builds unchanged (see env_cc below).
    lshpack_build.compiler(env_cc("CC").unwrap_or_else(|| fallback_cc(is_windows)));
    lshpack_build.opt_level(2);
    lshpack_build
        .flag("-DLS_HPACK_USE_LARGE_TABLES=1")
        .flag("-DLS_HPACK_BSS_LARGE_TABLES=1")
        .flag("-DXXH_HEADER_NAME=\"xxhash.h\"");
    lshpack_build.include(&csrc_dir); // vendored sys/queue.h first (musl, p3.5)
    lshpack_build.include(&lshpack_dir);
    lshpack_build.include(lshpack_dir.join("deps/xxhash"));
    if is_windows {
        // upstream lshpack.ts `cfg.windows` includes: ["compat/windows"]
        // (sys/uio.h shim; sys/queue.h is already served first by csrc/sys).
        lshpack_build.include(lshpack_dir.join("compat/windows"));
    }
    lshpack_build.file(lshpack_dir.join("lshpack.c"));
    lshpack_build.file(lshpack_dir.join("deps/xxhash/xxhash.c"));
    lshpack_build.file(crate_dir.join("src/lshpack_wrapper.c"));
    lshpack_build.compile("lshpack");

    // ── lsquic C source files (mirrors Bun's liblsquic array) ──────────────
    let lsquic_sources = [
        "ls-sfparser.c",
        "lsquic_adaptive_cc.c",
        "lsquic_alarmset.c",
        "lsquic_arr.c",
        "lsquic_attq.c",
        "lsquic_bbr.c",
        "lsquic_bw_sampler.c",
        "lsquic_cfcw.c",
        "lsquic_conn.c",
        "lsquic_crand.c",
        "lsquic_cubic.c",
        "lsquic_di_error.c",
        "lsquic_di_hash.c",
        "lsquic_di_nocopy.c",
        "lsquic_enc_sess_common.c",
        "lsquic_enc_sess_ietf.c",
        "lsquic_eng_hist.c",
        "lsquic_engine.c",
        "lsquic_ev_log.c",
        "lsquic_frab_list.c",
        "lsquic_full_conn_ietf.c",
        "lsquic_global.c",
        "lsquic_gquic_stubs.c",
        "lsquic_hash.c",
        "lsquic_hcsi_reader.c",
        "lsquic_hcso_writer.c",
        "lsquic_hkdf.c",
        "lsquic_hpi.c",
        "lsquic_http.c",
        "lsquic_http1x_if.c",
        "lsquic_logger.c",
        "lsquic_malo.c",
        "lsquic_min_heap.c",
        "lsquic_mini_conn_ietf.c",
        "lsquic_minmax.c",
        "lsquic_mm.c",
        "lsquic_pacer.c",
        "lsquic_packet_common.c",
        "lsquic_packet_in.c",
        "lsquic_packet_out.c",
        "lsquic_packet_resize.c",
        "lsquic_parse_common.c",
        "lsquic_parse_gquic_common.c",
        "lsquic_parse_ietf_v1.c",
        "lsquic_parse_iquic_common.c",
        "lsquic_pr_queue.c",
        "lsquic_purga.c",
        "lsquic_qdec_hdl.c",
        "lsquic_qenc_hdl.c",
        "lsquic_qlog.c",
        "lsquic_qpack_exp.c",
        "lsquic_rechist.c",
        "lsquic_rtt.c",
        "lsquic_send_ctl.c",
        "lsquic_senhist.c",
        "lsquic_set.c",
        "lsquic_sfcw.c",
        "lsquic_spi.c",
        "lsquic_stock_shi.c",
        "lsquic_str.c",
        "lsquic_stream.c",
        "lsquic_tokgen.c",
        "lsquic_trans_params.c",
        "lsquic_trechist.c",
        "lsquic_util.c",
        "lsquic_varint.c",
        "lsquic_version.c",
        "lsquic_versions_to_string.c",
    ];

    let mut build = cc::Build::new();
    // Env-first per-target probe (see the lshpack note above); clang / clang-cl
    // are only the no-cross-env fallbacks.
    build.compiler(env_cc("CC").unwrap_or_else(|| fallback_cc(is_windows)));
    build.opt_level(1);
    // lsquic emits many -Wsign-compare and -Wunused; upstream builds with -Werror
    // disabled. Suppress all warnings (treat as third-party lib). Upstream
    // feeds the same -w to its windows (clang-cl) build (lsquic.ts / lshpack.ts
    // cflags), so the flag is unconditional.
    build.flag("-w");

    // Defines (mirrors Bun's lsquic.ts)
    build
        .define("HAVE_BORINGSSL", Some("1"))
        // XXH_HEADER_NAME must be a quoted string for #include XXH_HEADER_NAME.
        // cc::Build .define() doesn't add quotes, so use .flag() instead.
        .flag("-DXXH_HEADER_NAME=\"xxhash.h\"")
        .define("LS_QPACK_USE_LARGE_TABLES", Some("1"))
        .define("LS_HPACK_BSS_LARGE_TABLES", Some("1"))
        .flag("-DLSQPACK_ENC_LOGGER_HEADER=\"lsquic_qpack_enc_logger.h\"")
        .flag("-DLSQPACK_DEC_LOGGER_HEADER=\"lsquic_qpack_dec_logger.h\"")
        .define("LSQUIC_DEBUG_NEXT_ADV_TICK", Some("0"))
        .define("LSQUIC_CONN_STATS", Some("0"))
        .define("LSQUIC_QIR", Some("0"))
        .define("LSQUIC_WEBTRANSPORT_SERVER_SUPPORT", Some("0"));
    if is_windows {
        // upstream lsquic.ts `cfg.windows` defines.
        build
            .define("WIN32", Some("1"))
            .define("WIN32_LEAN_AND_MEAN", Some("1"));
    }

    // Include paths. csrc/ comes first so `#include <sys/queue.h>` resolves
    // to the vendored BSD queue.h (csrc/sys/queue.h) before any system path —
    // musl has no sys/queue.h (issue #10 musl wave, p3.5), and windows gets
    // the same resolution ahead of the wincompat shims.
    build
        .include(&csrc_dir)
        .include(lsquic_dir.join("include"))
        .include(&lsquic_src)
        .include(boringssl_dir.join("include"))
        .include(&lshpack_dir)
        .include(lshpack_dir.join("deps").join("xxhash"))
        .include(&lsqpack_dir)
        .include(lsqpack_dir.join("deps").join("xxhash"));
    if is_windows {
        // upstream lsquic.ts `cfg.windows` includes: lsquic wincompat +
        // lsqpack wincompat (vc_compat.h / sys/queue.h header shims) and
        // lshpack compat/windows (sys/uio.h shim).
        build
            .include(lsquic_dir.join("wincompat"))
            .include(lsqpack_dir.join("wincompat"))
            .include(lshpack_dir.join("compat/windows"));
    }

    // Add lsquic C sources
    for src in &lsquic_sources {
        let path = lsquic_src.join(src);
        if path.exists() {
            build.file(&path);
        } else {
            panic!("lsquic source file not found: {:?}", path);
        }
    }

    // lsqpack.c compiled inline (lsquic feeds it a non-FILE* logger context)
    // lsqpack vendors its own xxhash; compile it to provide XXH32/XXH64.
    build.file(lsqpack_dir.join("lsqpack.c"));
    build.file(lsqpack_dir.join("deps").join("xxhash").join("xxhash.c"));

    build.compile("lsquic");

    // ── Link dependencies ─────────────────────────────────────────────────
    // Non-windows: lsquic depends on zlib (system lib). BoringSSL and lshpack
    // are propagated via Cargo dependencies (bun_boringssl_sys, this crate).
    //
    // Windows: ws2_32 per lsquic's own CMakeLists.txt (see header note); no
    // `-z` — the compile set references no zlib symbols (probe evidence in
    // the header note), so there is nothing for a zlib supply to resolve.
    if is_windows {
        println!("cargo:rustc-link-lib=ws2_32");
    } else {
        println!("cargo:rustc-link-lib=z");
    }

    // ── Rebuild hints ─────────────────────────────────────────────────────
    println!("cargo:rerun-if-changed={}", lsquic_src.join("lsquic_engine.c").display());
    println!("cargo:rerun-if-changed={}", lsqpack_dir.join("lsqpack.c").display());
    println!("cargo:rerun-if-changed={}", lshpack_dir.join("lshpack.c").display());
    println!("cargo:rerun-if-changed={}", crate_dir.join("src/lshpack_wrapper.c").display());
    println!("cargo:rerun-if-changed={}", lsquic_dir.join("wincompat").display());
    println!("cargo:rerun-if-changed={}", lsqpack_dir.join("wincompat").display());
    println!("cargo:rerun-if-changed={}", lshpack_dir.join("compat/windows").display());
    println!("cargo:rerun-if-changed=build.rs");
}

// No-cross-env compiler fallback for the build faces: clang-cl is the shape
// upstream exercises on windows (bun builds its windows C with clang-cl; the
// libuv probe produced a real COFF uv.lib through exactly this default), and
// clang stays the historical posix fallback.
fn fallback_cc(is_windows: bool) -> std::ffi::OsString {
    if is_windows {
        "clang-cl".into()
    } else {
        "clang".into()
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
