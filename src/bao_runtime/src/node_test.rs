// @trace REQ-ENG-048 [api:node:test] — Node.js test runner module
// @trace REQ-ENG-048 [api:node:test] — official-suite runner bridge surface:
// TestContext (t.test/t.diagnostic/t.todo/t.skip/t.plan + name/fullName/
// filePath), snapshot + expectFailure + getTestContext + only/skip/todo
// module exports, immediate subtest primitive (__run_one_bun_test)
//! node:test module — bridges to the existing `globalThis.__bun_test_module`
//! installed by `bun_test.rs`.
//!
//! Exposes the Node.js `node:test` API surface (test(), describe(), it(),
//! before/after hooks, mock, assert) by delegating to the bun:test
//! infrastructure on `globalThis.__bun_test_module`.
//!
//! ## Silent-fake eradication (group D)
//!
//! Two bugs are closed here:
//!
//! 1. **Install-order freeze**: the previous source resolved
//!    `__bun_test_module` once at module-eval time. `node_test::install`
//!    runs BEFORE `bun_test::install_bun_test` in `globals.rs`, so the
//!    resolution always saw `undefined` and froze an inert
//!    `test: function(){}` stub into the builtin cache — even under
//!    `bao test`, node:test files registered nothing and reported 0/0.
//!    All bridging is now resolved lazily at call time.
//! 2. **Run-form execution (node parity)**: `bao test` (cli.rs →
//!    run_test_file → run_bun_tests_report) drives registered tests from
//!    the Rust side. Every other form (`bao run <file>`, `bao -e`) follows
//!    node's `node file.js` contract: registration is accepted, and the
//!    FIRST gated registration arms a one-shot exit-drain that runs the
//!    SAME runner core after the main module has finished, renders the
//!    report on stderr (the same ✓/✗ + counters shape the bao test CLI
//!    renders) and carries failures in the exit code (`failed > 0` →
//!    `process.exitCode = 1`). An explicit `test.run()` call keeps
//!    ownership of execution (no double run, no spurious 0/0 line).
//!
//! ## Architecture
//!
//! Follows the same JS IIFE pattern as node_stream.rs / node_vm.rs:
//! - `TEST_SOURCE` const holds the JS source
//! - `install()` evaluates the IIFE and registers the RESULT itself via
//!   `cache_builtin(cx, "test", ...)` (require() strips the `node:` prefix,
//!   so the bare key serves both specifiers)
//!
//! ## TestContext surface (node v24 oracle 2026-10-20, REQ-ENG-048)
//!
//! Every body callback registered through this bridge receives a node-shaped
//! TestContext as its first argument — `t.test()` (immediate subtest with
//! report-coupled pass/fail on await), `t.diagnostic()` (TAP `ℹ` line),
//! `t.todo()` (runs, never fails the run), `t.skip()` (does not run),
//! `t.plan()` (advisory), `t.mock`/`t.assert` (the real module objects), and
//! `name`/`fullName`/`filePath`. Members with no backing subsystem
//! (signal/passed/workerId/runOnly/tags/waitFor/attempt/error) are ABSENT —
//! a fake AbortSignal or a dead boolean would be a silent lie; absence fails
//! loudly. `getTestContext()` returns the running body's context.
//!
//! ## Callable module contract (node v24 oracle, REQ-ENG-048)
//!
//! Node.js `require('node:test')` returns the primary test FUNCTION itself
//! carrying the named surface (`t.suite === t.describe`,
//! `typeof t === 'function'`; `t.test` on a CONTEXT is a bound variant, the
//! module's `t.test === t` identity is the module-level entry). The IIFE
//! therefore returns the callable `ntModule` function with the named surface
//! attached; caching a copy of the named exports onto a fresh plain object
//! would break that contract (official suite files call the module object
//! directly). The bridge also normalizes node's `(name, options, fn)`
//! signature — a raw passthrough would register the options object AS the
//! test body and drop the real callback.
//!
//! ## References
//!
//! - Bun upstream: `src/js/node/test.ts`
//! - Node.js docs: https://nodejs.org/api/test.html

use bun_core::ZBox;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use crate::require::cache_builtin;

