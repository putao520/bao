// @trace REQ-IMPL-01: Phase 1 SpiderMonkey engine replacement (completed)
// @trace REQ-IMPL-02: Phase 2 servo engine integration + rendering (completed)
// @trace REQ-IMPL-03: Phase 3 CDP Server implementation (completed)
// @trace REQ-IMPL-04: Phase 4 Stealth anti-fingerprinting (completed)
// @trace REQ-IMPL-05: Phase 5 Integration testing and release (completed)
// @trace REQ-ENG-006: Bun API adaptation — bao test runner execution/report/exit-code (run_test_file)

use bao_browser::BrowserConfig;
use bao_stealth::StealthProfile;
use clap::Parser;

#[derive(Parser)]
#[command(name = "bao", about = "Bao Runtime — SpiderMonkey + Servo")]
struct Cli {
    /// Evaluate the given code string (Bun-style top-level `-e`/`--eval`).
    /// When present, Bao runs the code and exits — no subcommand needed.
    /// Equivalent to `bao run --eval <code>`.
    #[arg(short, long)]
    eval: Option<String>,

    /// Terminate a runaway script after <MS> milliseconds (engine-native
    /// interrupt; exit code 124, GNU-timeout convention). While a controlled
    /// script entry is in flight, SIGINT (Ctrl-C) cancels it gracefully
    /// (exit code 130) instead of killing the process. Applies only to
    /// script execution (`bao run`, `bao -e`); without it nothing changes.
    #[arg(long, global = true, value_parser = parse_timeout_ms)]
    timeout: Option<u64>,

    // ── WPT official-toolchain face (REQ-BRW-002) ────────────────────────
    // Upstream servoshell-compatible flag surface: wptrunner's `servo`
    // product launches the binary as
    // `binary --webdriver=PORT --hard-fail --ignore-certificate-errors
    //  --enable-experimental-web-platform-features --window-size 800x600
    //  [--certificate-path …] [--user-stylesheet …] [--pref=K=V …]
    //  [--prefs-file …] --headless about:blank --config-dir … --temporary-storage`
    // and speaks WebDriver to it. Any of these flags (or the positional URL)
    // selects the browser entry; flags mirror servoshell's names exactly so
    // the runner command line is byte-compatible.
    /// Serve the WebDriver protocol on [PORT] (default 7000) and run until
    /// the WebDriver Shutdown command. Upstream `--webdriver[=port]`.
    #[arg(long, value_name = "PORT", num_args(0..=1), default_missing_value = "7000")]
    webdriver: Option<u16>,

    /// Exit non-zero if the engine hits a hard error (upstream `--hard-fail`;
    /// accepted for command-line parity — bao fails hard by default).
    #[arg(long)]
    hard_fail: bool,

    /// Skip peer-certificate verification failures (upstream
    /// `--ignore-certificate-errors`; wptrunner always passes it).
    #[arg(long)]
    ignore_certificate_errors: bool,

    /// Enable experimental web platform features (upstream
    /// `--enable-experimental-web-platform-features`).
    #[arg(long)]
    enable_experimental_web_platform_features: bool,

    /// Initial viewport as WxH (upstream `--window-size`).
    #[arg(long, value_name = "WxH")]
    window_size: Option<String>,

    /// CA certificate to trust for page TLS (upstream `--certificate-path`).
    #[arg(long, value_name = "PATH")]
    certificate_path: Option<String>,

    /// JSON file of preferences applied onto the global pref store (upstream
    /// `--prefs-file`; wptrunner passes resources/wpt-prefs.json).
    #[arg(long, value_name = "PATH")]
    prefs_file: Option<String>,

    /// Single preference override K=V (upstream `--pref`, repeatable).
    #[arg(long = "pref", value_name = "K=V")]
    pref: Vec<String>,

    /// User stylesheet injected into every page (upstream `--user-stylesheet`,
    /// repeatable).
    #[arg(long, value_name = "PATH")]
    user_stylesheet: Vec<String>,

    /// Per-profile config directory (upstream `--config-dir`).
    #[arg(long, value_name = "DIR")]
    config_dir: Option<String>,

