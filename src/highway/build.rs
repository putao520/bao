use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = crate_dir.parent().unwrap().parent().unwrap();
    let hwy_dir = workspace_dir.join("vendor/highway");

    if !hwy_dir.join("hwy/highway.h").exists() {
        panic!(
            "Google Highway source not found at {:?}. \
             Run: git submodule update --init or download manually.",
            hwy_dir
        );
    }

    // ── Build 1: Google Highway library (9 core TUs) ──
    let mut hwy_build = cc::Build::new();
    hwy_build.compiler("clang++");
    hwy_build.cpp(true);
    hwy_build.opt_level(2);
    hwy_build
        .flag("-std=c++17")
        .flag("-fno-exceptions")
        .flag("-fmath-errno")
        .flag("-fvisibility=hidden")
        .flag("-fPIC");
    hwy_build.define("HWY_STATIC_DEFINE", Some("1"));

    hwy_build.include(&hwy_dir);

    const HWY_SRCS: &[&str] = &[
        "hwy/abort.cc",
        "hwy/aligned_allocator.cc",
        "hwy/nanobenchmark.cc",
        "hwy/per_target.cc",
        "hwy/perf_counters.cc",
        "hwy/print.cc",
        "hwy/profiler.cc",
        "hwy/targets.cc",
        "hwy/timer.cc",
    ];

    for src in HWY_SRCS {
        hwy_build.file(hwy_dir.join(src));
    }

    hwy_build.compile("highway");

    // ── Build 2: Bao's highway_strings.cpp (SIMD string ops) ──
    let mut strings_build = cc::Build::new();
    strings_build.compiler("clang++");
    strings_build.cpp(true);
    strings_build.opt_level(2);
    strings_build
        .flag("-std=c++17")
        .flag("-fno-exceptions")
        .flag("-fmath-errno")
        .flag("-fPIC");
    strings_build.define("HWY_STATIC_DEFINE", Some("1"));

    // highway_strings.cpp #includes "root.h" and "highway_strings.cpp" (self-include via HWY_TARGET_INCLUDE)
    strings_build.include(crate_dir.join("src"));
    strings_build.include(&hwy_dir);

    strings_build.file(crate_dir.join("src/highway_strings.cpp"));

    strings_build.compile("highway_strings");

    println!("cargo:rerun-if-changed={}/", hwy_dir.display());
    println!("cargo:rerun-if-changed={}/", crate_dir.join("src").display());
}
