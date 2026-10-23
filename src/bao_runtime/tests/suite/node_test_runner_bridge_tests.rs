// @trace REQ-ENG-048 [level:integration]
//
// node:test runner bridge — callable-module contract regression tests.
//
// Contract under test (node v24 oracle anchored):
//   1. `require('node:test')` returns a CALLABLE function whose default entry
//      registers a real test (`typeof nt === 'function'`, `nt.test === nt`),
//      not a plain object — the plain-object bridge made the canonical
//      `const test = require('node:test'); test('name', fn)` form throw
//      "test is not a function" and official suite files reported vacuous
//      0/0 summaries.
//   2. `suite` is exported as the describe alias (`nt.suite === nt.describe`),
//      `describe`/`it`/hooks/mock/assert/run all present as named surface.
//   3. The node `(name, options, fn)` signature routes to the real callback —
//      a raw passthrough registered the options object AS the test body and
//      dropped `fn`, failing every 3-arg call site at run time.
//   4. Options normalization: `{ skip: true }` / `{ todo: true }` register
//      nothing; the real fn of a 3-arg non-skipped call still executes.
//   5. Exit-code semantics stay with the runner: registered-but-failing tests
//      produce failed>0 in the report (the CLI maps that to exit 1).
//   6. TestContext (node v24 oracle 2026-10-20): every body receives a
//      context with t.test/t.diagnostic/t.todo/t.skip/t.plan + name/fullName;
//      t.test awaits couple the parent to the subtest and every subtest
//      outcome lands in the report (no silent drop).
//   7. Module exports snapshot/expectFailure/getTestContext/only/skip/todo
//      exist with node's shapes (snapshot = config object, expectFailure =
//      expected-failure registration, top-level variant aliases are identity
//      aliases of the test variants).
//   8. execFile callbacks complete and count (the never-completing-callback
//      face made esm-cjs files report 0/0 with exit 0).
//   9. Honest counting: a hung test body counts as a FAILURE, not a vacuous
//      0/0 pass; process 'exit' listeners observe the COMPLETED run (node
//      ordering), not the pre-suite drain.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<node-test-bridge>") {
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

fn runner_sim(ctx: &mut JsContext) {
    // Simulate `bao test`: the runner gate keys on the CLI subcommand.
    eval_string(ctx, "process.argv[1] = 'test'; void 0;");
}

/// Run a registered report to completion via the same drive the runner uses
/// (`nt.run()` → report promise → `globalThis.__nt_rep`), and return the
/// JSON summary string the tests assert on.
fn drive_run(ctx: &mut JsContext) {
    let out = eval_string(
        ctx,
        r#"(function() {
  var p = require('node:test').run();
  if (!p || typeof p.then !== 'function') return 'NOT_PROMISE';
  p.then(function(r) { globalThis.__nt_rep = r; },
         function(e) { globalThis.__nt_rep = { error: String(e) }; });
  return 'STARTED';
})()"#,
    );
    assert_eq!(out, "STARTED");
}

fn rep_json(ctx: &mut JsContext) -> String {
    eval_string(
        ctx,
        r#"(function() {
  var r = globalThis.__nt_rep;
  if (!r) return 'NO_REPORT';
  if (r.error) return 'REPORT_ERR:' + r.error;
  return JSON.stringify({ passed: r.passed, failed: r.failed,
                          names: (r.passes || []).concat((r.failures || []).map(function(f) { return f.name; })).sort() });
})()"#,
    )
}

// ─── TestContext (item ①) ──────────────────────────────────────────────────

#[test]
fn node_test_testcontext_injected_into_body() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  globalThis.__ctxSeen = null;
  nt('ctx-shape', function(t) {
    globalThis.__ctxSeen = {
      typeof: typeof t,
      ownKeys: Object.keys(t).length,
      name: t.name,
      fullName: t.fullName,
      test: typeof t.test,
      diagnostic: typeof t.diagnostic,
      todo: typeof t.todo,
      skip: typeof t.skip,
      plan: typeof t.plan,
      mock: typeof t.mock,
      assert: typeof t.assert
    };
    // t.diagnostic is the TAP ℹ face — must not throw under console.log.
    t.diagnostic('a diagnostic line');
    return undefined;
  });
  return 'REG';
})()"#,
    );
    // Bodies run when the runner runs (node semantics: registration defers;
    // the v24 oracle observes the context INSIDE the body). Drive the same
    // run the runner uses, then read the shape the body captured.
    drive_run(&mut ctx);
    let out = eval_string(&mut ctx, "JSON.stringify(globalThis.__ctxSeen)");
    assert_eq!(
        out,
        r#"{"typeof":"object","ownKeys":0,"name":"ctx-shape","fullName":"ctx-shape","test":"function","diagnostic":"function","todo":"function","skip":"function","plan":"function","mock":"object","assert":"object"}"#
    );
}

