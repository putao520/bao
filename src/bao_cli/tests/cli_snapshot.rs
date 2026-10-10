// @trace REQ-CLI-001 [test:W22B-CLI-SNAPSHOT] [level:integration]
// W22b (#17-E) CLI contract snapshot — real-binary subprocess golden tests.
//
// Design (W22 §2): hand-rolled comparator (no insta — the compare logic is
// ~20 lines), REAL binary subprocess capture (clap renders the real argv[0]
// basename — that IS the contract; `Cli::command().render_help()` would not
// exercise it). Normalization institutionalizes the 220d0fe9 platform
// lesson: argv[0] basename (`bao`/`bao.exe`) → `<BIN>`, CRLF → LF,
// `v<semver>` → `v<VER>`. After normalization golden comparison is
// byte-equal.
//
// What is NOT snapshotted: the runtime output faces (`bao run/build/test`
// executing a program, browser session output, compat REPORTS) — the compat
// reports embed HOST-derived facts (local Node.js version, builtin-module
// counts) and are non-deterministic across hosts by design. Those faces are
// covered by the exit-code matrix only. The snapshotted faces are the
// static ones: every `--help` surface (clap rendering) and the
// misuse/error stderr paths.
//
// Regeneration: `BAO_SNAPSHOT_REGEN=1 cargo nt -p bao_cli` rewrites the
// golden files (printing a per-file summary). Comparison mode is the
// default (CI runs it).

use std::process::{Command, Output};

const GOLDEN_DIR: &str = "tests/golden/cli";

/// Candidates from `.cargo/config.toml` [build] target-dir (both profiles,
/// both OS spellings). The nested `if let` guards of the original ladder
/// are no-op fallthroughs — early returns are equivalent.
fn push_config_target_dir_candidates(
    candidates: &mut Vec<std::path::PathBuf>,
    root: &std::path::Path,
) {
    // .cargo/config.toml [build] target-dir (single-compile-universe shape).
    let cfg = root.join(".cargo/config.toml");
    let Ok(text) = std::fs::read_to_string(&cfg) else {
        return;
    };
    let Some(line) = text
        .lines()
        .find(|l| l.trim_start().starts_with("target-dir"))
    else {
        return;
    };
    let Some(idx) = line.find('"') else {
        return;
    };
    let Some(dir) = line[idx + 1..].split('"').next() else {
        return;
    };
    for profile in ["debug", "release"] {
        for name in ["bao", "bao.exe"] {
            let candidate = std::path::Path::new(dir).join(profile).join(name);
            if candidate.is_file() {
                candidates.push(candidate);
            }
        }
    }
}

