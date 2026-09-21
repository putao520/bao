# libuv vendor tree (windows symbol supply, issue #34)

Vendored C source of the **oven-sh/libuv fork**, `bun` branch, pinned at:

- commit: `8023581113b276e7c1aee3f82da57ca0893faab1`
- subject: `win: use LoadLibraryExW for the lazily loaded system DLLs (#16)`
- version: uv 1.51.1-dev (`include/uv/version.h`: 1.51, patch 1, suffix dev)
- fetched: 2026-09-21 (`git clone --branch bun`, verified by `git rev-parse HEAD`)
- pinned upstream: oven-sh/bun `scripts/build/deps/libuv.ts` `LIBUV_COMMIT`
  @ oven-sh/bun `origin/main` = `4af1842c8c`

`libuv/` is the upstream tree at that commit **with the two fork patches
already applied** (see `patches/`); no other byte differs from the upstream
checkout. `patches/` carries the unmodified copies of the two upstream patch
files (`oven-sh/bun patches/libuv/` @ `4af1842c8c`), kept as the replay
recipe for future libuv bumps:

| patch | semantics |
|-------|-----------|
| `win-poll-rearm-before-callback.patch` | re-submit the AFD ioctl **before** `poll_cb` (AFD is level-triggered; upstream re-arms after, leaving a window a same-process loopback `fetch().abort()` RST can fall into) |
| `win-poll-abort-with-disconnect.patch` | `UV_DISCONNECT` watchers also subscribe `AFD_POLL_ABORT` (RST wakes a write-only poll); `UV_PRIORITIZED` becomes an ABORT-only subscription |

Both were applied with `git apply` (each `--check`ed first); combined result:
`src/win/poll.c` 51 insertions, 4 deletions.

## Build recipe

`../build.rs` compiles 37 sources (`src/*.c` 12 shared + `src/win/*.c` 25)
with the flags translated from oven-sh/bun `scripts/build/deps/libuv.ts`
@ `4af1842c8c`. Unix libuv sources present in the tree are **not** compiled
(bun's posix event loop is epoll/kqueue direct; node-api's posix uv_*
references go through uv-posix-stubs upstream).

## Version notes

- The `#[repr(C)]` FFI mirror in `../libuv.rs` was written against the
  **uv 1.51.0 release** face (the condensed `<uv.h>` closure mirrored in
  `src/uws_sys/csrc/bun-usockets/src/deps/libuv/include`). This vendor tree
  is the real upstream headers at the fork tip (1.51.1-dev). Reconciling the
  FFI/include faces with this tree is the follow-up FFI-sync contract.