    /// Use throwaway storage for this run (upstream `--temporary-storage`;
    /// accepted for command-line parity — bao already keeps per-run state).
    #[arg(long)]
    temporary_storage: bool,

    /// Headless mode (upstream `--headless`; bao is headless by default —
    /// accepted for command-line parity).
    #[arg(long)]
    headless: bool,

    /// Initial URL for the browser entry (the trailing positional in the
    /// wptrunner command line, e.g. `about:blank`).
    url: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

/// `--timeout` parser: positive milliseconds (0 is rejected — an instant
/// deadline is always a flag mistake, fail-closed with a clap usage error).
fn parse_timeout_ms(value: &str) -> ::std::result::Result<u64, String> {
    match value.parse::<u64>() {
        Ok(ms) if ms > 0 => Ok(ms),
        _ => Err(String::from(
            "--timeout expects a positive number of milliseconds (e.g. --timeout 5000)",
        )),
    }
}

#[derive(clap::Subcommand)]
enum Commands {
    Run {
        #[arg(short, long)]
        eval: Option<String>,
        #[arg(short, long)]
        r#module: bool,
        file: Option<String>,
    },
    Build {
        #[arg(short, long)]
        outdir: Option<String>,
        #[arg(long, default_value = "bun")]
        target: String,
        #[arg(long, default_value = "esm")]
        format: String,
        #[arg(long)]
        minify: bool,
        #[arg(long)]
        sourcemap: bool,
        entrypoint: String,
    },
    Test {
        #[arg(short, long)]
        eval: Option<String>,
        files: Vec<String>,
    },
    /// Install dependencies (delegates to bun_install via bao_runtime)
    Install {
        /// All trailing args are forwarded to bun_install::CommandLineArguments::parse()
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    Browser {
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value = "9222")]
        cdp_port: u16,
        #[arg(long, default_value_t = true)]
        headless: bool,
        #[arg(long)]
        stealth: bool,
    },
    /// Diagnose the local environment (Rust, clang, SpiderMonkey, DISPLAY, CDP).
    ///
    /// Walks the native toolchain Bao depends on and reports what's present
    /// or missing, so a failed build can be understood without reading the
    /// whole monorepo. Informational only — never exits non-zero.
    Doctor,
    /// Compatibility inventory reports over the four INVENTORY SSOTs
    /// (compat/{node,bun,cdp,web}). Informational only — always exits 0 on
    /// success; Partial/Unsupported rows are inventory facts, not failures
    /// (consumers that need a gate read --json).
    Compat {
        /// Report a single domain; omit for the four-domain summary.
        #[command(subcommand)]
        domain: Option<CompatDomain>,
        /// Machine-readable JSON output.
        #[arg(long, global = true)]
        json: bool,
    },
}

/// `bao compat` report domains (the four INVENTORY SSOTs).
#[derive(clap::Subcommand)]
enum CompatDomain {
    /// Node.js API inventory (compat/node).
    Node,
    /// Bun API inventory (compat/bun).
    Bun,
    /// Chrome DevTools Protocol inventory (compat/cdp).
    Cdp,
    /// Web platform family inventory (compat/web).
    Web,
}

impl From<CompatDomain> for crate::compat::Domain {
    fn from(d: CompatDomain) -> Self {
        match d {
            CompatDomain::Node => crate::compat::Domain::Node,
            CompatDomain::Bun => crate::compat::Domain::Bun,
            CompatDomain::Cdp => crate::compat::Domain::Cdp,
            CompatDomain::Web => crate::compat::Domain::Web,
        }
    }
}

/// Drain buffered JS-side output (console.*, process.stdout/stderr.write)
/// before any CLI-side printing, so script output and CLI result/error lines
/// interleave in execution order. Process-exit drainage is covered by the
/// flush guard held in [`run`]; this only fixes mid-run ordering.
fn flush_js_output() {
    bun_core::output::flush();
}

/// Process entry — parses argv and dispatches to the appropriate command handler.
/// Returns `Err(exit_code)` on failure; callers should `std::process::exit(code)`.

