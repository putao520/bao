// POSIX program content throughout (/bin/sleep): the windows product
// correctly resolves these to ENOENT — cover on unix.
#![cfg(unix)]

// @trace TEST-ENG-005 [req:REQ-ENG-005] [level:integration]
// W33 (#31/#32 tail, W17 slice): a spawned child must start from the
// DEFAULT signal state even when the spawner host runs with hostile signal
// state (SIGTERM/SIGINT SIG_IGN, SIGPIPE blocked) — execve only resets
// CAUGHT handlers, so an inherited SIG_IGN disposition would make
// `child.kill('SIGTERM')` a no-op forever (the W17 open finding; cleanup
// then only converges via the sweep's SIGKILL escalation, 2s+1s).
// The reset is declared at spawn time (POSIX_SPAWN_SETSIGDEF|SETSIGMASK in
// bun_core `posix_spawn_bun`, issue #42) — this test pins it from the
// PUBLIC child_process face, under the hostile-state precondition, with a
// <2s convergence budget (no SIGKILL escalation).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;
use std::time::{Duration, Instant};

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{n}"),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("ERROR: {e:?}"),
    }
}

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Bounded post-eval drain hook (the production CLI pump path — the
/// ChildProcess poll chain is setTimeout-driven; a bare-Rust pump silently
/// drops timer callbacks).
fn bounded_drain_hook(cx: &mut mozjs::context::JSContext) -> bool {
    let exhausted = HOOK_BUDGET.with(|b| {
        let n = b.get();
        if n == 0 {
            return true;
        }
        b.set(n - 1);
        false
    });
    if exhausted {
        return false;
    }
    bun_runtime::timers::drain_and_check(cx)
}

fn wait_until(ctx: &mut JsContext, js_condition: &str, budget: usize) -> bool {
    for _ in 0..60 {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return true;
        }
    }
    false
}

/// Hostile spawner signal state: SIGTERM/SIGINT ignored (SIG_IGN survives
/// execve — the exact inherited state that makes child SIGTERM kills a
/// no-op) and SIGTERM/SIGINT/SIGPIPE blocked in the thread mask (inherited
/// blocked mask). Restored on drop — the suite's own process state is never
/// leaked into.
struct HostileSignalGuard {
    prev_term: usize, // libc::signal's returned handler
    prev_int: usize,
    prev_mask: libc::sigset_t,
}

impl HostileSignalGuard {
    fn install() -> Self {
        unsafe {
            let prev_term = libc::signal(libc::SIGTERM, libc::SIG_IGN);
            let prev_int = libc::signal(libc::SIGINT, libc::SIG_IGN);
            let mut block: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut block);
            libc::sigaddset(&mut block, libc::SIGTERM);
            libc::sigaddset(&mut block, libc::SIGINT);
            libc::sigaddset(&mut block, libc::SIGPIPE);
            // The third parameter receives the PREVIOUS mask — the exact
            // restore target for Drop (SIG_SETMASK).
            let mut prev_mask: libc::sigset_t = std::mem::zeroed();
            let rc = libc::pthread_sigmask(libc::SIG_BLOCK, &block, &raw mut prev_mask);
            assert_eq!(rc, 0, "pthread_sigmask(SIG_BLOCK) failed");
            HostileSignalGuard {
                prev_term: prev_term as usize,
                prev_int: prev_int as usize,
                prev_mask,
            }
        }
    }
}

impl Drop for HostileSignalGuard {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGTERM, self.prev_term as libc::sighandler_t);
            libc::signal(libc::SIGINT, self.prev_int as libc::sighandler_t);
            let _ = libc::pthread_sigmask(
                libc::SIG_SETMASK,
                &self.prev_mask,
                std::ptr::null_mut(),
            );
        }
    }
}

/// SIGTERM (default kill signal) must terminate a spawned child within 2s
/// EVEN WHEN the spawner ran with SIGTERM/SIGINT ignored and blocked — the
/// spawn-time SETSIGDEF|SETSIGMASK reset guarantees the child's default
/// disposition. The W17 failure form: the kill is a no-op and only the
/// sweep's SIGKILL escalation (2s window + kill) ever reaps the child.
#[test]
fn spawn_sigterm_kill_converges_under_hostile_spawner_state() {
    crate::exit_isolation::dispatch_timeout(
        "spawn_sigterm_reset_tests::spawn_sigterm_kill_converges_under_hostile_spawner_state",
        spawn_sigterm_kill_converges_under_hostile_spawner_state_body,
    );
}

fn spawn_sigterm_kill_converges_under_hostile_spawner_state_body() {
    let _hostile = HostileSignalGuard::install();

    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    // Spawn + arm listeners under the hostile state; the child is /bin/sleep
    // 30 — it will NOT exit on its own inside the test window.
    let setup = eval_str(
        &mut ctx,
        r#"
        var cp = require('child_process');
        globalThis.__done = false;
        globalThis.__exit = '';
        var child = cp.spawn('/bin/sleep', ['30']);
        child.on('exit', function(code, signal) {
            globalThis.__exit = code + '/' + signal;
            globalThis.__done = true;
        });
        'spawned:' + (child.pid > 0)
    "#,
    );
    assert!(setup.starts_with("spawned:true"), "spawn failed: {setup}");

    let t0 = Instant::now();
    let kill = eval_str(&mut ctx, "child.kill('SIGTERM'); 'sent'");
    assert_eq!(kill, "sent", "child.kill('SIGTERM') must be sendable");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 50);
    let elapsed = t0.elapsed();

    if !done {
        // Cleanup for a RED run: force-reap so the sleep child never leaks.
        let _ = eval_str(&mut ctx, "child.kill('SIGKILL'); 'killed'");
        let _ = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 50);
    }
    let exit = eval_str(&mut ctx, "globalThis.__exit");

    assert!(
        done,
        "W33: kill('SIGTERM') must terminate the child even under a hostile \
         spawner signal state (elapsed {elapsed:?}, exit={exit:?}) — \
         SIGKILL-escalation-only convergence is the defect form"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "W33: SIGTERM convergence must be < 2s (no SIGKILL escalation path), \
         took {elapsed:?}"
    );
    // The exit event reports the numeric form on this face: code=-1 (Node
    // reports null), signal=15 (SIGTERM). What matters for the reset
    // contract: the child DIED BY SIGTERM, not that it survived to the
    // SIGKILL escalation.
    assert_eq!(
        exit, "-1/15",
        "the exit event must report death by SIGTERM (code -1 / signal 15), got {exit:?}"
    );
    eprintln!("[w33] SIGTERM convergence: {elapsed:?} (exit={exit:?})");
}
