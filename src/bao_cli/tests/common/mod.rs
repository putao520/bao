//! Shared bao_cli integration-test helpers (BCE-20261018-STALEBIN).
//!
//! Single-point `bao` binary resolution for every bao_cli test binary
//! (cli_snapshot / cli_faces; cli_dispatch is pure-logic and spawns nothing).
//! Previously cli_snapshot.rs and cli_faces.rs each carried a private inline
//! copy of the ladder with a DIFFERENT liveness canary (`compat --help` vs
//! `--version`) and no argv-identity check — the 2026-10-18 llvm-cov battery
//! failure traced to `BAO_TEST_BAO_BIN` pinning a 2026-10-17 mid-WIP stale
//! artifact whose unknown-command handler printed canned text instead of
//! echoing argv; a mtime-fresh, behaviorally wrong binary passed both legacy
//! canaries on some ladders and fabricated golden drift against a healthy
//! product. Resolution is now single-point with a three-arm behavior canary
//! (see `run_behavior_canary`), and a rejected explicit pin panics instead of
//! silently falling through to another tree's binary.
//!
//! Boundary (negative-validation A/B/C, 2026-10-18): the nonce arm rejects
//! exactly the "does not reflect argv" class — the accident's canned-text
//! artifact. An argv-reflecting non-bao CLI (e.g. coreutils `uname`, whose
//! extra-operand diagnostic echoes its operand) passes arm 3 by the same
//! property; command-set currency (arm 2) and the golden/faces locks carry
//! the remaining identity load.
//!
//! SIBLING COPY: `src/bao_runtime/tests/suite/common/bin_resolver.rs` carries
//! the runtime-suite copy plus the per-source mtime gate this crate does not
//! need. Keep the ladder + canary set in sync.

#![allow(dead_code)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Upper bound for one canary probe spawn. A real `bao` answers in ~20ms;
/// 15s is the repo's bounded-wait convention and only trips on a wedged or
/// pathological candidate.
const CANARY_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Locate the real `bao` binary (W8/p0-cluster lineage: explicit override →
/// sibling-of-test-exe → CARGO_TARGET_DIR → config target-dir → manifest
/// target tree → copied-binary fallback; both OS spellings probed everywhere),
/// then prove it is the binary the golden locks mean to test.
///
/// Every candidate must pass the three-arm behavior canary before it is
/// trusted; the first canary-clean candidate wins and the choice is cached
/// for the life of this test process. A `BAO_TEST_BAO_BIN` pin that exists
/// but fails the canary panics immediately — the pin is the battery's
/// identity for the binary under test, so falling through would test the
/// wrong artifact.
pub fn find_bao_binary() -> std::path::PathBuf {
    static RESOLVED: OnceLock<PathBuf> = OnceLock::new();
    RESOLVED.get_or_init(resolve_bao_binary).clone()
}

fn resolve_bao_binary() -> PathBuf {
    if let Ok(p) = std::env::var("BAO_TEST_BAO_BIN") {
        let pin = PathBuf::from(&p);
        if pin.is_file() {
            if let Err(why) = run_behavior_canary(&pin) {
                panic!(
                    "BAO_TEST_BAO_BIN is pinned to a binary that failed the behavior canary — \
                     refusing to silently fall through to another tree's binary (the pin is \
                     what the battery means to test).\n  pinned: {}\n  {why}\n\
                     Rebuild (`cargo build -p bao_bin --profile test-ci --jobs 4`) and re-pin, \
                     or unset BAO_TEST_BAO_BIN to let the ladder pick a canary-clean binary.",
                    pin.display()
                );
            }
            return pin;
        }
        // Not a file → fall through to the ladder (legacy semantics: the
        // override only applies when the path exists).
    }

    let mut rejections: Vec<String> = Vec::new();
    for candidate in candidate_paths() {
        match run_behavior_canary(&candidate) {
            Ok(()) => return candidate,
            Err(why) => rejections.push(format!("  {}: {why}", candidate.display())),
        }
    }
    let detail = if rejections.is_empty() {
        "  (no candidate found on the ladder)".to_string()
    } else {
        rejections.join("\n")
    };
    panic!(
        "stale or unusable `bao` binary — all candidates failed the behavior canary \
         (--version / `compat --help` / unknown-command nonce echo):\n{detail}\n\
         Rebuild the binary for THIS profile (cargo build -p bao_bin --profile test-ci \
         --jobs 4) or point BAO_TEST_BAO_BIN at a fresh one.\n\
         (The snapshot tests must never compare golden faces against a binary \
         that predates the current command set or fails to reflect argv — that \
         fabricates drift.)"
    );
}

/// Candidate ladder (union of the two retired inline copies, canonical
/// order). The explicit `BAO_TEST_BAO_BIN` pin is handled by the caller.
fn candidate_paths() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    // Profile-agnostic probe: the test exe sits at $TARGET/<profile>/deps/
    // — the bao binary sits at $TARGET/<profile>/bao for EVERY cargo profile.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(profile_dir) = exe.parent().and_then(|d| d.parent()) {
            for name in ["bao", "bao.exe"] {
                push_existing(&mut candidates, profile_dir.join(name));
            }
        }
    }
    // Injected-target builds (CARGO_TARGET_DIR env, the farm/CI shape).
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        for profile in ["debug", "release"] {
            for name in ["bao", "bao.exe"] {
                push_existing(&mut candidates, Path::new(&t).join(profile).join(name));
            }
        }
    }
    // Static config injection (.cargo/config.toml [build] target-dir — the
    // single-compile-universe shape where the workspace target dir is NOT
    // under the manifest).
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());
    if let Some(root) = &root {
        if let Ok(text) = std::fs::read_to_string(root.join(".cargo/config.toml")) {
            if let Some(dir) = config_target_dir(&text) {
                for profile in ["debug", "release"] {
                    for name in ["bao", "bao.exe"] {
                        push_existing(&mut candidates, Path::new(&dir).join(profile).join(name));
                    }
                }
            }
        }
    }
    // Manifest-root target tree (test-ci first: the battery shape).
    if let Some(root) = &root {
        for profile in ["test-ci", "debug", "release"] {
            for name in ["bao", "bao.exe"] {
                push_existing(&mut candidates, root.join("target").join(profile).join(name));
            }
        }
    }
    // Copied-binary environment fallback: deployed next to this test exe.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["bao", "bao.exe"] {
                push_existing(&mut candidates, dir.join(name));
            }
        }
    }
    candidates
}

