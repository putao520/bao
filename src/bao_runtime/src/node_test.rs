// @trace REQ-ENG-009 [api:node:test] — Node.js test runner module
//! node:test module — bridges to the existing `globalThis.__bun_test_module`
//! installed by `bun_test.rs`.
//!
//! Exposes the Node.js `node:test` API surface (test(), describe(), it(),
//! before/after hooks, mock, assert) by delegating to the bun:test
//! infrastructure already present on `globalThis.__bun_test_module`.
//!
//! ## Architecture
//!
//! Follows the same JS IIFE pattern as node_stream.rs / node_vm.rs:
//! - `TEST_SOURCE` const holds the JS source
//! - `install()` evaluates the IIFE, extracts the returned object, and
//!   registers it via `cache_builtin(cx, "node:test", ...)`
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
  var _btm = _g.__bun_test_module;
  if (!_btm) {
    // bun:test shim not installed yet — return an empty module stub so
    // require("node:test") doesn't throw.
    return {
      test: function() {},
      describe: function() {},
      it: function() {},
      before: function() {},
      after: function() {},
      beforeEach: function() {},
      afterEach: function() {},
      mock: {},
      assert: {},
      run: function() {}
    };
  }

  // ── test function ──
  // Node.js node:test uses test() as the primary entry; Bun's bun:test uses
  // test() too. Bridge directly.
  var testFn = _btm.test || function() {};

  // Attach sub-methods from bun:test
  testFn.skip = _btm.test && _btm.test.skip ? _btm.test.skip : function() {};
  testFn.todo = _btm.test && _btm.test.todo ? _btm.test.todo : function() {};
  testFn.only = _btm.test && _btm.test.only ? _btm.test.only : function() {};
  testFn.failing = _btm.test && _btm.test.failing ? _btm.test.failing : function() {};
  testFn.if = _btm.test && _btm.test.if ? _btm.test.if : function(cond) { return cond ? testFn : { skip: function(){} }; };
  testFn.skipIf = _btm.test && _btm.test.skipIf ? _btm.test.skipIf : function(cond) { return cond ? { skip: function(){}, only: function(){} } : testFn; };
  testFn.onlyIf = _btm.test && _btm.test.onlyIf ? _btm.test.onlyIf : function(cond) { return cond ? testFn : { skip: function(){}, only: function(){} }; };
  testFn.each = _btm.test && _btm.test.each ? _btm.test.each : function() { return function(name, fn) { testFn(name, fn); }; };

  // ── describe ──
  var describeFn = _btm.describe || function() {};
  describeFn.skip = _btm.describe && _btm.describe.skip ? _btm.describe.skip : function() {};
  describeFn.todo = _btm.describe && _btm.describe.todo ? _btm.describe.todo : function() {};
  describeFn.only = _btm.describe && _btm.describe.only ? _btm.describe.only : function(name, fn) { describeFn(name, fn); };
  describeFn.each = _btm.describe && _btm.describe.each ? _btm.describe.each : function() { return function(name, fn) { describeFn(name, fn); }; };
  describeFn.if = _btm.describe && _btm.describe.if ? _btm.describe.if : function(cond) { return cond ? describeFn : { skip: function(){} }; };
  describeFn.skipIf = _btm.describe && _btm.describe.skipIf ? _btm.describe.skipIf : function(cond) { return cond ? { skip: function(){}, only: function(){}, if: function(){ return { skip: function(){} }; } } : describeFn; };

  // ── it ──
  var itFn = _btm.it || function() {};
  itFn.skip = _btm.it && _btm.it.skip ? _btm.it.skip : function() {};
  itFn.todo = _btm.it && _btm.it.todo ? _btm.it.todo : function() {};
  itFn.only = _btm.it && _btm.it.only ? _btm.it.only : function(name, fn) { itFn(name, fn); };
  itFn.each = _btm.it && _btm.it.each ? _btm.it.each : function() { return function(name, fn) { itFn(name, fn); }; };
  itFn.failing = _btm.it && _btm.it.failing ? _btm.it.failing : function() {};
  itFn.if = _btm.it && _btm.it.if ? _btm.it.if : function(cond) { return cond ? itFn : { skip: function(){} }; };
  itFn.skipIf = _btm.it && _btm.it.skipIf ? _btm.it.skipIf : function(cond) { return cond ? { skip: function(){}, only: function(){} } : itFn; };
  itFn.onlyIf = _btm.it && _btm.it.onlyIf ? _btm.it.onlyIf : function(cond) { return cond ? itFn : { skip: function(){}, only: function(){} }; };

  // ── hooks ──
  // Node.js node:test uses before/after (aliases for beforeAll/afterAll)
  var beforeFn = _btm.beforeAll || _btm.before || function() {};
  var afterFn = _btm.afterAll || _btm.after || function() {};
  var beforeEachFn = _btm.beforeEach || function() {};
  var afterEachFn = _btm.afterEach || function() {};

  // ── mock ──
  // Node.js node:test exposes test.mock with fn/spyOn/restore/clear/reset
  var jestObj = _btm.jest || {};
  var mockObj = {
    fn: jestObj.fn || function(impl) {
      var calls = [];
      var fn = impl ? impl : function() {};
      var wrapper = function() {
        calls.push(Array.prototype.slice.call(arguments));
        return fn.apply(this, arguments);
      };
      wrapper.mock = { calls: calls };
      wrapper.mockImplementation = function(newImpl) { fn = newImpl; return wrapper; };
      wrapper.mockReturnValue = function(val) { fn = function() { return val; }; return wrapper; };
      wrapper.mockRestore = function() {};
      wrapper.mockClear = function() { calls.length = 0; return wrapper; };
      wrapper.mockReset = function() { calls.length = 0; fn = function() {}; return wrapper; };
      return wrapper;
    },
    spyOn: jestObj.spyOn || function(obj, method) {
      if (!obj || typeof obj[method] !== 'function') {
        throw new Error('mock.spyOn requires an object with a function property');
      }
      var original = obj[method];
      var mock = mockObj.fn(original);
      obj[method] = mock;
      mock.mockRestore = function() { obj[method] = original; };
      return mock;
    },
    restore: function() {},
    clear: function() {},
    reset: function() {},
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
  // Node.js node:test assert subset. Delegate to the Node.js assert module
  // if available, otherwise provide a minimal implementation.
  var assertObj;
  var nodeAssert = (typeof _g.require === 'function') ? (function() { try { return _g.require('assert'); } catch(e) { return null; } })() : null;
  if (nodeAssert) {
    assertObj = {
      ok: nodeAssert.ok || function(val, msg) { if (!val) throw new Error(msg || 'assertion failed'); },
      equal: nodeAssert.equal || function(a, b, msg) { if (a != b) throw new Error(msg || 'expected ' + a + ' to equal ' + b); },
      notEqual: nodeAssert.notEqual || function(a, b, msg) { if (a == b) throw new Error(msg || 'expected values not to be equal'); },
      deepEqual: nodeAssert.deepEqual || function(a, b, msg) { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(msg || 'deep equal failed'); },
      notDeepEqual: nodeAssert.notDeepEqual || function(a, b, msg) { if (JSON.stringify(a) === JSON.stringify(b)) throw new Error(msg || 'expected values not to deep equal'); },
      strictEqual: nodeAssert.strictEqual || function(a, b, msg) { if (a !== b) throw new Error(msg || 'strict equal failed'); },
      notStrictEqual: nodeAssert.notStrictEqual || function(a, b, msg) { if (a === b) throw new Error(msg || 'expected values not to be strictly equal'); },
      throws: nodeAssert.throws || function(fn, expected, msg) {
        var threw = false;
        try { fn(); } catch(e) { threw = true; if (expected && typeof expected === 'function' && !(e instanceof expected)) throw new Error(msg || 'wrong error type'); }
        if (!threw) throw new Error(msg || 'expected function to throw');
      },
      doesNotThrow: nodeAssert.doesNotThrow || function(fn, expected, msg) {
        try { fn(); } catch(e) { throw new Error(msg || 'expected function not to throw'); }
      },
      rejects: nodeAssert.rejects || function(asyncFn, expected, msg) {
        return Promise.resolve().then(function() {
          var p = typeof asyncFn === 'function' ? asyncFn() : asyncFn;
          return p.then(function() { throw new Error(msg || 'expected promise to reject'); }, function(e) {
            if (expected && typeof expected === 'function' && !(e instanceof expected)) throw new Error(msg || 'wrong rejection type');
          });
        });
      },
      ifError: nodeAssert.ifError || function(err) { if (err) throw err; },
      match: function(actual, regex, msg) {
        if (!regex.test(actual)) throw new Error(msg || 'expected ' + actual + ' to match ' + regex);
      },
      doesNotMatch: function(actual, regex, msg) {
        if (regex.test(actual)) throw new Error(msg || 'expected ' + actual + ' not to match ' + regex);
      }
    };
  } else {
    // Minimal assert without node:assert
    assertObj = {
      ok: function(val, msg) { if (!val) throw new Error(msg || 'assertion failed'); },
      equal: function(a, b, msg) { if (a != b) throw new Error(msg || 'expected ' + a + ' to equal ' + b); },
      notEqual: function(a, b, msg) { if (a == b) throw new Error(msg || 'expected values not to be equal'); },
      deepEqual: function(a, b, msg) { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(msg || 'deep equal failed'); },
      notDeepEqual: function(a, b, msg) { if (JSON.stringify(a) === JSON.stringify(b)) throw new Error(msg || 'expected values not to deep equal'); },
      strictEqual: function(a, b, msg) { if (a !== b) throw new Error(msg || 'strict equal failed'); },
      notStrictEqual: function(a, b, msg) { if (a === b) throw new Error(msg || 'expected values not to be strictly equal'); },
      throws: function(fn, expected, msg) {
        var threw = false;
        try { fn(); } catch(e) { threw = true; if (expected && typeof expected === 'function' && !(e instanceof expected)) throw new Error(msg || 'wrong error type'); }
        if (!threw) throw new Error(msg || 'expected function to throw');
      },
      doesNotThrow: function(fn, expected, msg) {
        try { fn(); } catch(e) { throw new Error(msg || 'expected function not to throw'); }
      },
      rejects: function(asyncFn, expected, msg) {
        return Promise.resolve().then(function() {
          var p = typeof asyncFn === 'function' ? asyncFn() : asyncFn;
          return p.then(function() { throw new Error(msg || 'expected promise to reject'); }, function(e) {
            if (expected && typeof expected === 'function' && !(e instanceof expected)) throw new Error(msg || 'wrong rejection type');
          });
        });
      },
      ifError: function(err) { if (err) throw err; },
      match: function(actual, regex, msg) {
        if (!regex.test(actual)) throw new Error(msg || 'expected ' + actual + ' to match ' + regex);
      },
      doesNotMatch: function(actual, regex, msg) {
        if (regex.test(actual)) throw new Error(msg || 'expected ' + actual + ' not to match ' + regex);
      }
    };
  }

  // ── run() ──
  // Node.js node:test run() delegates to __run_bun_tests()
  function runFn() {
    if (typeof _g.__run_bun_tests === 'function') {
      return _g.__run_bun_tests();
    }
    return { passed: 0, failed: 0, errors: [] };
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
            "test", "describe", "it",
            "before", "after", "beforeAll", "afterAll",
            "beforeEach", "afterEach",
            "mock", "assert", "run",
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

        cache_builtin(cx, "node:test", mod_obj.get());
    }
}
