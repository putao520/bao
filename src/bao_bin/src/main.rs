fn main() {
    if let Err(code) = bao_cli::cli::run() {
        std::process::exit(code);
    }
}