/// Parse the `[build] target-dir` value out of a `.cargo/config.toml` body.
fn config_target_dir(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.trim_start().starts_with("target-dir"))?;
    let idx = line.find('"')?;
    line[idx + 1..].split('"').next().map(str::to_string)
}

fn push_existing(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_file() && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

/// Three-arm behavior canary (see module doc). All three arms are bounded
/// spawns; any failure names the arm and the observed shape so the rejection
/// is auditable.
fn run_behavior_canary(bin: &Path) -> Result<(), String> {
    // Arm 1 — generic liveness (cli_faces lineage): `--version` must exit 0
    // with output.
    let out = bounded_probe(bin, &["--version"])?;
    if out.code != Some(0) || out.stdout.is_empty() {
        return Err(format!(
            "--version exited {:?} (stdout {}B)",
            out.code,
            out.stdout.len()
        ));
    }
    // Arm 2 — command-set currency (cli_snapshot lineage, battery-v7 lesson):
    // a binary predating the current command set fabricates golden drift.
    let out = bounded_probe(bin, &["compat", "--help"])?;
    if out.code != Some(0) || out.stdout.is_empty() {
        return Err(format!(
            "compat --help exited {:?} (stdout {}B)",
            out.code,
            out.stdout.len()
        ));
    }
    // Arm 3 — argv-reflection identity (the BCE-20261018-STALEBIN
    // discriminator): a genuine `bao` echoes an unrecognized subcommand in its
    // clap diagnostic. The poisoned stale artifact printed canned text for ANY
    // unknown command; a binary that fails to reflect this nonce is stale,
    // poisoned, or foreign. The nonce is process-unique so a lucky canned
    // string cannot pass twice.
    let nonce = canary_nonce();
    let out = bounded_probe(bin, &[nonce.as_str()])?;
    let echoed = String::from_utf8_lossy(&out.stderr).contains(nonce.as_str())
        || String::from_utf8_lossy(&out.stdout).contains(nonce.as_str());
    if !echoed {
        return Err(format!(
            "unknown-command nonce probe: spawned `{nonce}` but neither stream echoed it back \
             (exit {:?}, stderr {}B, stdout {}B) — this binary does not reflect argv; it is \
             stale, poisoned, or foreign",
            out.code,
            out.stderr.len(),
            out.stdout.len()
        ));
    }
    Ok(())
}

fn canary_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    format!("stalebin-canary-{nanos:x}-{}", std::process::id())
}

struct ProbeOutput {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Spawn a canary probe with bounded wait and concurrent pipe drain (an
/// unknown candidate may hang or spew; the canary must never wedge the test
/// on it).
fn bounded_probe(bin: &Path, args: &[&str]) -> Result<ProbeOutput, String> {
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    }

    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let stdout_thread = drain(child.stdout.take());
    let stderr_thread = drain(child.stderr.take());

    let deadline = Instant::now() + CANARY_PROBE_TIMEOUT;
    // The loop evaluates to ExitStatus: every non-break arm returns Err.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("probe timed out after {CANARY_PROBE_TIMEOUT:?}"));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("try_wait failed: {e}"));
            }
        }
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| "stdout reader panicked".to_string())?;
    let stderr = stderr_thread
        .join()
        .map_err(|_| "stderr reader panicked".to_string())?;
    Ok(ProbeOutput {
        code: status.code(),
        stdout,
        stderr,
    })
}
