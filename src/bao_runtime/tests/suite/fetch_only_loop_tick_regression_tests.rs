// @trace TEST-ENG-FETCH-LOOP [req:REQ-ENG-004 REQ-ENG-010] [level:e2e]
// Fetch-only CLI wedge regression lock (E6 2026-08-21 class).
//
// A bare fetch script — `fetch(url).then(...)` with NO timers, NO HTTP
// server, NO fs/crypto async: nothing else that pins the event loop — must
// complete and exit. The delivery chain is the fetch liveness probe
// (9c7f8b45): fetch_async registers `has_pending` in node_http's
// LIVENESS_PROBES, so `node_http::has_active_servers()` reports true while
// a fetch is in flight and `timers::drain_and_check`'s has_http branch runs
// `tick_without_idle` each pass — draining the ConcurrentTask queue the
// HTTPThread's `on_http_done` completion lands in. If ANY link breaks
// (probe unregistered, has_active_servers stops consuming probes, the
// has_http branch stops ticking), the wedge class revives: the CLI never
// ticks, connect never progresses, the fetch Promise never settles and
// `bao run` hangs forever (E6 strace: the HTTP client thread spinning
// `epoll_pwait2(6, [], {0,0})` on an empty ready set).
//
// These tests spawn the REAL `bao run` binary on a bare-fetch script and
// assert BOTH delivery (the .then callback runs — "STATUS 200" / "ERR"
// written via writeSync) AND exit (the child terminates within a bound;
// a revived wedge kills at the deadline with a pointed message). Output
// uses fs.writeSync(1, ...) — console.log output is lost when a wedged
// child is killed (E6 harness lesson), so the assertions must not depend
// on buffered stdio.
//
// NOTE: these exec target/debug/bao — the binary must be current or the
// tests measure a stale runtime; an mtime guard over the probe-chain
// sources (fetch_async.rs, node_http.rs, timers.rs) panics with the
// rebuild command instead of reporting misleading data (same contract as
// cluster_isprimary_race_regression_tests).

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Locate the bao binary built in this workspace (debug preferred).
fn find_bao_binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("target");
    for profile in ["debug", "release"] {
        let candidate = target.join(profile).join("bao");
        if candidate.exists() {
            // Staleness guard: the fetch-only tick chain lives in
            // fetch_async.rs (probe registration), node_http.rs (probe
            // consumption) and timers.rs (the has_http tick branch). A
            // binary older than any of them measures the previous
            // behavior, not the current chain.
            let bin_mtime = candidate.metadata().and_then(|m| m.modified()).ok();
            for src in ["src/fetch_async.rs", "src/node_http.rs", "src/timers.rs"] {
                let src_mtime = manifest.join(src).metadata().and_then(|m| m.modified()).ok();
                if let (Some(b), Some(s)) = (bin_mtime, src_mtime) {
                    assert!(
                        b >= s,
                        "stale bao binary ({:?} older than {}) — rebuild with \
                         `cargo build -p bao_bin` before running this regression",
                        candidate,
                        src
                    );
                }
            }
            return candidate;
        }
    }
    panic!("bao binary not found under {:?} — build with `cargo build -p bao_bin`", target);
}

/// Bounded wait for the child, asserting it EXITS (the wedge's defining
/// symptom is a hang). Kills at the deadline with the diagnosis.
fn wait_for_exit(mut child: std::process::Child, what: &str) -> (std::process::ExitStatus, String) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .expect("try_wait on spawned bao run child")
        {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "fetch-only wedge reproduced ({what}): `bao run` with a bare fetch script \
                 (no timers / servers / fs-crypto pins) did not exit within 30s — the \
                 fetch liveness-probe chain (fetch_async::has_pending → node_http \
                 LIVENESS_PROBES → drain_and_check has_http tick branch) is broken"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        stdout.push_str(&String::from_utf8_lossy(&buf));
    }
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf);
        stderr.push_str(&String::from_utf8_lossy(&buf));
    }
    let _ = stderr; // surfaced via the exit-status assertion below
    (status, stdout)
}

