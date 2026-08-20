// @trace TEST-ENG-006-CLUSTER [req:REQ-ENG-006] [level:e2e+integration]
// #64 regression: cluster.isPrimary non-deterministic flip-to-false.
//
// Root cause class (pre-fix): the worker classification froze LAZILY at the
// first `node_cluster::install`. Any std::env write between process birth
// and that first install — `process.env.X = v` bridges to
// std::env::set_var, and multi-realm hosts (PagePool/embedder/test
// harnesses) create realms lazily, under parallel test execution with
// scheduler-dependent ordering — froze a polluted value process-wide.
// Fix: an .init_array ctor snapshots BAO_CLUSTER_WORKER_ID at process
// birth (pre-main, before any realm or env bridge exists).
//
// 1. env_write_before_any_realm_stays_primary — writes the worker env var
//    BEFORE any JSContext/realm exists in this process (the exact race
//    window), then installs globals and reads cluster.isPrimary.
//    Pre-fix: false (deterministic in this minimal binary — the writer
//    provably precedes the first installer); post-fix: true.
// 2. cold_start_large_script_x50_is_primary — spawns the real bao binary
//    on a ~0.5MB script 50 times; every run must classify isPrimary at
//    script start AND end. Fresh-process cold start carries no in-process
//    writer, so this pins the exec-env contract end to end.
// 3. fork_env_child_classifies_worker_x10 — spawns `bao run` with the env
//    cluster.fork() builds (BAO_CLUSTER_WORKER_ID=1/PRIMARY_PID/IPC_FD);
//    every child must classify isWorker===true && isPrimary===false.
//
// NOTE (2)/(3) exec target/debug/bao — the binary must be current or these
// tests measure a stale runtime; a mtime guard panics with the rebuild
// command instead of reporting misleading data.

use std::path::PathBuf;
use std::process::Command;

/// Locate the bao binary built in this workspace (debug preferred).
fn find_bao_binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("target");
    for profile in ["debug", "release"] {
        let candidate = target.join(profile).join("bao");
        if candidate.exists() {
            // Staleness guard: the cluster classification lives in
            // src/node_cluster.rs — a binary older than that file measures
            // the previous behavior, not the fix.
            let bin_mtime = candidate.metadata().and_then(|m| m.modified()).ok();
            let src_mtime = manifest
                .join("src/node_cluster.rs")
                .metadata()
                .and_then(|m| m.modified())
                .ok();
            if let (Some(b), Some(s)) = (bin_mtime, src_mtime) {
                assert!(
                    b >= s,
                    "stale bao binary ({:?} older than node_cluster.rs) — rebuild with \
                     `cargo build -p bao_bin` before running this regression",
                    candidate
                );
            }
            return candidate;
        }
    }
    panic!(
        "bao binary not found under {} — build it with `cargo build -p bao_bin`",
        target.display()
    );
}

/// Spawn `bao run <script>` with the cluster control vars scrubbed (primary
/// contract: an env WITHOUT a fork-issued worker id classifies primary).
fn bao_run_scrubbed(bao: &PathBuf, script: &std::path::Path) -> std::process::Output {
    Command::new(bao)
        .arg("run")
        .arg(script)
        .env_remove("BAO_CLUSTER_WORKER_ID")
        .env_remove("BAO_CLUSTER_PRIMARY_PID")
        .env_remove("BAO_CLUSTER_IPC_FD")
        .output()
        .expect("spawn bao run")
}

/// Generate a ~0.5MB "large script": require('cluster') first, thousands of
/// declarations (parse + instantiation pressure), a work loop, and the
/// isPrimary probe at both start and end.
fn write_large_script(path: &std::path::Path) {
    let mut src = String::with_capacity(600_000);
    src.push_str("const cluster = require('cluster');\n");
    src.push_str("console.log('BAO_ISPRIMARY_START:' + cluster.isPrimary);\n");
    src.push_str("let sink = 0;\n");
    for i in 0..8000u32 {
        src.push_str(&format!(
            "function f{i}(a, b) {{ let s = {i}; for (let k = 0; k < 3; k++) s += (a * {i} + b * k) | 0; return s; }} class C{i} {{ m() {{ return f{i}({i}, {i} % 7); }} }}\n"
        ));
        if i % 1000 == 0 {
            src.push_str(&format!("sink += f{i}({i}, 2);\n"));
        }
    }
    src.push_str("for (let i = 0; i < 8000; i++) { try { sink += (new Function('x', 'return x + 1;'))(i); } catch (e) {} }\n");
    src.push_str("if (sink === -1) console.log('impossible');\n");
    src.push_str("console.log('BAO_ISPRIMARY_END:' + cluster.isPrimary);\n");
    std::fs::write(path, src).expect("write large script");
}

