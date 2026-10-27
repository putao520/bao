// @trace REQ-ENG-007/REQ-ENG-008/REQ-ENG-009 [level:integration]
//
// Silent-fake eradication, group D regression tests.
//
// Contract under test: every node-compat surface either performs the real
// behavior or fails explicitly (throw / errno / rejected Promise) — never a
// silent success that pretends work happened.
//
// Covered surfaces:
//   1. node:test        — two-form runner gate (node parity, E-test wave):
//                         `bao test` registers for the Rust-driven runner;
//                         every other form (plain run / -e / ESM import)
//                         ACCEPTS registration and arms a one-shot exit-drain
//                         that executes the registered suites after the main
//                         module finishes, carries failures into
//                         process.exitCode (node v24.19.0 oracle 2026-10-27:
//                         plain-run registration never throws — the old
//                         "refuse outside the runner" pin contradicted node
//                         and is superseded; the silent-fake risk is gone
//                         because the drain really executes + counts)
//                         (CJS require + ESM import share the gated bridge)
//   1b. bao test runner — plain-script (CJS) files and `-e` evals drive the
//                         registered suites, not a vacuous 0/0 report
//   2. readline         — question() (sync + promises) reads a real line
//                         from stdin; EOF fails explicitly
//   3. _http_client     — ClientRequest construction throws instead of
//                         accepting a body it would silently drop; the
//                         http module's named typeof-surface classes
//                         (ClientRequest/IncomingMessage/OutgoingMessage)
//                         refuse construction the same way
//   4. worker_threads   — BroadcastChannel does real registry fan-out;
//                         bare MessagePort construction refuses
//   5. wasi             — fd_seek SEEK_END returns ENOSYS, not fake success
//   6. DOMParser        — CLI mode throws instead of returning a pseudo-DOM

use bao_engine::context::{JsContext, thread_realm_global};
use bao_engine::module_loader::ModuleLoader;
use bao_engine::value::JsValue;
use mozjs::rooted;
use std::sync::Mutex;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<silent-fake-d>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(_) => String::new(),
        Err(e) => format!("EVAL_ERR:{}", e),
    }
}

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Drive the run-form exit-drain to completion: fire the armed setTimeout(0)
/// (timers tick), settle the runner promise chain (RunJobs), repeat. The
/// node run-form contract is "registration accepted, execution deferred to
/// after the main module" — this is the in-test equivalent of the main
/// module finishing and the loop draining (node oracle: the body runs only
/// after the registering eval returns, and a failing body lands in
/// process.exitCode === 1). Same actuator set as the shared suite
/// pump_tick, inlined here because this file keeps its own harness.
fn drive_settle(ctx: &mut JsContext, passes: usize) {
    let cx_raw = ctx.raw_cx();
    for _ in 0..passes {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        // Drive bao timers too: the run-form drain is armed as a setTimeout(0)
        // anchor in BAO_REGISTRY, which tick_without_idle (tasklet/uWS face)
        // never fires — without this drain the anchor stays pending forever.
        bun_runtime::timers::drain_bao_timers(cx_raw);
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 1. node:test
// ─────────────────────────────────────────────────────────────────────────

// node v24.19.0 oracle (2026-10-27, /tmp/bao-oracle-c1 probes): plain-run
// node:test registration NEVER throws — `node -e "nt.test('x', fn)"` prints
// "✔ x" via the deferred run-form drain, and a failing body lands in
// process.exitCode === 1. The former pin ("refuse outside `bao test`")
// described a pre-E-test-wave bao behavior, not node's; the fake-pass risk
// it guarded against is eradicated by the drain REALLY executing (below).
#[test]
fn node_test_plain_run_registers_and_executes_via_drain() {
    let mut ctx = make_ctx();
    // Under the test binary process.argv[1] is the executable path, i.e. the
    // plain-run (non-runner) form: registration must be ACCEPTED and the
    // one-shot exit-drain armed (no throw — node parity).
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  try {
    nt.test('x', function() { globalThis.__drainExec = (globalThis.__drainExec || 0) + 1; });
    nt.it('x-it', function() { globalThis.__drainExec = (globalThis.__drainExec || 0) + 1; });
    nt.describe('suite', function() {
      nt.it('inner', function() { globalThis.__drainExec = (globalThis.__drainExec || 0) + 1; });
    });
    return 'REGISTERED';
  }
  catch (e) { return 'THREW:' + e.message.substring(0, 80); }
})()"#,
    );
    assert_eq!(
        out, "REGISTERED",
        "plain-run registration must be accepted (node run-form contract, not a refusal)"
    );

    // The drain really executes: after the registering eval returns (the
    // "main module finished"), the armed setTimeout(0) fires, the runner
    // executes all three bodies, and the report lands. Execution is the
    // anti-fake proof. exitCode === 0 = an all-pass run did NOT trip the
    // failed>0 → exitCode 1 mapping (bao's exitCode accessor defaults to
    // the int32 0 slot, unlike node's undefined).
    drive_settle(&mut ctx, 200);
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  return JSON.stringify({ exec: globalThis.__drainExec, exitCode: process.exitCode });
})()"#,
    );
    assert_eq!(
        out, r#"{"exec":3,"exitCode":0}"#,
        "run-form drain must actually execute every registered body (exec=3), \
         and an all-pass run must not map anything into a failure exit code"
    );
}

