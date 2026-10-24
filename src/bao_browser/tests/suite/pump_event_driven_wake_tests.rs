//! Pump-tail event-driven wake discriminants (REQ-ARCH-003, wave F2a).
//!
//! The three run loops' tail wait changed shape: fixed `sleep(1ms)` poll →
//! bounded park on the servo event queue (`recv_timeout(1ms)`, the deadline
//! is the servo spin cadence contract and never raised). These tests pin the
//! properties the migration must preserve:
//!
//! 1. **Wake** — a cross-thread `send` while the pump is parked resumes the
//!    drain immediately: on a quiet runner send→drain-callback wall time
//!    stays under 2ms over 100 rounds, ~7x faster than the retired poll's
//!    own worst case (measured: park max 61-179µs vs poll max ~1.1ms).
//! 2. **Deadline** — a quiet queue still parks for the full budget (the e26
//!    anti-hot-loop floor; a bare `try_recv` spin would return instantly).
//! 3. **Disconnected degradation** — after the sender side is gone (channel
//!    replaced by an embedder's `set_event_channel`, or teardown) the park
//!    degrades to the plain timed sleep instead of spinning hot on the
//!    instantly-returning dead queue; delivered events are never dropped by
//!    the wake splice.
//!
//! F2c (REQ-ARCH-003): the `run_with_bridge` tail grew a SECOND leg — the CDP
//! bridge command queue. It now parks on BOTH queues through
//! `bao_ipc::Select` and wakes on whichever message lands first:
//!
//! 4. **Bridge wake** — a CDP command landing mid-park is consumed on the
//!    wake path (`BridgeReceiver::process_request`) instead of waiting out
//!    the deadline for the next iteration's head drain (<2ms over 100
//!    rounds, same quiet gate as above).
//! 5. **Disconnected gating** — a dead queue is always instantly ready to
//!    Select, so once a leg surfaces Disconnected it is gated out of later
//!    parks: the dead leg neither spins the tail hot nor masks the live
//!    leg's wake; with both legs dead the tail degrades to the plain timed
//!    sleep.
//!
//! @trace REQ-ARCH-003 [req:REQ-ARCH-003] [level:e2e]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{drain_servo_events, servo_event_pumped_total, BaoServoDelegate};
use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeReceiver, BridgeResponse};
use bao_cdp_client::bridge::{ConsoleLevel, ServoEvent};

const ROUNDS: usize = 100;
const LATENCY_BUDGET_US: u64 = 2_000;

fn console_event(i: usize) -> ServoEvent {
    ServoEvent::Console {
        target_id: "7".to_string(),
        level: ConsoleLevel::Info,
        text: format!("wake-{i}"),
        url: None,
        line: None,
        column: None,
    }
}

