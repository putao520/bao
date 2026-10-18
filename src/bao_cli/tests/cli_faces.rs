// @trace REQ-CLI-001 [test:C9-CLI-FACES] [level:integration]
// C9 (coverage-convergence wave) — CLI handler-face tests over the REAL binary.
//
// Companion to `cli_snapshot.rs` (golden lock over the static clap faces) and
// `cli_dispatch.rs` (compile-time dispatch-skeleton assertions). This file
// exercises the handler bodies themselves — the `run()` dispatch arms and the
// `doctor` check functions — through real subprocess invocations, because
// `Cli::parse()` reads `std::env::args()` and exits in-process on misuse, so
// the handlers are only reachable via a fresh `bao` process.
//
// Coverage role: cli_snapshot spawns the binary but only pins its *output
// faces*; the per-arm handler lines (run_eval / run_file / run_build /
// run_test / doctor checks / browser-entry validation) had zero line coverage
// because the spawned binary is not instrumented (llvm-cov nextest mode builds
// test targets only). Those faces are asserted here structurally (exit code +
// stderr/stderr markers + file side effects), and the bin-mode measurement
// pipeline (scripts/coverage-cli-bin-mode.sh) makes the spawned process
// contribute profraw so the same runs count toward line coverage.
//
// Assertion discipline: every expectation below was pinned against the real
// binary's observed behavior (exit codes, exact error strings, fixture
// semantics from bao_runtime::run_test_file). Doctor output is asserted via
// structural invariants (check-census + terminator consistency) rather than
// host-specific ✓/✗ patterns — doctor is informational and host-dependent by
// design; the only hard ✗ requirement is the DISPLAY line under a stripped
// spawn environment.

use std::process::{Command, Output, Stdio};

// ═══════════════════════════════════════════════════════════════════════
// Spawn helpers (display pinning; binary resolution shared in common/mod.rs)
// ═══════════════════════════════════════════════════════════════════════

// BCE-20261018-STALEBIN: binary resolution + behavior canary are
// single-point in tests/common/mod.rs (nonce argv-reflection probe
// included); this file's retired `--version`-only canary accepted a
// mtime-fresh, behaviorally wrong stale artifact in the 2026-10-18 battery.
mod common;

use common::find_bao_binary;

/// Spawn descriptor: args + cwd override + env vars to add + extra vars to
/// strip (DISPLAY/WAYLAND_DISPLAY are always stripped unless re-added via
/// `envs`, mirroring cli_snapshot's display pinning — the golden faces were
/// generated display-less and servo/GL X-connection attempts under Xvfb leak
/// noise into stderr).
struct Spawn<'a> {
    args: Vec<&'a str>,
    cwd: Option<std::path::PathBuf>,
    envs: Vec<(&'a str, &'a str)>,
    keep_display: bool,
}

impl<'a> Spawn<'a> {
    fn args(args: Vec<&'a str>) -> Self {
        Self { args, cwd: None, envs: Vec::new(), keep_display: false }
    }
    fn cwd(mut self, dir: &std::path::Path) -> Self {
        self.cwd = Some(dir.to_path_buf());
        self
    }
    fn env(mut self, key: &'a str, value: &'a str) -> Self {
        self.envs.push((key, value));
        self
    }
}

fn run_spawn(bin: &std::path::Path, spawn: &Spawn<'_>) -> Output {
    let mut cmd = Command::new(bin);
    if !spawn.keep_display {
        for var in ["DISPLAY", "WAYLAND_DISPLAY"] {
            cmd.env_remove(var);
        }
    }
    for (k, v) in &spawn.envs {
        cmd.env(k, v);
    }
    cmd.args(&spawn.args);
    if let Some(dir) = &spawn.cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("spawn {} failed: {e}", bin.display()))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

/// Run with the ambient PATH visible (doctor check faces probe the ambient
/// toolchain; only the display vars are pinned).
fn run_spawn_with_ambient_path(bin: &std::path::Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(bin);
    for var in ["DISPLAY", "WAYLAND_DISPLAY"] {
        cmd.env_remove(var);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("spawn {} failed: {e}", bin.display()))
}

/// Unique per-test scratch dir (fixtures + cwd isolation). Not cleaned up:
/// temp pollution is bounded and lets post-mortem inspection of failures.
fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bao-cli-faces-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

// ═══════════════════════════════════════════════════════════════════════
// §1 Top-level faces: version / help surface / no-command
// ═══════════════════════════════════════════════════════════════════════

/// node/bun parity version face (REQ-CLI-001): bare version string, no name
/// prefix, exit 0 — for BOTH the long and short flag spellings.
#[test]
fn version_face_prints_bare_version() {
    let bin = find_bao_binary();
    for args in [vec!["--version"], vec!["-v"]] {
        let out = run_spawn(&bin, &Spawn::args(args.clone()));
        assert_eq!(exit_code(&out), 0, "`{:?}` must exit 0", args);
        assert_eq!(
            stdout(&out).trim(),
            env!("CARGO_PKG_VERSION"),
            "`{:?}` must print the bare crate version (node/bun parity face)",
            args
        );
        assert!(
            !stdout(&out).contains("bao "),
            "version face must not carry a name prefix"
        );
    }
}

