//! @trace REQ-ARCH-3
//!
//! Discriminating tests for the bao_ipc MPMC substrate contract: MPMC
//! no-loss / no-dup delivery, bounded backpressure, close semantics, clone
//! semantics, `Send + Sync` handles.

use std::collections::HashSet;
use std::thread;

use bao_ipc::{Receiver, RecvError, Sender, TrySendError, channel, unbounded};

/// Values are tagged with the producer id so cross-producer collisions are
/// impossible: `(producer << 32) | seq`.
fn tagged(producer: usize, seq: usize) -> u64 {
    ((producer as u64) << 32) | (seq as u64)
}

fn expected_set(producers: usize, per_producer: usize) -> HashSet<u64> {
    (0..producers)
        .flat_map(|p| (0..per_producer).map(move |s| tagged(p, s)))
        .collect()
}

/// Drives one MPMC round: `producers` threads each push `per_producer` unique
/// tagged values through their own sender; `consumers` threads race on clones
/// of `rx`. Returns every value received across all consumers.
fn run_mpmc(
    tx: Vec<Sender<u64>>,
    rx: Receiver<u64>,
    per_producer: usize,
    consumers: usize,
) -> HashSet<u64> {
    let consumer_handles: Vec<_> = (0..consumers)
        .map(|_| {
            let rx = rx.clone();
            thread::spawn(move || {
                let mut got = Vec::new();
                while let Ok(v) = rx.recv() {
                    got.push(v);
                }
                got
            })
        })
        .collect();
    // Our own handle must not keep the channel open past the clones.
    drop(rx);

    let producer_handles: Vec<_> = tx
        .into_iter()
        .enumerate()
        .map(|(p, tx)| {
            thread::spawn(move || {
                for s in 0..per_producer {
                    tx.send(tagged(p, s)).expect("send while receivers alive");
                }
            })
        })
        .collect();
    for h in producer_handles {
        h.join().expect("producer thread must not panic");
    }
    // All senders are gone now; consumers drain the buffer and observe the
    // close, which ends their recv loops.
    let mut received = HashSet::new();
    for h in consumer_handles {
        for v in h.join().expect("consumer thread must not panic") {
            received.insert(v);
        }
    }
    received
}

#[test]
fn mpmc_bounded_four_producers_four_consumers_no_loss_no_dup() {
    const PRODUCERS: usize = 4;
    const CONSUMERS: usize = 4;
    const PER_PRODUCER: usize = 1000;
    // Buffer far below the message count so producers spend real time in the
    // blocked-send / woken-by-consumer paths under contention.
    let (tx, rx) = channel::<u64>(64);
    let senders: Vec<Sender<u64>> = (0..PRODUCERS).map(|_| tx.clone()).collect();
    drop(tx);

    let received = run_mpmc(senders, rx, PER_PRODUCER, CONSUMERS);
    let expected = expected_set(PRODUCERS, PER_PRODUCER);
    assert_eq!(
        received.len(),
        PRODUCERS * PER_PRODUCER,
        "each message must be received exactly once (no duplicates)"
    );
    assert_eq!(received, expected, "no message may be lost");
}

#[test]
fn mpmc_unbounded_four_producers_four_consumers_no_loss_no_dup() {
    const PRODUCERS: usize = 4;
    const CONSUMERS: usize = 4;
    const PER_PRODUCER: usize = 1000;
    let (tx, rx) = unbounded::<u64>();
    let senders: Vec<Sender<u64>> = (0..PRODUCERS).map(|_| tx.clone()).collect();
    drop(tx);

    let received = run_mpmc(senders, rx, PER_PRODUCER, CONSUMERS);
    let expected = expected_set(PRODUCERS, PER_PRODUCER);
    assert_eq!(received.len(), PRODUCERS * PER_PRODUCER);
    assert_eq!(received, expected);
}

#[test]
fn bounded_capacity_one_backpressure_pins_try_send_full() {
    // Pinned discriminating choice (documented on `channel`): backpressure is
    // asserted on the deterministic `try_send` path; blocking `send` while
    // full is crossbeam's own contract and not re-tested here.
    let (tx, rx) = channel::<u64>(1);
    tx.send(1).expect("buffer has room for the first message");
    match tx.try_send(2) {
        Err(TrySendError::Full(2)) => {}
        other => panic!("expected Full(2) at capacity, got {other:?}"),
    }
    assert_eq!(rx.recv(), Ok(1), "the buffered message is delivered");
    tx.try_send(2)
        .expect("capacity is free again after the drain");
    assert_eq!(rx.recv(), Ok(2));
}

#[test]
fn zero_capacity_channel_is_rendezvous_handoff() {
    let (tx, rx) = channel::<u64>(0);
    let h = thread::spawn(move || tx.send(7).expect("receiver is waiting"));
    assert_eq!(rx.recv(), Ok(7), "rendezvous handoff delivers the value");
    h.join().expect("sender thread must not panic");
}

#[test]
fn close_after_all_senders_dropped_recv_returns_err() {
    let (tx, rx) = unbounded::<u64>();
    let tx2 = tx.clone();
    tx.send(1).expect("receivers alive");
    tx2.send(2).expect("receivers alive");
    drop(tx);
    // One sender clone still alive: the channel stays open and delivers.
    assert_eq!(rx.recv(), Ok(1));
    assert_eq!(rx.recv(), Ok(2));
    drop(tx2);
    // Drained + every sender gone -> recv errors.
    assert_eq!(rx.recv(), Err(RecvError));
    // Receiver clones observe the same closed state.
    let rx2 = rx.clone();
    assert_eq!(rx2.recv(), Err(RecvError));

    // Bounded channels share the drain-then-error close contract.
    let (btx, brx) = channel::<u64>(2);
    btx.send(9).expect("receivers alive");
    drop(btx);
    assert_eq!(brx.recv(), Ok(9), "buffered messages survive the close");
    assert_eq!(brx.recv(), Err(RecvError));
}

#[test]
fn send_after_all_receivers_dropped_is_send_error() {
    let (tx, rx) = unbounded::<u64>();
    drop(rx);
    match tx.send(5) {
        Err(e) => assert_eq!(e.0, 5, "SendError carries the rejected message"),
        Ok(()) => panic!("send must fail once every receiver is dropped"),
    }
}

#[test]
fn clones_share_one_queue_single_delivery_per_message() {
    let (tx, rx) = unbounded::<u64>();
    let tx2 = tx.clone();
    let rx2 = rx.clone();
    let p1 = thread::spawn(move || {
        for s in 0..100u64 {
            tx.send(s).expect("receivers alive");
        }
    });
    let p2 = thread::spawn(move || {
        for s in 100..200u64 {
            tx2.send(s).expect("receivers alive");
        }
    });
    let c1 = thread::spawn(move || {
        let mut got = Vec::new();
        while let Ok(v) = rx.recv() {
            got.push(v);
        }
        got
    });
    let c2 = thread::spawn(move || {
        let mut got = Vec::new();
        while let Ok(v) = rx2.recv() {
            got.push(v);
        }
        got
    });

    p1.join().expect("producer 1");
    p2.join().expect("producer 2");
    let mut all = c1.join().expect("consumer 1");
    all.extend(c2.join().expect("consumer 2"));
    all.sort_unstable();
    let expected: Vec<u64> = (0..200).collect();
    assert_eq!(
        all, expected,
        "sender clones keep the channel open; receiver clones split the \
         stream with exactly one delivery per message"
    );
}

#[test]
fn channel_handles_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Sender<u64>>();
    assert_send_sync::<Receiver<u64>>();
}
