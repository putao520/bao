// @trace TEST-ENG-005-LOCKED-STATE [req:REQ-ENG-005] [level:integration]
//
// absorb bun 30787ebeab regression gate: locked-stream errors of cancel /
// pipeTo / pipeThrough must carry code ERR_INVALID_STATE with the Node.js
// message text (Node v26 webstreams parity). Covers all five upstream sites:
//   1. cancel() on a locked ReadableStream
//   2. pipeTo() on a locked source
//   3. pipeThrough() on a locked source (sync throw)
//   4. pipeTo() into a locked WritableStream destination
//   5. pipeThrough() into a locked WritableStream destination
// plus negative controls: the unrelated `this is not a ReadableStream`
// TypeError keeps NO code, and an unlocked pipeTo still completes.
//
// Single #[test] (mozjs single-init pattern, mirrors stream_p0_fix_tests).

use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use mozjs::rooted;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Drive timers (realm-entered drain_and_check), the MiniEventLoop and
/// microtasks (js::RunJobs) so promise rejections settle. Mirrors the pump
/// in stream_p0_fix_tests.
fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
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

#[test]
fn test_web_stream_locked_state_error_codes() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Probe all five locked-stream error faces + negative controls. Each
    // record is "name|code|message" so one eval round-trip carries the full
    // assertion surface (absorb bun 30787ebeab upstream repro shape).
    let setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            var rec = function(e) {
                return e ? (e.name + '|' + String(e.code) + '|' + e.message) : 'no-error';
            };
            var out = {};

            // 1. cancel() on a locked source (promise rejection).
            var rs1 = new ReadableStream();
            rs1.getReader();
            out.cancel = 'pending';
            rs1.cancel().then(function() { out.cancel = 'resolved'; },
                              function(e) { out.cancel = rec(e); });

            // 2. pipeTo() on a locked source (promise rejection).
            var rs2 = new ReadableStream();
            rs2.getReader();
            out.pipeToSrc = 'pending';
            rs2.pipeTo(new WritableStream()).then(
                function() { out.pipeToSrc = 'resolved'; },
                function(e) { out.pipeToSrc = rec(e); });

            // 3. pipeThrough() on a locked source (sync throw).
            var rs3 = new ReadableStream();
            rs3.getReader();
            try {
                rs3.pipeThrough(new TransformStream());
                out.pipeThroughSrc = 'no-throw';
            } catch (e) { out.pipeThroughSrc = rec(e); }

            // 4. pipeTo() into a locked destination (promise rejection).
            var ws4 = new WritableStream();
            ws4.getWriter();
            out.pipeToDest = 'pending';
            new ReadableStream().pipeTo(ws4).then(
                function() { out.pipeToDest = 'resolved'; },
                function(e) { out.pipeToDest = rec(e); });

            // 5. pipeThrough() into a locked destination (sync throw).
            var ws5 = new WritableStream();
            ws5.getWriter();
            try {
                new ReadableStream().pipeThrough({
                    readable: new ReadableStream(), writable: ws5 });
                out.pipeThroughDest = 'no-throw';
            } catch (e) { out.pipeThroughDest = rec(e); }

            // Negative control A: the `this` check keeps a PLAIN TypeError
            // (upstream touched only the locked-stream sites).
            ReadableStream.prototype.cancel.call({}).then(
                function() { out.wrongThis = 'resolved'; },
                function(e) { out.wrongThis = rec(e); });

            // Negative control B: an unlocked pipeTo still completes.
            out.unlocked = 'pending';
            new ReadableStream({ start: function(c) { c.close(); } })
                .pipeTo(new WritableStream())
                .then(function() { out.unlocked = 'resolved'; },
                      function(e) { out.unlocked = rec(e); });

            globalThis.__lockedProbe = out;
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(setup, "scheduled", "locked-state probe scheduling failed");
    drive_event_loop(&mut ctx, 100);

    let mut get = |k: &str| eval_string(&mut ctx, &format!("String(globalThis.__lockedProbe.{})\n", k));

    assert_eq!(
        get("cancel"),
        "TypeError|ERR_INVALID_STATE|Invalid state: ReadableStream is locked",
        "cancel() on locked stream must reject with ERR_INVALID_STATE (Node parity)"
    );
    assert_eq!(
        get("pipeToSrc"),
        "TypeError|ERR_INVALID_STATE|Invalid state: The ReadableStream is locked",
        "pipeTo() on locked source must reject with ERR_INVALID_STATE"
    );
    assert_eq!(
        get("pipeThroughSrc"),
        "TypeError|ERR_INVALID_STATE|Invalid state: The ReadableStream is locked",
        "pipeThrough() on locked source must throw ERR_INVALID_STATE TypeError"
    );
    assert_eq!(
        get("pipeToDest"),
        "TypeError|ERR_INVALID_STATE|Invalid state: The WritableStream is locked",
        "pipeTo() into locked destination must reject with ERR_INVALID_STATE"
    );
    assert_eq!(
        get("pipeThroughDest"),
        "TypeError|ERR_INVALID_STATE|Invalid state: The WritableStream is locked",
        "pipeThrough() into locked destination must throw ERR_INVALID_STATE TypeError"
    );
    // Negative control A: sites upstream did not touch stay plain (no code).
    assert_eq!(
        get("wrongThis"),
        "TypeError|undefined|this is not a ReadableStream",
        "the `this is not a ReadableStream` check must stay a plain TypeError without code"
    );
    // Negative control B: happy path untouched.
    assert_eq!(
        get("unlocked"), "resolved",
        "unlocked pipeTo must still complete"
    );
}
