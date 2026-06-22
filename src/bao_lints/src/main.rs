//! `bao_lints` — BCE 防复发检测器 CLI 入口。
//!
//! 两个子命令:
//!
//! ```text
//! bao_lints --check <path> [--summary-only]
//!     扫描 <path> 下所有 `*.rs`,报 BCE-012 GC-unsafe Handle 构造违规。
//!     任一命中退出码 1,适合 CI 门禁。
//!
//! bao_lints spec-id <path> [--baseline <file>] [--summary-only]
//!     扫描 <path> 下所有 `*.html`,报 REQ-SPEC-001 SPEC API 元素 id 违规
//!     (method-path / path-only / 缺失 id)。`--baseline` 传入已知违规清单
//!     (每行一个 id,`#` 起始为注释)用于追踪历史技术债务而不阻断 CI。
//! ```
//!
//! 检测器本体见 `detector::scan_source`(BCE-012) 与 `spec_id::scan_path`
//! (REQ-SPEC-001)。本入口只做参数解析与退出码映射。

use bao_lints::{detector, spec_id};

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
        Command::Bce012Check { path, summary_only } => run_bce012_check(&path, summary_only),
        Command::SpecIdCheck { path, baseline, summary_only } => {
            run_spec_id_check(&path, baseline.as_deref(), summary_only)
        }
    }
}

enum Command {
    Bce012Check { path: PathBuf, summary_only: bool },
    SpecIdCheck {
        path: PathBuf,
        baseline: Option<PathBuf>,
        summary_only: bool,
    },
}

struct Parsed {
    command: Command,
}

fn usage() -> &'static str {
    "usage:\n\
     \n\
       bao_lints --check <path> [--summary-only]\n\
           Scan <path> (file or dir) for BCE-012 GC-unsafe Handle construction.\n\
           Recursively walks directories, scanning every `*.rs` file.\n\
           Exits non-zero on any finding — suitable for CI gating.\n\
     \n\
       bao_lints spec-id <path> [--baseline <file>] [--summary-only]\n\
           Scan <path> (file or dir) for REQ-SPEC-001 SPEC API element id\n\
           violations (method-path / path-only / missing id) in `*.html`.\n\
           --baseline suppresses ids listed in <file> (one per line, # = comment).\n\
           Exits non-zero on any non-baselined finding."
}

fn parse_args(args: &[String]) -> Result<Parsed, String> {
    if args.len() < 2 {
        return Err("missing arguments".to_string());
    }

    // 子命令分发:第一个非程序名参数若是 `spec-id` 走 SPEC id 分支;
    // 否则按旧契约走 BCE-012 `--check` 分支(保持向后兼容)。
    if args[1] == "spec-id" {
        return parse_spec_id_args(&args[2..]);
    }
    parse_bce012_args(&args[1..])
}

fn parse_bce012_args(args: &[String]) -> Result<Parsed, String> {
    let mut check: Option<PathBuf> = None;
    let mut summary_only = false;
    let mut i = 0;
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
        command: Command::Bce012Check { path, summary_only },
    })
}

fn parse_spec_id_args(args: &[String]) -> Result<Parsed, String> {
    let mut path: Option<PathBuf> = None;
    let mut baseline: Option<PathBuf> = None;
    let mut summary_only = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--baseline" => {
                i += 1;
                if i >= args.len() {
                    return Err("--baseline requires a path argument".to_string());
                }
                baseline = Some(PathBuf::from(&args[i]));
            }
            "--summary-only" => summary_only = true,
            "-h" | "--help" => return Err(usage().to_string()),
            other if !other.starts_with('-') && path.is_none() => {
                path = Some(PathBuf::from(other));
            }
            other => return Err(format!("unknown argument: {}", other)),
        }
        i += 1;
    }
    let path = path.ok_or_else(|| "missing <path> positional argument".to_string())?;
    Ok(Parsed {
        command: Command::SpecIdCheck {
            path,
            baseline,
            summary_only,
        },
    })
}

// ─── BCE-012 GC-unsafe Handle scan ──────────────────────────────────────────

fn run_bce012_check(path: &Path, summary_only: bool) -> ExitCode {
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

// ─── REQ-SPEC-001 SPEC API element id scan ──────────────────────────────────

fn run_spec_id_check(
    path: &Path,
    baseline: Option<&Path>,
    summary_only: bool,
) -> ExitCode {
    let result = match spec_id::scan_path(path, baseline) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bao_lints spec-id: scan failed: {}", e);
            return ExitCode::from(2);
        }
    };
    if !summary_only {
        for f in &result.findings {
            println!("{}", f.render());
        }
    }
    eprintln!(
        "bao_lints spec-id: scanned {} html file(s), {} finding(s)",
        result.files_scanned,
        result.findings.len()
    );
    // 按 reason 分桶计数,方便 CI 日志快速定位退化形态。
    if !result.findings.is_empty() {
        let mut method_path = 0usize;
        let mut path_only = 0usize;
        let mut missing = 0usize;
        let mut other = 0usize;
        for f in &result.findings {
            match f.reason {
                spec_id::Reason::MethodPath => method_path += 1,
                spec_id::Reason::PathOnly => path_only += 1,
                spec_id::Reason::MissingId => missing += 1,
                spec_id::Reason::Other => other += 1,
            }
        }
        eprintln!(
            "  breakdown: method-path={}, path-only={}, missing-id={}, other={}",
            method_path, path_only, missing, other
        );
    }

    if baseline.is_some() {
        eprintln!(
            "baseline: {} listed, {} matched, {} phantom",
            result.baseline_total, result.baseline_matched, result.baseline_unmatched
        );
    }

    if result.findings.is_empty() {
        ExitCode::from(0)
    } else {
        ExitCode::from(1)
    }
}
