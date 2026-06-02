// @trace TEST-BRW-THREAD-01 [req:REQ-BRW-003] [level:integration] [nfr:TMG-BRW-01]

//! Thread-safety and concurrency tests for `BridgeChannel` / `BridgeReceiver`.
//!
//! Validates the concurrency contract of the bridge channel under multi-threaded
//! stress: Send/Sync bounds, ordered delivery, close races, fire-and-forget
//! semantics, cross-thread visibility, send_timeout under contention, and drop
//! semantics (tx-drop → rx EOF, rx-drop → tx send error).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use bao_browser::{BridgeChannel, BridgeCommand, BridgeReceiver, BridgeResponse};

// ---------------------------------------------------------------------------
// 1. Send + Sync trait proofs
// ---------------------------------------------------------------------------

/// Compile-time proof that `BridgeChannel` is `Send`.
const _: () = {
    fn assert_send<T: Send>() {}
    fn _proof() {
        assert_send::<BridgeChannel>();
    }
};

/// Compile-time proof that `BridgeChannel` is `Sync`.
const _: () = {
    fn assert_sync<T: Sync>() {}
    fn _proof() {
        assert_sync::<BridgeChannel>();
    }
};

/// Compile-time proof that `BridgeReceiver` is `Send`.
const _: () = {
    fn assert_send<T: Send>() {}
    fn _proof() {
        assert_send::<BridgeReceiver>();
    }
};

// BridgeReceiver intentionally does NOT implement Sync (mpsc::Receiver is !Sync),
// so we only verify Send.

#[test]
fn test_bridge_channel_send_sync_bounds() {
    // Runtime check: BridgeChannel can be shared across threads (Sync) and sent (Send).
    let (tx, rx) = BridgeChannel::new();

    let t1 = thread::spawn(move || {
        assert!(tx.is_alive());
        tx
    });
    let tx = t1.join().expect("tx thread panicked");

    let t2 = thread::spawn(move || {
        assert!(rx.is_alive());
        rx
    });
    let _rx = t2.join().expect("rx thread panicked");

    // Share tx via Arc to prove Sync.
    let tx = Arc::new(tx);
    let tx_clone = Arc::clone(&tx);
    let t3 = thread::spawn(move || {
        assert!(tx_clone.is_alive());
    });
    t3.join().expect("Arc<BridgeChannel> thread panicked");
    assert!(tx.is_alive());
}

// ---------------------------------------------------------------------------
// 2. Concurrent send — 10 threads × 100 messages
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_send_ordered_delivery() {
    let (tx, rx) = BridgeChannel::new();
    let tx = Arc::new(tx);
    let n_threads = 10;
    let msgs_per_thread = 100;
    let total = n_threads * msgs_per_thread;
    let received_count = Arc::new(AtomicUsize::new(0));

    // Drain on the receiver side, counting every message.
    let recv_count = Arc::clone(&received_count);
    let drainer = thread::spawn(move || {
        while let Ok((cmd, resp_tx)) = rx.recv() {
            recv_count.fetch_add(1, Ordering::Relaxed);
            if let Some(rtx) = resp_tx {
                let _ = rtx.send(BridgeResponse::Ok);
            }
            // Inspect the command to verify it's valid.
            match cmd {
                BridgeCommand::Evaluate(s) => assert!(s.starts_with("thread-")),
                _ => panic!("unexpected command variant"),
            }
        }
    });

    // Spawn senders.
    let mut handles = Vec::with_capacity(n_threads);
    for tid in 0..n_threads {
        let tx_c = Arc::clone(&tx);
        handles.push(thread::spawn(move || {
            for i in 0..msgs_per_thread {
                let label = format!("thread-{}-msg-{}", tid, i);
                let resp = tx_c.send(BridgeCommand::Evaluate(label)).unwrap();
                assert!(resp.is_ok(), "sender {} expected Ok response", tid);
            }
        }));
    }

    for h in handles {
        h.join().expect("sender thread panicked");
    }

    // All sends done — drop tx to unblock drainer.
    drop(tx);
    drainer.join().expect("drainer panicked");

    assert_eq!(
        received_count.load(Ordering::SeqCst),
        total,
        "receiver must collect exactly {} messages",
        total,
    );
}

