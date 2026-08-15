// @trace REQ-ENG-009 [api:node:test] — Node.js test runner module
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
//! 2. **Plain-run fake pass**: only `bao test` (cli.rs → run_test_file →
//!    run_bun_tests_report) drives registered tests. In any other mode
//!    (`bao run`, `-e`, library embedding) registration would silently
//!    no-op and the user would believe tests passed. test()/describe()/it()
//!    and the hooks now refuse with an explicit throw unless
//!    `process.argv[1] === 'test'` (the `bao test` subcommand).
//!
//! ## Architecture
//!
//! Follows the same JS IIFE pattern as node_stream.rs / node_vm.rs:
//! - `TEST_SOURCE` const holds the JS source
//! - `install()` evaluates the IIFE, extracts the returned object, and
//!   registers it via `cache_builtin(cx, "test", ...)` (require() strips
//!   the `node:` prefix, so the bare key serves both specifiers)
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
use mozjs::rust::wrappers2 as w2;

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
  // `bao test` is the only mode that executes registered suites
  // (runtime.rs run_test_file → run_bun_tests_report). Everywhere else,
  // registration would silently no-op and fake a pass. Fail closed.
  function _runnerGate() {
    var argv = (_g.process && _g.process.argv) || [];
    if (argv[1] !== 'test') {
      throw new Error("node:test requires `bao test <file>`: this process is not the bao test runner (process.argv[1] !== 'test'), so registered tests would never execute and would silently fake a pass. Re-run with: bao test <file>");
    }
  }

  // ── test function ──
  // Node.js node:test uses test() as the primary entry; Bun's bun:test uses
  // test() too. Bridge directly, arguments pass through unchanged.
  function withGate(host, path, name) {
    var f = function() {
      _runnerGate();
      return _fn(host(), path, name).apply(host(), arguments);
    };
    f._target = name;
    return f;
  }
  var testFn = withGate(_btm, 'bun:test', 'test');
  // Sub-methods delegate to the bun:test variants of the same name.
  ['skip', 'todo', 'only', 'failing', 'if', 'skipIf', 'onlyIf', 'each'].forEach(function(variant) {
    testFn[variant] = withGate(function() { return _fn(_btm(), 'bun:test', 'test'); },
                               'bun:test.test', variant);
  });

  // ── describe ──
  var describeFn = withGate(_btm, 'bun:test', 'describe');
  ['skip', 'todo', 'only', 'each', 'if', 'skipIf'].forEach(function(variant) {
    describeFn[variant] = withGate(function() { return _fn(_btm(), 'bun:test', 'describe'); },
                                   'bun:test.describe', variant);
  });

  // ── it ──
  var itFn = withGate(_btm, 'bun:test', 'it');
  ['skip', 'todo', 'only', 'each', 'failing', 'if', 'skipIf', 'onlyIf'].forEach(function(variant) {
    itFn[variant] = withGate(function() { return _fn(_btm(), 'bun:test', 'it'); },
                             'bun:test.it', variant);
  });

  // ── hooks ──
  // Node.js node:test uses before/after (aliases for beforeAll/afterAll).
  var beforeFn = withGate(_btm, 'bun:test', 'beforeAll');
  var afterFn = withGate(_btm, 'bun:test', 'afterAll');
  var beforeEachFn = withGate(_btm, 'bun:test', 'beforeEach');
  var afterEachFn = withGate(_btm, 'bun:test', 'afterEach');

  // ── mock ──
  // Node.js node:test exposes test.mock with fn/spyOn/restore/clear/reset.
  // fn/spyOn delegate to bun:test jest (real call-tracking mocks); restore
  // actually restores every spy created through this module.
  var _spies = [];
  var mockObj = {
    fn: function(impl) {
      var jest = _fn(_btm(), 'bun:test', 'jest');
      return _fn(jest, 'bun:test.jest', 'fn')(impl);
    },
    spyOn: function(obj, method) {
      if (!obj || typeof obj[method] !== 'function') {
        throw new Error('mock.spyOn requires an object with a function property');
      }
      var jest = _fn(_btm(), 'bun:test', 'jest');
      var mock = _fn(jest, 'bun:test.jest', 'spyOn')(obj, method);
      _spies.push({ mock: mock, obj: obj, method: method });
      return mock;
    },
    restore: function() {
      for (var i = 0; i < _spies.length; i++) {
        var s = _spies[i];
        if (typeof s.mock.mockRestore === 'function') s.mock.mockRestore();
        else s.obj[s.method] = s.mock._original;
      }
      _spies.length = 0;
    },
    clear: function() {
      for (var i = 0; i < _spies.length; i++) {
        if (typeof _spies[i].mock.mockClear === 'function') _spies[i].mock.mockClear();
      }
    },
    reset: function() {
      for (var i = 0; i < _spies.length; i++) {
        if (typeof _spies[i].mock.mockReset === 'function') _spies[i].mock.mockReset();
      }
    },
    getter: function(obj, prop, impl) {
      var orig = Object.getOwnPropertyDescriptor(obj, prop);
      Object.defineProperty(obj, prop, { get: impl, configurable: true });
      return { mockRestore: function() { if (orig) Object.defineProperty(obj, prop, orig); } };
    },
    setter: function(obj, prop, impl) {
      var orig = Object.getOwnPropertyDescriptor(obj, prop);
      Object.defineProperty(obj, prop, { set: impl, configurable: true });
      return { mockRestore: function() { if (orig) Object.defineProperty(obj, prop, orig); } };
    }
  };

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
    if (typeof _g.__run_bun_tests !== 'function') {
      throw new Error("node:test run(): bun:test runner (globalThis.__run_bun_tests) is not installed — refusing to return a fake empty report.");
    }
    return _g.__run_bun_tests();
  }

  return {
    test: testFn,
    describe: describeFn,
    it: itFn,
    before: beforeFn,
    after: afterFn,
    beforeAll: beforeFn,
    afterAll: afterFn,
    beforeEach: beforeEachFn,
    afterEach: afterEachFn,
    mock: mockObj,
    assert: assertObj,
    run: runFn
  };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

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
        libc::free(opts as *mut _);

        if !ok || !rval.is_object() {
            return;
        }

        let exports_obj = rval.to_object();
        rooted!(&in(cx) let exports_rooted = exports_obj);

        // Copy all named exports from the IIFE result onto the module object.
        for name in &[
            "test",
            "describe",
            "it",
            "before",
            "after",
            "beforeAll",
            "afterAll",
            "beforeEach",
            "afterEach",
            "mock",
            "assert",
            "run",
        ] {
            let cname = ZBox::from_bytes(name.as_bytes());
            let mut val = UndefinedValue();
            JS_GetProperty(
                cx_raw,
                exports_rooted.handle().into(),
                cname.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut val,
                },
            );
            if !val.is_undefined() {
                rooted!(&in(cx) let val_root = val);
                JS_DefineProperty(
                    cx_raw,
                    mod_obj.handle().into(),
                    cname.as_ptr(),
                    val_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // Cache under the bare name "test": require() strips the "node:"
        // prefix (require.rs require_fn), so both require("test") and
        // require("node:test") resolve to "builtin:test". The previous
        // "node:test" key matched neither and require("node:test") threw
        // "Cannot find module".
        cache_builtin(cx, "test", mod_obj.get());
    }
}