#[test]
fn node_test_t_test_subtest_counts_into_report_with_parent_coupling() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    // node v24 oracle (2026-10-20): [parent-with-failing-subtest, outer-pass]
    // → tests 3, pass 1, fail 2; fullName "outer > inner"; the awaited
    // subtest's failure fails the parent.
    eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  nt('parent', function(t) {
    return t.test('inner', function(st) {
      if (st.name !== 'inner' || st.fullName !== 'parent > inner') {
        throw new Error('bad subtest context names: ' + st.name + ' / ' + st.fullName);
      }
      throw new Error('inner boom');
    });
  });
  nt('outer-pass', function() {});
  return 'REG';
})()"#,
    );
    drive_run(&mut ctx);
    // node v24 oracle 2026-10-20: pass 1 (outer-pass), fail 2 (the awaited
    // subtest's original error couples to the parent + the subtest itself);
    // the subtest row displays under its chained fullName.
    // Key order = insertion order (ECMA-262 JSON.stringify ordinary own-key
    // order; node v24.19.0 oracle 2026-10-23:
    // JSON.stringify({passed:1,failed:0,names:[]}) → {"passed":1,...}).
    assert_eq!(
        rep_json(&mut ctx),
        r#"{"passed":1,"failed":2,"names":["outer-pass","parent","parent > inner"]}"#
    );
}

#[test]
fn node_test_t_todo_runs_and_never_fails_run() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    // node oracle: a throwing todo body is ✔ (todo bucket, run never fails);
    // a passing todo body is ✔ too.
    eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  nt('todos', function(t) {
    t.todo('t-fail', function() { throw new Error('todo boom'); });
    return t.todo('t-pass', function() {});
  });
  return 'REG';
})()"#,
    );
    drive_run(&mut ctx);
    // Todo rows display under their chained fullName ("todos > t-fail") and
    // count as passes — the todo body's throw never fails the run.
    // Key order = insertion order (node v24.19.0 JSON.stringify oracle,
    // 2026-10-23: object own keys serialize in insertion order).
    assert_eq!(
        rep_json(&mut ctx),
        r#"{"passed":3,"failed":0,"names":["todos","todos > t-fail","todos > t-pass"]}"#
    );
}

#[test]
fn node_test_gettestcontext_inside_body_and_refused_outside() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  globalThis.__gtc_inside = null;
  nt('gtc', function() {
    var t = nt.getTestContext();
    globalThis.__gtc_inside = (t && t.name === 'gtc') ? 'INSIDE-OK' : 'INSIDE-BAD';
  });
  return 'REG';
})()"#,
    );
    // The body observes its context during the run (node v24 oracle:
    // getTestContext() is meaningful INSIDE a running body); the outside
    // probe must run after the run drained the context stack.
    drive_run(&mut ctx);
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  var inside = globalThis.__gtc_inside;
  try {
    nt.getTestContext();
    return inside + '|OUTSIDE-NO-THROW';
  } catch (e) {
    return inside + '|OUTSIDE-THREW';
  }
})()"#,
    );
    assert_eq!(out, "INSIDE-OK|OUTSIDE-THREW");
}

// ─── snapshot / expectFailure / aliases (item ②) ──────────────────────────

#[test]
fn node_test_snapshot_object_surface() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  var s = nt.snapshot;
  if (!s || typeof s !== 'object') return 'SNAPSHOT:' + typeof s;
  var keys = Object.keys(s).sort().join(',');
  if (keys !== 'setDefaultSnapshotSerializers,setResolveSnapshotPath') return 'KEYS:' + keys;
  if (typeof s.setDefaultSnapshotSerializers !== 'function') return 'S1';
  if (typeof s.setResolveSnapshotPath !== 'function') return 'S2';
  // node validates argument types with ERR_INVALID_ARG_TYPE shapes.
  try { s.setDefaultSnapshotSerializers('nope'); return 'NO-VALIDATE-SER'; }
  catch (e1) {
    if (String(e1.message).indexOf('serializers') < 0) return 'SER-MSG:' + e1.message;
    try { s.setResolveSnapshotPath(123); return 'NO-VALIDATE-RES'; }
    catch (e2) {
      if (String(e2.message).indexOf('fn') < 0) return 'RES-MSG:' + e2.message;
      // Valid configuration is accepted (node stores it; bao stores it inert
      // — the snapshot materialization itself is not implemented).
      s.setDefaultSnapshotSerializers([String]);
      s.setResolveSnapshotPath(function() { return 'x'; });
      return 'SNAPSHOT-OK';
    }
  }
})()"#,
    );
    assert_eq!(out, "SNAPSHOT-OK");
}

