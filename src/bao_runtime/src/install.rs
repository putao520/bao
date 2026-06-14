//! Package manager integration — mirrors `bun_runtime::cli::install_command::install_with_cli()`.
//!
//! Calls the same `bun_install` functions as Bun's upstream CLI, with identical
//! control flow. The only difference is the entry point: Bun enters via
//! `InstallCommand::exec(ctx)` (pub(crate) in bun_runtime), we enter directly
//! from `bao_bin`'s clap dispatch through `bao_runtime::run_install()`.

use bun_install::package_manager_real::{
    CommandLineArguments, PackageManager, Subcommand,
    install_with_manager, update_package_json_and_install_with_manager,
    ROOT_PACKAGE_JSON_PATH,
};
use bun_install::package_manager_real::Command;
use bun_core::{Global, Output};

/// Execute `bao install` / `bao add`.
///
/// This is the `bao_runtime` equivalent of
/// `bun_runtime::cli::install_command::install_with_cli()`.
///
/// `CommandLineArguments::parse` reads from `bun_core::argv()` (falls back to
/// `std::env::args_os()`), so the caller's argv must contain the install
/// subcommand and any package arguments.
///
/// Callers should invoke `force_link_bun_install()` before this to ensure the
/// `__bun_dispatch__` symbols are linked in.
pub fn run_install() -> Result<(), i32> {
    let mut ctx = Command::ContextData::default();
    ctx.start_time = bun_core::time::nano_timestamp();

    let cli = match CommandLineArguments::parse(Subcommand::Install) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("bao install: failed to parse arguments: {}", e);
            return Err(1);
        }
    };

    // Same logic as upstream install_with_cli():
    // positionals[0] = "install", positionals[1..] = package names → Add.
    let subcommand = if cli.positionals.len() > 1 {
        Subcommand::Add
    } else {
        Subcommand::Install
    };

    // Register the global context pointer so install_with_manager can access it.
    Command::GLOBAL_CTX.store(&mut ctx, std::sync::atomic::Ordering::Release);

    let (manager, original_cwd) = match PackageManager::init(&mut ctx, cli, Subcommand::Install) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("bao install: initialization failed: {}", e);
            return Err(1);
        }
    };

    if subcommand == Subcommand::Add {
        manager.subcommand = Subcommand::Add;
        if manager.options.should_print_command_name() {
            Output::prettyln(format_args!(
                "<r><b>bao add <r><d>v{}<r>\n",
                Global::package_json_version_with_sha,
            ));
            Output::flush();
        }
        return match update_package_json_and_install_with_manager(manager, &mut ctx, &original_cwd) {
            Ok(()) => Ok(()),
            Err(e) => { eprintln!("bao add: {}", e); Err(1) }
        };
    }

    if manager.options.should_print_command_name() {
        Output::prettyln(format_args!(
            "<r><b>bao install <r><d>v{}<r>\n",
            Global::package_json_version_with_sha,
        ));
        Output::flush();
    }

    // SAFETY: ROOT_PACKAGE_JSON_PATH is written exactly once inside
    // PackageManager::init (above) on this thread; only read thereafter.
    let root_package_json_path = unsafe { ROOT_PACKAGE_JSON_PATH.read() };
    match install_with_manager(manager, &mut ctx, root_package_json_path, &original_cwd) {
        Ok(()) => {
            if manager.any_failed_to_install {
                Err(1)
            } else {
                Ok(())
            }
        }
        Err(e) => { eprintln!("bao install: {}", e); Err(1) }
    }
}
