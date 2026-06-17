// @trace REQ-IMPL-01
// bun:test + harness compatibility shims for Bun upstream test compat
use bun_core::ZBox;
use ::std::ptr;

use mozjs::jsapi::*;
use mozjs::jsval::{UndefinedValue, Int32Value};

use crate::gc_store;

const BUN_TEST_SHIM: &str = r#"
(function() {
  var _g = globalThis;
  var _suites = [];
  var _currentDescribe = null;
  var _passed = 0;
  var _failed = 0;
  var _errors = [];
  var _beforeEachFns = [];
  var _afterEachFns = [];
  var _beforeAllFns = [];
  var _afterAllFns = [];

  var _passNames = [];
  var _failEntries = [];

  function _runTest(name, fn) {
    try {
      for (var i = 0; i < _beforeEachFns.length; i++) {
        _beforeEachFns[i]();
      }
      var result = fn();
      if (result && typeof result.then === 'function') {
        // Sync test runner — async tests not supported in shim mode
        throw new Error("bun:test shim does not support async tests");
      }
      for (var j = 0; j < _afterEachFns.length; j++) {
        _afterEachFns[j]();
      }
      _passed++;
      _passNames.push(name);
    } catch (e) {
      for (var k = 0; k < _afterEachFns.length; k++) {
        try { _afterEachFns[k](); } catch (_) {}
      }
      _failed++;
      _errors.push({ name: name, error: e });
      _failEntries.push({
        name: name,
        message: (e && (e.message || e.toString())) || String(e),
        stack: (e && e.stack) || ""
      });
    }
  }

  function _makeExpect(actual) {
    var e = {
      toBe: function(expected) {
        if (actual !== expected) {
          throw new Error("Expected " + JSON.stringify(actual) + " to be " + JSON.stringify(expected));
        }
        return e;
      },
      toEqual: function(expected) {
        var a = JSON.stringify(actual);
        var b = JSON.stringify(expected);
        if (a !== b) {
          throw new Error("Expected " + a + " to equal " + b);
        }
        return e;
      },
      toBeTruthy: function() {
        if (!actual) {
          throw new Error("Expected " + JSON.stringify(actual) + " to be truthy");
        }
        return e;
      },
      toBeFalsy: function() {
        if (actual) {
          throw new Error("Expected " + JSON.stringify(actual) + " to be falsy");
        }
        return e;
      },
      toBeNull: function() {
        if (actual !== null) {
          throw new Error("Expected " + JSON.stringify(actual) + " to be null");
        }
        return e;
      },
      toBeUndefined: function() {
        if (actual !== undefined) {
          throw new Error("Expected " + JSON.stringify(actual) + " to be undefined");
        }
        return e;
      },
      toBeDefined: function() {
        if (actual === undefined) {
          throw new Error("Expected value to be defined");
        }
        return e;
      },
      toBeNaN: function() {
        if (!Number.isNaN(actual)) {
          throw new Error("Expected " + JSON.stringify(actual) + " to be NaN");
        }
        return e;
      },
      toBeGreaterThan: function(expected) {
        if (!(actual > expected)) {
          throw new Error("Expected " + JSON.stringify(actual) + " > " + JSON.stringify(expected));
        }
        return e;
      },
      toBeGreaterThanOrEqual: function(expected) {
        if (!(actual >= expected)) {
          throw new Error("Expected " + JSON.stringify(actual) + " >= " + JSON.stringify(expected));
        }
        return e;
      },
      toBeLessThan: function(expected) {
        if (!(actual < expected)) {
          throw new Error("Expected " + JSON.stringify(actual) + " < " + JSON.stringify(expected));
        }
        return e;
      },
      toBeLessThanOrEqual: function(expected) {
        if (!(actual <= expected)) {
          throw new Error("Expected " + JSON.stringify(actual) + " <= " + JSON.stringify(expected));
        }
        return e;
      },
      toBeCloseTo: function(expected, precision) {
        precision = precision || 2;
        var diff = Math.abs(actual - expected);
        var threshold = Math.pow(10, -precision) / 2;
        if (diff >= threshold) {
          throw new Error("Expected " + JSON.stringify(actual) + " to be close to " + JSON.stringify(expected));
        }
        return e;
      },
      toContain: function(expected) {
        if (typeof actual === 'string') {
          if (actual.indexOf(expected) === -1) {
            throw new Error("Expected \"" + actual + "\" to contain \"" + expected + "\"");
          }
        } else if (Array.isArray(actual)) {
          if (actual.indexOf(expected) === -1) {
            throw new Error("Expected array to contain " + JSON.stringify(expected));
          }
        } else {
          throw new Error("toContain requires string or array");
        }
        return e;
      },
      toHaveLength: function(expected) {
        if (actual == null || actual.length !== expected) {
          throw new Error("Expected length " + expected + " but got " + (actual ? actual.length : "null"));
        }
        return e;
      },
      toThrow: function(expected) {
        var threw = false;
        var thrownError = null;
        try {
          actual();
        } catch (err) {
          threw = true;
          thrownError = err;
        }
        if (!threw) {
          throw new Error("Expected function to throw");
        }
        // Optional matcher: string (substring), RegExp (test), or Error class.
        // Matches Jest's toThrow semantics. e.g. toThrow(/out of range/i)
        // or toThrow("RangeError") or toThrow(RangeError).
        if (expected !== undefined && thrownError !== null) {
          var msg = (thrownError && (thrownError.message || thrownError.toString())) || String(thrownError);
          if (typeof expected === 'string') {
            if (msg.indexOf(expected) === -1 && thrownError.name !== expected) {
              throw new Error("Expected thrown error to contain \"" + expected + "\" but got \"" + msg + "\"");
            }
          } else if (expected instanceof RegExp) {
            if (!expected.test(msg)) {
              throw new Error("Expected thrown error \"" + msg + "\" to match " + expected);
            }
          } else if (typeof expected === 'function') {
            if (!(thrownError instanceof expected)) {
              throw new Error("Expected thrown error to be instance of " + (expected.name || 'function'));
            }
          } else if (expected && typeof expected === 'object' && expected.message !== undefined) {
            if (msg.indexOf(expected.message) === -1) {
              throw new Error("Expected thrown error to contain \"" + expected.message + "\" but got \"" + msg + "\"");
            }
          }
        }
        return e;
      },
      toThrowError: function(expectedMsgOrClass) {
        var threw = false;
        var thrownError = null;
        try {
          actual();
        } catch (err) {
          threw = true;
          thrownError = err;
        }
        if (!threw) {
          throw new Error("Expected function to throw");
        }
        if (expectedMsgOrClass) {
          if (typeof expectedMsgOrClass === 'string') {
            if (thrownError.message !== expectedMsgOrClass && thrownError.message.indexOf(expectedMsgOrClass) === -1) {
              throw new Error("Expected error message to contain \"" + expectedMsgOrClass + "\" but got \"" + thrownError.message + "\"");
            }
          } else if (typeof expectedMsgOrClass === 'function') {
            if (!(thrownError instanceof expectedMsgOrClass)) {
              throw new Error("Expected error to be instance of " + expectedMsgOrClass.name);
            }
          }
        }
        return e;
      },
      toMatch: function(expected) {
        var regex = typeof expected === 'string' ? new RegExp(expected) : expected;
        if (!regex.test(actual)) {
          throw new Error("Expected " + JSON.stringify(actual) + " to match " + regex);
        }
        return e;
      },
      toMatchObject: function(expected) {
        var keys = Object.keys(expected);
        for (var i = 0; i < keys.length; i++) {
          var key = keys[i];
          if (typeof expected[key] === 'object' && expected[key] !== null) {
            var sub = JSON.stringify(actual[key]);
            var exp = JSON.stringify(expected[key]);
            if (sub !== exp) {
              throw new Error("Expected " + key + " to match: got " + sub + " expected " + exp);
            }
          } else if (actual[key] !== expected[key]) {
            throw new Error("Expected " + key + " to be " + JSON.stringify(expected[key]) + " but got " + JSON.stringify(actual[key]));
          }
        }
        return e;
      },
      toHaveProperty: function(path, value) {
        var parts = typeof path === 'string' ? path.split('.') : [path];
        var obj = actual;
        for (var i = 0; i < parts.length; i++) {
          if (obj == null || obj[parts[i]] === undefined) {
            throw new Error("Expected object to have property \"" + parts.join('.') + "\"");
          }
          obj = obj[parts[i]];
        }
        if (arguments.length > 1 && obj !== value) {
          throw new Error("Expected property \"" + parts.join('.') + "\" to be " + JSON.stringify(value) + " but got " + JSON.stringify(obj));
        }
        return e;
      },
      // @trace REQ-ENG-006 — jest.fn() mock call assertions.
      // Jest/bun mock matchers: toHaveBeenCalled / toHaveBeenCalledTimes(n) /
      // toHaveBeenCalledWith(...args) / toHaveBeenLastCalledWith(...args) /
      // toHaveBeenNthCalledWith(n, ...args).
      // `actual` is the mock returned by jest.fn(). Its `.mock.calls` array
      // holds one entry per invocation; each entry is the call's arguments.
      toHaveBeenCalled: function() {
        var calls = _mockCalls(actual);
        if (calls === null) {
          throw new Error("toHaveBeenCalled requires a jest.fn() mock");
        }
        if (calls.length === 0) {
          throw new Error("Expected mock to have been called, but it was called 0 times");
        }
        return e;
      },
      toHaveBeenCalledTimes: function(n) {
        var calls = _mockCalls(actual);
        if (calls === null) {
          throw new Error("toHaveBeenCalledTimes requires a jest.fn() mock");
        }
        if (calls.length !== n) {
          throw new Error("Expected mock to have been called " + n + " times, but it was called " + calls.length + " times");
        }
        return e;
      },
      toHaveBeenCalledWith: function() {
        var calls = _mockCalls(actual);
        if (calls === null) {
          throw new Error("toHaveBeenCalledWith requires a jest.fn() mock");
        }
        var expectedArgs = Array.prototype.slice.call(arguments);
        var found = false;
        for (var i = 0; i < calls.length; i++) {
          if (_argsEqual(calls[i], expectedArgs)) { found = true; break; }
        }
        if (!found) {
          throw new Error("Expected mock to have been called with " + JSON.stringify(expectedArgs) + ", but actual calls were " + JSON.stringify(calls));
        }
        return e;
      },
      toHaveBeenLastCalledWith: function() {
        var calls = _mockCalls(actual);
        if (calls === null) {
          throw new Error("toHaveBeenLastCalledWith requires a jest.fn() mock");
        }
        if (calls.length === 0) {
          throw new Error("Expected mock to have been called, but it was called 0 times");
        }
        var expectedArgs = Array.prototype.slice.call(arguments);
        if (!_argsEqual(calls[calls.length - 1], expectedArgs)) {
          throw new Error("Expected last call to be " + JSON.stringify(expectedArgs) + ", but was " + JSON.stringify(calls[calls.length - 1]));
        }
        return e;
      },
      toHaveBeenNthCalledWith: function(nth) {
        var calls = _mockCalls(actual);
        if (calls === null) {
          throw new Error("toHaveBeenNthCalledWith requires a jest.fn() mock");
        }
        var expectedArgs = Array.prototype.slice.call(arguments, 1);
        if (nth < 1 || nth > calls.length) {
          throw new Error("Expected call #" + nth + " but mock was only called " + calls.length + " times");
        }
        if (!_argsEqual(calls[nth - 1], expectedArgs)) {
          throw new Error("Expected call #" + nth + " to be " + JSON.stringify(expectedArgs) + ", but was " + JSON.stringify(calls[nth - 1]));
        }
        return e;
      },
      resolves: {},
      rejects: {},
      not: {
        toBe: function(expected) {
          if (actual === expected) {
            throw new Error("Expected " + JSON.stringify(actual) + " not to be " + JSON.stringify(expected));
          }
          return e.not;
        },
        toEqual: function(expected) {
          var a = JSON.stringify(actual);
          var b = JSON.stringify(expected);
          if (a === b) {
            throw new Error("Expected values not to equal");
          }
          return e.not;
        },
        toBeTruthy: function() {
          if (actual) {
            throw new Error("Expected " + JSON.stringify(actual) + " not to be truthy");
          }
          return e.not;
        },
        toBeFalsy: function() {
          if (!actual) {
            throw new Error("Expected " + JSON.stringify(actual) + " not to be falsy");
          }
          return e.not;
        },
        toBeNull: function() {
          if (actual === null) {
            throw new Error("Expected not to be null");
          }
          return e.not;
        },
        toThrow: function(expected) {
          var threw = false;
          var thrownError = null;
          try { actual(); } catch (err) { threw = true; thrownError = err; }
          if (threw) {
            // If a matcher is provided, only fail when the matcher matches.
            if (expected !== undefined) {
              var msg = (thrownError && (thrownError.message || thrownError.toString())) || String(thrownError);
              var matches = false;
              if (typeof expected === 'string') {
                matches = (msg.indexOf(expected) !== -1 || thrownError.name === expected);
              } else if (expected instanceof RegExp) {
                matches = expected.test(msg);
              } else if (typeof expected === 'function') {
                matches = (thrownError instanceof expected);
              } else if (expected && typeof expected === 'object' && expected.message !== undefined) {
                matches = (msg.indexOf(expected.message) !== -1);
              }
              if (matches) {
                throw new Error("Expected function not to throw matching error, but threw: " + msg);
              }
            } else {
              throw new Error("Expected function not to throw");
            }
          }
          return e.not;
        },
        toContain: function(expected) {
          if (typeof actual === 'string') {
            if (actual.indexOf(expected) !== -1) {
              throw new Error("Expected \"" + actual + "\" not to contain \"" + expected + "\"");
            }
          } else if (Array.isArray(actual)) {
            if (actual.indexOf(expected) !== -1) {
              throw new Error("Expected array not to contain " + JSON.stringify(expected));
            }
          }
          return e.not;
        },
        toMatch: function(expected) {
          var regex = typeof expected === 'string' ? new RegExp(expected) : expected;
          if (regex.test(actual)) {
            throw new Error("Expected " + JSON.stringify(actual) + " not to match " + regex);
          }
          return e.not;
        },
        // @trace REQ-ENG-006 — negated jest.fn() mock assertions.
        toHaveBeenCalled: function() {
          var calls = _mockCalls(actual);
          if (calls !== null && calls.length > 0) {
            throw new Error("Expected mock not to have been called, but it was called " + calls.length + " times");
          }
          return e.not;
        },
        toHaveBeenCalledTimes: function(n) {
          var calls = _mockCalls(actual);
          if (calls !== null && calls.length === n) {
            throw new Error("Expected mock not to have been called exactly " + n + " times");
          }
          return e.not;
        },
        toHaveBeenCalledWith: function() {
          var calls = _mockCalls(actual);
          if (calls === null) { return e.not; }
          var expectedArgs = Array.prototype.slice.call(arguments);
          for (var i = 0; i < calls.length; i++) {
            if (_argsEqual(calls[i], expectedArgs)) {
              throw new Error("Expected mock not to have been called with " + JSON.stringify(expectedArgs));
            }
          }
          return e.not;
        }
      }
    };
    return e;
  }

  var expectFn = function(actual) { return _makeExpect(actual); };
  expectFn.extend = function(actual) { return _makeExpect(actual); };

  // @trace REQ-ENG-006 — jest.fn() mock infrastructure.
  //
  // A mock is a callable function that records every invocation on a hidden
  // `_mockState` property. The state holds `calls` (one array of args per
  // invocation), `results` (return value or thrown error per invocation),
  // and `instances` (`this` per invocation). The mock also exposes `.mock`
  // (jest's public surface: `mock.calls`, `mock.results`, `mock.instances`)
  // and chainable `.mockImplementation` / `.mockReturnValue` /
  // `.mockReturnValueOnce` / `.mockResolvedValue` builders.
  function _argsEqual(a, b) {
    if (a === b) { return true; }
    if (a == null || b == null) { return false; }
    if (a.length !== b.length) { return false; }
    for (var i = 0; i < a.length; i++) {
      var av = a[i], bv = b[i];
      if (av === bv) { continue; }
      if (av == null || bv == null) { return false; }
      if (typeof av !== typeof bv) { return false; }
      if (av instanceof RegExp && bv instanceof RegExp) {
        if (av.source !== bv.source) { return false; }
        continue;
      }
      if (Array.isArray(av) && Array.isArray(bv)) {
        if (!_argsEqual(av, bv)) { return false; }
        continue;
      }
      if (typeof av === 'object' && typeof bv === 'object') {
        // Shallow structural compare for plain arg objects.
        var akeys = Object.keys(av), bkeys = Object.keys(bv);
        if (akeys.length !== bkeys.length) { return false; }
        for (var k = 0; k < akeys.length; k++) {
          if (av[akeys[k]] !== bv[akeys[k]]) { return false; }
        }
        continue;
      }
      // NaN-aware numeric compare.
      if (typeof av === 'number' && typeof bv === 'number' && isNaN(av) && isNaN(bv)) { continue; }
      return false;
    }
    return true;
  }

  // Returns the mock's call list, or null if `value` is not a tracked mock.
  function _mockCalls(value) {
    if (typeof value !== 'function') { return null; }
    var st = value._mockState;
    if (!st) { return null; }
    return st.calls;
  }

  function _makeMock(impl) {
    impl = (typeof impl === 'function') ? impl : function() {};
    var state = { calls: [], results: [], instances: [] };
    var returnQueue = [];
    var returnValue;
    var hasReturnValue = false;
    var currentImpl = impl;

    var fn = function() {
      var args = Array.prototype.slice.call(arguments);
      state.calls.push(args);
      state.instances.push(this);
      try {
        var result;
        if (returnQueue.length > 0) {
          result = returnQueue.shift();
        } else if (hasReturnValue) {
          result = returnValue;
        } else {
          result = currentImpl.apply(this, args);
        }
        state.results.push({ type: 'return', value: result });
        return result;
      } catch (err) {
        state.results.push({ type: 'throw', value: err });
        throw err;
      }
    };

    // Public mock surface (jest-compatible).
    fn.mock = state;
    fn._mockState = state;

    fn.mockImplementation = function(newImpl) {
      if (typeof newImpl === 'function') { currentImpl = newImpl; }
      return fn;
    };
    fn.mockReturnValue = function(val) { returnValue = val; hasReturnValue = true; return fn; };
    fn.mockReturnValueOnce = function(val) { returnQueue.push(val); return fn; };
    fn.mockResolvedValue = function(val) {
      returnValue = Promise.resolve(val);
      hasReturnValue = true;
      return fn;
    };
    fn.mockResolvedValueOnce = function(val) {
      returnQueue.push(Promise.resolve(val));
      return fn;
    };
    fn.mockRejectedValue = function(err) {
      returnValue = Promise.reject(err);
      hasReturnValue = true;
      return fn;
    };
    fn.mockReset = function() {
      state.calls = []; state.results = []; state.instances = [];
      returnQueue = []; hasReturnValue = false; returnValue = undefined;
      return fn;
    };
    fn.mockClear = function() {
      state.calls = []; state.results = []; state.instances = [];
      return fn;
    };
    fn.getMockName = function() { return 'jest.fn()'; };
    fn.mockName = function() { return fn; };

    return fn;
  }

  function describeFn(name, fn) {
    _suites.push({ name: name, fn: fn });
  }
  describeFn.skip = function(name, fn) { /* no-op */ };
  describeFn.todo = function(name, fn) { /* no-op */ };
  describeFn.each = function() { return function(name, fn) { describeFn(name, fn); }; };
  describeFn.only = function(name, fn) { describeFn(name, fn); };
  describeFn.if = function(cond) { return cond ? describeFn : { skip: function(){} }; };

  function itFn(name, fn) {
    if (_currentDescribe) {
      _runTest(_currentDescribe + " > " + name, fn);
    } else {
      _runTest(name, fn);
    }
  }
  itFn.skip = function(name, fn) { /* no-op */ };
  itFn.todo = function(name, fn) { /* no-op */ };
  itFn.each = function() { return function(name, fn) { itFn(name, fn); }; };
  itFn.only = function(name, fn) { itFn(name, fn); };
  itFn.failing = function(name, fn) {
    // In failing mode, we expect the test to throw
    var fullName = _currentDescribe ? (_currentDescribe + " > " + name) : name;
    try {
      fn();
      _failed++;
      _errors.push({ name: fullName, error: new Error("Expected test to fail but it passed") });
      _failEntries.push({
        name: fullName,
        message: "Expected test to fail but it passed",
        stack: ""
      });
    } catch (e) {
      _passed++; // Expected to fail, so it's a pass
      _passNames.push(fullName);
    }
  };

  function testFn(name, fn) {
    itFn(name, fn);
  }
  testFn.skip = itFn.skip;
  testFn.todo = itFn.todo;
  testFn.each = itFn.each;
  testFn.only = itFn.only;
  testFn.failing = itFn.failing;
  testFn.if = function(cond) { return cond ? testFn : { skip: function(){} }; };

  function beforeEachFn(fn) { _beforeEachFns.push(fn); }
  function afterEachFn(fn) { _afterEachFns.push(fn); }
  function beforeAllFn(fn) { _beforeAllFns.push(fn); }
  function afterAllFn(fn) { _afterAllFns.push(fn); }

  var bunTestModule = {
    describe: describeFn,
    test: testFn,
    it: itFn,
    expect: expectFn,
    beforeEach: beforeEachFn,
    afterEach: afterEachFn,
    beforeAll: beforeAllFn,
    afterAll: afterAllFn,
    // @trace REQ-ENG-006 — jest.fn() returns a call-tracking mock (see _makeMock).
    jest: {
      fn: function(impl) { return _makeMock(impl); },
      spyOn: function(obj, methodName) {
        if (!obj || typeof obj[methodName] !== 'function') {
          throw new Error('jest.spyOn requires an object with a function property');
        }
        var original = obj[methodName];
        var mock = _makeMock(original);
        obj[methodName] = mock;
        mock.mockRestore = function() { obj[methodName] = original; };
        return mock;
      }
    },
    setDefaultTimeout: function() {},
    skip: function() {},
    todo: function() {},
    fail: function(msg) { throw new Error(msg || "Test failed explicitly"); },
    gc: function() {},
    printConsole: function() {}
  };

  _g.__bun_test_module = bunTestModule;

  // Test runner — called after all suites registered
  _g.__run_bun_tests = function() {
    for (var i = 0; i < _beforeAllFns.length; i++) {
      try { _beforeAllFns[i](); } catch (e) {
        _errors.push({ name: "beforeAll", error: e });
        _failed++;
        _failEntries.push({
          name: "beforeAll",
          message: (e && (e.message || e.toString())) || String(e),
          stack: (e && e.stack) || ""
        });
      }
    }
    for (var s = 0; s < _suites.length; s++) {
      _currentDescribe = _suites[s].name;
      try { _suites[s].fn(); } catch (e) {
        _failed++;
        _errors.push({ name: _suites[s].name, error: e });
        _failEntries.push({
          name: _suites[s].name,
          message: (e && (e.message || e.toString())) || String(e),
          stack: (e && e.stack) || ""
        });
      }
      _currentDescribe = null;
    }
    for (var j = 0; j < _afterAllFns.length; j++) {
      try { _afterAllFns[j](); } catch (e) {
        _errors.push({ name: "afterAll", error: e });
        _failed++;
        _failEntries.push({
          name: "afterAll",
          message: (e && (e.message || e.toString())) || String(e),
          stack: (e && e.stack) || ""
        });
      }
    }
    return { passed: _passed, failed: _failed, errors: _errors, passes: _passNames, failures: _failEntries };
  };
})();
"#;

