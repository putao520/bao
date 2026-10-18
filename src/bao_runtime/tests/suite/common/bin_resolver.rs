//! Single-point `bao` binary resolution for spawn-based suite tests
//! (BCE-20261018-STALEBIN consolidation).
//!
//! Before this module, five suite files each carried a private inline copy of
//! the resolution ladder with divergent candidate order and — critically — no
//! argv-identity check on the resolved binary: the 2026-10-18 llvm-cov
//! battery failure was traced to `BAO_TEST_BAO_BIN` pinning a 2026-10-17
//! mid-WIP stale artifact whose unknown-command handler printed canned text
//! instead of echoing argv; every ladder accepted it on an mtime/copy shape.
//! Resolution is now single-point here, and every candidate (explicit pin
//! included) must pass a three-arm behavior canary before it is trusted:
//!
//! 1. `--version` exits 0 with output (generic liveness),
//! 2. `compat --help` exits 0 with output (command-set currency),
//! 3. an unknown-command nonce probe: spawning `bao <nonce>` must echo the
//!    nonce back in its diagnostic. Any binary that fails to reflect argv is
//!    stale, poisoned, or foreign — rejected with the reason on the record.
//!
//! A rejected explicit `BAO_TEST_BAO_BIN` pin panics immediately instead of
//! falling through: the pin is the battery's identity for the binary under
//! test (llvm-cov pins a coverage-instrumented artifact), so silently testing
//! a different tree's binary would be the accident shape again.
//!
//! Boundary (negative-validation A/B/C, 2026-10-18): the nonce arm rejects
//! exactly the "does not reflect argv" class — the accident's canned-text
//! artifact. An argv-reflecting non-bao CLI (e.g. coreutils `uname`, whose
//! extra-operand diagnostic echoes its operand) passes arm 3 by the same
//! property; command-set currency (arm 2) and the per-source mtime gate carry
//! the remaining identity load.
//!
//! Source-mtime gating (`find_bao_binary_gated`) preserves the legacy
//! per-file guards verbatim — including their original scope: the gate
//! applies only to manifest-root-target candidates, exactly as the inline
//! copies did. Cross-check: `src/bao_cli/tests/common/mod.rs` carries the
//! bao_cli copy (no gate). Keep the ladder + canary set in sync.

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

/// Locate the real `bao` binary (debug preferred), canary-clean. Use this in
/// files that carry no per-source mtime gate.
pub fn find_bao_binary() -> PathBuf {
    static RESOLVED: OnceLock<PathBuf> = OnceLock::new();
    RESOLVED.get_or_init(|| resolve_bao_binary(&[])).clone()
}

/// Locate the real `bao` binary, canary-clean AND fresh against
/// `staleness_sources` (manifest-relative paths, e.g. `src/node_cluster.rs`).
/// A workspace-target candidate older than any source measures the previous
/// behavior, not the fix — panic with the rebuild command instead of
/// reporting misleading data (verbatim legacy guard semantics).
pub fn find_bao_binary_gated(staleness_sources: &[&str]) -> PathBuf {
    static RESOLVED: OnceLock<PathBuf> = OnceLock::new();
    RESOLVED.get_or_init(|| resolve_bao_binary(staleness_sources)).clone()
}

/// Candidate ladder (union of the five retired inline copies, canonical
/// order). The explicit `BAO_TEST_BAO_BIN` pin is handled by the caller —
/// first shot at the canary, no silent fall-through on rejection.
///
/// `true` marks the manifest-root-target leg, the only leg the legacy
/// source-mtime gate ever applied to.
fn candidate_paths() -> Vec<(PathBuf, bool)> {
    let mut candidates: Vec<(PathBuf, bool)> = Vec::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());

    // Profile-agnostic probe: this suite exe sits at $TARGET/<profile>/deps/
    // — the bao binary sits at $TARGET/<profile>/bao for EVERY cargo profile
    // (debug/release/test-ci/test-ci-dbg), so derive the sibling instead of
    // guessing the profile.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(profile_dir) = exe.parent().and_then(|d| d.parent()) {
            for name in ["bao", "bao.exe"] {
                push_existing(&mut candidates, profile_dir.join(name), false);
            }
        }
    }
    // Injected-target builds (CARGO_TARGET_DIR env, the farm/CI shape): the
    // workspace target dir is NOT under the manifest — probe the env target
    // or every binary-seeking test fails with a false "not found" despite a
    // fresh bao sitting in the real target tree.
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        for profile in ["debug", "release"] {
            for name in ["bao", "bao.exe"] {
                push_existing(&mut candidates, Path::new(&t).join(profile).join(name), false);
            }
        }
    }
    // Static config injection (.cargo/config.toml [build] target-dir — the
    // single-compile-universe shape where the workspace target dir is NOT
    // under the manifest): parse the key so binary-seeking tests resolve the
    // real target tree in every shell shape (env, config, manifest).
    if let Some(root) = &root {
        if let Ok(text) = std::fs::read_to_string(root.join(".cargo/config.toml")) {
            if let Some(dir) = config_target_dir(&text) {
                for profile in ["debug", "release"] {
                    for name in ["bao", "bao.exe"] {
                        push_existing(
                            &mut candidates,
                            Path::new(&dir).join(profile).join(name),
                            false,
                        );
                    }
                }
            }
        }
    }
    // Manifest-root target tree — the legacy gated leg (debug preferred).
    if let Some(root) = &root {
        for profile in ["debug", "release"] {
            for name in ["bao", "bao.exe"] {
                push_existing(&mut candidates, root.join("target").join(profile).join(name), true);
            }
        }
    }
    // Copied-binary environment fallback: deployed next to this suite exe.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["bao", "bao.exe"] {
                push_existing(&mut candidates, dir.join(name), false);
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