#[test]
fn node_test_under_runner_registers_and_executes() {
    let mut ctx = make_ctx();
    // Simulate `bao test`: the runner gate keys on the CLI subcommand.
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  process.argv[1] = 'test';
  var nt = require('node:test');
  nt.test('passes', function() { globalThis.__execCount = (globalThis.__execCount || 0) + 1; });
  nt.test('fails', function() { throw new Error('boom'); });
  nt.describe('suite', function() {
    nt.it('inner', function() { globalThis.__execCount = (globalThis.__execCount || 0) + 1; });
  });
  return 'REGISTERED';
})()"#,
    );
    assert_eq!(out, "REGISTERED");

    // Drive the real bun:test runner over the registered suites.
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var p = nt_run();
  if (!p || typeof p.then !== 'function') return 'NOT_PROMISE';
  p.then(function(r) { globalThis.__rep = r; },
         function(e) { globalThis.__rep = { error: String(e) }; });
  return 'STARTED';
  function nt_run() { return require('node:test').run(); }
})()"#,
    );
    assert_eq!(out, "STARTED");

    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var r = globalThis.__rep;
  if (!r) return 'NO_REPORT';
  return JSON.stringify({ passed: r.passed, failed: r.failed, exec: globalThis.__execCount });
})()"#,
    );
    // Two passing (test + describe/it) and one failing test body, and both
    // passing bodies actually EXECUTED (execCount proves real execution, not
    // fake registration).
    assert_eq!(out, r#"{"passed":2,"failed":1,"exec":2}"#);
}

#[test]
fn node_test_missing_infrastructure_refuses() {
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  // Simulate a context where the bun:test shim was never installed.
  var saved = globalThis.__bun_test_module;
  delete globalThis.__bun_test_module;
  process.argv[1] = 'test';
  try {
    require('node:test').test('x', function() {});
    return 'NO_THROW';
  } catch (e) {
    return /refusing to fake/.test(e.message) ? 'REFUSED' : 'WRONG:' + e.message.substring(0, 60);
  } finally {
    globalThis.__bun_test_module = saved;
  }
})()"#,
    );
    assert_eq!(out, "REFUSED");
}

// ESM import path — the synthetic `node:test` ESM module must go through the
// same gated CJS bridge as require('node:test'): outside `bao test` the
// test()/describe() calls are ACCEPTED (the run-form drain executes them
// after module evaluation — node parity; the former "fail the module
// evaluation" pin was the pre-E-test-wave refusal, superseded with the
// drain-carrying bridge) and the drain really executes the bodies.

