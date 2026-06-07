#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
#![warn(unused_must_use)]
pub mod brotli_c;

/// No-op function that forces cargo to propagate the native link dependency
/// (`libbrotli.a`) to any crate that depends on `bun_brotli_sys`.
#[inline(never)]
pub fn force_link() {}
