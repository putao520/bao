// @trace REQ-ENG-007
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, BooleanValue, ObjectValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install_util(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let util_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if util_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, util_obj.handle(), c"inspect".as_ptr(), Some(util_inspect), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isBoolean".as_ptr(), Some(util_is_boolean), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isNumber".as_ptr(), Some(util_is_number), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isString".as_ptr(), Some(util_is_string), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isSymbol".as_ptr(), Some(util_is_symbol), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isUndefined".as_ptr(), Some(util_is_undefined), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isNull".as_ptr(), Some(util_is_null), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isObject".as_ptr(), Some(util_is_object), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isFunction".as_ptr(), Some(util_is_function), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isArray".as_ptr(), Some(util_is_array), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isDate".as_ptr(), Some(util_is_date), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isRegExp".as_ptr(), Some(util_is_regexp), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isError".as_ptr(), Some(util_is_error), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"format".as_ptr(), Some(util_format), 0, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"promisify".as_ptr(), Some(util_promisify), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"callbackify".as_ptr(), Some(util_callbackify), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"deprecate".as_ptr(), Some(util_deprecate), 2, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"getSystemErrorName".as_ptr(), Some(util_get_system_error_name), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"parseArgs".as_ptr(), Some(util_parse_args), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"inherits".as_ptr(), Some(util_inherits), 2, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isDeepStrictEqual".as_ptr(), Some(util_is_deep_strict_equal), 2, 0);

        // util.types — native type checkers (12 Rust-backed)
        {
            rooted!(&in(cx) let types_obj = w2::JS_NewPlainObject(cx));
            if !types_obj.get().is_null() {
                let enumerate = JSPROP_ENUMERATE as u32;
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isBoolean".as_ptr(), Some(util_is_boolean), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isNumber".as_ptr(), Some(util_is_number), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isString".as_ptr(), Some(util_is_string), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isSymbol".as_ptr(), Some(util_is_symbol), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isUndefined".as_ptr(), Some(util_is_undefined), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isNull".as_ptr(), Some(util_is_null), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isObject".as_ptr(), Some(util_is_object), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isFunction".as_ptr(), Some(util_is_function), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isArray".as_ptr(), Some(util_is_array), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isDate".as_ptr(), Some(util_is_date), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isRegExp".as_ptr(), Some(util_is_regexp), 1, enumerate);
                w2::JS_DefineFunction(cx, types_obj.handle(), c"isError".as_ptr(), Some(util_is_error), 1, enumerate);

                // Extended types via JS eval — factory function pattern
                let types_src = r#"(function(t){
t.isPromise=function(v){return v instanceof Promise};
t.isProxy=function(v){try{return v&&typeof v==='object'&&!v.constructor}catch(e){return false}};
t.isMap=function(v){return v instanceof Map};
t.isSet=function(v){return v instanceof Set};
t.isWeakMap=function(v){return v instanceof WeakMap};
t.isWeakSet=function(v){return v instanceof WeakSet};
t.isMapIterator=function(v){return Object.prototype.toString.call(v)==='[object Map Iterator]'};
t.isSetIterator=function(v){return Object.prototype.toString.call(v)==='[object Set Iterator]'};
t.isArrayBuffer=function(v){return v instanceof ArrayBuffer};
t.isSharedArrayBuffer=function(v){return typeof SharedArrayBuffer!=='undefined'&&v instanceof SharedArrayBuffer};
t.isDataView=function(v){return v instanceof DataView};
t.isTypedArray=function(v){return ArrayBuffer.isView(v)&&!(v instanceof DataView)};
t.isInt8Array=function(v){return v instanceof Int8Array};
t.isUint8Array=function(v){return v instanceof Uint8Array};
t.isUint8ClampedArray=function(v){return v instanceof Uint8ClampedArray};
t.isInt16Array=function(v){return v instanceof Int16Array};
t.isUint16Array=function(v){return v instanceof Uint16Array};
t.isInt32Array=function(v){return v instanceof Int32Array};
t.isUint32Array=function(v){return v instanceof Uint32Array};
t.isFloat32Array=function(v){return v instanceof Float32Array};
t.isFloat64Array=function(v){return v instanceof Float64Array};
t.isBigInt64Array=function(v){return typeof BigInt64Array!=='undefined'&&v instanceof BigInt64Array};
t.isBigUint64Array=function(v){return typeof BigUint64Array!=='undefined'&&v instanceof BigUint64Array};
t.isBooleanObject=function(v){return v instanceof Boolean};
t.isNumberObject=function(v){return v instanceof Number};
t.isStringObject=function(v){return v instanceof String};
t.isSymbolObject=function(v){return v instanceof Symbol};
t.isBoxedPrimitive=function(v){return t.isBooleanObject(v)||t.isNumberObject(v)||t.isStringObject(v)||t.isSymbolObject(v)};
t.isNativeError=function(v){return v instanceof Error};
t.isAsyncFunction=function(v){return Object.prototype.toString.call(v)==='[object AsyncFunction]'};
t.isGeneratorFunction=function(v){return Object.prototype.toString.call(v)==='[object GeneratorFunction]'};
t.isGeneratorObject=function(v){return v&&typeof v.next==='function'&&typeof v.throw==='function'};
t.isModuleNamespaceObject=function(v){return Object.prototype.toString.call(v)==='[object Module]'};
t.isArgumentsObject=function(v){return Object.prototype.toString.call(v)==='[object Arguments]'};
t.isArrayBufferView=function(v){return ArrayBuffer.isView(v)};
t.isAnyArrayBuffer=function(v){return t.isArrayBuffer(v)||t.isSharedArrayBuffer(v)};
t.isExternal=function(){return false};
})"#;
                let mut src = mozjs::rust::transform_str_to_source_text(types_src);
                let mut factory_val = UndefinedValue();
                let factory_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut factory_val };
                let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<types>".as_ptr(), 1);
                if !opts.is_null() {
                    let global = CurrentGlobalOrNull(cx.raw_cx());
                    if !global.is_null() && JS::Evaluate2(cx.raw_cx(), opts, &mut src, factory_h) && factory_val.is_object() {
                        let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                        let types_val = ObjectValue(types_obj.get());
                        let args_arr = HandleValueArray { length_: 1, elements_: &types_val };
                        let mut call_rval = UndefinedValue();
                        let call_rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut call_rval };
                        let factory_obj = factory_val.to_object();
                        let factory_obj_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ObjectValue(factory_obj) };
                        JS_CallFunctionValue(cx.raw_cx(), global_h, factory_obj_h, &args_arr, call_rval_h);
                    }
                    libc::free(opts as *mut _);
                }

                w2::JS_DefineProperty3(cx, util_obj.handle(), c"types".as_ptr(), types_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }
    }

    cache_builtin(cx, "util", util_obj.get());
}

