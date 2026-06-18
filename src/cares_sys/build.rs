fn main() {
    // c-ares 系统库: bun_cares_sys FFI(c_ares.rs)调用 ares_* (ares_inet_ntop 等),
    // 需要 link 系统 libcares。cares 是系统库(非 bao 编译的 lsquic/boringssl)。
    // 历史: cares_sys 之前无 build.rs,增量构建曾掩盖 -lcares 缺失,
    // 全量 link(如 BCE-007-R2 改动触发)暴露 undefined symbol: ares_inet_ntop。
    println!("cargo:rustc-link-lib=cares");
}
