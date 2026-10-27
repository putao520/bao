// CdpServer cooperative shutdown: run() was an unconditional infinite loop,
// so a spawned CDP server thread leaked its listener port, registry and
// sessions for the whole process lifetime. Asserts the stop flag gives run()
// a deterministic, bounded exit against a real TcpListener (no mocks).
// @trace REQ-CDS-001 [entity:CdpServer]

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cdp_server::{CdpServer, ServerConfig};

use crate::common::port_probe::retry_probed_port;

fn ephemeral_server_config(port: u16) -> ServerConfig {
    ServerConfig::builder().host("127.0.0.1").port(port).build()
}

#[test]
fn server_run_returns_ok_after_stop_within_bound() {
    // E26 ledger class A (probe-release rebind window): run() binds first,
    // so an early `Err("bind: ... EADDRINUSE ...")` = this attempt lost the
    // port to a parallel suite process after the probe's drop — fresh
    // probed port, retry. Any other failure stays loud.
    retry_probed_port(|port| {
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
        loop {
            match rx.try_recv() {
                Ok(Err(e)) if e.contains("EADDRINUSE") => return None,
                Ok(other) => panic!("cdp server run() ended before bind: {other:?}"),
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("cdp server thread died before binding")
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            assert!(
                Instant::now() < bind_deadline,
                "cdp server did not bind to 127.0.0.1:{port} within 10s"
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
        Some(())
    });
}

#[test]
fn server_run_exits_immediately_when_pre_stopped() {
    // E26 ledger class A: same fresh-probe retry ring as the stop race —
    // run()'s bind is the first thing it does, so EADDRINUSE here is the
    // release-rebind race, not a defect.
    retry_probed_port(|port| {
        let mut server = CdpServer::new(ephemeral_server_config(port));
        // Stop before run(): the loop must observe the flag on its first
        // iteration, finalize (nothing to drain) and return without servicing
        // any connection.
        server.stop();
        let started = Instant::now();
        match server.run() {
            Ok(()) => {
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "pre-stopped run() took {:?}, expected a bounded immediate exit",
                    started.elapsed()
                );
                Some(())
            }
            Err(e) if e.contains("EADDRINUSE") => None,
            Err(e) => panic!("pre-stopped run() failed non-race: {e}"),
        }
    });
}
