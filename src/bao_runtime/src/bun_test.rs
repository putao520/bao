// bun:test + harness compatibility shims for Bun upstream test compat
use ::std::ffi::CString;
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
    } catch (e) {
      for (var k = 0; k < _afterEachFns.length; k++) {
        try { _afterEachFns[k](); } catch (_) {}
      }
      _failed++;
      _errors.push({ name: name, error: e });
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
      toThrow: function() {
        var threw = false;
        try {
          actual();
        } catch (err) {
          threw = true;
        }
        if (!threw) {
          throw new Error("Expected function to throw");
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
        toThrow: function() {
          var threw = false;
          try { actual(); } catch (_) { threw = true; }
          if (threw) {
            throw new Error("Expected function not to throw");
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
        }
      }
    };
    return e;
  }

  var expectFn = function(actual) { return _makeExpect(actual); };
  expectFn.extend = function(actual) { return _makeExpect(actual); };

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
    try {
      fn();
      _failed++;
      _errors.push({ name: name, error: new Error("Expected test to fail but it passed") });
    } catch (e) {
      _passed++; // Expected to fail, so it's a pass
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
    jest: { fn: function(impl) { return impl || function(){}; }, spyOn: function() { return { mockImplementation: function(){} }; } },
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
      try { _beforeAllFns[i](); } catch (e) { _errors.push({ name: "beforeAll", error: e }); }
    }
    for (var s = 0; s < _suites.length; s++) {
      _currentDescribe = _suites[s].name;
      try { _suites[s].fn(); } catch (e) {
        _failed++;
        _errors.push({ name: _suites[s].name, error: e });
      }
      _currentDescribe = null;
    }
    for (var j = 0; j < _afterAllFns.length; j++) {
      try { _afterAllFns[j](); } catch (e) { _errors.push({ name: "afterAll", error: e }); }
    }
    return { passed: _passed, failed: _failed, errors: _errors };
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
    let c_filename = CString::new(format!("<{}-shim>", label)).unwrap_or_default();
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
    let c_filename = CString::new("<shim-get>").unwrap_or_default();
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
pub unsafe fn run_bun_tests(raw: *mut JSContext) -> (u32, u32) {
    let result = eval_shim_get_obj(raw, "globalThis.__run_bun_tests()");
    if result.is_null() {
        log::warn!("__run_bun_tests() returned null");
        return (0, 0);
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

    (passed, failed)
}
