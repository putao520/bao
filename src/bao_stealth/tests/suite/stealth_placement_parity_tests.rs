// @trace TEST-STL-007 [req:REQ-STL-007] [level:integration]
//
// e148 Chromium-parity placement lock for the engine stealth getters.
//
// Oracle (probe, Chrome 150 — /tmp/e148-nav-placement-probe, committed in
// the e148 commit message): every covered navigator/screen member is an
// accessor on the INTERFACE PROTOTYPE; the navigator instance carries ZERO
// own names. Placement + enumerability are aligned; configurable stays
// FALSE — PERMANENT is load-bearing (e148-R1 bisect: the configurable
// variant SIGSEGV'd under PagePool churn), a registered Chrome-parity
// residual on that descriptor sub-face.
//
// The pre-e148 own-instance placement had two visible faces:
//   1. Object.getOwnPropertyNames(navigator) listed every covered member
//      (Chrome: []).
//   2. the native servo prototype accessor stayed reachable —
//      getOwnPropertyDescriptor(Navigator.prototype, p).get.call(navigator)
//      returned the TRUE host value (native fingerprint leak); the fix
//      replaces the native accessor IN PLACE, so a page-level delete yields
//      undefined exactly like Chrome.
//
// Single #[test] for JsContext tests (mozjs Runtime is per-process
// singleton — same convention as stealth_diagnostic_detection_tests).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bao_stealth::StealthProfile;
use mozjs::rooted;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<placement>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Bool(b)) => b.to_string(),
        other => format!("{other:?}"),
    }
}

#[test]
fn stealth_getters_chromium_placement_parity() {
    let profile = StealthProfile::firefox_default();
    bao_stealth::engine_props::set_profile(&profile);
    let mut ctx = JsContext::for_test().expect("JsContext");

    // ── Phase A: DOM-shaped navigator (custom interface prototype carrying
    //    a NATIVE-like accessor — the leak vector) ──
    ctx.eval(
        r#"
        var __NavProto = {};
        Object.defineProperty(__NavProto, 'userAgent', {
          get: function () { return 'NATIVE-HOST-LEAK'; },
          configurable: true, enumerable: true
        });
        globalThis.navigator = Object.create(__NavProto);
        "#,
        "<setup>",
    )
    .expect("phase A setup");

    // The install runs from bare Rust (no JS activation): resolve the
    // thread realm's global and enter its realm for the JSAPI calls (same
    // shape as the tls suite's pump helper).
    let global = bao_engine::context::thread_realm_global()
        .filter(|g| !g.is_null())
        .expect("thread realm global must resolve after eval");
    {
        let mut cxm = ctx.cx();
        rooted!(&in(cxm) let g_root = global);
        let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        assert!(unsafe {
            bao_stealth::engine_props::install_stealth_props(realm_cx.raw_cx(), global)
        });
    }

    let report = eval_str(
        &mut ctx,
        &r#"
        var p = Object.getPrototypeOf(navigator);
        var d = Object.getOwnPropertyDescriptor(p, 'userAgent');
        var out = [];
        out.push('protoHasGetter=' + (d && typeof d.get === 'function'));
        // e148-R1: configurable stays FALSE (PERMANENT is load-bearing — the
        // configurable variant SIGSEGV'd under PagePool churn; see
        // define_stealth_getter's bisect note). Registered Chrome-parity
        // residual on this descriptor sub-face.
        out.push('confFalse=' + (d && d.configurable === false));
        out.push('enum=' + (d && d.enumerable === true));
        out.push('ownAbsent=' + (Object.getOwnPropertyDescriptor(navigator, 'userAgent') === undefined));
        // The 15 getter-covered members must not appear as own names (other
        // spoof segments — plugins/mimeTypes/getBattery — own-shape by their
        // own design, out of this placement face).
        var covered = ['userAgent','platform','language','languages','vendor',
          'webdriver','deviceMemory','hardwareConcurrency','maxTouchPoints',
          'width','height','availWidth','availHeight','colorDepth','pixelDepth'];
        var own = Object.getOwnPropertyNames(navigator);
        out.push('coveredNotOwn=' + (covered.every(function (n) { return own.indexOf(n) === -1; })));
        out.push('valueIsProfile=' + (navigator.userAgent === __EXPECT_UA__));
        out.push('nativeReplaced=' + (d.get.call(navigator) === navigator.userAgent));
        // PERMANENT (e148-R1): a page-level delete is refused (Chrome would
        // allow it and yield undefined — registered residual). The
        // anti-leak property still holds: the surviving accessor is OURS,
        // never the native host value.
        var del = delete p.userAgent;
        out.push('deleteRefused=' + (del === false));
        out.push('valueSurvivesDelete=' + (navigator.userAgent === __EXPECT_UA__));
        out.join('|');
        "#
        .replace("__EXPECT_UA__", &format!("{:?}", profile.navigator.user_agent)),
    );
    for field in [
        "protoHasGetter=true",
        "confFalse=true",
        "enum=true",
        "ownAbsent=true",
        "coveredNotOwn=true",
        "valueIsProfile=true",
        "nativeReplaced=true",
        "deleteRefused=true",
        "valueSurvivesDelete=true",
    ] {
        assert!(
            report.contains(&format!("{field}|")) || report.ends_with(field),
            "Phase A (DOM-shaped navigator): {field} missing from report: {report}"
        );
    }

    // ── Phase B: plain-object navigator (test-mode fallback — proto IS
    //    Object.prototype): the define must land on the INSTANCE and must
    //    never pollute Object.prototype. ──
    ctx.eval("globalThis.navigator = {};", "<setup-b>").expect("phase B setup");
    {
        let mut cxm = ctx.cx();
        rooted!(&in(cxm) let g_root2 = global);
        let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root2.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        // Return value NOT asserted: the global's devicePixelRatio slot is
        // already PERMANENT from phase A — the re-install's dpr define is
        // refused (prior-install arm, expected) and install_stealth_props
        // reports false while the fresh plain navigator still got its
        // defines (asserted behaviorally below).
        let _ = unsafe {
            bao_stealth::engine_props::install_stealth_props(realm_cx.raw_cx(), global)
        };
    }

    let report_b = eval_str(
        &mut ctx,
        r#"
        var out = [];
        out.push('ownGetter=' + (typeof Object.getOwnPropertyDescriptor(navigator, 'userAgent').get === 'function'));
        out.push('valueOk=' + (navigator.userAgent.length > 0));
        out.push('objProtoClean=' + (Object.getOwnPropertyDescriptor(Object.prototype, 'userAgent') === undefined));
        out.push('plainObjsClean=' + ({}.userAgent === undefined));
        out.join('|');
        "#,
    );
    for field in [
        "ownGetter=true",
        "valueOk=true",
        "objProtoClean=true",
        "plainObjsClean=true",
    ] {
        assert!(
            report_b.contains(&format!("{field}|")) || report_b.ends_with(field),
            "Phase B (plain-object fallback): {field} missing from report: {report_b}"
        );
    }

    bao_engine::context::JsContext::shutdown_thread_sm();
}
