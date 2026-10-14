// @trace REQ-CLI-3 [level:system]
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
//   T10 `run --module --timeout 30000 -e 'process.exit(0)'` → exit 0
//       (AC03: an explicit orderly exit is not disturbed by the controlled-
//       code precedence; the unused 30s deadline never fires).
//   T11 `run --module --timeout 30000 -e 'process.exit(7)'` → exit 7
//       (AC03: nonzero explicit codes pass through unshadowed).
//   T12 module entry whose runaway timer callback records an unhandled
//       rejection (flushed through the uncaught machinery after the
//       interrupt → observable `request_exit(1)` side effect on stderr)
//       and then spins into the deadline → exit **124**: the controlled
//       code wins over the exit machinery's 1.
//   T13 same composition under `--timeout 60000` + kill -INT (a
//       writeFileSync marker pins the signal mid-callback) → exit **130**
//       (Cancelled variant of the same precedence).
//
// **运行约束**: 测试需要预先 `cargo build -p bao_bin` 产出 bao 二进制。
// 缺失时 skip 而非 fail(避免 CI 在未 build 时直接红)。

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[path = "common/mod.rs"]
mod common;

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
/// AC03 precedence-face deadline: 800ms leaves engine bring-up + module
/// compile + the first pump tick comfortable headroom to register and fire
/// the 0ms timer BEFORE the deadline, so the uncaught-route side effect
/// (post-interrupt rejection flush) is guaranteed to be part of the run the
/// deadline terminates.
const PRIORITY_DEADLINE_MS: u64 = 800;
/// Lower bound paired with PRIORITY_DEADLINE_MS: the runaway must actually
/// run until the deadline (not die instantly) — same rationale as
/// MIN_ELAPSED_MS above.
const PRIORITY_MIN_ELAPSED_MS: u64 = 700;

// ─── 辅助 — 定位 bao 二进制(与 bao_cli_e2e_tests 相同的定位序) ────────────

