# Cross-building Bao for `x86_64-unknown-linux-musl` — toolchain, sysroot, env

> Root-fix companion of [#10](https://github.com/putao520/bao/issues/10) under
> [REQ-DEPLOY-1](../.spec/10-REQUIREMENTS.html#req-deploy-1) criterion
> “freetype-sys 等 pkg-config 类交叉阻塞根治(交叉环境变量/sysroot 方案,
> 禁 `PKG_CONFIG_ALLOW_CROSS` 裸开关文档化收尾)”.
> Everything below is evidence-backed: the freetype-sys 0.23.0 PoC of 2026-09-09
> produced a **pure musl static-pie binary** (0 GLIBC strings, FT_Init →
> FT_New_Memory_Face → cmap → FT_LOAD_RENDER real 9x12 GRAY bitmap → `POC_OK`),
> end to end without a single `PKG_CONFIG_ALLOW_CROSS`.
>
> Scope: this document closes the *pkg-config-class* cross blocker
> (freetype-sys as the proven representative). The remaining musl bring-up
> (full `bao` binary link, downstream servo-stack crates, runtime smoke) runs
> under the daily-ops long-task protocol — see the platform matrix in
> [`platform-support.md`](platform-support.md).
>
> Last updated: 2026-09-09.

## 1. Verdict

Route A — **system-pkg-config with targeted env + a self-produced sysroot** — is
the supported path. Compile freetype statically with a musl cross-gcc, install
`libfreetype.a` + headers + a hand-written `freetype2.pc` into a sysroot
directory, then point the targeted pkg-config variables at it. `cargo build
--target x86_64-unknown-linux-musl` then passes freetype-sys's build.rs probe
unchanged.

Route B (the `bundled` feature) is **structurally broken in the crates.io
package** and is excluded — see §7.

## 2. Toolchain inventory

| Component | Path / command | Notes |
|---|---|---|
| musl.cc cross toolchain | `~/musl-cross/x86_64-linux-musl-cross/bin/x86_64-linux-musl-{gcc,g++,ar}` | GCC 11.2.1. Wired into `.cargo/config.toml` as per-target `CC_/CXX_/AR_x86_64_unknown_linux_musl` + `[target.x86_64-unknown-linux-musl] linker` (p1, commit `0a4a11f7`). Absolute paths are machine-local by design — the per-target key shape is structurally unread by gnu/host builds. |
| Ubuntu `musl-tools` wrapper | `/usr/bin/musl-gcc` | GCC 13.3.0 glibc wrapper driving musl libc. This is what the PoC used to compile `libfreetype.a`; both compilers work for the sysroot step (`MUSL_CC` overrides the script default). |
| rustup target | `x86_64-unknown-linux-musl` | `rustup target add x86_64-unknown-linux-musl` |
| `pkg-config` crate behavior | 0.3.34 | The cross gate/forwarding semantics in §3 are read from its source, not guessed. |

## 3. The env combination (complete)

```bash
env \
  PKG_CONFIG_SYSROOT_DIR_x86_64-unknown-linux-musl="$SYSROOT" \
  PKG_CONFIG_LIBDIR_x86_64-unknown-linux-musl="$SYSROOT/usr/lib/pkgconfig" \
  CC_x86_64_unknown_linux_musl=musl-gcc \
  AR_x86_64_unknown_linux_musl=ar \
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  RUSTFLAGS="-C link-arg=-lc" \
  cargo build --target x86_64-unknown-linux-musl --release
```

(For the bao repository, the `CC_/AR_` and linker parts already live in
`.cargo/config.toml` from p1 — do not duplicate them on the command line there.)

| Variable | Value | Purpose |
|---|---|---|
| `PKG_CONFIG_SYSROOT_DIR_<target>` | sysroot dir | **Opens the cross gate** (this is the sanctioned alternative to the bare `PKG_CONFIG_ALLOW_CROSS` switch) and makes pkg-config rewrite output paths with the sysroot prefix. |
| `PKG_CONFIG_LIBDIR_<target>` | `$SYSROOT/usr/lib/pkgconfig` | Isolates from host `.pc` files — only the sysroot is probed. SYSROOT_DIR alone opens the gate but without LIBDIR the host `.pc` set leaks in; both together are the working pair. |
| `CC_<target>` / `AR_<target>` | musl gcc / ar | cc-rs compilation of vendored C (e.g. libz-sys's zlib). |
| `CARGO_TARGET_<T>_LINKER` | musl gcc | Final link. |
| `RUSTFLAGS="-C link-arg=-lc"` | — | **Required** — see §4. |

### How the pkg-config crate makes this work (0.3.34, source-read)

- **Gate** (`target_supported`): when TARGET ≠ HOST and
  `PKG_CONFIG_ALLOW_CROSS` is unset, a targeted `PKG_CONFIG_SYSROOT_DIR` (or a
  `PKG_CONFIG` wrapper) is what lets the probe proceed.
- **Forwarding** (`command()`): the targeted variables are forwarded to the
  pkg-config subprocess, which rewrites its output with the sysroot prefix
  (observed: `-I$SYSROOT/usr/include -L$SYSROOT/usr/lib -lfreetype`).
- **The naming pitfall**: targeted variable names contain hyphens
  (`..._x86_64-unknown-linux-musl`), which bash `export` / in-script assignment
  reject (`not a valid identifier`). Two fixes: run through the `/usr/bin/env
  VAR=... cmd` prefix (or CI yaml `env:`), or use the underscore form
  `..._x86_64_unknown_linux_musl` — the crate resolves
  hyphen-form → underscore-form → `TARGET_`-prefixed → plain, in that order.
- Setting only the **plain** `PKG_CONFIG_LIBDIR` does **not** open the gate
  (0.3.34 only honors ALLOW_CROSS / `PKG_CONFIG` / SYSROOT_DIR for cross).

## 4. The hidden second layer: musl crt-static archive link order

Passing the build script is not enough. rustc's musl target defaults to
`crt-static`, and the link line places `-lc` inside a `-Wl,-Bstatic` group
(rust's own `rustlib/.../self-contained/libc.a`), while native static archives
(`-lfreetype`, injected from pkg-config metadata) land in a `-Wl,-Bdynamic`
section **after** `-lc`. A static archive is scanned once — libc symbols that
freetype pulls in (`strrchr` / `fseek` / `setjmp` / `longjmp` /
`__stack_chk_fail`) are left dangling.

Fix (consumer-side, zero upstream changes): `RUSTFLAGS="-C link-arg=-lc"`
appends one trailing `-lc` that lands after `-lfreetype`, so the archive is
rescanned and the missing symbols resolve (symbols are only *filled in* — no
duplicate definitions). Note that a build script's
`println!("cargo:rustc-link-lib=dylib=c")` does **not** work here: cargo orders
it *before* `-lfreetype` (observed).

## 5. Sysroot production: `scripts/build-musl-sysroot.sh`

```bash
scripts/build-musl-sysroot.sh <sysroot-dir>
# MUSL_CC=...  MUSL_AR=...  FT_SYS_SRC=...  # optional overrides
```

- Source tree: the vendored `freetype2/` of the installed
  `freetype-sys-0.23.0` registry package (zero downloads; read-only borrow,
  copied + `chmod u+w`).
- 42-module list = freetype-sys 0.23.0 `build.rs` bundled list verbatim, minus
  the PNG define (default `ftoption.h` uses the built-in zlib — no external
  zlib dependency).
- Products: `<sysroot>/usr/lib/libfreetype.a` (1,051,346 B, 267 `FT_` exports),
  `<sysroot>/usr/include/freetype/`,
  `<sysroot>/usr/lib/pkgconfig/freetype2.pc` (`Version: 26.1.20` = libtool
  version of freetype 2.13.2, comfortably above the build.rs gate `24.3.18`).
- Intermediate objects go to `<sysroot>.build/` (separate from the sysroot;
  delete either and re-run — the script is idempotent and byte-reproducible).

Replay evidence (2026-09-09, fresh sysroot `/tmp/bao-musl-sysroot-replay`):
script exit 0 twice (second run byte-identical .a/.pc), and the route-A
consumer cross-build against that sysroot produced a static-pie binary with
0 GLIBC strings and `POC_OK` at run time.

## 6. Detection commands

```bash
# 1. Toolchain existence
test -x ~/musl-cross/x86_64-linux-musl-cross/bin/x86_64-linux-musl-g++ \
  && echo "musl.cc toolchain OK"
command -v musl-gcc                       # Ubuntu musl-tools wrapper (PoC compiler)
rustup target list --installed | grep -x x86_64-unknown-linux-musl

# 2. Sysroot existence
SYSROOT=/tmp/bao-musl-sysroot             # wherever it was produced
test -f "$SYSROOT/usr/lib/libfreetype.a"  # 1,051,346 B expected
test -f "$SYSROOT/usr/lib/pkgconfig/freetype2.pc"
nm "$SYSROOT/usr/lib/libfreetype.a" | grep -c " T FT_"   # 267 expected

# 3. freetype2.pc version gate (mirrors freetype-sys build.rs atleast_version)
env PKG_CONFIG_SYSROOT_DIR="$SYSROOT" \
    PKG_CONFIG_LIBDIR="$SYSROOT/usr/lib/pkgconfig" \
    pkg-config --atleast-version=24.3.18 freetype2 && echo "version gate OK"
env PKG_CONFIG_SYSROOT_DIR="$SYSROOT" \
    PKG_CONFIG_LIBDIR="$SYSROOT/usr/lib/pkgconfig" \
    pkg-config --modversion freetype2     # 26.1.20
```

When upgrading `freetype-sys`, re-check its `build.rs` `atleast_version`
against the `.pc` `Version`; the vendored freetype version moves with the
registry package.

## 7. Route B (bundled): structurally broken in the crates.io package — excluded

- Observed (`features=["bundled"]`, cross): cc invokes
  `musl-gcc -I libpng -I libz-sys/src/zlib -c libpng/png.c` →
  `fatal error: zlib.h: No such file or directory` — verbatim the symptom
  reported in [#10](https://github.com/putao520/bao/issues/10).
- Root cause: the crates.io `freetype-sys-0.23.0` package contents are
  `build.rs / Cargo.toml / freetype2/ / libpng/ / src/` — **the `libz-sys/`
  directory is absent** (a git submodule that does not ship in the registry
  tarball). `build.rs` hardcodes the relative include path
  `build.include("libz-sys/src/zlib")`, which only exists in a git checkout.
  Pure-env workarounds cannot fix a missing file tree.
- If bundled is ever needed: `[patch.crates-io]` to a git source or fork, change
  `build.rs` to consume `DEP_Z_INCLUDE` (libz-sys metadata) instead of the
  hardcoded path, and enable libz-sys's `static` feature. Route A needs none of
  this.

## 8. Adoption guidance for the bao stack

1. Other pkg-config-based `-sys` crates on the servo font/graphics chain
   (`servo-fontconfig-sys`, `expat-sys`, …) follow the same shape: produce the
   static lib into the same sysroot, add one `.pc` each, reuse the identical
   env combination + `-C link-arg=-lc`.
2. For CI, bake sysroot production into the musl image / prebuild step
   (`scripts/build-musl-sysroot.sh` is the reproducible entry point) and set
   the env via an `env` prefix or yaml `env:` (avoiding the bash hyphen
   pitfall of §3).