const HARNESS_SHIM: &str = r#"
(function() {
  var _g = globalThis;
  _g.__harness_module = {
    gc: function() {},
    bunExe: function() { return "bao"; },
    bunEnv: function() { return _g.process ? _g.process.env : {}; },
    isWindows: function() { return _g.process && _g.process.platform === "win32"; },
    isLinux: function() { return _g.process && _g.process.platform === "linux"; },
    isMac: function() { return _g.process && _g.process.platform === "darwin"; },
    isASAN: function() { return false; },
    isDebug: function() { return false; },
    isMinified: function() { return false; },
    withoutAggressiveGC: function(fn) { return fn(); },
    expectOOM: function() { return false; },
    BunEnvironment: { browser: false, test: true }
  };
})();
"#;

/// # Safety
/// Caller must ensure `cx` is a valid JSContext with an active request on the current thread.
pub unsafe fn install_bun_test(cx: &mut mozjs::context::JSContext) {
    let raw = cx.raw_cx();

    // Eval bun:test shim — sets globalThis.__bun_test_module
    eval_shim(raw, BUN_TEST_SHIM, "bun:test");

    // The eval creates __bun_test_module on globalThis — use it directly as the builtin cache entry
    let src = eval_shim_get_obj(raw, "globalThis.__bun_test_module");
    if !src.is_null() {
        gc_store::gc_store_insert(raw, "builtin:bun:test", src);
    }

    // Eval harness shim
    eval_shim(raw, HARNESS_SHIM, "harness");
    let harness_src = eval_shim_get_obj(raw, "globalThis.__harness_module");
    if !harness_src.is_null() {
        gc_store::gc_store_insert(raw, "builtin:harness", harness_src);
    }
}

