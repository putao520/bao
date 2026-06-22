// @trace REQ-ENG-007 [api:node util/types module]
//
// Type-checking functions from Node.js `util.types`. In Bun these live in
// the main util.ts file (~338 lines). For Bao we implement them as an
// embedded JS IIFE since they are pure JS type checks.

use bun_core::ZBox;
use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use crate::require::cache_builtin;

const UTIL_TYPES_JS: &str = r#"
(function() {
  function isAnyArrayBuffer(v) { return v instanceof ArrayBuffer || v instanceof SharedArrayBuffer; }
  function isArgumentsObject(v) { return Object.prototype.toString.call(v) === '[object Arguments]'; }
  function isArrayBuffer(v) { return v instanceof ArrayBuffer; }
  function isArrayBufferView(v) { return ArrayBuffer.isView(v); }
  function isAsyncFunction(v) { return Object.prototype.toString.call(v) === '[object AsyncFunction]'; }
  function isBigInt64Array(v) { return v instanceof BigInt64Array; }
  function isBigUint64Array(v) { return v instanceof BigUint64Array; }
  function isBooleanObject(v) { return Object.prototype.toString.call(v) === '[object Boolean]'; }
  function isBoxedPrimitive(v) {
    var s = Object.prototype.toString.call(v);
    return s === '[object Boolean]' || s === '[object Number]' || s === '[object String]' || s === '[object Symbol]' || s === '[object BigInt]';
  }
  function isDataView(v) { return v instanceof DataView; }
  function isDate(v) { return v instanceof Date; }
  function isFloat32Array(v) { return v instanceof Float32Array; }
  function isFloat64Array(v) { return v instanceof Float64Array; }
  function isGeneratorFunction(v) { return Object.prototype.toString.call(v) === '[object GeneratorFunction]'; }
  function isGeneratorObject(v) { return Object.prototype.toString.call(v) === '[object Generator]'; }
  function isInt8Array(v) { return v instanceof Int8Array; }
  function isInt16Array(v) { return v instanceof Int16Array; }
  function isInt32Array(v) { return v instanceof Int32Array; }
  function isMap(v) { return v instanceof Map; }
  function isMapIterator(v) { return Object.prototype.toString.call(v) === '[object Map Iterator]'; }
  function isModuleNamespaceObject(v) { return Object.prototype.toString.call(v) === '[object Module]'; }
  function isNativeError(v) { return v instanceof Error; }
  function isNumberObject(v) { return Object.prototype.toString.call(v) === '[object Number]'; }
  function isPromise(v) { return v instanceof Promise; }
  function isProxy(v) { try { if (v === null || v === undefined) return false; return !Object.isExtensible(v) && typeof v === 'object'; } catch(e) { return false; } }
  function isRegExp(v) { return v instanceof RegExp; }
  function isSet(v) { return v instanceof Set; }
  function isSetIterator(v) { return Object.prototype.toString.call(v) === '[object Set Iterator]'; }
  function isSharedArrayBuffer(v) { return typeof SharedArrayBuffer !== 'undefined' && v instanceof SharedArrayBuffer; }
  function isStringObject(v) { return Object.prototype.toString.call(v) === '[object String]'; }
  function isSymbolObject(v) { return Object.prototype.toString.call(v) === '[object Symbol]'; }
  function isTypedArray(v) { return ArrayBuffer.isView(v) && !(v instanceof DataView); }
  function isUint8Array(v) { return v instanceof Uint8Array; }
  function isUint8ClampedArray(v) { return v instanceof Uint8ClampedArray; }
  function isUint16Array(v) { return v instanceof Uint16Array; }
  function isUint32Array(v) { return v instanceof Uint32Array; }
  function isWeakMap(v) { return v instanceof WeakMap; }
  function isWeakSet(v) { return v instanceof WeakSet; }
  function isKeyObject(v) { return false; }
  function isCryptoKey(v) { return false; }
  function isWebAssemblyCompiledModule(v) { return v instanceof WebAssembly.Module; }

  return {
    isAnyArrayBuffer: isAnyArrayBuffer,
    isArgumentsObject: isArgumentsObject,
    isArrayBuffer: isArrayBuffer,
    isArrayBufferView: isArrayBufferView,
    isAsyncFunction: isAsyncFunction,
    isBigInt64Array: isBigInt64Array,
    isBigUint64Array: isBigUint64Array,
    isBooleanObject: isBooleanObject,
    isBoxedPrimitive: isBoxedPrimitive,
    isDataView: isDataView,
    isDate: isDate,
    isFloat32Array: isFloat32Array,
    isFloat64Array: isFloat64Array,
    isGeneratorFunction: isGeneratorFunction,
    isGeneratorObject: isGeneratorObject,
    isInt8Array: isInt8Array,
    isInt16Array: isInt16Array,
    isInt32Array: isInt32Array,
    isMap: isMap,
    isMapIterator: isMapIterator,
    isModuleNamespaceObject: isModuleNamespaceObject,
    isNativeError: isNativeError,
    isNumberObject: isNumberObject,
    isPromise: isPromise,
    isProxy: isProxy,
    isRegExp: isRegExp,
    isSet: isSet,
    isSetIterator: isSetIterator,
    isSharedArrayBuffer: isSharedArrayBuffer,
    isStringObject: isStringObject,
    isSymbolObject: isSymbolObject,
    isTypedArray: isTypedArray,
    isUint8Array: isUint8Array,
    isUint8ClampedArray: isUint8ClampedArray,
    isUint16Array: isUint16Array,
    isUint32Array: isUint32Array,
    isWeakMap: isWeakMap,
    isWeakSet: isWeakSet,
    isKeyObject: isKeyObject,
    isCryptoKey: isCryptoKey,
    isWebAssemblyCompiledModule: isWebAssemblyCompiledModule,
  };
})()
"#;

/// Install util/types module.
pub fn install(cx: &mut mozjs::context::JSContext) {
    // Guard: never clobber a natively-implemented module.
    let cache_key = "builtin:util/types";
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes("<util/types>".as_bytes());
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(UTIL_TYPES_JS);
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
        cache_builtin(cx, "util/types", exports_obj);
    }
}
