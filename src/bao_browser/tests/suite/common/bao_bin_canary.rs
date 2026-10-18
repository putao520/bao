//! Single-point `bao` binary resolution + three-arm behavior canary
//! (BCE-20261018-STALEBIN sweep closure, spawn-based bao_browser suite).
//!
//! Source of truth for the canary semantics:
//! `src/bao_runtime/tests/suite/common/bin_resolver.rs` (runtime suite) and
//! `src/bao_cli/tests/common/mod.rs` (bao_cli). Those two are per-crate
//! siblings; this module is the bao_browser copy, extended with two
//! properties this crate's consumers need:
//!
//! 1. **Legacy `BAO_BIN` pin** — `scripts/run-tests.sh` exports `BAO_BIN`
//!    (and `scripts/local-smoke.sh` consumes it), and the pre-consolidation
//!    inline ladders in this suite checked it first. It is honored here and
//!    verified through the same canary: a pinned binary is exactly what the
//!    battery means to test, so it must never be trusted on existence alone.
//!    Precedence is canonical-first (`BAO_TEST_BAO_BIN` then `BAO_BIN`) so an
//!    explicit battery pin cannot be shadowed by a stale `BAO_BIN` left in a
//!    shell — the accident shape this BCE exists to prevent.
//! 2. **Tri-state failure** — [`BaoBinError::NotFound`] (nothing on the
//!    ladder: the binary simply was not built) is distinct from
//!    [`BaoBinError::Rejected`] (candidates exist but none passes the
//!    canary: stale/poisoned/foreign). Callers whose contract is "not built →
//!    skip" may skip on `NotFound`, but `Rejected` always panics: a
//!    stale/poisoned binary must never silently leave the battery.
//!
//! Three behavior arms (all bounded spawns, any failure names the arm):
//!
//! 1. `--version` exits 0 with output (generic liveness),
//! 2. `compat --help` exits 0 with output (command-set currency — a binary
//!    predating the current command set fabricates golden drift),
//! 3. an unknown-command nonce probe: spawning `bao <nonce>` must echo the
//!    nonce back in its diagnostic. Any binary that fails to reflect argv is
//!    stale, poisoned, or foreign — rejected with the reason on the record.
//!
//! A rejected explicit pin panics immediately instead of falling through: the
//! pin is the battery's identity for the binary under test.
//!
//! Boundary (negative-validation A/B/C, 2026-10-18): the nonce arm rejects
//! exactly the "does not reflect argv" class — the accident's canned-text
//! artifact. An argv-reflecting non-bao CLI (e.g. coreutils `uname`, whose
//! extra-operand diagnostic echoes its operand) passes arm 3 by the same
//! property; command-set currency (arm 2) and the golden/faces locks carry
//! the remaining identity load.
//!
//! Cross-crate inclusion: `src/bao_cdp_client/tests/suite/e2e_playwright_compat.rs`
//! pulls this exact file in with `#[path = "../../../bao_browser/tests/suite/common/bao_bin_canary.rs"]`
//! — one physical source, no copy drift (the BCE's root cause was divergent
//! inline copies). Both suites live at `src/<crate>`, so the manifest-root
//! derivation below (two parents up) is identical for both. No workspace
//! dependency edge is added.

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

/// Why binary resolution failed. The split matters: `NotFound` preserves the
/// legacy "not built → skip" contract, while `Rejected` is the BCE's
/// discriminator and must always be loud (panic), never a skip.
#[derive(Debug)]
pub enum BaoBinError {
    /// No candidate exists anywhere on the ladder (the binary was not built,
    /// and no explicit pin points at an existing file).
    NotFound(String),
    /// At least one candidate exists but none passed the three-arm behavior
    /// canary, or an explicit pin exists and failed it. Stale, poisoned, or
    /// foreign binaries land here and are never silently skipped.
    Rejected(String),
}

impl std::fmt::Display for BaoBinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BaoBinError::NotFound(why) | BaoBinError::Rejected(why) => f.write_str(why),
        }
    }
}

/// Locate the real `bao` binary and prove it is the binary the battery means
/// to test. Panics on both failure kinds (`NotFound` included): callers of
/// this entry point cannot run without a verified binary.
pub fn find_bao_binary() -> PathBuf {
    match try_find_bao_binary() {
        Ok(path) => path,
        Err(why) => panic!("{why}"),
    }
}

