// @trace TEST-ENG-010-MODULE-KEEPALIVE [req:REQ-ENG-010 REQ-ENG-006] [level:e2e]
// CLI timer keepalive for ESM entry files (Node process-semantics parity).
//
// Root cause: the four `eval_module*` post-eval hook pumps in
// bun_sm/src/module_loader.rs drove the loop `for _ in 0..1000`. With an
// active Bun.serve registered, `drain_and_check` takes the NON-BLOCKING
// `tick_without_idle` branch (~1ms per iteration), so the cap expired in
// ~1.2s and `bao run app.mjs` exited mid-serve — even though a pending
// timer (or the server itself) held the loop alive. The script (.cjs) path
// was already unbounded (`JsContext::eval`'s `loop {}`), making the bug
// module-path-only. Fix: unbounded loop; the hook's own `false` (no pending
// work / process.exit) is the only exit.
//
// These tests drive the REAL production path: `BaoRuntime::eval_module`
// installs `post_eval_drain_then_exit` (the exact `bao run` hook).

use std::time::Instant;

use bao_engine::value::JsValue;

fn eval_str(rt: &mut bun_runtime::BaoRuntime, code: &str) -> String {
    match rt.eval(code, "<keepalive-verify>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

/// Bun.serve + setTimeout: the module eval must stay pumping until the timer
/// fires (pre-fix: the 1000-tick cap expired at ~1.2s and the 2.5s timer's
/// shutdown marker never landed).
#[test]
fn module_server_and_timer_keep_process_alive() {
    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");
    let start = Instant::now();

    rt.eval_module(
        r#"
        const srv = Bun.serve({ port: 0, fetch() { return new Response("ok"); } });
        globalThis.__srvPort = srv.port;
        setTimeout(function () {
            srv.stop();
            globalThis.__marker = 'served';
        }, 2500);
    "#,
        "<keepalive-server.mjs>",
    )
    .expect("module eval must succeed");

    // The 2.5s timer must have fired BEFORE eval_module returned.
    assert!(start.elapsed() >= std::time::Duration::from_millis(2400),
        "module eval must wait for pending timers (elapsed {:?})", start.elapsed());
    assert_eq!(
        eval_str(&mut rt, "String(globalThis.__marker)"),
        "served",
        "server + timer must keep the module loop alive until the timer fires \
         (pre-fix: 1000-tick cap expired in ~1.2s and killed the process early)"
    );
}

/// clearInterval releases the loop: the module eval returns promptly once no
/// timer/server/fetch is pending (guards the fix against becoming a hang).
#[test]
fn module_cleared_interval_releases_loop() {
    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");
    let start = Instant::now();

    rt.eval_module(
        r#"
        let ticks = 0;
        const iv = setInterval(function () { ticks++; }, 25);
        setTimeout(function () {
            clearInterval(iv);
            globalThis.__marker = 'cleared:ticks=' + (ticks > 0);
        }, 400);
    "#,
        "<keepalive-clear.mjs>",
    )
    .expect("module eval must succeed");

    assert_eq!(
        eval_str(&mut rt, "String(globalThis.__marker)"),
        "cleared:ticks=true",
        "interval must fire at least once before being cleared"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(5),
        "cleared interval + no handles must release the loop promptly (elapsed {:?})",
        start.elapsed());
}

/// Explicit process.exit() still ends the loop immediately (exit semantics
/// unchanged by the keepalive fix).
#[test]
fn module_process_exit_unaffected() {
    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");

    rt.eval_module(
        r#"
        const iv = setInterval(function () {}, 25);
        setTimeout(function () {
            globalThis.__marker = 'exit-called';
            process.exit(0);
        }, 200);
    "#,
        "<keepalive-exit.mjs>",
    )
    .expect("module eval must succeed");

    assert_eq!(
        eval_str(&mut rt, "String(globalThis.__marker)"),
        "exit-called",
        "process.exit must terminate the loop with the pending interval still live"
    );
}
