// @trace TEST-ENG-007-CP-FOLLOWUP [req:REQ-ENG-007] [level:integration]
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
// 全部字面/形状 pin 自 node v24.19.0 oracle(2026-10-18 实测)。

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_str;
use common::eval_string_dbg;
use common::setup_ctx;
use common::setup_ctx_bounded;
use common::wait_until_with;

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

    // The live child answers null (running — probe must not fake an exit).
    let live_null = eval_str(
        &mut ctx,
        "String(globalThis.__fucp.__cp_peek_exit(globalThis.__fulive.pid))",
    );
    assert_eq!(live_null, "null", "probe on a live child must answer null");

    // Wait until the exited child is observable through the PROBE (the drain
    // thread's Exit push, folded by the probe's own pump).
    let seen = wait_until_with(
        &mut ctx,
        "(globalThis.__fucp.__cp_peek_exit(globalThis.__fupid) !== null) ? 'y' : 'n'",
        60,
        80,
        eval_str,
    );
    assert!(seen, "probe must observe the exit once published");

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

    let done = wait_until_with(
        &mut ctx,
        "(globalThis.__sr.ended === true) ? 'y' : 'n'",
        60,
        120,
        eval_str,
    );
    let data = eval_str(&mut ctx, "globalThis.__sr.data");
    assert!(done, "stream face must reach EOF; data so far: {data:?}");
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
        var cb = { done: false, err: "unset" };
        cp.exec("/bin/sh -c 'echo fubo >&2; exit 5'", function(err) {
            cb = { done: true, err: err };
        });
        check("exec_cb_err_full_parity", function() {
            var e = cb.err;
            return cb.done && e instanceof Error
                && String(e.message) === "Command failed: /bin/sh -c 'echo fubo >&2; exit 5'\nfubo\n"
                && e.code === 5 && e.killed === false && e.signal === null
                && e.cmd === "/bin/sh -c 'echo fubo >&2; exit 5'";
        });
        check("exec_cb_success_err_null", function() {
            var got = "pending";
            cp.exec("printf fu-ok", function(err) { got = err; });
            return got === null;
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
