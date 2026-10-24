// @trace TEST-ARCH-3-WORKER-CHANNEL-SUBSTRATE [req:REQ-ARCH-003] [level:integration]
//
// REQ-ARCH-003 wave R2 discriminators for the node_worker_threads dual-channel
// substrate swap (std::sync::mpsc → bao_ipc unbounded; payloads stay
// structured-clone bytes per DEC-WK-5, never JSObject pointers). Pins the
// two contract parity faces the swap must preserve:
//   1. Terminate message-order FIFO — data messages enqueued before
//      Terminate are consumed in send order before the worker loop breaks.
//   2. Exit close semantics — a worker that dies (thread exit drops the
//      worker→main sender) has every queued message delivered in order
//      (drain-then-close); after the drain the main-side receive observes
//      the disconnect (try_recv Err mapped to Empty) with no loss or hang.

use ::std::time::{Duration, Instant};
use bao_engine::context::{JsContext, thread_realm_global};
use bao_engine::value::JsValue;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;

use bun_runtime::node_worker_threads::{WorkerIncoming, worker_try_recv};

fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<worker-channel-test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        other => panic!("eval did not return a string: {:?}", other),
    }
}

fn unique_temp_path(tag: &str, ext: &str) -> ::std::path::PathBuf {
    ::std::env::temp_dir().join(format!(
        "bao-worker-{}-{}-{}.{}",
        tag,
        ::std::process::id(),
        ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos(),
        ext
    ))
}

/// One non-blocking poll of a worker's main-thread inbox. A data message is
/// published as `globalThis.<global_name>` in the main realm so the caller
/// can inspect it via eval (same shape as
/// node_worker_structured_clone_tests::recv_worker_reply).
fn poll_now(ctx: &mut JsContext, thread_id: u32, global_name: &str) -> WorkerIncoming {
    let mut cx = ctx.cx();
    let global_ptr = thread_realm_global().expect("main realm global");
    rooted!(&in(cx) let global = global_ptr);
    // Enter the main realm for the poll: JS_DefineProperty atomizes the
    // property name, which needs a current realm/zone on the context.
    let mut realm = AutoRealm::new_from_handle(&mut cx, global.handle());
    let rcx: &mut mozjs::context::JSContext = &mut realm;
    rooted!(&in(rcx) let mut rval = UndefinedValue());
    match worker_try_recv(rcx, thread_id, rval.handle_mut()) {
        WorkerIncoming::Data => {
            let name = format!("{}\0", global_name);
            unsafe {
                JS_DefineProperty(
                    rcx.raw_cx(),
                    global.handle().into(),
                    name.as_ptr() as *const ::std::os::raw::c_char,
                    rval.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            WorkerIncoming::Data
        }
        other => other,
    }
}

/// Poll until one message arrives (Empty = keep waiting); panics on timeout.
fn poll_one(ctx: &mut JsContext, thread_id: u32, global_name: &str) -> WorkerIncoming {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match poll_now(ctx, thread_id, global_name) {
            WorkerIncoming::Empty => {
                if Instant::now() > deadline {
                    panic!("timed out polling worker inbox (thread {})", thread_id);
                }
                ::std::thread::sleep(Duration::from_millis(25));
            }
            other => return other,
        }
    }
}

/// Discriminator 1 (Terminate FIFO): two data messages are enqueued, then
/// terminate. Terminate rides the SAME channel behind them, so the worker
/// must append 'A' then 'B' to the marker file before its loop breaks —
/// terminate() joins the thread, making the final state deterministic. A
/// substrate that let Terminate jump the queue, or dropped queued data on
/// close, leaves the file empty, truncated, or reordered.
#[test]
fn test_worker_terminate_consumes_queued_messages_in_order() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let marker = unique_temp_path("fifo-marker", "txt");
    let worker_path = unique_temp_path("fifo-src", "js");
    ::std::fs::write(
        &worker_path,
        format!(
            "var fs = require('fs');\nself.onmessage = function(e) {{\n  fs.appendFileSync({marker:?}, e.data + '\\n');\n}};\n",
            marker = marker.to_string_lossy().to_string()
        ),
    )
    .expect("write worker file");

    let mut ctx = make_ctx();

    eval_string(
        &mut ctx,
        &format!(
            r#"
(function() {{
  var wt = require('worker_threads');
  var w = new wt.Worker({worker_path:?});
  w.postMessage('A');
  w.postMessage('B');
  w.terminate();
  return 'terminated';
}})()
"#,
            worker_path = worker_path.to_string_lossy().to_string()
        ),
    );

    let content = ::std::fs::read_to_string(&marker).expect("marker file written by worker");
    assert_eq!(
        content, "A\nB\n",
        "messages enqueued before Terminate must be consumed in order before the loop breaks"
    );

    let _ = ::std::fs::remove_file(&marker);
    let _ = ::std::fs::remove_file(&worker_path);
    bun_runtime::shutdown_thread_sm();
}

