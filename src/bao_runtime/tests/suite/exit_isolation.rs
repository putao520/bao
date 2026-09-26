//! Isolation shim for tests whose body ends in the legacy force-exit.
//!
//! Background (suite consolidation, 2026-09-22): these files used to be
//! standalone test binaries. Their body ends with
//! `bun_http::http_thread::shutdown_for_exit()` +
//! `bun_runtime::shutdown_thread_sm()` + `std::process::exit(0)` — in the
//! standalone-binary era that exit(0) WAS the binary's success exit: it
//! terminated a process that deliberately leaves a parked HTTPThread alive
//! (parking avoids ticking freed sockets) and skips the atexit teardown.
//! Inside the consolidated suite harness the same call kills the whole run
//! at the first such test, and the parking is process-global
//! (`SHUTDOWN_REQUESTED` is a static, never reset) — it would poison every
//! later HTTP test sharing the process.
//!
//! The semantics are preserved verbatim by self-re-exec: in harness mode the
//! dispatcher spawns THIS binary again with `--exact <test name>`; the child
//! runs the same body in a fresh process and its force-exit is the child's
//! success exit (exactly what the standalone binary did). The parent asserts
//! the child's exit code — 0 = every in-body assertion held, 101 = a libtest
//! panic (child's failure report is forwarded), anything else = crash/kill.
//! A hung body is killed at a bounded deadline so one wedged scenario cannot
//! wedge the whole run.
//!
//! Under cargo-nextest this adds one redundant process hop per converted
//! test (nextest already runs each test in its own process); behavior is
//! identical, so the suite stays uniform across runners.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEPTH_ENV: &str = "BAO_SUITE_ISOLATION_DEPTH";
const TIMEOUT_ENV: &str = "BAO_SUITE_ISOLATION_TIMEOUT_SECS";
/// Recursion ceiling: depth 0 = harness, 1 = child running the body. A child
/// must never spawn again — past this the env plumbing is broken, so fail
/// instead of forking an infinite chain.
const MAX_DEPTH: u32 = 1;
/// Default ceiling for one force-exit body. Generous: these are full-engine
/// e2e bodies (JS runtime + local HTTP/TLS servers + worker threads).
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// True when this process is an isolation child spawned by [`dispatch`].
pub fn is_isolation_child() -> bool {
    std::env::var_os(DEPTH_ENV).is_some()
}

/// Entry point for a force-exit test: runs `body` appropriately for the
/// current process role. See the module docs for the semantics.
pub fn dispatch(test_name: &str, body: fn()) {
    if is_isolation_child() {
        body();
        // The body's contract is to end in shutdown_for_exit + process::exit.
        // Falling through here would mean the force-exit was dropped and this
        // process (with its parked HTTPThread) is about to keep running the
        // suite — fail loudly instead.
        panic!("{test_name}: body returned without its force-exit — isolation contract broken");
    }
    run_body_isolated(test_name);
}

/// Deadline isolation for tests that can WEDGE (not just fail): harness mode
/// runs the body in a bounded child and asserts its exit code — a hang
/// surfaces as a kill-at-deadline FAIL instead of stalling the whole run.
/// Child mode simply runs the body (normal libtest semantics on return).
/// Take and release the CRT stderr lock — establishes the thread's I/O
/// state and TLS slots that SM's _beginthreadex helper threads depend on
/// (lld-link cross-compile workaround, .200 wire-proven 2026-09-26).
///
/// ROOT CAUSE: in a fresh Win32 process (isolation child via re-exec),
/// SM's JS_NewContext starts ~10 C++ helper threads. These threads use
/// CRT facilities (fprintf, mutexes, TLS) that require the main thread
/// to have initialized CRT I/O first. Without this, the threads crash
/// at KERNELBASE+0xC41CA (mov rcx,[rsp+0xC0] — stack guard page hit).
/// The CLI binary (bao.exe) initializes stdio during normal bring-up;
/// test binaries must do the same before calling into SM.
///
/// The original (unintentional) fix was eprintln! progress checkpoints
/// in test bodies — each write takes/releases the CRT lock and makes
/// a kernel I/O call, which is what actually initializes the state.
#[cfg(windows)]
fn crt_stdio_bringup() {
    use std::io::Write;
    let mut stderr = std::io::stderr();
    // 4 writes ≈ what the instrumented builds did between init steps.
    for i in 0..4u32 {
        let _ = writeln!(stderr, "[isolation] crt-bringup {}", i);
    }
    let _ = stderr.flush();
}

