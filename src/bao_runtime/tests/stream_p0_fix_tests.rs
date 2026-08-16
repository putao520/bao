// @trace TEST-ENG-007-STREAM [req:REQ-ENG-007 REQ-ENG-001 REQ-ENG-005] [level:integration]
// BCE-20260816-STREAM-PUMP / BCE-20260816-STREAM-WEB / BCE-20260816-BUF-VIEW /
// BCE-20260816-FETCH-DATA regression gates (v-surface P0 audit wave).
//
//   1. Node stream classes' event pump: Readable data/end from buffered
//      pushes, Readable.from(async iterable) for-await collection, full
//      pipe chains with byte assertions, callback pipeline, PassThrough,
//      Transform transform fn.
//   2. Web TransformStream: identity roundtrip + custom transform (must
//      not hang the event loop — the old polyfill's write() rejected with
//      a ReferenceError and read() spun in a setTimeout poll forever).
//   3. Buffer.subarray/slice zero-copy views: shared backing ArrayBuffer,
//      byteOffset edges, write-through in both directions.
//   4. fetch("data:...") local scheme short-circuit: plain and base64
//      forms roundtrip through the real Response class; invalid payloads
//      and non-GET/HEAD methods reject with TypeError.
//
// Single #[test] (mozjs single-init pattern, mirrors stream_buffer_assert_tests).
// No HTTPThread is ever scheduled (data: URLs are short-circuited locally),
// so no shutdown_for_exit/process::exit dance is needed.

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

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        _ => false,
    }
}

