// CdpServer cooperative shutdown: run() was an unconditional infinite loop,
// so a spawned CDP server thread leaked its listener port, registry and
// sessions for the whole process lifetime. Asserts the stop flag gives run()
// a deterministic, bounded exit against a real TcpListener (no mocks).
// @trace REQ-CDS-001 [entity:CdpServer]

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cdp_server::{CdpServer, ServerConfig};

/// Bind 127.0.0.1:0 to reserve an ephemeral port, then release it for the
/// CdpServer to bind (tiny race window, test-only — same pattern as the
/// bao_browser CDP suites).
fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn ephemeral_server_config(port: u16) -> ServerConfig {
    ServerConfig::builder().host("127.0.0.1").port(port).build()
}

#[test]
fn server_run_returns_ok_after_stop_within_bound() {
    let port = pick_free_port();
    let mut server = CdpServer::new(ephemeral_server_config(port));
    let stop = server.stop_handle();

    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(server.run());
    });

    // Bounded wait for the listener bind: a TCP connect succeeds once the
    // run loop's accept() is live (10s deadline absorbs suite CPU load).
    // The probe connection is accepted, read as 0 bytes and dropped —
    // harmless, same as any scanner knock.
    let bind_deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < bind_deadline,
            "cdp server did not bind to 127.0.0.1:{} within 10s",
            port
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    stop.store(true, Ordering::Release);

    // Bounded shutdown: the run loop checks the flag each iteration (10ms
    // cadence), so run() must return well inside the 5s deadline instead
    // of hanging the suite on a regression.
    let result = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("run() returned within 5s of stop");
    handle.join().expect("cdp server thread joined");
    result.expect("run() returns Ok(()) on cooperative stop");
}

#[test]
fn server_run_exits_immediately_when_pre_stopped() {
    let port = pick_free_port();
    let mut server = CdpServer::new(ephemeral_server_config(port));
    // Stop before run(): the loop must observe the flag on its first
    // iteration, finalize (nothing to drain) and return without servicing
    // any connection.
    server.stop();
    let started = Instant::now();
    let result = server.run();
    result.expect("run() returns Ok(()) when pre-stopped");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "pre-stopped run() took {:?}, expected a bounded immediate exit",
        started.elapsed()
    );
}
