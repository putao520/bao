// @trace REQ-CLI-001 [level:system]
//
// # SM-EVOLUTION #24 S1 CLI wiring — `--timeout <ms>` + SIGINT→cancel E2E
//
// (ledger S1 legislation proposal consumed by user ruling 2026-09-10)
//
// Real-binary subprocess tests (same locator pattern as bao_cli_e2e_tests):
//
//   T1  `bao --timeout 400 -e 'while(true){}'`   → exit 124, stderr
//       "deadline exceeded", elapsed in [350ms, 10s] (deterministic window:
//       not an instant parse failure, and it actually terminates).
//   T2  `bao run --timeout 400 runaway.mjs`      → exit 124 (module file
//       entry: run_file_with_control → eval_module_with_control).
//   T3  `bao run --timeout 400 runaway.js`       → exit 124 (script file
//       entry: run_file_with_control → eval_with_control).
//   T4  `--timeout 30000` + fast script          → exit 0, output intact
//       (an unused deadline never fires — S1 semantics).
//   T5  `--timeout 60000` runaway + kill -INT    → exit 130, stderr
//       "cancelled" (SIGINT→InterruptBridge→cancel, stable Cancelled
//       terminal state surfaced as the conventional 128+2 code).
//   T6  runaway WITHOUT --timeout + kill -INT    → killed by SIGINT signal
//       (negative control: no flag → default disposition, byte-level
//       unchanged behavior).
//   T7  `--timeout 0`                            → clap rejection, exit 2.
//   T8  `--timeout` on a non-script subcommand   → fail-closed exit 2 (the
//       flag is never silently ignored).
//   T9  `bao run --module --timeout 400 -e`      → exit 124 (module eval
//       entry: run_module_eval controlled path).
//
// **运行约束**: 测试需要预先 `cargo build -p bao_bin` 产出 bao 二进制。
// 缺失时 skip 而非 fail(避免 CI 在未 build 时直接红)。

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Elapsed-window bounds shared by the runaway tests: the deadline (400ms)
/// must be honored from below (>= 350ms — proves the script actually ran
/// until the deadline, not an instant failure) and termination must arrive
/// promptly (<= 10s — generous CI slack, still catches a hang regression).
const DEADLINE_MS: u64 = 400;
const MIN_ELAPSED_MS: u64 = 350;
const MAX_ELAPSED_MS: u64 = 10_000;
/// How long the SIGINT tests let the child spin before signaling (must be
/// comfortably past engine bring-up so the kill lands mid-JS).
const SPIN_BEFORE_SIGNAL_MS: u64 = 800;

// ─── 辅助 — 定位 bao 二进制(与 bao_cli_e2e_tests 相同的定位序) ────────────