#[test]
fn node_test_esm_import_registers_and_executes_via_drain() {
    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(
        &mut cx,
        "import { test } from 'node:test';\ntest('esm-plain-run', function() { globalThis.__esmDrainExec = (globalThis.__esmDrainExec || 0) + 1; });",
        "<esm-plain-run>.mjs",
        None,
        global.handle(),
    )
    .expect(
        "ESM node:test registration outside the runner must be accepted \
         (node run-form contract: register now, execute after the module)",
    );

    // The run-form drain (armed by the FIRST gated registration through the
    // shared bridge) executes the ESM-registered body once the module eval
    // finished — real execution is the anti-fake proof.
    drive_settle(&mut ctx, 200);
    let out = eval_string(&mut ctx, "String(globalThis.__esmDrainExec === 1)");
    assert_eq!(
        out, "true",
        "the ESM-registered body must have actually executed via the run-form drain"
    );
}

#[test]
fn node_test_esm_import_under_runner_registers_and_executes() {
    let mut ctx = make_ctx();
    // Simulate `bao test`: the runner gate keys on the CLI subcommand.
    ctx.eval("process.argv[1] = 'test'; void 0;", "<runner-sim>")
        .expect("runner sim");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(
        &mut cx,
        "import { test, describe, it } from 'node:test';\n\
         test('esm-passes', function() { globalThis.__esmExec = (globalThis.__esmExec || 0) + 1; });\n\
         describe('suite', function() { it('inner', function() { globalThis.__esmExec = (globalThis.__esmExec || 0) + 1; }); });",
        "<esm-runner>.mjs",
        None,
        global.handle(),
    )
    .expect("ESM import under the runner must register for real");

    // Drive the real bun:test runner over the ESM-registered suites.
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var p = require('node:test').run();
  if (!p || typeof p.then !== 'function') return 'NOT_PROMISE';
  p.then(function(r) { globalThis.__rep = r; },
         function(e) { globalThis.__rep = { error: String(e) }; });
  return 'STARTED';
})()"#,
    );
    assert_eq!(out, "STARTED");

    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var r = globalThis.__rep;
  if (!r) return 'NO_REPORT';
  return JSON.stringify({ passed: r.passed, failed: r.failed, exec: globalThis.__esmExec });
})()"#,
    );
    // Both registered bodies actually EXECUTED (exec proves real execution
    // through the ESM bridge, not fake registration).
    assert_eq!(out, r#"{"passed":2,"failed":0,"exec":2}"#);
}

// bao test runner driving — plain-script (CJS) files and `-e` evals must
// execute the suites they registered, not return a vacuous 0/0 report.

#[test]
fn run_test_file_cjs_drives_registered_suites() {
    let path = std::env::temp_dir().join(format!(
        "bao-nodetest-gate-{}-{}.cjs",
        std::process::id(),
        line!()
    ));
    std::fs::write(
        &path,
        r#"
process.argv[1] = 'test';
var nt = require('node:test');
nt.test('cjs-passes', function() { globalThis.__cjsExec = (globalThis.__cjsExec || 0) + 1; });
nt.test('cjs-fails', function() { throw new Error('boom-cjs'); });
"#,
    )
    .expect("write temp CJS test file");

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let report = rt
        .run_test_file(path.to_str().unwrap())
        .expect("run_test_file must succeed");
    let _ = std::fs::remove_file(&path);

    assert_eq!(report.passed, 1, "CJS-registered passing suite must run");
    assert_eq!(report.failed, 1, "CJS-registered failing suite must run");
    let r = rt
        .eval("String(globalThis.__cjsExec)", "<verify>")
        .expect("verify eval");
    assert_eq!(
        r.as_string(),
        Some("1"),
        "the passing CJS test body must have actually executed"
    );
}

