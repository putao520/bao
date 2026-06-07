//! Compiled ls-hpack C library providing `lshpack_wrapper_*` symbols.
//!
//! This crate compiles the litespeedtech/ls-hpack C library and a thin C
//! wrapper (`lshpack_wrapper.c`) that bridges ls-hpack's API to the
//! `lshpack_wrapper_*` ABI expected by `bun_http::lshpack`.

// The C library exports the symbols; no Rust-side re-exports needed.
// `bun_http::lshpack` declares the same `unsafe extern "C"` block and
// the linker resolves to the compiled C library.

/// No-op function that forces cargo to propagate the native link dependency
/// (`liblshpack.a`) to any crate that depends on `bun_lshpack_sys`.
#[inline(never)]
pub fn force_link() {}
