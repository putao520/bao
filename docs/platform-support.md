# Bao platform support matrix

> Closes the documentation criterion of
> [REQ-DEPLOY-1](../.spec/10-REQUIREMENTS.html#req-deploy-1):
> “支持目标矩阵文档化:x86_64-unknown-linux-gnu=Supported、
> x86_64-unknown-linux-musl=Supported(用户裁决 2026-09-01),
> 其余平台显式列出状态禁灰区”.
> Every non-listed platform state below is explicit; there are no gray zones.
>
> Companion documents: [`build-macos.md`](build-macos.md) (macOS layer-by-layer
> map, real-machine checklist) and the issues linked throughout.
>
> Last updated: 2026-09-08.

## 1. Target matrix

| Target | Status | Notes |
|---|---|---|
| `x86_64-unknown-linux-gnu` | **Supported** | The daily-driver platform; full stack (SpiderMonkey, servo, boringssl, uWS) built and tested in-repo every wave. |
| `x86_64-unknown-linux-musl` | **Supported** | Decided 2026-09-01 under REQ-DEPLOY-1; the native-closure cross-build work runs under the daily-ops long-task protocol — surface blockers tracked in [#10](https://github.com/putao520/bao/issues/10) (e.g. freetype-sys cross pkg-config). |
| macOS (`x86_64`/`aarch64-apple-darwin`) | **Experimental** | Compile surfaces are landed (`bun_uws_sys` kqueue arm, `bao_uloop` kqueue backend, darwin root certs — see [`build-macos.md`](build-macos.md) §1/§2). No real-machine build/link/test pass has ever run: [#36](https://github.com/putao520/bao/issues/36) (test surface), [#37](https://github.com/putao520/bao/issues/37) (mozjs source build). |
| Windows (`x86_64-pc-windows-msvc` / `-gnu`) | **Unsupported** | Fail-closed by construction at three native points (§3): [#33](https://github.com/putao520/bao/issues/33) mimalloc MSVC arm, [#34](https://github.com/putao520/bao/issues/34) `uv_*` symbol closure, [#35](https://github.com/putao520/bao/issues/35) `bao_uloop` IOCP arm. See §4 for the open-item list. |

“Supported” means the repository treats the target as part of the delivery
contract (waves must not regress it). “Experimental” means code surfaces exist
and are expected to work but nothing has been machine-verified. “Unsupported”
means a build targeting it fails closed — loudly, at build time — rather than
producing a broken artifact.

## 2. Native-crate platform state

The crates below are the ones whose platform story is decided by native build
code, not by Rust `cfg` alone. Everything else in `src/` is platform-neutral
Rust or upstream Bun crates that already carry macOS arms.

| Crate | Linux | macOS | Windows | Fail-closed mechanism |
|---|---|---|---|---|
| `bun_mimalloc_sys` | **Supported** — `MI_MALLOC_OVERRIDE` arm active | Compile surface only: C++17 build of vendored mimalloc needs Apple SDK libc++ headers; blocked for *cross*-compile, untested native ([#37](https://github.com/putao520/bao/issues/37), [`build-macos.md`](build-macos.md) §3) | **Unsupported** — `build.rs` `panic!`s for `CARGO_CFG_TARGET_OS=windows` ([#33](https://github.com/putao520/bao/issues/33)) | Explicit panic in `src/mimalloc_sys/build.rs` |
| `bun_uws_sys` | **Supported** — `LIBUS_USE_EPOLL` | Compile surface ready — `LIBUS_USE_KQUEUE`, darwin root certs | **Unsupported** — the build script refuses before any C is compiled: `eprintln!` + `exit(1)` carrying the `uv_*` supply-path analysis (`src/uws_sys/build.rs:101-115`); the `LIBUS_USE_LIBUV` arm would otherwise end in undefined `uv_*` symbols, since no libuv object code is vendored ([#34](https://github.com/putao520/bao/issues/34)) | build-script `exit(1)` before any C compile; `build.rs:117` panics on unknown OS |
| `bun_libuv_sys` | n/a (crate root is `#[cfg(windows)]`) | n/a | `#[repr(C)]` FFI mirror of uv 1.51.0 headers — **declarations only, zero symbol definitions** ([#34](https://github.com/putao520/bao/issues/34)) | Missing symbols at link time |
| `bao_uloop` | **Supported** — epoll tick | Compile surface ready — kqueue backend (`src/bao_uloop/src/kqueue.rs`) | **Unsupported** — crate root carries an explicit `#[cfg(not(any(target_os = "linux", target_os = "macos")))] compile_error!` (`src/bao_uloop/src/lib.rs:44-49`): any windows build fails at compile time with an explicit Unsupported message. No IOCP arm exists ([#35](https://github.com/putao520/bao/issues/35)) | `compile_error!` gate at crate root |
| `bao-mozjs-sys` (SpiderMonkey) | **Supported** — the only target with real build/link/test history | Build paths present (apple `-lc++`, bindgen `c++` driver special case), **zero verification runs** ([#37](https://github.com/putao520/bao/issues/37)) | Build paths present (moztools/mozmake/MSVC link libs, §3), **zero verification runs** ([#37](https://github.com/putao520/bao/issues/37)) | Toolchain probing panics (`find_moztools`) |

The five BAO mozjs patches and the eleven servo customization files listed in
the repository `CLAUDE.md` are platform-neutral (C++/Rust logic, boringssl TLS
at the connector layer) — they are not additional per-platform risk.

## 3. `bao-mozjs-sys` external toolchain requirements

Taken from the actual probing in `vendor/mozjs/mozjs-sys/build.rs` (line
references are to that file), not from memory.

### Linux (verified — this is how every in-repo build works)

| Tool | How it is selected / probed |
|---|---|
| `make` (or `gmake`) | `MAKE` env override → probe `gmake --version` → fall back to `make` (`find_make`, `build.rs:937-948`). Make ≥ 4.4 is rejected: `--jobserver-style` probe detects the jobserver style cargo's pipe jobserver cannot serve (`is_buggy_make_version`, `build.rs:525-545`). |
| `python3` | Drives mozjs `configure` / `moz.build` (the make driver sets `PYTHONDONTWRITEBYTECODE=1`, `build.rs:383`). The python payload is vendored in-package via the `bao-mozjs-src-python` dependency (`DEP_BAO_MOZJS_SRC_PYTHON_ROOT`, `build.rs:179-203`). |
| C/C++ compiler + `ar` | Chosen by cc-rs per target; `AR`/`CC`/`CXX` are forwarded to the mozjs make run (`SM_TARGET_ENV_VARS`, `build.rs:376-380`). `ar t/d/q` is used by the stale-archive fixup (`build.rs:311-330`). Bindgen additionally requires a working `libclang` (`build.rs:631`). |

Repository-level statement of the same list: README “Build prerequisites”
(clang, python3, make).

### macOS (expected toolchain, never exercised — [#37](https://github.com/putao520/bao/issues/37))

| Tool | Evidence in `build.rs` |
|---|---|
| Xcode Command Line Tools (`clang`/`clang++`, SDK) | apple targets link `c++` (`build.rs:697`); bindgen special-cases the `c++` driver path on macos (`build.rs:600-603`); `bun_mimalloc_sys` additionally needs the SDK's libc++ headers ([`build-macos.md`](build-macos.md) §3). |

### Windows (expected toolchain, zero verification runs — [#37](https://github.com/putao520/bao/issues/37))

| Tool | Evidence in `build.rs` |
|---|---|
| moztools 4.0 | `MOZTOOLS_VERSION = "4.0"` (`build.rs:52-53`); resolved by `find_moztools`: `MOZTOOLS_PATH` env → `<target-root>/dependencies/moztools/4.0` → `MOZILLABUILD`/`MOZILLA_BUILD` env → `panic!` (`build.rs:911-934`). `MOZILLABUILD` and an msys2-augmented `PATH` are set for the make run (`build.rs:342-352`). |
| `mozmake` | The make driver on windows is hard-coded to `mozmake` (`build.rs:354`). |
| MSVC toolchain | cc-rs `is_like_msvc` branches for the cc build and bindgen clang-args (`build.rs:583-595`); windows links `winmm`/`psapi`/`user32`/`Dbghelp`/`advapi32` (`build.rs:682-688`). |

## 4. Open items (explicit, no gray zones)

| Item | Scope | Status |
|---|---|---|
| [#10](https://github.com/putao520/bao/issues/10) | musl cross surface: freetype-sys pkg-config and the rest of the native closure | REQ-DEPLOY-1 long-task protocol, in progress |
| [#33](https://github.com/putao520/bao/issues/33) | `bun_mimalloc_sys` MSVC arm (proper clang-cl/MSVC flags) | Open — registered as a follow-up wave once a Windows CI baseline exists; today the build fails closed |
| [#34](https://github.com/putao520/bao/issues/34) | `uv_*` symbol closure for the uWS windows arm (real libuv source/static-lib decision) | Open |
| [#35](https://github.com/putao520/bao/issues/35) | `bao_uloop` IOCP arm | Open |
| [#36](https://github.com/putao520/bao/issues/36) | macOS test-surface verification on real hardware | Open |
| [#37](https://github.com/putao520/bao/issues/37) | `bao-mozjs-sys` source build verification on real macOS/Windows machines | Open |

CI coverage of these targets is governed by its own issue and is intentionally
not promised anywhere in this document.