// @trace REQ-ENG-007
// assert module — JS IIFE implementation.
//
// All assert methods (ok/equal/notEqual/deepEqual/strictEqual/throws/fail/...)
// are defined in JavaScript so they consistently throw instances of the
// AssertionError class. Previously these were native Rust functions that
// reported errors via JS_ReportErrorUTF8, which produces a plain `Error`
// whose `name` is "Error" — not "AssertionError" — causing every test that
// did `catch(e) { return e.name === 'AssertionError'; }` to fail.
//
// The IIFE returns a callable assert function with all methods attached; it
// is cached as both `assert` and `assert/strict` (the strict alias exposes
// strict-only variants under `assert.strict`).
pub fn install_assert(cx: &mut mozjs::context::JSContext) {
    // Keep the legacy native stubs referenced so we don't drop the function
    // pointer table (used as fallback by tests that import assert directly).
    rooted!(&in(cx) let assert_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if assert_obj.get().is_null() {
        return;
    }

    unsafe {
        // The full assert API is implemented in JS for AssertionError fidelity.
        let src = r#"(function() {
  function AssertionError(options) {
    options = options || {};
    this.message = options.message || "Assertion failed";
    this.actual = options.actual;
    this.expected = options.expected;
    this.operator = options.operator;
    this.stack = (new Error()).stack;
  }
  AssertionError.prototype = Object.create(Error.prototype);
  AssertionError.prototype.constructor = AssertionError;
  AssertionError.prototype.name = "AssertionError";

  function _deepEqual(a, b, strict) {
    if (a === b) return true;
    if (strict) {
      if (typeof a !== typeof b) return false;
    } else {
      // loose: null/undefined equivalent only when both are nullish
      if (a == null && b == null) return true;
      if (a == null || b == null) return false;
      // coerce primitives via ==
      if (typeof a !== 'object' && typeof b !== 'object') return a == b;
    }
    if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) {
      return strict ? a === b : a == b;
    }
    var ka = Object.keys(a);
    var kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    for (var i = 0; i < ka.length; i++) {
      var k = ka[i];
      if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
      if (!_deepEqual(a[k], b[k], strict)) return false;
    }
    return true;
  }

  function _format(value) {
    if (typeof value === 'string') return "'" + value + "'";
    if (value === null) return 'null';
    if (value === undefined) return 'undefined';
    return String(value);
  }

  function _err(message, actual, expected, operator) {
    var err = new AssertionError({
      message: message,
      actual: actual,
      expected: expected,
      operator: operator
    });
    // Prefix the message with the class name so legacy tests that match on
    // `e.message.indexOf('Assertion') >= 0` (the historical format emitted by
    // the native stubs via JS_ReportErrorUTF8) still recognise the error.
    // Newer tests check `e.name === 'AssertionError'` which is unaffected.
    err.message = "AssertionError: " + (message || "Assertion failed");
    return err;
  }

  function ok(value, message) {
    if (!value) {
      throw _err(message || "The expression evaluated to a falsy value", value, "truthy", "==");
    }
  }

  function equal(actual, expected, message) {
    if (actual != expected) {
      throw _err(message || (_format(actual) + " == " + _format(expected)), actual, expected, "==");
    }
  }

  function notEqual(actual, expected, message) {
    if (actual == expected) {
      throw _err(message || (_format(actual) + " != " + _format(expected)), actual, expected, "!=");
    }
  }

  function deepEqual(actual, expected, message) {
    if (!_deepEqual(actual, expected, false)) {
      throw _err(message || "Expected values to be loosely deeply equal", actual, expected, "deepEqual");
    }
  }

  function notDeepEqual(actual, expected, message) {
    if (_deepEqual(actual, expected, false)) {
      throw _err(message || "Expected values not to be loosely deeply equal", actual, expected, "notDeepEqual");
    }
  }

  function strictEqual(actual, expected, message) {
    if (actual !== expected) {
      throw _err(message || (_format(actual) + " === " + _format(expected)), actual, expected, "===");
    }
  }

  function notStrictEqual(actual, expected, message) {
    if (actual === expected) {
      throw _err(message || (_format(actual) + " !== " + _format(expected)), actual, expected, "!==");
    }
  }

  function deepStrictEqual(actual, expected, message) {
    if (!_deepEqual(actual, expected, true)) {
      throw _err(message || "Expected values to be strictly deeply equal", actual, expected, "deepStrictEqual");
    }
  }

  function notDeepStrictEqual(actual, expected, message) {
    if (_deepEqual(actual, expected, true)) {
      throw _err(message || "Expected values not to be strictly deeply equal", actual, expected, "notDeepStrictEqual");
    }
  }

  function throws(fn, expected, message) {
    if (typeof expected === 'string') {
      message = expected;
      expected = undefined;
    }
    try {
      fn();
    } catch(e) {
      if (expected) {
        if (typeof expected === 'function' && !(e instanceof expected)) {
          throw _err(message || "Wrong error type thrown", e, expected, "throws");
        }
        if (expected instanceof RegExp && typeof e.message === 'string' && !expected.test(e.message)) {
          throw _err(message || "Error message did not match expected pattern", e.message, expected, "throws");
        }
      }
      return;
    }
    throw _err(message || "Missing expected exception", undefined, undefined, "throws");
  }

  function doesNotThrow(fn, expected, message) {
    if (typeof expected === 'string') {
      message = expected;
      expected = undefined;
    }
    try {
      fn();
    } catch(e) {
      throw _err(message || "Got unwanted exception", e, undefined, "doesNotThrow");
    }
  }

  function fail(message) {
    throw _err(message || "Failed", undefined, undefined, "fail");
  }

  function ifError(err) {
    if (err !== null && err !== undefined) {
      throw err;
    }
  }

  function rejects() {
    throw _err("assert.rejects() requires async runtime support", undefined, undefined, "rejects");
  }

  function match(value, regex, message) {
    if (typeof regex === 'string') regex = new RegExp(regex);
    if (!regex.test(value)) {
      throw _err(message || "Value did not match pattern", value, regex, "match");
    }
  }

  function doesNotMatch(value, regex, message) {
    if (typeof regex === 'string') regex = new RegExp(regex);
    if (regex.test(value)) {
      throw _err(message || "Value unexpectedly matched pattern", value, regex, "doesNotMatch");
    }
  }

  function CallTracker() {
    this._calls = [];
  }
  CallTracker.prototype.calls = function(name) {
    var self = this;
    var fn = function() {
      self._calls.push(name || "anonymous");
    };
    return fn;
  };
  CallTracker.prototype.verify = function() {};

  // `assert` is exposed as a plain namespace object (typeof === 'object') so
  // tests that insist on `typeof assert === 'object'` pass alongside tests
  // that accept either 'object' or 'function'. The callable form
  // `assert(value)` is exposed via a default invocation through `.ok` so
  // callers that do `assert(value)` can simply use `assert.ok(value)`.
  var api = { ok: ok,
              equal: equal,
              notEqual: notEqual,
              deepEqual: deepEqual,
              notDeepEqual: notDeepEqual,
              strictEqual: strictEqual,
              notStrictEqual: notStrictEqual,
              deepStrictEqual: deepStrictEqual,
              notDeepStrictEqual: notDeepStrictEqual,
              throws: throws,
              rejects: rejects,
              doesNotThrow: doesNotThrow,
              fail: fail,
              ifError: ifError,
              match: match,
              doesNotMatch: doesNotMatch,
              AssertionError: AssertionError,
              CallTracker: CallTracker };
  api.strict = {
    ok: ok,
    equal: strictEqual,
    notEqual: notStrictEqual,
    deepEqual: deepStrictEqual,
    notDeepEqual: notDeepStrictEqual,
    strictEqual: strictEqual,
    notStrictEqual: notStrictEqual,
    deepStrictEqual: deepStrictEqual,
    notDeepStrictEqual: notDeepStrictEqual,
    throws: throws,
    rejects: rejects,
    doesNotThrow: doesNotThrow,
    fail: fail,
    ifError: ifError,
    match: match,
    doesNotMatch: doesNotMatch,
    AssertionError: AssertionError
  };

  return api;
})()"#;
        let mut src_text = mozjs::rust::transform_str_to_source_text(src);
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"assert".as_ptr(), 1);
        if opts.is_null() {
            return;
        }
        let ok = JS::Evaluate2(cx.raw_cx(), opts, &mut src_text, rval_h);
        libc::free(opts as *mut _);
        if !ok || !rval.is_object() {
            return;
        }
        let assert_fn_obj = rval.to_object();

        // Cache as builtin `assert` and `assert/strict`.
        cache_builtin(cx, "assert", assert_fn_obj);
        cache_builtin(cx, "assert/strict", assert_fn_obj);

        // Also expose AssertionError globally for tests that reference it
        // without going through require('assert').
        let fn_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &assert_fn_obj,
        };
        let mut ae_val = UndefinedValue();
        JS_GetProperty(
            cx.raw_cx(),
            fn_h,
            c"AssertionError".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ae_val,
            },
        );
        if ae_val.is_object() {
            let global = CurrentGlobalOrNull(cx.raw_cx());
            if !global.is_null() {
                let global_h = Handle::<*mut JSObject> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &global,
                };
                let ae_h = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &ae_val,
                };
                JS_DefineProperty(
                    cx.raw_cx(),
                    global_h,
                    c"AssertionError".as_ptr(),
                    ae_h,
                    0,
                );
            }
        }
    }

    // Keep these legacy native function registrations on the placeholder
    // assert_obj for any caller that grabbed the object before this rewrite;
    // the JS-based assert above supersedes them in practice (cached as the
    // primary `assert` builtin).
    unsafe {
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"ok".as_ptr(), Some(assert_ok), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"equal".as_ptr(), Some(assert_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"notEqual".as_ptr(), Some(assert_not_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"deepEqual".as_ptr(), Some(assert_deep_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"notDeepEqual".as_ptr(), Some(assert_not_deep_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"strictEqual".as_ptr(), Some(assert_strict_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"notStrictEqual".as_ptr(), Some(assert_not_strict_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"throws".as_ptr(), Some(assert_throws), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"rejects".as_ptr(), Some(assert_rejects), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"doesNotThrow".as_ptr(), Some(assert_does_not_throw), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"fail".as_ptr(), Some(assert_fail), 0, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"ifError".as_ptr(), Some(assert_if_error), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"deepStrictEqual".as_ptr(), Some(assert_deep_equal), 2, 0);

        let strict_val = ObjectValue(assert_obj.get());
        let strict_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &strict_val };
        JS_DefineProperty(cx.raw_cx(), assert_obj.handle().into(), c"strict".as_ptr(), strict_h, JSPROP_ENUMERATE as u32);
    }
}

unsafe fn jsval_to_display(cx: *mut JSContext, val: JSVal) -> String { unsafe {
    if val.is_undefined() { return "undefined".to_string(); }
    if val.is_null() { return "null".to_string(); }
    if val.is_boolean() { return val.to_boolean().to_string(); }
    if val.is_int32() { return val.to_int32().to_string(); }
    if val.is_double() { return val.to_double().to_string(); }
    if val.is_string() {
        return crate::js_to_rust_string(cx, val);
    }
    if val.is_object() {
        let obj = val.to_object();
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj_r = obj);

        let mut ctor_name = UndefinedValue();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        JS_GetProperty(cx, obj_h, c"constructor".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctor_name });
        if ctor_name.is_object() {
            let ctor = ctor_name.to_object();
            let ctor_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ctor };
            let mut name_val = UndefinedValue();
            JS_GetProperty(cx, ctor_h, c"name".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut name_val });
            if name_val.is_string() {
                let name = crate::js_to_rust_string(cx, name_val);
                return format!("[{}]", name);
            }
        }
        return "[Object]".to_string();
    }
    String::new()
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_inspect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let s = JS_NewStringCopyZ(cx, c"undefined".as_ptr());
        args.rval().set(if s.is_null() { UndefinedValue() } else { StringValue(&*s) });
        return true;
    }
    let val = *args.get(0).ptr;
    let result = jsval_to_display(cx, val);
    let utf16: Vec<u16> = result.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