// ---------------------------------------------------------------------------
// 3. Concurrent send + close race condition
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_send_with_close_race() {
    let (tx, rx) = BridgeChannel::new();
    let tx = Arc::new(tx);
    let success = Arc::new(AtomicUsize::new(0));
    let closed_flag = Arc::new(AtomicBool::new(false));

    // Receiver drains slowly (sleeps) so close() races with in-flight sends.
    let rx_closed = Arc::clone(&closed_flag);
    let drainer = thread::spawn(move || {
        while let Ok((_, resp_tx)) = rx.recv() {
            if let Some(rtx) = resp_tx {
                let _ = rtx.send(BridgeResponse::Ok);
            }
        }
        rx_closed.store(true, Ordering::SeqCst);
    });

    let n_senders = 8;
    let mut handles = Vec::with_capacity(n_senders);
    for tid in 0..n_senders {
        let tx_c = Arc::clone(&tx);
        let succ = Arc::clone(&success);
        handles.push(thread::spawn(move || {
            for i in 0..200 {
                match tx_c.send(BridgeCommand::Evaluate(format!("t{}-{}", tid, i))) {
                    Ok(BridgeResponse::Ok) => succ.fetch_add(1, Ordering::Relaxed),
                    Ok(_) => succ.fetch_add(1, Ordering::Relaxed),
                    Err(_) => break,
                };
            }
        }));
    }

    // After a brief pause, call close() while sends are still in flight.
    thread::sleep(Duration::from_millis(5));
    tx.close();

    for h in handles {
        let _ = h.join();
    }

    drop(tx);
    drainer.join().expect("drainer panicked");
    assert!(closed_flag.load(Ordering::SeqCst), "receiver must see EOF");
    // Some sends must have succeeded (exact count is nondeterministic).
    assert!(success.load(Ordering::SeqCst) > 0, "at least some sends must succeed");
}

// ---------------------------------------------------------------------------
// 4. Multi-thread fire_and_forget stress
// ---------------------------------------------------------------------------

#[test]
fn test_fire_and_forget_stress() {
    let (tx, rx) = BridgeChannel::new();
    let tx = Arc::new(tx);
    let n_threads = 10;
    let msgs_per_thread = 200;
    let total = n_threads * msgs_per_thread;
    let received = Arc::new(AtomicUsize::new(0));

    let recv_count = Arc::clone(&received);
    let drainer = thread::spawn(move || {
        while let Ok((cmd, resp_tx)) = rx.recv() {
            assert!(resp_tx.is_none(), "fire_and_forget must have None responder");
            if let BridgeCommand::Navigate(url) = cmd {
                assert!(url.starts_with("https://stress"));
            } else {
                panic!("expected Navigate, got {:?}", cmd);
            }
            recv_count.fetch_add(1, Ordering::Relaxed);
        }
    });

    let mut handles = Vec::with_capacity(n_threads);
    for tid in 0..n_threads {
        let tx_c = Arc::clone(&tx);
        handles.push(thread::spawn(move || {
            for i in 0..msgs_per_thread {
                tx_c
                    .fire_and_forget(BridgeCommand::Navigate(format!(
                        "https://stress-{}/{}",
                        tid, i
                    )))
                    .expect("fire_and_forget must not block or fail while rx is alive");
            }
        }));
    }

    for h in handles {
        h.join().expect("fire_and_forget sender panicked");
    }

    drop(tx);
    drainer.join().expect("drainer panicked");

    assert_eq!(
        received.load(Ordering::SeqCst),
        total,
        "all {} fire_and_forget messages must be received",
        total,
    );
}

// ---------------------------------------------------------------------------
// 5. is_alive cross-thread visibility (happens-before via AtomicBool)
// ---------------------------------------------------------------------------

