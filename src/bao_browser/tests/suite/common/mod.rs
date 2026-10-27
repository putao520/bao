// @trace NFR-TEST-REPRODUCIBILITY [criterion:wait_for_condition]
// Test helper utilities for deterministic wait conditions.
// Replaces magic-number sleep polling with explicit timeout + predicate.

pub mod bao_bin_canary;
pub mod client_hello;
pub mod h2_server;
pub mod http_fixture;
pub mod port_probe;

use std::thread;
use std::time::{Duration, Instant};

/// Self-isolation for full-engine e2e tests (mozjs Runtime / servo Opts /
/// HTTPThread / bridge destination state are per-process singletons).
///
/// The suite merges every former top-level target into ONE harness binary;
/// cargo-nextest restores per-test process isolation, but plain
/// `cargo test` — even with `--test-threads=1` — still runs tests in the
/// SAME process, and singleton-dependent e2e tests poison each other there
/// (observed: fingerprint e2e green alone but leaving h2 state that crashes
/// the full-matrix e2e's h2 leg; parallel runs additionally lose img/css
/// subresource deliveries to worker-test contention).
///
/// Usage — wrap the test body:
///
/// ```ignore
/// #[test]
/// fn my_e2e() {
///     if !common::run_isolated("my_module::my_e2e") {
///         return; // parent path: the child process already ran the body
///     }
///     // …real body…
/// }
/// ```
///
/// Returns `true` in the child (or when already isolated): run the body.
/// Returns `false` in the parent: a fresh process ran just this test
/// (`--exact`, `--test-threads=1`, output inherited) and its exit status
/// was asserted — early-return instead of running the body again.
pub fn run_isolated(test_name: &str) -> bool {
    const ISOLATED_ENV: &str = "BAO_SUITE_ISOLATED_TEST";
    if std::env::var_os(ISOLATED_ENV).is_some() {
        return true; // child (or an outer runner already isolated us)
    }
    // cargo-nextest already runs each test in its own process — the re-exec
    // dance below exists only for plain `cargo test`, where libtest shares
    // one process across tests. Re-execing from under nextest would ALSO
    // silently break `#[ignore = "..."]`-marked tests: the child libtest
    // invocation has no `--ignored`, so it would report the test "ignored",
    // exit 0, and the parent would skip the body — a fake green.
    if std::env::var_os("NEXTEST").is_some() {
        return true; // per-test process isolation is the runner's job here
    }
    let exe = std::env::current_exe().expect("current_exe for isolated re-exec");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(test_name)
        .arg("--exact")
        .arg("--test-threads=1")
        .arg("--nocapture");
    // Propagate an `--ignored`/`--include-ignored` invocation so a statically
    // #[ignore]d test re-exec'd from `cargo test -- --ignored` actually runs
    // in the child (a bare re-exec would report it ignored and exit 0 —
    // another silent fake green).
    let ignored_invocation =
        std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    if ignored_invocation {
        cmd.arg("--ignored");
    }
    let status = cmd
        .env(ISOLATED_ENV, "1")
        .status()
        .unwrap_or_else(|e| panic!("spawn isolated test process for {test_name}: {e}"));
    if !status.success() {
        panic!("isolated test process {test_name} failed: {status}");
    }
    false
}

/// Wait for a condition to become true, with explicit timeout.
///
/// Spins with small sleep intervals (10ms) checking the predicate.
/// Returns `true` if the condition became true before timeout,
/// `false` if timeout was reached.
///
/// # Arguments
/// * `timeout` - Maximum time to wait for the condition
/// * `predicate` - Function that returns true when the condition is met
///
/// # Example
/// ```ignore
/// use bao_browser::tests::common::wait_for_condition;
/// use std::time::Duration;
///
/// assert!(
///     wait_for_condition(Duration::from_secs(3), || !worker.is_running()),
///     "worker should stop within 3 seconds"
/// );
/// ```
///
/// @trace NFR-TEST-REPRODUCIBILITY [criterion:no_magic_sleep]
pub fn wait_for_condition<F>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        if predicate() {
            return true;
        }
        if Instant::now() >= deadline {
            return predicate(); // Final check
        }
        // yield_now keeps the polling tight (matching the original deadline
        // polling pattern used by worker lifecycle tests, which rely on
        // frequent state checks against the worker's AtomicBool).
        // BCE-20260627-007: yield_now without a microsleep can busy-loop at
        // 100% CPU on some schedulers. A 1ms micro-sleep prevents this while
        // still being tight enough for the ~1ms worker teardown path.
        thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Wait for a WebWorker to stop running.