unsafe fn eval_shim(raw: *mut JSContext, source: &str, label: &str) {
    let c_filename = ZBox::from_vec(format!("<{}-shim>", label).into_bytes());
    let opts = mozjs::glue::NewCompileOptions(raw, c_filename.as_ptr(), 1);
    if opts.is_null() {
        log::warn!("Failed to create compile options for {} shim", label);
        return;
    }
    let mut src_text = mozjs::rust::transform_str_to_source_text(source);
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(raw, opts, &mut src_text, rval_h);
    libc::free(opts as *mut _);
    if !ok {
        log::warn!("Failed to eval {} shim", label);
    }
}

unsafe fn eval_shim_get_obj(raw: *mut JSContext, expr: &str) -> *mut JSObject {
    let c_filename = ZBox::from_bytes("<shim-get>".as_bytes());
    let opts = mozjs::glue::NewCompileOptions(raw, c_filename.as_ptr(), 1);
    if opts.is_null() {
        return ptr::null_mut();
    }
    let mut src_text = mozjs::rust::transform_str_to_source_text(expr);
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(raw, opts, &mut src_text, rval_h);
    libc::free(opts as *mut _);
    if ok && rval.is_object() {
        rval.to_object()
    } else {
        ptr::null_mut()
    }
}

/// Run registered bun:test suites and print results. Returns (passed, failed).
///
/// # Safety
/// Caller must ensure `raw` is a valid JSContext pointer with an active request.
pub unsafe fn run_bun_tests(raw: *mut JSContext) -> (u32, u32) {
    let r = run_bun_tests_report(raw);
    (r.passed, r.failed)
}

