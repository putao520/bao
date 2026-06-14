use bun_runtime::force_link_bun_install;

/// Execute `bao install` / `bao add`.
///
/// This is the bao_cli handler layer. It owns CLI-specific concerns
/// (force-linking), then delegates to `bun_runtime::install::run_install()`
/// for the actual package manager logic — mirroring how every other command
/// handler in bao_cli delegates to the appropriate crate.
pub fn run_install() -> Result<(), i32> {
    // Force-link bun_install compilation unit so the linker resolves
    // __bun_dispatch__ symbols from lifecycle_script_runner / security_scanner.
    force_link_bun_install();

    bun_runtime::install::run_install()
}