///
/// Domain-specific helper for worker lifecycle tests. The `is_running`
/// predicate is supplied by the caller to avoid a hard type dependency on
/// `bun_sm::WebWorker` (which is not re-exported from `bao_browser`).
///
/// # Arguments
/// * `timeout` - Maximum time to wait for the worker to stop
/// * `is_running` - Closure returning the worker's running state
///
/// # Returns
/// `true` if the worker stopped within timeout, `false` otherwise.
///
/// @trace REQ-BRW-004 [criterion:4,5,18] worker lifecycle wait
pub fn wait_for_worker_stopped<F>(timeout: Duration, mut is_running: F) -> bool
where
    F: FnMut() -> bool,
{
    wait_for_condition(timeout, || !is_running())
}

/// Wait for a worker to reach a target lifecycle state.
///
/// Domain-specific helper for worker state machine tests. Uses a caller-supplied
/// predicate so it is not coupled to any specific worker handle type.
///
/// # Arguments
/// * `timeout` - Maximum time to wait
/// * `reached` - Closure returning true when the target state is reached
///
/// @trace REQ-BRW-004 [criterion:18] worker state machine wait
pub fn wait_for_worker_state<F>(timeout: Duration, reached: F) -> bool
where
    F: FnMut() -> bool,
{
    wait_for_condition(timeout, reached)
}

/// Wait for a page's JS context to be ready for evaluation.
///
/// Domain-specific helper for page lifecycle tests. Uses a caller-supplied
/// predicate so it is not coupled to any specific page handle type.
///
/// # Arguments
/// * `timeout` - Maximum time to wait
/// * `ready` - Closure returning true when the page context is ready
///
/// @trace REQ-BRW-001 [sm:PageLifecycle] context ready wait
pub fn wait_for_context_ready<F>(timeout: Duration, ready: F) -> bool
where
    F: FnMut() -> bool,
{
    wait_for_condition(timeout, ready)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_wait_for_condition_immediate() {
        let start = Instant::now();
        let result = wait_for_condition(Duration::from_secs(1), || true);
        assert!(result, "immediate true should return true");
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "should not wait"
        );
    }

    #[test]
    fn test_wait_for_condition_timeout() {
        let start = Instant::now();
        let result = wait_for_condition(Duration::from_millis(100), || false);
        assert!(!result, "always false should timeout and return false");
        assert!(
            start.elapsed() >= Duration::from_millis(90),
            "should wait full timeout"
        );
    }

    #[test]
    fn test_wait_for_condition_becomes_true() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = Arc::clone(&flag);

        // Spawn a thread that will set the flag after 50ms
        std::thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            flag_clone.store(true, Ordering::Release);
        });

        let start = Instant::now();
        let result = wait_for_condition(Duration::from_secs(1), || flag.load(Ordering::Acquire));
        assert!(result, "flag should become true");
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(40),
            "should wait at least ~50ms"
        );
        assert!(
            elapsed < Duration::from_millis(300),
            "should not wait too long"
        );
    }
}

// ---------------------------------------------------------------------------
// e157 B3: worker-body encoder (8 inline copies consolidated)
// ---------------------------------------------------------------------------