/// The discriminant: two symmetric pump faces under identical contention —
/// P = the NEW event-driven park tail (the exact `run_with_bridge` shape:
/// `recv_timeout(1ms)` park, consumed first event spliced into the same
/// drain body), Q = the RETIRED `sleep(1ms)` poll tail (the pre-REQ-ARCH-3
/// shape). A sender feeds both faces one event per round; per-event
/// send→observed wall time is recorded per face.
///
/// Latency contract, scoped by the retired mechanism's own envelope:
///
/// The retired poll face in the SAME experiment is the load meter. Wherever
/// it met its historical worst case (quiet runs measured poll max
/// 1.05-1.14ms), the old code would have met a 2ms budget in that exact
/// window — so the new park MUST too: a red there is a true regression,
/// caught on every quiet runner.
///
/// Under heavier load (parallel-suite windows measured poll max 2.4-10.5ms)
/// the park's per-event wakeup eats raw scheduler wakeup delay that the
/// period-bound poll amortizes away, and NO absolute budget is meaningful
/// for ANY tail shape (the retired mechanism's own max blew past 2ms there
/// too: 2.7-10.5ms measured). The assertion then degrades EXPLICITLY to
/// zero-loss + bounded, never silently — scheduler wakeup delay is the OS's
/// domain, not the channel's contract.
///
/// Load-robust measurement model: every round's send timestamp lives in its
/// OWN slot (indexed by the sequence id carried in the event text), so two
/// events drained in one wake-up burst — which happens when a pump thread is
/// descheduled past a 3ms send spacing — still pair with their own send
/// times. A single shared slot would silently drop the burst followers'
/// latencies and under-count.
#[test]
fn pump_tail_park_wakes_on_cross_thread_send_within_2ms() {
    let (park_max, poll_max) = run_ab_rounds_and_assert_zero_loss();
    let psi_avg10 = cpu_stall_percent();
    // Quiet gate, two independent signals:
    //   - poll max < 1.4ms: the retired mechanism met its historical
    //     envelope in THIS experiment (quiet runs: 1.05-1.14ms);
    //   - PSI `some avg10` < 3%: the CPU itself reports near-no task stalled
    //     for CPU in the last 10s (idle dev box: ~1%; busy cores: 8-16%).
    //     The poll face is period-bound and can hold its ~1.1ms max even
    //     under moderate saturation where the park face already degrades
    //     (busy-run measured: poll max 1.13ms, park max 4.6ms) — the poll
    //     envelope alone has a blind spot there; PSI closes it.
    let quiet = matches!(psi_avg10, None) || psi_avg10.is_some_and(|v| v < 3.0);
    if quiet && poll_max < 1_400 {
        assert!(
            park_max < LATENCY_BUDGET_US,
            "send→drain wake latency must stay under 2ms over {ROUNDS} rounds on a quiet runner \
             (psi {psi_avg10:?}%, retired-poll max {poll_max}µs): park max {park_max}µs"
        );
        println!(
            "wake-latency discriminant (quiet: psi {psi_avg10:?}%, retired-poll max {poll_max}µs): \
             park max {park_max}µs < 2ms budget over {ROUNDS} rounds — the park wakes ~7x faster \
             than the mechanism it replaced"
        );
    } else {
        println!(
            "runner loaded (psi {psi_avg10:?}%, retired-poll max {poll_max}µs): no absolute latency \
             budget is measurable — park max {park_max}µs, asserting zero-loss + bounded only"
        );
        assert!(
            park_max < 50_000,
            "park wake must stay bounded even under load: park max {park_max}µs"
        );
    }
}

/// CPU pressure stall information — `some avg10` percentage (fraction of the
/// last 10s at least SOME task stalled waiting for CPU). Measured on the dev
/// box: ~1.1 idle, 8-16 under a 20-busy-loop burst, higher under the
/// parallel suite. `None` = unmeasurable (PSI disabled or non-Linux —
/// treated as quiet, dev machines).
fn cpu_stall_percent() -> Option<f64> {
    let psi = std::fs::read_to_string("/proc/pressure/cpu").ok()?;
    let line = psi.lines().find(|l| l.starts_with("some "))?;
    line.split_whitespace()
        .find_map(|f| f.strip_prefix("avg10="))?
        .parse::<f64>()
        .ok()
}

