// @trace REQ-IMPL-005 [level:e2e]
//
// # REQ-IMPL-005-C4 real-binary startup/memory thresholds
//
// Coverage-audit G3 (.plans/coverage-audit-eng-20261014.md): "启动延迟 < 2s /
// 内存开销 < 200MB" had no crate-level threshold carrier — the numbers lived
// only in bench/results engineering evidence. The screenshot ≤500ms leg of
// the same criterion is already carried by `runtime_invariant_latency_tests`
// (library face) and is NOT duplicated here.
//
// Face: the REAL `bao` binary (same locator pattern as
// `bao_cli_timeout_e2e_tests` / `bao_cli_e2e_tests`), CLI runtime bring-up =
// mozjs engine init + Node/Bun API install + eval + exit. This is the
// headless runtime face; the servo browser-mode memory face stays with the
// soak bench evidence (bench/results soak vm_rss family) — browser-mode RSS
// is workload-dominated (page churn over 60min) and not a startup property.
//
//   T1 startup: `bao -e 'console.log("ok")'` — full process wall clock
//      (spawn → engine init → eval → exit) < 2s, and stdout must prove the
//      eval actually ran (an instant parse failure must not count as fast
//      startup). Measured ~40ms on the debug profile → ~50× headroom.
//   T2 memory: `bao -e 'setTimeout(…, 1500)'` holds the process alive while
//      /proc/<pid>/status VmRSS is sampled every 25ms; the peak sample must
//      stay < 200MB (measured ~87MB debug profile). Zero samples is a
//      measurement FAILURE (fail-closed), never a pass; on platforms without
//      /proc the test skips explicitly.
//
// 运行约束: 需要 `cargo build -p bao_bin` 产物;缺失时 skip(同 timeout e2e 约定)。

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "common/mod.rs"]
mod common;

/// REQ-IMPL-005-C4 startup budget: full real-binary run < 2s.
const STARTUP_BUDGET_MS: u128 = 2_000;
/// REQ-IMPL-005-C4 memory budget: 200MB (VmRSS reports KiB).
const MEMORY_BUDGET_KIB: u64 = 200 * 1024;
/// Keep-alive window for the RSS sampling target (the setTimeout duration).
const MEM_PROBE_ALIVE_MS: u64 = 1_500;
/// RSS sampling cadence (~60 samples over the keep-alive window).
const MEM_SAMPLE_INTERVAL_MS: u64 = 25;
/// Hard ceiling for the sampling loop: keep-alive + generous exit slack.
const MEM_PROBE_DEADLINE_MS: u64 = MEM_PROBE_ALIVE_MS + 5_000;

// ─── bao binary locator (shared BCE-20261018-STALEBIN canary) ──────────────

/// Resolve `bao` through the shared three-arm behavior canary
/// (`common::bao_bin_canary`). This file's contract keeps the legacy
/// "binary not built → skip" posture: see `skip_if_no_binary` for the
/// `NotFound` path. A binary that EXISTS but fails the canary is a hard
/// rejection (panic), never a skip.
fn bao_path() -> PathBuf {
    common::bao_bin_canary::find_bao_binary()
}

/// Spawn `bao <args>` with piped stdio via the shared reaping-discipline
/// helper (own process group + PDEATHSIG + reap-on-drop).
fn spawn_bao(args: &[&str]) -> common::GroupChild {
    let bao = bao_path();
    common::GroupChild::spawn(
        Command::new(bao)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .expect("spawn bao failed")
}

/// Peak VmRSS (KiB) of `pid` from /proc — Linux only; None elsewhere or if
/// the process is already gone.
fn read_vm_rss_kib(pid: u32) -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kib = rest.trim().split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kib);
        }
    }
    None
}

fn skip_if_no_binary() -> bool {
    match common::bao_bin_canary::try_find_bao_binary() {
        Ok(_) => false,
        Err(common::bao_bin_canary::BaoBinError::NotFound(why)) => {
            eprintln!("[startup-memory-thresholds] skip: {why}");
            true
        }
        // A candidate exists but failed the three-arm canary: stale/poisoned
        // binaries are rejected loudly, never skipped (BCE-20261018-STALEBIN).
        Err(common::bao_bin_canary::BaoBinError::Rejected(why)) => panic!("{why}"),
    }
}

