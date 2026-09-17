// B1 first-writer-wins contract tests for the two embedder bridges
// (`BUN-EVOLUTION` B1 Runtime-thread ownership, rows R51/R52):
//   - R51 `timers::BAO_SETTINGS_RUNNER` (settings-stack runner)
//   - R52 `fetch_async::THREAD_WAKEUP_BRIDGE` (thread-wakeup lookup)
// Both are process-global `OnceLock<fn>` registries whose setters enforce:
// a re-registration with the SAME fn pointer is an idempotent no-op (the
// documented multi-runtime shape — every BaoRuntime registers the same
// zero-capture forwarder), while a re-registration with a DIFFERENT fn
// pointer is real semantic drift and fails closed under `debug_assertions`.
//
// nextest runs each test in its own process, so every test starts with an
// empty OnceLock. Each test still establishes its own baseline first, so the
// file is also order-safe under plain `cargo test --test-threads=1`.

use std::sync::atomic::{AtomicUsize, Ordering};

use mozjs::jsapi::{JSContext, JSObject};

// Distinct side effects per fn so identical-code folding can never merge a
// pair — fn-pointer inequality is load-bearing for the divergence tests.
static R51_TOUCH_A: AtomicUsize = AtomicUsize::new(0);
static R51_TOUCH_B: AtomicUsize = AtomicUsize::new(0);
static R52_TOUCH_A: AtomicUsize = AtomicUsize::new(0);
static R52_TOUCH_B: AtomicUsize = AtomicUsize::new(0);

fn settings_runner_a(_cx: *mut JSContext, _global: *mut JSObject, f: &mut dyn FnMut()) {
    R51_TOUCH_A.fetch_add(1, Ordering::Relaxed);
    f();
}

fn settings_runner_b(_cx: *mut JSContext, _global: *mut JSObject, f: &mut dyn FnMut()) {
    R51_TOUCH_B.fetch_add(1, Ordering::Relaxed);
    f();
}

fn wake_lookup_a() -> Option<bun_runtime::fetch_async::ThreadWakeup> {
    R52_TOUCH_A.fetch_add(1, Ordering::Relaxed);
    None
}

fn wake_lookup_b() -> Option<bun_runtime::fetch_async::ThreadWakeup> {
    R52_TOUCH_B.fetch_add(1, Ordering::Relaxed);
    None
}

// ── R51: timers::register_bao_settings_runner ─────────────────────────────

#[test]
fn r51_settings_runner_same_pointer_reregistration_is_idempotent() {
    bun_runtime::timers::register_bao_settings_runner(settings_runner_a);
    // The documented multi-runtime shape: N identical registrations of the
    // same zero-capture forwarder — idempotent no-ops, never a panic.
    bun_runtime::timers::register_bao_settings_runner(settings_runner_a);
    bun_runtime::timers::register_bao_settings_runner(settings_runner_a);
    // Registration is a pure registry write: it never invokes the runner.
    assert_eq!(R51_TOUCH_A.load(Ordering::Relaxed), 0);
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "divergence enforcement is debug_assertions-only by contract"
)]
#[should_panic(expected = "BAO_SETTINGS_RUNNER re-registration diverged")]
fn r51_settings_runner_divergent_pointer_fails_closed_in_debug() {
    bun_runtime::timers::register_bao_settings_runner(settings_runner_a);
    // A second, structurally different fn pointer = real semantic drift:
    // fail-closed in debug builds instead of being silently discarded.
    bun_runtime::timers::register_bao_settings_runner(settings_runner_b);
}

// ── R52: fetch_async::set_thread_wakeup_bridge ────────────────────────────

#[test]
fn r52_wakeup_bridge_same_pointer_reregistration_is_idempotent() {
    bun_runtime::fetch_async::set_thread_wakeup_bridge(wake_lookup_a);
    bun_runtime::fetch_async::set_thread_wakeup_bridge(wake_lookup_a);
    bun_runtime::fetch_async::set_thread_wakeup_bridge(wake_lookup_a);
    // Registration is a pure registry write: it never invokes the lookup.
    assert_eq!(R52_TOUCH_A.load(Ordering::Relaxed), 0);
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "divergence enforcement is debug_assertions-only by contract"
)]
#[should_panic(expected = "THREAD_WAKEUP_BRIDGE re-registration diverged")]
fn r52_wakeup_bridge_divergent_pointer_fails_closed_in_debug() {
    bun_runtime::fetch_async::set_thread_wakeup_bridge(wake_lookup_a);
    bun_runtime::fetch_async::set_thread_wakeup_bridge(wake_lookup_b);
}