fn bao_path() -> Option<PathBuf> {
    if let Ok(override_path) = std::env::var("BAO_BIN") {
        let candidate = PathBuf::from(override_path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let candidate = PathBuf::from(target_dir).join("debug").join("bao");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let mut here = std::env::current_dir().ok()?;
    for _ in 0..5 {
        let candidate = here.join("target/debug/bao");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !here.pop() {
            break;
        }
    }
    None
}

/// Spawn `bao <args>` with piped stdio. Panics (test fail) if spawn fails.
fn spawn_bao(args: &[&str]) -> std::process::Child {
    let bao = bao_path().expect("bao binary not found — run `cargo build -p bao_bin` first");
    Command::new(bao)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bao failed")
}

/// Poll-wait with a hard deadline (never blocks forever on a hang
/// regression): on deadline the child is SIGKILLed and `None` returned.
fn wait_with_deadline(child: &mut std::process::Child, limit: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if start.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Drain piped stdout/stderr after the child exited (runaway tests write
/// nothing, so the pipes never fill — no deadlock while polling above).
fn drain_pipes(child: &mut std::process::Child) -> (String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    (stdout, stderr)
}

/// Unique temp file for the file-entry tests.
fn write_runaway_file(name: &str, body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "bao-timeout-e2e-{}-{}",
        std::process::id(),
        name
    ));
    std::fs::write(&path, body).expect("write runaway fixture failed");
    path
}

// ─── T1: top-level `-e` script entry honors the deadline ────────────────────

/// @trace REQ-CLI-001 [level:e2e] — `--timeout` terminates a runaway script
/// (SM-EVOLUTION #24 S1 CLI wiring) with exit code 124 (GNU timeout
/// convention) inside the deterministic window, and the stable termination
/// error names the deadline.
#[test]
fn timeout_terminates_runaway_top_level_eval() {
    let start = Instant::now();
    let mut child = spawn_bao(&["--timeout", &DEADLINE_MS.to_string(), "-e", "while(true){}"]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let elapsed = start.elapsed();
    let (stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!(
            "runaway -e did not terminate within {}ms (--timeout dead?)",
            MAX_ELAPSED_MS
        );
    };
    assert_eq!(
        status.code(),
        Some(124),
        "timeout must exit 124 (GNU timeout convention); stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
    assert!(
        stderr.contains("deadline exceeded"),
        "stable termination error expected on stderr, got: {:?}",
        stderr
    );
    assert!(
        elapsed >= Duration::from_millis(MIN_ELAPSED_MS),
        "terminated too early ({:?}) — deadline not honored from below",
        elapsed
    );
    assert!(
        elapsed <= Duration::from_millis(MAX_ELAPSED_MS),
        "termination too late ({:?})",
        elapsed
    );
}

// ─── T2/T3: file entries (module / script dispatch) honor the deadline ─────

/// @trace REQ-CLI-001 [level:e2e] — `bao run --timeout <ms> file.mjs`: the
/// module file entry (run_file_with_control → eval_module_with_control,
/// whole-entry arm incl. the post-eval pump) terminates with 124.
#[test]
fn timeout_terminates_runaway_module_file() {
    let fixture = write_runaway_file("runaway.mjs", "while(true){}\n");
    let start = Instant::now();
    let path_str = fixture.to_string_lossy().into_owned();
    let mut child = spawn_bao(&[
        "run",
        "--timeout",
        &DEADLINE_MS.to_string(),
        &path_str,
    ]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let elapsed = start.elapsed();
    let (stdout, stderr) = drain_pipes(&mut child);
    let _ = std::fs::remove_file(&fixture);
    let Some(status) = status else {
        panic!("runaway .mjs did not terminate within {}ms", MAX_ELAPSED_MS);
    };
    assert_eq!(
        status.code(),
        Some(124),
        "module file timeout must exit 124; stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
    assert!(stderr.contains("deadline exceeded"), "stderr: {:?}", stderr);
    assert!(
        elapsed >= Duration::from_millis(MIN_ELAPSED_MS),
        "terminated too early ({:?})",
        elapsed
    );
    assert!(
        elapsed <= Duration::from_millis(MAX_ELAPSED_MS),
        "termination too late ({:?})",
        elapsed
    );
}

/// @trace REQ-CLI-001 [level:e2e] — `bao run --timeout <ms> file.js`: the
/// script file entry (run_file_with_control → eval_with_control) terminates
/// with 124 — the runtime-internal script-vs-module dispatch is preserved
/// under control.
#[test]
fn timeout_terminates_runaway_script_file() {
    let fixture = write_runaway_file("runaway.js", "while(true){}\n");
    let start = Instant::now();
    let path_str = fixture.to_string_lossy().into_owned();
    let mut child = spawn_bao(&[
        "run",
        "--timeout",
        &DEADLINE_MS.to_string(),
        &path_str,
    ]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let elapsed = start.elapsed();
    let (stdout, stderr) = drain_pipes(&mut child);
    let _ = std::fs::remove_file(&fixture);
    let Some(status) = status else {
        panic!("runaway .js did not terminate within {}ms", MAX_ELAPSED_MS);
    };
    assert_eq!(
        status.code(),
        Some(124),
        "script file timeout must exit 124; stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
    assert!(stderr.contains("deadline exceeded"), "stderr: {:?}", stderr);
    assert!(
        elapsed >= Duration::from_millis(MIN_ELAPSED_MS),
        "terminated too early ({:?})",
        elapsed
    );
    assert!(
        elapsed <= Duration::from_millis(MAX_ELAPSED_MS),
        "termination too late ({:?})",
        elapsed
    );
}

// ─── T4: an unused deadline never fires on a fast script ───────────────────

/// @trace REQ-CLI-001 [level:e2e] — a fast script under a large `--timeout`
/// completes normally (exit 0, output intact): the S1 contract is that a
/// deadline only terminates *executing* JS, never taxes a clean completion.
#[test]
fn timeout_does_not_affect_fast_script() {
    let mut child = spawn_bao(&[
        "run",
        "--timeout",
        "30000",
        "--eval",
        "console.log('fast-ok')",
    ]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let (stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!("fast script under unused deadline did not finish");
    };
    assert_eq!(
        status.code(),
        Some(0),
        "fast script must exit 0; stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
    assert!(stdout.contains("fast-ok"), "stdout: {:?}", stdout);
}

// ─── T5: SIGINT cancels a runaway under --timeout (stable Cancelled 终态) ──

/// @trace REQ-CLI-001 [level:e2e] — Ctrl-C equivalent (kill -INT) on a
/// runaway script running under `--timeout` produces the stable Cancelled
/// terminal state: the engine terminates the script (uncatchable), the CLI
/// surfaces "execution cancelled" on stderr and exits 130 (128+SIGINT).
#[test]
fn sigint_cancels_runaway_under_timeout() {
    let start = Instant::now();
    let mut child = spawn_bao(&[
        "run",
        "--timeout",
        "60000",
        "--eval",
        "while(true){}",
    ]);
    std::thread::sleep(Duration::from_millis(SPIN_BEFORE_SIGNAL_MS));
    // SAFETY: signal delivery to our own child pid.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let elapsed = start.elapsed();
    let (stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!("runaway did not terminate after SIGINT within {}ms", MAX_ELAPSED_MS);
    };
    assert_eq!(
        status.code(),
        Some(130),
        "SIGINT-cancelled runaway must exit 130 (128+SIGINT); stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
    assert!(
        stderr.contains("cancelled"),
        "stable Cancelled termination error expected on stderr, got: {:?}",
        stderr
    );
    assert!(
        elapsed <= Duration::from_millis(MAX_ELAPSED_MS),
        "cancel termination too late ({:?})",
        elapsed
    );
}

// ─── T6: WITHOUT --timeout, SIGINT keeps the default kill (行为不变) ───────

/// @trace REQ-CLI-001 [level:e2e] — negative control for the "no flag →
/// byte-level unchanged" contract: without `--timeout` no bridge is
/// installed, so kill -INT terminates bao by the default disposition —
/// the process is KILLED BY SIGNAL 2 (not a controlled exit code).
#[test]
fn sigint_without_timeout_keeps_default_kill() {
    use std::os::unix::process::ExitStatusExt;
    let mut child = spawn_bao(&["run", "--eval", "while(true){}"]);
    std::thread::sleep(Duration::from_millis(SPIN_BEFORE_SIGNAL_MS));
    // SAFETY: signal delivery to our own child pid.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let (_stdout, _stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!("default-disposition SIGINT did not kill the process within {}ms", MAX_ELAPSED_MS);
    };
    assert_eq!(
        status.signal(),
        Some(libc::SIGINT),
        "without --timeout SIGINT must kill by signal (default disposition), got code={:?} signal={:?}",
        status.code(),
        status.signal()
    );
}

// ─── T7/T8: fail-closed argument surface ────────────────────────────────────

/// @trace REQ-CLI-001 [level:e2e] — `--timeout 0` is a flag mistake (an
/// instant deadline): rejected by the clap value parser with exit 2, never
/// silently accepted.
#[test]
fn timeout_zero_is_rejected() {
    let mut child = spawn_bao(&["run", "--timeout", "0", "--eval", "1"]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let (_stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!("--timeout 0 run did not finish");
    };
    assert_eq!(status.code(), Some(2), "clap rejection must exit 2");
    assert!(
        stderr.contains("--timeout"),
        "rejection must name --timeout, got: {:?}",
        stderr
    );
}

/// @trace REQ-CLI-001 [level:e2e] — `--timeout` on a non-script subcommand is
/// rejected fail-closed (exit 2 + message) instead of being silently ignored:
/// the flag only drives script execution entries.
#[test]
fn timeout_rejected_on_non_script_subcommand() {
    let mut child = spawn_bao(&["doctor", "--timeout", "100"]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let (_stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!("doctor --timeout run did not finish");
    };
    assert_eq!(
        status.code(),
        Some(2),
        "non-script --timeout must be rejected with exit 2; stderr={:?}",
        stderr
    );
    assert!(
        stderr.contains("--timeout"),
        "rejection must name --timeout, got: {:?}",
        stderr
    );
}

// ─── T9: `run --module -e` module eval entry honors the deadline ───────────

/// @trace REQ-CLI-001 [level:e2e] — `bao run --module --timeout <ms> -e`:
/// the module eval entry (eval_module_with_control) terminates with 124.
#[test]
fn timeout_terminates_runaway_module_eval() {
    let start = Instant::now();
    let mut child = spawn_bao(&[
        "run",
        "--module",
        "--timeout",
        &DEADLINE_MS.to_string(),
        "--eval",
        "while(true){}",
    ]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let elapsed = start.elapsed();
    let (stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!("runaway module -e did not terminate within {}ms", MAX_ELAPSED_MS);
    };
    assert_eq!(
        status.code(),
        Some(124),
        "module eval timeout must exit 124; stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
    assert!(stderr.contains("deadline exceeded"), "stderr: {:?}", stderr);
    assert!(
        elapsed >= Duration::from_millis(MIN_ELAPSED_MS),
        "terminated too early ({:?})",
        elapsed
    );
    assert!(
        elapsed <= Duration::from_millis(MAX_ELAPSED_MS),
        "termination too late ({:?})",
        elapsed
    );
}
