// @trace TEST-ENG-005 [req:REQ-ENG-005] [level:integration]
// child_process.spawn EventEmitter surface. cp_spawn returns a plain data
// object — `child.on` was undefined and the CP_JS ChildProcess wrapper was
// stored but never used (filed from the bbe20a81 sweep). The JS-layer wrap
// (cp.spawn → new ChildProcess(native)) gives every child the full event
// surface: on/once/emit, 'exit'/'close', stdout/stderr 'data' streams, kill.
// Native FFI untouched.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Bounded post-eval drain hook (the production CLI pump path — see
/// net_echo_e2e_tests for why a bare-Rust pump silently drops timer
/// callbacks): the ChildProcess poll chain is setTimeout-driven.
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

/// Full event lifecycle: real /bin/sh child, stdout 'data' bytes, 'exit'
/// with (code, signal=null), 'close' after exit — all actually fired, and
/// the child fully REAPED (no zombie residue: the exit event firing proves
/// waitpid ran; /proc/<pid> disappearing proves the Z state never lingered).
#[test]
fn child_process_spawn_event_lifecycle() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var cp = require('child_process');
        var log = [];
        globalThis.__done = false;

        var child = cp.spawn('/bin/sh', ['-c', 'echo hi; exit 7']);

        // EventEmitter surface present on the RETURNED object.
        log.push('on=' + (typeof child.on === 'function'));
        log.push('once=' + (typeof child.once === 'function'));
        log.push('emit=' + (typeof child.emit === 'function'));
        log.push('kill=' + (typeof child.kill === 'function'));
        log.push('stdin=' + (typeof child.stdin === 'object'));
        log.push('pid=' + (child.pid > 0));

        var chunks = [];
        child.stdout.on('data', function(d) {
            chunks.push(String.fromCharCode.apply(null, new Uint8Array(d)));
        });
        child.on('exit', function(code, signal) {
            log.push('exit=' + code + '/' + signal);
        });
        child.on('close', function(code, signal) {
            log.push('close=' + code + '/' + signal);
            log.push('stdout=' + chunks.join(''));
            log.push('exitCode=' + child.exitCode);
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join('|'); };
        'setup-ok'
    "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 50);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "child lifecycle must complete; log: {log}");

    for part in [
        "on=true",
        "once=true",
        "emit=true",
        "kill=true",
        "stdin=true",
        "pid=true",
        "exit=7/null",
        "close=7/null",
        "stdout=hi\n",
        "exitCode=7",
    ] {
        assert!(
            log.split('|').any(|entry| entry == part),
            "lifecycle log must contain '{part}', got: {log}"
        );
    }

    // No zombie residue: the 'close' event firing proves waitpid ran inside
    // pipe_poll_thread; the /proc entry being GONE (not state Z) proves the
    // child was reaped rather than left zombifying — the deadlock's signature.
    let pid_str = eval_str(&mut ctx, "String(child.pid)");
    let pid: i32 = pid_str.parse().expect("pid parse");
    let proc_path = std::path::PathBuf::from(format!("/proc/{pid}"));
    let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.split_whitespace().nth(2).map(|st| st == "Z"))
        .unwrap_or(false);
    assert!(
        !proc_path.exists() || !zombie,
        "child must be reaped after exit (no Z zombie residue); /proc/{pid} state: {:?}",
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.split_whitespace().nth(2).map(str::to_string))
    );
}

/// once() semantics and stderr delivery: a once('exit') listener fires
/// exactly once; stderr 'data' reaches its stream; listener throw-isolation.
#[test]
fn child_process_spawn_once_stderr_and_throw_isolation() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var cp = require('child_process');
        var log = [];
        globalThis.__done = false;

        var child = cp.spawn('/bin/sh', ['-c', 'echo boom 1>&2; exit 0']);
        var exitFires = 0;
        child.once('exit', function() { exitFires++; });
        child.on('exit', function() {
            // Throwing listener must not block later listeners (the close
            // handler below still runs and completes the test).
            throw new Error('listener throw must be isolated');
        });
        var errChunks = [];
        child.stderr.on('data', function(d) {
            errChunks.push(String.fromCharCode.apply(null, new Uint8Array(d)));
        });
        child.on('close', function() {
            log.push('exitFires=' + exitFires);
            log.push('stderr=' + errChunks.join(''));
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join('|'); };
        'setup-ok'
    "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 50);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "stderr lifecycle must complete; log: {log}");
    assert!(
        log.split('|').any(|e| e == "exitFires=1"),
        "once('exit') must fire exactly once, got: {log}"
    );
    assert!(
        log.split('|').any(|e| e == "stderr=boom\n"),
        "stderr data must be delivered, got: {log}"
    );
}
