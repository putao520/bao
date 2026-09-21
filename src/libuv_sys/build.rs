// Build script for bun_libuv_sys: compiles the vendored oven-sh/libuv fork
// (`bun` branch @ 8023581113b276e7c1aee3f82da57ca0893faab1, uv 1.51.1-dev,
// with the two upstream win-poll patches already applied) into a static
// `uv` library for windows targets — the uv_* symbol supply that closes
// issue #34. Every other target: no-op (the crate stays declaration-only
// there and no link flags are emitted).
//
// Recipe provenance: a verbatim translation of upstream Bun's libuv build
// description (oven-sh/bun `scripts/build/deps/libuv.ts` @ 4af1842c8c,
// `kind: "direct"`): 12 shared sources + 25 windows sources, includes
// ["include", "src"], and the clang-cl flag set. Translation map:
//
//   deps/libuv.ts                                        → here
//   includes: ["include", "src"]                         → .include() × 2
//   defines: WIN32_LEAN_AND_MEAN                         → .define(None)
//            _CRT_DECLARE_NONSTDC_NAMES=0                → .define(Some("0"))
//            WIN32, _WINDOWS                             → .define(None)
//   cflags:  -D_WIN32_WINNT=0x0A00                       → .define(Some("0x0A00"))
//            /clang:-fno-strict-aliasing                 → .flag()
//            -Wno-int-conversion                         → .flag()
//            /wd4996                                     → .flag()
//
// Compiler: clang-cl is the shape upstream exercises (bun builds its
// windows C with clang-cl; the /clang:- passthrough and /wd4996 literals
// are clang-cl flags) and the default here. CC / CC_<triple> / TARGET_CC
// env overrides win over that default (same probe discipline as
// bun_uws_sys/build.rs — cc-rs honors `.compiler()` before its own env
// chain, so the env is probed explicitly). A plain cl.exe is honored with
// the MSVC subset of the flags. Cross builds from a non-windows host
// additionally need the MSVC/WinSDK surface (xwin-style sysroot: INCLUDE
// env or CFLAGS_<triple> with -imsvc/--sysroot, AR_<triple> for the
// archiver).
//
// Allocation invariant (upstream `forbidUndefined`): bun swaps mimalloc in
// via uv_replace_allocator, so every libuv allocation must flow through
// uv__malloc/uv__free — src/uv-common.c owns the default allocator table
// and is upstream's single exemption (a stray CRT call there gets uv__free'd
// and crashes; oven-sh/libuv#14). Upstream enforces this by scanning the
// compiled objects' undefined-symbol tables for libc allocation symbols;
// the audit belongs with the first real windows link face (the uWS
// LIBUS_USE_LIBUV arm, issue #34 follow-up), which is the point where a
// violation becomes a live crash.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

// Platform-neutral sources — deps/libuv.ts `SHARED`, compiled on every
// target upstream builds libuv for.
const SHARED: &[&str] = &[
    "fs-poll",
    "idna",
    "inet",
    "random",
    "strscpy",
    "strtok",
    "thread-common",
    "threadpool",
    "timer",
    "uv-common",
    "uv-data-getter-setters",
    "version",
];

// Windows sources — deps/libuv.ts `WIN`, the IOCP/windows backends.
const WIN: &[&str] = &[
    "async",
    "core",
    "detect-wakeup",
    "dl",
    "error",
    "fs",
    "fs-event",
    "getaddrinfo",
    "getnameinfo",
    "handle",
    "loop-watcher",
    "pipe",
    "thread",
    "poll",
    "process",
    "process-stdio",
    "signal",
    "snprintf",
    "stream",
    "tcp",
    "tty",
    "udp",
    "util",
    "winapi",
    "winsock",
];

fn main() {
    // The platform matrix keys on TARGET (CARGO_CFG_*), never the host:
    // a linux host cross-building x86_64-pc-windows-msvc must still compile
    // the C. Same discipline as bun_uws_sys/build.rs.
    let target_os = env::var("CARGO_CFG_TARGET_OS")
        .expect("CARGO_CFG_TARGET_OS must be set by cargo for build scripts");
    if target_os != "windows" {
        // libuv is a windows-only supply in this workspace: bun's posix
        // event loop is epoll/kqueue direct, and node-api addons that
        // reference libuv symbols on posix are served by the uv-posix-stubs
        // translation units upstream (deps/libuv.ts header comment). Compile
        // nothing, emit nothing.
        return;
    }

    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let libuv_dir = crate_dir.join("vendor").join("libuv");

    let compiler = env_cc("CC").unwrap_or_else(|| OsString::from("clang-cl"));
    let mut build = cc::Build::new();
    build.compiler(&compiler);
    shape_flags(&mut build, &compiler);

    // Order mirrors deps/libuv.ts `includes`.
    for dir in ["include", "src"] {
        build.include(libuv_dir.join(dir));
    }

    build
        .define("WIN32_LEAN_AND_MEAN", None)
        .define("_CRT_DECLARE_NONSTDC_NAMES", Some("0"))
        .define("WIN32", None)
        .define("_WINDOWS", None)
        // Hex literal required — sdkddkver.h token-pastes `ver##0000`.
        .define("_WIN32_WINNT", Some("0x0A00"));

    for name in SHARED {
        build.file(libuv_dir.join("src").join(format!("{name}.c")));
    }
    for name in WIN {
        build
            .file(libuv_dir.join("src").join("win").join(format!("{name}.c")));
    }

    // Emits cargo:rustc-link-lib=static=uv + the search path — this is the
    // link-time uv_* supply bun_uws_sys's windows arm consumes.
    build.compile("uv");
}

// Upstream's cflags are clang-cl literals ("/clang:-fno-strict-aliasing"
// passes the GNU option through clang-cl's MSVC-compatible driver; "/wd4996"
// silences the MSVC deprecated-CRT warning C4996). Shape them per compiler
// flavor:
//   clang-cl → verbatim upstream flags
//   cl.exe   → the MSVC subset: /wd4996 only. The two clang options have no
//              MSVC counterpart (MSVC has no strict-aliasing optimization to
//              disable, and there is no int-conversion diagnostic to quiet).
//   other    → refuse. An MSVC-target C build driven by a GNU-style driver
//              needs a different flag spelling; fail loudly here instead of
//              emitting flags the compiler misreads.
fn shape_flags(build: &mut cc::Build, compiler: &std::ffi::OsStr) {
    let stem = Path::new(compiler)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if stem.contains("clang-cl") {
        build
            .flag("/clang:-fno-strict-aliasing")
            .flag("-Wno-int-conversion")
            .flag("/wd4996");
    } else if stem == "cl" {
        build.flag("/wd4996");
    } else {
        panic!(
            "bun_libuv_sys windows supply requires clang-cl (upstream Bun's windows C \
             compiler); set CC_<target>=clang-cl — plus CFLAGS_<target>/AR_<target> for \
             the sysroot and archiver on cross hosts — found compiler {compiler:?}"
        );
    }
}

fn env_cc(key: &str) -> Option<OsString> {
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
