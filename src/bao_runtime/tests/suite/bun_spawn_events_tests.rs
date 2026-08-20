// @trace TEST-ENG-005 [req:REQ-ENG-005] [level:integration]
// Bun.spawn event surface (child_process.spawn parity + Bun docs contract):
//   * command shapes — the PRIMARY array form Bun.spawn(["exe", ...args], opts?)
//     plus the { cmd: [...] } / { cmd: "exe", args: [...] } object forms (the
//     array form used to fall through to a bare-`echo` default → output was
//     only "\n");
//   * stdio defaults — stdin "null" (no input), stdout/stderr "pipe" (a piped
//     stdin default stalled stdin-reading children and 'close' never fired);
//   * events — 'exit' at process death, stdout/stderr 'data'/'end' from the
//     pump-captured bytes at pipe EOF, 'close' last, always (empty stdout
//     included, listener registration or not);
//   * bytes — data events deliver the raw bytes (binary-safe, NUL bytes
//     survive; invalid UTF-8 maps byte-per-code-unit), text() decodes UTF-8;
//   * `exited` is a Promise<number> that never blocks the JS thread.

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
    // Deadline-driven (not iteration-driven): with the binary's tests
    // running on parallel threads, per-iteration cost varies wildly under
    // CPU contention and a fixed iteration count flakes — the child's
    // fork/exec alone can outlast it. 10s wall clock is plenty for
    // short-lived children and robust under load.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return true;
        }
    }
    false
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

#[test]
fn bun_spawn_event_surface_lifecycle() {
    let mut ctx = setup_ctx();

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

/// The primary Bun signature: Bun.spawn(["exe", "arg", ...], options?) — the
/// array is the command vector (element 0 = executable), NOT an options
/// object. Regression: this shape used to resolve to bare `echo` → the only
/// output ever delivered was "\n".
#[test]
fn bun_spawn_array_form_delivers_real_output() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;
        var proc = Bun.spawn(["echo", "hello-array-form"]);
        var chunks = [];
        proc.stdout.on("data", function(d) { chunks.push(d); });
        proc.on("close", function(code) {
            log.push("close=" + code);
            log.push("out=[" + chunks.join("") + "]");
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "array-form close must fire; log: {log}");
    assert!(
        log.contains("out=[hello-array-form\n]"),
        "array form must pass args through to the child, got: {log}"
    );
}

/// Multi-line / multi-command stdout must arrive as the full real output.
#[test]
fn bun_spawn_multiline_and_multi_cmd_output() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;
        var proc = Bun.spawn(["/bin/sh", "-c", "echo one; echo two; echo three"]);
        var chunks = [];
        proc.stdout.on("data", function(d) { chunks.push(d); });
        proc.on("close", function(code) {
            log.push("close=" + code);
            log.push("data=[" + chunks.join("") + "]");
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "multi-line close must fire; log: {log}");
    assert!(
        log.contains("data=[one\ntwo\nthree\n]"),
        "multi-line stdout must deliver the full output, got: {log}"
    );
}

/// Binary stdout must survive verbatim: NUL bytes and invalid-UTF-8 bytes
/// reach the data event byte-for-byte (charCodeAt(i) === byte[i]). The old
/// reader truncated at the first NUL via JS_NewStringCopyZ.
#[test]
fn bun_spawn_binary_stdout_verbatim() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;
        var proc = Bun.spawn({ cmd: ["/bin/sh", "-c", "printf 'a\\000b\\377c'"] });
        var chunks = [];
        proc.stdout.on("data", function(d) { chunks.push(d); });
        proc.on("close", function(code) {
            var s = chunks.join("");
            var codes = [];
            for (var i = 0; i < s.length; i++) codes.push(s.charCodeAt(i));
            log.push("close=" + code);
            log.push("bytes=" + codes.join(","));
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "binary close must fire; log: {log}");
    assert!(
        log.contains("bytes=97,0,98,255,99"),
        "binary stdout must deliver bytes 97,0,98,255,99 verbatim, got: {log}"
    );
}

/// Empty stdout: 'close' must still fire (with the exit code) once the child
/// is gone — and the data/end stream events still sequence for a piped but
/// empty stdout.
#[test]
fn bun_spawn_empty_stdout_close_fires() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;
        var proc = Bun.spawn({ cmd: ["/bin/sh", "-c", "exit 3"] });
        proc.stdout.on("data", function(d) { log.push("data-len=" + d.length); });
        proc.stdout.on("end", function() { log.push("stdout_end"); });
        proc.on("close", function(code) {
            log.push("close=" + code);
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "empty-stdout close must fire; log: {log}");
    assert!(
        log.split('|').any(|e| e == "close=3"),
        "close must carry the exit code, got: {log}"
    );
    assert!(
        log.split('|').any(|e| e == "stdout_end"),
        "stdout 'end' must fire for a piped-but-empty stdout, got: {log}"
    );
}

/// Two concurrent Bun.spawn procs: each delivers its own output and close —
/// no cross-proc state bleed through the shared pump registry.
#[test]
fn bun_spawn_two_procs_both_close() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__doneCount = 0;
        function mk(tag, arg) {
            var proc = Bun.spawn(["/bin/echo", arg]);
            var chunks = [];
            proc.stdout.on("data", function(d) { chunks.push(d); });
            proc.on("close", function(code) {
                log.push(tag + ":close=" + code + ":out=" + chunks.join(""));
                globalThis.__doneCount++;
            });
        }
        mk("a", "AAA");
        mk("b", "BBB");
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let done = wait_until(&mut ctx, "globalThis.__doneCount === 2 ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "both procs must close; log: {log}");
    assert!(
        log.split('|').any(|e| e == "a:close=0:out=AAA\n"),
        "proc A output, got: {log}"
    );
    assert!(
        log.split('|').any(|e| e == "b:close=0:out=BBB\n"),
        "proc B output, got: {log}"
    );
}

/// Bun's await-usage core: `await proc.exited` (a real Promise — must not
/// block the JS thread) plus `await proc.stdout.text()` (UTF-8 text shape,
/// distinct from the byte-verbatim data events).
#[test]
fn bun_spawn_exited_promise_and_text() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__state = { code: -99, out: '', isPromise: false };
        var proc = Bun.spawn({ cmd: ["/bin/sh", "-c", "echo line1; echo line2; exit 5"] });
        globalThis.__state.isPromise = typeof proc.exited === 'object' && typeof proc.exited.then === 'function';
        (async function() {
            globalThis.__state.code = await proc.exited;
            globalThis.__state.out = await proc.stdout.text();
        })();
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let done = wait_until(
        &mut ctx,
        "globalThis.__state.out.length > 0 ? 'y' : 'n'",
        60,
    );
    let state = eval_str(&mut ctx, "JSON.stringify(globalThis.__state)");
    assert!(done, "await exited+text must resolve; state: {state}");
    assert!(
        state.contains("\"code\":5"),
        "exited must resolve with the exit code, got: {state}"
    );
    assert!(
        state.contains("\"out\":\"line1\\nline2\\n\""),
        "stdout.text() must deliver the UTF-8 text, got: {state}"
    );
    assert!(
        state.contains("\"isPromise\":true"),
        "proc.exited must be a Promise (never a blocking getter), got: {state}"
    );
}

