// @trace TEST-ENG-006-WORKER-SC [req:REQ-ENG-006] [level:e2e]
//
// Worker postMessage structured-clone type-fidelity tests (real OS-thread
// workers, real mpsc wire): main serializes → worker deserializes → worker
// validates types in JS → worker serializes back → main deserializes via
// `worker_try_recv` and asserts. Under the old JSON serialization every
// non-JSON type here was corrupted (TypedArray → object, Map/Set → "{}",
// Date → string, BigInt → thrown away, cyclic → TypeError).

use ::std::time::{Duration, Instant};
use bao_engine::context::{thread_realm_global, JsContext};
use bao_engine::value::JsValue;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;

use bun_runtime::node_worker_threads::{worker_try_recv, WorkerIncoming};

fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<worker-sc-test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        other => panic!("eval did not return a string: {:?}", other),
    }
}

/// Write a worker script to a unique temp file, return its path.
fn write_worker_file(tag: &str, body: &str) -> String {
    let path =
        ::std::env::temp_dir().join(format!("bao-worker-sc-{}-{}.js", tag, ::std::process::id()));
    ::std::fs::write(&path, body).expect("write worker file");
    path.to_string_lossy().to_string()
}

/// Poll `worker_try_recv` until a data message arrives, then expose it to JS
/// as `globalThis.<global_name>` and return. Panics on error / timeout.
fn recv_worker_reply(ctx: &mut JsContext, thread_id: u32, global_name: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cx = ctx.cx();
    let global_ptr = thread_realm_global().expect("main realm global");
    rooted!(&in(cx) let global = global_ptr);
    // Enter the main realm for the whole receive: JS_DefineProperty atomizes
    // the property name, which requires a current realm/zone on the context.
    let mut realm = AutoRealm::new_from_handle(&mut cx, global.handle());
    let rcx: &mut mozjs::context::JSContext = &mut realm;
    rooted!(&in(rcx) let mut reply = UndefinedValue());
    loop {
        match worker_try_recv(rcx, thread_id, reply.handle_mut()) {
            WorkerIncoming::Data => {
                let name = format!("{}\0", global_name);
                unsafe {
                    JS_DefineProperty(
                        rcx.raw_cx(),
                        global.handle().into(),
                        name.as_ptr() as *const ::std::os::raw::c_char,
                        reply.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                return;
            }
            WorkerIncoming::Error(msg) => panic!("worker error: {}", msg),
            WorkerIncoming::Empty => {}
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for worker reply (thread {})", thread_id);
        }
        ::std::thread::sleep(Duration::from_millis(25));
    }
}

/// Full round trip: every structured-clone-only type must survive
/// main → worker → main with types and internal identity intact, plus
/// DataCloneError on uncloneable values and on the transfer list.
#[test]
fn test_worker_post_message_structured_clone_round_trip() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let worker_path = write_worker_file(
        "types",
        r#"
self.onmessage = function(e) {
  var d = e.data;
  var checks = [];
  checks.push(['u8', d.u8 instanceof Uint8Array && d.u8.length === 5 && d.u8[0] === 1 && d.u8[4] === 255]);
  checks.push(['i32', d.i32 instanceof Int32Array && d.i32[1] === -2]);
  checks.push(['f64', d.f64 instanceof Float64Array && d.f64[0] === 1.5]);
  checks.push(['map', d.m instanceof Map && d.m.get('k') === 42 && d.m.get('obj').deep === true]);
  checks.push(['set', d.s instanceof Set && d.s.has('x') && d.s.size === 3]);
  checks.push(['date', d.d instanceof Date && d.d.getTime() === 1234567890123]);
  checks.push(['bigint', typeof d.big === 'bigint' && d.big === 9007199254740993n]);
  checks.push(['cyclic', d.circ.self === d.circ && d.circ.name === 'c']);
  checks.push(['wd-tag', workerData.tag === 'sc']);
  checks.push(['wd-u8', workerData.arr instanceof Uint8Array && workerData.arr[1] === 9]);
  checks.push(['wd-date', workerData.when instanceof Date && workerData.when.getTime() === 777]);
  var failed = checks.filter(function(c) { return !c[1]; }).map(function(c) { return c[0]; });
  self.postMessage(failed.length === 0 ? 'ALL_OK' : 'FAIL:' + failed.join(','));
};
"#,
    );

    let mut ctx = make_ctx();

    let tid = eval_string(
        &mut ctx,
        &format!(
            r#"
(function() {{
  var wt = require('worker_threads');
  var circ = {{ name: 'c' }};
  circ.self = circ;
  var w = new wt.Worker({worker_path:?}, {{ workerData: {{ tag: 'sc', arr: new Uint8Array([9, 9]), when: new Date(777) }} }});
  globalThis.__testWorker = w;
  w.postMessage({{
    u8: new Uint8Array([1, 2, 3, 4, 255]),
    i32: new Int32Array([1, -2, 3]),
    f64: new Float64Array([1.5]),
    m: new Map([['k', 42], ['obj', {{ deep: true }}]]),
    s: new Set(['x', 'y', 'z']),
    d: new Date(1234567890123),
    big: 9007199254740993n,
    circ: circ,
  }});
  return String(w.threadId);
}})()
"#
        ),
    );
    let tid: u32 = tid.parse().expect("threadId as number");
    assert!(tid > 0, "worker threadId must be > 0, got {}", tid);

    recv_worker_reply(&mut ctx, tid, "__workerReply");

    let result = eval_string(
        &mut ctx,
        r#"
(function() {
  var ok1 = globalThis.__workerReply === 'ALL_OK';
  // Uncloneable value (function) must throw DataCloneError, not degrade to null.
  var ok2 = false;
  try { globalThis.__testWorker.postMessage(function() {}); }
  catch (e) { ok2 = String(e).indexOf('could not be cloned') !== -1; }
  // Non-empty transfer list is explicitly rejected.
  var ok3 = false;
  try { globalThis.__testWorker.postMessage({ a: 1 }, [new ArrayBuffer(8)]); }
  catch (e) { ok3 = true; }
  globalThis.__testWorker.terminate();
  return (ok1 ? 'ok1' : 'bad1') + ',' + (ok2 ? 'ok2' : 'bad2') + ',' + (ok3 ? 'ok3' : 'bad3');
})()
"#,
    );
    assert_eq!(
        result, "ok1,ok2,ok3",
        "round-trip/DataCloneError checks failed"
    );

    let _ = ::std::fs::remove_file(&worker_path);
    bun_runtime::shutdown_thread_sm();
}

/// Scalar edge cases survive the wire exactly (SC, not JSON): NaN, -0,
/// undefined, null, empty string, 0.
#[test]
fn test_worker_post_message_scalar_edges() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let worker_path = write_worker_file(
        "edges",
        r#"
self.onmessage = function(e) { self.postMessage(e.data); };
"#,
    );

    let mut ctx = make_ctx();

    let tid = eval_string(
        &mut ctx,
        &format!(
            r#"
(function() {{
  var wt = require('worker_threads');
  var w = new wt.Worker({worker_path:?});
  globalThis.__edgeWorker = w;
  w.postMessage({{ nan: NaN, neg0: -0, undef: undefined, nul: null, str: '', num: 0 }});
  return String(w.threadId);
}})()
"#
        ),
    );
    let tid: u32 = tid.parse().expect("threadId as number");

    recv_worker_reply(&mut ctx, tid, "__edgeReply");

    let result = eval_string(
        &mut ctx,
        r#"
(function() {
  var r = globalThis.__edgeReply;
  var ok = typeof r.nan === 'number' && isNaN(r.nan)
    && Object.is(r.neg0, -0)
    && r.undef === undefined
    && r.nul === null
    && r.str === '' && r.num === 0;
  globalThis.__edgeWorker.terminate();
  return ok ? 'EDGE_OK' : 'EDGE_FAIL';
})()
"#,
    );
    assert_eq!(result, "EDGE_OK", "scalar edge round trip failed");

    let _ = ::std::fs::remove_file(&worker_path);
    bun_runtime::shutdown_thread_sm();
}