fn push_existing(candidates: &mut Vec<(PathBuf, bool)>, candidate: PathBuf, gated: bool) {
    if candidate.is_file() && !candidates.iter().any(|(p, _)| p == &candidate) {
        candidates.push((candidate, gated));
    }
}

fn resolve_bao_binary(staleness_sources: &[&str]) -> PathBuf {
    // Explicit pin: first shot at the canary, no silent fall-through on
    // rejection (see module doc).
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
            if let Some(candidate) = gated_root_target_check(&pin, staleness_sources) {
                panic!("{candidate}");
            }
            return pin;
        }
        // Not a file → fall through to the ladder (legacy copied-binary
        // semantics: the override only applies when the path exists).
    }

    let mut rejections: Vec<String> = Vec::new();
    for (candidate, gated) in candidate_paths() {
        match run_behavior_canary(&candidate) {
            Ok(()) => {
                if gated {
                    if let Some(violation) = gated_root_target_check(&candidate, staleness_sources)
                    {
                        panic!("{violation}");
                    }
                }
                return candidate;
            }
            Err(why) => rejections.push(format!("  {}: {why}", candidate.display())),
        }
    }
    let detail = if rejections.is_empty() {
        "  (no candidate found on the ladder)".to_string()
    } else {
        rejections.join("\n")
    };
    panic!(
        "no usable `bao` binary — all candidates failed the behavior canary \
         (--version / `compat --help` / unknown-command nonce echo):\n{detail}\n\
         Build it with `cargo build -p bao_bin` (required as the cluster/fetch \
         regression executable), or point BAO_TEST_BAO_BIN at a fresh \
         canary-clean binary.",
    );
}

/// Legacy source-mtime gate, verbatim scope and message: fires only where the
/// caller passed sources, and only against a workspace-target candidate.
/// Returns `Some(message)` on violation (caller panics).
fn gated_root_target_check(bin: &Path, sources: &[&str]) -> Option<String> {
    if sources.is_empty() {
        return None;
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bin_mtime = bin.metadata().and_then(|m| m.modified()).ok();
    for src in sources {
        let src_mtime = manifest
            .join(src)
            .metadata()
            .and_then(|m| m.modified())
            .ok();
        if let (Some(b), Some(s)) = (bin_mtime, src_mtime) {
            if b < s {
                return Some(format!(
                    "stale bao binary ({bin:?} older than {src}) — rebuild with \
                     `cargo build -p bao_bin` before running this regression"
                ));
            }
        }
    }
    None
}

/// Three-arm behavior canary. All three arms are bounded spawns; any failure
/// names the arm and the observed shape so the rejection is auditable.
fn run_behavior_canary(bin: &Path) -> Result<(), String> {
    // Arm 1 — generic liveness: `--version` must exit 0 with output.
    let out = bounded_probe(bin, &["--version"])?;
    if out.code != Some(0) || out.stdout.is_empty() {
        return Err(format!(
            "--version exited {:?} (stdout {}B)",
            out.code,
            out.stdout.len()
        ));
    }
    // Arm 2 — command-set currency (battery-v7 stale-binary lesson): a binary
    // predating the current command set fabricates golden/faces drift.
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
/// unknown candidate may hang or spew; the canary must never wedge the suite
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
