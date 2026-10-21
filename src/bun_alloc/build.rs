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
    let version = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok());
    let is_nightly = version
        .as_deref()
        .map(|version| version.contains("nightly"))
        .unwrap_or(false);
    if is_nightly {
        println!("cargo:rustc-cfg=bao_nightly");
        if probe_allocator_ext(&rustc) {
            println!("cargo:rustc-cfg=bao_nightly_allocator_ext");
        }
    }
    // Declare the custom cfgs so `unexpected_cfgs` (workspace lints) accepts
    // them on both channels.
    println!("cargo:rustc-check-cfg=cfg(bao_nightly)");
    println!("cargo:rustc-check-cfg=cfg(bao_nightly_allocator_ext)");
    println!("cargo:rerun-if-changed=build.rs");
}

// `allocator_api` was partially stabilized in 1.100.0 and succeeded by
// `allocator_ext`, which now gates const `Vec::new_in` — but the successor
// feature name is unknown to earlier nightlies (bao pins one). Probe the
// live compiler instead of parsing version strings so every toolchain gets
// the exact feature set it accepts.
fn probe_allocator_ext(rustc: &str) -> bool {
    let out_dir = env::var("OUT_DIR").unwrap_or_else(|_| env::temp_dir().display().to_string());
    let out_dir = std::path::Path::new(&out_dir);
    let src = out_dir.join("bao_alloc_ext_probe.rs");
    let written = std::fs::write(
        &src,
        "#![feature(allocator_ext)]\n\
         use std::alloc::Global;\n\
         pub const fn bao_probe<T>() -> Vec<T, Global> { Vec::new_in(Global) }\n",
    );
    written.is_ok()
        && Command::new(rustc)
            .arg("--edition=2024")
            .arg("--crate-type=lib")
            .arg("--emit=metadata")
            .arg("-o")
            .arg(out_dir.join("bao_alloc_ext_probe.rmeta"))
            .arg(&src)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
}
