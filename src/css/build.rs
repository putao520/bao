// Dual-mode build (stable ⇄ nightly): mirror of bun_alloc/collections probe.
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
