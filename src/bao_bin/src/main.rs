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
        file: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Run { eval, file } => {
            if let Some(code) = eval {
                run_eval(&code)
            } else if let Some(path) = file {
                run_file(&path)
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
    let mut ctx = bao_engine::context::JsContext::new()
        .map_err(|_| { eprintln!("Error: Failed to initialize SpiderMonkey"); 1 })?;
    match ctx.eval(code, "<eval>") {
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

fn run_file(path: &str) -> ::std::result::Result<(), i32> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| { eprintln!("Error reading {}: {}", path, e); 1 })?;
    let mut ctx = bao_engine::context::JsContext::new()
        .map_err(|_| { eprintln!("Error: Failed to initialize SpiderMonkey"); 1 })?;
    match ctx.eval(&source, path) {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {}", e);
            Err(1)
        }
    }
}