/// Accept-loop HTTP origin: one 200 per request, `Connection: close`. Runs
/// until `stop`; non-blocking accept + poll so the test can end it cleanly.
fn spawn_close_server(listener: TcpListener, stop: Arc<AtomicBool>) {
    listener
        .set_nonblocking(true)
        .expect("set_nonblocking on test origin listener");
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut conn, _)) => {
                    // Linux: the accepted fd inherits O_NONBLOCK from the
                    // listener — restore blocking + a read bound so a
                    // slow/malformed request can't park the origin thread.
                    conn.set_nonblocking(false).ok();
                    conn.set_read_timeout(Some(Duration::from_secs(2))).ok();
                    let mut buf = [0u8; 4096];
                    let mut got = Vec::new();
                    loop {
                        match conn.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                got.extend_from_slice(&buf[..n]);
                                if got.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break; // full request head — enough for a GET
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let body = b"wedge-guard-origin";
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = conn.write_all(head.as_bytes());
                    let _ = conn.write_all(body);
                    let _ = conn.shutdown(Shutdown::Both);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
}

/// Write `source` to a unique temp file and return its path.
fn write_script(name: &str, source: &str) -> PathBuf {
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "bao_fetch_only_wedge_{}_{}_{}.js",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, source).expect("write fetch-only wedge script");
    path
}

#[test]
fn fetch_only_script_delivers_and_exits_without_loop_pins() {
    // Bare fetch against a local origin — no setTimeout, no HTTP server,
    // no fs/crypto async: NOTHING else pins the loop. If the fetch liveness
    // probe chain breaks, this child hangs (killed at the 30s deadline).
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral origin");
    let port = listener.local_addr().expect("origin local addr").port();
    let stop = Arc::new(AtomicBool::new(false));
    spawn_close_server(listener, stop.clone());

    let script = write_script(
        "happy",
        &format!(
            r#"fetch("http://127.0.0.1:{port}/").then((r) => {{
  require("fs").writeSync(1, "STATUS " + r.status + "\n");
  process.exit(0);
}}).catch((e) => {{
  require("fs").writeSync(1, "ERR " + e + "\n");
  process.exit(1);
}});
"#
        ),
    );

    let bao = find_bao_binary();
    let child = Command::new(&bao)
        .arg("run")
        .arg(&script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bao run fetch-only script");

    let (status, stdout) = wait_for_exit(child, "happy path");
    stop.store(true, Ordering::Relaxed);
    let _ = std::fs::remove_file(&script);

    // Double assertion — delivery AND clean exit:
    assert!(status.success(), "bao run fetch-only exited {status}: {stdout:?}");
    assert!(
        stdout.contains("STATUS 200"),
        "fetch .then callback not delivered (wedge class): stdout={stdout:?}"
    );
}

#[test]
fn fetch_only_refused_connection_rejects_and_exits() {
    // The refused twin (E6: `fetch('http://127.0.0.1:9/')` hung forever
    // pre-probe): rejection delivery rides the same probe-driven tick —
    // the HTTPThread fails the connect and its on_http_done completion
    // must reach the JS thread to reject the Promise.
    let script = write_script(
        "refused",
        r#"fetch("http://127.0.0.1:1/").then((r) => {
  require("fs").writeSync(1, "STATUS " + r.status + "\n");
  process.exit(0);
}).catch((e) => {
  require("fs").writeSync(1, "ERR " + e + "\n");
  process.exit(0);
});
"#,
    );

    let bao = find_bao_binary();
    let child = Command::new(&bao)
        .arg("run")
        .arg(&script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bao run fetch-only refused script");

    let (status, stdout) = wait_for_exit(child, "refused path");
    let _ = std::fs::remove_file(&script);

    assert!(
        status.success(),
        "bao run fetch-only (refused) exited {status}: {stdout:?}"
    );
    assert!(
        stdout.contains("ERR"),
        "fetch rejection not delivered (wedge class): stdout={stdout:?}"
    );
}
