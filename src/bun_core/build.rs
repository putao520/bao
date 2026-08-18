// Build scripts run on the host before bun_* crates are compiled; std is the only option.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]
//! Export `BUN_CODEGEN_DIR` and fingerprint `build_options.rs` for
//! `include!(concat!(env!("BUN_CODEGEN_DIR"), "/build_options.rs"))`.
//!
//! `build_options.rs` is written at configure time by
//! `scripts/build/buildOptionsRs.ts` from the resolved `Config` (sha,
//! version, baseline, …). This script does NOT run the generator — it just
//! resolves the path and tells cargo to track the file so a sha/version
//! change recompiles `bun_core`. Mirrors `src/{jsc,runtime}/build.rs`.
//!
//! Resolution order (W0a publish incorporation — a crates.io package can
//! only ship in-package files, so published consumers have no repo
//! `build/<profile>/codegen` to walk up to):
//!   1. `BUN_CODEGEN_DIR` env (embed builds via `scripts/build/rust.ts`)
//!   2. repo `build/debug/codegen` (local dev; `scripts/ensure-codegen.sh`)
//!   3. in-package `codegen_snapshot/build_options.rs` (byte-identical copy
//!      of the synthesized dev stub; refresh it at release time if real
//!      `Config` values are wanted for published builds)

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    // src/bun_core → repo root is two up.
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest
        .parent()
        .and_then(Path::parent)
        .expect("repo root from CARGO_MANIFEST_DIR")
        .to_path_buf();

    let codegen_dir = env::var("BUN_CODEGEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo.join("build/debug/codegen"));

    let build_options = codegen_dir.join("build_options.rs");
    let (dir, file) = if build_options.exists() {
        (codegen_dir, build_options)
    } else {
        let snapshot_dir = manifest.join("codegen_snapshot");
        let snapshot = snapshot_dir.join("build_options.rs");
        if !snapshot.exists() {
            panic!(
                "build_options.rs not found at {} (or in-package snapshot {}) \
                 — run `bun bd --configure-only` or scripts/ensure-codegen.sh first",
                build_options.display(),
                snapshot.display()
            );
        }
        (snapshot_dir, snapshot)
    };

    println!("cargo:rustc-env=BUN_CODEGEN_DIR={}", dir.display());
    println!("cargo:rerun-if-changed={}", file.display());
    println!("cargo:rerun-if-env-changed=BUN_CODEGEN_DIR");

    // Dual-mode (stable ⇄ nightly): nightly-only `#![feature]` attributes
    // are `cfg_attr(bao_nightly, …)`-gated in lib.rs; stable compiles the
    // equivalent stable paths. Channel detected from the compiler version
    // string (nightlies carry "-nightly").
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let is_nightly = std::process::Command::new(&rustc)
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
}
