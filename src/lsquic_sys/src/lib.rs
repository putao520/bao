//! Compiled lsquic C library providing QUIC/HTTP3 transport.
//!
//! This crate compiles the litespeedtech/lsquic C library (~85 .c files)
//! plus lsqpack (QPACK header compression for HTTP/3) and lshpack
//! (HPACK header compression for HTTP/2). The lsquic engine is used by
//! uSockets' quic.c for HTTP/3 support.

/// No-op function that forces cargo to propagate the native link dependency
/// (`liblsquic.a`) to any crate that depends on `bun_lsquic_sys`.
#[inline(never)]
pub fn force_link() {}

/// No-op function that forces cargo to propagate the native link dependency
/// (`liblshpack.a`) to any crate that depends on `bun_lsquic_sys`.
///
/// Merged from `bun_lshpack_sys` — lshpack is now compiled inline in this
/// crate's build.rs, so the force_link lives here too.
#[inline(never)]
pub fn force_link_lshpack() {}