#[test]
fn node_test_expect_failure_registration_semantics() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    // node oracle: a throwing body is the PASS path; a clean run is a
    // FAILURE ('test was expected to fail but passed').
    eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  nt.expectFailure('expected-throws', function() { throw new Error('planned boom'); });
  nt.expectFailure('unexpectedly-passes', function() {});
  return 'REG';
})()"#,
    );
    drive_run(&mut ctx);
    let rep = eval_string(
        &mut ctx,
        r#"(function() {
  var r = globalThis.__nt_rep;
  var msg = '';
  (r.failures || []).forEach(function(f) {
    if (f.name === 'unexpectedly-passes') msg = f.message;
  });
  return JSON.stringify({ passed: r.passed, failed: r.failed, msg: msg });
})()"#,
    );
    assert_eq!(
        rep,
        r#"{"passed":1,"failed":1,"msg":"test was expected to fail but passed"}"#
    );
    // Key order = insertion order (node v24.19.0 JSON.stringify oracle,
    // 2026-10-23); content = node expectFailure semantics (throwing body is
    // the pass path, clean run fails with the "expected to fail" message).
}

#[test]
fn node_test_top_level_variant_aliases() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  if (nt.only !== nt.test.only) return 'ONLY';
  if (nt.skip !== nt.test.skip) return 'SKIP';
  if (nt.todo !== nt.test.todo) return 'TODO';
  return 'ALIASES-OK';
})()"#,
    );
    assert_eq!(out, "ALIASES-OK");
}

// ─── honest counting (item ⑤) — NodeRuntime end-to-end ────────────────────