/// Every `--help` surface renders non-empty and exits 0 (all 7 subcommands +
/// compat's 4 domain subcommands + top-level long/short spellings).
#[test]
fn help_surface_all_subcommands_exit_zero() {
    let bin = find_bao_binary();
    let surfaces: &[&[&str]] = &[
        &["--help"],
        &["-h"],
        &["run", "--help"],
        &["build", "--help"],
        &["test", "--help"],
        &["install", "--help"],
        &["browser", "--help"],
        &["doctor", "--help"],
        &["compat", "--help"],
        &["compat", "node", "--help"],
        &["compat", "bun", "--help"],
        &["compat", "cdp", "--help"],
        &["compat", "web", "--help"],
    ];
    for args in surfaces {
        let out = run_spawn(&bin, &Spawn::args(args.to_vec()));
        assert_eq!(exit_code(&out), 0, "`{:?}` must exit 0", args);
        assert!(!stdout(&out).trim().is_empty(), "`{:?}` must render help", args);
    }
}

/// `bao` with no command → the handler-level error arm (not clap's): exit 1
/// with the "Try `bao --help`" pointer on stderr.
#[test]
fn no_command_gives_handler_error() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec![]));
    assert_eq!(exit_code(&out), 1, "no command must exit 1");
    let err = stderr(&out);
    assert!(
        err.contains("no command given") && err.contains("--help"),
        "no-command stderr must carry the handler message + help pointer, got: {err}"
    );
}

/// Unknown subcommand → clap's unrecognized-subcommand error, exit 1.
#[test]
fn unknown_command_is_rejected() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["definitely-not-a-command"]));
    assert_eq!(exit_code(&out), 1, "unknown command must exit 1");
    assert!(
        stderr(&out).contains("unrecognized subcommand"),
        "clap must report the unknown subcommand, got: {}",
        stderr(&out)
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §2 --timeout plumbing (fail-closed on non-script entries + value parser)
// ═══════════════════════════════════════════════════════════════════════

/// `--timeout` on non-script entries is rejected fail-closed (exit 2), never
/// silently ignored — both with and without a subcommand present.
#[test]
fn timeout_rejected_on_non_script_entries() {
    let bin = find_bao_binary();
    for args in [vec!["--timeout", "5000"], vec!["doctor", "--timeout", "5000"]] {
        let out = run_spawn(&bin, &Spawn::args(args.clone()));
        assert_eq!(exit_code(&out), 2, "`{:?}` must exit 2", args);
        assert!(
            stderr(&out).contains("--timeout applies only to script execution"),
            "`{:?}` stderr must carry the fail-closed message, got: {}",
            args,
            stderr(&out)
        );
    }
}

/// `--timeout` value parser: non-numeric and zero are clap usage errors
/// (exit 2) — an instant deadline is always a flag mistake.
#[test]
fn timeout_value_parser_rejects_non_positive() {
    let bin = find_bao_binary();
    for args in [vec!["--timeout", "abc"], vec!["--timeout", "0"]] {
        let out = run_spawn(&bin, &Spawn::args(args.clone()));
        assert_eq!(exit_code(&out), 2, "`{:?}` must exit 2", args);
        assert!(
            stderr(&out).contains("--timeout expects a positive number"),
            "`{:?}` stderr must carry the parser message, got: {}",
            args,
            stderr(&out)
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §3 Script execution faces: -e / run file / module / exit-code machinery
// ═══════════════════════════════════════════════════════════════════════

/// Top-level `-e` evaluates and prints a defined result; `--eval` long form
/// behaves identically.
#[test]
fn eval_prints_defined_result() {
    let bin = find_bao_binary();
    for args in [vec!["-e", "1+1"], vec!["--eval", "1+1"]] {
        let out = run_spawn(&bin, &Spawn::args(args.clone()));
        assert_eq!(exit_code(&out), 0, "`{:?}` must exit 0", args);
        assert_eq!(stdout(&out).trim(), "2", "`{:?}` must print the result", args);
    }
}

/// An undefined evaluation result prints nothing (no stray "undefined").
#[test]
fn eval_undefined_result_prints_nothing() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["-e", "undefined;"]));
    assert_eq!(exit_code(&out), 0);
    assert!(stdout(&out).trim().is_empty(), "undefined result must not print, got: {}", stdout(&out));
}

/// A syntax error in `-e` exits 1 with the engine error on stderr.
#[test]
fn eval_syntax_error_exits_1() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["-e", "syntax (( error"]));
    assert_eq!(exit_code(&out), 1, "syntax error must exit 1");
    assert!(
        stderr(&out).starts_with("Error:"),
        "stderr must carry the engine error, got: {}",
        stderr(&out)
    );
}