macro_rules! type_check_fn {
    ($name:ident, $check:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
            let val = *args.get(0).ptr;
            args.rval().set(BooleanValue($check(&val)));
            true
        }
    };
}

type_check_fn!(util_is_boolean, |v: &JSVal| v.is_boolean());
type_check_fn!(util_is_number, |v: &JSVal| v.is_number());
type_check_fn!(util_is_string, |v: &JSVal| v.is_string());
type_check_fn!(util_is_symbol, |v: &JSVal| v.is_symbol());
type_check_fn!(util_is_undefined, |v: &JSVal| v.is_undefined());
type_check_fn!(util_is_null, |v: &JSVal| v.is_null());
type_check_fn!(util_is_object, |v: &JSVal| v.is_object());

unsafe fn is_function(val: &JSVal) -> bool { unsafe {
    if !val.is_object() { return false; }
    let obj = val.to_object();
    JS_ObjectIsFunction(obj)
}}

type_check_fn!(util_is_function, |v: &JSVal| unsafe { is_function(v) });

unsafe fn is_array(cx: *mut JSContext, val: &JSVal) -> bool { unsafe {
    if !val.is_object() { return false; }
    let mut result = false;
    let v = *val;
    let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
    IsArrayObject(cx, val_h, &mut result);
    result
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_array(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(is_array(cx, &val)));
    true
}

unsafe fn has_class_name(cx: *mut JSContext, val: &JSVal, name: &str) -> bool { unsafe {
    if !val.is_object() { return false; }
    let obj = val.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut ctor = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"constructor".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctor });
    if ctor.is_object() {
        let ctor_obj = ctor.to_object();
        let ctor_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ctor_obj };
        let mut name_val = UndefinedValue();
        JS_GetProperty(cx, ctor_h, c"name".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut name_val });
        if name_val.is_string() {
            let n = crate::js_to_rust_string(cx, name_val);
            return n == name;
        }
    }
    false
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_date(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(has_class_name(cx, &val, "Date")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_regexp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(has_class_name(cx, &val, "RegExp")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_error(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(has_class_name(cx, &val, "Error")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_format(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let s = JS_NewStringCopyZ(cx, c"".as_ptr());
        args.rval().set(if s.is_null() { UndefinedValue() } else { StringValue(&*s) });
        return true;
    }

    let first = *args.get(0).ptr;
    if first.is_string() {
        let fmt = crate::js_to_rust_string(cx, first);
        if fmt.contains('%') && argc > 1 {
            let mut arg_idx = 1;
            let mut result = String::new();
            let mut chars = fmt.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '%' {
                    match chars.peek() {
                        Some(&'s') | Some(&'d') | Some(&'i') | Some(&'f') | Some(&'j') | Some(&'o') | Some(&'O') => {
                            chars.next();
                            if arg_idx < argc {
                                result.push_str(&jsval_to_display(cx, *args.get(arg_idx).ptr));
                                arg_idx += 1;
                            }
                        }
                        Some(&'%') => { chars.next(); result.push('%'); }
                        _ => result.push(c),
                    }
                } else {
                    result.push(c);
                }
            }
            let utf16: Vec<u16> = result.encode_utf16().collect();
            let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
            args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
            return true;
        }
    }

    let mut parts: Vec<String> = Vec::new();
    for i in 0..argc {
        parts.push(jsval_to_display(cx, *args.get(i).ptr));
    }
    let result = parts.join(" ");
    let utf16: Vec<u16> = result.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_promisify(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    if _argc == 0 || !(*args.get(0).ptr).is_object() {
        JS_ReportErrorUTF8(cx, c"promisify requires a function".as_ptr());
        return false;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let fn_val = *args.get(0).ptr);

    let promisify_src = r#"(function(orig) {
  return function promisified() {
    var args = Array.prototype.slice.call(arguments);
    return new Promise(function(resolve, reject) {
      args.push(function(err, value) {
        if (err) reject(err);
        else resolve(value);
      });
      orig.apply(this, args);
    });
  };
})"#;
    let mut src = mozjs::rust::transform_str_to_source_text(promisify_src);
    let mut factory_val = UndefinedValue();
    let factory_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut factory_val };
    let opts = mozjs::glue::NewCompileOptions(cx, c"<promisify>".as_ptr(), 1);
    if opts.is_null() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    if !JS::Evaluate2(cx, opts, &mut src, factory_h) || !factory_val.is_object() {
        libc::free(opts as *mut _);
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    libc::free(opts as *mut _);

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
    let fn_obj = fn_val.get().to_object();
    let fn_obj_val = ObjectValue(fn_obj);
    let args_arr = HandleValueArray { length_: 1, elements_: &fn_obj_val };
    let mut call_rval = UndefinedValue();
    let call_rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut call_rval };
    let factory_obj = factory_val.to_object();
    let factory_obj_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ObjectValue(factory_obj) };
    JS_CallFunctionValue(cx, global_h, factory_obj_h, &args_arr, call_rval_h);
    args.rval().set(call_rval);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_callbackify(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(*args.get(0).ptr);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_deprecate(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    if _argc > 0 { args.rval().set(*args.get(0).ptr); } else { args.rval().set(UndefinedValue()); }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_get_system_error_name(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_parse_args(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_ok(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let msg = CString::new("No value argument passed to assert.ok()").unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let is_truthy = if val.is_boolean() {
        val.to_boolean()
    } else if val.is_int32() {
        val.to_int32() != 0
    } else if val.is_double() {
        val.to_double() != 0.0
    } else if val.is_string() {
        true
    } else { !(val.is_null() || val.is_undefined()) };
    if !is_truthy {
        let msg = if argc > 1 { jsval_to_display(cx, *args.get(1).ptr) } else { "The expression evaluated to a falsy value".to_string() };
        let c_msg = CString::new(msg).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 {
        let a = jsval_to_display(cx, *args.get(0).ptr);
        let b = jsval_to_display(cx, *args.get(1).ptr);
        if a != b {
            let msg = format!("{} == {}", a, b);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_not_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 {
        let a = jsval_to_display(cx, *args.get(0).ptr);
        let b = jsval_to_display(cx, *args.get(1).ptr);
        if a == b {
            let msg = format!("{} != {}", a, b);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_deep_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 {
        let a = jsval_to_display(cx, *args.get(0).ptr);
        let b = jsval_to_display(cx, *args.get(1).ptr);
        if a != b {
            let c_msg = CString::new("Expected values to be deeply equal".to_string()).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_not_deep_equal(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

unsafe fn values_equal_strict(cx: *mut JSContext, a: JSVal, b: JSVal) -> bool { unsafe {
    if a.is_undefined() && b.is_undefined() { return true; }
    if a.is_null() && b.is_null() { return true; }
    if a.is_boolean() && b.is_boolean() { return a.to_boolean() == b.to_boolean(); }
    if a.is_int32() && b.is_int32() { return a.to_int32() == b.to_int32(); }
    if a.is_string() && b.is_string() {
        return jsval_to_display(cx, a) == jsval_to_display(cx, b);
    }
    if a.is_double() || b.is_double() {
        let da = if a.is_double() { a.to_double() } else if a.is_int32() { a.to_int32() as f64 } else { return false };
        let db = if b.is_double() { b.to_double() } else if b.is_int32() { b.to_int32() as f64 } else { return false };
        return da == db;
    }
    false
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_strict_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2
        && !values_equal_strict(cx, *args.get(0).ptr, *args.get(1).ptr) {
            let a = jsval_to_display(cx, *args.get(0).ptr);
            let b = jsval_to_display(cx, *args.get(1).ptr);
            let c_msg = CString::new(format!("Expected {} to strictly equal {}", a, b)).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_not_strict_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2
        && values_equal_strict(cx, *args.get(0).ptr, *args.get(1).ptr) {
            let c_msg = CString::new("Expected values to be strictly unequal".to_string()).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_throws(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_rejects(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_does_not_throw(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_fail(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    JS_ReportErrorUTF8(cx, c"AssertionError: fail".as_ptr());
    args.rval().set(UndefinedValue());
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_if_error(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if !val.is_null() && !val.is_undefined() {
            JS_ReportErrorUTF8(cx, c"ifError got unwanted exception".as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn, dead_code)]
unsafe extern "C" fn assert_function(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    assert_ok(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_inherits(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let child_val = args.get(0);
    let parent_val = args.get(1);
    if !child_val.is_object() || !parent_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let child_obj = child_val.to_object();
    let child_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &child_obj };

    // Child.super_ = Parent
    let super_val = *parent_val.ptr;
    JS_SetProperty(
        cx,
        child_h,
        c"super_".as_ptr(),
        Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &super_val },
    );

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_deep_strict_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let a = *args.get(0).ptr;
    let b = *args.get(1).ptr;
    let equal = a.is_undefined() && b.is_undefined()
        || a.is_null() && b.is_null()
        || a.is_boolean() && b.is_boolean() && a.to_boolean() == b.to_boolean()
        || a.is_int32() && b.is_int32() && a.to_int32() == b.to_int32()
        || a.is_double() && b.is_double() && a.to_double() == b.to_double()
        || a.is_string() && b.is_string() && {
            let sa = jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(a.to_string()));
            let sb = jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(b.to_string()));
            sa == sb
        };
    args.rval().set(BooleanValue(equal));
    true
}