pub fn run() -> ::std::result::Result<(), i32> {
    // Process-owner output bring-up (same contract as `bun_bin` main): publish
    // the global stdout/stderr stream slots from the real stdio fds and hold a
    // flush guard for the whole CLI lifetime. JS sink writes (console.*,
    // process.stdout.write) are buffered by default (`ENABLE_BUFFERING` =
    // IS_NATIVE); without this guard every buffered byte is silently dropped
    // at process exit — the script runs, the exit code is correct, but
    // console.log never reaches stdout.
    bun_core::output::stdio::init();
    let _output_flush = bun_core::output::flush_guard();

    // Bun.build native driver (REQ-ENG-006): register the full bun_bundler
    // BundleV2 pipeline behind bun_runtime's JS face. Idempotent; without it
    // Bun.build resolves with an explicit success:false + logs (fail-closed).
    bao_bundler::build_api::install();

    let cli = Cli::parse();
    // WPT official-toolchain face (REQ-BRW-002): any servoshell-compatible
    // browser flag (or the positional URL) selects the browser entry before
    // any other dispatch — wptrunner's command line carries no subcommand.
    let browser_entry = cli.webdriver.is_some()
        || cli.hard_fail
        || cli.ignore_certificate_errors
        || cli.enable_experimental_web_platform_features
        || cli.window_size.is_some()
        || cli.certificate_path.is_some()
        || cli.prefs_file.is_some()
        || !cli.pref.is_empty()
        || !cli.user_stylesheet.is_empty()
        || cli.config_dir.is_some()
        || cli.temporary_storage
        || cli.headless;
    if browser_entry || cli.url.is_some() {
        return run_browser_entry(&cli);
    }
    // --timeout only drives script execution entries (SM-EVOLUTION #24 S1
    // CLI wiring: `bao run` / top-level `-e`). On every other subcommand it
    // is rejected fail-closed instead of being silently ignored.
    let timeout_consumed = cli.eval.is_some() || matches!(cli.command, Some(Commands::Run { .. }));
    if !timeout_consumed && cli.timeout.is_some() {
        eprintln!("error: --timeout applies only to script execution (`bao run`, `bao -e`)");
        return Err(2);
    }
    // Top-level `-e` / `--eval` (Bun-compatible): runs the code as a script.
    // This is the form used by upstream test harnesses that spawn
    // `bunExe() -e script` to exercise TOCTOU PoCs in a fresh subprocess.
    if let Some(code) = cli.eval {
        return run_eval(&code, cli.timeout);
    }
    match cli.command {
        Some(Commands::Run {
            eval,
            r#module,
            file,
        }) => {
            if let Some(code) = eval {
                if r#module {
                    run_module_eval(&code, cli.timeout)
                } else {
                    run_eval(&code, cli.timeout)
                }
            } else if let Some(path) = file {
                run_file(&path, r#module, cli.timeout)
            } else {
                eprintln!("bao run: no input file");
                Err(1)
            }
        }
        Some(Commands::Build {
            outdir,
            target,
            format,
            minify,
            sourcemap,
            entrypoint,
        }) => run_build(
            &entrypoint,
            outdir.as_deref(),
            &target,
            &format,
            minify,
            sourcemap,
        ),
        Some(Commands::Test { eval, files }) => run_test(eval.as_deref(), &files),
        Some(Commands::Install { .. }) => crate::install::run_install(),
        Some(Commands::Browser {
            url,
            cdp_port,
            headless,
            stealth,
        }) => run_browser(url, cdp_port, headless, stealth),
        Some(Commands::Doctor) => crate::doctor::run(),
        Some(Commands::Compat { domain, json }) => {
            crate::compat::run(domain.map(crate::compat::Domain::from), json)
        }
        None => {
            eprintln!("bao: no command given. Try `bao --help`.");
            Err(1)
        }
    }
}

