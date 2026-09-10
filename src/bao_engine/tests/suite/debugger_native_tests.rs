// @trace TEST-ENG-001-DBG [req:REQ-ENG-001] [level:integration]
//
// SM-EVOLUTION #27 — engine-native Debugger JS API reachability proof.
//
// The 2026-09-10 #27 binding census found `JS_DefineDebuggerObject` fully
// wrapped (`jsapi2_wrappers.in.rs:380`, same call shape servo's own
// `dom/debugger/debuggerglobalscope.rs:136` uses). This locks down, with live
// engine evidence, that the complete JS-level SpiderMonkey Debugger surface is
// natively reachable from the existing binding with ZERO mozjs work:
//
//   1. `JS_DefineDebuggerObject` on a fresh realm global installs the
//      `Debugger` constructor (typeof === 'function').
//   2. The full hook surface exists on `Debugger.prototype` — onNewScript,
//      onDebuggerStatement, onEnterFrame, onExceptionUnwind, onPromiseSettled.
//   3. A bare `new Debugger()` is constructible with its method face intact
//      (findScripts/addDebuggee) — no debuggee is added, so no realm debugger
//      instrumentation is toggled (BCE-20260621-002 class stays untouched).
//
// This is the foundation fact for the #11/#27 verdict: Bao's CDP Debugger
// domain rides the JS-level SM Debugger API natively; the missing piece for a
// typed Rust adapter is the C++ `JS::Debugger` class API (Debug.h), which is
// NOT bindgen-reachable — see the ledger #27 section for the full verdict.
//
// Engine-fact proof only: zero product semantics, zero vendor changes, no
// binding regeneration (the wrappers are already compiled into the vendored
// mozjs crate).

use bao_engine::context::JsContext;
use mozjs::jsapi::OnNewGlobalHookOption;
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineDebuggerObject, JS_NewGlobalObject};
use mozjs::rust::{SIMPLE_GLOBAL_CLASS, evaluate_script};
use mozjs::rust::CompileOptionsWrapper;

/// Evaluate `source` in a fresh realm whose global had the native Debugger
/// object defined via `JS_DefineDebuggerObject`, returning the script result
/// as a bool (every script used here ends in a `===` comparison).
fn eval_bool_in_debugger_realm(
    cx: &mut mozjs::context::JSContext,
    source: &str,
    filename: &str,
) -> bool {
    let options = bun_sm::node_realm_options();

    rooted!(&in(cx) let global = unsafe {
        JS_NewGlobalObject(
            cx,
            &SIMPLE_GLOBAL_CLASS,
            std::ptr::null_mut(),
            OnNewGlobalHookOption::FireOnNewGlobalHook,
            &*options,
        )
    });
    assert!(
        !global.get().is_null(),
        "debugger-realm global creation must succeed"
    );

    let c_filename = std::ffi::CString::new(filename)
        .unwrap_or_else(|_| std::ffi::CString::new("<debugger-native>").unwrap());
    let compile_opts = CompileOptionsWrapper::new(cx, c_filename, 1);
    rooted!(&in(cx) let mut rval = UndefinedValue());

    {
        let mut realm = AutoRealm::new_from_handle(cx, global.handle());
        // The native install call under proof: defines `Debugger` on this
        // global. Must run with cx entered in the target realm (same shape as
        // servo's debuggerglobalscope).
        let defined = unsafe { JS_DefineDebuggerObject(&mut realm, global.handle()) };
        assert!(
            defined,
            "JS_DefineDebuggerObject must succeed on the fresh realm global"
        );

        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        let result = evaluate_script(
            realm_cx,
            global.handle(),
            source,
            rval.handle_mut(),
            compile_opts,
        );
        assert!(
            result.is_ok(),
            "evaluating in debugger realm must not error: {:?}",
            result.err()
        );
    }

    let jsval = unsafe { bun_sm::value::jsval_to_jsvalue(cx.raw_cx_no_gc(), rval.get()) };
    jsval
        .as_bool()
        .expect("script result must be a boolean === comparison")
}

#[test]
fn test_debugger_object_native_install() {
    let ctx = JsContext::for_test().expect("Failed to create JsContext");
    let mut cx = ctx.cx();

    // 1. The constructor is installed natively by the FFI call.
    assert!(
        eval_bool_in_debugger_realm(
            &mut cx,
            r#"typeof Debugger === "function""#,
            "dbg_typeof.js",
        ),
        "JS_DefineDebuggerObject must install a callable Debugger constructor"
    );

    // 2. The full hook surface the CDP Debugger domain relies on exists.
    assert!(
        eval_bool_in_debugger_realm(
            &mut cx,
            r#"["onNewScript", "onDebuggerStatement", "onEnterFrame",
                "onExceptionUnwind", "onPromiseSettled"]
               .every(function(n) { return n in Debugger.prototype; })"#,
            "dbg_hooks.js",
        ),
        "Debugger.prototype must expose the onNewScript/onDebuggerStatement/\
         onEnterFrame/onExceptionUnwind/onPromiseSettled hooks"
    );

    // 3. A bare Debugger instance is constructible with its method face. No
    //    addDebuggee call — this must NOT toggle realm debugger
    //    instrumentation (the BCE-20260621-002 crash class).
    assert!(
        eval_bool_in_debugger_realm(
            &mut cx,
            r#"(function() {
                   var d = new Debugger();
                   return typeof d.findScripts === "function"
                       && typeof d.addDebuggee === "function"
                       && typeof d.removeAllDebuggees === "function";
               })()"#,
            "dbg_instance.js",
        ),
        "new Debugger() must be constructible with findScripts/addDebuggee/\
         removeAllDebuggees methods"
    );

    bao_engine::context::JsContext::shutdown_thread_sm();
}
