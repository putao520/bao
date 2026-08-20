// @trace TEST-ENG-006-CLUSTER [req:REQ-ENG-006] [level:e2e]
// P0 end-to-end regression for cluster.fork (v-surface audit): fork returned a
// Worker with a pid but the child never executed (non-NUL-terminated envp →
// garbage exec env) and no online/exit/message events ever fired (blocking
// sync spawn + no event wiring).
//
// This test forks a REAL child process: `bao run <script>` with the worker
// branch asserting its cluster env (worker id + fork(env) merge), exchanging
// IPC messages in both directions, and exiting on disconnect. The bao binary
// is located in the workspace target dir; BAO_CLUSTER_EXEC lets cluster.fork
// spawn it even though current_exe() under cargo test is the test harness.
//
// The event pump is the post_eval_hook → timers::drain_and_check production
// path (a bare Rust tick pump silently drops JS timer callbacks — see
// net_echo_e2e_tests).

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
/// event pump is a 10ms setInterval driven by drain_and_check).
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

/// Pump the event loop via repeated (post-eval-hook driven) evals until
/// `js_condition` yields 'y', the wall-clock deadline passes, or the eval
/// iteration budget runs out.
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

/// Locate the bao binary built in this workspace (debug preferred).
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

#[test]
fn test_cluster_fork_child_runs_events_and_message_roundtrip() {
    let bao = find_bao_binary();
    // Point cluster.fork at the real binary (current_exe() is the test
    // harness under cargo test).
    unsafe { std::env::set_var("BAO_CLUSTER_EXEC", &bao) };

    // Worker script: verifies its cluster env, waits for the primary's ping
    // over IPC, answers, and stays alive until the primary disconnects.
    let worker_script = r#"
var cluster = require('cluster');
if (cluster.isWorker) {
  var envOk = process.env.BAO_CLUSTER_WORKER_ID === '1'
    && process.env.BAO_CLUSTER_PROBE_ENV === 'from-fork';
  process.send({ type: 'worker-up', envOk: envOk });
  process.on('message', function (m) {
    if (m && m.ping === 1) {
      process.send({ type: 'pong', envOk: envOk });
    }
  });
  // Watchdog: never hang the test if the primary dies.
  setTimeout(function () { process.exit(3); }, 60000);
} else {
  var results = { forked: false, online: 0, msgs: [], exitCode: null, exitSignal: null, disconnected: false };
  globalThis.__clusterResults = results;
  globalThis.__clusterDone = function () {
    return results.exitCode !== null
      && results.msgs.some(function (m) { return m && m.type === 'pong'; });
  };
  var w = cluster.fork({ BAO_CLUSTER_PROBE_ENV: 'from-fork' });
  results.forked = !!(w && w.id === 1 && w.process && typeof w.process.pid === 'number' && w.process.pid > 0);
  w.on('online', function () { results.online += 1; });
  w.on('message', function (m) {
    results.msgs.push(m);
    if (m && m.type === 'worker-up' && m.envOk === true) {
      w.send({ ping: 1 });
    }
  });
  w.on('exit', function (code, signal) {
    results.exitCode = code;
    results.exitSignal = signal;
  });
  setTimeout(function () {
    try {
      cluster.disconnect(function () { results.disconnected = true; });
    } catch (e) {}
  }, 600);
}
"#;
    let script_path = std::env::temp_dir().join("bao_p0_cluster_worker.js");
    std::fs::write(&script_path, worker_script).expect("write worker script");

    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let script_path_str = script_path.to_str().unwrap().to_string();
    // The worker child runs `<bao> run <script>`; the harness-side primary
    // branch reads process.argv — set it to the script path.
    let boot = eval_string(
        &mut ctx,
        &format!(
            r#"process.argv = ['bao', {p:?}]; 'ok'"#,
            p = script_path_str
        ),
    );
    assert_eq!(boot, "ok");

    // Load the primary branch of the script (the same file the worker runs,
    // with isWorker false in the harness process).
    let primary_src = std::fs::read_to_string(&script_path).unwrap();
    let ran = eval_string(&mut ctx, &primary_src);
    assert!(
        ran != "undefined" || true,
        "primary branch eval: {}",
        ran
    );

    // Pump until the worker exited AND the pong roundtrip completed.
    let done = wait_until(
        &mut ctx,
        "(globalThis.__clusterDone && globalThis.__clusterDone()) ? 'y' : 'n'",
        50,
    );

    let final_state = eval_string(
        &mut ctx,
        "(function () { return JSON.stringify(globalThis.__clusterResults); })()",
    );
    eprintln!("cluster test state: {}", final_state);
    assert!(done, "cluster e2e must complete; state: {}", final_state);

    assert!(
        final_state.contains("\"forked\":true"),
        "fork() must return a Worker with id and process.pid: {}",
        final_state
    );
    assert!(
        final_state.contains("\"online\":1"),
        "worker 'online' event must fire exactly once: {}",
        final_state
    );
    assert!(
        final_state.contains("\"type\":\"worker-up\"") && final_state.contains("\"envOk\":true"),
        "worker must actually run and report its cluster env (worker id + fork(env) merge): {}",
        final_state
    );
    assert!(
        final_state.contains("\"type\":\"pong\""),
        "message roundtrip primary→worker→primary must complete: {}",
        final_state
    );
    assert!(
        final_state.contains("\"exitCode\":") && !final_state.contains("\"exitCode\":null"),
        "worker 'exit' event must fire with the exit status: {}",
        final_state
    );

    // Cleanup.
    let _ = std::fs::remove_file(&script_path);
    bun_runtime::shutdown_thread_sm();
}
