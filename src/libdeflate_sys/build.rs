use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = crate_dir.parent().unwrap().parent().unwrap();
    let libdeflate_dir = workspace_dir.join("vendor/libdeflate");

    if !libdeflate_dir.join("lib/deflate_compress.c").exists() {
        panic!(
            "libdeflate source not found at {:?}. \
             Run: git submodule update --init or download manually.",
            libdeflate_dir
        );
    }

    let mut build = cc::Build::new();
    build.compiler("clang");
    build.opt_level(2);
    build
        .flag("-fvisibility=hidden")
        .flag("-fPIC");

    build.include(&libdeflate_dir);

    let sources = [
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
    ];

    for src in &sources {
        build.file(libdeflate_dir.join(src));
    }

    build.compile("libdeflate");

    println!("cargo:rerun-if-changed={}/", libdeflate_dir.display());
}
