// @trace REQ-CLI-002 [level:system]
//
// # e184 — webdriver host process SIGTERM/SIGINT exit E2E
//
// Real-binary subprocess tests (same locator/spawn pattern as
// bao_cli_timeout_e2e_tests): launch `bao --webdriver <port>`, wait for the
// WebDriver HTTP face to listen, deliver the signal, and require the PROCESS
// to be gone within the 5s liveness bound. Pre-fix (e177/e184 RED) the
// registry server's tokio face closed the HTTP listener on TERM but nothing
// wired the signal to bao's pump loop — the process survived indefinitely,
#[cfg(unix)]
// spinning a core (12 residual processes measured in the e177 campaign).
//
//   T1  idle + SIGTERM → exit ≤5s, code 0
//   T2  idle + SIGINT  → exit ≤5s, code 0
//   T3  busy dispatcher + SIGTERM → exit ≤5s (the dispatcher is occupied by
//       an in-flight `get` against a never-responding HTTP server — the e177
//       contention shape; the exit must not depend on the dispatcher)
//
// 运行约束: needs a prebuilt `bao` binary (BAO_BIN / sibling probe); missing →
// skip, not fail.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "common/mod.rs"]
mod common;

/// Whole-teardown liveness bound (mirrors the in-process watchdog constant).
const EXIT_BOUND: Duration = Duration::from_secs(5);
/// Engine bring-up until the WebDriver port listens (cold dev binary is slow;
/// this only bounds the wait, never the assertion).
const LISTEN_WAIT: Duration = Duration::from_secs(90);
/// How long T3 lets the `get` command occupy the dispatcher before signaling.
const BUSY_SETTLE: Duration = Duration::from_millis(1500);

// ─── helpers ────────────────────────────────────────────────────────────────

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
    // Profile-agnostic probe (same finder as bao_cli_e2e_tests): this suite
    // exe sits at $TARGET/<profile>/deps/ — the bao binary sits at
    // $TARGET/<profile>/bao for every cargo profile.
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
            let candidate = PathBuf::from(&target_dir).join(profile).join("bao");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Grab an OS-assigned free port by binding + releasing (the classic probe;
/// the release-to-spawn race is the standard accepted risk for test ports).
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0 for a free port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Spawn `bao --webdriver <port>` with piped stdio, in its own process group
/// with PDEATHSIG armed (e204 reaping discipline — see `common::GroupChild`).
fn spawn_bao_webdriver(port: u16) -> common::GroupChild {
    let bao = bao_path().expect("bao binary not found — run `cargo build -p bao_bin` first");
    common::GroupChild::spawn(
        Command::new(bao)
            .args(["--webdriver", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .expect("spawn bao --webdriver failed")
}

/// Block until the WebDriver port accepts (bounded), so the signal lands on a
/// fully-listening server and not mid-boot.
fn wait_for_listen(port: u16) {
    let start = Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        assert!(
            start.elapsed() < LISTEN_WAIT,
            "webdriver port {port} never came up within {LISTEN_WAIT:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Deliver `signal` and require the child to be reaped within `EXIT_BOUND`.
/// On breach: SIGKILL the whole process group (never leak it into the suite —
/// e204: the kill itself was missing on this path, so pre-fix a breach orphaned
/// the still-running `bao --webdriver` onto the shared display) and fail with
/// its stderr head for forensics.
fn assert_signal_exit(child: &mut common::GroupChild, signal: i32, label: &str) {
    // SAFETY: kill(2) with a live child pid.
    unsafe {
        libc::kill(child.id() as i32, signal);
    }
    let start = Instant::now();
    loop {
        match child.try_wait().expect("try_wait failed") {
            Some(status) => {
                assert!(
                    status.success(),
                    "{label}: child exited within the bound but with {status} (expected clean 0)"
                );
                return;
            }
            None => {
                if start.elapsed() >= EXIT_BOUND {
                    // e204: reap before failing. SIGKILL to the group also
                    // terminates a SIGSTOPped child, so this is unconditional.
                    child.kill_group();
                    panic!(
                        "{label}: process still alive {}s after the signal — the e177 residual-process class (status pending)",
                        EXIT_BOUND.as_secs()
                    );
                    // (the GroupChild drop is then a no-op: already reaped)
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// Minimal raw-HTTP WebDriver client over a throwaway TcpStream
/// (`Connection: close` → read-to-EOF response; no client deps).
fn webdriver_request(port: u16, method: &str, path: &str, body: Option<&str>) -> String {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).expect("connect to webdriver port");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(request.as_bytes())
        .expect("write webdriver request");
    let mut response = String::new();
    // Errors are fine here: T3's in-flight `get` gets its connection torn
    // down when the browser exits — the caller treats that as expected.
    let _ = stream.read_to_string(&mut response);
    response
}

fn webdriver_session(port: u16) -> String {
    let response = webdriver_request(
        port,
        "POST",
        "/session",
        Some(r#"{"capabilities":{}}"#),
    );
    let start = response
        .find("sessionId\":\"")
        .unwrap_or_else(|| panic!("no sessionId in webdriver response: {response:.400}"));
    let rest = &response[start + "sessionId\":\"".len()..];
    rest[..rest.find('"').expect("unterminated sessionId")].to_string()
}

/// A TCP server that accepts and never responds — the e177 contention shape
/// (a page load that hangs until the browser is told to die).
fn spawn_hanging_http_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind hanging server");
    let port = listener.local_addr().expect("local_addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            // Hold every connection open, respond to nothing.
            std::mem::forget(stream);
        }
    });
    port
}

// ─── T1/T2 — idle dual-signal ───────────────────────────────────────────────

#[test]
fn webdriver_sigterm_idle_exits_within_bound() {
    let port = free_port();
    let mut child = spawn_bao_webdriver(port);
    wait_for_listen(port);
    assert_signal_exit(&mut child, libc::SIGTERM, "T1 idle SIGTERM");
}

#[test]
fn webdriver_sigint_idle_exits_within_bound() {
    let port = free_port();
    let mut child = spawn_bao_webdriver(port);
    wait_for_listen(port);
    assert_signal_exit(&mut child, libc::SIGINT, "T2 idle SIGINT");
}

// ─── T3 — busy dispatcher + SIGTERM ─────────────────────────────────────────

#[test]
fn webdriver_sigterm_busy_dispatcher_exits_within_bound() {
    let hang_port = spawn_hanging_http_server();
    let port = free_port();
    let mut child = spawn_bao_webdriver(port);
    wait_for_listen(port);

    // Occupy the serial dispatcher with a `get` that cannot complete: the
    // page load against the never-responding server blocks until the browser
    // dies. The response never arrives on our socket either — expected.
    let session = webdriver_session(port);
    let _in_flight = webdriver_request(
        port,
        "POST",
        &format!("/session/{session}/url"),
        Some(&format!(r#"{{"url":"http://127.0.0.1:{hang_port}/"}}"#)),
    );
    std::thread::sleep(BUSY_SETTLE);

    assert_signal_exit(&mut child, libc::SIGTERM, "T3 busy-dispatcher SIGTERM");
}