#[test]
fn runner_eval_then_run_registered_tests_executes() {
    // Same primitives as `bao test -e` (cli.rs): eval the code, then drive
    // the registered suites via NodeRuntime::run_registered_tests.
    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    rt.eval(
        r#"
process.argv[1] = 'test';
var nt = require('node:test');
nt.test('eval-passes', function() { globalThis.__evalExec = (globalThis.__evalExec || 0) + 1; });
nt.test('eval-fails', function() { throw new Error('boom-eval'); });
"#,
        "<test-eval>",
    )
    .expect("eval must succeed");

    let report = rt.run_registered_tests();
    assert_eq!(report.passed, 1, "-e-registered passing suite must run");
    assert_eq!(report.failed, 1, "-e-registered failing suite must run");
    let r = rt
        .eval("String(globalThis.__evalExec)", "<verify>")
        .expect("verify eval");
    assert_eq!(
        r.as_string(),
        Some("1"),
        "the passing -e test body must have actually executed"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 2. readline question() — sync + promises read real stdin
// ─────────────────────────────────────────────────────────────────────────

/// RAII swap of fd 0 onto a pipe pre-filled with `input` (write end closed so
/// reads hit EOF after the data). Restores the original stdin on drop.
struct StdinSwap {
    saved: i32,
}

impl StdinSwap {
    fn new(input: &[u8]) -> Option<Self> {
        unsafe {
            let mut fds = [0i32; 2];
            #[cfg(unix)]
            let piped = unsafe { libc::pipe(fds.as_mut_ptr()) };
            // windows libc: `pipe(fds, bufsize, mode)` — _O_BINARY avoids CRLF.
            #[cfg(windows)]
            let piped = unsafe { libc::pipe(fds.as_mut_ptr(), 4096, 0x8000) };
            if piped != 0 {
                return None;
            }
            let mut written = 0usize;
            while written < input.len() {
                let n = libc::write(
                    fds[1],
                    input[written..].as_ptr() as *const libc::c_void,
                    (input.len() - written).try_into().unwrap(),
                );
                if n <= 0 {
                    libc::close(fds[0]);
                    libc::close(fds[1]);
                    return None;
                }
                written += n as usize;
            }
            libc::close(fds[1]); // EOF sentinel after the data
            let saved = libc::dup(0);
            if saved < 0 || libc::dup2(fds[0], 0) != 0 {
                libc::close(fds[0]);
                if saved >= 0 {
                    libc::close(saved);
                }
                return None;
            }
            libc::close(fds[0]);
            Some(StdinSwap { saved })
        }
    }
}

impl Drop for StdinSwap {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved, 0);
            libc::close(self.saved);
        }
    }
}

/// fd 0 is process-global: parallel tests swapping stdin would steal each
/// other's piped lines (and interleave dup2/restore). Hold this lock for the
/// entire lifetime of a StdinSwap.
static STDIN_SWAP_LOCK: Mutex<()> = Mutex::new(());

fn lock_stdin() -> std::sync::MutexGuard<'static, ()> {
    // A poisoned lock only means a prior stdin test panicked mid-swap; the
    // fd itself carries no inconsistent state worth failing over.
    STDIN_SWAP_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn readline_sync_question_reads_real_stdin_line() {
    let _stdin = lock_stdin();
    let guard = StdinSwap::new(b"hello\n");
    assert!(guard.is_some(), "pipe stdin swap failed");
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var iface = require('readline').createInterface({ input: process.stdin });
  var answer = '__PENDING__';
  iface.question('Name: ', function(a) { answer = a; });
  return answer;
})()"#,
    );
    assert_eq!(out, "hello", "sync question() must deliver the real line");
}

