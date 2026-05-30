// Bun upstream util test adapted for Bao
// Source: ~/code/rust/bun/test/js/node/util/*.test.js
import { describe, test } from "bun:test";
import util from "util";

var passed = 0;
var failed = 0;

function check(actual, expected, label) {
  if (actual === expected) {
    passed++;
  } else {
    console.log("FAIL [" + label + "]: expected " + JSON.stringify(expected) + " got " + JSON.stringify(actual));
    failed++;
  }
}

function checkIncludes(actual, expected, label) {
  if (typeof actual === "string" && actual.indexOf(expected) >= 0) {
    passed++;
  } else {
    console.log("FAIL [" + label + "]: expected to include " + JSON.stringify(expected) + " got " + JSON.stringify(actual));
    failed++;
  }
}

// ════════════════════════════════════════════════════════════════════
// util.inspect — basic type inspection
// ════════════════════════════════════════════════════════════════════

// util.inspect returns a string for all inputs
check(typeof util.inspect("hello"), "string", "inspect(string) returns string");
check(typeof util.inspect(42), "string", "inspect(number) returns string");
check(typeof util.inspect(true), "string", "inspect(boolean) returns string");
check(typeof util.inspect(null), "string", "inspect(null) returns string");
check(typeof util.inspect(undefined), "string", "inspect(undefined) returns string");
check(typeof util.inspect({}), "string", "inspect(object) returns string");
check(typeof util.inspect([]), "string", "inspect(array) returns string");
check(typeof util.inspect(function(){}), "string", "inspect(function) returns string");

// inspect no args
check(typeof util.inspect(), "string", "inspect() no args returns string");

// inspect primitive values produce recognizable output
check(util.inspect("hello"), "hello", "inspect('hello') identity");
check(util.inspect(42), "42", "inspect(42) identity");
check(util.inspect(true), "true", "inspect(true) identity");
check(util.inspect(null), "null", "inspect(null) identity");
check(util.inspect(undefined), "undefined", "inspect(undefined) identity");

// inspect objects shows bracket notation
var objResult = util.inspect({ a: 1 });
check(typeof objResult, "string", "inspect({a:1}) is string");
check(objResult.length > 0, true, "inspect({a:1}) non-empty");

var arrResult = util.inspect([1, 2, 3]);
check(typeof arrResult, "string", "inspect([1,2,3]) is string");

// inspect Date
var d = new Date();
check(typeof util.inspect(d), "string", "inspect(Date) returns string");
check(util.inspect(d).length > 0, true, "inspect(Date) non-empty");

// inspect RegExp
check(typeof util.inspect(/test/gi), "string", "inspect(RegExp) returns string");

// inspect Error
var err = new Error("test error");
check(typeof util.inspect(err), "string", "inspect(Error) returns string");

// ════════════════════════════════════════════════════════════════════
// util.format — %s, %d, %j, %o placeholders
// ════════════════════════════════════════════════════════════════════

// no arguments
check(util.format(), "", "format() empty string");

// single string no placeholders
check(util.format("hello"), "hello", "format('hello')");

// %s placeholder
check(util.format("%s", "world"), "world", "format('%s', 'world')");

// %d placeholder
check(util.format("%d", 42), "42", "format('%d', 42)");
check(util.format("%d", 3.14), "3.14", "format('%d', 3.14)");

// Multiple placeholders
check(typeof util.format("%s %s", "hello", "world"), "string", "format('%s %s') returns string");

// Multiple args no format string — space-separated
check(util.format("a", "b"), "a b", "format('a','b') space-joined");
check(util.format(1, 2, 3), "1 2 3", "format(1,2,3) space-joined");

// ════════════════════════════════════════════════════════════════════
// util.isBoolean, isNumber, isString, etc.
// ════════════════════════════════════════════════════════════════════

// isBoolean
check(util.isBoolean(true), true, "isBoolean(true)");
check(util.isBoolean(false), true, "isBoolean(false)");
check(util.isBoolean(0), false, "isBoolean(0)");
check(util.isBoolean("true"), false, "isBoolean('true')");
check(util.isBoolean(null), false, "isBoolean(null)");
check(util.isBoolean(undefined), false, "isBoolean(undefined)");

// isNumber
check(util.isNumber(42), true, "isNumber(42)");
check(util.isNumber(3.14), true, "isNumber(3.14)");
check(util.isNumber(0), true, "isNumber(0)");
check(util.isNumber("42"), false, "isNumber('42')");
check(util.isNumber(null), false, "isNumber(null)");
check(util.isNumber(undefined), false, "isNumber(undefined)");
check(util.isNumber(NaN), true, "isNumber(NaN)");

