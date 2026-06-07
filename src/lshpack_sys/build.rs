use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = crate_dir.parent().unwrap().parent().unwrap();
    let lshpack_dir = workspace_dir.join("vendor/lshpack");

    if !lshpack_dir.join("lshpack.c").exists() {
        panic!(
            "lshpack source not found at {:?}. \
             Run: git submodule update --init or download manually.",
            lshpack_dir
        );
    }

    let mut build = cc::Build::new();
    build.compiler("clang");
    build.opt_level(2);
    build
        .flag("-DLS_HPACK_USE_LARGE_TABLES=1")
        .flag("-DLS_HPACK_BSS_LARGE_TABLES=1")
        .flag("-DXXH_HEADER_NAME=\"xxhash.h\"");
    build.include(&lshpack_dir);
    build.include(lshpack_dir.join("deps/xxhash"));

    build.file(lshpack_dir.join("lshpack.c"));
    build.file(lshpack_dir.join("deps/xxhash/xxhash.c"));
    build.file(crate_dir.join("src/lshpack_wrapper.c"));

    build.compile("lshpack");

    println!("cargo:rerun-if-changed={}/lshpack.c", lshpack_dir.display());
    println!("cargo:rerun-if-changed={}/", crate_dir.join("src").display());
}
