//! Shared suite helpers (e157 M2 consolidation of e152 audit §1.2
//! DUP-TEST-HELPERS: the per-file inline copies of the eval family).
//!
//! Include from a sibling test file at `tests/suite/*.rs` (top of file,
//! after the `use` block):
//!
//! ```ignore
//! #[path = "common/mod.rs"]
//! mod common;
//! use common::eval_string;
//! ```
//!
//! The `#[path]` attribute is required in non-root files; from nested
//! directories use `#[path = "../common/mod.rs"]`.
//!
//! Provenance: every body below is a verbatim lift of the dominant inline
//! copy it replaces (masked-variant hash recorded per fn; the per-file
//! diff audit lives in the e157 B1 plan). `*_named` variants expose the
//! only parameter that legitimately differed between copies — the script
//! filename literal fed to `ctx.eval` — so files with unique filenames
//! keep their attribution via one-line delegators.
//!
//! `dead_code` is allowed because each including file instantiates the
//! module privately and uses only the subset it needs (same pattern as
//! conformance_common.rs).

#![allow(dead_code)]

pub mod bin_resolver;
pub mod capture_server;
pub mod port_probe;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

/// Minimal stringifier: String/Number/Bool formatted, everything else `""`.
/// (masked variant 4f3ea37e, 81 verbatim copies + 2 block-bool merges)
pub fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

/// Rich stringifier: explicit null/undefined/object arms, `ERROR:{message}`.
/// (masked variant f3479490, 27 verbatim copies + 1 block-bool merge)
pub fn eval_string_full(ctx: &mut JsContext, source: &str) -> String {
    eval_string_full_named(ctx, source, "<test>")
}