/// Exit code for a control-terminated entry (SM-EVOLUTION #24 S1 CLI wiring):
/// `Some(124)` = GNU timeout(1) convention for a timed-out command;
/// `Some(130)` = 128+SIGINT, the conventional shell-visible code for a
/// SIGINT-interrupted process (the signal is converted into a controlled
/// termination, so the equivalent code is surfaced instead of dying by
/// signal). `None` = not a control termination (plain script error → the
/// entries' existing exit-code-1 mapping). Rationale recorded per the S1
/// CLI legislation (user ruling 2026-09-10).
fn control_termination_exit_code(state: bun_runtime::runtime::TerminalState) -> Option<i32> {
    match state {
        bun_runtime::runtime::TerminalState::TimedOut => Some(124),
        bun_runtime::runtime::TerminalState::Cancelled => Some(130),
        _ => None,
    }
}

fn run_eval(code: &str, timeout_ms: Option<u64>) -> ::std::result::Result<(), i32> {
    let mut rt = bun_runtime::NodeRuntime::new().map_err(|_| {
        eprintln!("Error: Failed to initialize SpiderMonkey");
        1
    })?;
    // Set by the controlled arm when the entry was terminated by the control
    // (deadline / SIGINT→cancel); plain errors keep the exit-code-1 mapping.
    let mut termination_exit: Option<i32> = None;
    let eval_result = match timeout_ms {
        None => match rt.eval(code, "<eval>") {
            Ok(val) => {
                flush_js_output();
                if !val.is_undefined() {
                    println!("{}", val.to_display_string());
                }
                Ok(())
            }
            Err(e) => {
                flush_js_output();
                eprintln!("Error: {}", e);
                Err(1)
            }
        },
        Some(ms) => {
            // #24 S1 CLI wiring: engine-native control around the script
            // entry (deadline + SIGINT→cancel; see InterruptBridge).
            let control = rt.execution_control();
            let mut bridge = bun_runtime::interrupt_bridge::InterruptBridge::install();
            bridge.arm(&control);
            let result = rt.eval_with_control(
                &control,
                code,
                "<eval>",
                Some(::std::time::Duration::from_millis(ms)),
            );
            bridge.disarm();
            let mapped = match result {
                Ok(val) => {
                    flush_js_output();
                    if !val.is_undefined() {
                        println!("{}", val.to_display_string());
                    }
                    Ok(())
                }
                Err(e) => {
                    flush_js_output();
                    eprintln!("Error: {}", e);
                    termination_exit = control_termination_exit_code(control.terminal_state());
                    Err(termination_exit.unwrap_or(1))
                }
            };
            // Drop re-raises a swallowed-but-unhandled SIGINT under the
            // restored disposition before any further exit-code work.
            drop(bridge);
            mapped
        }
    };
    // Orderly exit: explicit process.exit() / Bun.exit(), or an exitCode
    // steered by the script / 'exit' listeners (Node: natural exit honours
    // process.exitCode).
    //
    // Control termination (deadline / SIGINT→cancel) takes precedence: the
    // module pipeline routes the uncatchable termination through the
    // uncaught-exception machinery, which request_exit(1)s as a side effect —
    // the 124/130 contract is the truthful signal and must win over it.
    if let Some(code) = termination_exit {
        return Err(code);
    }
    if bun_runtime::should_exit() || bun_runtime::exit_code() != 0 {
        return Err(bun_runtime::exit_code());
    }
    eval_result
}

