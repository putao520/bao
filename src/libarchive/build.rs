fn main() {
    println!("cargo:rustc-link-lib=archive");

    // Dual-mode (stable ⇄ nightly): nightly-only cfg-gated paths need the
    // channel probe (same template as bun_alloc). Detected from the compiler
    // version string; declared via check-cfg so `unexpected_cfgs` accepts it.
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
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
    println!("cargo:rustc-check-cfg=cfg(bao_nightly)");
}