/// One full A/B experiment (ROUNDS rounds through both faces). Zero-loss is
/// asserted here — strict in every environment. Returns
/// (park_max_µs, poll_max_µs).
fn run_ab_rounds_and_assert_zero_loss() -> (u64, u64) {
    let (park_tx, park_rx) = bao_ipc::unbounded::<ServoEvent>();
    let (poll_tx, poll_rx) = bao_ipc::unbounded::<ServoEvent>();
    let park_lat: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::with_capacity(ROUNDS)));
    let poll_lat: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::with_capacity(ROUNDS)));
    let park_send_at: Arc<Mutex<Vec<Option<Instant>>>> =
        Arc::new(Mutex::new((0..ROUNDS).map(|_| None).collect()));
    let poll_send_at: Arc<Mutex<Vec<Option<Instant>>>> =
        Arc::new(Mutex::new((0..ROUNDS).map(|_| None).collect()));

    // Pump P — the NEW park tail (run_with_bridge's shape).
    let park_pump = {
        let lat = Arc::clone(&park_lat);
        let send_at = Arc::clone(&park_send_at);
        let rx = park_rx;
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline && lat.lock().unwrap().len() < ROUNDS {
                let first = match rx.recv_timeout(Duration::from_millis(1)) {
                    Ok(servo_event) => Some(servo_event),
                    Err(bao_ipc::RecvTimeoutError::Timeout) => None,
                    Err(bao_ipc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(Duration::from_millis(1));
                        None
                    }
                };
                let observe = |servo_event: ServoEvent| {
                    let ServoEvent::Console { text, .. } = servo_event else {
                        return;
                    };
                    let Ok(i) = text["wake-".len()..].parse::<usize>() else {
                        return;
                    };
                    let mut slots = send_at.lock().unwrap();
                    if let Some(t0) = slots[i].take() {
                        drop(slots);
                        lat.lock().unwrap().push(t0.elapsed().as_micros() as u64);
                    }
                };
                if let Some(servo_event) = first {
                    observe(servo_event);
                }
                drain_servo_events(&rx, observe);
            }
        })
    };

    // Pump Q — the RETIRED poll tail (the pre-REQ-ARCH-3 shape verbatim).
    let poll_pump = {
        let lat = Arc::clone(&poll_lat);
        let send_at = Arc::clone(&poll_send_at);
        let rx = poll_rx;
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline && lat.lock().unwrap().len() < ROUNDS {
                let observe = |servo_event: ServoEvent| {
                    let ServoEvent::Console { text, .. } = servo_event else {
                        return;
                    };
                    let Ok(i) = text["wake-".len()..].parse::<usize>() else {
                        return;
                    };
                    let mut slots = send_at.lock().unwrap();
                    if let Some(t0) = slots[i].take() {
                        drop(slots);
                        lat.lock().unwrap().push(t0.elapsed().as_micros() as u64);
                    }
                };
                while let Ok(servo_event) = rx.try_recv() {
                    observe(servo_event);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };

    // One sender, one round = one event into EACH face (same load both).
    let sender = {
        let park_send_at = Arc::clone(&park_send_at);
        let poll_send_at = Arc::clone(&poll_send_at);
        let park_tx = park_tx.clone();
        std::thread::spawn(move || {
            for i in 0..ROUNDS {
                // Longer than the 1ms deadline: both faces are scheduled to
                // be idle again when the sends land; descheduling bursts are
                // absorbed by the per-round slots.
                std::thread::sleep(Duration::from_millis(3));
                // The slot write happens-before the send, and the send
                // happens-before the pump's receive, so a pump always
                // observes the timestamp of the event it woke on.
                park_send_at.lock().unwrap()[i] = Some(Instant::now());
                poll_send_at.lock().unwrap()[i] = Some(Instant::now());
                let _ = park_tx.send(console_event(i));
                let _ = poll_tx.send(console_event(i));
            }
        })
    };

    park_pump.join().expect("park pump thread");
    poll_pump.join().expect("poll pump thread");
    sender.join().expect("sender thread");

    assert_eq!(
        park_lat.lock().unwrap().len(),
        ROUNDS,
        "every park-face event must be observed exactly once"
    );
    assert_eq!(
        poll_lat.lock().unwrap().len(),
        ROUNDS,
        "every poll-face event must be observed exactly once"
    );
    let park_max = park_lat.lock().unwrap().iter().copied().max().unwrap_or(0);
    let poll_max = poll_lat.lock().unwrap().iter().copied().max().unwrap_or(0);
    (park_max, poll_max)
}

/// A quiet queue parks for the full deadline (the anti-hot-loop floor): the
/// recv_timeout park must NOT return instantly the way a `try_recv` spin
/// does (e26: that shape burned a full core per process).
#[test]
fn pump_tail_park_respects_quiet_deadline() {
    let (_tx, rx) = bao_ipc::unbounded::<ServoEvent>();
    let start = Instant::now();
    let result = rx.recv_timeout(Duration::from_millis(1));
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Err(bao_ipc::RecvTimeoutError::Timeout)),
        "quiet queue must time out, got {result:?}"
    );
    assert!(
        elapsed >= Duration::from_micros(900),
        "quiet park must consume the 1ms deadline, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(100),
        "quiet park must not grossly overshoot the deadline, got {elapsed:?}"
    );
}