fn bao_path() -> Option<PathBuf> {
    if let Ok(override_path) = std::env::var("BAO_BIN") {
        let candidate = PathBuf::from(override_path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Ok(p) = std::env::var("BAO_TEST_BAO_BIN") {
        let candidate = PathBuf::from(p);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // Profile-agnostic probe (BCE sweep with the bao_runtime finders): this
    // suite exe sits at $TARGET/<profile>/deps/ — the bao binary sits at
    // $TARGET/<profile>/bao for EVERY cargo profile, so derive the sibling
    // instead of hardcoding "debug" (test-ci runs never find a debug build).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(profile_dir) = exe.parent().and_then(|d| d.parent()) {
            for name in ["bao", "bao.exe"] {
                let candidate = profile_dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        for profile in ["test-ci", "debug", "release"] {
            for name in ["bao", "bao.exe"] {
                let candidate = PathBuf::from(&target_dir).join(profile).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let mut here = std::env::current_dir().ok()?;
    for _ in 0..5 {
        for profile in ["test-ci", "debug", "release"] {
            for name in ["bao", "bao.exe"] {
                let candidate = here.join("target").join(profile).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        if !here.pop() {
            break;
        }
    }
    None
}

/// Spawn `bao <args>` with piped stdio, in its own process group with
/// PDEATHSIG armed (e204 reaping discipline — see `common::GroupChild`).
/// Panics (test fail) if spawn fails.
fn spawn_bao(args: &[&str]) -> common::GroupChild {
    let bao = bao_path().expect("bao binary not found — run `cargo build -p bao_bin` first");
    common::GroupChild::spawn(
        Command::new(bao)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .expect("spawn bao failed")
}

/// Poll-wait with a hard deadline (never blocks forever on a hang
/// regression): on deadline the child's whole process group is SIGKILLed and
/// the child reaped (e204: the breach path must never leak the runaway into
/// the suite), then `None` returned.
fn wait_with_deadline(child: &mut common::GroupChild, limit: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if start.elapsed() >= limit {
            child.kill_group();
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Drain piped stdout/stderr after the child exited (runaway tests write
/// nothing, so the pipes never fill — no deadlock while polling above).
fn drain_pipes(child: &mut common::GroupChild) -> (String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout().take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr().take() {
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

/// @trace REQ-CLI-3 [level:e2e] — `--timeout` terminates a runaway script
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

/// @trace REQ-CLI-3 [level:e2e] — `bao run --timeout <ms> file.mjs`: the
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

/// @trace REQ-CLI-3 [level:e2e] — `bao run --timeout <ms> file.js`: the
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

/// @trace REQ-CLI-3 [level:e2e] — a fast script under a large `--timeout`
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

/// @trace REQ-CLI-3 [level:e2e] — Ctrl-C equivalent (kill -INT) on a
/// runaway script running under `--timeout` produces the stable Cancelled
/// terminal state: the engine terminates the script (uncatchable), the CLI
/// surfaces "execution cancelled" on stderr and exits 130 (128+SIGINT).
#[test]
#[cfg(unix)] // POSIX signal semantics — Windows equivalent is a separate mechanism
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

/// @trace REQ-CLI-3 [level:e2e] — negative control for the "no flag →
/// byte-level unchanged" contract: without `--timeout` no bridge is
/// installed, so kill -INT terminates bao by the default disposition —
/// the process is KILLED BY SIGNAL 2 (not a controlled exit code).
#[test]
#[cfg(unix)] // POSIX signal semantics — Windows equivalent is a separate mechanism
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

/// @trace REQ-CLI-3 [level:e2e] — `--timeout 0` is a flag mistake (an
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

/// @trace REQ-CLI-3 [level:e2e] — `--timeout` on a non-script subcommand is
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

/// @trace REQ-CLI-3 [level:e2e] — `bao run --module --timeout <ms> -e`:
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

// ─── T10/T11: 显式 process.exit(n) 在 armed --timeout 下不受优先序破坏 ──────

/// @trace REQ-CLI-3 [level:e2e] — AC03 explicit-exit half: `process.exit(0)`
/// under an armed (unused) `--timeout` still exits 0. The controlled-code
/// precedence (`termination_exit` wins over the exit machinery) must never
/// disturb an explicit, orderly exit that the control did not terminate —
/// and finishing inside the 10s window is itself the proof the 30s deadline
/// never fired.
#[test]
fn explicit_process_exit_zero_under_armed_timeout() {
    let mut child = spawn_bao(&[
        "run",
        "--module",
        "--timeout",
        "30000",
        "--eval",
        "process.exit(0)",
    ]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let (stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!("process.exit(0) under armed --timeout did not finish");
    };
    assert_eq!(
        status.code(),
        Some(0),
        "explicit process.exit(0) must keep exit code 0 under armed --timeout; \
         stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
}

/// @trace REQ-CLI-3 [level:e2e] — AC03 explicit-exit half, nonzero variant:
/// `process.exit(7)` under an armed (unused) `--timeout` passes 7 through
/// unshadowed — the precedence order must not flatten explicit nonzero exit
/// codes onto the control mapping (124/130) either.
#[test]
fn explicit_process_exit_nonzero_under_armed_timeout() {
    let mut child = spawn_bao(&[
        "run",
        "--module",
        "--timeout",
        "30000",
        "--eval",
        "process.exit(7)",
    ]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let (stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!("process.exit(7) under armed --timeout did not finish");
    };
    assert_eq!(
        status.code(),
        Some(7),
        "explicit process.exit(7) must keep exit code 7 under armed --timeout; \
         stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
}

// ─── T12: 受控终止码 124 优先于 module uncaught 路由的 request_exit(1) ──────

/// @trace REQ-CLI-3 [level:e2e] — AC03 precedence half: a module entry whose
/// runaway timer callback records an unhandled rejection FIRST, then spins
/// into the deadline. The post-interrupt rejection flush routes through the
/// uncaught machinery, whose `request_exit(1)` side effect is observable on
/// stderr — and the exit must still be **124**: the controlled termination
/// code is the truthful signal and wins over the exit machinery's 1. The
/// stderr assertions make this a real precedence proof (both effects
/// provably present in the same run), not a vacuous 124.
#[test]
fn deadline_beats_module_uncaught_request_exit_one() {
    let start = Instant::now();
    let mut child = spawn_bao(&[
        "run",
        "--module",
        "--timeout",
        &PRIORITY_DEADLINE_MS.to_string(),
        "--eval",
        "setTimeout(() => { Promise.reject(new Error('late')); while(true){} }, 0)",
    ]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let elapsed = start.elapsed();
    let (stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!(
            "priority-face runaway did not terminate within {}ms",
            MAX_ELAPSED_MS
        );
    };
    assert_eq!(
        status.code(),
        Some(124),
        "controlled 124 must beat the uncaught route's request_exit(1); \
         stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
    assert!(
        stderr.contains("uncaught exception"),
        "uncaught-route side effect (request_exit(1)) must be on stderr for \
         this to be a precedence proof, got: {:?}",
        stderr
    );
    assert!(
        stderr.contains("deadline exceeded"),
        "deadline termination must be on stderr, got: {:?}",
        stderr
    );
    assert!(
        elapsed >= Duration::from_millis(PRIORITY_MIN_ELAPSED_MS),
        "terminated too early ({:?}) — deadline not honored from below",
        elapsed
    );
}

// ─── T13: 受控终止码 130(SIGINT→cancel)优先于 request_exit(1) ───────────────

/// @trace REQ-CLI-3 [level:e2e] — AC03 precedence half, Cancelled variant:
/// the same composition under an armed `--timeout 60000`, terminated by
/// SIGINT→cancel, must exit **130** — beating the uncaught route's
/// `request_exit(1)` side effect (128+SIGINT is the truthful signal). The
/// file-marker handshake makes the signal deterministic: the timer callback
/// `writeFileSync`es a marker (kernel-side, visible immediately — console
/// output is only flushed at process exit and would never arrive mid-spin)
/// BEFORE spinning, and kill -INT is delivered only once the marker proves
/// the runaway callback is spinning inside the pump.
#[test]
#[cfg(unix)] // POSIX signal semantics — Windows equivalent is a separate mechanism
fn sigint_cancel_beats_module_uncaught_request_exit_one() {
    let marker = std::env::temp_dir().join(format!(
        "bao-timeout-e2e-spin-{}-{}",
        std::process::id(),
        "sigint-priority"
    ));
    let _ = std::fs::remove_file(&marker);
    let js = format!(
        "setTimeout(() => {{ require('node:fs').writeFileSync('{}', 'fired'); \
         Promise.reject(new Error('late')); while(true){{}} }}, 0)",
        marker.to_string_lossy()
    );
    let mut child = spawn_bao(&["run", "--module", "--timeout", "60000", "--eval", &js]);
    let armed = common::wait_for_condition(Duration::from_secs(15), || marker.is_file());
    assert!(
        armed,
        "timer callback never started spinning — cannot deliver a mid-callback SIGINT"
    );
    // SAFETY: signal delivery to our own child pid.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    let status = wait_with_deadline(&mut child, Duration::from_millis(MAX_ELAPSED_MS));
    let (_stdout, stderr) = drain_pipes(&mut child);
    let _ = std::fs::remove_file(&marker);
    let Some(status) = status else {
        panic!(
            "priority-face runaway did not terminate after SIGINT within {}ms",
            MAX_ELAPSED_MS
        );
    };
    assert_eq!(
        status.code(),
        Some(130),
        "controlled 130 (128+SIGINT) must beat the uncaught route's \
         request_exit(1); stderr={:?}",
        stderr
    );
    assert!(
        stderr.contains("uncaught exception"),
        "uncaught-route side effect (request_exit(1)) must be on stderr for \
         this to be a precedence proof, got: {:?}",
        stderr
    );
    assert!(
        stderr.contains("cancelled"),
        "stable Cancelled termination error expected on stderr, got: {:?}",
        stderr
    );
}
