// @trace TEST-ENG-001-DISPATCH [req:REQ-ENG-001] [level:integration]
//
// Wave 73-G verification: SpiderMonkey Jsc/Js arm dispatch path.
//
// Validates that `bao_engine::dispatch_sm::BaoEventLoop` correctly backs the
// `Js`/`Jsc` variants of `bun_io::EventLoopCtx` and `bun_event_loop::JsEventLoop`,
// producing valid pointers through the dispatch macros. This is the
// end-to-end dispatch path test — if it passes, the link_interface! /
// link_impl_*! machinery is wired correctly for SpiderMonkey.
//
// NOTE: The underlying uSockets C loop is a stub (`uws_get_loop()` returns
// null until Wave 74-B implements it via mio). Tests in this file therefore
// validate the *dispatch wiring* (variant resolution + lazy init + symbol
// emission) rather than the underlying loop's behavior. Tests that would
// require a live loop are marked with the `WAVE_74_B` cfg gate.

#![allow(clippy::missing_panics_doc)]
#![cfg_attr(not(feature = "live_uws_loop"), allow(unused_imports))]

// Pull in C-library stubs (uSockets uws_get_loop, SSL, etc.) so the test
// binary links. Without this, lazy-init of MiniEventLoop triggers
// `undefined symbol: uws_get_loop` at runtime.
//
// `force_link()` is called from a `#[used]` static initializer's drop glue so
// the linker keeps both the function and the stubs it references.
fn _force_native_stubs_link() {
    bao_native_stubs::force_link();
}

// Force the linker to retain `_force_native_stubs_link`.
#[used]
static NATIVE_STUBS_LINKER_ANCHOR: fn() = _force_native_stubs_link;

use bao_engine::dispatch_sm::BaoEventLoop;

#[test]
fn test_current_returns_static_ref() {
    let a = BaoEventLoop::current() as *const BaoEventLoop;
    let b = BaoEventLoop::current() as *const BaoEventLoop;
    assert_eq!(a, b, "BaoEventLoop::current() must return the same per-thread instance");
}

#[test]
fn test_current_is_thread_local() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    let main_ptr = BaoEventLoop::current() as *const BaoEventLoop as usize;
    let child_ptr = Arc::new(AtomicUsize::new(0));

    let observed = Arc::clone(&child_ptr);
    thread::spawn(move || {
        observed.store(
            BaoEventLoop::current() as *const BaoEventLoop as usize,
            Ordering::SeqCst,
        );
    })
    .join()
    .expect("child thread");

    let child = child_ptr.load(Ordering::SeqCst);
    assert_ne!(
        main_ptr, child,
        "BaoEventLoop must be thread-local — each thread gets its own instance"
    );
}

#[test]
fn test_dispatch_to_uws_loop_through_jseventloop() {
    // Wave 73-E: `JsEventLoop::current().uws_loop()` routes through
    // `link_impl_JsEventLoop! { Jsc for BaoEventLoop }`. Two calls on the
    // same thread must return the same pointer (lazy init is stable).
    //
    // Until Wave 74-B ships a real `uws_get_loop()`, the pointer is the
    // C-stub's null — both calls still agree, which is what we check here.
    let loop_a = bun_event_loop::JsEventLoop::current();
    let ptr_a = loop_a.uws_loop();
    let loop_b = bun_event_loop::JsEventLoop::current();
    let ptr_b = loop_b.uws_loop();
    assert_eq!(
        ptr_a, ptr_b,
        "Same thread → same uws_loop pointer (lazy init stable): ptr_a={:p}, ptr_b={:p}",
        ptr_a, ptr_b
    );
}

#[test]
fn test_enter_exit_depth_balance() {
    // Wave 73-E: `enter()` increments the reentrancy counter, `exit()`
    // decrements. Both route through dispatch but only touch BaoEventLoop's
    // internal Cell<u32> — no C-loop interaction.
    let el = bun_event_loop::JsEventLoop::current();
    el.enter();
    el.enter();
    el.exit();
    el.exit();
    // No panic = success. Counter underflow would panic on the third exit.
}

#[test]
fn test_pipe_read_buffer_non_null() {
    // Wave 73-D/E: `pipe_read_buffer()` is owned by MiniEventLoop (a Rust
    // Box<[u8; 65536]>), not by the C loop. Lazy-init must produce a
    // non-null, stable pointer regardless of uSockets state.
    let el = bun_event_loop::JsEventLoop::current();
    let buf_a = el.pipe_read_buffer();
    assert!(!buf_a.is_null(), "pipe_read_buffer must be non-null");
    let buf_b = el.pipe_read_buffer();
    assert_eq!(
        buf_a, buf_b,
        "pipe_read_buffer must be stable across calls (same MiniEventLoop)"
    );
}