/// Non-panicking probe for callers whose historical contract is "binary not
/// built → skip". `Ok` is cached for the life of the test process; a
/// `Rejected` outcome is returned so the caller can panic loudly.
pub fn try_find_bao_binary() -> Result<PathBuf, BaoBinError> {
    static RESOLVED: OnceLock<PathBuf> = OnceLock::new();
    if let Some(resolved) = RESOLVED.get() {
        return Ok(resolved.clone());
    }
    let resolved = resolve_bao_binary()?;
    Ok(RESOLVED.get_or_init(|| resolved).clone())
}

/// Presence assertion helper: `true` when a canary-clean binary is available,
/// `false` only when nothing was built. A stale/poisoned candidate panics —
/// presence assertions must not turn the BCE's rejection into a silent skip.
pub fn bao_binary_available() -> bool {
    match try_find_bao_binary() {
        Ok(_) => true,
        Err(BaoBinError::NotFound(_)) => false,
        Err(BaoBinError::Rejected(why)) => panic!("{why}"),
    }
}

fn resolve_bao_binary() -> Result<PathBuf, BaoBinError> {
    // Explicit pins, canonical battery identity first (see module doc). A
    // non-file pin falls through to the ladder — legacy override semantics:
    // the override only applies when the path exists.
    for var in ["BAO_TEST_BAO_BIN", "BAO_BIN"] {
        if let Ok(value) = std::env::var(var) {
            let pin = PathBuf::from(&value);
            if pin.is_file() {
                if let Err(why) = run_behavior_canary(&pin) {
                    return Err(BaoBinError::Rejected(format!(
                        "{var} is pinned to a binary that failed the behavior canary — \
                         refusing to silently fall through to another tree's binary (the pin \
                         is what the battery means to test).\n  pinned: {}\n  {why}\n\
                         Rebuild (`cargo build -p bao_bin --profile test-ci --jobs 4`) and \
                         re-pin, or unset {var} to let the ladder pick a canary-clean binary.",
                        pin.display()
                    )));
                }
                return Ok(pin);
            }
        }
    }

    let candidates = candidate_paths();
    if candidates.is_empty() {
        return Err(BaoBinError::NotFound(
            "no usable `bao` binary found — nothing on the resolution ladder \
             ($TARGET/<profile>/bao, CARGO_TARGET_DIR, .cargo/config.toml target-dir, \
             manifest target/, next to the test exe) and no existing explicit pin.\n\
             Build it with `cargo build -p bao_bin`, or point BAO_TEST_BAO_BIN at a fresh \
             canary-clean binary."
                .to_string(),
        ));
    }

    let mut rejections: Vec<String> = Vec::new();
    for candidate in candidates {
        match run_behavior_canary(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(why) => rejections.push(format!("  {}: {why}", candidate.display())),
        }
    }
    Err(BaoBinError::Rejected(format!(
        "stale or unusable `bao` binary — all candidates failed the behavior canary \
         (--version / `compat --help` / unknown-command nonce echo):\n{}\n\
         Rebuild the binary for THIS profile (cargo build -p bao_bin --profile test-ci \
         --jobs 4) or point BAO_TEST_BAO_BIN at a fresh one.\n\
         (Spawn-based tests must never run against a binary that predates the current \
         command set or fails to reflect argv — that fabricates drift.)",
        rejections.join("\n")
    )))
}

/// Candidate ladder (profile-agnostic sibling → injected target dir → static
/// config target dir → manifest target tree → copied-binary fallback; both OS
/// spellings probed everywhere). Explicit pins are handled by the caller.
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
        for profile in ["test-ci", "debug", "release"] {
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
                for profile in ["test-ci", "debug", "release"] {
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
    // Arm 1 — generic liveness: `--version` must exit 0 with output.
    let out = bounded_probe(bin, &["--version"])?;
    if out.code != Some(0) || out.stdout.is_empty() {
        return Err(format!(
            "--version exited {:?} (stdout {}B)",
            out.code,
            out.stdout.len()
        ));
    }
    // Arm 2 — command-set currency: a binary predating the current command
    // set fabricates golden/faces drift.
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