/// Output larger than the 64KB pipe buffer: the pump thread drains the pipes
/// while the child runs, so the child never blocks on a full pipe. The old
/// read-at-exit model deadlocked here (child blocked writing → never exited
/// → 'close' never fired).
#[test]
fn bun_spawn_large_output_no_pipe_deadlock() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;
        // seq 1 20000 → ~108KB of digits+newlines (> 64KB pipe buffer).
        var proc = Bun.spawn(["seq", "1", "20000"]);
        var chunks = [];
        proc.stdout.on("data", function(d) { chunks.push(d); });
        proc.on("close", function(code) {
            log.push("close=" + code);
            log.push("len=" + chunks.join("").length);
            log.push("tail=" + chunks.join("").slice(-6));
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 200);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, ">64KB output must not pipe-deadlock; log: {log}");
    assert!(
        log.contains("close=0"),
        "child must exit cleanly after >64KB output, got: {log}"
    );
    assert!(
        log.split('|').any(|e| e == "tail=19999" || e == "tail=99999" || e.ends_with("20000\n")),
        "full output must be captured through the last line, got: {log}"
    );
}

/// Default stdin is "null" (Bun docs: "provide no input") — a child that
/// reads stdin must see immediate EOF and exit instead of stalling until
/// parent teardown ('close' never fired under the old piped default).
#[test]
fn bun_spawn_default_stdin_eof_child_exits() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;
        var proc = Bun.spawn(["/bin/sh", "-c", "read line; echo after-read:$line"]);
        proc.stdout.on("data", function(d) { log.push("data=[" + d + "]"); });
        proc.on("close", function(code) {
            log.push("close=" + code);
            globalThis.__done = true;
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 100);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(
        done,
        "stdin-reading child must get EOF and close promptly; log: {log}"
    );
    assert!(
        log.contains("data=[after-read:\n]"),
        "child must read EOF from the null stdin, got: {log}"
    );
}
