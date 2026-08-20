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

use std::cell::Cell;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Ok(_) => "[other]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Bounded post-eval drain hook — the production CLI pump path (the cluster
/// event pump is driven by drain_and_check at a 10ms cadence).
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

fn find_bao_binary() -> std::path::PathBuf {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("target");
    for profile in ["debug", "release"] {
        let candidate = target.join(profile).join("bao");
        if candidate.exists() {
            return candidate;
        }
    }
    panic!(
        "bao binary not found under {} — build it with `cargo build -p bao_bin` \
         (required as the cluster.fork worker executable)",
        target.display()
    );
}

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
    results.workersLeft = Object.keys(cluster.workers || {}).length;
    results.processExitCode = w.process && w.process.exitCode;
  });
  setTimeout(function () {
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
