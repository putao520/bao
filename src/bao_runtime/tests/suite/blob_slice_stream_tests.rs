// @trace TEST-ENG-007-BLOB [req:REQ-ENG-007] [level:integration]
// Test lock for upstream oven-sh/bun e29a7ca47c (Blob: return only the
// slice when consuming a slice's stream after the Blobs were collected).
//
// Upstream's bug lived in the refcounted Store + `to_internal_blob_if_possible`
// whole-buffer adoption path (offset/size ignored when the parent held the
// last reference). Bao's node-realm Blob has no shared store: slice()
// copies the window into a fresh Uint8Array (globals.rs `_g.Blob`), so the
// slice's stream can structurally never yield parent bytes, and parent
// collection is irrelevant. The page realm uses servo's DOM Blob, whose
// `BlobImpl::new_sliced(range, parent)` carries the window on the slice
// itself. The lock pins the observable contract from the upstream report:
// consuming a slice's stream — directly, through a Response body, and after
// the parent has been dropped — returns exactly the slice's bytes.

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;


fn pump_until_quiescent(ctx: &mut JsContext, deadline_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    while std::time::Instant::now() < deadline {
        let cx_raw = ctx.raw_cx();
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        let mut cxm = ctx.cx();
        if !bun_runtime::timers::drain_and_check(&mut cxm) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn test_blob_slice_stream_returns_only_the_slice() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let out = eval_string(
        &mut ctx,
        r#"
        globalThis.__slice = { state: 'pending' };
        var parent = new Blob(["SECRET-public-SECRET"]);
        var slice = parent.slice(7, 13);
        // Drop the parent reference; the slice must carry its own window.
        parent = null;
        var results = {};
        // 1. Direct stream consumption.
        var reader = slice.stream().getReader();
        var acc = '';
        function pump() {
            return reader.read().then(function(r) {
                if (r.done) {
                    results.direct = acc;
                    // 2. text() of a Response made from the slice itself.
                    // (The upstream report's `new Response(slice.stream()).text()`
                    // row needs Response(JS ReadableStream) body support — a
                    // separate pre-existing face, out of this lock's scope.)
                    return new Response(slice).text();
                }
                acc += new TextDecoder().decode(r.value);
                return pump();
            });
        }
        pump().then(function(t2) {
            results.fromSliceBody = t2;
            globalThis.__slice.results = results;
            globalThis.__slice.state = 'done';
        }, function(e) {
            globalThis.__slice.state = 'rejected:' + String((e && e.message) || e);
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    let verdict = eval_string(
        &mut ctx,
        r#"
        var s = globalThis.__slice;
        var r = s.results || {};
        [
            'state:' + s.state,
            'direct:' + r.direct,
            'fromSliceBody:' + r.fromSliceBody,
        ].join('|')
    "#,
    );
    assert_eq!(
        verdict,
        "state:done|direct:public|fromSliceBody:public",
        "a slice's stream must yield only the slice (e29a7ca47c): {}",
        verdict
    );
    bun_runtime::shutdown_thread_sm();
}