#[test]
fn readline_promises_question_resolves_with_real_line_and_rejects_on_eof() {
    let _stdin = lock_stdin();
    let guard = StdinSwap::new(b"world\n");
    assert!(guard.is_some(), "pipe stdin swap failed");
    let mut ctx = make_ctx();

    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var iface = require('readline/promises').createInterface({ input: process.stdin });
  var p = iface.question('City: ');
  if (!p || typeof p.then !== 'function') return 'NOT_PROMISE';
  p.then(function(a) { globalThis.__answer = a; },
         function(e) { globalThis.__perr = (e && e.message) || String(e); });
  return 'PROMISE';
})()"#,
    );
    assert_eq!(out, "PROMISE");

    let out = eval_string(
        &mut ctx,
        r#"(function() { return globalThis.__answer === 'world' ? 'GOT_WORLD' : 'MISSING:' + String(globalThis.__answer) + ':' + String(globalThis.__perr); })()"#,
    );
    assert_eq!(out, "GOT_WORLD");

    // Pipe is now drained — the next question must reject (EOF), not resolve
    // with a fabricated empty answer.
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var iface = require('readline/promises').createInterface({ input: process.stdin });
  iface.question('Again: ').then(function(a) { globalThis.__eofAnswer = a; },
                                function(e) { globalThis.__eofErr = (e && e.message) || String(e); });
  return 'STARTED';
})()"#,
    );
    assert_eq!(out, "STARTED");

    let out = eval_string(
        &mut ctx,
        r#"(function() {
  if (globalThis.__eofErr) return /stdin closed/.test(globalThis.__eofErr) ? 'EOF_REJECTED' : 'WRONG_ERR:' + globalThis.__eofErr;
  return 'RESOLVED_FAKE:' + String(globalThis.__eofAnswer);
})()"#,
    );
    assert_eq!(out, "EOF_REJECTED");
}

#[test]
fn readline_sync_question_eof_throws() {
    // Empty pipe with the write end already closed: immediate EOF.
    let _stdin = lock_stdin();
    let guard = StdinSwap::new(b"");
    assert!(guard.is_some(), "pipe stdin swap failed");
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var iface = require('readline').createInterface({});
  try { iface.question('Q: ', function() {}); return 'NO_THROW'; }
  catch (e) { return /stdin closed/.test(e.message) ? 'EOF_THREW' : 'WRONG:' + e.message.substring(0, 60); }
})()"#,
    );
    assert_eq!(out, "EOF_THREW");
}

// ─────────────────────────────────────────────────────────────────────────
// 3. _http_client — phantom ClientRequest refuses
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn internal_http_client_request_refuses_instead_of_dropping_body() {
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var hc = require('_http_client');
  var symbols = typeof hc.kBodyChunks === 'symbol' && typeof hc.abortedSymbol === 'symbol';
  try { new hc.ClientRequest({ host: 'example.com' }); return 'NO_THROW:' + symbols; }
  catch (e) { return /http/.test(e.message) ? 'THREW:' + symbols : 'WRONG:' + e.message.substring(0, 60); }
})()"#,
    );
    assert_eq!(
        out, "THREW:true",
        "ClientRequest must throw explicitly (and keep symbol exports)"
    );
}

// http module's named typeof-surface classes — same disposition as
// _http_client: constructing them would hand the caller an object whose
// write()/end()/on() silently drop everything, so construction refuses;
// `typeof` stays 'function' for conformance checks.

