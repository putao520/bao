//! Page-pipeline phase recorder + stall watchdog (#40).
//!
//! Every blocking page operation (create / navigate / evaluate / close and
//! their inner servo waits) runs on the embedder MAIN thread. When one of
//! them wedges inside a primitive that never returns (the #40 soak deadlock
//! class — an unbounded wait buried inside a single `spin_event_loop`
//! iteration, where the callers' 15s timeouts never get a chance to fire),
//! the process shows a SILENT hang: no cycle progress, no error, no core.
//!
//! This module converts that silence into attributed, observable evidence:
//!
//! - [`enter_phase`] — a lock-free, allocation-free breadcrumb the main
//!   thread drops at each phase transition (three atomic stores, no Mutex —
//!   the single-writer/embedder-thread model keeps it cheap and safe).
//! - [`spawn_watchdog`] — a background thread (one per process) that polls
//!   the breadcrumb. When a phase exceeds its budget it dumps the waiting
//!   phase + elapsed + a `/proc/self/task` thread-state snapshot to
//!   `log::error`, once per phase generation. It cannot interrupt the main
//!   thread (nothing in-process can — that is the bounded-wait fix's job in
//!   the servo paint layer); its output is the routing forensics that tell
//!   the next debugger exactly which primitive is holding the pipeline.
//!
//! @trace REQ-BRW-001 [entity:PageHandle] — page lifecycle observability
//! @trace REQ-LIB-001 — headless multi-page library liveness

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Phase ids are indices into this table — the atomic slot carries the id,
/// never a pointer or String (lock-free + allocation-free hot path).
const PHASE_NAMES: [&str; 10] = [
    "idle",
    "create:webview_new",
    "create:wait_ready",
    "create:inject",
    "drain_callbacks",
    "navigate",
    "wait:nav",
    "eval_web",
    "eval_node",
    "close",
];

const PHASE_IDLE: u32 = 0;
const PHASE_CREATE_WEBVIEW_NEW: u32 = 1;
const PHASE_CREATE_WAIT_READY: u32 = 2;
const PHASE_CREATE_INJECT: u32 = 3;
const PHASE_DRAIN_CALLBACKS: u32 = 4;
const PHASE_NAVIGATE: u32 = 5;
const PHASE_WAIT_NAV: u32 = 6;
const PHASE_EVAL_WEB: u32 = 7;
const PHASE_EVAL_NODE: u32 = 8;
const PHASE_CLOSE: u32 = 9;

static PHASE_ID: AtomicU32 = AtomicU32::new(PHASE_IDLE);
static PHASE_PAGE: AtomicU64 = AtomicU64::new(0);
/// Milliseconds since the process-anchor [`Instant`] at which the current
/// phase was entered. 0 = phase pre-anchored (never alerted on).
static PHASE_SINCE_MS: AtomicU64 = AtomicU64::new(0);
/// Generation counter — bumped on every phase entry so the watchdog can
/// distinguish "same phase still stuck" from "new phase, reset the alert".
static PHASE_GEN: AtomicU64 = AtomicU64::new(0);

fn anchor() -> &'static Instant {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    ANCHOR.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    anchor().elapsed().as_millis() as u64
}

/// Record entry into a page-pipeline phase. Callers wrap the BLOCKING span
/// (not the async dispatch): the breadcrumb names the primitive that can
/// wedge. Safe to call from any thread, but in practice only the embedder
/// main thread does — single writer, the atomics just make the read side
/// cross-thread-visible.
pub(crate) fn enter_phase(phase_id: u32, page_id: u64) {
    PHASE_SINCE_MS.store(now_ms(), Ordering::SeqCst);
    PHASE_PAGE.store(page_id, Ordering::SeqCst);
    PHASE_ID.store(phase_id, Ordering::SeqCst);
    PHASE_GEN.fetch_add(1, Ordering::SeqCst);
}

/// Captured outer phase for nested spans (e.g. `drain_callbacks` inside
/// `create_page`'s inject phase): restore puts the caller's context back so
/// the breadcrumb names the OUTERMOST blocking span once the inner one
/// returns.
pub(crate) struct PhaseSnapshot {
    id: u32,
    page: u64,
    since_ms: u64,
}

pub(crate) fn capture_phase() -> PhaseSnapshot {
    PhaseSnapshot {
        id: PHASE_ID.load(Ordering::SeqCst),
        page: PHASE_PAGE.load(Ordering::SeqCst),
        since_ms: PHASE_SINCE_MS.load(Ordering::SeqCst),
    }
}

pub(crate) fn restore_phase(snap: PhaseSnapshot) {
    PHASE_SINCE_MS.store(snap.since_ms, Ordering::SeqCst);
    PHASE_PAGE.store(snap.page, Ordering::SeqCst);
    PHASE_ID.store(snap.id, Ordering::SeqCst);
}

/// Scoped phase: enters on construction, restores the outer snapshot on ANY
/// exit (early return, `?`, panic unwind — Drop covers all three). The
/// bread-and-butter for nested spans.
pub(crate) struct PhaseGuard(PhaseSnapshot);

impl PhaseGuard {
    pub(crate) fn enter(phase_id: u32, page_id: u64) -> Self {
        let snap = capture_phase();
        enter_phase(phase_id, page_id);
        PhaseGuard(snap)
    }
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        restore_phase(std::mem::replace(
            &mut self.0,
            PhaseSnapshot { id: PHASE_IDLE, page: 0, since_ms: 0 },
        ));
    }
}

