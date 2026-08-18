// Dual-mode build (stable ⇄ nightly): nightly-only `#![feature]` attributes
// in this crate are written as `#![cfg_attr(bao_nightly, feature(...))]` and
// nightly-only paths route through `bun_alloc::core_alloc` (which is core's
// allocator_api on nightly and the `allocator_api2` mirror on stable). The
// channel is detected from the compiler version string (nightlies carry
// "-nightly").
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
    // it on both channels — without this every `#[cfg(bao_nightly)]` errors.
    println!("cargo:rustc-check-cfg=cfg(bao_nightly)");
    println!("cargo:rerun-if-changed=build.rs");
}
