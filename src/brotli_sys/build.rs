use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = crate_dir.parent().unwrap().parent().unwrap();
    let brotli_dir = workspace_dir.join("vendor/brotli");

    if !brotli_dir.join("c/common/constants.c").exists() {
        panic!(
            "brotli source not found at {:?}. \
             Run: git submodule update --init or download manually.",
            brotli_dir
        );
    }

    let mut build = cc::Build::new();
    build.compiler("clang");
    build.opt_level(2);
    build
        .flag("-fvisibility=hidden")
        .flag("-fPIC");

    build.define("BROTLI_HAVE_LOG2", Some("1"));
    #[cfg(target_os = "linux")]
    build.define("OS_LINUX", Some("1"));
    #[cfg(target_os = "macos")]
    build.define("OS_MACOSX", Some("1"));

    build.include(brotli_dir.join("c/include"));

    let sources = [
        // common
        "common/constants", "common/context", "common/dictionary", "common/platform",
        "common/shared_dictionary", "common/transform",
        // dec
        "dec/bit_reader", "dec/decode", "dec/huffman", "dec/state",
        // enc
        "enc/backward_references", "enc/backward_references_hq", "enc/bit_cost",
        "enc/block_splitter", "enc/brotli_bit_stream", "enc/cluster", "enc/command",
        "enc/compound_dictionary", "enc/compress_fragment", "enc/compress_fragment_two_pass",
        "enc/dictionary_hash", "enc/encode", "enc/encoder_dict", "enc/entropy_encode",
        "enc/fast_log", "enc/histogram", "enc/literal_cost", "enc/memory",
        "enc/metablock", "enc/static_dict", "enc/utf8_util",
    ];

    for src in &sources {
        build.file(brotli_dir.join(format!("c/{}.c", src)));
    }

    build.compile("brotli");

    println!("cargo:rerun-if-changed={}/", brotli_dir.display());
}
