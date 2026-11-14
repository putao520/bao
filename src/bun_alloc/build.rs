// Dual-mode build (stable ⇄ nightly): nightly-only `#![feature]` attributes
// in this crate are written as `#![cfg_attr(bao_nightly, feature(...))]` and
// the fast paths that need them are `#[cfg(bao_nightly)]`-gated; stable
// builds compile against the `allocator_api2` mirror (see `core_alloc` in
// lib.rs). The channel is detected from the compiler version string
// (nightlies carry "-nightly").
//
// Gate selection is PROVEN, never assumed. `ast_alloc::AstAlloc::vec` is a
// `const fn` built on `Vec::new_in`, so the crate compiles only under a
// feature gate that makes `Vec::new_in` const on the running compiler:
//
//   nightly <  1.100  → `#![feature(allocator_api)]` alone gates const
//                       `Vec::new_in` (1.99.0-nightly and older)
//   nightly >= 1.100  → `allocator_api` is partially stabilized; the const
//                       surface moved to `allocator_ext` (1.101.0-nightly)
//   stable            → the `allocator_api2` mirror's `Vec`, no feature at all
//
// The `allocator_ext` arm is chosen only when the live compiler compiles a
// const `Vec::new_in` under it; otherwise the legacy `allocator_api` arm is
// chosen only when the live compiler compiles the same probe under that. If
// neither probe compiles, the build fails HERE with the compiler version
// instead of surfacing as a bare E0658/E0015 in `AstAlloc::vec` — that
// silent-fallback shape is exactly the 0.1.4/0.2.0 failure mode of issue #57.
use ::std::env;
use ::std::path::PathBuf;
use ::std::process::Command;

fn main() {
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    // Channel detection must be proven too: defaulting an unreadable
    // `rustc --version` to "stable" would select the allocator_api2 mirror on
    // a nightly and fail in `lib.rs` with a diagnostic that blames our code
    // instead of the toolchain (issue #57's failure shape, one layer down).
    let version = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|version| version.trim().to_string())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| {
            panic!(
                "bun_alloc: `{} --version` did not report a compiler version; the \
                 allocator feature gate cannot be selected without knowing the \
                 compiler channel",
                rustc
            )
        });
    let is_nightly = version.contains("nightly");
    if is_nightly {
        println!("cargo:rustc-cfg=bao_nightly");
        if probe_gate(&rustc, "allocator_ext") {
            println!("cargo:rustc-cfg=bao_nightly_allocator_ext");
        } else if !probe_gate(&rustc, "allocator_api") {
            // Neither known gate lets this compiler const-evaluate `Vec::new_in`.
            // `AstAlloc::vec` is a `const fn`; picking an arm by guesswork
            // would only move the failure into the crate with a diagnostic
            // that no longer names the gate (issue #57 class). Fail here,
            // where the compiler version and the gate table are both visible.
            panic!(
                "bun_alloc: no allocator feature gate known to this build script \
                 (allocator_ext, allocator_api) makes const `Vec::new_in` compile \
                 on this compiler (rustc: {version}). If the const surface has \
                 been stabilized, teach build.rs the no-feature arm; otherwise \
                 add the new gate name to the ladder."
            );
        }
    }
    // Declare the custom cfgs so `unexpected_cfgs` (workspace lints) accepts
    // them on both channels.
    println!("cargo:rustc-check-cfg=cfg(bao_nightly)");
    println!("cargo:rustc-check-cfg=cfg(bao_nightly_allocator_ext)");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Does the live compiler accept `#![feature(<feature>)]` *and* allow a const
/// `Vec::new_in` under it? That pair is exactly what `AstAlloc::vec` needs, so
/// a probe success is the proof for selecting the corresponding arm.
///
/// `-D stable_features` mirrors the crate's lint posture (`[lints] workspace =
/// true` ⇒ `warnings = deny`): a feature attribute that is only *accepted with
/// a stability warning* is not usable here — the real build would deny that
/// warning. Without it a stabilized successor name would be "proven" by the
/// probe and then fail in `lib.rs` with a warning-turned-error.
///
/// `allocator_api` was partially stabilized in 1.100.0 and succeeded by
/// `allocator_ext`, which now gates const `Vec::new_in` — but the successor
/// feature name is unknown to earlier nightlies (bao pins one). Probe the
/// live compiler instead of parsing version strings so every toolchain gets
/// the exact feature set it accepts.
fn probe_gate(rustc: &str, feature: &str) -> bool {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR for build scripts"));
    let src = out_dir.join(format!("bao_alloc_{feature}_probe.rs"));
    let body = format!(
        "#![feature({feature})]\n\
         use std::alloc::Global;\n\
         pub const fn bao_probe<T>() -> Vec<T, Global> {{ Vec::new_in(Global) }}\n"
    );
    std::fs::write(&src, body).expect("bun_alloc: allocator-gate probe source write failed");
    Command::new(rustc)
        .arg("--edition=2024")
        .arg("--crate-type=lib")
        .arg("--emit=metadata")
        // Deny the stability warning: `warnings = deny` (workspace lints) turns
        // it into a build error, so a gate that only warns is not a valid arm.
        .arg("-D")
        .arg("stable_features")
        .arg("-o")
        .arg(out_dir.join(format!("bao_alloc_{feature}_probe.rmeta")))
        .arg(&src)
        .output()
        .expect("bun_alloc: allocator-gate probe rustc spawn failed")
        .status
        .success()
}