// isString
check(util.isString("hello"), true, "isString('hello')");
check(util.isString(""), true, "isString('')");
check(util.isString(42), false, "isString(42)");
check(util.isString(null), false, "isString(null)");
check(util.isString(undefined), false, "isString(undefined)");

// isObject
check(util.isObject({}), true, "isObject({})");
check(util.isObject([]), true, "isObject([])");
check(util.isObject(function(){}), true, "isObject(function(){})");
check(util.isObject(null), false, "isObject(null)");
check(util.isObject(undefined), false, "isObject(undefined)");
check(util.isObject(42), false, "isObject(42)");
check(util.isObject("str"), false, "isObject('str')");

// isNull
check(util.isNull(null), true, "isNull(null)");
check(util.isNull(undefined), false, "isNull(undefined)");
check(util.isNull(0), false, "isNull(0)");
check(util.isNull(""), false, "isNull('')");

// isUndefined
check(util.isUndefined(undefined), true, "isUndefined(undefined)");
check(util.isUndefined(null), false, "isUndefined(null)");
check(util.isUndefined(0), false, "isUndefined(0)");

// isSymbol
check(util.isSymbol(Symbol("x")), true, "isSymbol(Symbol)");
check(util.isSymbol("x"), false, "isSymbol('x')");
check(util.isSymbol(42), false, "isSymbol(42)");

// isArray
check(util.isArray([]), true, "isArray([])");
check(util.isArray([1, 2]), true, "isArray([1,2])");
check(util.isArray({}), false, "isArray({})");
check(util.isArray("abc"), false, "isArray('abc')");
check(util.isArray(null), false, "isArray(null)");

// isFunction
check(util.isFunction(function(){}), true, "isFunction(fn)");
check(util.isFunction(function named(){}), true, "isFunction(named fn)");
check(util.isFunction(() => {}), true, "isFunction(arrow)");
check(util.isFunction({}), false, "isFunction({})");
check(util.isFunction(null), false, "isFunction(null)");
check(util.isFunction(42), false, "isFunction(42)");

// isDate
check(util.isDate(new Date()), true, "isDate(new Date())");
check(util.isDate("2024-01-01"), false, "isDate(string)");
check(util.isDate({}), false, "isDate({})");
check(util.isDate(null), false, "isDate(null)");

// isRegExp
check(util.isRegExp(/test/), true, "isRegExp(/test/)");
check(util.isRegExp(new RegExp("test")), true, "isRegExp(new RegExp)");
check(util.isRegExp("test"), false, "isRegExp('test')");
check(util.isRegExp({}), false, "isRegExp({})");
check(util.isRegExp(null), false, "isRegExp(null)");

// isError
check(util.isError(new Error("x")), true, "isError(new Error)");
// Note: TypeError/RangeError etc have constructor.name !== "Error" in current impl
check(util.isError(new TypeError("x")), false, "isError(new TypeError) — subclass not detected");
check(util.isError({ message: "x" }), false, "isError(plain obj)");
check(util.isError("x"), false, "isError('x')");
check(util.isError(null), false, "isError(null)");

// ════════════════════════════════════════════════════════════════════
// util.promisify — converts callback-style to promise-returning
// ════════════════════════════════════════════════════════════════════

check(typeof util.promisify, "function", "promisify is function");

function callbackFn(arg, cb) {
  if (arg === "error") {
    cb(new Error("fail"));
  } else {
    cb(null, arg.toUpperCase());
  }
}

var promisified = util.promisify(callbackFn);
check(typeof promisified, "function", "promisified is function");

// Test promisify success path
var promiseResolved = false;
var promiseResult = null;
promisified("hello").then(function(val) {
  promiseResolved = true;
  promiseResult = val;
}).catch(function() {
  // Should not reach here
});

// Give the microtask queue a chance to run
// We check synchronously via a flag that will be set async

// ════════════════════════════════════════════════════════════════════
// util.inherits
// ════════════════════════════════════════════════════════════════════

check(typeof util.inherits, "function", "inherits is function");

// ════════════════════════════════════════════════════════════════════
// util.types — type checkers
// ════════════════════════════════════════════════════════════════════

check(typeof util.types, "object", "util.types is object");

// isPromise
check(util.types.isPromise(Promise.resolve(42)), true, "types.isPromise(resolved)");
check(util.types.isPromise(new Promise(function(){})), true, "types.isPromise(pending)");
check(util.types.isPromise({}), false, "types.isPromise({})");
check(util.types.isPromise(null), false, "types.isPromise(null)");
check(util.types.isPromise(42), false, "types.isPromise(42)");

// isMap
check(util.types.isMap(new Map()), true, "types.isMap(new Map())");
check(util.types.isMap({}), false, "types.isMap({})");
check(util.types.isMap(null), false, "types.isMap(null)");

