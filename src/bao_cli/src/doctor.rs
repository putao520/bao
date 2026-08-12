// `bao doctor` — environment diagnostics for the Bao native stack.
//
// A programmable browser runtime built on Servo + SpiderMonkey has a heavy
// native toolchain (nightly Rust, clang/C++, mozjs compiled from source).
// `bao doctor` walks the environment and reports what's present/missing so a
// new contributor can see at a glance why their build fails — without having
// to understand the whole monorepo.

use std::process::Command;
use std::time::Duration;

/// One check result. `ok` is the binary pass/fail; `detail` carries version
/// strings or a fix hint.
struct Check {
    label: &'static str,
    ok: bool,
    detail: String,
}

pub fn run() -> Result<(), i32> {
    let version = env!("CARGO_PKG_VERSION");
    println!("Bao doctor v{}\n", version);

    let checks = vec![
        rustc_check(),
        cargo_check(),
        clang_check(),
        pkg_config_check(),
        display_check(),
        mozjs_artifact_check(),
        cdp_port_check(),
    ];

    let mut all_ok = true;
    let width = checks.iter().map(|c| c.label.len()).max().unwrap_or(8);
    for c in &checks {
        let mark = if c.ok { "✓" } else { "✗" };
        println!(
            "  {} {:<width$}  {}",
            mark,
            c.label,
            c.detail,
            width = width
        );
        if !c.ok {
            all_ok = false;
        }
    }

    println!();
    if all_ok {
        println!("All checks passed. Bao should build and run on this machine.");
        Ok(())
    } else {
        println!("Some checks failed — see the ✗ lines above for fix hints.");
        // Doctor itself is informational; never hard-fail the process so it can
        // be piped into a bug report.
        Ok(())
    }
}

/// Run `cmd --version` (or equivalent) and return the first stdout line,
/// trimmed. `None` if the binary is absent or the command fails.
fn version_line(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
}

fn rustc_check() -> Check {
    match version_line("rustc", &["--version"]) {
        Some(v) => {
            // Bao requires nightly. rustc prints e.g. "rustc 1.90.0-nightly (...)".
            let is_nightly = v.contains("nightly");
            Check {
                label: "Rust (rustc)",
                ok: is_nightly,
                detail: if is_nightly {
                    v
                } else {
                    format!("{}  ← Bao requires nightly; run: rustup default nightly", v)
                },
            }
        }
        None => Check {
            label: "Rust (rustc)",
            ok: false,
            detail: "not found — install via https://rustup.rs".into(),
        },
    }
}

fn cargo_check() -> Check {
    match version_line("cargo", &["--version"]) {
        Some(v) => Check {
            label: "Cargo",
            ok: true,
            detail: v,
        },
        None => Check {
            label: "Cargo",
            ok: false,
            detail: "not found — ships with rustup".into(),
        },
    }
}

fn clang_check() -> Check {
    // mozjs compiles SpiderMonkey (C++) from source; a C/C++ compiler is mandatory.
    for c in ["clang", "gcc", "cc"] {
        if let Some(v) = version_line(c, &["--version"]) {
            return Check {
                label: "C/C++ compiler",
                ok: true,
                detail: format!("{}: {}", c, first_token(&v)),
            };
        }
    }
    Check {
        label: "C/C++ compiler",
        ok: false,
        detail: "none of clang/gcc/cc found — mozjs needs a C++ compiler".into(),
    }
}

fn pkg_config_check() -> Check {
    match version_line("pkg-config", &["--version"]) {
        Some(v) => Check {
            label: "pkg-config",
            ok: true,
            detail: v,
        },
        None => Check {
            label: "pkg-config",
            ok: false,
            detail: "not found — install pkg-config (apt: pkg-config)".into(),
        },
    }
}

fn display_check() -> Check {
    // Servo/WebRender need a DISPLAY (headless via Xvfb is fine). No DISPLAY
    // is not fatal for a pure `bao run` script, but blocks browser rendering.
    let has_display = std::env::var_os("DISPLAY").is_some();
    Check {
        label: "DISPLAY (rendering)",
        ok: has_display,
        detail: if has_display {
            std::env::var("DISPLAY").unwrap_or_default()
        } else {
            "unset — browser rendering needs X (or Xvfb :99 for headless)".into()
        },
    }
}

fn mozjs_artifact_check() -> Check {
    // If target/ has a compiled libmozjs_sys rlib, the first build (the slow
    // part) is already done. This is just a hint, not a requirement.
    let found = walk_target_for_mozjs();
    Check {
        label: "SpiderMonkey (built)",
        ok: true, // informational only — absence is not a failure
        detail: if found {
            "libmozjs_sys artifact present (incremental build will be fast)".into()
        } else {
            "no cached artifact — first build will compile SpiderMonkey (slow)".into()
        },
    }
}

/// Scan target/ for any compiled mozjs artifact. Best-effort, non-recursive
/// beyond a shallow check — we only want a presence hint.
fn walk_target_for_mozjs() -> bool {
    let Ok(entries) = std::fs::read_dir("target") else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if dir_mentions_mozjs(&path.join("debug")) {
                return true;
            }
            if dir_mentions_mozjs(&path.join("release")) {
                return true;
            }
        }
    }
    false
}

fn dir_mentions_mozjs(dir: &std::path::Path) -> bool {
    // Look in deps/ and incremental/ shallowly for a mozjs-named file.
    for sub in ["deps", "incremental"] {
        let p = dir.join(sub);
        if let Ok(it) = std::fs::read_dir(&p) {
            for e in it.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.contains("mozjs") {
                    return true;
                }
            }
        }
    }
    false
}

fn cdp_port_check() -> Check {
    // If something answers on 9222, a Bao (or Chrome) CDP server is likely
    // already running. Connect-only, short timeout — never start a server here.
    let addr = "127.0.0.1:9222";
    match std::net::TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(300)) {
        Ok(_) => Check {
            label: "CDP :9222",
            ok: true,
            detail: "reachable — a CDP server is already listening".into(),
        },
        Err(_) => Check {
            label: "CDP :9222",
            ok: true, // not listening is the normal state, not a failure
            detail: "not listening (normal unless `bao browser` is running)".into(),
        },
    }
}

/// First whitespace-delimited token of a version string (drops the long
/// commit hash / target triple noise).
fn first_token(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}