fn scratch_dir(tag: &str) -> ::std::path::PathBuf {
    let dir = ::std::env::temp_dir().join(format!(
        "bao-nt-bridge-{}-{}",
        tag,
        ::std::process::id()
    ));
    let _ = ::std::fs::remove_dir_all(&dir);
    ::std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn write_file(dir: &::std::path::Path, name: &str, content: &str) -> ::std::path::PathBuf {
    let p = dir.join(name);
    ::std::fs::write(&p, content).expect("write");
    p
}

fn node_runtime_with_test_argv() -> bun_runtime::NodeRuntime {
    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    // The runner gate keys on process.argv[1] === 'test' — set it the same
    // way the CLI subcommand does before the file evaluates.
    rt.eval("process.argv[1] = 'test'; void 0;", "<argv-sim>")
        .expect("argv sim");
    rt
}

/// The ③⑤ flagship: a test file whose body drives `execFile` through the
/// async-callback face. The former face ignored the callback entirely — the
/// promise never settled, the report never arrived, and the vacuous 0/0 pass
/// exited 0. Both the success shape (decoded string with trailing newline)
/// and the failure shape (real Error, "Command failed: <file>\n<stderr>",
/// numeric code) are node-oracle-pinned.
#[test]
fn node_test_execfile_callback_completes_and_counts() {
    let dir = scratch_dir("execfile");
    let src = r#"
const nt = require('node:test');
const cp = require('child_process');
var isWin = process.platform === 'win32';
nt('execfile-callback-completes', function() {
  return new Promise(function(resolve, reject) {
    var call = isWin ? ['cmd.exe', ['/C', 'echo', 'hello']] : ['echo', ['hello']];
    cp.execFile(call[0], call[1], function(err, stdout, stderr) {
      if (err) { reject(err); return; }
      var want = isWin ? 'hello\r\n' : 'hello\n';
      if (stdout !== want) { reject(new Error('bad stdout: ' + JSON.stringify(stdout))); return; }
      resolve();
    });
  });
});
nt('execfile-failure-error-shape', function() {
  return new Promise(function(resolve, reject) {
    var call = isWin ? ['cmd.exe', ['/C', 'exit', '7']] : ['false', []];
    cp.execFile(call[0], call[1], function(err, stdout, stderr) {
      if (!err) { reject(new Error('expected an error')); return; }
      if (!(err instanceof Error)) { reject(new Error('not an Error: ' + typeof err)); return; }
      if (String(err.message).indexOf('Command failed: ' + call[0]) !== 0) {
        reject(new Error('bad message: ' + err.message)); return;
      }
      var wantCode = isWin ? 7 : 1;
      if (err.code !== wantCode) { reject(new Error('bad code: ' + JSON.stringify(err.code))); return; }
      if (err.killed !== false) { reject(new Error('bad killed: ' + err.killed)); return; }
      if (err.cmd !== call[0]) { reject(new Error('bad cmd: ' + err.cmd)); return; }
      resolve();
    });
  });
});
"#;
    let path = write_file(&dir, "execfile_test.js", src);
    let mut rt = node_runtime_with_test_argv();
    let report = rt.run_test_file(path.to_str().unwrap()).expect("run");
    assert_eq!(report.passed, 2, "passes: {:?}", report.passes);
    assert_eq!(report.failed, 0, "failures: {:?}", report.failures);
}

/// The vacuous 0/0 fake-pass eradication pin: a test body whose promise never
/// settles used to leave the report missing → `TestReport::default()` →
/// 0 passed / 0 failed → exit 0. The report guard counts it as a FAILURE.
#[test]
fn node_test_hung_body_counts_as_failure_not_vacuous_pass() {
    let dir = scratch_dir("hung");
    let path = write_file(
        &dir,
        "hung_test.js",
        "const nt = require('node:test');\nnt('never-settles', function() { return new Promise(function() {}); });\n",
    );
    let mut rt = node_runtime_with_test_argv();
    let report = rt.run_test_file(path.to_str().unwrap()).expect("run");
    assert_eq!(report.passed, 0);
    assert_eq!(report.failed, 1, "failures: {:?}", report.failures);
    assert_eq!(report.failures[0].name, "run_bun_tests");
}

/// process 'exit' ordering (item ④), CJS arm: the listener must observe the
/// COMPLETED run — 'eval;test;exit;' — the former wiring fired 'exit' during
/// the pre-suite drain, before any test body executed ('eval;exit;test;').
#[test]
fn node_test_exit_listener_fires_after_suites_cjs() {
    let dir = scratch_dir("exit-order-cjs");
    let out = dir.join("out.txt");
    let out_lit = out.to_string_lossy().replace('\\', "\\\\");
    let src = format!(
        "const fs = require('fs');\nconst nt = require('node:test');\nfs.writeFileSync('{o}', 'eval;');\nprocess.on('exit', function() {{ fs.appendFileSync('{o}', 'exit;'); }});\nnt('t1', function() {{ fs.appendFileSync('{o}', 'test;'); }});\n",
        o = out_lit
    );
    let path = write_file(&dir, "exit_order_test.js", &src);
    let mut rt = node_runtime_with_test_argv();
    let report = rt.run_test_file(path.to_str().unwrap()).expect("run");
    assert_eq!(report.failed, 0, "failures: {:?}", report.failures);
    let observed = ::std::fs::read_to_string(&out).expect("read out");
    assert_eq!(observed, "eval;test;exit;");
}

/// process 'exit' ordering (item ④), ESM arm: same ordering through the
/// module loader path (drain-only pre-suite pump, 'exit' after the report).
#[test]
fn node_test_exit_listener_fires_after_suites_esm() {
    let dir = scratch_dir("exit-order-esm");
    let out = dir.join("out.txt");
    let out_lit = out.to_string_lossy().replace('\\', "\\\\");
    let src = format!(
        "import fs from 'node:fs';\nimport test from 'node:test';\nfs.writeFileSync('{o}', 'eval;');\nprocess.on('exit', function() {{ fs.appendFileSync('{o}', 'exit;'); }});\ntest('t1', function() {{ fs.appendFileSync('{o}', 'test;'); }});\n",
        o = out_lit
    );
    let path = write_file(&dir, "exit_order_test.mjs", &src);
    let mut rt = node_runtime_with_test_argv();
    let report = rt.run_test_file(path.to_str().unwrap()).expect("run");
    assert_eq!(report.failed, 0, "failures: {:?}", report.failures);
    let observed = ::std::fs::read_to_string(&out).expect("read out");
    assert_eq!(observed, "eval;test;exit;");
}

#[test]
fn node_test_module_is_callable_function() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  if (typeof nt !== 'function') return 'TYPE:' + typeof nt;
  if (nt.test !== nt) return 'NOT-SELF';
  // Default call registers a real deferred test.
  nt('via-module-call', function() { globalThis.__execMod = (globalThis.__execMod || 0) + 1; });
  return 'CALLABLE';
})()"#,
    );
    assert_eq!(out, "CALLABLE");
}