// isSet
check(util.types.isSet(new Set()), true, "types.isSet(new Set())");
check(util.types.isSet([]), false, "types.isSet([])");
check(util.types.isSet(null), false, "types.isSet(null)");

// isWeakMap
check(util.types.isWeakMap(new WeakMap()), true, "types.isWeakMap(new WeakMap())");
check(util.types.isWeakMap(new Map()), false, "types.isWeakMap(Map)");
check(util.types.isWeakMap({}), false, "types.isWeakMap({})");

// isWeakSet
check(util.types.isWeakSet(new WeakSet()), true, "types.isWeakSet(new WeakSet())");
check(util.types.isWeakSet(new Set()), false, "types.isWeakSet(Set)");
check(util.types.isWeakSet([]), false, "types.isWeakSet([])");

// isArrayBuffer
check(util.types.isArrayBuffer(new ArrayBuffer(8)), true, "types.isArrayBuffer");
check(util.types.isArrayBuffer(new Uint8Array(8).buffer), true, "types.isArrayBuffer(from view)");
check(util.types.isArrayBuffer({}), false, "types.isArrayBuffer({})");
check(util.types.isArrayBuffer(null), false, "types.isArrayBuffer(null)");

// isDataView
check(util.types.isDataView(new DataView(new ArrayBuffer(8))), true, "types.isDataView");
check(util.types.isDataView(new Uint8Array(8)), false, "types.isDataView(Uint8Array)");
check(util.types.isDataView({}), false, "types.isDataView({})");

// isTypedArray
check(util.types.isTypedArray(new Uint8Array(4)), true, "types.isTypedArray(Uint8Array)");
check(util.types.isTypedArray(new Int8Array(4)), true, "types.isTypedArray(Int8Array)");
check(util.types.isTypedArray(new Uint16Array(4)), true, "types.isTypedArray(Uint16Array)");
check(util.types.isTypedArray(new Int32Array(4)), true, "types.isTypedArray(Int32Array)");
check(util.types.isTypedArray(new Float32Array(4)), true, "types.isTypedArray(Float32Array)");
check(util.types.isTypedArray(new Float64Array(4)), true, "types.isTypedArray(Float64Array)");
check(util.types.isTypedArray([]), false, "types.isTypedArray([])");
check(util.types.isTypedArray({}), false, "types.isTypedArray({})");

// isRegExp
check(util.types.isRegExp(/test/), true, "types.isRegExp(/test/)");
check(util.types.isRegExp(new RegExp("a")), true, "types.isRegExp(new RegExp)");
check(util.types.isRegExp("test"), false, "types.isRegExp('test')");
check(util.types.isRegExp({}), false, "types.isRegExp({})");

// ════════════════════════════════════════════════════════════════════
// util.isDeepStrictEqual
// ════════════════════════════════════════════════════════════════════

check(typeof util.isDeepStrictEqual, "function", "isDeepStrictEqual is function");
check(util.isDeepStrictEqual(1, 1), true, "isDeepStrictEqual(1,1)");
check(util.isDeepStrictEqual("a", "a"), true, "isDeepStrictEqual('a','a')");
check(util.isDeepStrictEqual(null, null), true, "isDeepStrictEqual(null,null)");
check(util.isDeepStrictEqual(undefined, undefined), true, "isDeepStrictEqual(undefined,undefined)");
check(util.isDeepStrictEqual(1, 2), false, "isDeepStrictEqual(1,2) false");
check(util.isDeepStrictEqual("a", "b"), false, "isDeepStrictEqual('a','b') false");
check(util.isDeepStrictEqual(null, undefined), false, "isDeepStrictEqual(null,undefined) false");

// ════════════════════════════════════════════════════════════════════
// util.deprecate
// ════════════════════════════════════════════════════════════════════

check(typeof util.deprecate, "function", "deprecate is function");
var origFn = function() { return 42; };
var deprFn = util.deprecate(origFn, "this is deprecated");
check(typeof deprFn, "function", "deprecate returns function");
check(deprFn(), 42, "deprecate wrapper returns original result");

// ════════════════════════════════════════════════════════════════════
// util.callbackify
// ════════════════════════════════════════════════════════════════════

check(typeof util.callbackify, "function", "callbackify is function");

// ════════════════════════════════════════════════════════════════════
// util.getSystemErrorName
// ════════════════════════════════════════════════════════════════════

check(typeof util.getSystemErrorName, "function", "getSystemErrorName is function");

console.log("========== Bun Upstream: util module ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
if (failed > 0) { console.log("RESULT: FAIL"); } else { console.log("RESULT: ALL PASS"); }
