// @trace TEST-ENG-006-CLUSTER-PUMP [req:REQ-ENG-006] [level:e2e]
// Regression for the cluster parent-loop stall (BCE). Root cause chain: the
// CLUSTER_JS shim drove BOTH IPC polls with setInterval(10) timers that were
// never cleared/unref'd — the worker's interval pinned the worker alive
// after its script completed, so the worker never exited; the primary's
// pollTimer (alive while cluster.workers is non-empty) then pinned the
// primary forever. p2 shape (fork re-runs the FULL module script): both
// processes spun >200s with zero stdout flush (block-buffered stdout that
// only flushes at exit).
//
// The fix drives the same JS poll logic from the native cluster pump
// (timers::drain_and_check → node_cluster::cluster_pump_all) with Node
// liveness semantics: a worker whose loop drains EXITS (its pump never
// pins); the primary stays alive while workers run and exits cleanly once
// they exit. These tests fork REAL bao child processes.

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
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        HOOK_BUDGET.with(|b| b.set(per_eval_budget));
        if eval_string(ctx, js_condition) == "y" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(3));
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
        "bao binary not found under {} — build it with `cargo build -p bao_bin`",
        target.display()
    );
}

/// The p2-stall shape: the worker runs the FULL module script and NEVER calls
/// process.exit — its event loop must drain (native pump, no pinning
/// interval), the worker must exit on its own, and the primary must observe
/// 'exit' and finish. Pre-fix this hung both processes forever.
#[test]
fn test_worker_script_end_exits_and_primary_observes_exit() {
    let bao = find_bao_binary();
    unsafe { std::env::set_var("BAO_CLUSTER_EXEC", &bao) };

    // p2 shape: the SAME script runs in the primary and (re-exec'd) worker.
    // The worker branch does NOT process.exit — it finishes its script, one
    // short timer fires, and then its loop must drain to exit.
    let worker_script = r#"
var fs = require('fs');
var logPath = process.env.BAO_PUMP_LOG;
function log(line) { try { fs.appendFileSync(logPath, line + '\n'); } catch (e) {} }
var cluster = require('cluster');
if (cluster.isWorker) {
  log('C:boot');
  setTimeout(function () { log('C:timer'); }, 150);
  // NO process.exit anywhere — the drained loop must end the worker.
} else {
  var results = { online: 0, exitCode: null };
  globalThis.__results = results;
  globalThis.__done = function () { return results.exitCode !== null; };
  var w = cluster.fork();
  w.on('online', function () { results.online += 1; });
  w.on('exit', function (code) {
    results.exitCode = code;
    log('P:exit code=' + code);
  });
  setTimeout(function () { log('P:timeout'); }, 8000);
}
"#;
    let log_path = std::env::temp_dir().join("bao_cluster_pump.log");
    let _ = std::fs::remove_file(&log_path);
    let script_path = std::env::temp_dir().join("bao_cluster_pump_worker.js");
    std::fs::write(&script_path, worker_script).expect("write worker script");

    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    // Route the log + script argv the way `bao run` would.
    let boot = eval_string(
        &mut ctx,
        &format!(
            r#"process.argv = ['bao', {p:?}]; process.env.BAO_PUMP_LOG = {l:?}; 'ok'"#,
            p = script_path.to_str().unwrap(),
            l = log_path.to_str().unwrap()
        ),
    );
    assert_eq!(boot, "ok");

    let primary_src = std::fs::read_to_string(&script_path).unwrap();
    let _ = eval_string(&mut ctx, &primary_src);

    // The worker boots a full runtime (~300ms) + 150ms timer; a healthy pump
    // observes exit well inside 15s. Pre-fix: never.
    let done = wait_until(
        &mut ctx,
        "(globalThis.__done && globalThis.__done()) ? 'y' : 'n'",
        120,
    );
    let state = eval_string(
        &mut ctx,
        "(function () { return JSON.stringify(globalThis.__results); })()",
    );
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);
    let _ = std::fs::remove_file(&script_path);

    assert!(
        done,
        "primary must observe worker 'exit' (worker loop drains, no pinning interval); state: {} log: {}",
        state, log
    );
    assert!(
        state.contains("\"exitCode\":0"),
        "worker exit code must surface: {} log: {}",
        state, log
    );
    // The worker actually ran (boot log + drained timer) — proves the child
    // executed the full module and exited by loop drain, not by error.
    assert!(log.contains("C:boot"), "worker must boot: {}", log);
    assert!(log.contains("C:timer"), "worker timer must fire: {}", log);
    assert!(log.contains("P:exit code=0"), "primary exit log: {}", log);
    assert!(
        !log.contains("P:timeout"),
        "primary 8s watchdog must NOT fire (stall regression): {}",
        log
    );
    bun_runtime::shutdown_thread_sm();
}

/// Pump registration surface: cluster.__cluster_pump_register exists and the
/// native pump registry reports liveness while a pins pump returns true.
#[test]
fn test_cluster_pump_register_surface() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let out = eval_string(
        &mut ctx,
        r#"(function () {
var cluster = require('cluster');
if (typeof cluster.__cluster_pump_register !== 'function') return 'missing-register';
var called = 0;
var ok = cluster.__cluster_pump_register(function () { called += 1; return false; }, false);
return 'register=' + ok;
})()"#,
    );
    assert_eq!(out, "register=true", "pump register surface: {}", out);
    // Drive one drain pass: the shim's primary pump runs with zero workers
    // (returns false) and the test's non-pins pump returns false — after
    // which NEITHER contributes loop liveness.
    HOOK_BUDGET.with(|b| b.set(30));
    let _ = eval_string(&mut ctx, "'driven'");
    std::thread::sleep(Duration::from_millis(30));
    HOOK_BUDGET.with(|b| b.set(30));
    let _ = eval_string(&mut ctx, "'driven-2'");
    assert!(
        !bun_runtime::node_cluster::cluster_loop_alive(),
        "idle primary + non-pins (worker) pumps must never keep the loop alive"
    );
    bun_runtime::shutdown_thread_sm();
}