const TEST_SOURCE: &str = r#"
(function() {
  var _g = globalThis;

  // ── lazy infrastructure resolution ──
  // Resolved at CALL time, never at module-eval time: node_test::install runs
  // before bun_test::install_bun_test, so an install-time check would freeze
  // a dead stub into the builtin cache (see module docs).
  function _btm() {
    var btm = _g.__bun_test_module;
    if (!btm) {
      throw new Error("node:test: bun:test runner infrastructure (globalThis.__bun_test_module) is not installed in this context — refusing to fake test registration.");
    }
    return btm;
  }
  function _fn(host, path, name) {
    var f = host && host[name];
    if (typeof f !== 'function') {
      throw new Error("node:test: bun:test does not implement " + path + "." + name + " — refusing to fake it.");
    }
    return f;
  }

  // ── runner gate ──
  // @trace REQ-ENG-048 — two-form gate (node parity). `bao test`
  // (process.argv[1] === 'test') keeps the Rust-driven form: cli.rs
  // run_test_file → run_bun_tests_report starts the runner itself, so
  // registration passes through WITHOUT arming (no double run). Every
  // other form (`bao run <file>`, `bao -e`) follows node's `node file.js`
  // contract: registration is accepted, and the FIRST gated registration
  // arms a one-shot exit-drain — after the main module has finished, the
  // same runner core executes what was registered, renders the report on
  // stderr (the same ✓/✗ + counters shape the bao test CLI renders) and
  // carries failures in the exit code (failed > 0 → process.exitCode = 1).
  //
  // Ordering holds without extra machinery: the drain anchor is a
  // setTimeout(0), and due timers fire inside the run loop's drain pass —
  // which only starts AFTER the main module evaluation completes — so
  // tests never start mid-module (node ordering). The run loop stays
  // alive while the runner chain and its async dependencies settle (the
  // standard liveness predicate: timers/servers/fetch/fs/crypto/...), and
  // 'exit' listeners still fire only after the suites (same ordering the
  // bao test path implements). A hung test body pins the loop exactly
  // like node's run-form (no built-in timeout there either) — a hang is
  // honest; a silent 0/0 pass is not.
  var _drainArmed = false;
  var _explicitRun = false;
  function _runnerGate() {
    var argv = (_g.process && _g.process.argv) || [];
    if (argv[1] === 'test') {
      return; // `bao test`: the Rust runner drives execution.
    }
    if (_drainArmed) {
      return;
    }
    _drainArmed = true;
    // Fail closed at the gate: no event-loop anchor or no stderr writer
    // means registered tests could never be honestly executed and
    // reported — a loud refusal beats a silent fake pass.
    if (typeof _g.setTimeout !== 'function') {
      throw new Error("node:test: no setTimeout on globalThis — the run-form exit-drain has no event-loop anchor, refusing to register tests that would silently never execute.");
    }
    if (!_g.process || !_g.process.stderr || typeof _g.process.stderr.write !== 'function') {
      throw new Error("node:test: no process.stderr.write — the run-form report could not be rendered, refusing to register tests whose outcome would be invisible.");
    }
    _g.setTimeout(function() {
      // An explicit run() call owns execution: its own __run_bun_tests
      // start already consumed the pending tests, and a second start here
      // would walk an empty collector and print a spurious 0/0 line.
      if (_explicitRun) {
        return;
      }
      if (typeof _g.__run_bun_tests !== 'function') {
        throw new Error("node:test: bun:test runner (globalThis.__run_bun_tests) is not installed — refusing to drop registered tests silently.");
      }
      var p = _g.__run_bun_tests();
      if (!(p && typeof p.then === 'function')) {
        throw new Error("node:test: __run_bun_tests() did not return a Promise — runner/bridge version skew.");
      }
      p.then(function(report) {
        _renderRunFormReport(report);
        if (report && report.failed > 0) {
          _g.process.exitCode = 1;
        }
      }, function(err) {
        // Runner-level rejection: render one counted failure — the run
        // turns red instead of exiting silently as if nothing was pending.
        _renderRunFormReport({
          passed: 0,
          failed: 1,
          passes: [],
          failures: [{
            name: 'node:test',
            message: (err && (err.message || String(err))) || String(err),
            stack: (err && err.stack) || ''
          }]
        });
        _g.process.exitCode = 1;
      });
    }, 0);
  }

  // Same shape the `bao test` CLI renders (bao_cli/src/cli.rs render_report):
  // ✓ per pass, ✗ + message + first stack line per failure, then the
  // counters line — all on stderr. The run-form drain is the same report,
  // one form later.
  function _renderRunFormReport(report) {
    var r = report || {};
    var passes = r.passes || [];
    var failures = r.failures || [];
    var i;
    for (i = 0; i < passes.length; i++) {
      _g.process.stderr.write('✓ ' + passes[i] + '\n');
    }
    for (i = 0; i < failures.length; i++) {
      var f = failures[i] || {};
      _g.process.stderr.write('✗ ' + (f.name || '') + '\n');
      var msg = (f.message === null || f.message === undefined) ? '' : String(f.message);
      if (msg !== '') {
        _g.process.stderr.write('  ' + msg + '\n');
      }
      var stack = (f.stack === null || f.stack === undefined) ? '' : String(f.stack);
      if (stack !== '') {
        var first = stack.split('\n')[0];
        if (first !== '') {
          _g.process.stderr.write('  at: ' + first + '\n');
        }
      }
    }
    _g.process.stderr.write('  -> ' + (r.passed || 0) + ' passed, ' + (r.failed || 0) + ' failed\n');
  }

  // ── callable module ──
  // Node.js `require('node:test')` returns a CALLABLE function whose default
  // entry is the primary test function (`test(name, fn)`), plus named surface
  // (describe/it/suite/hooks/mock/assert/run/...). The previous bridge
  // returned a plain object, so `test('name', fn)` (node's canonical form)
  // threw "test is not a function" and silently produced vacuous 0/0 reports.
  // The module object IS the test function (node oracle: t.test === t): call
  // it, or call any named entry.
  // .length === 3 (node v24 oracle 2026-10-20: the primary entries report
  // arity 3 — (name, options, fn) — the documented max signature; the named
  // params below give the function its length).
  function ntModule(name, options, fn) {
    _runnerGate();
    return _callEntry('test', arguments);
  }

  // ── node signature normalization ──
  // Node.js test/describe/it accept both (name, fn) and (name, options, fn).
  // bun:test only implements (name, fn); with a raw passthrough the options
  // object would be registered AS the test body and the real callback dropped
  // (every 3-arg call site would then fail with a TypeError at run time).
  // Normalize here: options.skip/todo route to no registration (node counts
  // them in separate skipped/todo buckets the bun:test report has no rows
  // for — counting-neutral, the ✓/✗ lines and exit code stay honest);
  // advisory options (timeout/concurrency/...) don't alter registration,
  // matching the runner's flat sequential execution.
  function _normalizeArgs(args) {
    var name = args[0];
    var fn = null;
    var opts = null;
    if (args.length >= 3 && typeof args[2] === 'function') {
      opts = args[1];
      fn = args[2];
    } else if (args.length >= 2 && typeof args[1] === 'function') {
      fn = args[1];
    } else if (args.length >= 1 && typeof args[0] === 'function') {
      // (fn) shorthand: name defaults to the function's own name.
      name = args[0].name || '<anonymous>';
      fn = args[0];
    } else if (args.length >= 1 && typeof args[0] === 'string') {
      // (name) with no fn: node defaults fn to a no-op (the test passes).
      fn = function() {};
    } else {
      throw new Error("node:test test() requires a name or a callback function");
    }
    return { name: name, fn: fn, opts: opts };
  }

  // @trace REQ-ENG-048 — TestContext injection. Every body registered through
  // this bridge runs with a node-shaped TestContext as its first argument
  // (node v24 oracle: the body callback ALWAYS receives a context object —
  // a zero-arg body simply ignores it). The wrapper keeps the context on a
  // stack for the WHOLE body lifetime — including awaited continuations — so
  // nested t.test()/getTestContext() calls chain correctly; the runner is
  // strictly sequential so frames never interleave. Boundary: a body that
  // returns a never-settling promise pins its frame (the same body already
  // hangs the file — counted as a failure by the report guard).
  var _runningStack = [];
  function _currentCtx() {
    return _runningStack.length > 0 ? _runningStack[_runningStack.length - 1] : null;
  }
  function _bodyWithCtx(fn, name) {
    if (typeof fn !== 'function') return fn;
    return function() {
      var ctx = _mkCtx(name, _currentCtx());
      _runningStack.push(ctx);
      var r;
      try {
        // node contract: the body's FIRST parameter is the TestContext (the
        // runner invokes the wrapper with zero args, so the context is
        // injected here, not taken from the call site).
        r = fn.call(this, ctx);
      } catch (e) {
        _runningStack.pop();
        throw e;
      }
      if (r && typeof r.then === 'function') {
        return r.then(
          function(v) { _runningStack.pop(); return v; },
          function(e) { _runningStack.pop(); throw e; }
        );
      }
      _runningStack.pop();
      return r;
    };
  }

  function _callEntry(entryName, args) {
    var n = _normalizeArgs(args);
    if (n.opts && typeof n.opts === 'object') {
      if (n.opts.skip || n.opts.todo) {
        return undefined;
      }
    }
    var host = _btm();
    var entry = _fn(host, 'bun:test', entryName);
    return entry.call(host, n.name, _bodyWithCtx(n.fn, n.name));
  }

  // Gated delegate: check the runner gate, then route through _callEntry so
  // both (name, fn) and (name, options, fn) call sites register correctly.
  function entryGated(entryName) {
    return function() {
      _runnerGate();
      return _callEntry(entryName, arguments);
    };
  }

  // Plain gated passthrough (hooks and single-signature entries): resolve the
  // entry lazily at call time via host(), gate, delegate.
  function withGate(host, path, name) {
    var f = function(a, b) {
      _runnerGate();
      return _fn(host(), path, name).apply(host(), arguments);
    };
    f._target = name;
    // node oracle 2026-10-20: before/after (hooks) report arity 2.
    Object.defineProperty(f, 'length', { value: 2 });
    return f;
  }

  // ── test / describe / it ──
  // describe/it are gated delegates; the PRIMARY test entry is the module
  // function itself (node v24 oracle 2026-10-23, local binary:
  // `require('node:test').test === require('node:test')` → true — the module
  // IS the test entry, so there is no separate testFn). describe/it report
  // arity 3 (the documented (name, options, fn) max signature).
  var describeFn = entryGated('describe');
  var itFn = entryGated('it');
  Object.defineProperty(describeFn, 'length', { value: 3 });
  Object.defineProperty(itFn, 'length', { value: 3 });
  // Variant sub-methods delegate to the bun:test entry of the same name
  // (skip/todo/only/...). .each() returns a registrar per bun:test's shape.
  // Bodies registered through ANY variant get the same TestContext injection
  // as the primary entries (node: every body callback receives a context —
  // .failing/.only included), and conditional variants (.if/.skipIf/.onlyIf)
  // return a registrar that keeps the injection too.
  function _wrapLastFnArg(args) {
    for (var i = args.length - 1; i >= 0; i--) {
      if (typeof args[i] === 'function') {
        var vname = (typeof args[0] === 'string') ? args[0]
          : (typeof args[i].name === 'string' && args[i].name ? args[i].name : '<anonymous>');
        args[i] = _bodyWithCtx(args[i], vname);
        break;
      }
    }
    return args;
  }
  function withVariants(target, entryName, variants) {
    variants.forEach(function(variant) {
      target[variant] = function() {
        _runnerGate();
        var host = _btm();
        var base = _fn(host, 'bun:test', entryName);
        var v = _fn(base, 'bun:test.' + entryName, variant);
        var result = v.apply(base, _wrapLastFnArg(Array.prototype.slice.call(arguments)));
        if (typeof result === 'function') {
          // Conditional-variant registrar: the returned entry registers
          // bodies later — keep the injection on those registrations.
          // @trace REQ-ENG-048 — preserve the registrar's own chain surface
          // (.skip/.if/.skipIf/...): the bun:test conditional trio returns a
          // callable no-run registrar (bun 1.4.2 oracle: skipIf/if return
          // functions on both branches), and callers may chain another
          // conditional or .skip on it (`test.skipIf(true).skipIf(false)`).
          // The bare wrapper below used to drop that surface, so any chained
          // call threw TypeError and failed the file — the same skip-only
          // no-report class the shim-side registrar normalization closes.
          var registrar = result;
          var wrapped = function() {
            return registrar.apply(this, _wrapLastFnArg(Array.prototype.slice.call(arguments)));
          };
          for (var regKey in registrar) { wrapped[regKey] = registrar[regKey]; }
          result = wrapped;
        }
        return result;
      };
      // node oracle 2026-10-20: every variant entry reports arity 3.
      Object.defineProperty(target[variant], 'length', { value: 3 });
    });
    return target;
  }
  // The test variants attach to the module function itself (nt.test === nt,
  // so nt.test.skip IS nt.skip — the node identity contract above).
  withVariants(describeFn, 'describe', ['skip', 'todo', 'only', 'each', 'if', 'skipIf']);
  withVariants(itFn, 'it', ['skip', 'todo', 'only', 'each', 'failing', 'if', 'skipIf', 'onlyIf']);

  // ── TestContext ──
  // node v24 oracle (2026-10-20, node --test): the body callback receives a
  // TestContext object whose OWN key set is empty (name/fullName/diagnostic/
  // test/... all live on the prototype — Object.getOwnPropertyNames(t) === []
  // on the oracle, the prototype carries after/afterEach/assert/attempt/
  // before/beforeEach/diagnostic/error/filePath/fullName/mock/name/passed/
  // plan/runOnly/signal/skip/tags/test/todo/waitFor/workerId). The bridge
  // materializes the members it can implement FOR REAL (test/diagnostic/todo/
  // skip/plan/mock/assert/filePath + name/fullName) and leaves the ones with
  // no backing subsystem ABSENT (signal/passed/workerId/runOnly/tags/
  // waitFor/attempt/error — a fake EventTarget or a dead boolean would be a
  // silent lie; absence fails loudly instead).
  function _mkCtx(name, parent) {
    var full = parent ? parent.fullName + ' > ' + name : name;
    var filePath = typeof _g.__filename === 'string' ? _g.__filename : undefined;
    // Prototype carries every member (oracle parity: own keys stay empty).
    var proto = {
      name: name,
      fullName: full,
      filePath: filePath,
      // @trace REQ-ENG-048 — t.diagnostic is the spec'd TAP diagnostic
      // ("ℹ <message>" on the runner's stderr); console.log is the accepted
      // TAP-14 synonym (node's spec reporter renders console.log output from
      // inside a test as the same ℹ line).
      diagnostic: function() {
        if (arguments.length > 0) console.log.apply(console, arguments);
      },
      // t.test() creates a real subtest that runs IMMEDIATELY (its outcome
      // lands in the current report — no silent drop) and returns a Promise
      // the parent may await; awaiting couples the parent's pass/fail to the
      // subtest exactly like node. fullName chains node's " > " separator.
      // Boundary vs node: an UN-awaited subtest still runs and still counts
      // into the report, but does not fail the parent (node couples it at
      // parent completion; the flat runner has no per-test completion hook).
      test: function(subName, subOpts, subFn) {
        _runnerGate();
        var n = _normalizeArgs([subName, subOpts, subFn]);
        if (n.opts && typeof n.opts === 'object' && (n.opts.skip || n.opts.todo)) {
          // node: skipped/todo subtests don't run and don't fail anything;
          // bao's report has no skipped/todo rows — counting-neutral.
          return undefined;
        }
        // The immediate-run primitive lives on the bun:test runner shim (the
        // counters and hook stacks are closed over there — no fallback:
        // a missing primitive is a runner-shim/bridge version skew and
        // must fail loudly, not degrade to deferred registration that
        // reorders every subtest after the whole file).
        if (typeof _g.__run_one_bun_test !== 'function') {
          throw new Error("node:test t.test(): the bun:test runner shim does not expose __run_one_bun_test — runner/bridge version skew, refusing to reorder subtests behind a silent drop.");
        }
        // node coupling: an AWAITED subtest's failure fails the parent too
        // (oracle: [parent-with-failing-subtest, outer-pass] → pass 1,
        // fail 2). The promise rejects with the subtest's ORIGINAL error;
        // the no-op catch below pre-marks it handled so a fire-and-forget
        // caller never trips an unhandled rejection — the failure is already
        // counted into the report either way (exit-code honesty holds).
        var sub = _g.__run_one_bun_test(n.name, _bodyWithCtx(n.fn, n.name), full + ' > ' + n.name)
          .then(function(outcome) {
            if (outcome && outcome.ok === false) {
              throw (outcome.error || new Error('subtest failed: ' + n.name));
            }
            return outcome;
          });
        sub.catch(function() {});
        return sub;
      },
      // t.todo(msg[, fn]): node RUNS the body and never lets a todo outcome
      // fail the run (oracle: throwing body → "✔ … # <msg>", todo bucket).
      todo: function(msg, fn) {
        _runnerGate();
        var todoName = typeof msg === 'string' ? msg : '<todo>';
        var todoFn = typeof fn === 'function' ? fn : function() {};
        if (typeof _g.__run_one_bun_test !== 'function') {
          throw new Error("node:test t.todo(): the bun:test runner shim does not expose __run_one_bun_test — runner/bridge version skew.");
        }
        return _g.__run_one_bun_test(todoName, _bodyWithCtx(function(ctx) {
          var r;
          try {
            r = todoFn(ctx);
          } catch (e) { return undefined; }
          if (r && typeof r.then === 'function') {
            return r.then(function() {}, function() {});
          }
          return undefined;
        }, todoName), full + ' > ' + todoName);
      },
      // t.skip(msg): node registers a skipped test (body never runs, skipped
      // bucket). bao's report has no skipped row — the honest face is to not
      // run the body and count nothing (same counting-neutral shape as the
      // module-level {skip:true} option).
      skip: function() { return undefined; },
      // plan(n): node fails the test when the assertion count mismatches.
      // The flat runner derives its counts from actual executions, so the
      // count source t.plan feeds does not exist here — advisory no-op
      // (documented degraded mode), NOT an enforced check pretending to run.
      plan: function() {},
      mock: mockObj,
      assert: assertObj
    };
    return Object.create(proto);
  }

  // ── hooks ──
  // Node.js node:test uses before/after (aliases for beforeAll/afterAll).
  var beforeFn = withGate(_btm, 'bun:test', 'beforeAll');
  var afterFn = withGate(_btm, 'bun:test', 'afterAll');
  var beforeEachFn = withGate(_btm, 'bun:test', 'beforeEach');
  var afterEachFn = withGate(_btm, 'bun:test', 'afterEach');

  // ── mock ──
  // @trace REQ-ENG-048 — node v24.19.0 oracle (2026-10-27, host binary):
  // MockTracker prototype = [fn, getter, method, property, reset, restoreAll,
  // setter, timers] with empty own keys; a MockFunction's own keys are
  // [length, name] with `.mock` on its prototype chain; MockFunctionContext
  // prototype = [callCount, calls, mockImplementation, mockImplementationOnce,
  // resetCalls, restore]; every call entry carries all six slots in the
  // oracle's key order — {arguments, error, result, stack, target, this} —
  // with `target` a reserved slot the oracle leaves undefined, `this` the
  // call's receiver, and exactly one of error/result set; resetCalls
  // REBINDS the backing array (a reference captured before the reset keeps
  // its content). Pinned oracle behaviors: mockImplementationOnce is a
  // SINGLE slot consumed by the next call (queue A then B → call 1 "B",
  // call 2 falls through to the original);
  // without a queued/permanent implementation the call forwards to `original`
  // (this-preserving, return/throw transparent); tracker reset() restores
  // spied properties and clears permanent implementations but leaves
  // once-queues and call history; ctx.restore() additionally clears the once
  // queue (calls preserved); the mock function's name is the original's own
  // name verbatim ("" when anonymous), or "original" when mock.fn() got no
  // original at all; every context method returns undefined (no chaining).
  //
  // The former bridge delegated fn/spyOn to `bun:test.jest` — that face IS
  // on the bun:test module, but as an OBJECT (Bun's jest compat), so the
  // `_fn` function-shape guard rejected every mock.fn() call with
  // "bun:test does not implement bun:test.jest — refusing to fake it"
  // (final-green4-r4 golden-node corpus: test-fs-write-stream-eagain.mjs
  // `_write`, `mock.fn(fs.write)`). Even past the guard, a jest-compat mock
  // is not node's MockFunctionContext contract (calls entry shape,
  // single-slot once, restore/resetCalls surface), so node:test's mock face
  // is implemented here natively on oracle semantics — the same
  // self-contained shape the getter/setter entries already used. Documented
  // absences (no invented face): mock.timers (node's fake-timer engine — a
  // backing subsystem this bridge does not have; a fake clock would lie),
  // and the property-mock's mockImplementation/mockImplementationOnce proto
  // members (their semantics on a value property are unprobed — omitted
  // rather than guessed).
  var _mocks = [];

  function _argTypeErr(argName, value) {
    var e = new TypeError('The "' + argName + '" argument must be a function. Received ' +
      (value === null ? 'null' : typeof value));
    e.code = 'ERR_INVALID_ARG_TYPE';
    return e;
  }
  // node routes method/getter/setter property validation through one check
  // ("must be a method", ERR_INVALID_ARG_VALUE) — a getter called on a value
  // property throws the same shape (oracle: getter({x:7},'x') →
  // ERR_INVALID_ARG_VALUE "Received undefined").
  function _methodArgErr(received) {
    var e = new TypeError("The argument 'propertyName' must be a method. Received " + received);
    e.code = 'ERR_INVALID_ARG_VALUE';
    return e;
  }

  // Shared mock core. Returns a RECORD (never exposed to callers): the
  // MockFunction, its `.mock` MockFunctionContext, and the reset/restore
  // hooks the tracker walks. `spy` is set when the mock replaced a real
  // property (method/getter/setter) — restore() puts the original
  // property descriptor back.
  function _mkMockFunction(original, initialImpl, spy) {
    var impl = (typeof initialImpl === 'function') ? initialImpl : null;
    var onceImpl = null; // single slot — a second mockImplementationOnce overwrites the first (oracle)
    var calls = [];

    function restoreSpy() {
      if (spy) Object.defineProperty(spy.object, spy.property, spy.descriptor);
    }
    function restoreMock() {
      restoreSpy();
      impl = null;
      onceImpl = null;
    }
    var ctxProto = {
      mockImplementation: function(f) {
        if (typeof f !== 'function') throw _argTypeErr('implementation', f);
        impl = f;
      },
      mockImplementationOnce: function(f) {
        if (typeof f !== 'function') throw _argTypeErr('implementation', f);
        onceImpl = f;
      },
      callCount: function() { return calls.length; },
      // oracle: resetCalls REBINDS the backing array — a reference captured
      // before the reset keeps its content, later calls land in a fresh one.
      resetCalls: function() { calls = []; },
      restore: restoreMock
    };
    Object.defineProperty(ctxProto, 'calls', { get: function() { return calls; }, configurable: true });
    var mockCtx = Object.create(ctxProto);
    var fnProto = Object.create(Function.prototype);
    fnProto.mock = mockCtx;
    // Method shorthand: own keys are exactly [length, name] (no sloppy-mode
    // prototype/caller/arguments own props — oracle parity), and the strict
    // this semantics match the oracle's recorded `this` slot (undefined for
    // plain calls, the receiver for call()/method-style use).
    var fn = {
      mockCall() {
        'use strict'; // node's mock is strict-mode code: a plain call records `this: undefined`
        // Resolution chain per call: once slot (consumed on use — fires
        // exactly once) → permanent implementation → original.
        var target;
        if (onceImpl !== null) {
          target = onceImpl;
          onceImpl = null;
        } else {
          target = impl;
        }
        if (target === null && typeof original !== 'function') {
          // node: mock.fn() with neither original nor implementation — the
          // call is still recorded and returns undefined.
          calls.push({ arguments: Array.prototype.slice.call(arguments), error: undefined, result: undefined, stack: new Error(), target: undefined, this: this });
          return undefined;
        }
        if (target === null) target = original;
        var entry = { arguments: Array.prototype.slice.call(arguments), error: undefined, result: undefined, stack: new Error(), target: undefined, this: this };
        calls.push(entry);
        try {
          entry.result = target.apply(this, arguments);
          return entry.result;
        } catch (e) {
          entry.error = e;
          throw e;
        }
      }
    }.mockCall;
    Object.setPrototypeOf(fn, fnProto);
    Object.defineProperty(fn, 'name', {
      value: original ? String(original.name || '') : 'original',
      configurable: true
    });
    Object.defineProperty(fn, 'length', { value: original ? original.length : 0, configurable: true });
    return {
      fn: fn,
      ctx: mockCtx,
      // tracker reset(): restore the spied property + drop the permanent
      // implementation, KEEP the once queue and call history (oracle quirk:
      // after reset, a queued mockImplementationOnce still fires).
      reset: function() {
        restoreSpy();
        impl = null;
      },
      restore: restoreMock
    };
  }

  // method/getter/setter spy installation: replace one property with a mock
  // function, preserving the rest of the original descriptor.
  function _installSpy(object, propertyName, kind, descriptor, mockFn) {
    var desc = { enumerable: descriptor.enumerable, configurable: true };
    desc[kind] = mockFn;
    var other = (kind === 'get') ? 'set' : 'get';
    if (typeof descriptor[other] === 'function') desc[other] = descriptor[other];
    Object.defineProperty(object, propertyName, desc);
  }

  function _spyAccessor(object, propertyName, kind, hasImpl, implementation) {
    var d = Object.getOwnPropertyDescriptor(object, propertyName);
    if (!d || typeof d[kind] !== 'function') {
      throw _methodArgErr(d ? String(d[kind]) : 'undefined');
    }
    if (hasImpl && typeof implementation !== 'function') throw _argTypeErr('implementation', implementation);
    var rec = _mkMockFunction(d[kind], hasImpl ? implementation : null,
      { object: object, property: propertyName, descriptor: d });
    _installSpy(object, propertyName, kind, d, rec.fn);
    _mocks.push(rec);
    return rec.fn;
  }

  var mockTrackerProto = {
    fn: function(original, implementation) {
      if (arguments.length > 0 && typeof original !== 'function') throw _argTypeErr('original', original);
      if (arguments.length > 1 && typeof implementation !== 'function') throw _argTypeErr('implementation', implementation);
      var rec = _mkMockFunction(
        arguments.length > 0 ? original : null,
        arguments.length > 1 ? implementation : null,
        null
      );
      _mocks.push(rec);
      return rec.fn;
    },
    getter: function(object, propertyName, implementation) {
      return _spyAccessor(object, propertyName, 'get', arguments.length > 2, implementation);
    },
    setter: function(object, propertyName, implementation) {
      return _spyAccessor(object, propertyName, 'set', arguments.length > 2, implementation);
    },
    method: function(object, propertyName, implementation) {
      var d = Object.getOwnPropertyDescriptor(object, propertyName);
      if (!d || typeof d.value !== 'function') {
        throw _methodArgErr(d ? String(d.value) : 'undefined');
      }
      if (arguments.length > 2 && typeof implementation !== 'function') throw _argTypeErr('implementation', implementation);
      var rec = _mkMockFunction(d.value, arguments.length > 2 ? implementation : null,
        { object: object, property: propertyName, descriptor: d });
      Object.defineProperty(object, propertyName, {
        value: rec.fn,
        writable: d.writable,
        enumerable: d.enumerable,
        configurable: true
      });
      _mocks.push(rec);
      return rec.fn;
    },
    // node v24 oracle: property(obj, name, value) replaces a property's VALUE
    // (a function value is stored verbatim, not called), records every
    // access as {type: get|set, value, stack: Error}, and returns a handle
    // whose own key is the property name (a live view of the current value)
    // with `.mock` on its prototype [accesses, accessCount, resetAccesses,
    // restore]. Accessor-backed properties delegate to the original
    // getter/setter while recording (replacing their behavior with a storage
    // slot would break the property's invariants).
    property: function(object, propertyName, value) {
      var d = Object.getOwnPropertyDescriptor(object, propertyName);
      if (!d) throw _methodArgErr('undefined');
      var storage = value;
      var origGet = (typeof d.get === 'function') ? d.get : null;
      var origSet = (typeof d.set === 'function') ? d.set : null;
      var accesses = [];
      var ctxProto = {
        accessCount: function() { return accesses.length; },
        resetAccesses: function() { accesses.length = 0; },
        restore: function() { Object.defineProperty(object, propertyName, d); }
      };
      Object.defineProperty(ctxProto, 'accesses', { get: function() { return accesses; }, configurable: true });
      var rec = {
        reset: ctxProto.restore,
        restore: ctxProto.restore
      };
      Object.defineProperty(object, propertyName, {
        get: origGet ? function() {
          var r = origGet.call(this);
          accesses.push({ type: 'get', value: r, stack: new Error() });
          return r;
        } : function() {
          accesses.push({ type: 'get', value: storage, stack: new Error() });
          return storage;
        },
        set: origSet ? function(v) {
          accesses.push({ type: 'set', value: v, stack: new Error() });
          origSet.call(this, v);
        } : function(v) {
          accesses.push({ type: 'set', value: v, stack: new Error() });
          storage = v;
        },
        enumerable: d.enumerable,
        configurable: true
      });
      var handle = Object.create({ mock: Object.create(ctxProto) });
      // The handle's property-name key reads THROUGH the mocked property
      // (oracle: reading handle.p records a get access and yields the
      // current value — a live view, not a snapshot).
      Object.defineProperty(handle, propertyName, {
        get: function() { return object[propertyName]; },
        configurable: true
      });
      _mocks.push(rec);
      return handle;
    },
    reset: function() {
      for (var i = 0; i < _mocks.length; i++) _mocks[i].reset();
    },
    restoreAll: function() {
      for (var i = 0; i < _mocks.length; i++) _mocks[i].restore();
    }
  };
  var mockObj = Object.create(mockTrackerProto);

  // ── assert ──
  // Node.js node:test assert subset. Delegates to the Node.js assert module
  // (real implementation); the local fallback is itself a real asserting
  // implementation, not a pass-through stub.
  var _nodeAssert = null;
  function _assert() {
    if (_nodeAssert === null) {
      _nodeAssert = (typeof _g.require === 'function')
        ? (function() { try { return _g.require('assert'); } catch (e) { return undefined; } })()
        : undefined;
    }
    return _nodeAssert;
  }
  function _delegateAssert(name, fallback) {
    return function() {
      var na = _assert();
      if (na && typeof na[name] === 'function') return na[name].apply(na, arguments);
      return fallback.apply(null, arguments);
    };
  }
  var assertObj = {
    ok: _delegateAssert('ok', function(val, msg) { if (!val) throw new Error(msg || 'assertion failed'); }),
    equal: _delegateAssert('equal', function(a, b, msg) { if (a != b) throw new Error(msg || 'expected ' + a + ' to equal ' + b); }),
    notEqual: _delegateAssert('notEqual', function(a, b, msg) { if (a == b) throw new Error(msg || 'expected values not to be equal'); }),
    deepEqual: _delegateAssert('deepEqual', function(a, b, msg) { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(msg || 'deep equal failed'); }),
    notDeepEqual: _delegateAssert('notDeepEqual', function(a, b, msg) { if (JSON.stringify(a) === JSON.stringify(b)) throw new Error(msg || 'expected values not to deep equal'); }),
    strictEqual: _delegateAssert('strictEqual', function(a, b, msg) { if (a !== b) throw new Error(msg || 'strict equal failed'); }),
    notStrictEqual: _delegateAssert('notStrictEqual', function(a, b, msg) { if (a === b) throw new Error(msg || 'expected values not to be strictly equal'); }),
    throws: _delegateAssert('throws', function(fn, expected, msg) {
      var threw = false;
      try { fn(); } catch(e) { threw = true; if (expected && typeof expected === 'function' && !(e instanceof expected)) throw new Error(msg || 'wrong error type'); }
      if (!threw) throw new Error(msg || 'expected function to throw');
    }),
    doesNotThrow: _delegateAssert('doesNotThrow', function(fn, expected, msg) {
      try { fn(); } catch(e) { throw new Error(msg || 'expected function not to throw'); }
    }),
    rejects: _delegateAssert('rejects', function(asyncFn, expected, msg) {
      return Promise.resolve().then(function() {
        var p = typeof asyncFn === 'function' ? asyncFn() : asyncFn;
        return p.then(function() { throw new Error(msg || 'expected promise to reject'); }, function(e) {
          if (expected && typeof expected === 'function' && !(e instanceof expected)) throw new Error(msg || 'wrong rejection type');
        });
      });
    }),
    ifError: _delegateAssert('ifError', function(err) { if (err) throw err; }),
    match: function(actual, regex, msg) {
      if (!regex.test(actual)) throw new Error(msg || 'expected ' + actual + ' to match ' + regex);
    },
    doesNotMatch: function(actual, regex, msg) {
      if (regex.test(actual)) throw new Error(msg || 'expected ' + actual + ' not to match ' + regex);
    }
  };

  // ── run() ──
  // Delegates to the real bun:test runner. Explicit API — not gated, the
  // caller asked for execution, and a missing runner fails explicitly.
  function runFn() {
    // Explicit-API ownership: the caller started the runner themselves, so
    // the run-form exit-drain must not start a second pass over the (now
    // empty) collector.
    _explicitRun = true;
    if (typeof _g.__run_bun_tests !== 'function') {
      throw new Error("node:test run(): bun:test runner (globalThis.__run_bun_tests) is not installed — refusing to return a fake empty report.");
    }
    return _g.__run_bun_tests();
  }

  // ── snapshot ──
  // node v24 oracle (2026-10-20): module-level `snapshot` is an OBJECT whose
  // full key set is exactly [setDefaultSnapshotSerializers,
  // setResolveSnapshotPath] (both functions, no flag needed). Node accepts
  // valid configuration and validates argument types with
  // ERR_INVALID_ARG_TYPE ("The \"serializers\" argument must be an instance
  // of Array. Received type string ('nope')" / "The \"fn\" argument must be
  // of type function. Received type number (123)"). The stored configuration
  // is INERT under bao (the flat runner has no snapshot store — a snapshot
  // assert would fail at its own call site); that degraded state is
  // documented here rather than hidden: the config face is real node parity,
  // the snapshot materialization is not implemented.
  var _snapshotSerializers = [];
  var _snapshotResolve = null;
  function _receivedType(v) {
    if (v === null) return 'null';
    if (Array.isArray(v)) return 'an Array';
    return 'type ' + typeof v + ' (' + String(v) + ')';
  }
  var snapshotObj = {
    setDefaultSnapshotSerializers: function(serializers) {
      if (!Array.isArray(serializers)) {
        throw new Error('The "serializers" argument must be an instance of Array. Received ' + _receivedType(serializers));
      }
      _snapshotSerializers = serializers;
    },
    setResolveSnapshotPath: function(fn) {
      if (typeof fn !== 'function') {
        throw new Error('The "fn" argument must be of type function. Received ' + _receivedType(fn));
      }
      _snapshotResolve = fn;
    }
  };

  // ── expectFailure ──
  // node v24 oracle (2026-10-20): `expectFailure(name, options?, fn?)`
  // registers an expected-failure test — arity 3, aliases test with
  // expectFail=true. A throwing/rejecting body is the PASS path; a clean run
  // is a FAILURE ('test was expected to fail but passed'). Delegates to the
  // bun:test failing registration (the inversion logic lives there).
  function expectFailureFn(name, options, fn) {
    _runnerGate();
    var n = _normalizeArgs(arguments);
    var host = _btm();
    var failing = _fn(_fn(host, 'bun:test', 'test'), 'bun:test.test', 'failing');
    return failing.call(host, n.name, _bodyWithCtx(n.fn, n.name));
  }

  // ── node-true surface assembly ──
  // Attach the full node:test named surface onto the callable module function
  // so `const test = require('node:test')` and
  // `const { describe, it, suite } = require('node:test')` both work, and
  // destructuring a named entry (`test.test`, `test.describe`, ...) also
  // reaches the real implementations. This mirrors Node.js v24 where
  // `require('node:test')` IS the primary test function with named exports.
  var nt = ntModule;
  // @trace REQ-ENG-048 — node v24 oracle (2026-10-23 local binary):
  // `nt.test === nt` — the module function IS the primary test entry, and
  // the test variant sub-methods therefore live on the module itself
  // (nt.test.skip === nt.skip). The former bridge attached a SEPARATE testFn
  // here, which broke the identity contract every official-suite destructure
  // (`const { test } = nt`) and the `t.test === t` probe relies on.
  nt.test = nt;
  withVariants(nt, 'test', ['skip', 'todo', 'only', 'failing', 'if', 'skipIf', 'onlyIf', 'each']);
  nt.describe = describeFn;
  nt.it = itFn;
  // Node.js `suite()` is an alias of `describe()` (node oracle: t.suite ===
  // t.describe). Surface both bindings exactly as node does.
  nt.suite = describeFn;
  nt.before = beforeFn;
  nt.after = afterFn;
  nt.beforeAll = beforeFn;
  nt.afterAll = afterFn;
  nt.beforeEach = beforeEachFn;
  nt.afterEach = afterEachFn;
  nt.mock = mockObj;
  nt.assert = assertObj;
  nt.run = runFn;
  // @trace REQ-ENG-048 — snapshot + expectFailure named exports (node v24
  // module keys; the former bridge omitted both).
  nt.snapshot = snapshotObj;
  nt.expectFailure = expectFailureFn;
  // getTestContext(): node v24 exposes the test context of the RUNNING test
  // body. The TestContext surface is materialized now (t.test/t.diagnostic/
  // t.todo/t.skip/t.plan + name/fullName); outside any body there is no
  // current context — node's own contract would hand back a dead object, the
  // fail-closed throw beats a silent zero-context lie.
  nt.getTestContext = function() {
    var ctx = _currentCtx();
    if (ctx === null) {
      throw new Error("node:test getTestContext() called outside a running test body — no current TestContext exists (the context is the first argument of the test body callback).");
    }
    return ctx;
  };

  return nt;
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes("node:test".as_bytes());
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(TEST_SOURCE);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);
        mozjs::glue::DeleteCompileOptions(opts);

        if !ok || !rval.is_object() {
            return;
        }

        // The IIFE result IS the module object: a callable function carrying
        // the full node:test named surface (test/describe/it/suite/hooks/
        // mock/assert/run). Node.js `require('node:test')` returns this
        // function itself — copying named exports onto a fresh plain object
        // would destroy the callable-module contract (typeof === 'function')
        // that official suite files rely on.
        let module_fn = rval.to_object();
        rooted!(&in(cx) let module_rooted = module_fn);

        // Cache under the bare name "test": require() strips the "node:"
        // prefix (require.rs require_fn), so both require("test") and
        // require("node:test") resolve to "builtin:test". The previous
        // "node:test" key matched neither and require("node:test") threw
        // "Cannot find module".
        cache_builtin(cx, "test", module_rooted.get());
    }
}
