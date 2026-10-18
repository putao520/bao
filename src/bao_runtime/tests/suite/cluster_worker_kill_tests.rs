// @trace TEST-ENG-006-CLUSTER [req:REQ-ENG-006] [level:e2e]
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
// kills it, and verifies process-level death (the pid is gone from /proc),
// the 'exit' event (code=-1, signal=SIGTERM), isDead flipped by the observed
// exit, and the cluster.workers registry emptied.

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
  w.on('online', function () { results.online += 1; });
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
  setTimeout(function () {
    results.killedAt = Date.now() - results.bootAt;
    results.killed = true;
    results.isDeadAtKill = w.isDead; // Node: still false — death is observed, not assumed
    w.kill(); // default SIGTERM
  }, 300);
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
    assert!(
        final_state.contains("\"exitSignal\":15"),
        "exit must carry SIGTERM (signal 15): {}",
        final_state
    );
    assert!(
        final_state.contains("\"exitCode\":-1"),
        "signal death reports exitCode -1: {}",
        final_state
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
    let mut gone = false;
    for _ in 0..100 {
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
