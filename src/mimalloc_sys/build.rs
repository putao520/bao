use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = crate_dir.parent().unwrap().parent().unwrap();
    let mi_dir = workspace_dir.join("vendor/mimalloc");

    if !mi_dir.join("src/static.c").exists() {
        panic!(
            "mimalloc source not found at {:?}. \
             Run: git submodule update --init or download manually.",
            mi_dir
        );
    }

    let mut build = cc::Build::new();
    build.compiler("clang++");
    build.opt_level(2);

    // Compile as C++. Required because we link against C++ code that uses
    // mimalloc types, and C/C++ ABI can differ (notably around structs
    // with trailing flexible arrays).
    build.cpp(true);

    build
        .flag("-std=c++17")
        .flag("-fvisibility=hidden")
        .flag("-fno-exceptions")
        .flag("-fno-rtti")
        .flag("-Wno-deprecated")
        .flag("-Wno-static-in-inline")
        .flag("-ftls-model=initial-exec");

    // Defines
    build.define("MI_STATIC_LIB", Some("1"));
    build.define("MI_SKIP_COLLECT_ON_EXIT", Some("1"));
    build.define("MI_NO_PROCESS_DETACH", Some("1"));
    build.define("MI_NO_SET_VMA_NAME", Some("1"));
    build.define("MI_DEFAULT_ALLOW_THP", Some("0"));

    // Only override malloc on Linux
    #[cfg(target_os = "linux")]
    {
        build.define("MI_MALLOC_OVERRIDE", Some("1"));
        build.flag("-fno-builtin-malloc");
    }

    // Include dirs
    build.include(mi_dir.join("include"));

    // Unity build — single TU that #includes everything
    build.file(mi_dir.join("src/static.c"));

    build.compile("mimalloc");

    println!("cargo:rerun-if-changed={}/", mi_dir.display());
}
