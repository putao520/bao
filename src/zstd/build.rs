use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = crate_dir.parent().unwrap().parent().unwrap();
    let zstd_dir = workspace_dir.join("vendor/zstd");

    if !zstd_dir.join("lib/common/zstd_common.c").exists() {
        panic!(
            "zstd source not found at {:?}. \
             Run: git submodule update --init or download manually.",
            zstd_dir
        );
    }

    let mut build = cc::Build::new();
    build.compiler("clang");
    build.opt_level(2);
    build
        .flag("-fvisibility=hidden")
        .flag("-fPIC");

    build.define("ZSTD_MULTITHREAD", Some("1"));
    build.define("ZSTD_LEGACY_SUPPORT", Some("0"));
    // Namespace xxhash to avoid clashes with lshpack/libarchive
    build.flag("-DXXH_NAMESPACE=ZSTD_");

    build.include(zstd_dir.join("lib"));
    build.include(zstd_dir.join("lib/common"));

    let sources = [
        "common/debug", "common/entropy_common", "common/error_private",
        "common/fse_decompress", "common/pool", "common/threading", "common/xxhash",
        "common/zstd_common",
        "compress/fse_compress", "compress/hist", "compress/huf_compress",
        "compress/zstd_compress", "compress/zstd_compress_literals",
        "compress/zstd_compress_sequences", "compress/zstd_compress_superblock",
        "compress/zstd_double_fast", "compress/zstd_fast", "compress/zstd_lazy",
        "compress/zstd_ldm", "compress/zstd_opt", "compress/zstd_preSplit",
        "compress/zstdmt_compress",
        "decompress/huf_decompress", "decompress/zstd_ddict",
        "decompress/zstd_decompress", "decompress/zstd_decompress_block",
        "dictBuilder/cover", "dictBuilder/divsufsort", "dictBuilder/fastcover",
        "dictBuilder/zdict",
    ];

    for src in &sources {
        build.file(zstd_dir.join(format!("lib/{}.c", src)));
    }

    // x86_64 ASM decompressor
    #[cfg(target_arch = "x86_64")]
    build.file(zstd_dir.join("lib/decompress/huf_decompress_amd64.S"));

    build.compile("zstd");

    println!("cargo:rerun-if-changed={}/", zstd_dir.display());
}
