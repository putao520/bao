// First-writer-wins contract tests for the job-queue embedder hooks
// (B1 same-pattern slice of `UNCAUGHT_HOOK` / `FLUSH_HOOK`, mirrors the
// bun_runtime bridge_contract_tests shape):
//   - `job_queue::UNCAUGHT_HOOK` (uncaught-exception router)
//   - `job_queue::FLUSH_HOOK` (pending-rejection flusher)
// Both are process-global `OnceLock<unsafe fn>` registries whose setter
// `set_uncaught_hooks` enforces: a re-registration with the SAME fn pointers
// is an idempotent no-op (the documented multi-context shape — every
// bao_runtime context installs the same zero-capture router), while a
// re-registration with a DIFFERENT fn pointer is real semantic drift and
// fails closed under `debug_assertions`.
//
// nextest runs each test in its own process, so every test starts with an
// empty OnceLock. Each test still establishes its own baseline first, so the
// file is also order-safe under plain `cargo test --test-threads=1`.

use std::sync::atomic::{AtomicUsize, Ordering};

use mozjs::jsapi::JSContext;
use mozjs::jsval::JSVal;

use bao_engine::job_queue::set_uncaught_hooks;

// Distinct side effects per fn so identical-code folding can never merge a
// pair — fn-pointer inequality is load-bearing for the divergence tests.
static UNCAUGHT_TOUCH_A: AtomicUsize = AtomicUsize::new(0);
static UNCAUGHT_TOUCH_B: AtomicUsize = AtomicUsize::new(0);
static FLUSH_TOUCH_A: AtomicUsize = AtomicUsize::new(0);
static FLUSH_TOUCH_B: AtomicUsize = AtomicUsize::new(0);

unsafe fn uncaught_router_a(_cx: *mut JSContext, _reason: JSVal) {
    UNCAUGHT_TOUCH_A.fetch_add(1, Ordering::Relaxed);
}

unsafe fn uncaught_router_b(_cx: *mut JSContext, _reason: JSVal) {
    UNCAUGHT_TOUCH_B.fetch_add(1, Ordering::Relaxed);
}

unsafe fn flusher_a(_cx: *mut JSContext) {
    FLUSH_TOUCH_A.fetch_add(1, Ordering::Relaxed);
}

unsafe fn flusher_b(_cx: *mut JSContext) {
    FLUSH_TOUCH_B.fetch_add(1, Ordering::Relaxed);
}

// ── idempotent same-pointer re-registration ───────────────────────────────

#[test]
fn hook_contract_uncaught_hook_same_pointer_reregistration_is_idempotent() {
    // The documented multi-context shape: N identical registrations of the
    // same zero-capture router pair — idempotent no-ops, never a panic.
    set_uncaught_hooks(uncaught_router_a, flusher_a);
    set_uncaught_hooks(uncaught_router_a, flusher_a);
    set_uncaught_hooks(uncaught_router_a, flusher_a);
    // Registration is a pure registry write: it never invokes either hook.
    assert_eq!(UNCAUGHT_TOUCH_A.load(Ordering::Relaxed), 0);
    assert_eq!(FLUSH_TOUCH_A.load(Ordering::Relaxed), 0);
}

#[test]
fn hook_contract_flush_hook_same_pointer_reregistration_is_idempotent() {
    // Same shape, second hook: re-registering with the same FLUSH fn (and
    // the same already-installed UNCAUGHT fn) stays an idempotent no-op.
    set_uncaught_hooks(uncaught_router_b, flusher_b);
    set_uncaught_hooks(uncaught_router_b, flusher_b);
    assert_eq!(FLUSH_TOUCH_B.load(Ordering::Relaxed), 0);
    assert_eq!(UNCAUGHT_TOUCH_B.load(Ordering::Relaxed), 0);
}

// ── divergent-pointer fail-closed (debug_assertions only) ─────────────────

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "divergence enforcement is debug_assertions-only by contract"
)]
#[should_panic(expected = "UNCAUGHT_HOOK re-registration diverged")]
fn hook_contract_uncaught_hook_divergent_pointer_fails_closed_in_debug() {
    set_uncaught_hooks(uncaught_router_a, flusher_a);
    // A second, structurally different fn pointer = real semantic drift:
    // fail-closed in debug builds instead of being silently discarded.
    // (flusher_a matches the installed FLUSH fn, so only UNCAUGHT diverges.)
    set_uncaught_hooks(uncaught_router_b, flusher_a);
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "divergence enforcement is debug_assertions-only by contract"
)]
#[should_panic(expected = "FLUSH_HOOK re-registration diverged")]
fn hook_contract_flush_hook_divergent_pointer_fails_closed_in_debug() {
    set_uncaught_hooks(uncaught_router_a, flusher_a);
    // (uncaught_router_a matches the installed UNCAUGHT fn, so only FLUSH
    // diverges.)
    set_uncaught_hooks(uncaught_router_a, flusher_b);
}