/// A single failing test entry extracted from the JS shim.
#[derive(Debug, Clone, Default)]
pub struct TestFailure {
    pub name: String,
    pub message: String,
    pub stack: String,
}

/// Full report of a test run: counters plus the per-test names/failures.
#[derive(Debug, Clone, Default)]
pub struct TestReport {
    pub passed: u32,
    pub failed: u32,
    pub passes: Vec<String>,
    pub failures: Vec<TestFailure>,
}

/// Run registered bun:test suites and extract a full report (counters + named
/// passes/failures). The CLI layer renders the ✓/✗ output.
///
/// # Safety
/// Caller must ensure `raw` is a valid JSContext pointer with an active request.
pub unsafe fn run_bun_tests_report(raw: *mut JSContext) -> TestReport {
    let result = eval_shim_get_obj(raw, "globalThis.__run_bun_tests()");
    if result.is_null() {
        log::warn!("__run_bun_tests() returned null");
        return TestReport::default();
    }

    let obj_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &result,
    };

    let mut passed: u32 = 0;
    let mut failed: u32 = 0;

    let mut p_val = UndefinedValue();
    JS_GetProperty(
        raw,
        obj_h,
        c"passed".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut p_val,
        },
    );
    if p_val.is_int32() {
        passed = p_val.to_int32() as u32;
    }

    let mut f_val = UndefinedValue();
    JS_GetProperty(
        raw,
        obj_h,
        c"failed".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut f_val,
        },
    );
    if f_val.is_int32() {
        failed = f_val.to_int32() as u32;
    }

    let passes = read_string_array(raw, obj_h, c"passes".as_ptr());
    let failures = read_failure_array(raw, obj_h, c"failures".as_ptr());

    TestReport { passed, failed, passes, failures }
}

