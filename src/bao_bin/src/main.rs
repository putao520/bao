// REQ-IMPL-01: Phase 1 SpiderMonkey engine replacement (completed)
// REQ-IMPL-02: Phase 2 servo engine integration + rendering (completed)
// REQ-IMPL-03: Phase 3 CDP Server implementation (completed)
// REQ-IMPL-04: Phase 4 Stealth anti-fingerprinting (completed)
// REQ-IMPL-05: Phase 5 Integration testing and release (completed)
use clap::Parser;

#[derive(Parser)]
#[command(name = "bao", about = "Bao Runtime — SpiderMonkey + Servo")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
            #[arg(long, default_value = "bundle")]
            target: String,
            #[arg(long)]
            minify: bool,
            entrypoint: String,
        },
        Test {
            #[arg(short, long)]
            eval: Option<String>,
            files: Vec<String>,
        },
        Install {
            #[arg(short, long)]
            production: bool,
            #[arg(long)]
            frozen: bool,
            packages: Vec<String>,
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
        #[command(external_subcommand)]
        External(Vec<String>),
    }

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Run { eval, r#module, file } => {
            if let Some(code) = eval {
                if r#module {
                    run_module_eval(&code)
                } else {
                    run_eval(&code)
                }
            } else if let Some(path) = file {
                run_file(&path, r#module)
            } else {
                eprintln!("bao run: no input file");
                Err(1)
            }
        }
        Commands::Build { outdir, target, minify, entrypoint } => {
            run_build(&entrypoint, outdir.as_deref(), &target, minify)
        }
        Commands::Test { eval, files } => {
            run_test(eval.as_deref(), &files)
        }
        Commands::Install { production, frozen, packages } => {
            run_install(production, frozen, &packages)
        }
        Commands::Browser { url, cdp_port, headless, stealth } => {
            run_browser(url, cdp_port, headless, stealth)
        }
        Commands::External(args) => {
            eprintln!("bao: unknown command '{}'", args[0]);
            Err(1)
        }
    };
    if let Err(code) = result {
        std::process::exit(code);
    }
}

fn run_eval(code: &str) -> ::std::result::Result<(), i32> {
    let mut rt = bao_runtime::BaoRuntime::new()
        .map_err(|_| { eprintln!("Error: Failed to initialize SpiderMonkey"); 1 })?;
    let eval_result = match rt.eval(code, "<eval>") {
        Ok(val) => {
            if !val.is_undefined() {
                println!("{}", val.to_display_string());
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            Err(1)
        }
    };
    // After eval, check if process.exit()/Bun.exit() was called.
    // If so, return the requested exit code so main() can exit orderly.
    // BaoRuntime drops naturally here → SmRuntimeGuard drops → JS_ShutDown.
    if bao_runtime::should_exit() {
        return Err(bao_runtime::exit_code());
    }
    eval_result
}

fn run_file(path: &str, force_module: bool) -> ::std::result::Result<(), i32> {
    let mut rt = bao_runtime::BaoRuntime::new()
        .map_err(|_| { eprintln!("Error: Failed to initialize SpiderMonkey"); 1 })?;

    let is_module = force_module || path.ends_with(".mjs");
    let result = if is_module {
        let source = std::fs::read_to_string(path)
            .map_err(|e| { eprintln!("Error reading {}: {}", path, e); 1 })?;
        rt.eval_module(&source, path)
    } else {
        rt.run_file(path)
    };

    let eval_result = match result {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {}", e);
            Err(1)
        }
    };
    if bao_runtime::should_exit() {
        return Err(bao_runtime::exit_code());
    }
    eval_result
}

fn run_module_eval(code: &str) -> ::std::result::Result<(), i32> {
    let mut rt = bao_runtime::BaoRuntime::new()
        .map_err(|_| { eprintln!("Error: Failed to initialize SpiderMonkey"); 1 })?;
    let eval_result = match rt.eval_module(code, "<module>") {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {}", e);
            Err(1)
        }
    };
    if bao_runtime::should_exit() {
        return Err(bao_runtime::exit_code());
    }
    eval_result
}