// ─── 1. REQ-IMPL-005-C4 — startup latency < 2s (real binary) ────────────────

#[test]
// @trace REQ-IMPL-005 [criterion:REQ-IMPL-005-C4] [req:REQ-IMPL-005] [level:e2e]
fn startup_latency_within_2s() {
    if skip_if_no_binary() {
        return;
    }
    let start = Instant::now();
    let mut child = spawn_bao(&["-e", "console.log(\"ok\")"]);
    let output = child.wait_with_output().expect("wait bao -e");
    let elapsed = start.elapsed();

    assert!(
        output.status.success(),
        "bao -e exited {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    // The timing only counts if the engine genuinely brought up and evaluated:
    // an instant parse failure is not a fast startup.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ok"),
        "eval must actually run before timing counts, stdout={stdout:?}"
    );

    eprintln!("[startup-latency REQ-IMPL-005-C4] elapsed={}ms", elapsed.as_millis());
    assert!(
        elapsed.as_millis() < STARTUP_BUDGET_MS,
        "startup latency {}ms >= {STARTUP_BUDGET_MS}ms budget",
        elapsed.as_millis()
    );
}

// ─── 2. REQ-IMPL-005-C4 — runtime memory overhead < 200MB (peak VmRSS) ──────

#[test]
// @trace REQ-IMPL-005 [criterion:REQ-IMPL-005-C4] [req:REQ-IMPL-005] [level:e2e]
fn memory_overhead_rss_within_200mb() {
    if skip_if_no_binary() {
        return;
    }
    // Hold the runtime alive on its event loop so RSS sampling has a stable
    // window; the marker proves the loop genuinely ran (not a crash-at-start
    // whose early-init RSS would understate the real footprint).
    let mut child = spawn_bao(&[
        "-e",
        "setTimeout(() => console.log(\"MEM_PROBE_DONE\"), 1500)",
    ]);
    let pid = child.id();

    let deadline = Instant::now() + Duration::from_millis(MEM_PROBE_DEADLINE_MS);
    let mut peak_kib: u64 = 0;
    loop {
        if let Some(kib) = read_vm_rss_kib(pid) {
            peak_kib = peak_kib.max(kib);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                assert!(
                    status.success(),
                    "mem probe process exited {:?} before its timer fired",
                    status.code()
                );
                break;
            }
            Ok(None) => {}
            Err(e) => panic!("try_wait failed: {e}"),
        }
        if Instant::now() > deadline {
            child.kill_group();
            panic!("mem probe process did not exit within {}ms", MEM_PROBE_DEADLINE_MS);
        }
        std::thread::sleep(Duration::from_millis(MEM_SAMPLE_INTERVAL_MS));
    }

    // Drain the pipe after exit and prove the event loop actually ran.
    let mut stdout_bytes = Vec::new();
    if let Some(mut pipe) = child.stdout().take() {
        let _ = pipe.read_to_end(&mut stdout_bytes);
    }
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    assert!(
        stdout.contains("MEM_PROBE_DONE"),
        "keep-alive timer must have fired for the sampling window to be real, stdout={stdout:?}"
    );

    if peak_kib == 0 {
        if cfg!(target_os = "linux") {
            panic!(
                "no /proc RSS sample collected while the child was alive — measurement failure"
            );
        }
        eprintln!("[startup-memory-thresholds] skip: /proc RSS unavailable on this platform");
        return;
    }

    eprintln!(
        "[memory-overhead REQ-IMPL-005-C4] peak_vm_rss={}KiB ({}MiB) budget={MEMORY_BUDGET_KIB}KiB",
        peak_kib,
        peak_kib / 1024
    );
    assert!(
        peak_kib < MEMORY_BUDGET_KIB,
        "peak RSS {peak_kib}KiB ({}MiB) >= {}KiB (200MB) budget",
        peak_kib / 1024,
        MEMORY_BUDGET_KIB
    );
}
