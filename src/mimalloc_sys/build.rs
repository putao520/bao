use std::env;
use std::path::PathBuf;

fn main() {
    // ── Platform gate (TARGET, never host) ───────────────────────────────
    // A build script is compiled for the HOST, so `#[cfg(target_os = ...)]`
    // here describes the machine running cargo, not the artifact being
    // produced (same trap src/uws_sys/build.rs documents). Every
    // target-dependent decision below reads CARGO_CFG_TARGET_OS instead.
    // windows is unsupported today: the clang++/GNU-flag build below has no
    // MSVC arm and the malloc override was never verified there — fail
    // closed instead of emitting a broken archive. Track issue #33 and
    // docs/platform-support.md.
    let target_os = env::var("CARGO_CFG_TARGET_OS")
        .expect("CARGO_CFG_TARGET_OS must be set by cargo for build scripts");
    if target_os == "windows" {
        panic!(
            "bun_mimalloc_sys does not support target OS 'windows' yet (no MSVC arm; \
             explicit Unsupported per issue #33) — see docs/platform-support.md for the \
             platform support matrix and the open work items."
        );
    }

    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // W0b publish incorporation: mimalloc source vendored in-package under
    // csrc/mimalloc (src/ unity-build set + include/ tree; byte-identical
    // copy of vendor/mimalloc subsets). A crates.io package can only ship
    // in-package files, so local builds and published builds compile the
    // exact same bytes. Keep csrc/ in sync when absorbing upstream mimalloc.
    let mi_dir = crate_dir.join("csrc").join("mimalloc");

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

    // Only override malloc on Linux (target OS — see the platform gate above;
    // musl shares CARGO_CFG_TARGET_OS=linux, which is the intended behavior)
    if target_os == "linux" {
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
