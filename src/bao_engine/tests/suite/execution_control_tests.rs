// @trace TEST-ENG-001-EXECCTRL [req:REQ-ENG-001] [level:integration]
//
// SM-EVOLUTION #24 — interrupt/cancellation minimal closed loop.
//
// All SpiderMonkey-dependent assertions run within a single #[test] function
// (ENGINE_HANDLE is thread-local; Rust's harness gives each #[test] its own
// thread — same discipline as job_queue_context_tests.rs). One JsContext is
// created once and reused across the sub-tests.
//
// Contract under test (SM-EVOLUTION S0-A / #24 slice):
//   1. `while(true){}` under a ≤5s deadline reaches a deterministic TimedOut
//      terminal state — no crash, no hang — with an uncatchable termination
//      (the loop body's try/catch cannot rescue it).
//   2. Normal eval is unaffected: completes, correct value, does NOT block
//      until its (unused) deadline, and plain no-control eval keeps working.
//   3. After a timed-out execution, a reset + second eval is not polluted by
//      the previous termination (latch cleared, context still usable).
//   4. External-thread cancel() terminates a runaway loop → Cancelled state.

use bao_engine::context::JsContext;
use bao_engine::execution_control::{ExecutionControl, TerminalState};

use std::time::{Duration, Instant};

fn test_01_timeout_terminates_runaway_loop(ctx: &mut JsContext) {
    let ctrl = ExecutionControl::new();
    let start = Instant::now();
    // try/catch inside the loop: the interrupt termination is UNCATCHABLE —
    // if the engine let JS catch it, this test would hang to the harness cap.
    let result = ctx.eval_with_control(
        &ctrl,
        "while (true) { try { } catch (e) { } }",
        "runaway_timeout.js",
        Some(Duration::from_millis(500)),
    );
    let elapsed = start.elapsed();

    let err = result.expect_err("runaway loop must be terminated, not completed");
    assert!(
        err.message.contains("deadline"),
        "stable termination error expected, got: {}",
        err.message
    );
    assert_eq!(err.filename, "<execution-control>");
    assert_eq!(ctrl.terminal_state(), TerminalState::TimedOut);
    // Deterministic: terminated by the ~500ms deadline, well inside the ≤5s
    // contract bound — a hang or a busy spin past the deadline fails here.
    assert!(
        elapsed < Duration::from_secs(5),
        "termination must be prompt, took {:?}",
        elapsed
    );
    assert!(
        elapsed >= Duration::from_millis(400),
        "termination must come from the deadline, not an early abort: {:?}",
        elapsed
    );
}

fn test_02_normal_eval_unaffected(ctx: &mut JsContext) {
    // Controlled eval with a generous deadline completes with the right value
    // and — critically — returns immediately instead of blocking until the
    // deadline (the watcher is condvar-cancelled by the eval frame).
    let ctrl = ExecutionControl::new();
    let start = Instant::now();
    let val = ctx
        .eval_with_control(&ctrl, "6 * 7", "normal.js", Some(Duration::from_secs(5)))
        .expect("normal eval under control must succeed");
    let elapsed = start.elapsed();

    assert_eq!(val.as_number(), Some(42.0));
    assert_eq!(ctrl.terminal_state(), TerminalState::Completed);
    assert!(
        elapsed < Duration::from_secs(1),
        "fast eval must not block until its unused deadline, took {:?}",
        elapsed
    );

    // A JS error under control latches Errored (not a control termination)
    // and still surfaces the real message.
    let ctrl_err = ExecutionControl::new();
    let err = ctx
        .eval_with_control(&ctrl_err, "throw new Error('boom')", "normal_err.js", None)
        .expect_err("JS error must propagate");
    assert!(err.message.contains("boom"), "got: {}", err.message);
    assert_eq!(ctrl_err.terminal_state(), TerminalState::Errored);

    // Plain eval with NO control on the same context keeps working (callback
    // continues execution when nothing is armed).
    let plain = ctx.eval("'p' + 'lain'", "plain.js");
    assert!(plain.is_ok());
}

fn test_03_reset_prevents_pollution_of_next_eval(ctx: &mut JsContext) {
    let ctrl = ExecutionControl::new();

    // First execution: times out on a runaway loop.
    let r1 = ctx.eval_with_control(
        &ctrl,
        "while (true) {}",
        "pollute_runaway.js",
        Some(Duration::from_millis(300)),
    );
    assert!(r1.is_err());
    assert_eq!(ctrl.terminal_state(), TerminalState::TimedOut);

    // Reset: stale latch/cancel cleared, next execution starts pristine.
    ctrl.reset();
    assert_eq!(ctrl.terminal_state(), TerminalState::Running);

    // Second execution with the SAME control: must complete correctly —
    // neither the previous TimedOut latch nor a late interrupt from eval 1
    // may terminate it.
    let r2 = ctx.eval_with_control(
        &ctrl,
        "'ok-' + (40 + 2)",
        "pollute_after.js",
        Some(Duration::from_secs(5)),
    );
    let val = r2.expect("post-reset eval must not be polluted by the previous timeout");
    assert_eq!(val.as_string(), Some("ok-42"));
    assert_eq!(ctrl.terminal_state(), TerminalState::Completed);

    // And an uncontrolled eval after a termination is unaffected too.
    let plain = ctx.eval("[1, 2, 3].length", "post_termination_plain.js");
    assert!(plain.is_ok());
}

fn test_04_external_thread_cancel(ctx: &mut JsContext) {
    let ctrl = ExecutionControl::new();
    let remote = ctrl.clone();

    // External thread may ONLY submit cancellation (atomic + documented
    // thread-safe interrupt request) — never touches a JSObject.
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        remote.cancel();
    });

    let start = Instant::now();
    // Generous deadline: if cancel were broken this would hang ~30s and the
    // elapsed assertion below would fail long before that.
    let result = ctx.eval_with_control(
        &ctrl,
        "while (true) {}",
        "cancel_runaway.js",
        Some(Duration::from_secs(30)),
    );
    let elapsed = start.elapsed();
    canceller.join().expect("canceller thread must not panic");

    let err = result.expect_err("cancelled runaway loop must terminate");
    assert!(
        err.message.contains("cancelled"),
        "stable cancellation error expected, got: {}",
        err.message
    );
    assert_eq!(ctrl.terminal_state(), TerminalState::Cancelled);
    assert!(
        elapsed < Duration::from_secs(5),
        "cancellation must be prompt, took {:?}",
        elapsed
    );
}

#[test]
fn test_execution_control_all() {
    let mut ctx = JsContext::for_test().expect("Failed to create JsContext");

    test_01_timeout_terminates_runaway_loop(&mut ctx);
    test_02_normal_eval_unaffected(&mut ctx);
    test_03_reset_prevents_pollution_of_next_eval(&mut ctx);
    test_04_external_thread_cancel(&mut ctx);

    bao_engine::context::JsContext::shutdown_thread_sm();
}