unsafe fn read_string_array(raw: *mut JSContext, obj_h: Handle<*mut JSObject>, key: *const i8) -> Vec<String> {
    let mut arr_val = UndefinedValue();
    JS_GetProperty(
        raw,
        obj_h,
        key,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut arr_val,
        },
    );
    if !arr_val.is_object() {
        return Vec::new();
    }
    let arr_obj = arr_val.to_object();
    let arr_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &arr_obj,
    };

    let mut len_val = UndefinedValue();
    JS_GetProperty(
        raw,
        arr_h,
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { 0 };

    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let mut elem = UndefinedValue();
        JS_GetElement(
            raw,
            arr_h,
            i as u32,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        out.push(crate::js_to_rust_string(raw, elem));
    }
    out
}

unsafe fn read_failure_array(raw: *mut JSContext, obj_h: Handle<*mut JSObject>, key: *const i8) -> Vec<TestFailure> {
    let mut arr_val = UndefinedValue();
    JS_GetProperty(
        raw,
        obj_h,
        key,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut arr_val,
        },
    );
    if !arr_val.is_object() {
        return Vec::new();
    }
    let arr_obj = arr_val.to_object();
    let arr_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &arr_obj,
    };

    let mut len_val = UndefinedValue();
    JS_GetProperty(
        raw,
        arr_h,
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { 0 };

    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let mut elem = UndefinedValue();
        JS_GetElement(
            raw,
            arr_h,
            i as u32,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        if !elem.is_object() {
            continue;
        }
        let elem_obj = elem.to_object();
        let elem_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &elem_obj,
        };
        out.push(TestFailure {
            name: read_obj_string(raw, elem_h, c"name".as_ptr()),
            message: read_obj_string(raw, elem_h, c"message".as_ptr()),
            stack: read_obj_string(raw, elem_h, c"stack".as_ptr()),
        });
    }
    out
}

unsafe fn read_obj_string(raw: *mut JSContext, obj_h: Handle<*mut JSObject>, key: *const i8) -> String {
    let mut v = UndefinedValue();
    JS_GetProperty(
        raw,
        obj_h,
        key,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    crate::js_to_rust_string(raw, v)
}
