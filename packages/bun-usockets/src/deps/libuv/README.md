# Vendored libuv include face (windows closure)

libuv public headers vendored so `eventing/libuv.c` (the LIBUS_USE_LIBUV
backend, windows-only) and every TU that reaches `internal/eventing/libuv.h`
can compile. This is the include face only — no libuv sources; all `uv_*`
symbols are supplied at link time by `bun_libuv_sys` (the cfg(windows)
dependency of `bun_uws_sys`), whose `#[repr(C)]` mirrors target the same
version.

- Source: upstream Bun `src/jsc/bindings/libuv/` (verbatim copy of libuv
  include/ at commit bb706f5fe71827f667f0bce532e95ce0698a498d)
- Version: uv 1.51.0 (`uv/version.h`; matches `src/libuv_sys/libuv.rs` mirrors)
- Subset: the `_WIN32` transitive closure of `uv.h` — `uv/errno.h`,
  `uv/version.h`, `uv/win.h`, `uv/tree.h`, `uv/threadpool.h`. The unix branch
  of `uv.h` (`uv/unix.h` + per-OS children) is deliberately NOT vendored: the
  libuv backend is never compiled on unix (epoll/kqueue are), so those headers
  would be dead files.
- Upstream sync: re-copy from upstream Bun (or libuv include/ at the pinned
  commit) and keep this tree byte-identical; `uv/win.h` system includes
  (`winsock2.h`, `windows.h`, ...) are supplied by the Windows SDK toolchain.