#[test]
fn node_test_named_surface_matches_node_oracle() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  var missing = [];
  // node v24.19.0 oracle (2026-10-23, local binary): mock AND assert are
  // plain OBJECTS on the module (typeof nt.assert === 'object'; it is not
  // the raw assert module — nt.assert !== require('assert')); every other
  // named entry is a function.
  ['test', 'describe', 'it', 'suite', 'before', 'after', 'beforeEach', 'afterEach', 'mock', 'assert', 'run'].forEach(function(k) {
    if (typeof nt[k] !== 'function' && k !== 'mock' && k !== 'assert') missing.push(k + ':' + typeof nt[k]);
  });
  if (typeof nt.mock !== 'object') missing.push('mock:' + typeof nt.mock);
  if (typeof nt.assert !== 'object') missing.push('assert:' + typeof nt.assert);
  if (nt.suite !== nt.describe) missing.push('suite!==describe');
  // Variant sub-methods exist on the primary entries.
  ['skip', 'todo', 'only'].forEach(function(v) {
    if (typeof nt.test[v] !== 'function') missing.push('test.' + v);
    if (typeof nt.describe[v] !== 'function') missing.push('describe.' + v);
    if (typeof nt.it[v] !== 'function') missing.push('it.' + v);
  });
  return missing.length === 0 ? 'SURFACE-OK' : 'MISSING:' + missing.join(',');
})()"#,
    );
    assert_eq!(out, "SURFACE-OK");
}

#[test]
fn node_test_three_arg_options_form_registers_real_fn() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  // Execution flags are booleans — the report check below reads `=== true`
  // ("the real body executed"), so the bodies stamp true, not a counter.
  nt('opts-pass', { concurrency: 1 }, function() { globalThis.__exec3 = true; });
  nt('opts-skip', { skip: true }, function() { globalThis.__execSkip = true; });
  nt('opts-todo', { todo: true }, function() { globalThis.__execTodo = true; });
  nt.describe('d3', { concurrency: 2 }, function() {
    nt.it('i3', function() { globalThis.__execD3 = true; });
  });
  return 'REG';
})()"#,
    );

    // Drive the real bun:test runner over the registered suites.
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
  if (r.error) return 'REPORT_ERR:' + r.error;
  return JSON.stringify({
    passed: r.passed,
    failed: r.failed,
    exec3: globalThis.__exec3 === true,
    execD3: globalThis.__execD3 === true,
    skippedNotRun: globalThis.__execSkip !== true && globalThis.__execTodo !== true
  });
})()"#,
    );
    // The two real bodies executed; the skipped/todo bodies never ran.
    assert_eq!(
        out,
        r#"{"passed":2,"failed":0,"exec3":true,"execD3":true,"skippedNotRun":true}"#
    );
}

#[test]
fn node_test_suite_alias_registers_describe_suite() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  // Canonical golden-suite form: `const { test, suite } = require('node:test')`.
  // The flag is a boolean — the report check reads `=== true`.
  nt.suite('suite-alias', function() {
    nt.it('inner', function() { globalThis.__execSuite = true; });
  });
  return 'REG';
})()"#,
    );

    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var p = require('node:test').run();
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
  return JSON.stringify({ passed: r.passed, exec: globalThis.__execSuite === true });
})()"#,
    );
    assert_eq!(out, r#"{"passed":1,"exec":true}"#);
}

#[test]
fn node_test_plain_run_still_refuses() {
    let mut ctx = make_ctx();
    // argv[1] is NOT 'test' (plain run / library embedding): registration
    // must fail closed, callable or not.
    let out = eval_string(
        &mut ctx,
        r#"(function() {
  process.argv[1] = '/some/script.js';
  try {
    var nt = require('node:test');
    nt('nope', function() {});
    return 'REGISTERED-FALSELY';
  } catch (e) {
    return e.message.indexOf('bao test') >= 0 ? 'REFUSED' : 'WRONG-ERR:' + e.message;
  }
})()"#,
    );
    assert_eq!(out, "REFUSED");
}

#[test]
fn node_test_run_report_carries_failure_exit_semantics() {
    let mut ctx = make_ctx();
    runner_sim(&mut ctx);
    eval_string(
        &mut ctx,
        r#"(function() {
  var nt = require('node:test');
  nt('ok', function() {});
  nt('bad', function() { throw new Error('bridge-boom'); });
  return 'REG';
})()"#,
    );

    let out = eval_string(
        &mut ctx,
        r#"(function() {
  var p = require('node:test').run();
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
  // failed>0 is what the CLI layer maps to exit code 1 (node parity).
  return JSON.stringify({ passed: r.passed, failed: r.failed,
                          failName: r.failures && r.failures[0] && r.failures[0].name });
})()"#,
    );
    assert_eq!(
        out,
        r#"{"passed":1,"failed":1,"failName":"bad"}"#
    );
}
