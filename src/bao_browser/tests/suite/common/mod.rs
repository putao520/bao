// @trace NFR-TEST-REPRODUCIBILITY [criterion:wait_for_condition]
// Test helper utilities for deterministic wait conditions.
// Replaces magic-number sleep polling with explicit timeout + predicate.

pub mod h2_server;

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
    let exe = std::env::current_exe().expect("current_exe for isolated re-exec");
    let status = std::process::Command::new(exe)
        .arg(test_name)
        .arg("--exact")
        .arg("--test-threads=1")
        .arg("--nocapture")
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