fn run_file(
    path: &str,
    force_module: bool,
    timeout_ms: Option<u64>,
) -> ::std::result::Result<(), i32> {
    let mut rt = bun_runtime::NodeRuntime::new().map_err(|_| {
        eprintln!("Error: Failed to initialize SpiderMonkey");
        1
    })?;

    // Set by the controlled arm when the entry was terminated by the control
    // (deadline / SIGINT→cancel); plain errors keep the exit-code-1 mapping.
    let mut termination_exit: Option<i32> = None;
    let result = match timeout_ms {
        None => {
            let is_module = force_module || path.ends_with(".mjs");
            if is_module {
                let source = std::fs::read_to_string(path).map_err(|e| {
                    eprintln!("Error reading {}: {}", path, e);
                    1
                })?;
                rt.eval_module(&source, path)
            } else {
                rt.run_file(path)
            }
        }
        Some(ms) => {
            // #24 S1 CLI wiring: the file entry (script-vs-module dispatch
            // inside the runtime) runs under engine-native control.
            let control = rt.execution_control();
            let mut bridge = bun_runtime::interrupt_bridge::InterruptBridge::install();
            bridge.arm(&control);
            let outcome = if force_module || path.ends_with(".mjs") {
                let source = std::fs::read_to_string(path).map_err(|e| {
                    eprintln!("Error reading {}: {}", path, e);
                    1
                })?;
                rt.eval_module_with_control(
                    &control,
                    &source,
                    path,
                    Some(::std::time::Duration::from_millis(ms)),
                )
            } else {
                rt.run_file_with_control(
                    &control,
                    path,
                    Some(::std::time::Duration::from_millis(ms)),
                )
            };
            bridge.disarm();
            drop(bridge);
            // Capture the terminal-state exit code while `control` is live;
            // the shared printer below cannot see it.
            outcome.map_err(|e| {
                termination_exit = control_termination_exit_code(control.terminal_state());
                e
            })
        }
    };

    let eval_result = match result {
        Ok(_) => {
            flush_js_output();
            Ok(())
        }
        Err(e) => {
            flush_js_output();
            eprintln!("Error: {}", e);
            Err(termination_exit.unwrap_or(1))
        }
    };
    // Orderly exit: explicit process.exit() / Bun.exit(), or an exitCode
    // steered by the script / 'exit' listeners (Node: natural exit honours
    // process.exitCode).
    //
    // Control termination takes precedence over the exit machinery (see
    // run_module_eval for the module-path rationale).
    if let Some(code) = termination_exit {
        return Err(code);
    }
    if bun_runtime::should_exit() || bun_runtime::exit_code() != 0 {
        return Err(bun_runtime::exit_code());
    }
    eval_result
}

fn run_module_eval(code: &str, timeout_ms: Option<u64>) -> ::std::result::Result<(), i32> {
    let mut rt = bun_runtime::NodeRuntime::new().map_err(|_| {
        eprintln!("Error: Failed to initialize SpiderMonkey");
        1
    })?;
    // Set by the controlled arm when the entry was terminated by the control
    // (deadline / SIGINT→cancel); plain errors keep the exit-code-1 mapping.
    let mut termination_exit: Option<i32> = None;
    let eval_result = match timeout_ms {
        None => match rt.eval_module(code, "<module>") {
            Ok(_) => {
                flush_js_output();
                Ok(())
            }
            Err(e) => {
                flush_js_output();
                eprintln!("Error: {}", e);
                Err(1)
            }
        },
        Some(ms) => {
            // #24 S1 CLI wiring: module entry under engine-native control
            // (whole-entry arm incl. the post-eval event-loop pump).
            let control = rt.execution_control();
            let mut bridge = bun_runtime::interrupt_bridge::InterruptBridge::install();
            bridge.arm(&control);
            let result = rt.eval_module_with_control(
                &control,
                code,
                "<module>",
                Some(::std::time::Duration::from_millis(ms)),
            );
            bridge.disarm();
            let mapped = match result {
                Ok(_) => {
                    flush_js_output();
                    Ok(())
                }
                Err(e) => {
                    flush_js_output();
                    eprintln!("Error: {}", e);
                    termination_exit = control_termination_exit_code(control.terminal_state());
                    Err(termination_exit.unwrap_or(1))
                }
            };
            drop(bridge);
            mapped
        }
    };
    // Orderly exit: explicit process.exit() / Bun.exit(), or an exitCode
    // steered by the script / 'exit' listeners (Node: natural exit honours
    // process.exitCode).
    //
    // Control termination takes precedence over the exit machinery: the
    // module pipeline routes the uncatchable termination through the
    // uncaught-exception machinery, which request_exit(1)s as a side effect —
    // the 124/130 contract is the truthful signal and must win over it.
    if let Some(code) = termination_exit {
        return Err(code);
    }
    if bun_runtime::should_exit() || bun_runtime::exit_code() != 0 {
        return Err(bun_runtime::exit_code());
    }
    eval_result
}

