#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
#![warn(unused_must_use)]
//! Per-OS APIs that don't fit in `bun_sys` (signposts, splice/preadv2 wrappers).

// Zig's `Environment.isLinux` is `os.tag == .linux`, which is true on Android
// (Zig models Android as `os = linux, abi = android`); Rust splits them, so
// list both so the `#[no_mangle]` C exports (`ioctl_ficlone`, …) reach the
// linker on the `*-linux-android` targets. (`sys_epoll_pwait2` moved to its
// single owner uws_sys::c_hooks — 2026-08-19 dual-def sweep.)
#[cfg(target_os = "macos")]
pub mod darwin;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod linux;
