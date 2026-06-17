// @trace REQ-PURE-008 [level:library] [entity:HtmlRewriter,HtmlRewriterConfig]
//! bun_lolhtml_sys - thin compat layer over lol_html crate (pure Rust, BSD-3-Clause).
//! vendor/lolhtml/c-api stub deleted. No C dependency.
#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
#![warn(unused_must_use)]
pub mod lol_html;
pub use lol_html::*;
