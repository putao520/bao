// surfman embedder obligation (`declare_surfman!`, surfman src/macros.rs):
// on Windows the GPU drivers look for the exported `NvOptimusEnablement` /
// `AmdPowerXpressRequestHighPerformance` symbols in the executable to pick the
// discrete GPU; without the `.drectve` /export link args surfman prints
// "Could not find the NVIDIA and/or AMD GPU selection symbols" and the
// hybrid-GPU selection defaults to the wrong adapter.
// Expanded verbatim instead of invoking the upstream macro because this crate
// is edition 2024, where `#[no_mangle]` / `#[link_section]` must be written in
// `#[unsafe(...)]` form (the upstream macro emits the bare edition-2021 forms),
// and renamed `BAO_SURFMAN_*` because a leading-underscore no_mangle static
// diverges on the windows-msvc link face: the defined COFF symbol loses the
// underscore while rustc's export machinery keeps it, so lld-link dies with
// "undefined symbol: _SURFMAN_LINK_ARGS" (2026-09-27 observed). The static's
// NAME is not load-bearing — the .drectve bytes exporting the two driver
// symbols are. Must live in the binary crate root so the linker sees the
// export directives. Twin: src/bao_browser/tests/suite/main.rs (edition 2021,
// same expansion).
#[cfg(target_os = "windows")]
#[used]
#[unsafe(link_section = ".drectve")]
static BAO_SURFMAN_LINK_ARGS: [u8; 74] =
    *b" /export:NvOptimusEnablement /export:AmdPowerXpressRequestHighPerformance ";
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static mut NvOptimusEnablement: i32 = 1;
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static mut AmdPowerXpressRequestHighPerformance: i32 = 1;

fn main() {
    if let Err(code) = bao_cli::cli::run() {
        std::process::exit(code);
    }
}
