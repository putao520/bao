fn main() {
    // Export this package's source root to dependents' build scripts via the
    // `links` mechanism (bao-mozjs-sys consumes DEP_BAO_MOZJS_SRC_*_ROOT to
    // synthesize the virtual mozjs topsrcdir).
    println!("cargo:root={}", std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=mozjs/");
}
