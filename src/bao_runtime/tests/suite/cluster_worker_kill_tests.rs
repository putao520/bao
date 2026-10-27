// @trace TEST-ENG-6-CLUSTER [req:REQ-ENG-006] [level:e2e]
// cluster worker.kill() regression (SILENT: kill never killed):
//   * child-side — the PosixStdio::Ipc spawn path clones the worker while
//     the C++ spawner holds all signals blocked, so the worker inherited an
//     all-blocked signal mask across exec: a directed SIGTERM stayed pending
//     forever, the worker lived on (6s+ observed), and the primary never saw
//     'exit'. __cluster_worker_boot now resets the mask at worker boot.
//   * parent-side — worker.kill() used to set isDead=true immediately, and
//     handleExit early-returns on isDead, so the REAL exit event was dropped:
//     'exit'/'close' never fired, the worker stayed in cluster.workers, and
//     the primary's loop-liveness contribution spun forever. kill() now only
//     sends the signal (Node semantics) with a 1s SIGKILL escalation.
//
// This test forks a REAL worker (`bao run <script>`, BAO_CLUSTER_EXEC),
// kills it after the worker comes ONLINE (event-driven: the former fixed
// 300ms timer raced the worker boot + IPC handshake under fleet load —
// a pre-online kill delivers 'exit' but 'online' never fired), and verifies
// process-level death (the pid is gone from /proc), the 'exit' event in the
// node-oracle shape — node v24.19.0 (live probe 2026-10-25, child_process
// AND cluster worker killed via kill()/SIGTERM): exit(code=null,
// signal='SIGTERM'), w.process.exitCode=null; a signal death has NO exit
// code — isDead flipped by the observed exit, and the cluster.workers
// registry emptied.

use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;
use common::HOOK_BUDGET;
use common::bounded_drain_hook;

use common::eval_string_dbg as eval_string;




