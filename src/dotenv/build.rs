// Dual-mode build (stable ⇄ nightly): nightly-only `#![feature]` attributes
// in this crate are written as `#![cfg_attr(bao_nightly, feature(...))]` and
// the fast paths that need them are `#[cfg(bao_nightly)]`-gated; stable
// builds compile against the `allocator_api2` mirror (see `core_alloc` in
// lib.rs). The channel is detected from the compiler version string
// (nightlies carry "-nightly").
use ::std::env;
use ::std::process::Command;

fn main() {
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let is_nightly = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|version| version.contains("nightly"))
        .unwrap_or(false);
    if is_nightly {
        println!("cargo:rustc-cfg=bao_nightly");
    }
    // Declare the custom cfg so `unexpected_cfgs` (workspace lints) accepts
    // it on both channels.
    println!("cargo:rustc-check-cfg=cfg(bao_nightly)");
    println!("cargo:rerun-if-changed=build.rs");
}
