// @trace TEST-ENG-FETCH-RSB [req:REQ-ENG-001] [level:integration]
// Response constructed with a JS ReadableStream body — the e129-registered
// candidate (blob_slice_stream_tests.rs:66 noted `new Response(stream())`
// as a separate pre-existing face; this lock closes it).
//
// WHATWG fetch §dom-response: the constructor accepts a ReadableStream
// body; extraction throws TypeError when the stream is disturbed or
// locked; otherwise the stream itself is the body — the `body` getter
// returns that exact stream object on every access (body identity, same
// contract as the Request face's `_bodyStreamSource` branch), bodyUsed is
// true once the stream is locked or drained, and the body mixin
// (text/json/arrayBuffer/blob) drains it through getReader().
//
// Locks, in verdict order:
//   sync  — bodyIdentity  resp.body === the constructed stream
//         — lockedCtor   new Response(locked stream) throws TypeError
//         — usedBefore/usedAfterReader
//                        bodyUsed flips false → true when a consumer calls
//                        getReader() on resp.body (locked = the JS-observable
//                        half of "disturbed or locked")
//   async — anchor       the e129 row itself: blob.slice().stream() piped
//                        through new Response(...).text() returns the slice
//         — identityLive resp.body identity holds for a pull-model stream
//         — sizes/streamRead
//                        getReader().read() yields the queued chunks
//                        incrementally in order (2 then 2 bytes), not a
//                        buffered terminal copy
//         — textWhole    multi-chunk text() concat, body identity + used
//                        flags after the drain, second text() rejects
//         — arrayBuffer  byte-exact across chunk boundaries
//         — json         JSON text parsed through the stream drain
//         — blobText/blobType
//                        blob() carries the drained bytes + Content-Type
//         — clone        clone() on a stream body refuses loudly
//                        (single-consumer refusal, same idiom as a live
//                        transport stream)
//
// All probes are realm-local (no network): the Response is constructed
// directly, per the registered gap's scenario.

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    common::eval_string_full_named(ctx, source, "<rsb>")
}