#[test]
fn test_env_initially_null() {
    // Wave 73-E: `env()` returns the env loader pointer. Until bao_runtime
    // registers one, it must be null (not a dangling pointer).
    let el = bun_event_loop::JsEventLoop::current();
    let env = el.env();
    assert!(
        env.is_null(),
        "env must be null until bao_runtime registration (got {:p})",
        env
    );
}

#[test]
fn test_global_object_initially_null() {
    // Wave 73-E: `global_object()` returns SpiderMonkey global pointer.
    // Until bao_runtime JsContext wires up, it must be null.
    let el = bun_event_loop::JsEventLoop::current();
    let g = el.global_object();
    assert!(g.is_null(), "global_object must be null until JsContext registration");
}

#[test]
fn test_bun_vm_initially_null() {
    // Wave 73-E: `bun_vm()` returns SpiderMonkey VM wrapper. Until
    // bao_runtime wires up, it must be null.
    let el = bun_event_loop::JsEventLoop::current();
    let vm = el.bun_vm();
    assert!(vm.is_null(), "bun_vm must be null until JsContext registration");
}

#[test]
fn test_event_loop_ctx_through_dispatch() {
    // Wave 73-D: `EventLoopCtx` can be formed from the BaoEventLoop owner
    // and dispatched through the `Js` arm. The dispatch resolves the variant
    // and returns the platform loop pointer — even if that pointer is null
    // (C stub), the dispatch mechanics must not panic.
    use bun_io::EventLoopCtxKind;
    let owner_ptr = BaoEventLoop::current() as *const BaoEventLoop as *mut ();
    // SAFETY: owner is the live thread-local BaoEventLoop instance.
    let ctx = unsafe { bun_io::EventLoopCtx::new(EventLoopCtxKind::Js, owner_ptr) };
    // Call platform_event_loop_ptr — exercises the dispatch arm. The result
    // may be null until Wave 74-B; we only require dispatch not to crash.
    let _ptr = ctx.platform_event_loop_ptr();
}

#[test]
fn test_js_event_loop_current_symbol_resolves() {
    // Wave 73-E: `__bun_js_event_loop_current` is the extern "Rust" symbol
    // `bun_event_loop::JsEventLoop::current()` calls. It must return a
    // non-null pointer to the thread-local BaoEventLoop.
    unsafe extern "Rust" {
        fn __bun_js_event_loop_current() -> *mut ();
    }
    let p = unsafe { __bun_js_event_loop_current() };
    assert!(!p.is_null(), "__bun_js_event_loop_current must return non-null");
    // The pointer must match BaoEventLoop::current() (same thread).
    let direct = BaoEventLoop::current() as *const BaoEventLoop as *mut ();
    assert_eq!(
        p, direct,
        "__bun_js_event_loop_current must match BaoEventLoop::current()"
    );
}

#[test]
fn test_after_event_loop_callback_roundtrip() {
    // Wave 73-D: `set_after_event_loop_callback` + `after_event_loop_callback`
    // must round-trip through the dispatch arm. Pure Rust fields on
    // MiniEventLoop — no C-loop interaction.
    //
    // The methods are inherent on `EventLoopCtx` via the dispatch macro; the
    // `ctx: Option<NonNull<c_void>>` parameter must round-trip through.
    use bun_io::{EventLoopCtx, EventLoopCtxKind, OpaqueCallback};
    let owner_ptr = BaoEventLoop::current() as *const BaoEventLoop as *mut ();
    // SAFETY: owner is the live thread-local BaoEventLoop instance.
    let ctx = unsafe { EventLoopCtx::new(EventLoopCtxKind::Js, owner_ptr) };

    // Initial state: callback unset.
    assert!(
        ctx.after_event_loop_callback().is_none(),
        "callback must be unset initially"
    );

    // Set a no-op callback + a sentinel context.
    unsafe extern "C" fn noop_cb(_ctx: *mut core::ffi::c_void) {}
    let sentinel_ctx =
        core::ptr::NonNull::new(0xdeadbeef_usize as *mut core::ffi::c_void);
    ctx.set_after_event_loop_callback(Some(noop_cb), sentinel_ctx);

    // Read back: callback must be Some(noop_cb).
    let cb_after = ctx.after_event_loop_callback();
    assert_eq!(
        cb_after,
        Some(noop_cb as OpaqueCallback),
        "round-tripped callback must match the one set"
    );

    // Clear it.
    ctx.set_after_event_loop_callback(None, None);
    assert!(
        ctx.after_event_loop_callback().is_none(),
        "callback must be cleared after set(None)"
    );
}
