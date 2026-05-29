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
    };
    if let Err(code) = result {
        std::process::exit(code);
    }
}

fn run_eval(code: &str) -> ::std::result::Result<(), i32> {
    let mut rt = bao_runtime::BaoRuntime::new()
        .map_err(|_| { eprintln!("Error: Failed to initialize SpiderMonkey"); 1 })?;
    match rt.eval(code, "<eval>") {
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
    }
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

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {}", e);
            Err(1)
        }
    }
}

fn run_module_eval(code: &str) -> ::std::result::Result<(), i32> {
    let mut rt = bao_runtime::BaoRuntime::new()
        .map_err(|_| { eprintln!("Error: Failed to initialize SpiderMonkey"); 1 })?;
    match rt.eval_module(code, "<module>") {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {}", e);
            Err(1)
        }
    }
}
