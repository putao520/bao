// @trace TEST-ENG-7-CP-FOLLOWUP [req:REQ-ENG-007] [level:integration]
//
// 774f7088f 基线四登记项 follow-up 验证面(src/bao_runtime/src/node_child_process.rs):
//   ① __cp_peek_exit 非消费探针 + attach_streams(build_stdio_streams)切换 —
//      双消费者竞争面根治:probe 幂等零消费、take 后 probe 仍答(sticky
//      exit_payload)、consume-once 语义保持;native 流面 read 与 take 并行
//      不再丢 exit(旧 glue 在 read() 里 poll take-slot → 流偷走 exit 或
//      take 先行后流面 EOF 失明)。
//   ② exec 回调错误 node parity:message "Command failed: <cmd>\n<stderr>"
//      (空 stderr 也带尾 \n)、code 数字(信号死亡 null)、killed/signal/cmd。
//   ③ spawnSync fail-object 真 Error 实例(经全局 Error 构造器桥接,原
//      JS_NewError 缺位 follow-up)+ SystemError 字段与 spawnargs 直接挂
//      Error 本体;execSync/execFileSync 抛出面真 Error + node 消息字面
//      (stderr 非空才以 \n 追加 — 与 exec 回调空 stderr 尾 \n 形态不同,
//      双 oracle 实测)。
//   ④ windows 孪生面(cp_detect_exit_windows 同 pipes_drained 门 + 孤儿
//      潜伏缺陷)在本文件之外:交叉编译门验证,本文件 unix-only。
//   ⑤ E21 负载态红根治(2026-10-25,E17 fleet 15 轮 5/15 红的归因处置):
//      两个 wait 原为 common::wait_until_with 迭代有界纯自旋(无 sleep),
//      墙钟窗 = iters × eval-latency(~1-30ms),与 cp drain 线程的
//      fork/exec/waitpid/Exit-push 链(~70-90ms,farm 背景负载)成反比
//      竞态 — 自旋越快窗口越短。取证实证:窗口结束后 +10ms exit 即可观察
//      (DELAYED publication,not loss — 产品 push-once/sticky 链全程存活),
//      换墙钟窗口(10s 窗/10ms 间隔)后红根除。见 wait_until_deadline。
// 全部字面/形状 pin 自 node v24.19.0 oracle(2026-10-18 实测)。

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_str;
use common::eval_string_dbg;
use common::setup_ctx;
use common::setup_ctx_bounded;

/// Load-robust wall-clock condition wait: poll `js_condition` every 10ms
/// until it answers 'y' or `secs` elapse.
///
/// Why not [`common::wait_until_with`]: it is an ITERATION-bounded spin with
/// no sleep — its wall window is `iters × eval-latency` (tens of ms, and the
/// faster the host the shorter the window). That races any awaited state
/// produced on ANOTHER thread's schedule. Here the awaited state is the cp
/// drain thread's fork/exec/waitpid/Exit-push chain: under fleet load the
/// child is reaped ~70-90ms after spawn while the 80-iteration spin closed
/// its window in ~1-30ms, so the probe legitimately answered null and the
/// test redged 5/15 E17 fleet rounds (E21 stage-1 forensics: the extended
/// probe observed the exit +10ms AFTER the spin window ended — delayed
/// publication, not loss). A wall-clock window with an inter-probe sleep
/// costs nothing when green (idle arrival ≈ 1-5ms) and turns a scheduler
/// race into an honest bounded wait: red within the window = genuine event
/// loss, a product defect.
///
/// `budget` passthrough keeps the bounded-ctx timer drain alive
/// ([`common::HOOK_BUDGET`] feeds `setup_ctx_bounded`'s post-eval hook,
/// which drives the stream face's pump); it is inert for hookless contexts.
fn wait_until_deadline(
    ctx: &mut JsContext,
    js_condition: &str,
    budget: usize,
    secs: u64,
) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        common::HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