/// Filename-parameterized [`eval_string_full`] for files whose inline copy
/// used a script-specific filename literal.
pub fn eval_string_full_named(ctx: &mut JsContext, source: &str, file: &str) -> String {
    match ctx.eval(source, file) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Rich stringifier with an explicit `[other]` catch-all arm.
/// (masked variant 8f93172e, 6 verbatim copies)
pub fn eval_string_dbg(ctx: &mut JsContext, source: &str) -> String {
    eval_string_dbg_named(ctx, source, "<test>")
}

/// Filename-parameterized [`eval_string_dbg`].
pub fn eval_string_dbg_named(ctx: &mut JsContext, source: &str, file: &str) -> String {
    match ctx.eval(source, file) {
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

/// Debug-fallback stringifier: non-primitives via `{:?}`, errors via
/// `ERROR: {:?}`. (masked variant 1f8576ba, 10 verbatim copies + 1
/// inline-capture merge)
pub fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    eval_str_named(ctx, code, "<test>")
}

/// Filename-parameterized [`eval_str`].
pub fn eval_str_named(ctx: &mut JsContext, code: &str, file: &str) -> String {
    match ctx.eval(code, file) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

/// Integer-preserving stringifier: whole numbers must not print as `1.0`;
/// non-primitives become `""`, errors become `<eval-error>`.
/// (masked variant 0dcf157a, 4 copies, all with unique filenames)
pub fn eval_str_int_named(ctx: &mut JsContext, source: &str, file: &str) -> String {
    match ctx.eval(source, file) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => {
            // integral numbers must not print as "1.0"
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", n as i64)
            } else {
                format!("{}", n)
            }
        }
        Ok(JsValue::Bool(b)) => {
            if b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Ok(_) => String::new(),
        Err(_) => "<eval-error>".to_string(),
    }
}

/// Number extractor: non-Number results (including errors) yield `NaN`.
/// (masked variant 3fe28cb6, 9 verbatim copies)
pub fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

// ---------------------------------------------------------------------------
// B2 cluster: context setup / event-loop driving / bounded-drain wait
// (setup_ctx ×9+3, drive_event_loop ×8+4, bounded_drain_hook ×20; the
// wait_until ×6+3 iteration-bounded spin family was eradicated by the E27
// sweep — superseded by [`wait_until_deadline`] + [`pump_passes`])
// ---------------------------------------------------------------------------

use std::cell::Cell;
use std::time::{Duration, Instant};

use mozjs::rooted;

// Pump budget shared between [`bounded_drain_hook`] and [`wait_until_deadline`]
// (thread_local is per-process under nextest's one-test-per-process model;
// 20 verbatim inline copies consolidated).
thread_local! {
    pub static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Post-eval drain hook bounded by [`HOOK_BUDGET`]: each call first decrements
/// the budget, returning `false` once exhausted, else drains timers.
/// (masked variant 365c3adb, 20 verbatim copies)
pub fn bounded_drain_hook(cx: &mut mozjs::context::JSContext) -> bool {
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

/// Fresh JsContext with exit handler, process start and all globals
/// installed. (masked variant f4648dc3, 9 verbatim copies)
pub fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// [`setup_ctx`] + the bounded drain post-eval hook.
/// (masked variant 579bf8a0, 3 verbatim copies)
pub fn setup_ctx_bounded() -> JsContext {
    let mut ctx = setup_ctx();
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

/// Run microjobs then tick the JS event loop, `max_iters` times.
/// (masked variants 8a80db65 ×5 + b2adef58 ×3 — identical modulo
/// `timers::` path qualification, unified on the fully-qualified form)
pub fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
    let cx_raw = ctx.raw_cx();
    for _ in 0..max_iters {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Realm-aware drain variant: drains timers through the thread realm global
/// when present, then ticks the loop and runs microjobs.
/// (masked variant bf3ef3eb, 4 verbatim copies)
pub fn drive_event_loop_drain(ctx: &mut JsContext, max_iters: usize) {
    let cx_raw = ctx.raw_cx();
    for _ in 0..max_iters {
        {
            let mut cxm = ctx.cx();
            let global = bao_engine::context::thread_realm_global();
            if let Some(g) = global {
                rooted!(&in(cxm) let g_root = g);
                let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
                let realm_cx: &mut mozjs::context::JSContext = &mut realm;
                bun_runtime::timers::drain_and_check(realm_cx);
            } else {
                bun_runtime::timers::drain_and_check(&mut cxm);
            }
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// No-progress tripwire window for [`pump_until`]: an unchanged poll value
/// for this long while the loop is still ticking ⇒ `PumpStuck` panic.
pub const PUMP_STALL: Duration = Duration::from_secs(2);

/// One settle-pump tick: realm-aware timer drain (fires due timers),
/// MiniEventLoop `tick_without_idle` (dispatches queued ConcurrentTasks onto
/// the JS thread), `RunJobs` (settles reactions), then a 2ms yield. The tick
/// body is the `drive_event_loop_drain` idiom verbatim (c17/
/// abort_signal_timeout inline-pump lineage) with the settle sleep kept at
/// 2ms. Shared by [`pump_until`] and [`pump_passes`].
// @trace REQ-ENG-004
fn pump_tick(ctx: &mut JsContext) {
    let cx_raw = ctx.raw_cx();
    {
        let mut cxm = ctx.cx();
        let global = bao_engine::context::thread_realm_global();
        if let Some(g) = global {
            rooted!(&in(cxm) let g_root = g);
            let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            bun_runtime::timers::drain_and_check(realm_cx);
        } else {
            bun_runtime::timers::drain_and_check(&mut cxm);
        }
    }
    bun_runtime::timers::with_event_loop(|loop_| {
        loop_.tick_without_idle(std::ptr::null_mut());
    });
    unsafe {
        mozjs_sys::jsapi::js::RunJobs(cx_raw);
    }
    std::thread::sleep(Duration::from_millis(2));
}

/// Fixed-pass pump: run exactly `passes` [`pump_tick`]s with no settle
/// predicate and no tripwire — for callers that want N ticks of loop
/// service (server accept/dispatch pacing), not a settle wait. The
/// historical form pumped `passes × 5ms` of wall-clock budget through
/// [`pump_until`] with a constant poll and an always-false settle; that
/// shape only worked while the no-progress tripwire was dead code (E17) —
/// with the tripwire revived, a constant poll frozen past [`PUMP_STALL`]
/// panics, so fixed-pass callers must not route through [`pump_until`].
// @trace REQ-ENG-004
pub fn pump_passes(ctx: &mut JsContext, passes: usize) {
    for _ in 0..passes {
        pump_tick(ctx);
    }
}

/// Unified settle-pump for cross-thread async contracts (fetch ConcurrentTask
/// delivery, timers, promise reactions): each tick drives all three
/// actuators (see [`pump_tick`]) then polls `poll_js` through the caller's
/// eval binding.
///
/// Exits early the moment `settled(poll)` is true (the inline-pump "marker
/// left PENDING" semantics); returns the last poll value on `timeout` expiry
/// (caller asserts on it). No-progress tripwire: if the polled value is
/// unchanged for [`PUMP_STALL`] of wall clock SINCE THE FIRST READ while the
/// loop is still ticking and not yet settled, the consumption side is
/// stalled (producer dead / delivery chain broken) — panic `PumpStuck`
/// immediately, so a stalled settle surfaces as a diagnostic FAIL naming the
/// poll expression and the frozen value, not a silent hang. The first poll
/// value is the tripwire's BASELINE, not a change: only a value that
/// differs from a previous poll disarms the zero-progress signature.
///
/// Known limitation (honest boundary): the tripwire only observes *between*
/// ticks. If a single tick call itself parks at the syscall level
/// (mutex/condvar/futex), the loop never returns to the tripwire check and
/// this function cannot fire — that full-park shape needs an outer backstop
/// (nextest slow-timeout / harness watchdog), not this tripwire.
// @trace REQ-ENG-010
pub fn pump_until(
    ctx: &mut JsContext,
    poll_js: &str,
    timeout: Duration,
    mut eval: impl FnMut(&mut JsContext, &str) -> String,
    mut settled: impl FnMut(&str) -> bool,
) -> String {
    let deadline = Instant::now() + timeout;
    // First-read baseline (E17 revival): the previous-value slot starts as
    // `None`. Seeding it with an empty string made the tripwire dead code
    // for every poll whose first value was non-empty ("PENDING", a registry
    // count, ...): the first read compared unequal to "" and permanently
    // cleared `never_changed` — the main diagnostic face of 40+ files could
    // not fire.
    let mut last_poll: Option<String> = None;
    let mut last_change = Instant::now();
    let mut never_changed = true;
    let mut final_poll = String::new();
    while Instant::now() < deadline {
        pump_tick(ctx);
        final_poll = eval(ctx, poll_js);
        if settled(&final_poll) {
            break;
        }
        if last_poll.as_deref() != Some(final_poll.as_str()) {
            // First read (baseline) or a genuine change after it. Only a
            // change AFTER the first read counts as progress — the first
            // read must leave the zero-progress signature armed.
            if last_poll.is_some() {
                never_changed = false;
            }
            last_poll = Some(final_poll.clone());
            last_change = Instant::now();
        } else if never_changed && last_change.elapsed() >= PUMP_STALL {
            // Tripwire fires ONLY on the zero-progress signature (the c17
            // deadlock shape: producer dead, poll frozen from the first read).
            // A pump that made progress and then goes stationary is waiting
            // on a slow event (e.g. GC finalization after settle) — that case
            // belongs to the outer deadline, not this tripwire.
            panic!(
                "PumpStuck: poll value {:?} never changed since first read ({:?} elapsed) \
                 while pumping (poll_js={}) — consumption stalled or producer starved",
                final_poll, PUMP_STALL, poll_js
            );
        }
    }
    final_poll
}

/// Load-robust wall-clock condition wait (E21 shape): poll `js_condition`
/// every 10ms until it answers `"y"` or `secs` elapse.
///
/// Why not an iteration-bounded spin: the deleted `wait_until_with` had a
/// wall window of `iters × eval-latency` (tens of ms — the faster the host,
/// the shorter the window), racing any awaited state produced on ANOTHER
/// thread's schedule (E21 cp-drain forensics: the exit landed +10ms AFTER an
/// 80-iteration spin had already closed its ~1-30ms window — delayed
/// publication, not loss; 5/15 fleet rounds red). A wall-clock window with
/// an inter-probe sleep costs nothing when green (idle arrival ≈ 1-5ms) and
/// turns a scheduler race into an honest bounded wait: red within the
/// window = genuine event loss (a product defect), not a spin-window
/// artifact.
///
/// `budget` passthrough keeps the bounded-ctx timer drain alive
/// ([`HOOK_BUDGET`] feeds `setup_ctx_bounded`'s post-eval hook); it is
/// inert for hookless contexts.
// @trace REQ-ENG-004
pub fn wait_until_deadline(
    ctx: &mut JsContext,
    js_condition: &str,
    budget: usize,
    secs: u64,
    eval: fn(&mut JsContext, &str) -> String,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval(ctx, js_condition) == "y" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}