fn pump_until_done(ctx: &mut JsContext, deadline_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    while std::time::Instant::now() < deadline {
        let cx_raw = ctx.raw_cx();
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        let mut cxm = ctx.cx();
        let _ = bun_runtime::timers::drain_and_check(&mut cxm);
        let status = eval_string(ctx, "globalThis.__rsb.state");
        if status == "done" {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    false
}

#[test]
fn test_response_readablestream_body_streaming_reads() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let out = eval_string(
        &mut ctx,
        r#"
        globalThis.__rsb = { state: 'pending', log: [] };
        function L(k, v) { globalThis.__rsb.log.push(k + '=' + v); }
        function enc(s) { return new TextEncoder().encode(s); }

        // ── sync probes ──────────────────────────────────────────────
        var stream = new ReadableStream({
          start: function(c) { c.enqueue(enc('he')); c.enqueue(enc('llo')); c.close(); }
        });
        var resp = new Response(stream);
        L('bodyIdentity', resp.body === stream);

        var locked = new ReadableStream({ start: function(c) { c.close(); } });
        locked.getReader();
        try {
          new Response(locked);
          L('lockedCtor', 'NO_THROW');
        } catch (e) {
          L('lockedCtor', e instanceof TypeError ? 'TypeError' : 'OTHER:' + String(e));
        }

        var s2 = new ReadableStream({ start: function(c) { c.close(); } });
        var r2 = new Response(s2);
        L('usedBefore', r2.bodyUsed);
        var rd2 = s2.getReader();
        L('usedAfterReader', r2.bodyUsed);

        // ── async chain (strictly sequential, deterministic log order) ──
        var chain = Promise.resolve();

        chain = chain.then(function() {
          // The registered e129 row: a Blob slice's stream through a
          // Response body.
          var blob = new Blob(['SECRET-public-SECRET']);
          return new Response(blob.slice(7, 13).stream()).text();
        }).then(function(t) { L('anchor', t); });

        chain = chain.then(function() {
          var pulls = 0;
          var cs = new ReadableStream({
            start: function(c) { c.enqueue(enc('ab')); },
            pull: function(c) { pulls++; if (pulls === 1) c.enqueue(enc('cd')); else c.close(); }
          });
          var cr = new Response(cs);
          L('identityLive', cr.body === cs);
          var reader = cr.body.getReader();
          var sizes = [];
          var acc = '';
          function pump() {
            return reader.read().then(function(r) {
              if (r.done) return Promise.reject({ __done: true, acc: acc, sizes: sizes });
              sizes.push(r.value.byteLength);
              acc += new TextDecoder().decode(r.value);
              return pump();
            });
          }
          return pump().then(null, function(e) {
            if (e && e.__done) return e;
            throw e;
          });
        }).then(function(r) {
          L('sizes', r.sizes.join(','));
          L('streamRead', r.acc);
        });

        chain = chain.then(function() {
          var ts = new ReadableStream({
            start: function(c) { c.enqueue(enc('foo')); c.enqueue(enc('bar')); c.enqueue(enc('!')); c.close(); }
          });
          var tr = new Response(ts);
          return tr.text().then(function(t) {
            L('textWhole', t);
            L('bodyAfterText', tr.body === ts);
            L('usedAfterText', tr.bodyUsed);
            return tr.text();
          }).then(function(t2) {
            L('secondText', 'RESOLVED:' + t2);
          }, function(e) {
            L('secondText', 'REJECT:' + String((e && e.message) || e));
          });
        });

        chain = chain.then(function() {
          var as = new ReadableStream({
            start: function(c) { c.enqueue(enc('AB')); c.enqueue(enc('CD')); c.close(); }
          });
          return new Response(as).arrayBuffer();
        }).then(function(buf) {
          L('arrayBuffer', new TextDecoder().decode(new Uint8Array(buf)));
        });

        chain = chain.then(function() {
          var js = new ReadableStream({
            start: function(c) { c.enqueue(enc('{"x":42}')); c.close(); }
          });
          return new Response(js).json();
        }).then(function(v) {
          L('json', v.x);
        });

        chain = chain.then(function() {
          var bs = new ReadableStream({
            start: function(c) { c.enqueue(enc('zz')); c.close(); }
          });
          return new Response(bs, { headers: { 'content-type': 'text/plain' } }).blob();
        }).then(function(b) {
          L('blobType', b.type);
          return b.text();
        }).then(function(t) {
          L('blobText', t);
        });

        chain = chain.then(function() {
          try {
            new Response(new ReadableStream()).clone();
            L('clone', 'NO_THROW');
          } catch (e) {
            L('clone', e instanceof TypeError ? 'TypeError' : 'OTHER:' + String(e));
          }
        });

        chain.then(function() {
          globalThis.__rsb.state = 'done';
        }, function(e) {
          L('chainError', String((e && e.message) || e));
          globalThis.__rsb.state = 'done';
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled", "probe script must schedule cleanly");

    assert!(
        pump_until_done(&mut ctx, 10_000),
        "probe chain did not finish in time; state={}",
        eval_string(&mut ctx, "globalThis.__rsb.state + '|' + globalThis.__rsb.log.join('|')")
    );

    let verdict = eval_string(
        &mut ctx,
        "globalThis.__rsb.log.join('|')",
    );
    assert_eq!(
        verdict,
        "bodyIdentity=true|lockedCtor=TypeError|usedBefore=false|usedAfterReader=true|\
         anchor=public|identityLive=true|sizes=2,2|streamRead=abcd|\
         textWhole=foobar!|bodyAfterText=true|usedAfterText=true|\
         secondText=REJECT:Body is unusable|arrayBuffer=ABCD|json=42|\
         blobType=text/plain|blobText=zz|clone=TypeError",
        "new Response(ReadableStream) must surface the stream as its body and \
         drain it through getReader() (e129 registered candidate): {}",
        verdict
    );
    bun_runtime::shutdown_thread_sm();
}
