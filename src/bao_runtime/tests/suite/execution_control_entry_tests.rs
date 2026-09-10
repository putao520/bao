// @trace TEST-ENG-001-EXECCTRL-ENTRY [req:REQ-ENG-001 REQ-CLI-001] [level:integration]
//
// SM-EVOLUTION #24/#25 S1 — ExecutionControl wired to the REAL bao_runtime
// script/module entries (`BaoRuntime::eval_with_control` /
// `eval_module_with_control` — the exact bodies `bao -e` / `bao run` use) plus
// the #25 scheduler ordering contract.
//
// Engine discipline: one BaoRuntime per #[test] (nextest runs each test in
// its own process; the JSEngine/JSContext are thread/process singletons).
//
// Contracts under test (ledger S1, 2026-09-10):
//   1. Module entry runaway (`while(true)` in module top level) under a
//      deadline → deterministic TimedOut terminal state, prompt (<5s),
//      deadline-sourced (≥400ms of a 500ms budget), runtime reusable after.
//   2. Script entry runaway killed by an EXTERNAL thread's cancel() →
//      deterministic Cancelled terminal state, prompt.
//   3. Runaway inside a timer callback (the post-eval event-loop pump phase)
//      is terminated too — the control is armed around the WHOLE entry.
//   4. Exit-code boundary contract: a termination surfaces as a real Err
//      from the entry (→ the CLI's unchanged `Err` → exit-code-1 arm,
//      bao_cli/src/cli.rs run_file/run_eval) and NOT as a silent
//      process.exit(0): should_exit() stays false, exit_code() stays 0.
//   5. #25 scheduler ordering: sync body → microtasks (nextTick via
//      queueMicrotask / promise continuations, FIFO by enqueue) → event-loop
//      passes (due bao timers batch-fire, then the job queue drains in the
//      SAME pass) — locked as the deterministic sequence.

use std::time::{Duration, Instant};

use bao_engine::execution_control::TerminalState;
use bao_engine::value::JsValue;

use bun_runtime::BaoRuntime;

fn eval_str(rt: &mut BaoRuntime, code: &str) -> String {
    match rt.eval(code, "<execctrl-verify>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {}", e.message),
    }
}

/// 1. Module entry (the `bao run *.mjs` body) + deadline → TimedOut.
#[test]
fn module_entry_runaway_deadline_deterministic_timeout() {
    let mut rt = BaoRuntime::new().expect("BaoRuntime");
    let ctrl = rt.execution_control();
    let start = Instant::now();
    // try/catch inside the loop: the interrupt termination is UNCATCHABLE —
    // if the engine let JS rescue it, this test would hang to the cap.
    let result = rt.eval_module_with_control(
        &ctrl,
        "while (true) { try { } catch (e) { } }",
        "runaway_timeout.mjs",
        Some(Duration::from_millis(500)),
    );
    let elapsed = start.elapsed();

    let err = result.expect_err("runaway module must be terminated, not completed");
    assert!(
        err.message.contains("deadline"),
        "stable termination error expected, got: {}",
        err.message
    );
    assert_eq!(err.filename, "<execution-control>");
    assert_eq!(ctrl.terminal_state(), TerminalState::TimedOut);
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

    // The runtime (context + persistent realm) survives the termination and
    // is reusable — a killed entry must not poison the next one.
    let after = rt.eval("'alive-' + (40 + 2)", "<after_termination.js>");
    let val = after.expect("post-termination eval must succeed");
    assert_eq!(val.as_string(), Some("alive-42"));
}

/// 2. Script entry (the `bao -e` / CJS `bao run` body) + external-thread
///    cancel() → Cancelled.
#[test]
fn script_entry_runaway_external_thread_cancel() {
    let mut rt = BaoRuntime::new().expect("BaoRuntime");
    let ctrl = rt.execution_control();
    let remote = ctrl.clone();

    // External thread may ONLY submit cancellation (atomic flag + the
    // documented thread-safe interrupt request) — never a JSObject.
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        remote.cancel();
    });

    let start = Instant::now();
    // Generous deadline: if cancel were broken this would hang ~30s and the
    // elapsed assertion below would fail long before that.
    let result = rt.eval_with_control(
        &ctrl,
        "while (true) {}",
        "cancel_runaway.js",
        Some(Duration::from_secs(30)),
    );
    let elapsed = start.elapsed();
    canceller.join().expect("canceller thread must not panic");

    let err = result.expect_err("cancelled runaway script must terminate");
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

    // 4. Exit-code boundary: the termination is a real error out of the
    // entry — NOT a silent process.exit. The CLI maps any entry Err to exit
    // code 1 (its unchanged `Err(_) => Err(1)` arm); asserting
    // should_exit()==false + exit_code()==0 here proves the failure flows
    // through that error arm instead of masquerading as a clean exit 0.
    assert!(
        !bun_runtime::should_exit(),
        "termination must surface as an entry error, not process.exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        0,
        "termination must not steer the process exit code away from the CLI error arm"
    );
}