/// Locate the real `bao` binary (W8/p0-cluster find_bao_binary lineage:
/// explicit override → sibling-of-test-exe → config target-dir → manifest
/// target tree; both OS spellings probed everywhere).
fn find_bao_binary() -> std::path::PathBuf {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("BAO_TEST_BAO_BIN") {
        let p = std::path::PathBuf::from(p);
        if p.is_file() {
            candidates.push(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(profile_dir) = exe.parent().and_then(|d| d.parent()) {
            for name in ["bao", "bao.exe"] {
                let candidate = profile_dir.join(name);
                if candidate.is_file() {
                    candidates.push(candidate);
                }
            }
        }
    }
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent().and_then(|p| p.parent()).expect("workspace root");
    push_config_target_dir_candidates(&mut candidates, root);
    let target = root.join("target");
    for profile in ["debug", "release"] {
        for name in ["bao", "bao.exe"] {
            let candidate = target.join(profile).join(name);
            if candidate.is_file() {
                candidates.push(candidate);
            }
        }
    }

    // ── Freshness canary (battery-v7 stale-binary lesson) ──────────────────
    // A binary whose `compat --help` does not exit 0 predates the current
    // command set (e.g. a pre-W14 leftover in the resolved target dir).
    // Snapshot-comparing against it would fabricate a golden "drift" that is
    // really staleness — fail LOUD with the fix instead.
    let mut last: Option<(std::path::PathBuf, String)> = None;
    for candidate in candidates {
        let out = Command::new(&candidate)
            .args(["compat", "--help"])
            .output();
        match out {
            Ok(o) if o.status.code() == Some(0) && !o.stdout.is_empty() => return candidate,
            Ok(o) => {
                last = Some((
                    candidate.clone(),
                    format!("compat --help exited {:?} (stdout {}B)", o.status.code(), o.stdout.len()),
                ));
            }
            Err(e) => {
                last = Some((candidate.clone(), format!("spawn failed: {e}")));
            }
        }
    }
    let detail = last
        .map(|(p, why)| format!("last candidate {} : {why}", p.display()))
        .unwrap_or_else(|| "no candidate binary found".into());
    panic!(
        "stale or unusable `bao` binary — {detail}.\n\
         Rebuild the binary for THIS profile (cargo build -p bao_bin --profile test-ci --jobs 4) \
         or point BAO_TEST_BAO_BIN at a fresh one.\n\
         (The snapshot tests must never compare golden faces against a binary \
         that predates the current command set — that fabricates drift.)"
    );
}

/// Capture one invocation: (stdout, stderr, exit code).
fn run_bin(bin: &std::path::Path, args: &[&str]) -> Output {
    Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn {} failed: {e}", bin.display()))
}

/// Platform/diff-noise normalization (byte-equal AFTER this):
///   - CRLF → LF;
///   - argv[0] basename forms → `<BIN>` (usage lines, "Try `bao --help`.");
///   - `v<major.minor.patch…>` tokens → `v<VER>` (crate/inventory versions).
fn normalize(s: &str) -> String {
    let mut s = s.replace("\r\n", "\n");
    for name in ["bao.exe", "bao"] {
        s = s.replace(&format!("Usage: {name} "), "Usage: <BIN> ");
        s = s.replace(&format!("Usage: {name}\n"), "Usage: <BIN>\n");
        s = s.replace(&format!("Try `{name} "), "Try `<BIN> ");
        s = s.replace(&format!("`{name} run"), "`<BIN> run");
        s = s.replace(&format!("`{name} --help`"), "`<BIN> --help`");
        s = s.replace(&format!("`{name} <COMMAND>`"), "`<BIN> <COMMAND>`");
        // handler error-message prefix form (no backticks): "bao run: ..."
        for cmd in ["run", "test", "browser", "build", "install"] {
            s = s.replace(&format!("{name} {cmd}:"), &format!("<BIN> {cmd}:"));
        }
    }
    s = normalize_versions(&s);
    s
}

/// Replace `v<maj.min.patch>` tokens (optionally `v maj.min.patch-build`) with
/// `v<VER>` — hand-rolled scan, no regex dep.
fn normalize_versions(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if (b[i] == b'v' || b[i] == b'V')
            && i + 1 < b.len()
            && b[i + 1].is_ascii_digit()
        {
            // v<digits>.<digits>.<digits>[<tail>]
            let start = i;
            let mut j = i + 1;
            let mut dots = 0;
            let mut ok = true;
            while j < b.len() {
                let c = b[j];
                if c.is_ascii_digit() {
                    j += 1;
                } else if c == b'.' && dots < 2 {
                    dots += 1;
                    j += 1;
                    // a dot must be followed by a digit to stay in a version
                    if j >= b.len() || !b[j].is_ascii_digit() {
                        ok = false;
                        break;
                    }
                } else {
                    break;
                }
            }
            if ok && dots == 2 {
                out.push_str("v<VER>");
                i = j;
                continue;
            }
            let _ = start;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// The snapshotted faces: (golden name, args, stream).
fn snapshot_cases() -> Vec<(&'static str, Vec<&'static str>, &'static str)> {
    vec![
        // 12 help surfaces (clap render — static).
        ("help-top", vec!["--help"], "stdout"),
        ("help-h", vec!["-h"], "stdout"),
        ("help-run", vec!["run", "--help"], "stdout"),
        ("help-build", vec!["build", "--help"], "stdout"),
        ("help-test", vec!["test", "--help"], "stdout"),
        ("help-install", vec!["install", "--help"], "stdout"),
        ("help-browser", vec!["browser", "--help"], "stdout"),
        ("help-doctor", vec!["doctor", "--help"], "stdout"),
        ("help-compat", vec!["compat", "--help"], "stdout"),
        ("help-compat-node", vec!["compat", "node", "--help"], "stdout"),
        ("help-compat-bun", vec!["compat", "bun", "--help"], "stdout"),
        ("help-compat-cdp", vec!["compat", "cdp", "--help"], "stdout"),
        ("help-compat-web", vec!["compat", "web", "--help"], "stdout"),
        // Misuse / error stderr paths (static one-liners + clap errors).
        ("err-no-args", vec![], "stderr"),
        ("err-unknown-command", vec!["unknown-cmd"], "stderr"),
        ("err-run-no-input", vec!["run"], "stderr"),
        ("err-timeout-missing-value", vec!["--timeout"], "stderr"),
        ("err-timeout-zero", vec!["--timeout", "0"], "stderr"),
    ]
}

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_DIR).join(format!("{name}.txt"))
}

fn compare_or_regen(name: &str, normalized: &str) {
    let path = golden_path(name);
    let regen = std::env::var("BAO_SNAPSHOT_REGEN").as_deref() == Ok("1");
    if regen {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("create golden dir");
        }
        let existed = path.is_file();
        std::fs::write(&path, normalized).expect("write golden");
        println!(
            "[snapshot-regen] {} {} ({} bytes)",
            if existed { "REWROTE" } else { "CREATED" },
            path.display(),
            normalized.len()
        );
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read golden {} ({e}) — regenerate with BAO_SNAPSHOT_REGEN=1 cargo nt -p bao_cli",
            path.display()
        )
    });
    assert_eq!(
        normalized,
        expected,
        "snapshot drift for {name}: the CLI face changed without a golden update. \
         If the change is intended, regenerate with BAO_SNAPSHOT_REGEN=1 cargo nt -p bao_cli \
         and review the diff as a contract change."
    );
}

