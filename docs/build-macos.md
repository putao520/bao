# Building Bao on macOS — what works, what doesn't

> Root-fix of [#5](https://github.com/putao520/bao/issues/5). Bao's primary
> platform is **Linux x86_64**; this document replaces the vague "macOS: not
> yet proven" note with a precise map of the macOS build path: which layers
> are mac-ready at the compile level, where a cross-build stops today, and
> the exact checklist for the first real-machine verification pass.
>
> Status labels: **ready** = code landed and probe-verified · **untested** =
> the upstream project supports it but this fork has never built it on mac
> hardware · **blocked** = known blocker with evidence · **open** = decision
> pending.
>
> Last updated: 2026-08-21, after the macOS enablement milestones M1+M2
> (commit `142e0097`) and the wave-4 platform-wiring closure (`4a956b27`).

## 1. Status matrix (compile layer)

| Layer | macOS state | Evidence |
|---|---|---|
| `bun_*` pure Rust base crates (~85) | **ready** — mac cfg branches are upstream-proven (Bun itself is macOS-first); vendored unmodified | 48 files under `src/` carry `target_os = "macos"` arms |
| `bao_uloop` (event-loop tick) | **ready** — kqueue backend landed (M2) | `src/bao_uloop/src/kqueue.rs` (kevent64 tick); crate gate `#![cfg(any(target_os = "linux", target_os = "macos"))]` at `src/bao_uloop/src/lib.rs:62`; Linux suite zero-regression at landing (45/45, commit `142e0097`) |
| `bun_uws_sys` (uSockets/uWS C build) | **ready** — build.rs platform matrix landed (M1) | `src/uws_sys/build.rs:64-90` selects eventing backend + root-cert TU + socket shape from `CARGO_CFG_TARGET_OS`; unknown OS panics (fail-closed) |
| System root certificates (TLS) | **ready** — no link-time framework dependency | `csrc/bun-usockets/src/crypto/root_certs_darwin.cpp` `dlopen`s the Security/CoreFoundation frameworks at runtime; no `-framework Security` at link |
| `bun_mimalloc_sys` (allocator) | **blocked for cross-compile**, expected fine natively | C++17 compile of vendored mimalloc needs Apple SDK libc++ headers — see §3 |
| `bao-mozjs` / SpiderMonkey fork | **untested** | `vendor/mozjs/mozjs-sys/build.rs` carries full apple target paths (e.g. lines 459, 600-603, 692, 798-801); the 5 BAO patches are platform-neutral C++/Rust |
| `bao-servo` fork (10 patched files) | **untested** | servo upstream supports macOS; the vendored snapshot includes the macOS font platform (`vendor/servo/components/fonts/platform/macos/`, Core Text); the Bao patches are platform-neutral (stealth TLS connector speaks boringssl, not OS APIs) |
| boringssl | **untested** | upstream build includes darwin sources (`crypto/cpu_aarch64_apple.cc` in `src/boringssl_sys/build.rs:114`); the stealth connector patch lives above the OS layer |
| Media stack (GStreamer) | **open** | `bao_browser` pins servo feature `media-gstreamer` unconditionally (`src/bao_browser/Cargo.toml:24`); the macOS runtime-dependency story is undecided — §5 gap 4 |
| Link level (full `bao` binary) | **unverified** | no milestone has reached the linker on/for macOS: cross-compile dies before link (§3), no mac hardware has run M4 |

The one-line summary, as landed in the wave-4 commit message: *"macOS M1+M2
+ windows W2 code surfaces now complete, machine-verification remains."*

## 2. What has landed for macOS (M1 + M2)

Both milestones shipped in commit `142e0097` (wave 3) and were verified by
probe-level evidence (per-target compiler flags inspected in real build
invocations) plus a Linux zero-regression run. Neither has run on Apple
hardware — that is what §4 is for.

### M1 — `bun_uws_sys` build.rs platform matrix

The uSockets/uWS C build used to pick its eventing backend with host
`#[cfg]`, which describes the machine running cargo, not the artifact being
produced. The matrix (`src/uws_sys/build.rs:30-90`) is now driven by
`CARGO_CFG_TARGET_OS` (the target triple) and mirrors the dispatch the
vendored C sources already apply to themselves, so there is exactly one
truth per platform:

| Target OS | Eventing macro | Eventing source | Root-cert TU |
|---|---|---|---|
| `linux` | `LIBUS_USE_EPOLL` | `eventing/epoll_kqueue.c` | `root_certs_linux.cpp` |
| `macos` / `ios` | `LIBUS_USE_KQUEUE` | `eventing/epoll_kqueue.c` | `root_certs_darwin.cpp` |
| `freebsd` | `LIBUS_USE_KQUEUE` | `eventing/epoll_kqueue.c` | `root_certs_linux.cpp` |
| `windows` | `LIBUS_USE_LIBUV` | `eventing/libuv.c` | `root_certs_windows.cpp` |
| anything else | build fails (`panic!`) | — | — |

Details that matter on macOS:

- Probe-verified: the `-DLIBUS_USE_KQUEUE=1` flag set (and the rest of the
  per-target defines) is emitted correctly for an `aarch64-apple-darwin`
  target during the M1 cross-probe.
- `root_certs_darwin.cpp` declares the Security framework types itself and
  `dlopen`s the frameworks at runtime — no link-time framework dependency,
  which keeps the crate's link line OS-agnostic.
- The Linux-only `Bun__isEpollPwait2SupportedOnLinuxKernel` extern is
  inside `#if defined(LIBUS_USE_EPOLL)` in the vendored C, so the kqueue
  half never references it — no dangling symbol on mac.

### M2 — `bao_uloop` kqueue backend

`bao_uloop` owns the Rust tick that replaces the C `epoll_wait` harvest on
JS threads (threads without a `BaoLoopState` still delegate to the C
`us_loop_run_bun_tick` — unchanged on mac). The macOS arm:

- `src/bao_uloop/src/kqueue.rs` (186 lines) — a single `kevent64` +
  dispatch, the mirror of the Linux `run_epoll`:
  - **BCE-007 controlled timeout**: pending work, NULL timeout, or a zero
    timespec → non-blocking harvest under `KEVENT_FLAG_IMMEDIATE` (XNU
    returns right after `kqueue_process()`, no scheduler round-trip); an
    explicit timespec bounds the wait, passed to the kernel as sec/nsec
    with no millisecond rounding.
  - `EINTR` retry loop, and `n <= 0` returns without dispatch — the tick
    never fabricates events.
  - **Wakeup is the C layer's**: `us_create_loop` arms the
    `EVFILT_MACHPORT` wakeup whose kevent flows through normal untagged
    dispatch; the Rust side builds no wakeup primitive of its own (no
    eventfd on mac).
- `src/bao_uloop/src/poll.rs` gained the kqueue normalization arm
  (`#[cfg(target_os = "macos")]`, ~line 456): `EVFILT_*`/`EV_*` → libus
  interest units, per-poll coalescing with the backward two-pass scan,
  mirroring `epoll_kqueue.c`'s kqueue branch.
- The crate-level gate is `#![cfg(any(target_os = "linux", target_os =
  "macos"))]` (`src/bao_uloop/src/lib.rs:62`); `bao_loop_tick` dispatches
  to `run_epoll` (Linux) or `kqueue::run_kqueue` (macOS) at `lib.rs:623-630`.
- macOS-gated unit tests exist in `kqueue.rs` (constants/layout invariants:
  `EVFILT_*` values, `EV_ERROR`/`EV_EOF` bits, `KEVENT_FLAG_IMMEDIATE`,
  `kevent64_s` 48-byte layout, lossless timespec conversion). They compile
  and run only on a mac host — executing them is part of the §4 checklist.
- Verification at landing: **Linux 45/45 zero regression** (commit
  `142e0097`) — that count is exact: the tree at wave 3 carried 47 test
  fns, 2 of them macos-gated, leaving 45 linux-visible, all green. The
  tree has since grown (`poll.rs` gained 7 more), so expect ~52
  linux-visible today.

## 3. Where the build stops today

### 3.1 Cross-compiling from Linux (`--target aarch64-apple-darwin`)

Probe-verified during M1:

1. Everything up to `bun_mimalloc_sys` in the dependency graph configures
   correctly for the apple target — the uws_sys matrix emits the right
   per-target defines (see §2).
2. **The blocker is `bun_mimalloc_sys`** (`src/mimalloc_sys/build.rs`): the
   vendored mimalloc is compiled as C++17 with `clang++`, and targeting
   `*-apple-darwin` without an Apple SDK (or an osxcross toolchain) fails
   in the C++ standard headers — `cstddef` not found. This is a toolchain
   gap, not a code gap.
   - Fix areas if cross builds ever matter: install an Apple SDK +
     osxcross, or (recommended) verify on real hardware instead.
   - Related nuance: `mimalloc_sys/build.rs` still gates
     `MI_MALLOC_OVERRIDE` with host-cfg `#[cfg(target_os = "linux")]`. On a
     real Mac (host == target) this behaves correctly — the malloc override
     stays off, exactly as intended — but it is wrong-shaped for
     cross-compiling and should be converted to `CARGO_CFG_TARGET_OS` if
     the cross path is ever exercised (uws_sys already did this in M1).
3. **Past mimalloc, nothing is known**: the link level has never been
   attempted for macOS. Cross-compilation stopped at the SDK wall before
   linking, so linker-level surprises (framework flags, symbol issues in
   servo/mozjs) are uncharted. Treat "cross-compile past mimalloc" as its
   own verification step, not a given.

### 3.2 On a real Mac (the supported path)

Prerequisites:

- **Rust nightly-2026-07-20** — pinned repo-wide (`rust-toolchain.toml`);
  stable fails with E0554 (`#![feature]` on a non-nightly compiler).
- **Xcode Command Line Tools** — clang, make; plus **python3** (mozjs
  build). Same native-toolchain class as the Linux path.
- **GStreamer runtime libraries** *if* media playback matters — the
  decision is still open (§5 gap 4).

No compile-layer blocker is known for the native path — and that statement
is exactly what the M3 checklist (§4) is designed to falsify, not confirm
by assumption.

## 4. First real-machine verification checklist (M3–M6)

Ordered cheapest-falsification-first; each step's failure mode feeds the
next fix. All `cargo test` invocations follow the repo's test discipline
(`--jobs 1` for cargo; nextest runs each test in its own process).

### M3 — full workspace type-check (macOS)

```bash
cargo check --workspace --jobs 1
```

Expected outcome per current knowledge: green. Any red item here is a
residual `cfg` gap (linux-only code without a mac arm) — file it with the
crate name; the matrix in §1 predicted none, which is precisely the claim
under test.

### M4 — full native build

```bash
cargo build -p bao_bin --jobs 1
```

First build compiles SpiderMonkey from source (20–40 min, cached
afterwards). This exercises the mozjs apple paths, the servo build, the
uws_sys kqueue matrix and the boringssl darwin sources end-to-end — the
first link-level evidence for macOS that exists. The explicit
falsification target here is platform-analysis knowledgeGap #2: the
`should_build_from_source() -> true` hardcode and the
`fix_stale_archive_objects()` make patch have never run under mac
clang/`CLANG_PATH` (§5.7).

### M5 — runtime semantics (the "event loop not yet proven" claim)

```bash
# Full bao_uloop suite — includes the macos-gated kqueue.rs tests,
# which execute for the first time on this hardware.
cargo nt -p bao_uloop

# The three loop-behaviour families the Linux side regression-runs:
cargo nt -p bao_uloop -E 'test(hangup)'
cargo nt -p bao_uloop -E 'test(paused_eof)'
cargo nt -p bao_uloop -E 'test(write_rearm)'

# Fetch streaming family (bounded staging + backpressure, loop-driven):
cargo nt -p bun_runtime -E 'test(fetch_stream)'
```

Prerequisite: §5 gap 2 (linux-faces of the `#[cfg(test)]` mods must be
mac-gated first, otherwise the *test target* does not compile on mac —
M3/M4 are unaffected because `cargo check`/`build` do not compile test
code). Plain `cargo test` alternative: `cargo test --test-threads=1`
(suite harness single-binary constraint — see repo CLAUDE.md).

M5 is also where platform-analysis knowledgeGap #5 gets its first
real-machine evidence: the `FilePoll` kqueue registration code
(`src/io/posix_event_loop.rs` carries the macos/freebsd arms and
`on_kqueue_event` handling) has never been built on mac — the path that
registers FilePoll-managed fds into the usockets `loop->fd` kqueue needs
on-hardware integration (§5.7).

### M6 — browser/CDP smoke

Run `examples/01-browser` (navigate → evaluate → screenshot) and the CDP
connect path (`Browser::connect("memory://bao")`). This is the first
end-to-end proof of the servo + SpiderMonkey stack on macOS; no prior
milestone has ever run a Bao page on Apple hardware. The critical
falsification target is platform-analysis knowledgeGap #1: software
rendering (`create_software_adapter()`, backed by surfman — a servo
workspace dependency, `vendor/servo/components/servo/Cargo.toml:151`)
depends on the surfman platform backend, which on mac is the CGL
software renderer; upstream servo uses this for linux/mac headless CI,
but this fork has never exercised it. It is the runtime lifeline of the
headless browser on macOS — if M6 fails, look here first (§5.7).

## 5. Known gaps (registered)

1. **kqueue arm: behavioural tests are macos-gated stubs until hardware
   arrives.** The unit tests in `kqueue.rs` check constants/layout
   invariants only; harvest + dispatch round-trip coverage requires a mac
   host (they are written and `cfg`'d in, waiting for M5).
2. **Linux-faces in `#[cfg(test)]` mods are not yet mac-gated.** `poll.rs`
   tests drive `libc::epoll_ctl` / `libc::epoll_event` (Linux-only libc
   surface; e.g. `src/bao_uloop/src/poll.rs:852-866`), and the
   `ipc_recvmsg_tests` helper reads `/proc/self/fd`
   (`src/bao_uloop/src/lib.rs:1813-1830`). Compiling the bao_uloop test
   target on mac requires gating these behind `#[cfg(target_os =
   "linux")]` (plus mac equivalents where the behaviour is testable).
3. **Performance tooling.** Linux `perf` is unavailable on mac; the
   equivalents are Instruments / `sample` / `leaks`. Any perf-sensitive
   verification done as part of M5+ should record the tool substitution.
4. **GStreamer decision.** `bao_browser` pins servo's `media-gstreamer`
   feature unconditionally (`src/bao_browser/Cargo.toml:24`). Either mac
   adopts Homebrew GStreamer runtime deps (upstream servo supports
   gstreamer on mac) or the feature needs per-target gating — decision
   pending, does not block M3–M5 (media loads at runtime). The wider
   servo-media platform-backend matrix on mac is itself unverified
   (platform-analysis knowledgeGap #6, §5.7).
5. **Link level unverified.** No full-binary link has ever succeeded (or
   failed) for macOS — M4 is the first datapoint.
6. **Cross-compile is not a supported path.** It stops at the Apple SDK
   wall in `bun_mimalloc_sys` (§3.1); even with an SDK, the link level is
   uncharted. Real hardware first.

### 5.7 Platform-analysis knowledgeGaps (2026-08-20 six-section report)

The platform adaptation analysis enumerated 8 knowledgeGaps — assumptions
that could not be verified against this repo or upstream, each needing a
real machine. Mac-related items are marked; two entries have since been
overturned by landed code and are kept here only with their resolution
status, so nobody cites a stale conclusion:

| # | KnowledgeGap | Scope | Status in this doc |
|---|---|---|---|
| 1 | Software rendering availability: `create_software_adapter()` depends on the surfman 0.13 platform backend (mac = CGL software renderer); upstream servo runs it for linux/mac headless CI. Runtime lifeline of the headless browser | **mac** | open — M6 falsification target (§4 M6) |
| 2 | mozjs compile pass-ability on mac: `should_build_from_source() -> true` hardcode (`vendor/mozjs/mozjs-sys/build.rs:715`) + `fix_stale_archive_objects()` make patch never ran under mac clang / Xcode CLT (`CLANG_PATH`) | **mac** | open — M4 falsification target (§4 M4) |
| 3 | libuv binary supplier: `src/libuv_sys` is pure FFI; upstream Bun supplies symbols via C++/CMake | win | header closure vendored in wave 4; remaining gap is windows link-level — out of scope here |
| 4 | crash_handler windows arm existence (scan: windows 0 hits, linux 22 / macos 24) | win | out of scope here |
| 5 | kqueue fd sharing: the FilePoll kqueue registration (`src/io/posix_event_loop.rs:760-815` and `:1255-1281` — `EVFILT_READ`/`WRITE`/`PROC`/`MACHPORT`) is in place but never built on mac; the path registering into the usockets `loop->fd` kqueue needs real-machine integration | **mac** | open — M5 falsification target (§4 M5); extends gap 1 |
| 6 | media-gstreamer alternative on mac/win: needs native gstreamer dev packages or a servo-media platform-backend swap; the backend matrix is unverified | mac-adjacent | folded into gap 4 |
| 7 | Cross-compile semantics: build.rs `#[cfg]` host-evaluation mismatch | mac+win | **resolved by M1** — the uws_sys matrix is `CARGO_CFG_TARGET_OS`-driven (§2); one residual host-cfg remains in `mimalloc_sys` (§3.1) |
| 8 | boringssl windows asm toolchain (nasm/perl; `OPENSSL_NO_ASM` downgrade cost unquantified) | win | out of scope here — the mac side is routine cmake/perl, low risk |

Items #1, #2 and #5 are the three mac real-machine verification points;
all three are wired into the §4 checklist at the milestone that first
exercises them.

## 6. File map (verify everything above yourself)

| Claim | Where |
|---|---|
| Platform matrix (eventing + root certs + fail-closed) | `src/uws_sys/build.rs:30-90` |
| Darwin root certs via dlopen (no link-time framework) | `src/uws_sys/csrc/bun-usockets/src/crypto/root_certs_darwin.cpp` |
| kqueue tick (kevent64, controlled timeout, EINTR, no-fabricate) | `src/bao_uloop/src/kqueue.rs` |
| kqueue event normalization arm | `src/bao_uloop/src/poll.rs:456+` (`#[cfg(target_os = "macos")]`) |
| Crate gate + tick dispatch | `src/bao_uloop/src/lib.rs:62`, `lib.rs:623-630` |
| macos-gated kqueue unit tests | `src/bao_uloop/src/kqueue.rs:137-186` |
| Cross-compile blocker (C++17 mimalloc, host-cfg nuance) | `src/mimalloc_sys/build.rs` |
| mozjs apple build paths | `vendor/mozjs/mozjs-sys/build.rs` (459, 600-603, 692, 798-801) |
| Servo macOS font platform (Core Text) | `vendor/servo/components/fonts/platform/macos/` |
| GStreamer feature pin | `src/bao_browser/Cargo.toml:24` |
| FilePoll kqueue registration arms (never built on mac) | `src/io/posix_event_loop.rs:134-142,333` (`target_os = "macos"` arm) |
| Software rendering backend (surfman, knowledgeGap #1) | `vendor/servo/components/servo/Cargo.toml:151` |
| M1+M2 landing (verification summary in message) | `git show 142e0097` |
| "Code surfaces complete, machine-verification remains" | `git show 4a956b27` |