#[test]
fn http_named_classes_refuse_construction_with_function_typeof() {
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var h = require('http');
  var typeofOk = typeof h.ClientRequest === 'function'
    && typeof h.IncomingMessage === 'function'
    && typeof h.OutgoingMessage === 'function';
  var results = [];
  ['ClientRequest', 'IncomingMessage', 'OutgoingMessage'].forEach(function(name) {
    try { new h[name]({ host: 'example.com' }); results.push(name + ':NO_THROW'); }
    catch (e) { results.push(/require\('http'\)\.request/.test(e.message) ? name + ':THREW' : name + ':WRONG:' + e.message.substring(0, 40)); }
  });
  return (typeofOk ? 'FN:' : 'NOT_FN:') + results.join(',');
})()"#,
    );
    assert_eq!(
        out, "FN:ClientRequest:THREW,IncomingMessage:THREW,OutgoingMessage:THREW",
        "all three named classes: typeof stays function, construction throws an http.request hint"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 4. worker_threads — real BroadcastChannel fan-out; MessagePort refuses
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn broadcast_channel_real_fanout() {
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var wt = require('worker_threads');
  var a = new wt.BroadcastChannel('ch1');
  var b = new wt.BroadcastChannel('ch1');
  var c = new wt.BroadcastChannel('ch2');
  var gotA = null, gotB = null, gotC = null;
  a.onmessage = function(ev) { gotA = ev.data; };
  b.onmessage = function(ev) { gotB = ev.data; };
  c.onmessage = function(ev) { gotC = ev.data; };

  a.postMessage({ hello: 'world' });
  var fanout = JSON.stringify({ gotB: gotB, gotA: gotA, gotC: gotC });

  // addEventListener path
  var listenerHits = 0;
  b.addEventListener('message', function() { listenerHits++; });
  a.postMessage('second');
  var listener = (listenerHits === 1 && gotB === 'second');

  // close() stops delivery and postMessage on a closed channel throws
  b.close();
  var closedThrow = null;
  try { b.postMessage('x'); } catch (e) { closedThrow = 'threw'; }
  gotB = null;
  a.postMessage('third');
  var afterClose = (gotB === null);

  return JSON.stringify({ fanout: fanout, listener: listener, closedThrow: closedThrow, afterClose: afterClose });
})()"#,
    );
    assert_eq!(
        out,
        r#"{"fanout":"{\"gotB\":{\"hello\":\"world\"},\"gotA\":null,\"gotC\":null}","listener":true,"closedThrow":"threw","afterClose":true}"#,
        "BroadcastChannel: peer receives, sender does not self-receive, other channel isolated, close stops delivery"
    );
}

#[test]
fn message_port_bare_construction_refuses() {
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var wt = require('worker_threads');
  var isFn = typeof wt.MessagePort === 'function';
  try { new wt.MessagePort(); return 'NO_THROW:' + isFn; }
  catch (e) { return /MessageChannel|Worker/.test(e.message) ? 'THREW:' + isFn : 'WRONG:' + e.message.substring(0, 60); }
})()"#,
    );
    assert_eq!(
        out, "THREW:true",
        "bare MessagePort construction must throw (typeof stays function)"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 5. wasi fd_seek — SEEK_END fails closed with ENOSYS
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn wasi_fd_seek_seek_end_returns_enosys() {
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var wasi = require('wasi');
  var w = new wasi.WASI({ preopens: { '/tmp': '/tmp' } });
  w._memory = new WebAssembly.Memory({ initial: 1 });
  var imp = w.wasiSnapshotPreview1();
  var seekSet = imp.fd_seek(3, 10, 0, 0);  // SEEK_SET works
  var seekCur = imp.fd_seek(3, 5, 1, 0);   // SEEK_CUR works
  var seekEnd = imp.fd_seek(3, 0, 2, 0);   // SEEK_END → ENOSYS (52)
  var seekBad = imp.fd_seek(3, 0, 9, 0);   // unknown whence → ENOSYS (52)
  return seekSet + ',' + seekCur + ',' + seekEnd + ',' + seekBad;
})()"#,
    );
    assert_eq!(
        out, "0,0,52,52",
        "SEEK_SET/SEEK_CUR stay 0; SEEK_END and unknown whence must return ENOSYS"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 6. DOMParser — CLI mode refuses instead of returning a pseudo-DOM
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn domparser_cli_mode_refuses_pseudo_dom() {
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  try { new DOMParser().parseFromString('<html><body><p>hi</p></body></html>', 'text/html'); return 'NO_THROW'; }
  catch (e) { return /browser context/.test(e.message) ? 'THREW' : 'WRONG:' + e.message.substring(0, 60); }
})()"#,
    );
    assert_eq!(out, "THREW", "CLI DOMParser must throw a browser-context error");
}