/// Engine-native deadline control: an infinite loop under `--timeout` exits
/// 124 (GNU timeout(1) convention; SM-EVOLUTION #24 S1 CLI wiring).
#[test]
fn eval_deadline_termination_exits_124() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["-e", "while(true){}", "--timeout", "600"]));
    assert_eq!(
        exit_code(&out),
        124,
        "deadline termination must surface exit 124, got stderr: {}",
        stderr(&out)
    );
}

/// Orderly-exit machinery: `process.exitCode` steering (exit 3) and an
/// explicit `process.exit(7)` are both honoured.
#[test]
fn eval_exit_code_machinery_is_honoured() {
    let bin = find_bao_binary();
    let steered = run_spawn(&bin, &Spawn::args(vec!["-e", "process.exitCode = 3;"]));
    assert_eq!(exit_code(&steered), 3, "exitCode steering must surface exit 3");
    let explicit = run_spawn(&bin, &Spawn::args(vec!["-e", "process.exit(7);"]));
    assert_eq!(exit_code(&explicit), 7, "process.exit(7) must surface exit 7");
}

/// `bao run <file>` executes a plain script file.
#[test]
fn run_file_executes_plain_script() {
    let bin = find_bao_binary();
    let dir = scratch_dir("run-file");
    std::fs::write(dir.join("ok.js"), "console.log('hello-file');\n").expect("write fixture");
    let out = run_spawn(
        &bin,
        &Spawn::args(vec!["run", "ok.js"]).cwd(&dir),
    );
    assert_eq!(exit_code(&out), 0);
    assert_eq!(stdout(&out).trim(), "hello-file");
}

/// `bao run <file.mjs>` takes the module path (is_module dispatch on the
/// extension).
#[test]
fn run_file_mjs_takes_module_path() {
    let bin = find_bao_binary();
    let dir = scratch_dir("run-mjs");
    std::fs::write(dir.join("ok.mjs"), "export const x = 41 + 1;\nconsole.log('mjs:', x);\n")
        .expect("write fixture");
    let out = run_spawn(&bin, &Spawn::args(vec!["run", "ok.mjs"]).cwd(&dir));
    assert_eq!(exit_code(&out), 0);
    assert!(stdout(&out).contains("mjs: 42"), "module output missing, got: {}", stdout(&out));
}

/// A missing file fails with the engine read error, exit 1.
#[test]
fn run_file_missing_exits_1() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["run", "definitely-missing.js"]));
    assert_eq!(exit_code(&out), 1);
    assert!(
        stderr(&out).contains("Error reading definitely-missing.js"),
        "stderr must name the unreadable file, got: {}",
        stderr(&out)
    );
}

/// `run --module` forces the module path for a .js file; the read failure arm
/// of that path is distinct from the plain-script read failure.
#[test]
fn run_module_flag_forces_module_read_path() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["run", "--module", "definitely-missing.js"]));
    assert_eq!(exit_code(&out), 1);
    assert!(
        stderr(&out).contains("Error reading definitely-missing.js"),
        "forced-module read failure must be reported, got: {}",
        stderr(&out)
    );
}

/// `bao run -m -e` runs module eval (Ok arm), and a module syntax error hits
/// the module-path Err arm with the module filename in the error.
#[test]
fn run_module_eval_ok_and_error_arms() {
    let bin = find_bao_binary();
    let ok = run_spawn(&bin, &Spawn::args(vec!["run", "-m", "-e", "const y = 5; console.log('mod:', y);"]));
    assert_eq!(exit_code(&ok), 0);
    assert!(stdout(&ok).contains("mod: 5"));
    let bad = run_spawn(&bin, &Spawn::args(vec!["run", "-m", "-e", "syntax (( bad"]));
    assert_eq!(exit_code(&bad), 1);
    assert!(
        stderr(&bad).contains("<module>"),
        "module-path error must name <module>, got: {}",
        stderr(&bad)
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §4 build faces
// ═══════════════════════════════════════════════════════════════════════

/// `bao build` bundles the entry into outdir and reports the face; the
/// `--sourcemap` flag surfaces the Phase-1 no-sourcemap warning.
#[test]
fn build_writes_bundle_and_warns_on_sourcemap() {
    let bin = find_bao_binary();
    let dir = scratch_dir("build-ok");
    std::fs::write(dir.join("entry.js"), "const v = 1;\nconsole.log(v);\n").expect("write fixture");
    let outdir = dir.join("dist").to_string_lossy().into_owned();
    let out = run_spawn(
        &bin,
        &Spawn::args(vec![
            "build", "--outdir", &outdir, "--sourcemap", "entry.js",
        ])
        .cwd(&dir),
    );
    assert_eq!(exit_code(&out), 0, "build must exit 0, stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("bundled →"),
        "build must print the bundle face, got: {}",
        stderr(&out)
    );
    let written =
        std::fs::read_to_string(dir.join("dist").join("entry.js")).expect("bundle written");
    assert!(written.contains("console.log"), "bundle must contain the bundled code");
    assert!(
        stderr(&out).contains("no sourcemap generated"),
        "--sourcemap without generated sourcemap must warn, got: {}",
        stderr(&out)
    );
}

/// A missing entrypoint fails the bundle with exit 1.
#[test]
fn build_missing_entry_exits_1() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["build", "definitely-missing-entry.js"]));
    assert_eq!(exit_code(&out), 1);
    assert!(
        stderr(&out).contains("Error reading definitely-missing-entry.js"),
        "stderr must name the unreadable entry, got: {}",
        stderr(&out)
    );
}

