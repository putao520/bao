// @trace REQ-ENG-001 [entity:RawValueRootGuard]
/// RawValueRootGuard unit tests — RAII heap rooting for async windows.
///
/// Covers the four contract points of the guard:
/// 1. root/drop round-trip: the rooted value survives a full GC held only
///    by the guard, and dropping the guard unroots (later GCs never touch
///    freed slot memory — crash-freedom).
/// 2. `get` returns the LIVE (GC-updated) slot, not a spawn-time snapshot.
/// 3. `into_inner` releases the roots and hands ownership back (and refuses
///    — leaking instead — when it cannot unroot).
/// 4. The two leak-instead-of-UAF drop paths: foreign thread (root table
///    still alive) and dead runtime — both must drop without crashing and
///    without leaving the GC a dangling scan address.
///
/// Why this guard exists (BCE-class eradication): the three former call
/// sites (fetch_async / node_fs / node_crypto) registered a STACK slot with
/// `AddRawValueRoot`, returned from the frame, and later "removed" the root
/// with a different stack address — a silent no-op (`rootsHash.remove` is
/// keyed by pointer). The GC kept scanning dead stack memory forever. The
/// guard pins the slots in a `Box` so the registered address stays valid and
/// identical until removal.
///
/// Harness note: objects are created via `eval` (not `JS_NewPlainObject` on
/// the bare context) because eval establishes the persistent realm — SM
/// APIs deriving from the current realm NULL-deref outside any realm
/// activation (BCE-BUG-ENG-370).

use bao_engine::context::{JsContext, RawValueRootGuard, thread_realm_global};
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;

/// Plant `{ marker: N }` on the realm global via eval and return it as a
/// raw JSVal (the caller roots it immediately; no GC can run in between).
fn make_marker_object(ctx: &mut JsContext, marker: i32) -> JSVal {
    ctx.eval(
        &format!("globalThis.__rvrg_obj = {{ marker: {marker} }};"),
        "<guard-test>",
    )
    .expect("setup eval must succeed");
    let mut cx = ctx.cx();
    let raw = unsafe { cx.raw_cx_no_gc() };
    let global = thread_realm_global().expect("realm global after eval");
    rooted!(&in(cx) let g = global);
    let mut realm = AutoRealm::new_from_handle(&mut cx, g.handle());
    rooted!(&in(realm) let mut out = UndefinedValue());
    let got = unsafe {
        JS_GetProperty(
            raw,
            g.handle().into(),
            c"__rvrg_obj".as_ptr(),
            out.handle_mut().into(),
        )
    };
    assert!(got, "JS_GetProperty(__rvrg_obj) must succeed");
    assert!(out.get().is_object(), "planted value must be an object");
    out.get()
}

/// Read `marker` back off a rooted object value (inside its realm).
fn read_marker(cx: &mut mozjs::context::JSContext, val: JSVal) -> Option<i32> {
    let raw = unsafe { cx.raw_cx_no_gc() };
    rooted!(&in(cx) let obj = val.to_object());
    let mut realm = AutoRealm::new_from_handle(cx, obj.handle());
    rooted!(&in(realm) let mut out = UndefinedValue());
    let got = unsafe {
        JS_GetProperty(
            raw,
            obj.handle().into(),
            c"marker".as_ptr(),
            out.handle_mut().into(),
        )
    };
    if got && out.get().is_int32() {
        Some(out.get().to_int32())
    } else {
        None
    }
}

/// Root/drop round-trip: the value survives a GC held only by the guard;
/// after drop, further GCs must not crash (root removed with the same
/// registered address).
#[test]
fn guard_roots_survive_gc_and_drop_unroots() {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    let raw = ctx.raw_cx();

    let val = make_marker_object(&mut ctx, 7);
    let guard = unsafe {
        RawValueRootGuard::new(raw, ::std::slice::from_ref(&val), c"guard_test_obj")
    }
    .expect("RawValueRootGuard::new must succeed");
    assert_eq!(guard.len(), 1);

    // Full GC with the guard as the only extra root: the live slot must
    // still resolve the object and its property.
    unsafe { JS_GC(raw, JS::GCReason::API) };
    let live = guard.get(0);
    assert!(live.is_object(), "guarded value must survive GC");
    let mut cx = ctx.cx();
    assert_eq!(read_marker(&mut cx, live), Some(7));

    drop(guard);
    // Root removed — subsequent GCs must never touch the freed slot.
    unsafe { JS_GC(raw, JS::GCReason::API) };
}

/// `into_inner` releases the roots and returns ownership of the values;
/// after the transfer, GCs must not crash (roots were removed).
#[test]
fn into_inner_unroots_and_transfers_values() {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    let raw = ctx.raw_cx();

    let v1 = make_marker_object(&mut ctx, 11);
    let v2 = make_marker_object(&mut ctx, 22);
    let guard = unsafe { RawValueRootGuard::new(raw, &[v1, v2], c"guard_test_pair") }
        .expect("RawValueRootGuard::new must succeed");
    assert_eq!(guard.len(), 2);

    let vals = guard
        .into_inner()
        .expect("into_inner on live cx must unroot");
    assert_eq!(vals.len(), 2);
    assert!(vals[0].is_object() && vals[1].is_object());

    // Roots are gone: GC after the ownership transfer must not crash and
    // must not update the (now unowned) memory.
    unsafe { JS_GC(raw, JS::GCReason::API) };
}

/// Dropping the guard from a foreign thread (root table still alive) must
/// leak the rooted memory instead of freeing it — the GC may still scan the
/// slots, so a later GC on the owning thread must not crash.
#[test]
fn drop_on_foreign_thread_leaks_without_dangling() {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    let raw = ctx.raw_cx();

    let val = make_marker_object(&mut ctx, 33);
    let guard = unsafe {
        RawValueRootGuard::new(raw, ::std::slice::from_ref(&val), c"guard_foreign_drop")
    }
    .expect("RawValueRootGuard::new must succeed");

    // Smuggle through a usize (same pattern as FsAsyncCtx's ctx_ptr): the
    // guard is !Send by design (it owns GC-visible memory); the owning
    // PendingFetch-style structs carry their own `unsafe impl Send` and drop
    // on the JS thread. This test exercises the mis-drop case the guard
    // must survive.
    let ptr = ::std::boxed::Box::into_raw(::std::boxed::Box::new(guard)) as usize;
    let handle = ::std::thread::spawn(move || unsafe {
        drop(::std::boxed::Box::from_raw(ptr as *mut RawValueRootGuard));
    });
    handle.join().expect("foreign-thread drop must not panic");

    // The rooted slot was leaked (still registered, still mapped): a full
    // GC on the owning thread must not crash scanning it.
    unsafe { JS_GC(raw, JS::GCReason::API) };
}

/// Dropping the guard after the runtime was destroyed must not crash
/// (liveness guard skips the unroot; the root table died with the runtime).
#[test]
fn drop_after_runtime_teardown_does_not_crash() {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    let raw = ctx.raw_cx();

    let val = make_marker_object(&mut ctx, 44);
    let guard = unsafe {
        RawValueRootGuard::new(raw, ::std::slice::from_ref(&val), c"guard_teardown")
    }
    .expect("RawValueRootGuard::new must succeed");

    drop(ctx);
    JsContext::shutdown_thread_sm();
    // Liveness-guarded Drop: Runtime::get() no longer resolves to `raw`, so
    // the guard must skip the unroot (and leak) instead of touching the
    // destroyed runtime's root table.
    drop(guard);
}