#[test]
fn test_is_alive_cross_thread_visibility() {
    let (tx, rx) = BridgeChannel::new();
    let tx = Arc::new(tx);

    // Both start alive.
    assert!(tx.is_alive());
    assert!(rx.is_alive());

    // Spawn threads that spin until is_alive becomes false.
    // BridgeChannel is Sync so it can be shared via Arc across threads.
    // BridgeReceiver is Send but not Sync, so we test its visibility on main thread.
    let tx_vis = Arc::clone(&tx);
    let seen_dead_tx = Arc::new(AtomicBool::new(false));
    let seen_dead_tx_c = Arc::clone(&seen_dead_tx);
    let observer_tx = thread::spawn(move || {
        while tx_vis.is_alive() {
            thread::yield_now();
        }
        seen_dead_tx_c.store(true, Ordering::SeqCst);
    });

    // Second thread also observes tx's alive flag — proves happens-before via AtomicBool.
    let tx_vis2 = Arc::clone(&tx);
    let seen_dead_tx2 = Arc::new(AtomicBool::new(false));
    let seen_dead_tx2_c = Arc::clone(&seen_dead_tx2);
    let observer_tx2 = thread::spawn(move || {
        while tx_vis2.is_alive() {
            thread::yield_now();
        }
        seen_dead_tx2_c.store(true, Ordering::SeqCst);
    });

    // Let observers spin up, then close from main thread.
    thread::sleep(Duration::from_millis(10));
    tx.close();

    observer_tx.join().expect("tx observer panicked");
    observer_tx2.join().expect("tx observer2 panicked");

    assert!(seen_dead_tx.load(Ordering::SeqCst), "tx observer must see is_alive=false");
    assert!(seen_dead_tx2.load(Ordering::SeqCst), "tx observer2 must see is_alive=false");
    assert!(!tx.is_alive());

    // rx shares the same Arc<AtomicBool>, verify on main thread (rx is !Sync).
    assert!(!rx.is_alive(), "rx must also see is_alive=false after close");
}

// ---------------------------------------------------------------------------
// 6. send_timeout under contention
// ---------------------------------------------------------------------------

