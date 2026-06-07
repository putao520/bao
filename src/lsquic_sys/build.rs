// Build script for bun_lsquic_sys: compiles lsquic (LiteSpeed QUIC/HTTP3)
// + lsqpack (QPACK header compression) using the `cc` crate.
//
// Mirrors Bun's scripts/build/deps/lsquic.ts exactly:
//   - ~70 C files from src/liblsquic/
//   - lsqpack.c compiled inline (lsquic feeds it a non-FILE* logger context)
//   - Links against BoringSSL + lshpack + zlib

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = crate_dir.parent().unwrap().parent().unwrap().to_path_buf();
    let vendor_dir = workspace_dir.join("vendor");

    let lsquic_dir = vendor_dir.join("lsquic");
    let lsquic_src = lsquic_dir.join("src").join("liblsquic");
    let lsqpack_dir = vendor_dir.join("lsqpack");
    let lshpack_dir = vendor_dir.join("lshpack");
    let boringssl_dir = vendor_dir.join("boringssl");

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
    build.compiler("clang");
    build.opt_level(1);
    // lsquic emits many -Wsign-compare and -Wunused; upstream builds with -Werror
    // disabled. Suppress all warnings (treat as third-party lib).
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

    // Include paths
    build
        .include(lsquic_dir.join("include"))
        .include(&lsquic_src)
        .include(boringssl_dir.join("include"))
        .include(&lshpack_dir)
        .include(lshpack_dir.join("deps").join("xxhash"))
        .include(&lsqpack_dir)
        .include(lsqpack_dir.join("deps").join("xxhash"));

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
    // lsquic depends on zlib (system lib). BoringSSL and lshpack are
    // propagated via Cargo dependencies (bun_boringssl_sys, bun_lshpack_sys).
    println!("cargo:rustc-link-lib=z");

    // ── Rebuild hints ─────────────────────────────────────────────────────
    println!("cargo:rerun-if-changed={}", lsquic_src.join("lsquic_engine.c").display());
    println!("cargo:rerun-if-changed={}", lsqpack_dir.join("lsqpack.c").display());
    println!("cargo:rerun-if-changed=build.rs");
}