fn run_build(
    entrypoint: &str,
    outdir: Option<&str>,
    target: &str,
    format: &str,
    minify: bool,
    sourcemap: bool,
) -> ::std::result::Result<(), i32> {
    let out_dir = outdir.unwrap_or("dist");
    ::std::fs::create_dir_all(out_dir).ok();

    // Resolve target enum from string (matches bun_bundler::options::Target)
    let _target = parse_target(target);

    // Resolve output format enum
    let _format = parse_format(format);

    let basename = ::std::path::Path::new(entrypoint)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bundle.js".into());
    let out_path = format!("{}/{}", out_dir, basename);

    let bundle = bao_bundler::build(entrypoint, minify, target).map_err(|e| {
        eprintln!("Error: {}", e);
        1
    })?;

    ::std::fs::write(&out_path, bundle.code.as_bytes()).map_err(|e| {
        eprintln!("Error writing {}: {}", out_path, e);
        1
    })?;

    if let Some(sm) = &bundle.source_map {
        let sm_path = format!("{}.map", out_path);
        ::std::fs::write(&sm_path, sm.as_bytes()).map_err(|e| {
            eprintln!("Error writing sourcemap {}: {}", sm_path, e);
            1
        })?;
    }

    eprintln!(
        "{} bundled → {} (target: {}, format: {}{})",
        entrypoint,
        out_path,
        target,
        format,
        if sourcemap { ", sourcemap" } else { "" }
    );

    if bundle.source_map.is_none() && sourcemap {
        eprintln!("warning: --sourcemap requested but no sourcemap generated (Phase 1 limitation)");
    }

    Ok(())
}

/// Parse target string into the bundler's Target enum.
/// Accepts: browser, bun, node, macro
fn parse_target(target: &str) -> bun_ast::Target {
    match target {
        "browser" => bun_ast::Target::Browser,
        "bun" => bun_ast::Target::Bun,
        "node" => bun_ast::Target::Node,
        "macro" | "bun_macro" => bun_ast::Target::BunMacro,
        other => {
            eprintln!("warning: unknown target '{}', defaulting to 'bun'", other);
            bun_ast::Target::Bun
        }
    }
}

/// Parse format string into the bundler's Format enum.
/// Accepts: esm, cjs, iife
fn parse_format(format: &str) -> bun_options_types::Format {
    match format {
        "esm" => bun_options_types::Format::Esm,
        "cjs" => bun_options_types::Format::Cjs,
        "iife" => bun_options_types::Format::Iife,
        other => {
            eprintln!("warning: unknown format '{}', defaulting to 'esm'", other);
            bun_options_types::Format::Esm
        }
    }
}

/// Run one test file, render its report, and accumulate the counters
/// (`bao test` per-file execution path shared by the discovery and
/// explicit-files branches).
fn run_one_test_file(
    rt: &mut bun_runtime::NodeRuntime,
    path_str: &str,
    total_passed: &mut u32,
    total_failed: &mut u32,
) {
    let report = rt.run_test_file(path_str);
    flush_js_output();
    match report {
        Ok(report) => {
            render_report(&report);
            *total_passed += report.passed;
            *total_failed += report.failed;
        }
        Err(e) => {
            eprintln!("FAIL [{}]: {}", path_str, e);
            *total_failed += 1;
        }
    }
    bun_runtime::clear_exit();
}

