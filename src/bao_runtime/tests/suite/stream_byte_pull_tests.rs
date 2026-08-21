// @trace TEST-ENG-007-STREAM-BYTE-PULL [req:REQ-ENG-007] [level:integration]
// Regression gate for the WHATWG ReadableByteStreamController [[PullSteps]]
// port in web_streams.js (byte-controller + default reader, the mixed
// branch ReadableStreamDefaultReader.prototype.read used to park a read
// request with no pull call and no queue settlement):
//
//   1. hwm=0 byte stream: a parked read request must trigger pull — the
//      spec's pull steps end with CallPullIfNeeded after AddReadRequest;
//      with hwm=0 desiredSize never goes positive, so without that call
//      the read hung forever.
//   2. Pre-queued bytes (enqueued while unlocked) must settle the first
//      reads in FIFO order, and a close deferred behind a non-empty queue
//      must fire once the queue drains (FillReadRequestFromQueue +
//      HandleQueueDrain).
//   3. enqueue with NO parked read request must queue the chunk, not
//      silently drop it (the byte mirror of the default-controller enqueue
//      guard): two enqueues inside one pull call must both be readable.
//   4. BYOB readInto parks → pull must fire (spec pull steps' final
//      CallPullIfNeeded applies to BYOB reads; hwm=0 has no other trigger).
//   5. Pre-queued bytes + close under a BYOB reader: partial views drain
//      the queue across reads and the deferred close fires exactly when
//      the queue empties (HandleQueueDrain in the readInto queue branch).
//   6. autoAllocateChunkSize + default reader: parked read allocates the
//      byobRequest, the pull source writes into it and respond() settles
//      the parked read request with the filled prefix (minimal model of
//      spec pull steps 4-5).
//   7. enqueue settling a parked default read invalidates an outstanding
//      auto-allocated byobRequest — a late respond() on it must throw
//      TypeError("invalidated"), not double-settle.
//
// Single #[test] (mozjs single-init pattern, mirrors stream_p0_fix_tests).
// No HTTPThread is scheduled, so no shutdown dance is needed.

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
/// microtasks (js::RunJobs) so promise/timer-based assertions settle.
/// Mirrors the two-part pump in stream_p0_fix_tests.
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
fn test_stream_byte_controller_pull_steps() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ══════════════════════════════════════════════════════════════════
    // Item 1 — parked read request triggers pull (hwm=0 byte stream)
    // ══════════════════════════════════════════════════════════════════
    let t1_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__t1 = 'pending';
            var pulls = 0;
            var stream = new ReadableStream({
                type: 'bytes',
                strategy: { highWaterMark: 0 },
                pull: function(c) {
                    pulls++;
                    if (pulls <= 3) c.enqueue(new Uint8Array([pulls]));
                }
            });
            var reader = stream.getReader();
            reader.read().then(function(r1) {
                return reader.read().then(function(r2) {
                    globalThis.__t1 = r1.value[0] + ',' + r2.value[0] +
                        ',pulls=' + pulls +
                        ',u8=' + (r1.value instanceof Uint8Array) +
                        ',done=' + r1.done + ',' + r2.done;
                });
            }).catch(function(e) { globalThis.__t1 = 'ERR:' + e; });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(t1_setup, "scheduled", "byte pull probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let t1 = eval_string(&mut ctx, "globalThis.__t1");
    assert_eq!(
        t1, "1,2,pulls=2,u8=true,done=false,false",
        "byte-stream parked read must settle via pull with Uint8Array chunks (got: {})",
        t1
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 2 — pre-queued bytes FIFO + deferred close behind the queue
    // ══════════════════════════════════════════════════════════════════
    let t2_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__t2 = 'pending';
            var stream = new ReadableStream({
                type: 'bytes',
                start: function(c) {
                    c.enqueue(new Uint8Array([10, 11]));
                    c.enqueue(new Uint8Array([20]));
                    c.close();
                },
                pull: function() { globalThis.__t2pull = (globalThis.__t2pull || 0) + 1; }
            });
            var reader = stream.getReader();
            var out = [];
            function next() {
                return reader.read().then(function(r) {
                    out.push(r.done ? 'done' : Array.from(r.value).join('.'));
                    if (out.length < 3) return next();
                    globalThis.__t2 = out.join('|');
                });
            }
            next().catch(function(e) { globalThis.__t2 = 'ERR:' + e; });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(t2_setup, "scheduled", "byte queue probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let t2 = eval_string(&mut ctx, "globalThis.__t2");
    assert_eq!(
        t2, "10.11|20|done",
        "pre-queued byte chunks must drain FIFO then the deferred close must fire (got: {})",
        t2
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 3 — enqueue with no parked request queues, never drops
    // ══════════════════════════════════════════════════════════════════
    let t3_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__t3 = 'pending';
            var stream = new ReadableStream({
                type: 'bytes',
                strategy: { highWaterMark: 0 },
                pull: function(c) {
                    c.enqueue(new Uint8Array([1]));
                    c.enqueue(new Uint8Array([2]));
                }
            });
            var reader = stream.getReader();
            reader.read().then(function(r1) {
                return reader.read().then(function(r2) {
                    globalThis.__t3 = r1.value[0] + ',' + r2.value[0];
                });
            }).catch(function(e) { globalThis.__t3 = 'ERR:' + e; });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(t3_setup, "scheduled", "byte enqueue probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let t3 = eval_string(&mut ctx, "globalThis.__t3");
    assert_eq!(
        t3, "1,2",
        "second enqueue inside one pull call must queue for the next read, not drop (got: {})",
        t3
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 4 — BYOB parked readInto triggers pull (hwm=0)
    // ══════════════════════════════════════════════════════════════════
    let t4_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__t4 = 'pending';
            var pulls = 0;
            var stream = new ReadableStream({
                type: 'bytes',
                strategy: { highWaterMark: 0 },
                pull: function(c) {
                    pulls++;
                    c.enqueue(new Uint8Array([7, 7, 7, 7]));
                }
            });
            var reader = stream.getReader({ mode: 'byob' });
            reader.read(new Uint8Array(new ArrayBuffer(4))).then(function(r) {
                globalThis.__t4 = 'val=' + r.value[0] + ',len=' + r.value.byteLength + ',pulls=' + pulls;
            }).catch(function(e) { globalThis.__t4 = 'ERR:' + e; });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(t4_setup, "scheduled", "byob pull probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let t4 = eval_string(&mut ctx, "globalThis.__t4");
    assert_eq!(
        t4, "val=7,len=4,pulls=1",
        "parked BYOB readInto must settle via pull (got: {})",
        t4
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 5 — pre-queued bytes + deferred close under a BYOB reader
    // ══════════════════════════════════════════════════════════════════
    let t5_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__t5 = 'pending';
            var stream = new ReadableStream({
                type: 'bytes',
                start: function(c) {
                    c.enqueue(new Uint8Array([1, 2, 3, 4]));
                    c.close();
                }
            });
            var reader = stream.getReader({ mode: 'byob' });
            var out = [];
            function next() {
                return reader.read(new Uint8Array(new ArrayBuffer(2))).then(function(r) {
                    out.push(r.done ? 'done' : r.value[0] + '.' + r.value[1]);
                    if (out.length < 3) return next();
                    globalThis.__t5 = out.join('|');
                });
            }
            next().catch(function(e) { globalThis.__t5 = 'ERR:' + e; });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(t5_setup, "scheduled", "byob close probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let t5 = eval_string(&mut ctx, "globalThis.__t5");
    assert_eq!(
        t5, "1.2|3.4|done",
        "BYOB partial views must drain the queue then the deferred close must fire (got: {})",
        t5
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 6 — autoAllocateChunkSize: byobRequest + respond() settle a
    // parked default-reader read (spec pull steps 4-5, minimal model)
    // ══════════════════════════════════════════════════════════════════
    let t6_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__t6 = 'pending';
            var stream = new ReadableStream({
                type: 'bytes',
                autoAllocateChunkSize: 8,
                strategy: { highWaterMark: 0 },
                pull: function(c) {
                    if (c.byobRequest) {
                        var v = c.byobRequest.view;
                        v.set([9, 8, 7]);
                        c.byobRequest.respond(3);
                    }
                }
            });
            var reader = stream.getReader();
            reader.read().then(function(r) {
                globalThis.__t6 = r.value[0] + ',' + r.value[1] + ',' + r.value[2] + ',len=' + r.value.byteLength;
            }).catch(function(e) { globalThis.__t6 = 'ERR:' + e; });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(t6_setup, "scheduled", "autoAllocate probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let t6 = eval_string(&mut ctx, "globalThis.__t6");
    assert_eq!(
        t6, "9,8,7,len=3",
        "auto-allocated byobRequest must settle the parked default read via respond (got: {})",
        t6
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 7 — enqueue settling the parked read invalidates the stale
    // byobRequest (late respond throws TypeError, not double-settle)
    // ══════════════════════════════════════════════════════════════════
    let t7_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__t7 = 'pending';
            var stream = new ReadableStream({
                type: 'bytes',
                autoAllocateChunkSize: 8,
                strategy: { highWaterMark: 0 },
                pull: function(c) {
                    globalThis.__staleReq = c.byobRequest;
                    c.enqueue(new Uint8Array([5]));
                }
            });
            var reader = stream.getReader();
            reader.read().then(function(r1) {
                var err = 'none';
                try { globalThis.__staleReq.respond(1); } catch (e) {
                    err = e instanceof TypeError ? 'TypeError' : String(e);
                }
                globalThis.__t7 = 'first=' + r1.value[0] + ',stale=' + err;
            }).catch(function(e) { globalThis.__t7 = 'ERR:' + e; });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(t7_setup, "scheduled", "stale byob probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let t7 = eval_string(&mut ctx, "globalThis.__t7");
    assert_eq!(
        t7, "first=5,stale=TypeError",
        "enqueue-settled read must invalidate its byobRequest (got: {})",
        t7
    );
}
