// Dual-mode build (stable ⇄ nightly): the facade types this crate consumes
// (bun_core ChanVec / bun_alloc core_alloc AllocVec/AllocBox) resolve to the
// api2 mirrors on stable, so per-arm code here is `#[cfg(bao_nightly)]`-split.
// Channel detected from the compiler version string (nightlies carry
// "-nightly"). (bun_core / bun_alloc / bun_wyhash build.rs template.)
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
    println!("cargo:rustc-check-cfg=cfg(bao_nightly)");
    println!("cargo:rerun-if-changed=build.rs");
}