/// Isolation-child context creation with codegen padding — the SM helper
/// thread crash (KERNELBASE+0xC41CA) is layout-dependent: the caller's
/// machine code determines whether JS_NewContext's helper threads start
/// successfully. The eprintln! writes inside this function change the
/// codegen to match the known-good instrumented builds. This is an
/// lld-link cross-compile workaround (tracked as a toolchain issue).
#[cfg(windows)]
#[inline(never)]
pub(crate) fn isolation_create_ctx() -> Result<bao_engine::context::JsContext, bao_engine::error::JsError> {
    eprintln!("[iso-ctx] enter");
    let r = bao_engine::context::JsContext::for_test();
    eprintln!("[iso-ctx] for_test returned");
    r
}

pub fn dispatch_timeout(test_name: &str, body: fn()) {
    if is_isolation_child() {
        #[cfg(windows)]
        crt_stdio_bringup();
        body();
        return;
    }
    run_body_isolated(test_name);
}

fn run_body_isolated(test_name: &str) {
    let depth: u32 = std::env::var(DEPTH_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    assert!(
        depth < MAX_DEPTH,
        "{test_name}: isolation depth {depth} >= {MAX_DEPTH} — env plumbing broken"
    );

    let timeout = std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    let exe = std::env::current_exe().expect("current_exe for isolation re-exec");
    let mut child = Command::new(&exe)
        .args(["--exact", test_name, "--test-threads=1", "--nocapture"])
        .env(DEPTH_ENV, (depth + 1).to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{test_name}: isolation child spawn failed: {e}"));

    // Drain both pipes on threads while waiting: a chatty child must never
    // fill a pipe buffer and wedge (that would mask a healthy test as a
    // timeout kill).
    let mut stdout_pipe = child.stdout.take().expect("child stdout pipe");
    let mut stderr_pipe = child.stderr.take().expect("child stderr pipe");
    let drain = |pipe: &mut dyn Read| -> Vec<u8> {
        let mut buf = Vec::new();
        pipe.read_to_end(&mut buf).unwrap_or(0);
        buf
    };
    let err_handle = std::thread::spawn(move || drain(&mut stderr_pipe));
    let out_handle = std::thread::spawn(move || drain(&mut stdout_pipe));

    let deadline = Instant::now() + Duration::from_secs(timeout);
    let status = loop {
        match child.try_wait().expect("poll isolation child") {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let out = out_handle.join().unwrap_or_default();
                    let err = err_handle.join().unwrap_or_default();
                    forward_child_output(&out, &err);
                    panic!(
                        "{test_name}: isolation child killed at the {timeout}s deadline \
                         (body hung before its force-exit)"
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };

    let out = out_handle.join().unwrap_or_default();
    let err = err_handle.join().unwrap_or_default();
    forward_child_output(&out, &err);

    match status.code() {
        Some(0) => {}
        Some(101) => panic!(
            "{test_name}: isolation child FAILED (assertion panic) — child report above"
        ),
        Some(c) => panic!("{test_name}: isolation child exited {c} — child report above"),
        None => panic!(
            "{test_name}: isolation child terminated by signal ({status}) — child report above"
        ),
    }
}

/// Print the child's captured stdout+stderr. Raw stdout writes bypass
/// libtest's capture machinery, so this lands in the run log even inside a
/// captured test.
fn forward_child_output(out: &[u8], err: &[u8]) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(out);
    if !err.is_empty() {
        let _ = stdout.write_all(b"\n[child stderr]\n");
        let _ = stdout.write_all(err);
    }
    let _ = stdout.flush();
}