/// URL-encode a JS worker body for data: URL (minimal percent-encoding).
/// (variants 8dc3ef97 ×7 verbatim + 91748917 ×1 — identical modulo match-arm
/// trailing commas)
pub fn encode_worker_body(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// e204: test-host process reaping discipline (spawn/kill face, REQ-LIB-001)
// ---------------------------------------------------------------------------

/// A child process spawned with test-host reaping guarantees.
///
/// The spawn/kill e2e family (`bao_cli_e2e_tests` / `bao_cli_timeout_e2e_tests`
/// / `webdriver_signal_exit_e2e_tests`) drives the real `bao` binary as a
/// potentially long-lived subprocess (`bao browser` / `bao --webdriver` /
/// runaway `while(true){}`). Pre-fix, every kill path signaled the single
/// direct pid, which left three leak faces (e204 RED: a breach-path panic left
/// `bao --webdriver` orphaned at PPID=1):
///
/// 1. **Panic/breach path** — an assertion between spawn and reap dropped the
///    `Child` without killing: the orphaned engine process outlived the test
///    and pollutes the shared X display for the rest of the battery.
/// 2. **Test-process death** — a SIGKILLed test host (harness kill / OOM)
///    orphaned the child; nothing in the tree had cleanup duty.
/// 3. **Process trees** — a single-pid kill orphaned anything the child itself
///    spawned.
///
/// `GroupChild` closes all three:
///
/// * own process group (`CommandExt::process_group(0)`) → `kill_group`
///   SIGKILLs the whole tree, not just the direct pid;
/// * PDEATHSIG (Linux) → a child whose test host died is SIGKILLed by the
///   kernel, covering the one path `Drop` cannot reach;
/// * reap-on-drop → `kill_group` + `wait()` runs even on the panic path, and
///   is a no-op once the child was already reaped through this handle (the
///   unreaped-zombie state is what pins the pid against reuse, so the
///   `-pid` group signal is only ever sent while it is still ours).
///
/// Every new suite test that spawns a subprocess must go through this type —
/// see the `.config/nextest.toml` process-hygiene note.
// allow(dead_code): `common/mod.rs` is `#[path]`-included by ~30 sibling
// modules (per-instance compilation), and most instances don't construct
// this type — the dead-code lint would otherwise fire per non-using instance
// (same shape as the pre-existing never-used helpers in this file).
#[allow(dead_code)]
pub struct GroupChild {
    child: std::process::Child,
    reaped: bool,
}

#[allow(dead_code)]
impl GroupChild {
    /// Spawn `cmd` with the reaping guarantees documented on the type.
    /// Takes `&mut Command` so builder chains (`Command::new(..).args(..)`)
    /// pass through directly (builder methods return `&mut Command`).
    pub fn spawn(cmd: &mut std::process::Command) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Own process group: makes `kill_group` a whole-tree kill.
            cmd.process_group(0);
            #[cfg(target_os = "linux")]
            {
                let expected_ppid = std::process::id();
                unsafe {
                    cmd.pre_exec(move || {
                        // SAFETY: prctl/kill/getppid are async-signal-safe and
                        // run in our own child between fork and exec.
                        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                        if libc::getppid() as u32 != expected_ppid {
                            // Parent died inside the fork→prctl window:
                            // PDEATHSIG would never fire on its own.
                            libc::kill(libc::getpid(), libc::SIGKILL);
                        }
                        Ok(())
                    });
                }
            }
        }
        Ok(Self {
            child: cmd.spawn()?,
            reaped: false,
        })
    }

    /// The child's pid (which is also its process-group id).
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn stdin(&mut self) -> &mut Option<std::process::ChildStdin> {
        &mut self.child.stdin
    }

    pub fn stdout(&mut self) -> &mut Option<std::process::ChildStdout> {
        &mut self.child.stdout
    }

    pub fn stderr(&mut self) -> &mut Option<std::process::ChildStderr> {
        &mut self.child.stderr
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        let status = self.child.try_wait();
        if matches!(&status, Ok(Some(_))) {
            self.reaped = true;
        }
        status
    }

    pub fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait();
        if status.is_ok() {
            self.reaped = true;
        }
        status
    }

    /// SIGKILL the child's whole process group and reap the direct child.
    /// No-op once the child was already reaped through this handle.
    pub fn kill_group(&mut self) {
        if self.reaped {
            return;
        }
        #[cfg(unix)]
        {
            let pid = self.child.id() as i32;
            if pid > 0 {
                // SAFETY: kill(2) on our own child's process group (-pid).
                // ESRCH (group already gone) is the benign already-exited case.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
        // Reap: no zombie left behind. SIGKILL also terminates a stopped
        // (SIGSTOPped) child, so the breach path reaps unconditionally.
        let _ = self.child.wait();
        self.reaped = true;
    }

    /// Consume the handle, drain piped stdio, and reap
    /// (`Child::wait_with_output` equivalent, same sequential-drain shape).
    pub fn wait_with_output(mut self) -> std::io::Result<std::process::Output> {
        use std::io::Read;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = self.child.stdout.take() {
            let _ = pipe.read_to_end(&mut stdout);
        }
        if let Some(mut pipe) = self.child.stderr.take() {
            let _ = pipe.read_to_end(&mut stderr);
        }
        let status = self.wait()?;
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for GroupChild {
    fn drop(&mut self) {
        self.kill_group();
    }
}