/// The delegate park face (`run` / `pump_cdp` tails): an event arriving
/// during the park wakes it and is DELIVERED through the drain (never
/// dropped by the wake splice); a disconnected queue degrades to the plain
/// timed sleep (bounded calls — the hot-loop guard).
#[test]
fn delegate_park_delivers_event_and_disconnected_park_degrades_to_sleep() {
    let delegate = BaoServoDelegate::new();
    let (event_tx, event_rx) = bao_ipc::unbounded::<ServoEvent>();
    delegate.install_runtime_event_drain(event_rx, None);

    // Quiet park consumes the budget (same floor as the raw-queue pin).
    let start = Instant::now();
    delegate.park_runtime_events(Duration::from_millis(15));
    let quiet_elapsed = start.elapsed();
    assert!(
        quiet_elapsed >= Duration::from_millis(10),
        "quiet delegate park must consume the budget, got {quiet_elapsed:?}"
    );

    // Event during the park: immediate wake AND delivery (the splice).
    let pumped_before = servo_event_pumped_total();
    let sender = {
        let tx = event_tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5)); // main thread enters the park
            let _ = tx.send(console_event(0));
        })
    };
    let start = Instant::now();
    delegate.park_runtime_events(Duration::from_millis(50));
    let wake_elapsed = start.elapsed();
    sender.join().expect("sender thread");
    assert_eq!(
        servo_event_pumped_total() - pumped_before,
        1,
        "the park waker must be delivered through the drain"
    );
    assert!(
        wake_elapsed < Duration::from_millis(40),
        "park must wake on the event well before the 50ms budget, got {wake_elapsed:?}"
    );

    // Disconnected: every call must still be bounded by the budget (the
    // degraded sleep), never spin hot on the dead queue.
    drop(event_tx);
    let start = Instant::now();
    for _ in 0..5 {
        delegate.park_runtime_events(Duration::from_millis(2));
    }
    let disconnected_elapsed = start.elapsed();
    assert!(
        disconnected_elapsed >= Duration::from_millis(8),
        "disconnected park must degrade to the timed sleep (anti-hot-loop), got {disconnected_elapsed:?}"
    );
    assert!(
        disconnected_elapsed < Duration::from_millis(200),
        "degraded park must stay bounded, got {disconnected_elapsed:?}"
    );
}

// ─── F2c (REQ-ARCH-003): the two-channel tail (servo events + CDP bridge) ───

/// One iteration of the exact `run_with_bridge` F2c tail shape — the mirror
/// of the crate-private `park_for_event_or_bridge` (`bao_ipc::Select` over
/// the servo event queue and the bridge queue, 1ms budget, per-channel
/// Disconnected gating, wake splice: servo event into the drain, bridge
/// request through `BridgeReceiver::process_request`). The suite harness
/// mirrors the pump verbatim because the helper is pub(crate) — the same
/// convention the F2a tests above use for the single-channel park.
fn park_two_channel_tail(
    servo_rx: &bao_ipc::Receiver<ServoEvent>,
    servo_live: &mut bool,
    bridge_rx: &BridgeReceiver,
    bridge_live: &mut bool,
    budget: Duration,
    servo_wake: &mut usize,
    bridge_wake: &mut usize,
) {
    let mut sel = bao_ipc::Select::new();
    let servo_idx = servo_live.then(|| sel.recv(servo_rx));
    let bridge_idx = bridge_live.then(|| sel.recv(bridge_rx.select_receiver()));
    match sel.select_timeout(budget) {
        Ok(op) => {
            if Some(op.index()) == servo_idx {
                match op.recv(servo_rx) {
                    Ok(_servo_event) => *servo_wake += 1, // splice stand-in: delivered
                    Err(_) => *servo_live = false,        // Disconnected → gate out
                }
            } else if Some(op.index()) == bridge_idx {
                match op.recv(bridge_rx.select_receiver()) {
                    Ok(request) => {
                        *bridge_wake += 1;
                        bridge_rx.process_request(request, |_cmd| BridgeResponse {
                            result: Ok(serde_json::Value::Null),
                        });
                    }
                    Err(_) => *bridge_live = false, // Disconnected → gate out
                }
            }
        }
        Err(bao_ipc::SelectTimeoutError) => {} // both queues quiet: the deadline floor
    }
}