/// Drive timers (realm-entered drain_and_check), the MiniEventLoop and
/// microtasks (js::RunJobs) so promise/timer-based assertions settle.
/// Mirrors the two-part pump in fetch_abort_e2e_tests / fetch_e2e_tests.
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
fn test_stream_p0_fixes() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ══════════════════════════════════════════════════════════════════
    // Item 1 — Node stream event pump
    // ══════════════════════════════════════════════════════════════════

    // 1a. Readable: pushes buffered BEFORE on('data') must drain through
    //     data events and end (BCE-20260816-STREAM-PUMP fix 1). Flow starts
    //     on a microtask (Node nextTick parity), so the result is read
    //     after driving the loop. String pushes land as Buffers (Node
    //     readableAddChunk byte-mode encoding — BCE-20260817-STREAM-STRCHUNK).
    let r1a_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            var stream = require('stream');
            var r = new stream.Readable({ read() {} });
            r.push('abc'); r.push('def'); r.push(null);
            var data = [];
            globalThis.__r1a = 'pending';
            r.on('data', function(c) { data.push((Buffer.isBuffer(c) ? 'B:' + c.toString() : 'S:' + String(c))); });
            r.on('end', function() { globalThis.__r1a = data.join(',') + '|true'; });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(r1a_setup, "scheduled", "Readable probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let readable_events = eval_string(&mut ctx, "globalThis.__r1a");
    assert_eq!(
        readable_events, "B:abc,B:def|true",
        "Readable push-before-listen must drain data + end with Buffer chunks (got: {})",
        readable_events
    );

    // 1b. Transform: the user transform fn must be invoked (Duplex's own
    //     _write used to shadow Transform.prototype._write — fix 2).
    let transform_out = eval_string(
        &mut ctx,
        r#"
        (function() {
            var stream = require('stream');
            var t = new stream.Transform({ transform(c, e, cb) { cb(null, String(c).toUpperCase()); } });
            var out = '';
            var transformFnCalled = false;
            t._transform('probe', null, function() { transformFnCalled = true; });
            t.on('data', function(d) { out += (Buffer.isBuffer(d) ? d.toString() : d); });
            t.write('ab'); t.end();
            return out + '|' + transformFnCalled;
        })()
    "#,
    );
    assert_eq!(
        transform_out, "AB|true",
        "Transform transform fn must run and push output (got: {})",
        transform_out
    );

    // 1c. PassThrough: transparent passthrough with shared byte flow.
    let passthrough_out = eval_string(
        &mut ctx,
        r#"
        (function() {
            var stream = require('stream');
            var pt = new stream.PassThrough();
            var out = '';
            pt.on('data', function(d) { out += (Buffer.isBuffer(d) ? d.toString() : d); });
            pt.write('hello '); pt.write('world'); pt.end();
            return out;
        })()
    "#,
    );
    assert_eq!(
        passthrough_out, "hello world",
        "PassThrough must pass chunks through (got: {})",
        passthrough_out
    );

    // 1d. Pipe full chain: src.pipe(transform).pipe(sink) with byte
    //     assertions on the final sink output.
    let r1d_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            var stream = require('stream');
            var src = new stream.Readable({ read() {} });
            src.push(Buffer.from('chunk1|'));
            src.push(Buffer.from('chunk2|'));
            src.push(null);
            var upper = new stream.Transform({ transform(c, e, cb) { cb(null, Buffer.from(String(c).toUpperCase())); } });
            var sinkBytes = [];
            var sink = new stream.Writable({ write(c, e, cb) { sinkBytes.push(Buffer.from(c)); cb(); } });
            globalThis.__r1d = 'pending';
            src.pipe(upper).pipe(sink).on('finish', function() {
                var joined = '';
                for (var i = 0; i < sinkBytes.length; i++) joined += sinkBytes[i].toString();
                globalThis.__r1d = joined;
            });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(r1d_setup, "scheduled", "pipe chain probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let pipe_chain = eval_string(&mut ctx, "globalThis.__r1d");
    assert_eq!(
        pipe_chain, "CHUNK1|CHUNK2|",
        "pipe(src).pipe(transform).pipe(sink) must deliver transformed bytes (got: {})",
        pipe_chain
    );

    // 1e. pipeline callback form: cb(null) after the sink finishes.
    let r1e_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            var stream = require('stream');
            var src = new stream.Readable({ read() {} });
            src.push('a'); src.push('b'); src.push(null);
            var xf = new stream.Transform({ transform(c, e, cb) { cb(null, Buffer.concat([c, c])); } });
            var got = [];
            var sink = new stream.Writable({ write(c, e, cb) { got.push(Buffer.isBuffer(c) ? c.toString() : String(c)); cb(); } });
            globalThis.__r1e = 'pending';
            stream.pipeline(src, xf, sink, function(err) {
                globalThis.__r1e = got.join(',') + '|' + (err ? err.message : 'null');
            });
            return 'scheduled';
        })()
    "#,
    );
    assert_eq!(r1e_setup, "scheduled", "pipeline probe scheduling failed");
    drive_event_loop(&mut ctx, 100);
    let pipeline_cb = eval_string(&mut ctx, "globalThis.__r1e");
    assert_eq!(
        pipeline_cb, "aa,bb|null",
        "pipeline callback must fire with null error after sink finish (got: {})",
        pipeline_cb
    );

    // 1f. Readable.from(async iterable) → for await collect (BCE fix 3:
    //     the async-iterator waiter must wake on data).
    let setup = eval_string(
        &mut ctx,
        r#"
        (async function() {
            var stream = require('stream');
            var src = (async function*() {
                yield 'one';
                yield 'two';
                yield 'three';
            })();
            var r = stream.Readable.from(src);
            var collected = [];
            for await (var chunk of r) { collected.push(String(chunk)); }
            globalThis.__forawait_result = collected.join(',');
        })().catch(function(e) {
            globalThis.__forawait_result = 'THREW:' + (e && e.message);
        });
        'scheduled'
    "#,
    );
    assert_eq!(setup, "scheduled", "for-await probe scheduling failed");
    drive_event_loop(&mut ctx, 300);
    let forawait = eval_string(&mut ctx, "globalThis.__forawait_result");
    assert_eq!(
        forawait, "one,two,three",
        "Readable.from(async iterable) for-await must collect all chunks (got: {})",
        forawait
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 2 — Web TransformStream (must not hang; full WHATWG impl)
    // ══════════════════════════════════════════════════════════════════

    // 2a. Identity ctor roundtrip: write → read → close.
    let ts_setup = eval_string(
        &mut ctx,
        r#"
        (async function() {
            var ts = new TransformStream();
            var writer = ts.writable.getWriter();
            var reader = ts.readable.getReader();
            var got = [];
            await writer.write('hello ');
            await writer.write('world');
            await writer.close();
            var res = await reader.read();
            got.push(res.done ? '<done>' : String(res.value));
            res = await reader.read();
            got.push(res.done ? '<done>' : String(res.value));
            res = await reader.read();
            got.push(res.done ? '<done>' : String(res.value));
            globalThis.__ts_identity = got.join(',');
        })().catch(function(e) {
            globalThis.__ts_identity = 'THREW:' + (e && e.message);
        });
        'scheduled'
    "#,
    );
    assert_eq!(ts_setup, "scheduled", "TransformStream probe scheduling failed");
    drive_event_loop(&mut ctx, 300);
    let ts_identity = eval_string(&mut ctx, "globalThis.__ts_identity");
    assert_eq!(
        ts_identity, "hello ,world,<done>",
        "TransformStream identity roundtrip must deliver both written chunks then close (got: {})",
        ts_identity
    );

    // 2b. Custom transform: transform fn enqueues transformed output.
    let ts_custom_setup = eval_string(
        &mut ctx,
        r#"
        (async function() {
            var ts = new TransformStream({
                transform: function(chunk, controller) {
                    controller.enqueue(String(chunk).toUpperCase());
                }
            });
            var writer = ts.writable.getWriter();
            var reader = ts.readable.getReader();
            await writer.write('abc');
            await writer.close();
            var out = '';
            var res;
            while (!(res = await reader.read()).done) { out += res.value; }
            globalThis.__ts_custom = out;
        })().catch(function(e) {
            globalThis.__ts_custom = 'THREW:' + (e && e.message);
        });
        'scheduled'
    "#,
    );
    assert_eq!(ts_custom_setup, "scheduled", "custom TS probe scheduling failed");
    drive_event_loop(&mut ctx, 300);
    let ts_custom = eval_string(&mut ctx, "globalThis.__ts_custom");
    assert_eq!(
        ts_custom, "ABC",
        "TransformStream custom transform must enqueue transformed chunks (got: {})",
        ts_custom
    );

    // 2c. Event loop liveness after TransformStream use: a timer scheduled
    //     alongside must still fire (the old polyfill spun a setTimeout
    //     poll that blocked the loop from ever idling).
    let liveness_setup = eval_string(
        &mut ctx,
        r#"
        globalThis.__ts_timer = 'pending';
        var ts = new TransformStream();
        var w = ts.writable.getWriter();
        w.write('x').then(function() { return w.close(); });
        setTimeout(function() { globalThis.__ts_timer = 'fired'; }, 20);
        'scheduled'
    "#,
    );
    assert_eq!(liveness_setup, "scheduled", "liveness probe scheduling failed");
    drive_event_loop(&mut ctx, 400);
    let ts_timer = eval_string(&mut ctx, "globalThis.__ts_timer");
    assert_eq!(
        ts_timer, "fired",
        "timers must fire while/after TransformStream writes settle (got: {})",
        ts_timer
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 3 — Buffer.subarray / slice zero-copy views
    // ══════════════════════════════════════════════════════════════════

    // 3a. subarray shares the backing store; writes propagate both ways.
    let sub_view = eval_string(
        &mut ctx,
        r#"
        (function() {
            var b = Buffer.from('abcdef');
            var s = b.subarray(1, 4);
            var shared = (s.buffer === b.buffer);
            var offsetOk = s.byteOffset === 1 && s.length === 3;
            var content = s.toString();
            b[1] = 88; // write through the parent
            var parentVisible = s.toString();
            s[0] = 89; // write through the view
            var viewVisible = b.toString();
            return shared + ',' + offsetOk + ',' + content + ',' + parentVisible + ',' + viewVisible;
        })()
    "#,
    );
    assert_eq!(
        sub_view, "true,true,bcd,Xcd,aYcdef",
        "subarray must be a zero-copy view with bidirectional write-through (got: {})",
        sub_view
    );

    // 3b. slice keeps the same view semantics (already native; locked here).
    let slice_view = eval_string(
        &mut ctx,
        r#"
        (function() {
            var b = Buffer.from('abcdef');
            var s = b.slice(1, 4);
            var shared = (s.buffer === b.buffer);
            var offsetOk = s.byteOffset === 1 && s.length === 3;
            s[0] = 88;
            return shared + ',' + offsetOk + ',' + b.toString();
        })()
    "#,
    );
    assert_eq!(
        slice_view, "true,true,aXcdef",
        "slice must remain a shared-backing view (got: {})",
        slice_view
    );

    // 3c. Offset boundaries: negative start, clamped end, inverted range,
    //     whole-buffer subarray of a subarray (chained offsets).
    let sub_edges = eval_string(
        &mut ctx,
        r#"
        (function() {
            var b = Buffer.from('0123456789');
            var neg = b.subarray(-3).toString();               // '789'
            var clamped = b.subarray(2, 100).toString();       // '23456789'
            var inverted = b.subarray(5, 2).toString();        // ''
            var inner = b.subarray(2, 8);                      // offset 2, len 6
            var innerView = inner.subarray(1, 3).toString();   // '34'
            var innerOffset = inner.subarray(1, 3).byteOffset; // 3
            return neg + ',' + clamped + ',' + inverted + ',' + innerView + ',' + innerOffset;
        })()
    "#,
    );
    assert_eq!(
        sub_edges, "789,23456789,,34,3",
        "subarray offset edge cases (negative/clamp/invert/chained) (got: {})",
        sub_edges
    );

    // ══════════════════════════════════════════════════════════════════
    // Item 4 — fetch("data:...") local short-circuit
    // ══════════════════════════════════════════════════════════════════

    // 4a/4b. Plain + base64 forms roundtrip through the real Response.
    let data_setup = eval_string(
        &mut ctx,
        r#"
        (async function() {
            var out = [];
            try {
                var r1 = await fetch('data:text/plain,hello%20world');
                out.push('plain:' + r1.status + ':' + r1.headers.get('content-type') + ':' + (await r1.text()));
            } catch (e) { out.push('plain:THREW:' + (e && e.message)); }
            try {
                var r2 = await fetch('data:application/octet-stream;base64,aGVsbG8gZGF0YQ==');
                var ab = await r2.arrayBuffer();
                var bytes = new Uint8Array(ab);
                var acc = '';
                for (var i = 0; i < bytes.length; i++) acc += String.fromCharCode(bytes[i]);
                out.push('b64:' + r2.status + ':' + r2.headers.get('content-type') + ':' + acc + ':' + bytes.length);
            } catch (e) { out.push('b64:THREW:' + (e && e.message)); }
            try {
                await fetch('data:text/plain;base64,%%%invalid');
                out.push('badb64:RESOLVED');
            } catch (e) { out.push('badb64:' + (e instanceof TypeError ? 'TypeError' : ('OTHER:' + (e && e.message)))); }
            try {
                await fetch('data:text/plain,hi', { method: 'POST' });
                out.push('post:RESOLVED');
            } catch (e) { out.push('post:' + (e instanceof TypeError ? 'TypeError' : ('OTHER:' + (e && e.message)))); }
            globalThis.__data_fetch = out.join('|');
        })().catch(function(e) {
            globalThis.__data_fetch = 'TOPLEVEL:' + (e && e.message);
        });
        'scheduled'
    "#,
    );
    assert_eq!(data_setup, "scheduled", "data: fetch probe scheduling failed");
    drive_event_loop(&mut ctx, 300);
    let data_fetch = eval_string(&mut ctx, "globalThis.__data_fetch");
    assert_eq!(
        data_fetch,
        "plain:200:text/plain:hello world|b64:200:application/octet-stream:hello data:10|badb64:TypeError|post:TypeError",
        "data: fetch plain/base64 roundtrip + fail-closed rejections (got: {})",
        data_fetch
    );

    // 4c. Liveness: timers keep firing while data: fetches settle (the DNS
    //     retry loop used to freeze the whole JS thread).
    let data_liveness_setup = eval_string(
        &mut ctx,
        r#"
        globalThis.__data_timer = 'pending';
        fetch('data:text/plain,ok').then(function(r) { return r.text(); }).then(function(t) {
            globalThis.__data_timer = 'body:' + t;
        });
        setTimeout(function() {
            if (globalThis.__data_timer === 'pending') globalThis.__data_timer = 'TIMER-ONLY';
        }, 20);
        'scheduled'
    "#,
    );
    assert_eq!(data_liveness_setup, "scheduled", "data liveness probe failed");
    drive_event_loop(&mut ctx, 400);
    let data_timer = eval_string(&mut ctx, "globalThis.__data_timer");
    assert_eq!(
        data_timer, "body:ok",
        "event loop must stay live through data: fetch resolution (got: {})",
        data_timer
    );

    // Web-stream classes must also be re-exported on require('stream').
    assert!(
        eval_bool(
            &mut ctx,
            "var s = require('stream'); typeof s.ReadableStream === 'function' && typeof s.TransformStream === 'function' && typeof s.WritableStream === 'function'"
        ),
        "require('stream') must re-export the Web Streams constructors"
    );
    // require('stream/web') re-exports the same global constructors.
    assert!(
        eval_bool(
            &mut ctx,
            "var sw = require('stream/web'); typeof sw.TransformStream === 'function' && sw.TransformStream === globalThis.TransformStream"
        ),
        "require('stream/web') must re-export the global TransformStream"
    );

    bun_runtime::shutdown_thread_sm();
}
