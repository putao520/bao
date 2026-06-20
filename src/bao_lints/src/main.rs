//! `bao_lints` — format-immune AST-based BCE-012 detector.
//!
//! Scans a path (file or directory) for `Handle::<T> { ..., ptr: &... }`
//! struct literals that construct GC-unsafe Handles, matching the
//! BCE-20260619-012 pattern documented in `src/BUG-KNOWLEDGE.md`.
//!
//! Detection is AST-based (via `syn`) — immune to rustfmt line-wrapping and
//! field-order shuffling, the failure modes that defeated prior regex/grep
//! detectors.
//!
//! CLI:
//! ```text
//! bao_lints --check <path>      # scan path, exit non-zero on any finding
//! bao_lints --check <path> --summary-only   # only print counts
//! ```

use bao_lints::detector;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use walkdir::WalkDir;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let parsed = match parse_args(&args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("{}", msg);
            eprintln!();
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };

    match parsed.command {
        Command::Check { path, summary_only } => run_check(&path, summary_only),
    }
}

enum Command {
    Check { path: PathBuf, summary_only: bool },
}

struct Parsed {
    command: Command,
}

fn usage() -> &'static str {
    "usage: bao_lints --check <path> [--summary-only]\n\
     \n\
     Scans <path> (file or directory) for BCE-012 GC-unsafe Handle construction\n\
     patterns. Recursively walks directories, scanning every `*.rs` file.\n\
     Exits non-zero when any finding is reported — suitable for CI gating."
}

fn parse_args(args: &[String]) -> Result<Parsed, String> {
    if args.len() < 2 {
        return Err("missing arguments".to_string());
    }

    let mut check: Option<PathBuf> = None;
    let mut summary_only = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--check" => {
                i += 1;
                if i >= args.len() {
                    return Err("--check requires a path argument".to_string());
                }
                check = Some(PathBuf::from(&args[i]));
            }
            "--summary-only" => summary_only = true,
            "-h" | "--help" => return Err(usage().to_string()),
            other => return Err(format!("unknown argument: {}", other)),
        }
        i += 1;
    }

    let path = check.ok_or_else(|| "missing --check <path>".to_string())?;
    Ok(Parsed {
        command: Command::Check { path, summary_only },
    })
}

fn run_check(path: &Path, summary_only: bool) -> ExitCode {
    let files = collect_rust_files(path);
    let mut total = 0usize;
    let mut files_scanned = 0usize;
    let mut unparseable = 0usize;

    for file in &files {
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("bao_lints: cannot read {}: {}", file.display(), e);
                continue;
            }
        };
        files_scanned += 1;
        let findings = detector::scan_source(file, &src);
        if findings.iter().any(|f| f.line == 0) {
            unparseable += 1;
        }
        if !summary_only {
            for f in &findings {
                println!("{}", f.render());
            }
        }
        total += findings.iter().filter(|f| f.line != 0).count();
    }

    eprintln!(
        "bao_lints: scanned {} file(s), {} finding(s), {} unparseable",
        files_scanned, total, unparseable
    );

    if total > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}

fn collect_rust_files(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path.to_path_buf());
        }
        return out;
    }
    if !path.is_dir() {
        return out;
    }
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
    {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
    out
}