/// Discriminator 2 (exit close semantics): the worker script posts two
/// messages then throws — worker_entry reports the script error and returns,
/// the thread exits, and the TLS worker→main sender drops (all-sender drop =
/// channel close). Every message queued before the death must still be
/// delivered in order (drain-then-close, not drop-on-close), and after the
/// drain the main-side receive maps the disconnect to Empty — stably, with
/// no hang and no resurrected messages.
#[test]
fn test_worker_death_delivers_queued_messages_then_disconnects() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let worker_path = unique_temp_path("death-src", "js");
    ::std::fs::write(
        &worker_path,
        "self.postMessage('first');\nself.postMessage('second');\nthrow new Error('boom');\n",
    )
    .expect("write worker file");

    let mut ctx = make_ctx();

    let tid = eval_string(
        &mut ctx,
        &format!(
            r#"
(function() {{
  var wt = require('worker_threads');
  var w = new wt.Worker({worker_path:?});
  globalThis.__deathWorker = w;
  return String(w.threadId);
}})()
"#,
            worker_path = worker_path.to_string_lossy().to_string()
        ),
    );
    let tid: u32 = tid.parse().expect("threadId as number");

    // Drain in order: both data messages queued before the death, then the
    // script-error report (FIFO on the single-producer worker→main channel).
    assert!(
        matches!(poll_one(&mut ctx, tid, "__wm0"), WorkerIncoming::Data),
        "first queued message must survive the worker's death"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__wm0)"),
        "first",
        "FIFO slot 1"
    );
    assert!(
        matches!(poll_one(&mut ctx, tid, "__wm1"), WorkerIncoming::Data),
        "second queued message must survive the worker's death"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__wm1)"),
        "second",
        "FIFO slot 2"
    );
    match poll_one(&mut ctx, tid, "__wm2") {
        WorkerIncoming::Error(msg) => assert!(
            msg.contains("boom"),
            "script error must ride the channel before close: {}",
            msg
        ),
        other => panic!(
            "expected Error message after the data messages, got {:?}",
            other
        ),
    }

    // Post-drain: the worker thread has exited (the Error report is sent
    // just before worker_entry returns), so its sender is dropped — the
    // closed channel yields Empty repeatedly, with no hang and no
    // resurrected messages.
    for _ in 0..5 {
        assert_eq!(
            poll_now(&mut ctx, tid, "__wmDrained"),
            WorkerIncoming::Empty,
            "disconnected channel must stay empty after the drain"
        );
    }

    // Release the registry entry (thread already finished: the join is
    // immediate; the Terminate send hits the closed main→worker channel and
    // is swallowed by the existing `let _ =`).
    eval_string(&mut ctx, "globalThis.__deathWorker.terminate(); 'ok'");

    let _ = ::std::fs::remove_file(&worker_path);
    bun_runtime::shutdown_thread_sm();
}