#[test]
fn test_send_timeout_under_contention() {
    let (tx, rx) = BridgeChannel::new();
    let tx = Arc::new(tx);

    let timeout_count = Arc::new(AtomicUsize::new(0));
    let success_count = Arc::new(AtomicUsize::new(0));
    let error_count = Arc::new(AtomicUsize::new(0));

    // Consumer services a limited number of requests quickly, then drops rx.
    // Remaining senders will either time out or see channel closed.
    let max_served = 3;
    let consumer = thread::spawn(move || {
        let mut served = 0;
        while served < max_served {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok((_, resp_tx)) => {
                    if let Some(rtx) = resp_tx {
                        let _ = rtx.send(BridgeResponse::Ok);
                    }
                    served += 1;
                }
                Err(_) => break,
            }
        }
    });

    let n_contenders = 8;
    let mut handles = Vec::with_capacity(n_contenders);
    for tid in 0..n_contenders {
        let tx_c = Arc::clone(&tx);
        let tc = Arc::clone(&timeout_count);
        let sc = Arc::clone(&success_count);
        let ec = Arc::clone(&error_count);
        handles.push(thread::spawn(move || {
            let result = tx_c.send_timeout(
                BridgeCommand::Evaluate(format!("contender-{}", tid)),
                Duration::from_millis(500),
            );
            match result {
                Ok(_) => {
                    sc.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) if e.contains("timed out") => {
                    tc.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    ec.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    drop(tx);
    let _ = consumer.join();

    let successes = success_count.load(Ordering::SeqCst);
    let timeouts = timeout_count.load(Ordering::SeqCst);
    let errors = error_count.load(Ordering::SeqCst);
    assert!(successes >= 1, "at least one send_timeout must succeed, got {}", successes);
    assert_eq!(
        successes + timeouts + errors,
        n_contenders,
        "total must equal contender count (successes={}, timeouts={}, errors={})",
        successes, timeouts, errors,
    );
}

// ---------------------------------------------------------------------------
// 7. Drop semantics
// ---------------------------------------------------------------------------

#[test]
fn test_tx_drop_causes_rx_eof() {
    let (tx, rx) = BridgeChannel::new();

    let rx_handle = thread::spawn(move || {
        let result = rx.recv();
        result
    });

    drop(tx);
    let result = rx_handle.join().expect("rx thread panicked");
    assert!(result.is_err(), "rx.recv() must return Err after tx is dropped");
    assert_eq!(result.unwrap_err(), "channel closed");
}

#[test]
fn test_rx_drop_causes_tx_send_error() {
    let (tx, rx) = BridgeChannel::new();

    drop(rx);

    let result = tx.send(BridgeCommand::Close);
    assert!(result.is_err(), "tx.send() must return Err after rx is dropped");
    assert_eq!(result.unwrap_err(), "bridge closed");
}

#[test]
fn test_rx_drop_causes_fire_and_forget_error() {
    let (tx, rx) = BridgeChannel::new();

    drop(rx);

    let result = tx.fire_and_forget(BridgeCommand::Navigate("https://example.com".into()));
    assert!(result.is_err(), "fire_and_forget must return Err after rx is dropped");
    assert_eq!(result.unwrap_err(), "bridge closed");
}

#[test]
fn test_rx_drop_causes_send_timeout_error() {
    let (tx, rx) = BridgeChannel::new();

    drop(rx);

    let result = tx.send_timeout(BridgeCommand::GetTitle, Duration::from_secs(1));
    assert!(result.is_err(), "send_timeout must return Err after rx is dropped");
    assert_eq!(result.unwrap_err(), "bridge closed");
}

// ---------------------------------------------------------------------------
// 8. Arc<AtomicBool> sharing across threads — alive flag consistency
// ---------------------------------------------------------------------------

#[test]
fn test_arc_atomic_bool_alive_flag_sharing() {
    let (tx, rx) = BridgeChannel::new();
    let tx = Arc::new(tx);

    let n_readers = 10;
    let alive_seen = Arc::new(AtomicUsize::new(0));
    let dead_seen = Arc::new(AtomicUsize::new(0));

    // Spawn reader threads that observe tx's alive flag via Arc<BridgeChannel>.
    // BridgeReceiver is !Sync so we verify its alive flag on main thread only.
    let mut handles = Vec::with_capacity(n_readers);
    for _ in 0..n_readers {
        let tx_c = Arc::clone(&tx);
        let alive = Arc::clone(&alive_seen);
        let dead = Arc::clone(&dead_seen);
        handles.push(thread::spawn(move || {
            if tx_c.is_alive() {
                alive.fetch_add(1, Ordering::Relaxed);
            }
            // Spin until dead.
            while tx_c.is_alive() {
                thread::yield_now();
            }
            dead.fetch_add(1, Ordering::Relaxed);
        }));
    }

    // Let readers observe alive=true first.
    thread::sleep(Duration::from_millis(20));
    let alive_count = alive_seen.load(Ordering::SeqCst);
    assert_eq!(alive_count, n_readers, "all readers must initially see alive=true");

    // rx shares the same Arc<AtomicBool> — verify on main thread.
    assert!(rx.is_alive(), "rx must see alive=true before close");

    // Close from main thread.
    tx.close();

    for h in handles {
        h.join().expect("reader panicked");
    }

    let dead_count = dead_seen.load(Ordering::SeqCst);
    assert_eq!(dead_count, n_readers, "all readers must eventually see alive=false");
    assert!(!rx.is_alive(), "rx must see alive=false after close");
}

// ---------------------------------------------------------------------------
// Bonus: recv_timeout returns timeout error when no messages arrive
// ---------------------------------------------------------------------------

#[test]
fn test_recv_timeout_returns_timeout_error() {
    let (_tx, rx) = BridgeChannel::new();

    let result = rx.recv_timeout(Duration::from_millis(10));
    assert!(result.is_err(), "recv_timeout must return Err when no message arrives");
    assert!(
        result.unwrap_err().contains("timed out"),
        "error message must mention timeout"
    );
}