/// Unknown --target/--format values fall back with warnings (parse_target /
/// parse_format default arms) and the build still succeeds.
#[test]
fn build_unknown_target_and_format_warn_and_default() {
    let bin = find_bao_binary();
    let dir = scratch_dir("build-warn");
    std::fs::write(dir.join("entry.js"), "console.log(1);\n").expect("write fixture");
    let outdir = dir.join("dist").to_string_lossy().into_owned();
    let out = run_spawn(
        &bin,
        &Spawn::args(vec![
            "build",
            "--outdir",
            &outdir,
            "--target",
            "nonsense-target",
            "--format",
            "nonsense-format",
            "entry.js",
        ])
        .cwd(&dir),
    );
    assert_eq!(exit_code(&out), 0, "fallback build must succeed, stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("unknown target 'nonsense-target'"), "target warning missing: {err}");
    assert!(err.contains("unknown format 'nonsense-format'"), "format warning missing: {err}");
}

// ═══════════════════════════════════════════════════════════════════════
// §5 test-runner faces (bun_test / node:test gate: argv[1] === 'test')
// ═══════════════════════════════════════════════════════════════════════

/// Canonical CJS fixture shape (node_silent_fake_eradication_tests.rs lineage):
/// `process.argv[1]='test'` + `require('node:test')` registers suites that the
/// runner then drives. A pass+fail mix renders ✓/✗/message/stack lines, the
/// per-file counters, the summary, and exits 1.
#[test]
fn test_file_reports_pass_fail_mix_and_exits_1() {
    let bin = find_bao_binary();
    let dir = scratch_dir("test-mix");
    std::fs::write(
        dir.join("mix.cjs"),
        "process.argv[1] = 'test';\n\
         var nt = require('node:test');\n\
         nt.test('mix-passes', function() {});\n\
         nt.test('mix-fails', function() { throw new Error('boom-mix'); });\n",
    )
    .expect("write fixture");
    let out = run_spawn(&bin, &Spawn::args(vec!["test", "mix.cjs"]).cwd(&dir));
    assert_eq!(exit_code(&out), 1, "failing suite must exit 1");
    let err = stderr(&out);
    assert!(err.contains("✓ mix-passes"), "pass line missing: {err}");
    assert!(err.contains("✗ mix-fails"), "fail line missing: {err}");
    assert!(err.contains("boom-mix"), "failure message missing: {err}");
    assert!(err.contains("at:"), "stack line missing: {err}");
    assert!(err.contains("-> 1 passed, 1 failed"), "per-file counters missing: {err}");
    assert!(err.contains("# Summary: 1 passed, 1 failed"), "summary missing: {err}");
}

/// An all-pass file exits 0 and reports the zero-failure counters.
#[test]
fn test_file_all_pass_exits_0() {
    let bin = find_bao_binary();
    let dir = scratch_dir("test-pass");
    std::fs::write(
        dir.join("pass.cjs"),
        "process.argv[1] = 'test';\n\
         require('node:test').test('pass-only', function() {});\n",
    )
    .expect("write fixture");
    let out = run_spawn(&bin, &Spawn::args(vec!["test", "pass.cjs"]).cwd(&dir));
    assert_eq!(exit_code(&out), 0, "all-pass file must exit 0");
    let err = stderr(&out);
    assert!(err.contains("✓ pass-only"), "pass line missing: {err}");
    assert!(err.contains("-> 1 passed, 0 failed"), "counters missing: {err}");
}

/// An unreadable file hits the run_one_test_file Err arm: `FAIL [<path>]` and
/// a +1 failure count.
#[test]
fn test_file_missing_fails_per_file() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["test", "definitely-missing.cjs"]));
    assert_eq!(exit_code(&out), 1);
    let err = stderr(&out);
    assert!(err.contains("FAIL [definitely-missing.cjs]"), "per-file FAIL missing: {err}");
    assert!(err.contains("# Summary: 0 passed, 1 failed"), "failure must be counted: {err}");
}