#[test]
fn env_write_before_any_realm_stays_primary() {
    // The race window, pinned: the worker env var enters the PROCESS env
    // before any realm exists (a host writing env early — the bun_api env
    // bridge does exactly this from JS, and PagePool/embedder hosts run
    // early code before lazily creating the first full realm).
    //
    // SAFETY: single-threaded test start; no other test in this binary
    // creates a realm before this one.
    unsafe { std::env::set_var("BAO_CLUSTER_WORKER_ID", "7") };

    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = bao_engine::context::JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    let out = match ctx.eval(
        "String(require('cluster').isPrimary)",
        "<cluster-isprimary-regression>",
    ) {
        Ok(bao_engine::value::JsValue::String(s)) => s,
        Ok(bao_engine::value::JsValue::Bool(b)) => b.to_string(),
        Ok(other) => panic!("unexpected eval result: {:?}", other),
        Err(e) => panic!("eval failed: {}", e.message),
    };

    // Cleanup FIRST so an assertion failure still leaves the env clean for
    // any process spawned by sibling tests.
    unsafe { std::env::remove_var("BAO_CLUSTER_WORKER_ID") };

    assert_eq!(
        out, "true",
        "#64 regression: env written before the first realm must NOT flip isPrimary \
         (classification is a process-birth property, not a mutable-env property)"
    );
}

#[test]
fn cold_start_large_script_x50_is_primary() {
    let bao = find_bao_binary();
    let script = std::env::temp_dir().join("bao_isprimary_coldstart_big.js");
    write_large_script(&script);

    let mut flips = 0usize;
    for run in 0..50 {
        let out = bao_run_scrubbed(&bao, &script);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let start_ok = stdout.contains("BAO_ISPRIMARY_START:true");
        let end_ok = stdout.contains("BAO_ISPRIMARY_END:true");
        let flipped = stdout.contains("BAO_ISPRIMARY_START:false")
            || stdout.contains("BAO_ISPRIMARY_END:false");
        if flipped || !start_ok || !end_ok {
            flips += 1;
            eprintln!(
                "cold-start run {}: start_ok={} end_ok={} flipped={} stdout={:?} stderr={:?}",
                run,
                start_ok,
                end_ok,
                flipped,
                stdout,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    let _ = std::fs::remove_file(&script);
    assert_eq!(
        flips, 0,
        "cold-start isPrimary flips: {}/50 runs (must be 0/50)",
        flips
    );
}

#[test]
fn fork_env_child_classifies_worker_x10() {
    let bao = find_bao_binary();
    let script = std::env::temp_dir().join("bao_isprimary_worker_probe.js");
    std::fs::write(
        &script,
        r#"
const cluster = require('cluster');
console.log('BAO_CLASS:' + (cluster.isWorker ? 'worker' : 'primary') + ':' + cluster.isPrimary);
process.exit(0);
"#,
    )
    .expect("write worker probe script");

    let mut wrong = 0usize;
    for worker_id in 1..=10 {
        let out = Command::new(&bao)
            .arg("run")
            .arg(&script)
            // The exact env cluster.fork() builds for its child
            // (env_map.insert in node_cluster::cluster_fork).
            .env("BAO_CLUSTER_WORKER_ID", worker_id.to_string())
            .env("BAO_CLUSTER_PRIMARY_PID", std::process::id().to_string())
            .env("BAO_CLUSTER_IPC_FD", "3")
            .output()
            .expect("spawn bao run worker");
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !stdout.contains("BAO_CLASS:worker:false") {
            wrong += 1;
            eprintln!(
                "worker-env run {}: stdout={:?} stderr={:?}",
                worker_id,
                stdout,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    let _ = std::fs::remove_file(&script);
    assert_eq!(
        wrong, 0,
        "fork-env children misclassifying as primary: {}/10 (must be 0/10)",
        wrong
    );
}