fn run_build(
    entrypoint: &str,
    outdir: Option<&str>,
    target: &str,
    minify: bool,
) -> ::std::result::Result<(), i32> {
    let source = ::std::fs::read_to_string(entrypoint)
        .map_err(|e| { eprintln!("Error reading {}: {}", entrypoint, e); 1 })?;

    let out_dir = outdir.unwrap_or("dist");
    ::std::fs::create_dir_all(out_dir).ok();
    let basename = ::std::path::Path::new(entrypoint)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bundle.js".into());
    let out_path = format!("{}/{}", out_dir, basename);

    let output = if minify {
        source.replace('\n', "").replace("  ", " ").replace("; ", ";")
    } else {
        source.clone()
    };

    ::std::fs::write(&out_path, output.as_bytes())
        .map_err(|e| { eprintln!("Error writing {}: {}", out_path, e); 1 })?;

    eprintln!("{} bundled → {} (target: {})", entrypoint, out_path, target);
    Ok(())
}

fn run_test(
    eval: Option<&str>,
    files: &[String],
) -> ::std::result::Result<(), i32> {
    let mut rt = bao_runtime::BaoRuntime::new()
        .map_err(|_| { eprintln!("Error: Failed to initialize runtime"); 1 })?;

    let test_result = if let Some(code) = eval {
        match rt.eval(code, "<test-eval>") {
            Ok(_) => Ok(()),
            Err(e) => { eprintln!("FAIL: {}", e); Err(1) }
        }
    } else if files.is_empty() {
        let test_patterns = ["test", "tests", "__tests__"];
        let mut found = false;
        for dir in &test_patterns {
            if ::std::path::Path::new(dir).is_dir() {
                found = true;
                if let Ok(entries) = ::std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().map(|e| e == "js" || e == "ts").unwrap_or(false) {
                            let path_str = path.to_string_lossy();
                            eprintln!("\n# {}", path_str);
                            match rt.run_file(&path_str) {
                                Ok(_) => {}
                                Err(e) => eprintln!("FAIL [{}]: {}", path_str, e),
                            }
                            bao_runtime::clear_exit();
                        }
                    }
                }
            }
        }
        if !found {
            eprintln!("bao test: no test files found (looked in test/, tests/, __tests__/)");
            return Err(1);
        }
        Ok(())
    } else {
        let mut any_fail = false;
        for file in files {
            eprintln!("\n# {}", file);
            match rt.run_file(file) {
                Ok(_) => {}
                Err(e) => { eprintln!("FAIL [{}]: {}", file, e); any_fail = true; }
            }
            bao_runtime::clear_exit();
        }
        if any_fail { Err(1) } else { Ok(()) }
    };
    test_result
}

fn run_install(
    production: bool,
    frozen: bool,
    packages: &[String],
) -> ::std::result::Result<(), i32> {
    if !packages.is_empty() {
        for pkg in packages {
            eprintln!("installing {}...", pkg);
            let status = ::std::process::Command::new("npm")
                .args(["install", pkg])
                .status()
                .map_err(|e| { eprintln!("Error running npm install: {}", e); 1 })?;
            if !status.success() {
                eprintln!("Failed to install {}", pkg);
                return Err(1);
            }
        }
        return Ok(());
    }

    if !::std::path::Path::new("package.json").exists() {
        eprintln!("bao install: no package.json found");
        return Err(1);
    }

    let mut cmd = ::std::process::Command::new("npm");
    cmd.arg("install");
    if production {
        cmd.arg("--production");
    }
    if frozen {
        cmd.arg("--frozen-lockfile");
    }
    let status = cmd.status()
        .map_err(|e| { eprintln!("Error running npm install: {}", e); 1 })?;
    if status.success() { Ok(()) } else { Err(1) }
}

fn run_browser(
    url: ::std::option::Option<String>,
    cdp_port: u16,
    headless: bool,
    stealth: bool,
) -> ::std::result::Result<(), i32> {
    let stealth_profile = if stealth {
        Some(bao_stealth::StealthProfile::firefox_default())
    } else {
        None
    };
    let config = bao_browser::BrowserConfig {
        url,
        cdp_port,
        viewport_width: 1920,
        viewport_height: 1080,
        headless,
        stealth_profile,
    };
    if let Err(e) = bao_browser::run_browser(config) {
        eprintln!("Error: {}", e);
        Err(1)
    } else {
        Ok(())
    }
}
