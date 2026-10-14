// @trace REQ-ENG-004 [level:e2e]
//
// # REQ-ENG-004 AC04/AC05 — macro→micro 顺序 CLI 级二进制 e2e
//
// (coverage-audit-eng-20261014 G2-6:Rust 层 job_queue order 断言在案,缺
//  二进制 `-e` 级顺序输出断言 — 本文件补该缺口)
//
// Real-binary subprocess tests(same spawn skeleton as
// bao_cli_timeout_e2e_tests:GroupChild + hard-deadline poll + drain)。
//
// 语义真源 = HTML spec event loop:microtask checkpoint 在下一个 macrotask
// 之前 drain(bao_engine job_queue 的 RunJobs 在 eval 尾 + timers drain 头)。
//
//   T1  `bao -e` 同时排队 setTimeout(0) 'macro' 与
//       Promise.resolve().then 'micro' → stdout 序 = micro 先于 macro
//       (脚本体结束后微任务 checkpoint 先于零延时定时器宏任务)。
//   T2  SPEC AC05 字面例:同步 console.log('macro') + 已排队微任务
//       'micro' → macro 先(同步语句不属于宏任务,先于微任务输出)。
//   T3  宏任务间 checkpoint:t1 的回调内排队微任务 m1,另一宏任务 t2
//       (20ms 隔开 deadline)→ t1, m1, t2(m1 的 checkpoint 在 t2 之前)。
//
// **运行约束**: 需要预先 `cargo build -p bao_bin` 产出 bao 二进制;缺失时
// fail(参照 bao_cli_timeout_e2e_tests 的 fail-loud 形态)。

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[path = "common/mod.rs"]
mod common;

/// Hard deadline for one `-e` child: engine bring-up + a 0ms/20ms timer
/// round-trip is well under a second; 30s is generous CI slack while still
/// converting a hang regression into a bounded red (e204 reaping discipline).
const CHILD_DEADLINE_MS: u64 = 30_000;

// ─── 辅助 — 定位 bao 二进制(与 bao_cli_timeout_e2e_tests 相同的定位序) ────

