// Dual-mode build (stable ⇄ nightly): the `chan_vec_to_std` handoff (and any
// future channel-divergent code) is `#[cfg(bao_nightly)]`-gated; the channel
// is detected from the compiler version string (nightlies carry "-nightly").
// Same probe as bun_alloc / crash_handler / dotenv (W4 house template).
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