/// Golden lock over the 13 help surfaces + 5 error stderr faces.
#[test]
fn cli_snapshot_golden_lock() {
    let bin = find_bao_binary();
    for (name, args, stream) in snapshot_cases() {
        let out = run_bin(&bin, &args);
        let text = match stream {
            "stdout" => String::from_utf8_lossy(&out.stdout).into_owned(),
            _ => String::from_utf8_lossy(&out.stderr).into_owned(),
        };
        let normalized = normalize(&text);
        assert!(
            !normalized.trim().is_empty(),
            "{name}: captured empty {stream} — the face must render something"
        );
        compare_or_regen(name, &normalized);
    }
}

/// Deterministic exit-code matrix (design W22 §2 — table-driven, no golden):
/// each row = (args, expected exit code).
#[test]
fn cli_exit_code_matrix() {
    let bin = find_bao_binary();
    let matrix: &[(&[&str], i32)] = &[
        (&["--help"], 0),
        (&["-h"], 0),
        (&["run", "--help"], 0),
        (&["build", "--help"], 0),
        (&["test", "--help"], 0),
        (&["install", "--help"], 0),
        (&["browser", "--help"], 0),
        (&["doctor", "--help"], 0),
        (&["compat", "--help"], 0),
        // no command / unknown command / misuse
        (&[], 1),
        (&["unknown-cmd"], 1),
        (&["run"], 1),
        (&["--timeout"], 2),
        (&["--timeout", "0"], 2),
        // compat report domains run off the INVENTORY SSOTs — exit 0
        (&["compat"], 0),
        (&["compat", "node"], 0),
        (&["compat", "bun"], 0),
        (&["compat", "cdp"], 0),
        (&["compat", "web"], 0),
    ];
    for (args, expected) in matrix {
        let out = run_bin(&bin, args);
        let code = out.status.code().unwrap_or(-1);
        assert_eq!(
            code, *expected,
            "exit-code matrix drift: `bao {:?}` exited {code}, expected {expected}",
            args
        );
    }
}