fn run_test(eval: Option<&str>, files: &[String]) -> ::std::result::Result<(), i32> {
    let mut rt = bun_runtime::NodeRuntime::new().map_err(|_| {
        eprintln!("Error: Failed to initialize runtime");
        1
    })?;

    let test_result = if let Some(code) = eval {
        match rt.eval(code, "<test-eval>") {
            Ok(_) => {
                flush_js_output();
                // `bao test -e` IS the test runner (argv[1] === 'test', the
                // node:test gate passes): drive the registered suites instead
                // of exiting right after registration — same execution path
                // as `bao test <file>`.
                let report = rt.run_registered_tests();
                flush_js_output();
                render_report(&report);
                if report.failed > 0 { Err(1) } else { Ok(()) }
            }
            Err(e) => {
                flush_js_output();
                eprintln!("FAIL: {}", e);
                Err(1)
            }
        }
    } else if files.is_empty() {
        let test_patterns = ["test", "tests", "__tests__"];
        let mut found = false;
        let mut total_passed: u32 = 0;
        let mut total_failed: u32 = 0;
        for dir in &test_patterns {
            if ::std::path::Path::new(dir).is_dir() {
                found = true;
                if let Ok(entries) = ::std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path
                            .extension()
                            .map(|e| e == "js" || e == "ts")
                            .unwrap_or(false)
                        {
                            let path_str = path.to_string_lossy().into_owned();
                            eprintln!("\n# {}", path_str);
                            run_one_test_file(
                                &mut rt,
                                &path_str,
                                &mut total_passed,
                                &mut total_failed,
                            );
                        }
                    }
                }
            }
        }
        if !found {
            eprintln!("bao test: no test files found (looked in test/, tests/, __tests__/)");
            return Err(1);
        }
        eprintln!(
            "\n# Summary: {} passed, {} failed",
            total_passed, total_failed
        );
        if total_failed > 0 { Err(1) } else { Ok(()) }
    } else {
        let mut total_passed: u32 = 0;
        let mut total_failed: u32 = 0;
        for file in files {
            eprintln!("\n# {}", file);
            run_one_test_file(&mut rt, file, &mut total_passed, &mut total_failed);
        }
        eprintln!(
            "\n# Summary: {} passed, {} failed",
            total_passed, total_failed
        );
        if total_failed > 0 { Err(1) } else { Ok(()) }
    };
    test_result
}

/// Render a single file's test report: ✓ for passes, ✗ + message + first
/// stack line for failures, then a per-file counters line.
fn render_report(report: &bun_runtime::bun_test::TestReport) {
    for name in &report.passes {
        eprintln!("✓ {}", name);
    }
    for f in &report.failures {
        eprintln!("✗ {}", f.name);
        if !f.message.is_empty() {
            eprintln!("  {}", f.message);
        }
        if !f.stack.is_empty() {
            let first_line = f.stack.lines().next().unwrap_or("");
            if !first_line.is_empty() {
                eprintln!("  at: {}", first_line);
            }
        }
    }
    eprintln!("  -> {} passed, {} failed", report.passed, report.failed);
}

fn run_browser(
    url: ::std::option::Option<String>,
    cdp_port: u16,
    headless: bool,
    stealth: bool,
) -> ::std::result::Result<(), i32> {
    let stealth_profile = if stealth {
        Some(StealthProfile::firefox_default())
    } else {
        None
    };
    let config = BrowserConfig {
        url,
        cdp_port,
        viewport_width: 1920,
        viewport_height: 1080,
        headless,
        stealth_profile,
        ..Default::default()
    };
    if let Err(e) = bao_browser::run_browser(config) {
        eprintln!("Error: {}", e);
        Err(1)
    } else {
        Ok(())
    }
}

/// Preferences activated by `--enable-experimental-web-platform-features`
/// (mirror of servoshell's EXPERIMENTAL_PREFS, ports/servoshell/prefs.rs —
/// keep in sync on vendor snapshot updates). wptrunner's servo product
/// passes the flag unconditionally, so the expansion is what puts any
/// pref-gated WPT domain (editing via `dom_exec_command_enabled`, …) on the
/// same footing as upstream servo's own test runs. Schema-mismatch names
/// are serde no-ops on the vendor `Preferences`.
const EXPERIMENTAL_PREFS: &[&str] = &[
    "dom_async_clipboard_enabled",
    "dom_exec_command_enabled",
    "dom_fontface_enabled",
    "dom_indexeddb_enabled",
    "dom_intersection_observer_enabled",
    "dom_navigator_protocol_handlers_enabled",
    "dom_notification_enabled",
    "dom_offscreen_canvas_enabled",
    "dom_permissions_enabled",
    "dom_sanitizer_enabled",
    "dom_storage_manager_api_enabled",
    "dom_webgl2_enabled",
    "dom_webgpu_enabled",
    "layout_css_alpha_color_function_enabled",
    "layout_css_attr_enabled",
    "layout_css_ellipse_corners_enabled",
    "layout_css_progress_function_enabled",
    "layout_columns_enabled",
    "layout_container_queries_enabled",
    "layout_variable_fonts_enabled",
];