/// Discovery: a cwd with a `tests/` directory is scanned; a plain .js there
/// runs (0/0) and the summary exits 0.
#[test]
fn test_discovery_scans_tests_dir() {
    let bin = find_bao_binary();
    let dir = scratch_dir("test-disc");
    std::fs::create_dir_all(dir.join("tests")).expect("mkdir tests");
    std::fs::write(dir.join("tests").join("disc.js"), "console.log('disc-ran');\n")
        .expect("write fixture");
    let out = run_spawn(&bin, &Spawn::args(vec!["test"]).cwd(&dir));
    assert_eq!(exit_code(&out), 0, "discovered 0/0 run must exit 0, stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("# tests/disc.js"), "discovered file header missing: {err}");
    assert!(stdout(&out).contains("disc-ran"), "discovered file must have run");
    assert!(err.contains("# Summary: 0 passed, 0 failed"), "summary missing: {err}");
}

/// A cwd with none of test/ tests/ __tests__/ reports the miss and exits 1.
#[test]
fn test_discovery_miss_reports_no_files() {
    let bin = find_bao_binary();
    let dir = scratch_dir("test-nodisc");
    let out = run_spawn(&bin, &Spawn::args(vec!["test"]).cwd(&dir));
    assert_eq!(exit_code(&out), 1);
    assert!(
        stderr(&out).contains("no test files found"),
        "discovery miss must be reported, got: {}",
        stderr(&out)
    );
}

/// `bao test -e` IS the runner: suites registered by the eval are driven and
/// rendered (Ok arm, exit 0).
#[test]
fn test_eval_drives_registered_suites() {
    let bin = find_bao_binary();
    let code = "process.argv[1] = 'test'; require('node:test').test('inline-ok', () => {});";
    let out = run_spawn(&bin, &Spawn::args(vec!["test", "-e", code]));
    assert_eq!(exit_code(&out), 0, "registered pass must exit 0, stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("✓ inline-ok"), "rendered pass missing: {}", stderr(&out));
}

/// A syntax error in `test -e` hits the eval Err arm: `FAIL: <message>`, exit 1.
#[test]
fn test_eval_syntax_error_fails() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["test", "-e", "syntax (( bad"]));
    assert_eq!(exit_code(&out), 1);
    assert!(
        stderr(&out).starts_with("FAIL:"),
        "eval failure must print FAIL:, got: {}",
        stderr(&out)
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §6 install forwarding (delegates to bun_runtime::install)
// ═══════════════════════════════════════════════════════════════════════

/// The Install dispatch arm reaches bun_install's argument parse; in a barren
/// cwd the package-manager init fails deterministically (MissingPackageJSON)
/// with exit 1 — no network, no side effects.
#[test]
fn install_forward_fails_without_package_json() {
    let bin = find_bao_binary();
    let dir = scratch_dir("install-barren");
    let out = run_spawn(&bin, &Spawn::args(vec!["install"]).cwd(&dir));
    assert_eq!(exit_code(&out), 1, "install without package.json must exit 1");
    assert!(
        stderr(&out).contains("bao install:"),
        "install failure must be bao-prefixed, got: {}",
        stderr(&out)
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §7 compat dispatch (CompatDomain routing, --json machine face)
// ═══════════════════════════════════════════════════════════════════════

/// All four CompatDomain variants route to their inventory and the --json
/// face emits the machine-readable envelope for the requested domain.
#[test]
fn compat_domains_route_with_json() {
    let bin = find_bao_binary();
    for (domain, anchor) in [
        ("node", "\"domain\":\"node\""),
        ("bun", "\"domain\":\"bun\""),
        ("cdp", "\"domain\":\"cdp\""),
        ("web", "\"domain\":\"web\""),
    ] {
        let out = run_spawn(&bin, &Spawn::args(vec!["compat", domain, "--json"]));
        assert_eq!(exit_code(&out), 0, "`compat {domain} --json` must exit 0");
        let text = stdout(&out);
        assert!(
            text.contains(anchor),
            "`compat {domain} --json` must emit the domain envelope, got: {}",
            &text[..text.len().min(200)]
        );
    }
}

/// The no-domain summary face exits 0 and renders all four domains.
#[test]
fn compat_summary_exits_zero() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["compat"]));
    assert_eq!(exit_code(&out), 0);
    let text = stdout(&out);
    for domain in ["node", "bun", "cdp", "web"] {
        assert!(text.contains(domain), "summary must mention {domain}");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §8 browser-entry validation arms (all fail-closed BEFORE any launch)
// ═══════════════════════════════════════════════════════════════════════

/// `--pref K` without `=value` fails the launch validation (exit 2).
#[test]
fn browser_entry_pref_validation_rejects_missing_value() {
    let bin = find_bao_binary();
    let out = run_spawn(&bin, &Spawn::args(vec!["--pref", "NOEQUALS"]));
    assert_eq!(exit_code(&out), 2);
    assert!(
        stderr(&out).contains("--pref expects K=V"),
        "pref validation message missing, got: {}",
        stderr(&out)
    );
}

/// `--window-size` validation: malformed shape, W < 800, and H < 600 all fail
/// the launch (exit 2) — wptrunner sizes reftests against this viewport.
#[test]
fn browser_entry_window_size_validation_arms() {
    let bin = find_bao_binary();
    for args in [
        vec!["--window-size", "garbage"],
        vec!["--window-size", "100x100"],
        vec!["--window-size", "1599x50"],
    ] {
        let out = run_spawn(&bin, &Spawn::args(args.clone()));
        assert_eq!(exit_code(&out), 2, "`{:?}` must exit 2", args);
        assert!(
            stderr(&out).contains("--window-size expects WxH"),
            "`{:?}` validation message missing, got: {}",
            args,
            stderr(&out)
        );
    }
}

/// `--prefs-file` pointing at a corrupt file fails the launch (exit 2); a
/// missing file is tolerated like upstream servoshell (warn + skip) and the
/// pipeline continues to the next validation gate.
#[test]
fn browser_entry_prefs_file_arms() {
    let bin = find_bao_binary();
    let dir = scratch_dir("prefs");
    std::fs::write(dir.join("corrupt.json"), "not json at all {{{").expect("write fixture");
    let corrupt = dir.join("corrupt.json").to_string_lossy().into_owned();
    let out = run_spawn(&bin, &Spawn::args(vec!["--prefs-file", &corrupt]));
    assert_eq!(exit_code(&out), 2, "corrupt prefs must fail the launch");
    assert!(
        stderr(&out).contains("parsing prefs file"),
        "corrupt prefs message missing, got: {}",
        stderr(&out)
    );
    let out = run_spawn(
        &bin,
        &Spawn::args(vec!["--prefs-file", "/nonexistent/prefs.json", "--window-size", "garbage"]),
    );
    assert_eq!(exit_code(&out), 2, "missing prefs must skip, then window-size gate fires");
    let err = stderr(&out);
    assert!(
        err.contains("not found (skipping, like upstream servoshell)"),
        "missing prefs must warn-and-skip, got: {err}"
    );
    assert!(err.contains("--window-size expects WxH"), "next gate must fire, got: {err}");
}

/// The experimental-features expansion runs during entry handling; combined
/// with a later validation gate the arm is exercised without any launch.
#[test]
fn browser_entry_experimental_flag_expansion_then_gate() {
    let bin = find_bao_binary();
    let out = run_spawn(
        &bin,
        &Spawn::args(vec![
            "--enable-experimental-web-platform-features",
            "--window-size",
            "garbage",
        ]),
    );
    assert_eq!(exit_code(&out), 2, "gate must fire after the expansion");
    assert!(stderr(&out).contains("--window-size expects WxH"));
}

// ═══════════════════════════════════════════════════════════════════════
// §9 doctor — environment diagnostics (informational: always exit 0)
// ═══════════════════════════════════════════════════════════════════════

/// The check-census contract: exactly the 20 registered checks render, the
/// terminator is consistent with the observed ✓/✗ census, and the process
/// always exits 0 (doctor is informational by contract).
#[test]
fn doctor_check_census_and_terminator_consistency() {
    let bin = find_bao_binary();
    let out = run_spawn_with_ambient_path(&bin, &["doctor"]);
    assert_eq!(exit_code(&out), 0, "doctor never exits non-zero");
    let text = stdout(&out);
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    assert!(
        header.starts_with("Bao doctor v") && header.contains(env!("CARGO_PKG_VERSION")),
        "header must carry the version, got: {header}"
    );

    const EXPECTED_LABELS: &[&str] = &[
        "Rust (rustc)",
        "Cargo",
        "C/C++ compiler",
        "Clang (vendored builds)",
        "pkg-config",
        "libc-ares",
        "zlib",
        "libarchive",
        "libdeflate",
        "fontconfig",
        "freetype",
        "glib-2.0",
        "GStreamer core",
        "GStreamer plugins-base",
        "GStreamer GL",
        "GStreamer webrtc",
        "GStreamer play",
        "DISPLAY (rendering)",
        "SpiderMonkey (built)",
        "CDP :9222",
    ];

    let check_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("  ✓ ") || l.starts_with("  ✗ "))
        .collect();
    assert_eq!(
        check_lines.len(),
        EXPECTED_LABELS.len(),
        "check census drifted: rendered {:?}",
        check_lines
    );
    for label in EXPECTED_LABELS {
        assert!(
            check_lines.iter().any(|l| l.contains(label)),
            "check line for {label} missing"
        );
    }

    // Terminator consistency: the census decides the closing message.
    let any_failed = check_lines.iter().any(|l| l.starts_with("  ✗ "));
    let expected_terminator = if any_failed {
        "Some checks failed — see the ✗ lines above for fix hints."
    } else {
        "All checks passed. Bao should build and run on this machine."
    };
    let last = text.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or_default();
    assert_eq!(last, expected_terminator, "terminator must match the ✗ census");
}

/// With DISPLAY stripped the DISPLAY check must be the ✗ arm with the fix
/// hint; with DISPLAY set it must be the ✓ arm echoing the value. Doctor does
/// not require a live X server — it only reads the environment variable.
#[test]
fn doctor_display_check_reflects_environment() {
    let bin = find_bao_binary();
    let unset = run_spawn(&bin, &Spawn::args(vec!["doctor"]));
    assert_eq!(exit_code(&unset), 0);
    let unset_text = stdout(&unset);
    let line = unset_text
        .lines()
        .find(|l| l.contains("DISPLAY (rendering)"))
        .expect("DISPLAY check line must render");
    assert!(line.starts_with("  ✗ "), "stripped DISPLAY must be the ✗ arm: {line}");
    assert!(
        line.contains("unset — browser rendering needs X"),
        "✗ arm must carry the fix hint: {line}"
    );

    let set = run_spawn(&bin, &Spawn::args(vec!["doctor"]).env("DISPLAY", ":99"));
    assert_eq!(exit_code(&set), 0);
    let set_text = stdout(&set);
    let line = set_text
        .lines()
        .find(|l| l.contains("DISPLAY (rendering)"))
        .expect("DISPLAY check line must render");
    assert!(line.starts_with("  ✓ "), "set DISPLAY must be the ✓ arm: {line}");
    assert!(line.contains(":99"), "✓ arm must echo the DISPLAY value: {line}");
}

/// Toolchain-absent environment (PATH pointing nowhere): every tool probe
/// takes its not-found arm and the system libs degrade to the unverifiable
/// arm instead of guessing "missing". All invoked by absolute path, so the
/// spawn itself works.
#[test]
fn doctor_pathless_environment_takes_not_found_arms() {
    let bin = find_bao_binary();
    let mut cmd = Command::new(&bin);
    for var in ["DISPLAY", "WAYLAND_DISPLAY", "PATH"] {
        cmd.env_remove(var);
    }
    cmd.env("PATH", "/nonexistent");
    cmd.arg("doctor").stdin(Stdio::null());
    let out = cmd.output().expect("spawn doctor with barren PATH");
    assert_eq!(exit_code(&out), 0);
    let text = stdout(&out);
    for marker in [
        "not found — install via https://rustup.rs",
        "not found — ships with rustup",
        "none of clang/gcc/cc found — mozjs needs a C++ compiler",
        "clang not found — boringssl/lsquic/uws build scripts require it (apt: clang)",
        "not found — install pkg-config (apt: pkg-config)",
        "unverifiable — install pkg-config first",
    ] {
        assert!(text.contains(marker), "pathless arm missing `{marker}`");
    }
}

/// The rustc check's nightly gate: the mark and detail must be consistent
/// with what the ambient `rustc --version` actually reports (✓ + version on
/// nightly; ✗ + `Bao requires nightly` hint otherwise).
#[test]
fn doctor_rustc_check_is_consistent_with_ambient_rustc() {
    let bin = find_bao_binary();
    let ambient = Command::new("rustc")
        .arg("--version")
        .output()
        .expect("ambient rustc must be spawnable");
    let version_line = String::from_utf8_lossy(&ambient.stdout).trim().to_owned();
    assert!(!version_line.is_empty(), "rustc --version produced no output");

    let out = run_spawn_with_ambient_path(&bin, &["doctor"]);
    let text = stdout(&out);
    let line = text
        .lines()
        .find(|l| l.contains("Rust (rustc)"))
        .expect("Rust check line must render");
    if version_line.contains("nightly") {
        assert!(line.starts_with("  ✓ "), "nightly rustc must be ✓: {line}");
        assert!(line.contains(&version_line), "✓ arm must echo the version: {line}");
    } else {
        assert!(line.starts_with("  ✗ "), "stable rustc must be ✗: {line}");
        assert!(
            line.contains("← Bao requires nightly; run: rustup default nightly"),
            "✗ arm must carry the nightly hint: {line}"
        );
    }
}

/// The ✓-nightly arm driven deterministically: point PATH at the repo-pinned
/// nightly toolchain's bin dir (resolved via `rustup which rustc` from the
/// workspace) and the Rust check must turn ✓ with the nightly version echoed.
#[test]
fn doctor_nightly_rustc_check_passes() {
    let bin = find_bao_binary();
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let which = Command::new("rustup")
        .args(["which", "rustc"])
        .current_dir(&workspace)
        .output()
        .unwrap_or_else(|e| panic!("rustup must be available to resolve the pinned toolchain: {e}"));
    assert!(which.status.success(), "rustup which rustc failed: {}", stderr(&which));
    let rustc_path = String::from_utf8_lossy(&which.stdout).trim().to_owned();
    assert!(
        rustc_path.contains("nightly"),
        "the workspace-pinned rustc must be nightly, got: {rustc_path}"
    );
    let nightly_bin = std::path::Path::new(&rustc_path)
        .parent()
        .expect("rustc path must have a parent")
        .to_string_lossy()
        .into_owned();

    let mut cmd = Command::new(&bin);
    for var in ["DISPLAY", "WAYLAND_DISPLAY", "PATH"] {
        cmd.env_remove(var);
    }
    cmd.env("PATH", &nightly_bin);
    cmd.arg("doctor").stdin(Stdio::null());
    let out = cmd.output().expect("spawn doctor with nightly PATH");
    let text = stdout(&out);
    let line = text
        .lines()
        .find(|l| l.contains("Rust (rustc)"))
        .expect("Rust check line must render");
    assert!(line.starts_with("  ✓ "), "nightly toolchain must pass the gate: {line}");
    assert!(line.contains("nightly"), "✓ detail must echo the nightly version: {line}");
}

/// SpiderMonkey artifact hint: a barren cwd reports the no-cache arm; a cwd
/// whose target/debug/deps holds a mozjs-named artifact reports the
/// incremental-build arm (walk_target_for_mozjs + dir_mentions_mozjs).
#[test]
fn doctor_mozjs_artifact_hint_reflects_target_state() {
    let bin = find_bao_binary();
    let barren = scratch_dir("mozjs-barren");
    let out = run_spawn(&bin, &Spawn::args(vec!["doctor"]).cwd(&barren));
    let barren_text = stdout(&out);
    let line = barren_text
        .lines()
        .find(|l| l.contains("SpiderMonkey (built)"))
        .expect("SpiderMonkey line must render");
    assert!(
        line.contains("no cached artifact — first build will compile SpiderMonkey (slow)"),
        "barren cwd must report the no-cache arm: {line}"
    );

    let cached = scratch_dir("mozjs-cached");
    let deps = cached.join("target").join("debug").join("deps");
    std::fs::create_dir_all(&deps).expect("mkdir deps");
    std::fs::write(deps.join("libmozjs_sys-abcdef.rlib"), b"stub").expect("write artifact stub");
    let out = run_spawn(&bin, &Spawn::args(vec!["doctor"]).cwd(&cached));
    let cached_text = stdout(&out);
    let line = cached_text
        .lines()
        .find(|l| l.contains("SpiderMonkey (built)"))
        .expect("SpiderMonkey line must render");
    assert!(
        line.contains("libmozjs_sys artifact present"),
        "cached artifact must flip the hint: {line}"
    );
}

/// CDP port probe: while THIS test holds a listener on 127.0.0.1:9222 the
/// check must report the reachable arm. The listener is held under a
/// nextest `threads-required = num-test-threads` isolation override
/// (.config/nextest.toml) so no sibling test co-runs against the port.
#[test]
fn doctor_cdp_port_reachable_arm() {
    let bin = find_bao_binary();
    let bind = std::net::TcpListener::bind("127.0.0.1:9222");
    if bind.is_err() {
        // The port is held by another process right now — that other holder
        // makes the probe reachable too; assert the same arm.
        eprintln!("[cli-faces] 9222 held elsewhere; asserting via the external holder");
    }
    let _guard = bind.ok(); // hold our listener (when we won the bind) for the spawn
    let out = run_spawn_with_ambient_path(&bin, &["doctor"]);
    assert_eq!(exit_code(&out), 0);
    let text = stdout(&out);
    let line = text
        .lines()
        .find(|l| l.contains("CDP :9222"))
        .expect("CDP line must render");
    assert!(line.starts_with("  ✓ "), "CDP check is always ok: {line}");
    assert!(
        line.contains("reachable — a CDP server is already listening"),
        "held port must report the reachable arm: {line}"
    );
}

/// The default (non-listening) CDP arm: `not listening` is the normal state
/// and must render with the same ✓ mark. Only asserted when this test can
/// observe the port free (a held port by another process flips the detail,
/// which the reachable-arm test covers).
#[test]
fn doctor_cdp_port_free_arm_when_port_free() {
    let bin = find_bao_binary();
    // Probe the port the way doctor will: if something already answers, this
    // test cannot observe the free arm — the run still passes (the arm is
    // covered whenever the port is free), but with a visible note.
    let occupied = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:9222".parse().expect("static addr"),
        std::time::Duration::from_millis(300),
    )
    .is_ok();
    let out = run_spawn_with_ambient_path(&bin, &["doctor"]);
    let text = stdout(&out);
    let line = text
        .lines()
        .find(|l| l.contains("CDP :9222"))
        .expect("CDP line must render");
    assert!(line.starts_with("  ✓ "), "CDP check is always ok: {line}");
    if !occupied {
        assert!(
            line.contains("not listening (normal unless `bao browser` is running)"),
            "free port must report the not-listening arm: {line}"
        );
    } else {
        eprintln!("[cli-faces] 9222 occupied; free arm not observable this run");
    }
}
