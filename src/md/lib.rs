#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
#![warn(unused_must_use)]

pub mod ansi_renderer;
pub mod entity;
pub mod helpers;
pub mod parser;
pub mod root;
pub mod types;

pub use root::RenderOptions;