/// Loop state for [`park_two_channel_tail`] — the per-channel liveness flags
/// and the wake counters, exactly the loop-carried state the pump keeps.
struct TwoChannelTail<'a> {
    servo_rx: &'a bao_ipc::Receiver<ServoEvent>,
    bridge_rx: &'a BridgeReceiver,
    servo_live: bool,
    bridge_live: bool,
    servo_wake: usize,
    bridge_wake: usize,
}

impl TwoChannelTail<'_> {
    fn park_once(&mut self, budget: Duration) {
        park_two_channel_tail(
            self.servo_rx,
            &mut self.servo_live,
            self.bridge_rx,
            &mut self.bridge_live,
            budget,
            &mut self.servo_wake,
            &mut self.bridge_wake,
        );
    }
}

/// F2c headline discriminant: a CDP bridge command landing while the tail is
/// parked is consumed on the WAKE path, not by waiting out the 1ms deadline.
/// Same A/B structure as the servo test above — P = the NEW two-channel tail
/// (exact `run_with_bridge` shape), Q = the RETIRED pre-F2c tail (park on
/// the servo queue only; a mid-park bridge command waits out the full
/// deadline for the next iteration's head drain). The bridge sender's
/// blocking `send` IS the measurement: send→response wall time per round.
///
/// Quiet gate identical to the servo test: the retired Q face is the load
/// meter — wherever it held its historical ~1.1ms deadline envelope, the old
/// shape would have met 2ms in that exact window, so the new tail must too.
/// Under load no absolute budget is meaningful for any shape; the assertion
/// degrades explicitly to zero-loss + bounded, never silently.
#[test]
fn pump_tail_bridge_command_wake_within_2ms() {
    let (servo_tx, servo_rx) = bao_ipc::unbounded::<ServoEvent>();
    // Quiet servo leg: the deadline stays the only wake source but the queue
    // must stay CONNECTED (a dead leg would gate itself out mid-experiment).
    let _keep_servo_queue_alive = servo_tx;
    let (p_bridge_tx, p_bridge_rx) = bridge_channel(Duration::from_secs(1));
    let (q_bridge_tx, q_bridge_rx) = bridge_channel(Duration::from_secs(1));

    let p_lat: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::with_capacity(ROUNDS)));
    let q_lat: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::with_capacity(ROUNDS)));
    let p_done = Arc::new(Mutex::new(0usize));
    let q_done = Arc::new(Mutex::new(0usize));

    // Pump P — the NEW two-channel tail (run_with_bridge's F2c shape).
    let p_pump = {
        let servo_rx = servo_rx.clone();
        let p_done = Arc::clone(&p_done);
        std::thread::spawn(move || {
            let mut tail = TwoChannelTail {
                servo_rx: &servo_rx,
                bridge_rx: &p_bridge_rx,
                servo_live: true,
                bridge_live: true,
                servo_wake: 0,
                bridge_wake: 0,
            };
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline && *p_done.lock().unwrap() < ROUNDS {
                tail.park_once(Duration::from_millis(1));
                *p_done.lock().unwrap() = tail.bridge_wake;
            }
        })
    };

    // Pump Q — the RETIRED pre-F2c tail (servo-queue park only; bridge
    // commands drained at the next iteration's head).
    let q_pump = {
        let servo_rx = servo_rx.clone();
        let q_done = Arc::clone(&q_done);
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut head_processed = 0usize;
            while Instant::now() < deadline && head_processed < ROUNDS {
                match servo_rx.recv_timeout(Duration::from_millis(1)) {
                    Ok(_servo_event) => {} // servo leg quiet in this experiment
                    Err(bao_ipc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(bao_ipc::RecvTimeoutError::Timeout) => {}
                }
                head_processed += q_bridge_rx.drain(|_cmd| BridgeResponse {
                    result: Ok(serde_json::Value::Null),
                });
            }
            *q_done.lock().unwrap() = head_processed;
        })
    };

    // One sender, one round = one command into EACH face (same load both).
    // The blocking `send` is the measurement: it returns exactly when the
    // pump consumed the command and routed the response back.
    let sender = {
        let p_lat = Arc::clone(&p_lat);
        let q_lat = Arc::clone(&q_lat);
        std::thread::spawn(move || {
            for _ in 0..ROUNDS {
                // Longer than the 1ms deadline: both tails are parked (idle)
                // again when each send lands.
                std::thread::sleep(Duration::from_millis(3));
                let t0 = Instant::now();
                let resp = p_bridge_tx.send(BridgeCommand::ListTargets);
                if resp.result.is_ok() {
                    p_lat.lock().unwrap().push(t0.elapsed().as_micros() as u64);
                }
                let t0 = Instant::now();
                let resp = q_bridge_tx.send(BridgeCommand::ListTargets);
                if resp.result.is_ok() {
                    q_lat.lock().unwrap().push(t0.elapsed().as_micros() as u64);
                }
            }
        })
    };

    p_pump.join().expect("P (two-channel) pump thread");
    q_pump.join().expect("Q (retired) pump thread");
    sender.join().expect("sender thread");

    // Zero loss: every command must come back Ok inside the 1s response
    // timeout — a missed wake must never become a silently-lost command.
    assert_eq!(
        p_lat.lock().unwrap().len(),
        ROUNDS,
        "every P-face command must be processed"
    );
    assert_eq!(
        q_lat.lock().unwrap().len(),
        ROUNDS,
        "every Q-face command must be processed"
    );
    let p_max = p_lat.lock().unwrap().iter().copied().max().unwrap_or(0);
    let q_max = q_lat.lock().unwrap().iter().copied().max().unwrap_or(0);

    let psi_avg10 = cpu_stall_percent();
    let quiet = matches!(psi_avg10, None) || psi_avg10.is_some_and(|v| v < 3.0);
    if quiet && q_max < 1_400 {
        assert!(
            p_max < LATENCY_BUDGET_US,
            "bridge-command send→process must stay under 2ms over {ROUNDS} rounds on a quiet \
             runner (psi {psi_avg10:?}%, retired-tail max {q_max}µs): two-channel max {p_max}µs"
        );
        println!(
            "bridge-wake discriminant (quiet: psi {psi_avg10:?}%, retired-tail max {q_max}µs): \
             two-channel max {p_max}µs < 2ms budget over {ROUNDS} rounds — the bridge leg rides \
             the same wake path as the servo leg"
        );
    } else {
        println!(
            "runner loaded (psi {psi_avg10:?}%, retired-tail max {q_max}µs): no absolute latency \
             budget is measurable — two-channel max {p_max}µs, asserting zero-loss + bounded only"
        );
        assert!(
            p_max < 50_000,
            "bridge wake must stay bounded even under load: two-channel max {p_max}µs"
        );
    }
}