fn wait_until(ctx: &mut JsContext, js_condition: &str, per_eval_budget: usize) -> bool {
    let deadline = Instant::now() + Duration::from_secs(25);
    for _ in 0..6000 {
        if Instant::now() > deadline {
            return false;
        }
        HOOK_BUDGET.with(|b| b.set(per_eval_budget));
        if eval_string(ctx, js_condition) == "y" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

// BCE-20261018-STALEBIN: binary resolution + behavior canary are
// single-point in common/bin_resolver.rs (nonce argv-reflection probe
// included); the retired inline ladder accepted any existing file without an
// identity check, so the 2026-10-18 battery's poisoned stale artifact
// (BAO_TEST_BAO_BIN, 2026-10-17 mid-WIP) was silently accepted here.
use common::bin_resolver::find_bao_binary;

fn pid_is_gone(pid: i32) -> bool {
    // A zombie (state Z) also counts as "not gone" for the purposes of the
    // "really killed" assertion only if never reaped; the cp-poll thread
    // reaps, so a dead worker disappears from /proc entirely.
    !std::path::Path::new(&format!("/proc/{}", pid)).exists()
}

#[test]
fn test_cluster_worker_kill_kills_child_and_delivers_exit() {
    let bao = find_bao_binary();
    unsafe { std::env::set_var("BAO_CLUSTER_EXEC", &bao) };

    // Worker branch: require('cluster') (boots the IPC + signal-mask reset),
    // then stay alive forever — only worker.kill() may end it.
    let worker_script = r#"
var cluster = require('cluster');
if (cluster.isWorker) {
  setInterval(function () {}, 100);
  setTimeout(function () { process.exit(9); }, 60000); // test watchdog
} else {
  var results = { pid: 0, online: 0, killed: false, isDeadAtKill: null,
                  killOk: null, escOk: null, bootAt: Date.now(), killedAt: null, hbAt: null,
                  exitCode: null, exitSignal: null, isDeadAfterExit: null,
                  workersLeft: -1, processExitCode: null };
  globalThis.__results = results;
  globalThis.__done = function () { return results.exitSignal !== null; };
  var w = cluster.fork();
  results.pid = w._pid;
  // Event-driven kill: the former fixed-300ms timer raced the worker's
  // boot + IPC handshake under fleet load — a pre-online kill delivers
  // 'exit' but the worker never emitted 'online' (e15 load run observed
  // online:0 with exitSignal:15). Killing only after 'online' is the Node
  // flow this test pins (healthy worker → kill → observed death); the
  // exit-driven assertions below are unchanged.
  w.on('online', function () {
    results.online += 1;
    setTimeout(function () {
      results.killedAt = Date.now() - results.bootAt;
      results.killed = true;
      results.isDeadAtKill = w.isDead; // Node: still false — death is observed, not assumed
      w.kill(); // default SIGTERM
    }, 50);
  });
  w.on('exit', function (code, signal) {
    results.exitCode = code;
    results.exitSignal = signal;
    results.isDeadAfterExit = w.isDead;
    results.killOk = w._lastKillOk === undefined ? null : w._lastKillOk;
    results.escOk = w._lastEscOk === undefined ? null : w._lastEscOk;
    results.workersLeft = Object.keys(cluster.workers || {}).length;
    results.processExitCode = w.process && w.process.exitCode;
  });
  setTimeout(function () { results.hbAt = Date.now() - results.bootAt; }, 50);
}
"#;
    let script_path = std::env::temp_dir().join("bao_cluster_kill_worker.js");
    std::fs::write(&script_path, worker_script).expect("write worker script");

    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let script_path_str = script_path.to_str().unwrap().to_string();
    let boot = eval_string(
        &mut ctx,
        &format!(
            r#"process.argv = ['bao', {p:?}]; 'ok'"#,
            p = script_path_str
        ),
    );
    assert_eq!(boot, "ok");

    let primary_src = std::fs::read_to_string(&script_path).unwrap();
    let _ = eval_string(&mut ctx, &primary_src);

    // 'exit' must arrive — before the fixes the worker ignored SIGTERM and
    // the event was dropped by the premature isDead flag.
    let done = wait_until(
        &mut ctx,
        "(globalThis.__done && globalThis.__done()) ? 'y' : 'n'",
        50,
    );
    let final_state = eval_string(
        &mut ctx,
        "(function () { return JSON.stringify(globalThis.__results); })()",
    );
    eprintln!("cluster kill test state: {}", final_state);
    assert!(
        done,
        "worker.kill() must produce an 'exit' event; state: {}",
        final_state
    );

    assert!(
        final_state.contains("\"online\":1"),
        "worker must come online before the kill: {}",
        final_state
    );
    assert!(
        final_state.contains("\"isDeadAtKill\":false"),
        "kill() must not assume death (isDead flips on the observed exit): {}",
        final_state
    );
    // node-oracle precise exit shape (E17): the former substring asserts
    // `contains("\"exitCode\":-1")` / `contains("\"exitSignal\":15")` were
    // PREFIX-WEAK — "-1" also matches "-15", which is exactly the value bao's
    // native poll produces for a signal death (node_child_process.rs encoding
    // contract: "signal deaths → code = -signal", passed through by the
    // cluster exit poll verbatim). node v24.19.0 (live probe 2026-10-25,
    // cluster worker kill()) delivers exit(code=null, signal='SIGTERM' string)
    // and w.process.exitCode=null. Per-field strict equality below — no
    // substring matching, so a -signal-encoded code can never satisfy them.
    // These pins hold the wire face to the node contract: bao's current
    // (-signal, signum) passthrough is the registered deviation (the
    // normalization belongs in the child_process/cluster exit delivery, not
    // in this assertion).
    let exit_code = eval_string(&mut ctx, "String(globalThis.__results.exitCode)");
    assert_eq!(
        exit_code, "null",
        "node oracle: signal death reports exitCode null (code and signal are separate fields); got {exit_code}: {final_state}"
    );
    let exit_signal = eval_string(&mut ctx, "String(globalThis.__results.exitSignal)");
    assert_eq!(
        exit_signal, "SIGTERM",
        "node oracle: signal death reports the signal NAME string 'SIGTERM', not a signum; got {exit_signal}: {final_state}"
    );
    let proc_exit_code =
        eval_string(&mut ctx, "String(globalThis.__results.processExitCode)");
    assert_eq!(
        proc_exit_code, "null",
        "node oracle: w.process.exitCode reads back null on signal death; got {proc_exit_code}: {final_state}"
    );
    assert!(
        final_state.contains("\"isDeadAfterExit\":true"),
        "isDead must flip once the exit is observed: {}",
        final_state
    );
    assert!(
        final_state.contains("\"workersLeft\":0"),
        "the exited worker must leave cluster.workers (loop-liveness leak): {}",
        final_state
    );

    // Process-level verification: the worker pid is really gone (dead AND
    // reaped — not a zombie, not a survivor).
    let pid: i32 = {
        let probe = eval_string(&mut ctx, "String(globalThis.__results.pid)");
        probe.parse().unwrap_or(0)
    };
    assert!(pid > 0, "worker pid must be recorded, got {}", pid);
    // 15s reap margin: the cp-poll thread reaps promptly, but under fleet
    // load the waitpid side can lag; the pid disappears on the first pass in
    // the healthy case so the margin costs nothing when green.
    let mut gone = false;
    for _ in 0..300 {
        if pid_is_gone(pid) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(gone, "worker pid {} must be dead after kill()", pid);

    let _ = std::fs::remove_file(&script_path);
    bun_runtime::shutdown_thread_sm();
}
