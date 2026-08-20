// @trace TEST-ENG-007 [req:REQ-ENG-006 REQ-ENG-007] [level:integration]
//
// Audit wave: CSV 杂项域补齐 — 7 silent-fake fixes + 8 missing additions
// (util/module/global/async_hooks/perf_hooks/vm/v8/inspector/crypto/zlib/
// bun:test). Each test drives the REAL path and asserts observable behaviour
// (no typeof-only checks).

use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

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

/// Drive the JS thread's event loop for up to `max_iters` iterations so timer
/// callbacks (and the microtasks they schedule) run. Same primitive the
/// existing harness tests use (bun_p0_face_tests::pump → drain_and_check).
fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
    for _ in 0..max_iters {
        let mut cxm = ctx.cx();
        bun_runtime::timers::drain_and_check(&mut cxm);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// ══════════════════════════════════════════════════════════════════════════
// Item 1 — util.callbackify (was: identity passthrough, callback never ran)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_util_callbackify_real_semantics() {
    let mut ctx = setup_ctx();

    // Resolved value → cb(null, value); array result spreads; rejection →
    // cb(err); falsy rejection wrapped in Error with .cause; missing callback
    // throws TypeError synchronously.
    let setup = r#"
      var util = require('util');
      globalThis.__cb = {};
      util.callbackify(async (x) => x * 2)(21, function(err, v) {
        __cb.resolve = err ? 'ERR' : v;
      });
      util.callbackify(async () => [1, 2, 3])(function(err, a, b, c) {
        __cb.spread = err ? 'ERR' : [a, b, c].join(',');
      });
      util.callbackify(async () => { throw new Error('boom'); })(function(err) {
        __cb.reject = err ? err.message : 'NOERR';
      });
      util.callbackify(async () => { throw 0; })(function(err) {
        __cb.falsy = (err instanceof Error) ? (err.message + '|' + String(err.cause === 0)) : 'NOTERR';
      });
      globalThis.__cb.undef = 'unset';
      util.callbackify(async () => {})(function(err) {
        __cb.undef = err ? 'ERR' : (arguments.length === 1 ? 'null-only' : 'extra');
      });
      try {
        util.callbackify(async () => 1)();
        __cb.nocb = 'NO_THROW';
      } catch (e) {
        __cb.nocb = e instanceof TypeError ? 'THROWN' : 'WRONG:' + e;
      }
      'registered';
    "#;
    assert_eq!(eval_string(&mut ctx, setup), "registered");
    // ctx.eval drains the microtask queue, so the callbacks have run now.
    let out = eval_string(
        &mut ctx,
        r#"JSON.stringify([__cb.resolve, __cb.spread, __cb.reject, __cb.falsy, __cb.undef, __cb.nocb])"#,
    );
    assert_eq!(out, r#"[42,"1,2,3","boom","falsy rejection reason|true","null-only","THROWN"]"#);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 2 — crypto.pbkdf2Sync returns a Buffer (was: Array of ints)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_pbkdf2_sync_returns_buffer() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var c = require('crypto');
      var k = c.pbkdf2Sync('password', 'salt', 1, 32, 'sha256');
      JSON.stringify([
        Array.isArray(k),                       // must NOT be an Array
        typeof k.toString === 'function',
        k.toString('hex'),
        k.length
      ]);
    "#,
    );
    // RFC 7914 / scrypt-paper PBKDF2-HMAC-SHA-256 vector for c=1.
    assert_eq!(
        out,
        r#"[false,true,"120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b",32]"#
    );
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 3 — vm sandbox write-through (writes inside vm code land on sandbox)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_sandbox_write_through() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var vm = require('vm');
      var sandbox = { existing: 1 };
      var ctxified = vm.createContext(sandbox);
      // Script.runInContext: implicit global write, var declaration, and
      // mutation of a seeded prop must all be visible on the sandbox object.
      new vm.Script('inner = 42; var declared = 7; existing = 99;').runInContext(ctxified);
      var scriptWrites = [sandbox.inner, sandbox.declared, sandbox.existing].join(',');

      // Props added to the sandbox AFTER createContext are visible inside.
      sandbox.late = 5;
      vm.runInContext('lateDoubled = late * 2', ctxified);
      var lateSeen = sandbox.lateDoubled;

      // Module-level vm.runInContext write-through.
      vm.runInContext('viaFn = 11', ctxified);
      var fnWrites = sandbox.viaFn;

      // vm.runInNewContext reflects vm writes back onto the passed sandbox.
      var sb2 = {};
      vm.runInNewContext('q = 5', sb2);

      JSON.stringify([scriptWrites, lateSeen, fnWrites, sb2.q]);
    "#,
    );
    assert_eq!(out, r#"["42,7,99",10,11,5]"#);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 4 — async_hooks.createHook callbacks actually fire
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_async_hooks_hooks_fire() {
    let mut ctx = setup_ctx();
    // NOTE: timer-CALLBACK firing is not observable under the for_test
    // harness (its event loop never advances macrotasks — pre-existing
    // harness limitation; the real runtime drives them via drain_and_check
    // in the CLI main loop). destroy is therefore observed through the
    // synchronous clearTimeout path, init through construction.
    let setup = r#"
      var ah = require('async_hooks');
      globalThis.__ah = [];
      var hook = ah.createHook({
        init: function(id, type) { __ah.push('init:' + type); },
        destroy: function(id) { __ah.push('destroy'); }
      });
      hook.enable();
      // PROMISE init (patched constructor)…
      var p = new Promise(function(r) { r(1); });
      p.then(function() {});
      // …and TIMER init ('Timeout') + destroy via clearTimeout.
      var t1 = setTimeout(function() {}, 1);
      var t2 = setTimeout(function() {}, 1);
      clearTimeout(t1);
      'ok';
    "#;
    assert_eq!(eval_string(&mut ctx, setup), "ok");

    let log = eval_string(&mut ctx, "globalThis.__ah.join(' ')");
    let parts: Vec<&str> = log.split(' ').collect();
    assert!(
        parts.iter().any(|s| *s == "init:PROMISE"),
        "PROMISE init missing: {}",
        log
    );
    assert!(
        parts.iter().any(|s| *s == "init:Timeout"),
        "Timeout init missing: {}",
        log
    );
    // clearTimeout emits destroy exactly once for the cleared id.
    let destroy_count = parts.iter().filter(|s| **s == "destroy").count();
    assert_eq!(destroy_count, 1, "clearTimeout destroy count wrong: {}", log);
    // The still-pending timer was NOT destroyed.
    assert_eq!(parts.iter().filter(|s| **s == "init:Timeout").count(), 2, "{}", log);

    // disable() stops emission.
    let disable_probe = r#"
      globalThis.__ah2 = [];
      var hook2 = ah.createHook({ init: function(id, type) { __ah2.push(type); } });
      hook2.enable();
      hook2.disable();
      new Promise(function(r) { r(2); });
      'done';
    "#;
    assert_eq!(eval_string(&mut ctx, disable_probe), "done");
    let after = eval_string(&mut ctx, "globalThis.__ah2.length");
    assert_eq!(after, "0", "disable() must stop hook emission");
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 5 — perf_hooks marks/measures really land in getEntries
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_perf_hooks_entries_real() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var ph = require('perf_hooks');
      var perf = ph.performance;
      var m1 = perf.mark('m1');
      var m2 = perf.mark('m2', { startTime: 100 });
      var m3 = perf.mark('m3', { startTime: 250 });
      // measure between two explicit startTimes → deterministic 150.
      var me = perf.measure('span', 'm2', 'm3');
      var entries = perf.getEntries();
      var result = [
        ph.performance === globalThis.performance,   // Node identity
        typeof globalThis.performance.mark,          // global carries mark too
        entries.length,                              // 4 entries buffered
        entries.map(function(e) { return e.entryType; }).join(','),
        m1 instanceof ph.PerformanceMark,
        me instanceof ph.PerformanceMeasure,
        me.startTime,                                // 100 (from m2)
        me.duration,                                 // 150 (250 - 100)
        perf.getEntriesByName('span').length,
        perf.getEntriesByType('measure').length
      ];
      perf.clearMarks('m1');
      result.push(perf.getEntriesByName('m1').length);   // 0 — cleared by name
      result.push(perf.getEntriesByName('m2').length);   // still there
      var timed = ph.timerify(function() { return 42; })();
      result.push(timed);                                // behavior preserved
      result.push(perf.getEntriesByType('function').length);
      JSON.stringify(result);
    "#,
    );
    assert_eq!(
        out,
        r#"[true,"function",4,"mark,mark,mark,measure",true,true,100,150,1,1,0,1,42,1]"#
    );
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 6 — expect(...).resolves / .rejects matcher chaining
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_bun_test_resolves_rejects() {
    let mut ctx = setup_ctx();
    let setup = r#"
      var expect = __bun_test_module.expect;
      globalThis.__ar = {};
      function record(key, p) {
        p.then(function() { __ar[key] = 'PASS'; },
               function(e) { __ar[key] = 'FAIL:' + (e && e.message); });
      }
      record('resolves_toBe', expect(Promise.resolve(7)).resolves.toBe(7));
      record('resolves_toEqual', expect(Promise.resolve({ a: 1 })).resolves.toEqual({ a: 1 }));
      record('resolves_not', expect(Promise.resolve(7)).resolves.not.toBe(8));
      record('resolves_fail', expect(Promise.resolve(7)).resolves.toBe(8));
      record('rejects_toBe', expect(Promise.reject(new Error('nope'))).rejects.toThrow('nope'));
      record('rejects_toEqual', expect(Promise.reject(new TypeError('bad'))).rejects.toEqual(new TypeError('bad')));
      record('rejects_class', expect(Promise.reject(new TypeError('bad'))).rejects.toThrow(TypeError));
      record('rejects_unexpected_resolve', expect(Promise.resolve(1)).rejects.toBeNull());
      record('resolves_propagates_rejection', expect(Promise.reject(new Error('orig'))).resolves.toBe(1));
      // Non-promise input throws synchronously (Jest contract).
      try {
        expect(5).resolves.toBe(5);
        __ar.non_promise = 'NO_THROW';
      } catch (e) {
        __ar.non_promise = 'THROWN';
      }
      'registered';
    "#;
    assert_eq!(eval_string(&mut ctx, setup), "registered");
    let out = eval_string(
        &mut ctx,
        r#"JSON.stringify([__ar.resolves_toBe, __ar.resolves_toEqual, __ar.resolves_not,
             __ar.resolves_fail, __ar.rejects_toBe, __ar.rejects_toEqual, __ar.rejects_class,
             __ar.rejects_unexpected_resolve, __ar.resolves_propagates_rejection, __ar.non_promise])"#,
    );
    assert!(
        out.starts_with(r#"["PASS","PASS","PASS","FAIL:"#),
        "resolves chaining broken: {}",
        out
    );
    assert!(
        out.contains(r#""PASS","PASS","PASS","FAIL:promise resolved unexpectedly"#),
        "rejects chaining broken: {}",
        out
    );
    // Original rejection propagates through .resolves.
    assert!(
        out.contains(r#""FAIL:orig""#),
        "resolves must propagate original rejection: {}",
        out
    );
    assert!(
        out.ends_with(r#","THROWN"]"#),
        "non-promise .resolves must throw synchronously: {}",
        out
    );
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 7 — describe-block hooks really trigger (order audit)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_bun_test_describe_hook_order() {
    let mut ctx = setup_ctx();
    let setup = r#"
      var t = __bun_test_module;
      globalThis.__order = [];
      function log(s) { __order.push(s); }
      t.beforeAll(function() { log('top-beforeAll'); });
      t.it('top-1', function() { log('top-it'); });
      t.describe('outer', function() {
        t.beforeAll(function() { log('outer-beforeAll'); });
        t.beforeEach(function() { log('outer-beforeEach'); });
        t.afterEach(function() { log('outer-afterEach'); });
        t.afterAll(function() { log('outer-afterAll'); });
        t.it('a', function() { log('it-a'); });
        t.describe('inner', function() {
          t.beforeEach(function() { log('inner-beforeEach'); });
          t.it('b', function() { log('it-b'); });
        });
      });
      t.afterAll(function() { log('top-afterAll'); });
      'registered';
    "#;
    assert_eq!(eval_string(&mut ctx, setup), "registered");

    // Run the REAL runner (the same __run_bun_tests the CLI drives). The
    // runner chain is pure microtasks, so ctx.eval's job-queue drain settles
    // it completely; run_bun_tests_report's timer-pump fallback is not usable
    // under the for_test harness (macrotask limitation, see item 4 note).
    let start = r#"
      globalThis.__rep = null;
      globalThis.__run_bun_tests().then(
        function(r) { globalThis.__rep = r; },
        function(e) { globalThis.__rep = { passed: 0, failed: 1 }; }
      );
      'started';
    "#;
    assert_eq!(eval_string(&mut ctx, start), "started");
    let report = eval_string(
        &mut ctx,
        "JSON.stringify(globalThis.__rep && [globalThis.__rep.passed, globalThis.__rep.failed])",
    );
    assert_eq!(report, "[3,0]", "hook-order run must pass 3/0: {} order={}", report, eval_string(&mut ctx, "globalThis.__order.join(',')"));

    let order = eval_string(&mut ctx, "globalThis.__order.join(' ')");
    assert_eq!(
        order,
        "top-beforeAll top-it outer-beforeAll outer-beforeEach it-a outer-afterEach \
         outer-afterAll outer-beforeEach inner-beforeEach it-b outer-afterEach \
         top-afterAll",
        "describe hook order mismatch"
    );
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 8 — util.TextEncoder/TextDecoder re-export (identity with globals)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_util_text_encoder_reexport() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var util = require('util');
      JSON.stringify([
        util.TextEncoder === globalThis.TextEncoder,
        util.TextDecoder === globalThis.TextDecoder,
        new util.TextEncoder().encode('ab').length,
        new util.TextDecoder('utf-8').decode(new Uint8Array([104, 105]))
      ]);
    "#,
    );
    assert_eq!(out, r#"[true,true,2,"hi"]"#);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 9 — require.cache (path → module object, singleton, delete→reload)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_require_cache_singleton() {
    let mut ctx = setup_ctx();
    let dir = tempfile::tempdir().expect("tempdir");
    let mod_path = dir.path().join("counter_mod.js");
    std::fs::write(
        &mod_path,
        "globalThis.__loads = (globalThis.__loads || 0) + 1; module.exports = { n: __loads };",
    )
    .expect("write module");

    let spec = mod_path.to_string_lossy().to_string();
    let setup = format!(
        r#"
      globalThis.__spec = {spec:?};
      var m1 = require(__spec);
      var m2 = require(__spec);
      globalThis.__singleton = (m1 === m2);
      var entry = require.cache[__spec];
      globalThis.__entryOk = !!entry && entry.exports === m1 && entry.loaded === true &&
                             entry.id === __spec && entry.filename === __spec;
      // Builtin modules are not in require.cache (Node semantics).
      globalThis.__builtinLeak = Object.keys(require.cache).some(function(k) {{
        return k.indexOf('builtin:') === 0 || k.indexOf('node:') === 0;
      }});
      // delete forces a reload (Node semantics).
      delete require.cache[__spec];
      var m3 = require(__spec);
      globalThis.__reloaded = m3.n;
      'ok';
    "#,
    );
    assert_eq!(eval_string(&mut ctx, &setup), "ok");
    let out = eval_string(
        &mut ctx,
        "JSON.stringify([__singleton, __entryOk, __builtinLeak, __reloaded])",
    );
    assert_eq!(out, r#"[true,true,false,2]"#);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 10 — `global` alias of globalThis
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_global_alias() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      global.via_alias = 1;
      JSON.stringify([
        global === globalThis,
        Object.prototype.hasOwnProperty.call(globalThis, 'global'),
        globalThis.via_alias
      ]);
    "#,
    );
    assert_eq!(out, r#"[true,true,1]"#);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 11 — crypto.Hash class form
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_crypto_hash_class() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var c = require('crypto');
      var h = new c.Hash('sha256');
      h.update('abc');
      var viaClass = h.digest('hex');
      var viaFactory = c.createHash('sha256').update('abc').digest('hex');
      JSON.stringify([
        typeof c.Hash,
        h instanceof c.Hash,
        viaClass,
        viaClass === viaFactory
      ]);
    "#,
    );
    // sha256("abc") starts with ba7816bf.
    assert!(out.starts_with(r#"["function",true,"ba7816bf"#), "Hash class mismatch: {}", out);
    assert!(out.ends_with(r#",true]"#), "Hash/factory digest mismatch: {}", out);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 12 — zlib.crc32
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_zlib_crc32() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var zlib = require('zlib');
      var hello = zlib.crc32('hello');                    // 0x3610a686
      var cont = zlib.crc32('world', hello);              // == crc32('helloworld')
      var whole = zlib.crc32('helloworld');
      var viaBuf = zlib.crc32(Buffer.from('hello'));
      JSON.stringify([hello, cont === whole, hello === viaBuf, zlib.crc32('')]);
    "#,
    );
    assert_eq!(out, r#"[907060870,true,true,0]"#);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 13 — vm.runInContext / runInThisContext / compileFunction / isContext
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_missing_api_surface() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var vm = require('vm');
      var sb = {};
      var ctxified = vm.createContext(sb);
      vm.runInContext('inCtx = 3 * 3', ctxified);
      var fn = vm.compileFunction('return a + b', ['a', 'b']);
      JSON.stringify([
        typeof vm.runInContext,
        vm.runInContext('40 + 2', ctxified),
        sb.inCtx,
        vm.runInThisContext('6 * 7'),
        fn(3, 4),
        vm.isContext(ctxified),
        vm.isContext({})
      ]);
    "#,
    );
    assert_eq!(out, r#"["function",42,9,42,7,true,false]"#);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 14 — v8.serialize / v8.deserialize (engine structured clone)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_v8_serialize_roundtrip() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var v8 = require('v8');
      var buf = v8.serialize({ a: 1, list: [1, 2, 3], when: new Date(1234) });
      var back = v8.deserialize(buf);
      var m = v8.deserialize(v8.serialize(new Map([[1, 'one']]))).get(1);
      var threw = false;
      try { v8.serialize(function() {}); } catch (e) { threw = true; }
      JSON.stringify([
        typeof buf,            // object
        typeof buf.length,     // number — Buffer/TypedArray shape
        back.a, back.list[2], back.when.getTime(),
        m,
        threw                  // functions are not cloneable
      ]);
    "#,
    );
    assert_eq!(out, r#"["object","number",1,3,1234,"one",true]"#);
    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// Item 15 — inspector.Session (explicit throw, no silent fake)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_inspector_session_explicit_throw() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
      var inspector = require('inspector');
      var s = new inspector.Session();
      var connectMsg = '';
      try { s.connect(); } catch (e) { connectMsg = e.message; }
      var postMsg = '';
      try { s.post('Runtime.evaluate', {}, function() {}); } catch (e) { postMsg = e.message; }
      var disconnectOk = true;
      try { s.disconnect(); } catch (e) { disconnectOk = false; }
      JSON.stringify([
        typeof inspector.Session,
        s instanceof inspector.Session,
        connectMsg.indexOf('not implemented in Bao') !== -1,
        postMsg,
        disconnectOk
      ]);
    "#,
    );
    assert_eq!(out, r#"["function",true,true,"Session is not connected",true]"#);
    bun_runtime::shutdown_thread_sm();
}