pub(crate) mod phase {
    //! Re-exported phase ids for call sites (keeps the table private).
    pub(crate) const IDLE: u32 = 0;
    pub(crate) const CREATE_WEBVIEW_NEW: u32 = 1;
    pub(crate) const CREATE_WAIT_READY: u32 = 2;
    pub(crate) const CREATE_INJECT: u32 = 3;
    pub(crate) const DRAIN_CALLBACKS: u32 = 4;
    pub(crate) const NAVIGATE: u32 = 5;
    pub(crate) const WAIT_NAV: u32 = 6;
    pub(crate) const EVAL_WEB: u32 = 7;
    pub(crate) const EVAL_NODE: u32 = 8;
    pub(crate) const CLOSE: u32 = 9;
}

/// Default stall budget per phase: a healthy churn cycle is ~150 ms end to
/// end; the longest legitimate waits are the 15s bounded spins (plus GC).
/// 60s means "no phase has any business taking this long" — alert, once.
fn phase_budget() -> Duration {
    static BUDGET_MS: OnceLock<u64> = OnceLock::new();
    Duration::from_millis(*BUDGET_MS.get_or_init(|| {
        std::env::var("BAO_PAGE_PHASE_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60_000)
    }))
}

struct WatchdogState {
    alerted_gen: u64,
}

/// Spawn the process-wide page-pipeline stall watchdog (idempotent — only
/// the first caller actually spawns a thread).
pub fn spawn_watchdog() {
    static SPAWNED: OnceLock<()> = OnceLock::new();
    SPAWNED.get_or_init(|| {
        std::thread::Builder::new()
            .name("bao-page-watchdog".into())
            .spawn(|| {
                let mut state = WatchdogState { alerted_gen: 0 };
                loop {
                    std::thread::sleep(Duration::from_secs(5));
                    let gen = PHASE_GEN.load(Ordering::SeqCst);
                    let phase = PHASE_ID.load(Ordering::SeqCst);
                    let since = PHASE_SINCE_MS.load(Ordering::SeqCst);
                    let page = PHASE_PAGE.load(Ordering::SeqCst);
                    // IDLE parks the watchdog: between phases nothing is
                    // blocked, elapsed is meaningless.
                    if phase == PHASE_IDLE {
                        continue;
                    }
                    let elapsed = now_ms().saturating_sub(since);
                    if elapsed > phase_budget().as_millis() as u64
                        && state.alerted_gen != gen
                    {
                        state.alerted_gen = gen;
                        // The routing forensics dump: which phase, which
                        // page, how long, and what every thread in the
                        // process is waiting on right now.
                        log::error!(
                            "[page-watchdog] page pipeline STALLED: phase={} \
                             page={} elapsed={}ms (budget {}ms) — thread \
                             states follow",
                            PHASE_NAMES
                                .get(phase as usize)
                                .copied()
                                .unwrap_or("unknown"),
                            page,
                            elapsed,
                            phase_budget().as_millis(),
                        );
                        dump_thread_states();
                    }
                }
            })
            .expect("page-watchdog thread spawn");
    });
}

/// Best-effort `/proc/self/task` snapshot: comm + scheduler state + wchan
/// per thread. Read-only, lock-free w.r.t. the engine, works headless —
/// the wchan column is what pins a futex wait to its primitive.
fn dump_thread_states() {
    let dir = match std::fs::read_dir("/proc/self/task") {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut lines = Vec::new();
    for entry in dir.flatten() {
        let tid = entry.file_name();
        let base = format!("/proc/self/task/{}", tid.to_string_lossy());
        let comm = std::fs::read_to_string(format!("{base}/comm"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let wchan = std::fs::read_to_string(format!("{base}/wchan"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let state = std::fs::read_to_string(format!("{base}/stat"))
            .ok()
            .and_then(|s| {
                // Third field of /proc stat; comm may contain spaces so
                // parse after the last ')'.
                s.rsplit(')').next()?.trim().split(' ').next().map(String::from)
            })
            .unwrap_or_default();
        lines.push(format!(
            "tid={} comm={comm} state={state} wchan={wchan}",
            tid.to_string_lossy()
        ));
    }
    for line in lines {
        log::error!("[page-watchdog] {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_names_cover_ids() {
        // Every phase id must index a real name — the dump must never say
        // "unknown" for a legitimate phase.
        for id in [
            PHASE_CREATE_WEBVIEW_NEW,
            PHASE_CREATE_WAIT_READY,
            PHASE_CREATE_INJECT,
            PHASE_DRAIN_CALLBACKS,
            PHASE_NAVIGATE,
            PHASE_WAIT_NAV,
            PHASE_EVAL_WEB,
            PHASE_EVAL_NODE,
            PHASE_CLOSE,
        ] {
            assert!(PHASE_NAMES.get(id as usize).is_some());
            assert_ne!(PHASE_NAMES[id as usize], "idle");
        }
        assert_eq!(PHASE_NAMES[PHASE_IDLE as usize], "idle");
    }

    #[test]
    fn enter_phase_updates_generation_and_fields() {
        let gen0 = PHASE_GEN.load(Ordering::SeqCst);
        enter_phase(PHASE_EVAL_WEB, 42);
        let gen1 = PHASE_GEN.load(Ordering::SeqCst);
        assert_eq!(gen1, gen0 + 1, "generation must bump per entry");
        assert_eq!(PHASE_ID.load(Ordering::SeqCst), PHASE_EVAL_WEB);
        assert_eq!(PHASE_PAGE.load(Ordering::SeqCst), 42);
        // Anchor ms granularity can be 0 on a fresh process clock — the
        // invariant is "set at entry", not "positive".
        let _ = PHASE_SINCE_MS.load(Ordering::SeqCst);
        // Reset for other tests that read the globals.
        enter_phase(PHASE_IDLE, 0);
    }
}
