// BAO FORK (2.3.0, #18 — msvc cross unlock; upstream tov/libffi-rs):
// 1. Target dispatch moved from the host `#[cfg(target_env = "msvc")]` to
//    `CARGO_CFG_TARGET_ENV` — the cfg form keys on the host running cargo, so
//    every linux-host cross build of a windows-msvc target wrongly took the
//    autoconf/configure path ("checking build system type... x86_64-pc-linux-
//    gnu ... OS msvc not recognized"). Both arms now always compile and the
//    TARGET env decides.
mod common;
mod msvc;
mod not_msvc;

fn main() {
    let is_msvc_target = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if cfg!(feature = "system") {
        if is_msvc_target {
            msvc::probe_and_link();
        } else {
            not_msvc::probe_and_link();
        }
    } else {
        if is_msvc_target {
            msvc::build_and_link();
        } else {
            not_msvc::build_and_link();
        }
    }
}