/// 3. Runaway inside a timer callback — killed during the post-eval
///    event-loop pump (drain_bao_timers' JS dispatch), proving the control
///    is armed around the WHOLE module entry, not just ModuleEvaluate.
#[test]
fn module_entry_timer_callback_runaway_terminated() {
    let mut rt = BaoRuntime::new().expect("BaoRuntime");
    let ctrl = rt.execution_control();
    let start = Instant::now();
    let result = rt.eval_module_with_control(
        &ctrl,
        r#"
        globalThis.__fired = 'armed';
        setTimeout(function () {
            globalThis.__fired = 'in-callback';
            while (true) { }
        }, 25);
    "#,
        "timer_runaway.mjs",
        Some(Duration::from_millis(600)),
    );
    let elapsed = start.elapsed();

    let err = result.expect_err("runaway timer callback must be terminated");
    assert!(
        err.message.contains("deadline"),
        "stable termination error expected, got: {}",
        err.message
    );
    assert_eq!(ctrl.terminal_state(), TerminalState::TimedOut);
    assert!(
        elapsed < Duration::from_secs(5),
        "termination must be prompt, took {:?}",
        elapsed
    );
    // The timer really fired and really entered the runaway loop before the
    // deadline killed it (distinguishes a mid-pump kill from the timer never
    // having run).
    assert_eq!(eval_str(&mut rt, "globalThis.__fired"), "in-callback");
}

/// Normal controlled module entry: completes fast with the right value and
/// does NOT block until its (unused) deadline; the same control is reusable
/// afterwards (reset-on-arm semantics).
#[test]
fn module_entry_controlled_normal_and_control_reuse() {
    let mut rt = BaoRuntime::new().expect("BaoRuntime");
    let ctrl = rt.execution_control();

    let start = Instant::now();
    let val = rt
        .eval_module_with_control(
            &ctrl,
            "globalThis.__x = 6 * 7;",
            "normal.mjs",
            Some(Duration::from_secs(5)),
        )
        .expect("normal module under control must succeed");
    let elapsed = start.elapsed();
    assert_eq!(ctrl.terminal_state(), TerminalState::Completed);
    assert!(
        elapsed < Duration::from_secs(1),
        "fast module must not block until its unused deadline, took {:?}",
        elapsed
    );
    assert_eq!(eval_str(&mut rt, "String(globalThis.__x)"), "42");

    // Reuse the SAME control for a second (timed-out, then normal) run:
    // arm-time reset must clear the previous Completed latch, and a
    // termination must not leak into a subsequent entry.
    let r1 = rt.eval_module_with_control(
        &ctrl,
        "while (true) {}",
        "reuse_runaway.mjs",
        Some(Duration::from_millis(300)),
    );
    assert!(r1.is_err());
    assert_eq!(ctrl.terminal_state(), TerminalState::TimedOut);

    let r2 = rt.eval_module_with_control(
        &ctrl,
        "globalThis.__x = 'second';",
        "reuse_normal.mjs",
        Some(Duration::from_secs(5)),
    );
    r2.expect("post-timeout module must not be polluted by the previous termination");
    assert_eq!(ctrl.terminal_state(), TerminalState::Completed);
    assert_eq!(eval_str(&mut rt, "String(globalThis.__x)"), "second");
}

/// 5. #25 scheduler ordering contract (locked 2026-09-10, ledger S1):
///
///   sync body → microtask checkpoint (RunJobs right after script evaluate:
///   nextTick [enqueued via queueMicrotask], promise continuation,
///   queueMicrotask — FIFO by enqueue time) → event-loop passes
///   (drain_and_check: due bao timers batch-fire, THEN the job queue drains
///   in the SAME pass — a timer callback's promise continuation runs before
///   the next pass).
///
/// Known divergences from Node recorded in the ledger (not changed here):
/// due timers batch-fire before the microtask drain (Node drains between
/// timer callbacks); nextTick shares the microtask FIFO (Node runs a
/// separate, strictly-prior nextTick queue).
#[test]
fn scheduler_ordering_contract_microtasks_before_timers() {
    let mut rt = BaoRuntime::new().expect("BaoRuntime");

    rt.eval(
        r#"
        globalThis.__order = [];
        const o = globalThis.__order;
        process.nextTick(() => o.push('nextTick'));
        Promise.resolve().then(() => o.push('microtask'));
        queueMicrotask(() => o.push('qmt'));
        setTimeout(() => {
            o.push('timer');
            Promise.resolve().then(() => o.push('timer-cont'));
        }, 50);
        o.push('sync');
    "#,
        "<ordering.js>",
    )
    .expect("ordering script must evaluate");

    let order = eval_str(&mut rt, "JSON.stringify(globalThis.__order)");
    assert_eq!(
        order, "[\"sync\",\"nextTick\",\"microtask\",\"qmt\",\"timer\",\"timer-cont\"]",
        "scheduler ordering contract violated"
    );
}