// ═══════════════════════════════════════════════════════════════════════════
// ① exit 槽双消费者 — probe 幂等 / take 后 sticky / consume-once 保持
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[cfg(unix)]
fn followup_peek_exit_no_dual_consumer_loss() {
    crate::exit_isolation::dispatch_timeout(
        "cp_followup_tests::followup_peek_exit_no_dual_consumer_loss",
        followup_peek_exit_no_dual_consumer_loss_body,
    );
}

#[cfg(unix)]
fn followup_peek_exit_no_dual_consumer_loss_body() {
    let t0 = std::time::Instant::now();
    let mut ctx = setup_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__fucp = require("child_process");
        globalThis.__fulive = globalThis.__fucp.spawn("/bin/sh", ["-c", "sleep 30"]);
        globalThis.__fupid = globalThis.__fucp.spawn("/bin/sh", ["-c", "exit 7"]).pid;
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");
    let setup_ms = t0.elapsed().as_millis();

    // The live child answers null (running — probe must not fake an exit).
    let live_null = eval_str(
        &mut ctx,
        "String(globalThis.__fucp.__cp_peek_exit(globalThis.__fulive.pid))",
    );
    assert_eq!(live_null, "null", "probe on a live child must answer null");

    // Wait until the exited child is observable through the PROBE (the drain
    // thread's Exit push, folded by the probe's own pump). Wall-clock window
    // (E21): the probe must not race the drain thread's reap chain — see
    // wait_until_deadline. Red within the 10s window = genuine event loss.
    let cond = "(globalThis.__fucp.__cp_peek_exit(globalThis.__fupid) !== null) ? 'y' : 'n'";
    let wait_started = std::time::Instant::now();
    let seen = wait_until_deadline(&mut ctx, cond, 60, 10);
    assert!(
        seen,
        "probe must observe the exit once published within the 10s wall window \
         (setup_ms={setup_ms}, waited {}ms) — genuine loss if red",
        wait_started.elapsed().as_millis()
    );

    // The competition story, in order:
    //   p1/p2 — probe is idempotent and consumes nothing;
    //   take  — the authoritative consume-once take still delivers;
    //   p3    — probe answers AFTER the take (sticky payload — the stream-glue
    //           EOF detector must not go blind when the shim took first);
    //   take2 — consume-once preserved (second take sees null).
    let story = eval_string_dbg(
        &mut ctx,
        r#"
        var cp = globalThis.__fucp;
        var pid = globalThis.__fupid;
        var p1 = cp.__cp_peek_exit(pid);
        var p2 = cp.__cp_peek_exit(pid);
        var take = cp.__cp_poll_exit(pid);
        var p3 = cp.__cp_peek_exit(pid);
        var take2 = cp.__cp_poll_exit(pid);
        Array.from(p1).join(",") + "|" + Array.from(p2).join(",") + "|"
            + Array.from(take).join(",") + "|" + Array.from(p3).join(",") + "|"
            + String(take2)
    "#,
    );
    assert_eq!(
        story, "7,0|7,0|7,0|7,0|null",
        "probe/take interleaving must lose no exit and keep consume-once; got: {story}"
    );

    eval_str(
        &mut ctx,
        "globalThis.__fulive.kill('SIGKILL'); 'killed'",
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// ① e2e — native 流面(peek)读到 EOF 后,take 槽仍完好的 exit 可取
// (修前形态:流面 read() 里 __cp_poll_exit 抢先 take → 本测试的 take=null)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[cfg(unix)]
fn followup_stream_read_then_take_still_delivers() {
    crate::exit_isolation::dispatch_timeout(
        "cp_followup_tests::followup_stream_read_then_take_still_delivers",
        followup_stream_read_then_take_still_delivers_body,
    );
}

#[cfg(unix)]
fn followup_stream_read_then_take_still_delivers_body() {
    let mut ctx = setup_ctx_bounded();

    // Drive ONLY the native stream face (child.stdout from
    // build_stdio_streams — the Readable the glue switched to the probe).
    // The wrapper is inert: no on() on it → its pollTick never runs and the
    // stdout_buf has exactly one consumer, so the byte assertions are
    // deterministic.
    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__sr = { data: "", ended: false };
        var w = require("child_process").spawn("/bin/sh", ["-c", "printf fu-data; exit 7"]);
        globalThis.__srw = w;
        var s = w._native.stdout;
        globalThis.__sr_isreadable = typeof s.on === "function" && typeof s.push === "function";
        s.on("data", function(d) {
            globalThis.__sr.data += String.fromCharCode.apply(null, new Uint8Array(d));
        });
        s.on("end", function() { globalThis.__sr.ended = true; });
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");

    // Wall-clock window (E21, same class as the peek wait): the stream face's
    // data/end events are produced by the drain thread + pump chain, not by
    // the eval itself — don't race them with an iteration-bounded spin.
    let scond = "(globalThis.__sr.ended === true) ? 'y' : 'n'";
    let wait_started = std::time::Instant::now();
    let done = wait_until_deadline(&mut ctx, scond, 60, 10);
    let data = eval_str(&mut ctx, "globalThis.__sr.data");
    assert!(
        done,
        "stream face must reach EOF within the 10s wall window; data so far: {data:?} \
         (waited {}ms) — genuine stall if red",
        wait_started.elapsed().as_millis()
    );
    assert_eq!(data, "fu-data", "stream face must deliver the child's bytes");
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__sr_isreadable)"),
        "true",
        "native stdout must be the Readable stream face"
    );

    // The no-lost-exit contract: the stream reached EOF via the probe, so
    // the consume-once slot is untouched — the take still delivers.
    let take = eval_str(
        &mut ctx,
        "String(require('child_process').__cp_poll_exit(globalThis.__srw.pid))",
    );
    assert_eq!(take, "7,0", "exit slot must be intact after the stream read; got: {take}");
}

