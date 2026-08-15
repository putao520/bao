// @trace TEST-ENG-005 [req:REQ-ENG-005] [level:integration]
// Bun.spawn event surface (child_process.spawn parity, task #21 item 3):
// the proc object gets on/once/off/emit, 'exit'/'close' dispatch driven by
// the non-blocking _pollExited native (never stalls the JS thread), and
// stdout/stderr 'data'/'end' delivered from the captured output at exit
// (the native readers are read_to_end — capture-at-exit stdio model).

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

/// Bounded post-eval drain hook — the production CLI pump path (see
/// net_echo_e2e_tests for why a bare-Rust pump silently drops timers).
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

#[test]
fn bun_spawn_event_surface_lifecycle() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;

        var proc = Bun.spawn({ cmd: ["/bin/sh", "-c", "echo hi; exit 7"] });

        // EventEmitter surface on the returned proc.
        log.push("on=" + (typeof proc.on === "function"));
        log.push("once=" + (typeof proc.once === "function"));
        log.push("emit=" + (typeof proc.emit === "function"));
        log.push("off=" + (typeof proc.off === "function"));
        log.push("pid=" + (proc.pid > 0));

        var outChunks = [];
        proc.stdout.on("data", function(d) { outChunks.push(d); });
        proc.stdout.on("end", function() { log.push("stdout_end"); });
        var exitFires = 0;
        proc.once("exit", function(code) { exitFires++; });
        proc.on("exit", function(code) { log.push("exit=" + code + "/" + exitFires); });
        proc.on("close", function(code) {
            log.push("close=" + code);
            log.push("stdout=" + outChunks.join(""));
            log.push("exitCode=" + proc.exitCode);
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "Bun.spawn lifecycle must complete; log: {log}");

    for part in [
        "on=true",
        "once=true",
        "emit=true",
        "off=true",
        "pid=true",
        "exit=7/1",
        "close=7",
        "stdout=hi\n",
        "stdout_end",
        "exitCode=7",
    ] {
        assert!(
            log.split('|').any(|entry| entry == part),
            "Bun.spawn lifecycle log must contain '{part}', got: {log}"
        );
    }
}