/// WPT official-toolchain face (REQ-BRW-002): the servoshell-compatible
/// browser entry — apply pref plumbing, translate the wptrunner command line
/// into a `BrowserConfig`, and hand the process to the WebDriver run loop.
fn run_browser_entry(cli: &Cli) -> ::std::result::Result<(), i32> {
    // Pref plumbing: --prefs-file entries first, then --pref=K=V (the
    // command line wins; wptrunner's proxy prefs arrive via --pref). The
    // prefs file is optional upstream too (servoshell's find_wpt_prefs
    // tolerates a missing resources/wpt-prefs.json) — a missing file is a
    // logged no-op, a corrupt one fails the launch.
    let mut pref_overrides = Vec::new();
    if let Some(ref path) = cli.prefs_file {
        if ::std::path::Path::new(path).exists() {
            match bao_browser::webdriver_host::load_prefs_file(path) {
                Ok(mut from_file) => pref_overrides.append(&mut from_file),
                Err(error) => {
                    eprintln!("bao: {error}");
                    return Err(2);
                }
            }
        } else {
            eprintln!("bao: prefs file {path} not found (skipping, like upstream servoshell)");
        }
    }
    for pref in &cli.pref {
        match pref.split_once('=') {
            Some((key, value)) => pref_overrides.push((key.to_string(), value.to_string())),
            None => {
                eprintln!("bao: --pref expects K=V, got {pref:?}");
                return Err(2);
            }
        }
    }
    // `--enable-experimental-web-platform-features` (user ruling 2026-10-08
    // ruling A): the flag was previously accepted for entry dispatch but
    // never consumed — servoshell expands it to its EXPERIMENTAL_PREFS list
    // (ports/servoshell/prefs.rs), which is how servo's WPT runs activate
    // pref-gated domains (wptrunner passes the flag unconditionally). Mirror
    // that expansion onto the same override list; schema-mismatch names are
    // serde no-ops (the vendor Preferences has no deny_unknown_fields).
    if cli.enable_experimental_web_platform_features {
        for name in EXPERIMENTAL_PREFS {
            pref_overrides.push(((*name).to_string(), "true".to_string()));
        }
    }
    // Early fail-closed validation of the override values against the
    // Preferences schema (the durable application happens on the
    // ServoBuilder preferences inside BrowserRuntime::new — Servo::new's
    // prefs::set would wipe a purely global application).
    if let Err(error) = bao_browser::webdriver_host::apply_pref_overrides(&pref_overrides) {
        eprintln!("bao: {error}");
        return Err(2);
    }

    // --window-size WxH → viewport. A malformed size fails the launch
    // instead of silently running at the default (wptrunner sizes reftests
    // against this viewport).
    let (viewport_width, viewport_height) = match cli.window_size.as_deref() {
        None => (1920, 1080),
        Some(size) => match size.split_once('x') {
            Some((w, h)) => match (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
                (Ok(w), Ok(h)) if w >= 800 && h >= 600 => (w, h),
                _ => {
                    eprintln!("bao: --window-size expects WxH with W >= 800 and H >= 600, got {size:?}");
                    return Err(2);
                }
            },
            None => {
                eprintln!("bao: --window-size expects WxH, got {size:?}");
                return Err(2);
            }
        },
    };

    let config = BrowserConfig {
        url: cli.url.clone(),
        cdp_port: 9222,
        viewport_width,
        viewport_height,
        headless: true,
        stealth_profile: None,
        webdriver_port: cli.webdriver,
        ignore_certificate_errors: cli.ignore_certificate_errors,
        certificate_path: cli.certificate_path.clone(),
        // Durable pref surface (user ruling 2026-10-08 ruling A): rides the
        // ServoBuilder preferences so `Servo::new`'s prefs reset cannot wipe
        // it (see BrowserRuntime::new).
        pref_overrides,
    };
    if let Err(e) = bao_browser::run_browser(config) {
        eprintln!("Error: {}", e);
        Err(1)
    } else {
        Ok(())
    }
}