// ═══════════════════════════════════════════════════════════════════════════
// ②③ 同步面 label checks — exec 回调错误 parity / spawnSync fail-object 真
// Error / execSync+execFileSync 抛出面真 Error
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[cfg(unix)]
fn followup_error_faces_node_parity() {
    crate::exit_isolation::dispatch_timeout(
        "cp_followup_tests::followup_error_faces_node_parity",
        followup_error_faces_node_parity_body,
    );
}

#[cfg(unix)]
fn followup_error_faces_node_parity_body() {
    let mut ctx = setup_ctx();
    // exec is ASYNC (node v24.19.0 oracle 2026-10-27, E-cp wave): the
    // callback is delivered at 'close', not inline — schedule the two exec
    // children, settle the poll pump through the bounded drain hook
    // (wait_until_deadline's per-eval HOOK_BUDGET), then run the checks.
    ctx.set_post_eval_hook(common::bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var cp = require("child_process");
        globalThis.__fuCb = { done: false, err: "unset" };
        cp.exec("/bin/sh -c 'echo fubo >&2; exit 5'", function(err) {
            globalThis.__fuCb = { done: true, err: err };
        });
        globalThis.__fuCbNull = "pending";
        cp.exec("printf fu-ok", function(err) { globalThis.__fuCbNull = err; });
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");

    // Settle window (E21 law): the delivery rides the shim's poll pump, not
    // the scheduling eval — a wall-clock window with an inter-eval timer
    // drain; red within it = genuine event loss.
    let settled = wait_until_deadline(
        &mut ctx,
        "(globalThis.__fuCb.done === true && globalThis.__fuCbNull !== 'pending') ? 'y' : 'n'",
        60,
        10,
    );
    let state = eval_str(
        &mut ctx,
        "JSON.stringify({done: globalThis.__fuCb.done, msg: String(globalThis.__fuCb.err && globalThis.__fuCb.err.message), nullErr: String(globalThis.__fuCbNull)})",
    );
    assert!(
        settled,
        "exec callbacks must settle within the 10s window; state: {state}"
    );

    // Label-check harness identical to the c5 suite's.
    let results = eval_string_dbg(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch (e) { results.push(label + " ERR:" + (e && e.message ? String(e.message).substring(0, 90) : String(e))); }
        }
        var cp = require("child_process");

        // ── ② exec callback error: node literal + property surface ──
        // (children scheduled + settled above — the callback lands at 'close'
        // per node's async exec contract; the checks read the recorded
        // deliveries from globalThis.__fuCb / __fuCbNull.)
        check("exec_cb_err_full_parity", function() {
            var e = globalThis.__fuCb.err;
            return globalThis.__fuCb.done && e instanceof Error
                && String(e.message) === "Command failed: /bin/sh -c 'echo fubo >&2; exit 5'\nfubo\n"
                && e.code === 5 && e.killed === false && e.signal === null
                && e.cmd === "/bin/sh -c 'echo fubo >&2; exit 5'";
        });
        check("exec_cb_success_err_null", function() {
            return globalThis.__fuCbNull === null;
        });

        // ── ③ spawnSync fail-object: real Error + SystemError fields ──
        var r = cp.spawnSync("/nonexistent-fu-xyz");
        check("syncspawn_failobj_status_shape", function() {
            return r.status === null && r.signal === null && r.pid === 0;
        });
        check("syncspawn_failobj_real_error", function() {
            return r.error instanceof Error
                && String(r.error.message) === "spawnSync /nonexistent-fu-xyz ENOENT";
        });
        check("syncspawn_failobj_syserr_fields", function() {
            return r.error.code === "ENOENT" && r.error.errno === -2
                && r.error.syscall === "spawnSync /nonexistent-fu-xyz"
                && r.error.path === "/nonexistent-fu-xyz";
        });
        check("syncspawn_failobj_spawnargs", function() {
            return Array.isArray(r.error.spawnargs)
                && r.error.spawnargs.length === 1
                && r.error.spawnargs[0] === "/nonexistent-fu-xyz";
        });

        // ── ③ execSync throw: real Error + stderr suffix (non-empty only) ──
        check("execsync_throw_real_error_stderr_suffix", function() {
            try { cp.execSync("/bin/sh -c 'echo zfu >&2; exit 7'"); return "NO-THROW"; }
            catch (err) {
                return err instanceof Error
                    && String(err.message) === "Command failed: /bin/sh -c 'echo zfu >&2; exit 7'\nzfu\n"
                    && err.status === 7;
            }
        });
        check("execsync_throw_empty_stderr_no_suffix", function() {
            try { cp.execSync("/bin/sh -c 'exit 7'"); return "NO-THROW"; }
            catch (err) {
                // node oracle: empty stderr → NO trailing newline on this face
                // (unlike the exec callback, which keeps it).
                return err instanceof Error
                    && String(err.message) === "Command failed: /bin/sh -c 'exit 7'";
            }
        });

        // ── ③ execFileSync throw: real Error + argv-joined literal ──
        check("execfilesync_throw_real_error_literal", function() {
            try { cp.execFileSync("/bin/sh", ["-c", "echo wfu >&2; exit 3"]); return "NO-THROW"; }
            catch (err) {
                return err instanceof Error
                    && String(err.message) === "Command failed: /bin/sh -c echo wfu >&2; exit 3\nwfu\n"
                    && err.status === 3
                    && Array.isArray(err.output) && err.output.length === 3;
            }
        });

        results.join("|")
    "#,
    );

    let mut pass = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for item in results.split('|') {
        if item.is_empty() {
            continue;
        }
        if item.contains(" PASS") {
            pass += 1;
        } else {
            failures.push(item.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "{} label check(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(pass >= 8, "expected >= 8 passes, got {pass}");
}