fn bao_path() -> Option<PathBuf> {
    if let Ok(override_path) = std::env::var("BAO_BIN") {
        let candidate = PathBuf::from(override_path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Ok(p) = std::env::var("BAO_TEST_BAO_BIN") {
        let candidate = PathBuf::from(p);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // Profile-agnostic probe: this suite exe sits at $TARGET/<profile>/deps/
    // — the bao binary sits at $TARGET/<profile>/bao for EVERY cargo profile.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(profile_dir) = exe.parent().and_then(|d| d.parent()) {
            for name in ["bao", "bao.exe"] {
                let candidate = profile_dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        for profile in ["test-ci", "debug", "release"] {
            for name in ["bao", "bao.exe"] {
                let candidate = PathBuf::from(&target_dir).join(profile).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let mut here = std::env::current_dir().ok()?;
    for _ in 0..5 {
        for profile in ["test-ci", "debug", "release"] {
            for name in ["bao", "bao.exe"] {
                let candidate = here.join("target").join(profile).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        if !here.pop() {
            break;
        }
    }
    None
}

/// Spawn `bao <args>` with piped stdio, own process group + PDEATHSIG armed
/// (e204 reaping discipline — see `common::GroupChild`).
fn spawn_bao(args: &[&str]) -> common::GroupChild {
    let bao = bao_path().expect("bao binary not found — run `cargo build -p bao_bin` first");
    common::GroupChild::spawn(
        Command::new(bao)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .expect("spawn bao failed")
}

/// Poll-wait with a hard deadline: on breach the whole process group is
/// SIGKILLed and the child reaped, then `None` returned (never blocks on a
/// hang regression).
fn wait_with_deadline(child: &mut common::GroupChild, limit: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if start.elapsed() >= limit {
            child.kill_group();
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Drain piped stdout/stderr after the child exited (the marker outputs are
/// tiny — the pipes never fill, so polling above cannot deadlock).
fn drain_pipes(child: &mut common::GroupChild) -> (String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout().take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr().take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    (stdout, stderr)
}

/// Run one `bao -e <script>` child to completion under the hard deadline and
/// return (stdout, stderr, exit code). Panics with full captured output when
/// the child fails to terminate — the deadline path must stay observable.
fn run_eval(script: &str) -> (String, String, i32) {
    let mut child = spawn_bao(&["-e", script]);
    let status = wait_with_deadline(&mut child, Duration::from_millis(CHILD_DEADLINE_MS));
    let (stdout, stderr) = drain_pipes(&mut child);
    let Some(status) = status else {
        panic!(
            "bao -e did not terminate within {}ms; stdout={:?} stderr={:?}",
            CHILD_DEADLINE_MS, stdout, stderr
        );
    };
    let code = status.code().unwrap_or(-1);
    (stdout, stderr, code)
}

/// Byte-position ordering assertion over one child run: every marker must
/// appear exactly once and in the given relative order. Position-based (not
/// line-split) so a trailing completion-value print cannot reorder anything.
fn assert_marker_order(stdout: &str, stderr: &str, code: i32, markers: &[&str]) {
    assert_eq!(
        code, 0,
        "clean script must exit 0; stdout={:?} stderr={:?}",
        stdout, stderr
    );
    assert!(
        !stderr.contains("Error:"),
        "unexpected JS/runtime error on stderr: {:?}",
        stderr
    );
    let mut last: Option<usize> = None;
    for marker in markers {
        let positions: Vec<usize> = stdout
            .match_indices(marker)
            .map(|(idx, _)| idx)
            .collect();
        assert_eq!(
            positions.len(),
            1,
            "marker {:?} must appear exactly once, got {:?}; stdout={:?} stderr={:?}",
            marker,
            positions.len(),
            stdout,
            stderr
        );
        let pos = positions[0];
        if let Some(prev) = last {
            assert!(
                pos > prev,
                "output order violated: {:?} (at {}) must come after previous marker (at {}); \
                 stdout={:?} stderr={:?}",
                marker,
                pos,
                prev,
                stdout,
                stderr
            );
        }
        last = Some(pos);
    }
}

// ─── T1: microtask checkpoint 先于零延时定时器宏任务 ─────────────────────────

/// @trace REQ-ENG-004 [level:e2e] — the script queues BOTH a zero-delay
/// setTimeout macrotask ('macro') and a promise microtask ('micro'); the
/// HTML-spec microtask checkpoint runs the microtask BEFORE the timer's
/// macrotask, so stdout order is micro → macro.
#[test]
fn cli_eval_microtask_drains_before_zero_delay_timer_macrotask() {
    let script = "\
setTimeout(function () { console.log('macro'); }, 0);
Promise.resolve().then(function () { console.log('micro'); });
undefined;
";
    let (stdout, stderr, code) = run_eval(script);
    assert_marker_order(&stdout, &stderr, code, &["micro", "macro"]);
}

// ─── T2: SPEC AC05 字面例 — 同步语句先于已排队微任务 ─────────────────────────

/// @trace REQ-ENG-004 [level:e2e] — the SPEC's literal AC05 example:
/// `bao -e "Promise.resolve().then(()=>console.log('micro'));console.log('macro')"`
/// must print macro first (a synchronous statement is not a macrotask — it
/// finishes inside the running script, before the queued microtask drains).
#[test]
fn cli_eval_sync_statement_precedes_queued_microtask() {
    let script = "\
Promise.resolve().then(function () { console.log('micro'); });
console.log('macro');
";
    let (stdout, stderr, code) = run_eval(script);
    assert_marker_order(&stdout, &stderr, code, &["macro", "micro"]);
}

// ─── T3: 宏任务之间的 microtask checkpoint ──────────────────────────────────

/// @trace REQ-ENG-004 [level:e2e] — checkpoint BETWEEN macrotasks: t1's
/// callback queues a microtask ('m1'); a second timer 't2' (20ms — deadline
/// separated so the fire order is deterministic) must run only after m1.
/// Expected stdout order: t1, m1, t2.
#[test]
fn cli_eval_microtask_checkpoint_runs_between_timer_macrotasks() {
    let script = "\
setTimeout(function () {
  console.log('t1');
  Promise.resolve().then(function () { console.log('m1'); });
}, 0);
setTimeout(function () { console.log('t2'); }, 20);
undefined;
";
    let (stdout, stderr, code) = run_eval(script);
    assert_marker_order(&stdout, &stderr, code, &["t1", "m1", "t2"]);
}