/// F2c gating discriminants for the two-channel tail:
/// 1. both queues live and quiet → the park consumes the deadline (the
///    anti-hot-loop floor, same as the single-channel pin above);
/// 2. a bridge command mid-park is consumed through `process_request` (the
///    wake path) and counted exactly once;
/// 3. bridge Disconnected → the dead queue is gated out: subsequent parks
///    still bound on the live servo deadline (no hot spin on the
///    instantly-ready dead queue);
/// 4. the servo wake stays IMMEDIATE after the gate (a dead leg must not
///    mask the live leg), the event delivered through the splice;
/// 5. both legs dead → the tail degrades to the plain timed sleep.
#[test]
fn pump_tail_two_channel_park_gating_and_servo_wake_preserved() {
    let (servo_tx, servo_rx) = bao_ipc::unbounded::<ServoEvent>();
    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(1));
    let mut tail = TwoChannelTail {
        servo_rx: &servo_rx,
        bridge_rx: &bridge_rx,
        servo_live: true,
        bridge_live: true,
        servo_wake: 0,
        bridge_wake: 0,
    };

    // 1. Quiet two-channel park consumes the deadline.
    let start = Instant::now();
    tail.park_once(Duration::from_millis(1));
    let quiet_elapsed = start.elapsed();
    assert!(
        quiet_elapsed >= Duration::from_micros(900),
        "quiet two-channel park must consume the deadline, got {quiet_elapsed:?}"
    );
    assert!(
        quiet_elapsed < Duration::from_millis(100),
        "quiet two-channel park must not grossly overshoot the deadline, got {quiet_elapsed:?}"
    );

    // 2. Bridge command mid-park: consumed through the wake path, exactly
    //    once, servo leg untouched.
    let sender = {
        let tx = bridge_tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2)); // main is parked by then
            tx.send_fire_and_forget(BridgeCommand::ListTargets);
        })
    };
    let start = Instant::now();
    while tail.bridge_wake == 0 && start.elapsed() < Duration::from_secs(2) {
        tail.park_once(Duration::from_millis(1));
    }
    sender.join().expect("bridge sender thread");
    assert_eq!(
        tail.bridge_wake, 1,
        "the bridge command must be consumed through the wake path"
    );
    assert_eq!(tail.servo_wake, 0, "the servo leg must stay untouched");

    // 3. Bridge Disconnected → gated out; the tail keeps bounding on the
    //    live servo deadline afterwards.
    drop(bridge_tx);
    let start = Instant::now();
    while tail.bridge_live && start.elapsed() < Duration::from_secs(2) {
        tail.park_once(Duration::from_millis(2));
    }
    assert!(
        !tail.bridge_live,
        "the disconnected bridge must be gated out of later parks"
    );
    let start = Instant::now();
    for _ in 0..5 {
        tail.park_once(Duration::from_millis(2));
    }
    let gated_elapsed = start.elapsed();
    assert!(
        gated_elapsed >= Duration::from_millis(8),
        "post-gate parks must still consume the live servo deadline (anti-hot-loop), \
         got {gated_elapsed:?}"
    );
    assert!(
        gated_elapsed < Duration::from_millis(200),
        "post-gate parks must stay bounded, got {gated_elapsed:?}"
    );

    // 4. Servo wake preserved after the gate: the dead bridge leg must not
    //    mask the live servo leg.
    let sender = {
        let tx = servo_tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(3)); // main is parked by then
            let _ = tx.send(console_event(0));
        })
    };
    let start = Instant::now();
    tail.park_once(Duration::from_millis(50));
    let wake_elapsed = start.elapsed();
    sender.join().expect("servo sender thread");
    assert_eq!(
        tail.servo_wake, 1,
        "the servo event must be delivered through the wake splice"
    );
    assert!(
        wake_elapsed < Duration::from_millis(40),
        "servo wake must stay immediate after the bridge gate, got {wake_elapsed:?}"
    );

    // 5. Both legs dead: degrade to the plain timed sleep (the same floor as
    //    `park_for_event`'s disconnected face).
    drop(servo_tx);
    let start = Instant::now();
    for _ in 0..5 {
        tail.park_once(Duration::from_millis(2));
    }
    let both_dead_elapsed = start.elapsed();
    assert!(
        both_dead_elapsed >= Duration::from_millis(7),
        "all-dead tail must degrade to the timed sleep (anti-hot-loop), got {both_dead_elapsed:?}"
    );
    assert!(
        both_dead_elapsed < Duration::from_millis(500),
        "degraded tail must stay bounded, got {both_dead_elapsed:?}"
    );
}
