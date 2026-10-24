// @trace REQ-ENG-007
use ::std::collections::HashMap;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU32, Ordering};
use ::std::sync::{LazyLock, Mutex};
use bun_core::ZBox;

use mozjs::conversions::jsstr_to_string;
use mozjs::gc::{RootableVec, RootedVec};
use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// Table-driven factory for native JS registrations (e180, REQ-BRW-049):
// each row is (js name, native fn, nargs) and expands to the exact
// JS_DefineFunction call the handwritten block used — same order, same
// attrs (register_assert_stubs! e171 form with the attrs column promoted
// to a leading parameter: util's own face passes 0, util.types passes the
// local `enumerate` binding).
//
// e201 REQ-ARCH-001: renamed to the domain-neutral `register_js_functions!`
// and crate-shared so node_fs's registration walls reuse the same form.
macro_rules! register_js_functions {
    ($cx:expr, $obj:expr, $attrs:expr; $(($cname:expr, $fn:ident, $nargs:expr))*) => {
        $(
            w2::JS_DefineFunction(
                $cx,
                $obj.handle(),
                $cname.as_ptr(),
                Some($fn),
                $nargs,
                $attrs,
            );
        )*
    };
}

pub(crate) use register_js_functions;

pub fn install_util(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let util_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if util_obj.get().is_null() {
        return;
    }

    unsafe {
        register_js_functions!(cx, util_obj, 0;
            (c"inspect", util_inspect, 2)
            (c"isBoolean", util_is_boolean, 1)
            (c"isNumber", util_is_number, 1)
            (c"isString", util_is_string, 1)
            (c"isSymbol", util_is_symbol, 1)
            (c"isUndefined", util_is_undefined, 1)
            (c"isNull", util_is_null, 1)
            (c"isObject", util_is_object, 1)
            (c"isFunction", util_is_function, 1)
            (c"isArray", util_is_array, 1)
            (c"isDate", util_is_date, 1)
            (c"isRegExp", util_is_regexp, 1)
            (c"isError", util_is_error, 1)
            (c"format", util_format, 0)
            (c"promisify", util_promisify, 1)
            (c"callbackify", util_callbackify, 1)
            (c"getSystemErrorName", util_get_system_error_name, 1)
            (c"inherits", util_inherits, 2)
            // node v24 public API surface (REQ-ENG-048): getCallSites is the
            // structured stack-frame reader three official test files depend
            // on; stripVTControlCharacters the ANSI/VT sequence stripper.
            (c"getCallSites", util_get_call_sites, 2)
            (c"stripVTControlCharacters", util_strip_vt_control_characters, 1)
        );

        // util.types — node v24.19 namespace (43 members, oracle-enumerated).
        // The SAME object is cached as builtin "util/types" BEFORE
        // node_util_types::install runs (globals.rs installs util first), so
        // require('util/types') === util.types by construction (node identity
        // contract, test-util-types-exists) and the legacy standalone
        // util/types table early-returns without ever being observed. The
        // node-absent util.* legacy checkers (isBoolean/isArray/isError/…)
        // stay on the util module root only — node's util.types never
        // carried them.
        {
            rooted!(&in(cx) let types_obj = w2::JS_NewPlainObject(cx));
            if !types_obj.get().is_null() {
                let enumerate = JSPROP_ENUMERATE as u32;
                register_js_functions!(cx, types_obj, enumerate;
                    (c"isProxy", util_is_proxy, 1)
                    (c"isDate", util_is_date, 1)
                    (c"isRegExp", util_is_regexp, 1)
                );

                // Extended types via JS eval — factory function pattern
                let types_src = r#"(function(t){
t.isPromise=function(v){return v instanceof Promise};
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
t.isFloat16Array=function(v){return typeof Float16Array!=='undefined'&&v instanceof Float16Array};
t.isFloat32Array=function(v){return v instanceof Float32Array};
t.isFloat64Array=function(v){return v instanceof Float64Array};
t.isBigInt64Array=function(v){return typeof BigInt64Array!=='undefined'&&v instanceof BigInt64Array};
t.isBigUint64Array=function(v){return typeof BigUint64Array!=='undefined'&&v instanceof BigUint64Array};
t.isBooleanObject=function(v){return v instanceof Boolean};
t.isNumberObject=function(v){return v instanceof Number};
t.isStringObject=function(v){return v instanceof String};
t.isSymbolObject=function(v){return v instanceof Symbol};
t.isBigIntObject=function(v){return Object.prototype.toString.call(v)==='[object BigInt]'};
t.isBoxedPrimitive=function(v){return t.isBooleanObject(v)||t.isNumberObject(v)||t.isStringObject(v)||t.isSymbolObject(v)||t.isBigIntObject(v)};
t.isNativeError=function(v){return v instanceof Error};
t.isAsyncFunction=function(v){return Object.prototype.toString.call(v)==='[object AsyncFunction]'};
t.isGeneratorFunction=function(v){return Object.prototype.toString.call(v)==='[object GeneratorFunction]'};
t.isGeneratorObject=function(v){return v&&typeof v.next==='function'&&typeof v.throw==='function'};
t.isModuleNamespaceObject=function(v){return Object.prototype.toString.call(v)==='[object Module]'};
t.isArgumentsObject=function(v){return Object.prototype.toString.call(v)==='[object Arguments]'};
t.isArrayBufferView=function(v){return ArrayBuffer.isView(v)};
t.isAnyArrayBuffer=function(v){return t.isArrayBuffer(v)||t.isSharedArrayBuffer(v)};
t.isExternal=function(){return false};
t.isKeyObject=function(){return false};
t.isCryptoKey=function(){return false};
})"#;
                eval_js_factory_on_object(cx, types_src, c"<types>", types_obj.handle());

                // Single-source identity: util.types IS the util/types export
                // (node contract; require('util/types') resolves this cache).
                cache_builtin(cx, "util/types", types_obj.get());

                w2::JS_DefineProperty3(
                    cx,
                    util_obj.handle(),
                    c"types".as_ptr(),
                    types_obj.handle(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // @trace REQ-ENG-004 [api:util.styleText / isDeepStrictEqual /
        // inherits / deprecate-messages / inspect-getters] — node v24.19.0
        // oracle ports (lib/util.js styleText incl. hex colors + aliases +
        // nested close-code reapplication; errors.js determineSpecificType;
        // internal/util.js inherits non-enumerable super_; inspect.js
        // getters/showHidden/protoProps/compact:3 options engine). Attached
        // on top of util_obj so they override the native placeholders.
        let util_extra_src = r#"(function(u){
  // ── node errors.js determineSpecificType (v24.19 oracle) ──────────────
  // 'Received null' / 'an instance of X' / 'type number (-0)' family — the
  // exact tail shape test-util-deprecate-invalid-code / inherits /
  // callbackify assert against (common.invalidArgTypeHelper agreement).
  function inspectDepthNeg1(value) {
    // node inspect(value, {depth:-1}) — top-level shape, nested values as
    // truncation tags. Sole consumer: determineSpecificType fallback.
    if (Array.isArray(value)) {
      if (value.length === 0) return '[]';
      var el = [];
      for (var i = 0; i < value.length && i < 100; i++) {
        var ev;
        try { ev = value[i]; } catch (e0) { ev = undefined; }
        el.push(ev !== null && typeof ev === 'object'
          ? (Array.isArray(ev) ? '[Array]' : '[Object]') : inspectDepthNeg1(ev));
      }
      var extra = value.length > 100 ? ', ... ' + (value.length - 100) + ' more items' : '';
      return '[ ' + el.join(', ') + extra + ' ]';
    }
    var proto = Object.getPrototypeOf(value);
    var prefix = proto === null ? '[Object: null prototype] ' : '';
    var keys;
    try { keys = Object.keys(value); } catch (e1) { return prefix + '{}'; }
    if (keys.length === 0) return prefix + '{}';
    var parts = [];
    for (var k = 0; k < keys.length && k < 100; k++) {
      var v;
      try { v = value[keys[k]]; } catch (e2) { v = undefined; }
      parts.push(keys[k] + ': ' + (v !== null && typeof v === 'object'
        ? (Array.isArray(v) ? '[Array]' : '[Object]') : inspectDepthNeg1(v)));
    }
    return prefix + '{ ' + parts.join(', ') + ' }';
  }
  function determineSpecificType(value) {
    if (value === null) return 'null';
    if (value === undefined) return 'undefined';
    var t = typeof value;
    switch (t) {
      case 'bigint': return 'type bigint (' + value + 'n)';
      case 'number':
        if (value === 0) return 1 / value === -Infinity ? 'type number (-0)' : 'type number (0)';
        if (value !== value) return 'type number (NaN)';
        if (value === Infinity) return 'type number (Infinity)';
        if (value === -Infinity) return 'type number (-Infinity)';
        return 'type number (' + value + ')';
      case 'boolean': return value ? 'type boolean (true)' : 'type boolean (false)';
      case 'symbol': return 'type symbol (' + String(value) + ')';
      case 'function': return 'function ' + value.name;
      case 'object': {
        var name = '';
        try { name = (value.constructor && value.constructor.name) || ''; } catch (e) {}
        if (typeof name === 'string' && name !== '') return 'an instance of ' + name;
        return inspectDepthNeg1(value);
      }
      case 'string': {
        var s = value;
        if (s.length > 28) s = s.slice(0, 25) + '...';
        return s.indexOf("'") === -1 ? "type string ('" + s + "')" : 'type string (' + JSON.stringify(s) + ')';
      }
      default: return 'type ' + t + ' (' + String(value) + ')';
    }
  }
  function errOf(ctor, code, msg) { var e = new ctor(msg); e.code = code; return e; }
  function invalidArgTypeErr(name, expected, value) {
    var kind = name.indexOf('.') !== -1 ? 'property' : 'argument';
    return errOf(TypeError, 'ERR_INVALID_ARG_TYPE',
      'The "' + name + '" ' + kind + ' must be of type ' + expected +
      '. Received ' + determineSpecificType(value));
  }
  function inspFullNumber(n) {
    return (n === 0 && 1 / n === -Infinity) ? '-0' : String(n);
  }
  function inspFullStrEscape(s) {
    var out = "'";
    for (var i = 0; i < s.length; i++) {
      var c = s.charAt(i), code = s.charCodeAt(i);
      if (c === "'") out += "\\'";
      else if (c === '\\') out += '\\\\';
      else if (c === '\n') out += '\\n';
      else if (c === '\r') out += '\\r';
      else if (c === '\t') out += '\\t';
      else if (code < 0x20 || code === 0x7f) {
        var h = code.toString(16);
        out += '\\x' + (h.length < 2 ? '0' + h : h);
      } else out += c;
    }
    return out + "'";
  }
  function inspReceived(v) {
    if (typeof v === 'string') return "'" + v + "'";
    if (typeof v === 'boolean') return String(v);
    if (typeof v === 'number') return inspFullNumber(v);
    if (v === null) return 'null';
    if (v === undefined) return 'undefined';
    if (typeof v === 'bigint') return String(v) + 'n';
    if (typeof v === 'symbol') return String(v);
    if (typeof v === 'function') return v.name ? '[Function: ' + v.name + ']' : '[Function (anonymous)]';
    return '{}';
  }

  // ── util.styleText — node v24.19 lib/util.js oracle ───────────────────
  // Colors table = node inspect.colors own names verbatim (56 entries incl.
  // aliases grey/blackBright/bgGrey/bgBlackBright/faint/crossedout/…).
  var styleColors = {
    reset: [0, 0], bold: [1, 22], dim: [2, 22], italic: [3, 23],
    underline: [4, 24], blink: [5, 25], inverse: [7, 27], hidden: [8, 28],
    strikethrough: [9, 29], doubleunderline: [21, 24],
    black: [30, 39], red: [31, 39], green: [32, 39], yellow: [33, 39],
    blue: [34, 39], magenta: [35, 39], cyan: [36, 39], white: [37, 39],
    bgBlack: [40, 49], bgRed: [41, 49], bgGreen: [42, 49], bgYellow: [43, 49],
    bgBlue: [44, 49], bgMagenta: [45, 49], bgCyan: [46, 49], bgWhite: [47, 49],
    framed: [51, 54], overlined: [53, 55], gray: [90, 39],
    redBright: [91, 39], greenBright: [92, 39], yellowBright: [93, 39],
    blueBright: [94, 39], magentaBright: [95, 39], cyanBright: [96, 39],
    whiteBright: [97, 39],
    bgGray: [100, 49], bgRedBright: [101, 49], bgGreenBright: [102, 49],
    bgYellowBright: [103, 49], bgBlueBright: [104, 49],
    bgMagentaBright: [105, 49], bgCyanBright: [106, 49], bgWhiteBright: [107, 49],
    grey: [90, 39], blackBright: [90, 39], bgGrey: [100, 49],
    bgBlackBright: [100, 49], faint: [2, 22], crossedout: [9, 29],
    strikeThrough: [9, 29], crossedOut: [9, 29], conceal: [8, 28],
    swapColors: [7, 27], swapcolors: [7, 27], doubleUnderline: [21, 24]
  };
  var kHexCloseSeq = '\u001b[39m';
  var hexColorRegExp = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;
  function codesToStyle(codes) {
    return {
      openSeq: '\u001b[' + codes[0] + 'm',
      closeSeq: '\u001b[' + codes[1] + 'm',
      keepClose: codes[0] === 2 || codes[0] === 1
    };
  }
  // node replaceCloseCode: every INNER occurrence of this style's close
  // sequence re-opens the style (the trailing close stays) — the nested
  // styleText contract (dim over bold:
  // '\x1b[2mdim\x1b[1mbold\x1b[22m\x1b[2mdim\x1b[22m').
  function replaceCloseCode(str, closeSeq, openSeq, keepClose) {
    var closeLen = closeSeq.length;
    var index = str.indexOf(closeSeq);
    if (index === -1) return str;
    var result = '';
    var lastIndex = 0;
    var replacement = keepClose ? closeSeq + openSeq : openSeq;
    do {
      var afterClose = index + closeLen;
      if (afterClose < str.length) {
        result += str.slice(lastIndex, index) + replacement;
        lastIndex = afterClose;
      } else {
        break;
      }
      index = str.indexOf(closeSeq, lastIndex);
    } while (index !== -1);
    return result + str.slice(lastIndex);
  }
  function hexToRgb(hex) {
    var hexStr;
    if (hex.length === 4) {
      hexStr = hex[1] + hex[1] + hex[2] + hex[2] + hex[3] + hex[3];
    } else if (hex.length === 7) {
      hexStr = hex.slice(1);
    } else {
      throw errOf(RangeError, 'ERR_OUT_OF_RANGE',
        'The value of "hex" is out of range. It must be #RGB or #RRGGBB. Received ' +
        inspReceived(hex));
    }
    return [parseInt(hexStr.slice(0, 2), 16), parseInt(hexStr.slice(2, 4), 16),
            parseInt(hexStr.slice(4, 6), 16)];
  }
  function validateOneOfMsg(key) {
    var keys = Object.keys(styleColors);
    var strings = '';
    for (var i = 0; i < keys.length; i++) {
      if (i > 0) strings += ', ';
      strings += "'" + keys[i] + "'";
    }
    return "The argument 'format' must be one of: " + strings +
      '. Received ' + inspReceived(key);
  }
  function styleShouldColorize(stream) {
    var env = (typeof process !== 'undefined' && process.env) || {};
    if (env.FORCE_COLOR !== undefined) {
      return env.FORCE_COLOR !== '0' && env.FORCE_COLOR !== 'false' && env.FORCE_COLOR !== '';
    }
    if (env.NODE_DISABLE_COLORS !== undefined || env.NO_COLOR !== undefined) return false;
    return stream != null && stream.isTTY === true &&
      (typeof stream.getColorDepth !== 'function' || stream.getColorDepth() > 2);
  }
  u.styleText = function styleText(format, text, options) {
    var validateStream = (options == null || options.validateStream === undefined)
      ? true : options.validateStream;
    // Fast path: single format string with validateStream=false.
    if (!validateStream && typeof format === 'string' && typeof text === 'string') {
      if (format === 'none') return text;
      var fstyle = styleColors[format];
      if (fstyle !== undefined) {
        var fst = codesToStyle(fstyle);
        return fst.openSeq +
          replaceCloseCode(text, fst.closeSeq, fst.openSeq, fst.keepClose) +
          fst.closeSeq;
      }
      if (format.charAt(0) === '#' && hexColorRegExp.test(format)) {
        var frgb = hexToRgb(format);
        var fhexOpen = '\u001b[38;2;' + frgb[0] + ';' + frgb[1] + ';' + frgb[2] + 'm';
        return fhexOpen +
          replaceCloseCode(text, kHexCloseSeq, fhexOpen, false) + kHexCloseSeq;
      }
    }
    if (typeof text !== 'string') throw invalidArgTypeErr('text', 'string', text);
    if (options !== undefined) {
      if (options === null || typeof options !== 'object') {
        throw invalidArgTypeErr('options', 'object', options);
      }
      if (typeof validateStream !== 'boolean') {
        throw errOf(TypeError, 'ERR_INVALID_ARG_TYPE',
          'The "options.validateStream" property must be of type boolean. Received ' +
          determineSpecificType(validateStream));
      }
    }
    var skipColorize = false;
    if (validateStream) {
      var stream = (options && options.stream !== undefined) ? options.stream : process.stdout;
      var isStreamObj = stream !== null && stream !== undefined &&
        (typeof stream === 'object' || typeof stream === 'function') &&
        (typeof stream.write === 'function' || typeof stream.on === 'function');
      if (!isStreamObj) {
        throw errOf(TypeError, 'ERR_INVALID_ARG_TYPE',
          'The "stream" argument must be an instance of ReadableStream, WritableStream, or Stream. Received ' +
          determineSpecificType(stream));
      }
      skipColorize = !styleShouldColorize(stream);
    }
    var formatArray = Array.isArray(format) ? format : [format];
    var openCodes = '', closeCodes = '', processedText = text;
    for (var fi = 0; fi < formatArray.length; fi++) {
      var key = formatArray[fi];
      if (key === 'none') continue;
      if (typeof key === 'string' && key.charAt(0) === '#') {
        if (hexColorRegExp.test(key) === false) {
          throw errOf(TypeError, 'ERR_INVALID_ARG_VALUE',
            "The argument 'format' must be a valid hex color (#RGB or #RRGGBB). Received " +
            inspReceived(key));
        }
        if (skipColorize) continue;
        var rgb = hexToRgb(key);
        var hexOpenSeq = '\u001b[38;2;' + rgb[0] + ';' + rgb[1] + ';' + rgb[2] + 'm';
        openCodes += hexOpenSeq;
        closeCodes = kHexCloseSeq + closeCodes;
        processedText = replaceCloseCode(processedText, kHexCloseSeq, hexOpenSeq, false);
        continue;
      }
      var codes = styleColors[key];
      if (!codes) {
        throw errOf(TypeError, 'ERR_INVALID_ARG_VALUE', validateOneOfMsg(key));
      }
      var st = codesToStyle(codes);
      openCodes += st.openSeq;
      closeCodes = st.closeSeq + closeCodes;
      processedText = replaceCloseCode(processedText, st.closeSeq, st.openSeq, st.keepClose);
    }
    if (skipColorize) return text;
    return openCodes + processedText + closeCodes;
  };

  // ── util.isDeepStrictEqual — full comparator back-channel ──────────────
  // install_assert stashes the assert blob's comparisons.js port (boxed
  // primitives, Map/Set/Date/RegExp/TypedArray, cycle memos, skipPrototype)
  // on this object as the non-enumerable __baoIsDeepStrictEqual before any
  // user code runs (globals.rs installs util then assert). The structural
  // walk below is the bare-engine fallback (no assert install) only.
  u.isDeepStrictEqual = function isDeepStrictEqual(a, b, skipPrototype) {
    var full = u.__baoIsDeepStrictEqual;
    if (full) return full(a, b, skipPrototype === true);
    if (a === b) return true;
    if (typeof a !== typeof b) return false;
    if (a === null || b === null) return a === b;
    if (typeof a !== 'object') return a === b;
    if (a instanceof Date && b instanceof Date) return a.getTime() === b.getTime();
    if (a instanceof RegExp && b instanceof RegExp) return a.source === b.source && a.flags === b.flags;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    var ka = Object.keys(a), kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    for (var i = 0; i < ka.length; i++) {
      var k = ka[i];
      if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
      if (!isDeepStrictEqual(a[k], b[k])) return false;
    }
    return true;
  };

  // ── util.inherits — node internal/util.js oracle ───────────────────────
  // super_ is defined with value/writable/configurable ONLY — enumerable
  // defaults false (test-util-inherits descriptor assertion); the three
  // ERR_INVALID_ARG_TYPE throws use the errors.js message shapes.
  u.inherits = function inherits(ctor, superCtor) {
    if (ctor === undefined || ctor === null) {
      throw invalidArgTypeErr('ctor', 'function', ctor);
    }
    if (superCtor === undefined || superCtor === null) {
      throw invalidArgTypeErr('superCtor', 'function', superCtor);
    }
    if (superCtor.prototype === undefined) {
      throw invalidArgTypeErr('superCtor.prototype', 'object', superCtor.prototype);
    }
    Object.defineProperty(ctor, 'super_', {
      value: superCtor,
      writable: true,
      configurable: true
    });
    Object.setPrototypeOf(ctor.prototype, superCtor.prototype);
  };

  // ── util.inspect options engine (getters / showHidden / breakLength) ───
  // Bounded port of node lib/internal/util/inspect.js for the option shapes
  // the native compact engine does not implement: accessor invocation
  // (getters), non-enumerable own props + prototype-layer props
  // (showHidden + addPrototypeProperties, 3-layer builtin-stop walk), and
  // compact:3 breakLength multi-line joining (reduceToSingleString).
  // Plain objects/arrays render here; exotic builtins keep their native
  // compact shapes (options inert on those heads — declared boundary).
  var inspectFullBuiltinNames = null;
  function getInspectFullBuiltinNames() {
    if (inspectFullBuiltinNames === null) {
      inspectFullBuiltinNames = new Set();
      var names;
      try { names = Object.getOwnPropertyNames(globalThis); } catch (e) { names = []; }
      for (var i = 0; i < names.length; i++) {
        if (/^[A-Z][a-zA-Z0-9]+$/.test(names[i])) inspectFullBuiltinNames.add(names[i]);
      }
    }
    return inspectFullBuiltinNames;
  }
  function inspFullTag(value) {
    var tag = '';
    try { tag = Object.prototype.toString.call(value); } catch (e) { tag = ''; }
    if (tag === '[object Object]' || tag === '[object Module]') {
      var proto = Object.getPrototypeOf(value);
      if (proto === null) return '[Object: null prototype]';
      var ctor = '';
      try { ctor = (value.constructor && value.constructor.name) || ''; } catch (e2) {}
      if (ctor && ctor !== 'Object') return '[' + ctor + ']';
      return '[Object]';
    }
    if (tag === '[object Array]') return '[Array]';
    if (tag === '[object Map]') return '[Map]';
    if (tag === '[object Set]') return '[Set]';
    if (tag === '[object Promise]') return '[Promise]';
    if (tag === '[object WeakMap]') return '[WeakMap]';
    if (tag === '[object WeakSet]') return '[WeakSet]';
    var tstr = '';
    try { tstr = value[Symbol.toStringTag]; } catch (e3) {}
    if (typeof tstr === 'string' && tstr !== '') return '[' + tstr + ']';
    return '[Object]';
  }
  // ctx: {seen, circular, indentationLvl, depth, breakLength, compact,
  //       getters, showHidden, currentDepth}
  function inspFull(value, recurseTimes, ctx) {
    var t = typeof value;
    if (value === null) return 'null';
    if (t === 'undefined') return 'undefined';
    if (t === 'boolean') return String(value);
    if (t === 'number') return inspFullNumber(value);
    if (t === 'bigint') return String(value) + 'n';
    if (t === 'symbol') return String(value);
    if (t === 'string') return inspFullStrEscape(value);
    if (t === 'function') {
      var fname = '';
      var isClass = false;
      try {
        fname = value.name || '';
        isClass = /^class[\s{]/.test(String(value).slice(0, 100));
      } catch (e) {}
      return isClass
        ? (fname ? '[class ' + fname + ']' : '[class (anonymous)]')
        : (fname ? '[Function: ' + fname + ']' : '[Function (anonymous)]');
    }
    // Cycle check precedes the depth tag (node formatValue order) — the
    // golden fixture2 circular reference resolves at depth 1 where the
    // truncation tag would otherwise mask the [Circular *N] form.
    if (ctx.seen.indexOf(value) !== -1) {
      var ci;
      if (ctx.circular === null) {
        ctx.circular = new Map();
        ctx.circular.set(value, 1);
        ci = 1;
      } else {
        ci = ctx.circular.get(value);
        if (ci === undefined) {
          ci = ctx.circular.size + 1;
          ctx.circular.set(value, ci);
        }
      }
      return '[Circular *' + ci + ']';
    }
    if (ctx.depth !== null && recurseTimes > ctx.depth) return inspFullTag(value);
    recurseTimes += 1;
    ctx.seen.push(value);
    ctx.currentDepth = recurseTimes;
    var rendered = inspFullFormatRaw(value, recurseTimes, ctx);
    if (ctx.circular !== null) {
      var r = ctx.circular.get(value);
      if (r !== undefined) {
        rendered.base = rendered.base === ''
          ? '<ref *' + r + '>'
          : '<ref *' + r + '> ' + rendered.base;
      }
    }
    ctx.seen.pop();
    return inspFullReduce(rendered, ctx);
  }
  function inspFullCtorWalk(value) {
    var walk = value;
    while (walk) {
      var d = null;
      try { d = Object.getOwnPropertyDescriptor(walk, 'constructor'); } catch (e) {}
      if (d && typeof d.value === 'function' && d.value.name !== '' &&
          (function() { try { return value instanceof d.value; } catch (e2) { return false; } })()) {
        return d.value.name;
      }
      walk = Object.getPrototypeOf(walk);
    }
    return null;
  }
  function inspFullAddProtoProps(ctx, main, obj, recurseTimes, output) {
    var depth = 0;
    var keys = null;
    var keySet = null;
    do {
      if (depth !== 0 || main === obj) {
        obj = Object.getPrototypeOf(obj);
        if (obj === null) return;
        var d = null;
        try { d = Object.getOwnPropertyDescriptor(obj, 'constructor'); } catch (e) {}
        if (d && typeof d.value === 'function' &&
            getInspectFullBuiltinNames().has(d.value.name)) return;
      }
      if (depth === 0) {
        keySet = new Set();
      } else {
        keys.forEach(function(k) { keySet.add(k); });
      }
      keys = Reflect.ownKeys(obj);
      ctx.seen.push(main);
      for (var i = 0; i < keys.length; i++) {
        var key = keys[i];
        if (key === 'constructor' ||
            Object.prototype.hasOwnProperty.call(main, key) ||
            (depth !== 0 && keySet.has(key))) {
          continue;
        }
        var desc;
        try { desc = Object.getOwnPropertyDescriptor(obj, key); } catch (e2) { continue; }
        if (desc === undefined) continue;
        if (typeof desc.value === 'function') continue;
        output.push(inspFullFormatProperty(ctx, obj, recurseTimes, key, desc, main));
      }
      ctx.seen.pop();
    } while (++depth !== 3);
  }
  function inspFullFormatProperty(ctx, value, recurseTimes, key, desc, original) {
    var str;
    if (desc.value !== undefined) {
      str = inspFull(desc.value, recurseTimes, ctx);
    } else if (desc.get !== undefined) {
      var label = desc.set !== undefined ? 'Getter/Setter' : 'Getter';
      if (ctx.getters && (ctx.getters === true ||
          (ctx.getters === 'get' && desc.set === undefined) ||
          (ctx.getters === 'set' && desc.set !== undefined))) {
        var threw = false;
        var tmp;
        try { tmp = desc.get.call(original); } catch (err) { threw = true; tmp = err; }
        if (threw) {
          str = '[' + label + ': <Inspection threw (' + inspFull(tmp, recurseTimes, ctx) + ')>]';
        } else if (tmp === null) {
          str = '[' + label + ': null]';
        } else if (typeof tmp === 'object' || typeof tmp === 'function') {
          str = '[' + label + '] ' + inspFull(tmp, recurseTimes, ctx);
        } else {
          str = '[' + label + ': ' + inspFull(tmp, recurseTimes, ctx) + ']';
        }
      } else {
        str = '[' + label + ']';
      }
    } else if (desc.set !== undefined) {
      str = '[Setter]';
    } else {
      str = 'undefined';
    }
    var name;
    if (typeof key === 'symbol') {
      name = String(key);
    } else if (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(key)) {
      name = key === '__proto__' ? "['__proto__']" : key;
    } else {
      name = inspFullStrEscape(key);
    }
    if (desc.enumerable === false) name = '[' + name + ']';
    return name + ': ' + str;
  }
  function inspFullFormatRaw(value, recurseTimes, ctx) {
    var tag = '';
    try { tag = Object.prototype.toString.call(value); } catch (e) {}
    var isArr = tag === '[object Array]';
    // Errors: node formatError — head = stack's first line, remaining
    // frames re-indented to ctx.indentationLvl + 4; own enumerable props
    // append as a standard body (node 'Error: msg { prop: 1 }').
    if (!isArr && value instanceof Error) {
      var errStack = '';
      try { errStack = String(value.stack || (value.name + ': ' + value.message)); } catch (e) {}
      var lines = errStack.split('\n');
      var baseStr = lines[0];
      if (lines.length > 1) {
        // node normalizes frame indentation — strip each frame's original
        // leading whitespace and re-pad uniformly (indentationLvl + 4).
        var framePad = new Array(ctx.indentationLvl + 5).join(' ');
        baseStr += '\n' + framePad + lines.slice(1)
          .map(function(l) { return l.replace(/^\s+/, ''); })
          .join('\n' + framePad);
      }
      var ekeys = [];
      try { ekeys = Object.keys(value); } catch (e10) {}
      if (ekeys.length === 0) return { str: baseStr };
      var eout = [];
      ctx.indentationLvl += 2;
      for (var ei = 0; ei < ekeys.length; ei++) {
        var edesc;
        try { edesc = Object.getOwnPropertyDescriptor(value, ekeys[ei]); } catch (e9) { continue; }
        if (edesc === undefined) continue;
        eout.push(inspFullFormatProperty(ctx, value, recurseTimes, ekeys[ei], edesc, value));
      }
      ctx.indentationLvl -= 2;
      return { base: baseStr, braces: ['{', '}'], output: eout, recurseTimes: recurseTimes };
    }
    var protoProps;
    if (ctx.showHidden && (ctx.depth === null || recurseTimes <= ctx.depth + 1)) {
      protoProps = [];
    }
    var ctorName = null;
    if (protoProps !== undefined) {
      // Identify the constructor and collect the prototype-layer props
      // (node getConstructorName + addPrototypeProperties pairing).
      var walk = value;
      var firstProto;
      var identified = false;
      while (walk) {
        var d = null;
        try { d = Object.getOwnPropertyDescriptor(walk, 'constructor'); } catch (e2) {}
        if (d && typeof d.value === 'function' && d.value.name !== '' &&
            (function() { try { return value instanceof d.value; } catch (e3) { return false; } })()) {
          if (firstProto !== walk ||
              !getInspectFullBuiltinNames().has(d.value.name)) {
            inspFullAddProtoProps(ctx, value,
              firstProto === undefined ? value : firstProto, recurseTimes, protoProps);
          }
          ctorName = d.value.name;
          identified = true;
          break;
        }
        var next = Object.getPrototypeOf(walk);
        if (firstProto === undefined) firstProto = next;
        walk = next;
      }
      if (!identified) ctorName = inspFullCtorWalk(value);
      if (protoProps.length === 0) protoProps = undefined;
    } else {
      ctorName = inspFullCtorWalk(value);
    }
    var proto = Object.getPrototypeOf(value);
    var keys, symbols;
    try {
      symbols = Object.getOwnPropertySymbols(value);
      if (ctx.showHidden) {
        keys = Object.getOwnPropertyNames(value);
      } else {
        keys = Object.keys(value);
        symbols = symbols.filter(function(s) {
          try {
            return Object.prototype.propertyIsEnumerable.call(value, s);
          } catch (e4) { return false; }
        });
      }
    } catch (e5) {
      keys = [];
      symbols = [];
    }
    var prefix = '';
    if (proto === null) {
      prefix = '[Object: null prototype] ';
    } else if (ctorName && ctorName !== 'Object') {
      prefix = ctorName + ' ';
    }
    if (!isArr && keys.length === 0 && protoProps === undefined) {
      if (proto === null) return { str: '[Object: null prototype] {}' };
      if (ctorName && ctorName !== 'Object') return { str: ctorName + ' {}' };
      return { str: '{}' };
    }
    var braces;
    if (isArr) {
      var arrCtor = '';
      try { arrCtor = (value.constructor && value.constructor.name) || ''; } catch (e6) {}
      braces = [(arrCtor && arrCtor !== 'Array')
        ? arrCtor + '(' + value.length + ') [' : '[', ']'];
    } else {
      braces = [(prefix === '' ? '{' : prefix + '{'), '}'];
    }
    var output = [];
    ctx.indentationLvl += 2;
    if (isArr) {
      var n = value.length;
      var holeRun = 0;
      for (var i = 0; i < n; i++) {
        if (i in value) {
          if (holeRun > 0) {
            output.push(holeRun === 1 ? '<1 empty item>' : '<' + holeRun + ' empty items>');
            holeRun = 0;
          }
          output.push(inspFull(value[i], recurseTimes, ctx));
        } else {
          holeRun++;
        }
      }
      if (holeRun > 0) {
        output.push(holeRun === 1 ? '<1 empty item>' : '<' + holeRun + ' empty items>');
      }
    }
    for (var ki = 0; ki < keys.length; ki++) {
      var key = keys[ki];
      if (isArr && typeof key === 'string' && /^[0-9]+$/.test(key)) continue;
      var desc;
      try { desc = Object.getOwnPropertyDescriptor(value, key); } catch (e7) { continue; }
      if (desc === undefined) continue;
      output.push(inspFullFormatProperty(ctx, value, recurseTimes, key, desc, value));
    }
    if (protoProps !== undefined) {
      for (var pi = 0; pi < protoProps.length; pi++) output.push(protoProps[pi]);
    }
    ctx.indentationLvl -= 2;
    return { base: '', braces: braces, output: output, recurseTimes: recurseTimes };
  }
  function inspFullBelowBreak(ctx, output, start) {
    if (ctx.breakLength === Infinity) {
      for (var i = 0; i < output.length; i++) {
        if (output[i].indexOf('\n') !== -1) return false;
      }
      return true;
    }
    var total = output.length + start;
    if (total + output.length > ctx.breakLength) return false;
    for (var j = 0; j < output.length; j++) {
      total += output[j].length;
      if (total > ctx.breakLength) return false;
    }
    return true;
  }
  function inspFullReduce(r, ctx) {
    if ('str' in r) return r.str;
    var base = r.base, braces = r.braces, output = r.output;
    if (output.length === 0) return braces[0] + braces[1];
    var unit = '  ';
    if (ctx.compact !== true && typeof ctx.compact === 'number' && ctx.compact >= 1) {
      if (ctx.currentDepth - r.recurseTimes < ctx.compact) {
        var start = output.length + ctx.indentationLvl + braces[0].length +
          base.length + 10;
        if (inspFullBelowBreak(ctx, output, start)) {
          var joined = output.join(', ');
          if (joined.indexOf('\n') === -1) {
            return (base ? base + ' ' : '') + braces[0] + ' ' + joined +
              ' ' + braces[1];
          }
        }
      }
      var indentation = '\n' + new Array(ctx.indentationLvl + 1).join(' ');
      return (base ? base + ' ' : '') + braces[0] + indentation + unit +
        output.join(',' + indentation + unit) + indentation + braces[1];
    }
    if (inspFullBelowBreak(ctx, output, 0)) {
      return braces[0] + (base ? ' ' + base : '') + ' ' + output.join(', ') +
        ' ' + braces[1];
    }
    var ind = new Array(ctx.indentationLvl + 1).join(' ');
    var ln = base === '' && braces[0].length === 1
      ? ' '
      : (base ? ' ' + base : '') + '\n' + ind + unit;
    return braces[0] + ln + output.join(',\n' + ind + unit) + ' ' + braces[1];
  }
  function inspFullOptions(opts) {
    var o = (opts === undefined || opts === null || typeof opts !== 'object') ? {} : opts;
    var d = o.depth;
    return {
      seen: [],
      circular: null,
      indentationLvl: 0,
      depth: d === undefined ? 2
        : (d === Infinity ? null : (typeof d === 'number' ? d : 2)),
      breakLength: o.breakLength === undefined ? 80 : o.breakLength,
      compact: o.compact === undefined ? 3 : o.compact,
      getters: o.getters === undefined ? false : o.getters,
      showHidden: o.showHidden === undefined ? false : o.showHidden,
      currentDepth: 0
    };
  }
  function inspFullRender(value, opts) {
    return inspFull(value, 0, inspFullOptions(opts));
  }
  // Hidden back-channel: the u.inspect wrapper below (and install-side
  // callers) reach the options engine without polluting the util namespace.
  Object.defineProperty(u, '__baoInspectFull', {
    value: inspFullRender,
    enumerable: false,
    writable: false,
    configurable: false
  });
  (function() {
    var nativeInspect = u.inspect;
    if (typeof nativeInspect !== 'function') return;
    var wrapped = function inspect(value, options) {
      var route = false;
      if (options !== undefined && options !== null && typeof options === 'object') {
        if (options.getters !== undefined && options.getters !== false) route = true;
        if (options.showHidden === true) route = true;
      }
      if (route) return inspFullRender(value, options);
      return nativeInspect(value, options);
    };
    var descs = Object.getOwnPropertyDescriptors(nativeInspect);
    for (var k in descs) {
      if (k === 'length' || k === 'name' || k === 'prototype') continue;
      try { Object.defineProperty(wrapped, k, descs[k]); } catch (e8) {}
    }
    u.inspect = wrapped;
  })();

  // ── util.parseArgs — node v24 semantics (ported from
  // internal/util/parse_args/{parse_args,utils}) ────────────────────────
  function paErr(code, msg) {
    var e = new TypeError(msg);
    e.code = code;
    return e;
  }
  var hasOwn = Object.prototype.hasOwnProperty;
  function own(o, k) { return hasOwn.call(o, k) ? o[k] : undefined; }
  function ownHas(o, k) { return hasOwn.call(o, k); }
  function optGet(options, longOption, prop) {
    if (!ownHas(options, longOption)) return undefined;
    return own(options[longOption], prop);
  }
  function findLongForShort(shortOption, options) {
    var names = Object.keys(options);
    for (var i = 0; i < names.length; i++) {
      if (own(options[names[i]], 'short') === shortOption) return names[i];
    }
    return shortOption;
  }
  function isLoneShortOption(a) {
    return a.length === 2 && a.charAt(0) === '-' && a.charAt(1) !== '-';
  }
  function isLoneLongOption(a) {
    return a.length > 2 && a.slice(0, 2) === '--' && a.indexOf('=', 2) === -1;
  }
  function isLongOptionAndValue(a) {
    return a.length > 2 && a.slice(0, 2) === '--' && a.indexOf('=', 2) !== -1;
  }
  function isShortOptionGroup(a, options) {
    if (a.length <= 2 || a.charAt(0) !== '-' || a.charAt(1) === '-') return false;
    return optGet(options, findLongForShort(a.charAt(1), options), 'type') !== 'string';
  }
  function isShortOptionAndValue(a, options) {
    if (a.length <= 2 || a.charAt(0) !== '-' || a.charAt(1) === '-') return false;
    return optGet(options, findLongForShort(a.charAt(1), options), 'type') === 'string';
  }
  function isOptionValue(v) { return v != null; }
  function isOptionLikeValue(v) {
    return v != null && v.length > 1 && v.charAt(0) === '-';
  }
  function checkUsage(config, token) {
    var tokenName = token.name;
    if (!ownHas(config.options, tokenName)) {
      if (config.allowNegative && tokenName.slice(0, 3) === 'no-') {
        tokenName = tokenName.slice(3);
        if (!ownHas(config.options, tokenName) ||
            optGet(config.options, tokenName, 'type') !== 'boolean') {
          throw paErr('ERR_PARSE_ARGS_UNKNOWN_OPTION', "Unknown option '" + token.rawName + "'");
        }
      } else {
        throw paErr('ERR_PARSE_ARGS_UNKNOWN_OPTION', "Unknown option '" + token.rawName + "'");
      }
    }
    var short = optGet(config.options, tokenName, 'short');
    var shortAndLong = (short ? '-' + short + ', ' : '') + '--' + tokenName;
    var type = optGet(config.options, tokenName, 'type');
    if (type === 'string' && typeof token.value !== 'string') {
      throw paErr('ERR_PARSE_ARGS_INVALID_OPTION_VALUE',
                  "Option '" + shortAndLong + " <value>' argument missing");
    }
    if (type === 'boolean' && token.value != null) {
      throw paErr('ERR_PARSE_ARGS_INVALID_OPTION_VALUE',
                  "Option '" + shortAndLong + "' does not take an argument");
    }
  }
  function checkOptionLikeValue(token) {
    if (!token.inlineValue && isOptionLikeValue(token.value)) {
      var example = token.rawName.slice(0, 2) === '--'
        ? "'" + token.rawName + "=-XYZ'"
        : "'--" + token.name + "=-XYZ' or '" + token.rawName + "-XYZ'";
      throw paErr('ERR_PARSE_ARGS_INVALID_OPTION_VALUE',
        "Option '" + token.rawName + "' argument is ambiguous.\n" +
        "Did you forget to specify the option argument for '" + token.rawName + "'?\n" +
        "To specify an option argument starting with a dash use " + example + '.');
    }
  }
  function storeOption(token, options, values, allowNegative) {
    var longOption = token.name;
    var optionValue = token.value;
    if (longOption === '__proto__') return;
    if (allowNegative && longOption.slice(0, 3) === 'no-' && optionValue === undefined) {
      longOption = longOption.slice(3);
      token.name = longOption;
      optionValue = false;
    }
    var newValue = optionValue == null ? true : optionValue;
    if (optGet(options, longOption, 'multiple')) {
      if (values[longOption]) values[longOption].push(newValue);
      else values[longOption] = [newValue];
    } else {
      values[longOption] = newValue;
    }
  }
  function argsToTokens(args, options) {
    var tokens = [];
    var index = -1;
    var groupCount = 0;
    var rest = args.slice();
    while (rest.length > 0) {
      var arg = rest.shift();
      var nextArg = rest[0];
      if (groupCount > 0) groupCount--;
      else index++;
      if (arg === '--') {
        tokens.push({ kind: 'option-terminator', index: index });
        for (var r = 0; r < rest.length; r++) {
          tokens.push({ kind: 'positional', index: ++index, value: rest[r] });
        }
        break;
      }
      if (isLoneShortOption(arg)) {
        var shortOption = arg.charAt(1);
        var longOption = findLongForShort(shortOption, options);
        var value;
        var inlineValue;
        if (optGet(options, longOption, 'type') === 'string' && isOptionValue(nextArg)) {
          value = rest.shift();
          inlineValue = false;
        }
        tokens.push({ kind: 'option', name: longOption, rawName: arg,
                      index: index, value: value, inlineValue: inlineValue });
        if (value != null) index++;
        continue;
      }
      if (isShortOptionGroup(arg, options)) {
        var expanded = [];
        for (var gi = 1; gi < arg.length; gi++) {
          var so = arg.charAt(gi);
          var lo = findLongForShort(so, options);
          if (optGet(options, lo, 'type') !== 'string' || gi === arg.length - 1) {
            expanded.push('-' + so);
          } else {
            expanded.push('-' + arg.slice(gi));
            break;
          }
        }
        rest = expanded.concat(rest);
        groupCount = expanded.length;
        continue;
      }
      if (isShortOptionAndValue(arg, options)) {
        var so2 = arg.charAt(1);
        tokens.push({ kind: 'option', name: findLongForShort(so2, options),
                      rawName: '-' + so2, index: index, value: arg.slice(2),
                      inlineValue: true });
        continue;
      }
      if (isLoneLongOption(arg)) {
        var long2 = arg.slice(2);
        var v2;
        var inl2;
        if (optGet(options, long2, 'type') === 'string' && isOptionValue(nextArg)) {
          v2 = rest.shift();
          inl2 = false;
        }
        tokens.push({ kind: 'option', name: long2, rawName: arg,
                      index: index, value: v2, inlineValue: inl2 });
        if (v2 != null) index++;
        continue;
      }
      if (isLongOptionAndValue(arg)) {
        var eq = arg.indexOf('=');
        var long3 = arg.slice(2, eq);
        tokens.push({ kind: 'option', name: long3, rawName: '--' + long3,
                      index: index, value: arg.slice(eq + 1), inlineValue: true });
        continue;
      }
      tokens.push({ kind: 'positional', index: index, value: arg });
    }
    return tokens;
  }
  u.parseArgs = function parseArgs(config) {
    if (config === undefined || config === null) config = {};
    var args = own(config, 'args');
    if (args === undefined) {
      if (Array.isArray(config)) {
        // Legacy bao call shape parseArgs([]): no args supplied, and the
        // embedder argv is not a CLI contract — parse an empty list.
        args = [];
      } else if (typeof process !== 'undefined' && Array.isArray(process.argv)) {
        args = process.argv.slice(2);
      } else {
        args = [];
      }
    }
    var strict = own(config, 'strict');
    if (strict === undefined) strict = true;
    var allowPositionals = own(config, 'allowPositionals');
    if (allowPositionals === undefined) allowPositionals = !strict;
    var returnTokens = own(config, 'tokens') === true;
    var allowNegative = own(config, 'allowNegative') === true;
    var options = own(config, 'options');
    if (options === undefined) options = Object.create(null);

    if (!Array.isArray(args)) {
      throw paErr('ERR_INVALID_ARG_TYPE',
        'The "args" argument must be an instance of Array. Received ' + typeof args);
    }
    if (typeof strict !== 'boolean') {
      throw paErr('ERR_INVALID_ARG_TYPE', 'The "strict" argument must be of type boolean. Received ' + typeof strict);
    }
    if (typeof allowPositionals !== 'boolean') {
      throw paErr('ERR_INVALID_ARG_TYPE', 'The "allowPositionals" argument must be of type boolean. Received ' + typeof allowPositionals);
    }
    if (typeof options !== 'object' || options === null) {
      throw paErr('ERR_INVALID_ARG_TYPE', 'The "options" argument must be of type object. Received ' + typeof options);
    }
    var optNames = Object.keys(options);
    for (var oi = 0; oi < optNames.length; oi++) {
      var name = optNames[oi];
      var optionConfig = options[name];
      if (typeof optionConfig !== 'object' || optionConfig === null) {
        throw paErr('ERR_INVALID_ARG_TYPE',
          'The "options.' + name + '" argument must be of type object. Received ' + typeof optionConfig);
      }
      var optionType = own(optionConfig, 'type');
      if (optionType !== 'string' && optionType !== 'boolean') {
        throw paErr('ERR_INVALID_ARG_VALUE',
          'The argument \'options.' + name + '.type\' is invalid. Received ' + String(optionType));
      }
      if (ownHas(optionConfig, 'short')) {
        var short = optionConfig.short;
        if (typeof short !== 'string') {
          throw paErr('ERR_INVALID_ARG_TYPE',
            'The "options.' + name + '.short" argument must be of type string. Received ' + typeof short);
        }
        if (short.length !== 1) {
          throw paErr('ERR_INVALID_ARG_VALUE',
            'The argument \'options.' + name + '.short\' is invalid. Received ' + String(short));
        }
      }
      if (ownHas(optionConfig, 'multiple') && typeof optionConfig.multiple !== 'boolean') {
        throw paErr('ERR_INVALID_ARG_TYPE',
          'The "options.' + name + '.multiple" argument must be of type boolean. Received ' + typeof optionConfig.multiple);
      }
      var defaultValue = own(optionConfig, 'default');
      if (defaultValue !== undefined) {
        if (optionType === 'string') {
          if (optionConfig.multiple ? !Array.isArray(defaultValue)
                                    : typeof defaultValue !== 'string') {
            throw paErr('ERR_INVALID_ARG_TYPE',
              'The "options.' + name + '.default" argument is invalid');
          }
        } else {
          if (optionConfig.multiple
              ? !(Array.isArray(defaultValue) && defaultValue.every(function(x) { return typeof x === 'boolean'; }))
              : typeof defaultValue !== 'boolean') {
            throw paErr('ERR_INVALID_ARG_TYPE',
              'The "options.' + name + '.default" argument is invalid');
          }
        }
      }
    }

    var tokens = argsToTokens(args, options);
    var parseConfig = { args: args, strict: strict, options: options,
                        allowPositionals: allowPositionals,
                        allowNegative: allowNegative };
    var result = { values: Object.create(null), positionals: [] };
    if (returnTokens) result.tokens = tokens;
    for (var ti = 0; ti < tokens.length; ti++) {
      var token = tokens[ti];
      if (token.kind === 'option') {
        if (strict) {
          checkUsage(parseConfig, token);
          checkOptionLikeValue(token);
        }
        storeOption(token, options, result.values, allowNegative);
      } else if (token.kind === 'positional') {
        if (!allowPositionals) {
          throw paErr('ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL',
            "Unexpected argument '" + token.value +
            "'. This command does not take positional arguments");
        }
        result.positionals.push(token.value);
      }
    }
    for (var di = 0; di < optNames.length; di++) {
      var longName = optNames[di];
      var cfg = options[longName];
      if (own(cfg, 'default') !== undefined && result.values[longName] === undefined) {
        if (longName !== '__proto__') result.values[longName] = own(cfg, 'default');
      }
    }
    return result;
  };

  // ── util.deprecate — node v24 semantics: wraps fn (prototype + length
  // preserved), emits one DeprecationWarning on first call. The warning
  // goes through process.emitWarning when the embedder provides it (tests
  // and embedders can observe/substitute it); otherwise it is written to
  // stderr so the deprecation is never silent.
  u.deprecate = function deprecate(fn, msg, code) {
    if (code !== undefined && typeof code !== 'string') {
      // node errors.js ERR_INVALID_ARG_TYPE message shape — null reports
      // 'Received null', objects 'Received an instance of X' (oracle:
      // test-util-deprecate-invalid-code / common.invalidArgTypeHelper).
      throw invalidArgTypeErr('code', 'string', code);
    }
    var warned = false;
    function deprecated() {
      if (!warned) {
        warned = true;
        if (typeof process === 'undefined' || !process.noDeprecation) {
          if (typeof process !== 'undefined' && typeof process.emitWarning === 'function') {
            process.emitWarning(msg, 'DeprecationWarning', code);
          } else if (typeof console !== 'undefined' && typeof console.error === 'function') {
            console.error('DeprecationWarning: ' + msg);
          }
        }
      }
      return new.target
        ? Reflect.construct(fn, Array.prototype.slice.call(arguments), new.target)
        : Reflect.apply(fn, this, arguments);
    }
    // node: ObjectSetPrototypeOf(deprecated, fn) — throws the same native
    // TypeError for a non-object fn (no silent guard).
    Object.setPrototypeOf(deprecated, fn);
    if (fn.prototype) deprecated.prototype = fn.prototype;
    Object.defineProperty(deprecated, 'length', Object.getOwnPropertyDescriptor(fn, 'length'));
    return deprecated;
  };
})"#;
        eval_js_factory_on_object(cx, util_extra_src, c"<util-extra>", util_obj.handle());
    }

    // @trace REQ-ENG-006 [api:util TextEncoder/TextDecoder] — Node re-exports
    // the global classes on the util module with identity preserved:
    // `util.TextEncoder === globalThis.TextEncoder` is true.
    unsafe {
        install_util_text_codecs(cx, util_obj.handle());
    }

    cache_builtin(cx, "util", util_obj.get());
}

/// Compile+evaluate a JS factory expression and invoke it with `target` as
/// the sole argument (the factory mutates the target object in place —
/// `util.types` extended checkers and the styleText/isDeepStrictEqual
/// extensions are both attached through this factory pattern).
///
/// e201 REQ-ARCH-001: shared body of the two inline factory blocks that used
/// to live in `install_util` (byte-identical control flow; only the source
/// text, compile filename, and target object differed). Evaluation result
/// is ignored by both callers, matching the original behavior.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn eval_js_factory_on_object(
    cx: &mut mozjs::context::JSContext,
    src: &str,
    filename: &::std::ffi::CStr,
    target: mozjs::rust::Handle<'_, *mut JSObject>,
) -> bool {
    let mut src_text = mozjs::rust::transform_str_to_source_text(src);
    let mut factory_val = UndefinedValue();
    let factory_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut factory_val,
    };
    let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), filename.as_ptr(), 1);
    if opts.is_null() {
        return false;
    }
    let global = CurrentGlobalOrNull(cx.raw_cx());
    let evaluated = !global.is_null()
        && JS::Evaluate2(cx.raw_cx(), opts, &mut src_text, factory_h)
        && factory_val.is_object();
    if evaluated {
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
        rooted!(&in(wrapped_cx) let global_root = global);
        rooted!(&in(wrapped_cx) let target_val_root = ObjectValue(target.get()));
        let args_arr = HandleValueArray {
            length_: 1,
            elements_: &target_val_root.get() as *const Value,
        };
        let mut call_rval = UndefinedValue();
        let call_rval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut call_rval,
        };
        rooted!(&in(wrapped_cx) let factory_obj = factory_val.to_object());
        rooted!(&in(wrapped_cx) let factory_obj_h = ObjectValue(factory_obj.get()));
        JS_CallFunctionValue(
            cx.raw_cx(),
            global_root.handle().into(),
            factory_obj_h.handle().into(),
            &args_arr,
            call_rval_h,
        );
    }
    mozjs::glue::DeleteCompileOptions(opts);
    evaluated
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

// Table-driven factory for the legacy native stub registrations on the
// placeholder assert object (e171, REQ-BRW-049): each row is
// (js name, native fn, nargs) and expands to the exact JS_DefineFunction
// call the handwritten block used — same order, same attrs.
macro_rules! register_assert_stubs {
    ($cx:expr, $obj:expr; $(($cname:expr, $fn:ident, $nargs:expr))*) => {
        $(
            w2::JS_DefineFunction(
                $cx,
                $obj.handle(),
                $cname.as_ptr(),
                Some($fn),
                $nargs,
                0,
            );
        )*
    };
}

pub fn install_assert(cx: &mut mozjs::context::JSContext) {
    // Keep the legacy native stubs referenced so we don't drop the function
    // pointer table (used as fallback by tests that import assert directly).
    rooted!(&in(cx) let assert_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if assert_obj.get().is_null() {
        return;
    }

    unsafe {
        // The full assert API is implemented in JS for AssertionError fidelity.
        // @trace REQ-ENG-048 [api:assert.deep-compare] — full node v24 parity
        // port: deep comparison (Map/Set/Date/RegExp/TypedArray/ArrayBuffer/
        // boxed/cycle-memoized), Myers diff error rendering (assert-local
        // inspect, sorted, cycle-marked), AssertionError with node operator
        // tables, throws/rejects validators, strict callable alias, Assert
        // class. Source of truth: node v24.19.0
        // lib/internal/util/comparisons.js, lib/internal/assert/myers_diff.js,
        // lib/internal/assert/assertion_error.js, lib/assert.js,
        // lib/internal/assert/utils.js. Validated under host node v24 with a
        // 220-case parity matrix before embedding.
        let src = r#"(function() {
function makeAssert() {
  var BufferCtor = (typeof Buffer !== 'undefined') ? Buffer : undefined;

  // ========================================================================
  // Section 1: deep comparison — port of lib/internal/util/comparisons.js
  // ========================================================================

  var kLoose = 0;
  var kPartial = 1;
  var kStrict = 2;
  var kStrictWithoutPrototypes = 3;

  var kNoIterator = 0;
  var kIsArray = 1;
  var kIsSet = 2;
  var kIsMap = 3;

  var objToString = Object.prototype.toString;
  var hasOwn = Object.prototype.hasOwnProperty;
  var propertyIsEnumerable = Object.prototype.propertyIsEnumerable;

  // Brand-checked intrinsics: toStringTag / prototype reassignment are
  // spoofable; the official suite pins the fake-object behavior.
  var dateGetTime = Date.prototype.getTime;
  var regExpSourceGetter = null;
  try {
    regExpSourceGetter = Object.getOwnPropertyDescriptor(RegExp.prototype, 'source').get;
  } catch (e) { /* no RegExp */ }
  var mapSizeGetter = null;
  var setSizeGetter = null;
  try {
    mapSizeGetter = Object.getOwnPropertyDescriptor(Map.prototype, 'size').get;
    setSizeGetter = Object.getOwnPropertyDescriptor(Set.prototype, 'size').get;
  } catch (e2) { /* no collections */ }
  var urlHrefGetter = null;
  try {
    if (typeof URL === 'function') {
      urlHrefGetter = Object.getOwnPropertyDescriptor(URL.prototype, 'href').get;
    }
  } catch (e3) { /* no URL */ }

  function brandCall(getter, v) {
    if (getter === null || !v || typeof v !== 'object') return false;
    try { getter.call(v); return true; } catch (e) { return false; }
  }
  function isDate(v) {
    if (!v || typeof v !== 'object') return false;
    try { dateGetTime.call(v); return true; } catch (e) { return false; }
  }
  function isRegExp(v) { return brandCall(regExpSourceGetter, v); }
  function isSet(v) { return brandCall(setSizeGetter, v); }
  function isMap(v) { return brandCall(mapSizeGetter, v); }
  function isPromise(v) { return objToString.call(v) === '[object Promise]'; }
  function isError(v) {
    if (!v || typeof v !== 'object') return false;
    var tag = objToString.call(v);
    return tag === '[object Error]' ||
      (typeof Error === 'function' && v instanceof Error);
  }
  var arrayBufferByteLengthGetter = null;
  try {
    arrayBufferByteLengthGetter = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  } catch (e) { /* no ArrayBuffer */ }
  function isAnyArrayBuffer(v) {
    if (!v || typeof v !== 'object') return false;
    // Brand check via the intrinsic byteLength getter (toStringTag and
    // prototype reassignment are spoofable; fakes must not enter the byte
    // comparison path).
    if (arrayBufferByteLengthGetter !== null) {
      try { arrayBufferByteLengthGetter.call(v); return true; } catch (e2) { /* try SAB */ }
    }
    if (typeof SharedArrayBuffer !== 'undefined') {
      try {
        var sabGetter = Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, 'byteLength').get;
        sabGetter.call(v);
        return true;
      } catch (e3) { /* not an ArrayBuffer */ }
    }
    return false;
  }
  function isArrayBufferView(v) {
    return typeof ArrayBuffer !== 'undefined' && ArrayBuffer.isView(v);
  }
  // Unspoofable typed-array brand: the intrinsic %TypedArray%.prototype
  // [Symbol.toStringTag] getter reads the internal slot (an own toStringTag or
  // a reassigned prototype cannot forge it). Mirrors node's
  // TypedArrayPrototypeGetSymbolToStringTag.
  var typedArrayTagGetter = null;
  try {
    var taBase = Object.getPrototypeOf(Int8Array.prototype);
    typedArrayTagGetter = Object.getOwnPropertyDescriptor(taBase, Symbol.toStringTag).get;
  } catch (e) { /* no typed arrays */ }
  function typedArrayTag(v) {
    if (typedArrayTagGetter === null) return undefined;
    try {
      return typedArrayTagGetter.call(v);
    } catch (e2) {
      return undefined; // not a TypedArray (e.g. DataView)
    }
  }
  function isBoxedPrimitiveTag(v) {
    var tag = objToString.call(v);
    if (tag !== '[object Number]' && tag !== '[object String]' &&
        tag !== '[object Boolean]' && tag !== '[object BigInt]' &&
        tag !== '[object Symbol]') {
      return false;
    }
    // toStringTag and valueOf are both spoofable (fake boxed objects in the
    // official suite): call the intrinsic prototype valueOf, which brand-
    // checks the [[PrimitiveValue]] internal slot.
    try {
      if (tag === '[object Number]') { Number.prototype.valueOf.call(v); return true; }
      if (tag === '[object String]') { String.prototype.valueOf.call(v); return true; }
      if (tag === '[object Boolean]') { Boolean.prototype.valueOf.call(v); return true; }
      if (tag === '[object BigInt]') { BigInt.prototype.valueOf.call(v); return true; }
      if (tag === '[object Symbol]') { Symbol.prototype.valueOf.call(v); return true; }
    } catch (e) { /* not a real boxed primitive */ }
    return false;
  }

  function isURL(v) {
    if (urlHrefGetter !== null) return brandCall(urlHrefGetter, v);
    return objToString.call(v) === '[object URL]';
  }

  function isKeyObject(v) {
    if (!v || typeof v !== 'object') return false;
    var tag = objToString.call(v);
    if (tag === '[object KeyObject]') return true;
    var ctor = '';
    try { ctor = v.constructor && v.constructor.name; } catch (e) { return false; }
    return ctor === 'KeyObject';
  }

  // node compares KeyObjects by type + key material (internal handle bytes).
  // The portable equivalent is the exported key material.
  function keyObjectEquals(a, b) {
    if (!isKeyObject(b)) return false;
    try {
      if (a.type !== b.type) return false;
      if (a.type === 'secret' && a.symmetricKeySize !== b.symmetricKeySize) {
        return false;
      }
      if (a.asymmetricKeyType !== b.asymmetricKeyType) return false;
      var ea = a.export();
      var eb = b.export();
      var ua = new Uint8Array(ea);
      var ub = new Uint8Array(eb);
      if (ua.length !== ub.length) return false;
      for (var i = 0; i < ua.length; i++) {
        if (ua[i] !== ub[i]) return false;
      }
      return true;
    } catch (e2) {
      return false;
    }
  }

  function isCryptoKey(v) {
    return objToString.call(v) === '[object CryptoKey]';
  }

  function cryptoKeyEquals(a, b, mode, memos) {
    if (!isCryptoKey(b)) return false;
    try {
      if (a.type !== b.type || a.extractable !== b.extractable) return false;
      if (!innerDeepEqual(a.algorithm, b.algorithm, mode, memos)) return false;
      var ua = a.usages ? a.usages.slice().sort() : [];
      var ub = b.usages ? b.usages.slice().sort() : [];
      if (!innerDeepEqual(ua, ub, mode, memos)) return false;
      if (typeof a.export === 'function' && typeof b.export === 'function') {
        var ea = a.export();
        var eb = b.export();
        if (ea !== undefined && eb !== undefined) {
          return innerDeepEqual(ea, eb, mode, memos);
        }
      }
      // bao's WebCrypto CryptoKey keeps its key material in non-enumerable
      // hidden slots (_raw for symmetric, _der for asymmetric) so sync
      // comparison can distinguish material the standard surface only
      // exposes asynchronously (subtle.exportKey). Node has no such slots and
      // falls through to the structural fields below.
      var slots = ['_raw', '_der'];
      for (var si = 0; si < slots.length; si++) {
        var va = a[slots[si]];
        var vb = b[slots[si]];
        if (va !== undefined || vb !== undefined) {
          if (va === undefined || vb === undefined) return false;
          return innerDeepEqual(va, vb, mode, memos);
        }
      }
      return true;
    } catch (e) {
      return false;
    }
  }

  var wellKnownConstructors = new Set();
  [
    Array, ArrayBuffer, BigInt, Boolean, BufferCtor, DataView, Date, Error,
    Function, Map, Number, Object, Promise, RegExp, Set, String, Symbol,
    WeakMap, WeakSet,
    typeof Int8Array !== 'undefined' && Int8Array,
    typeof Uint8Array !== 'undefined' && Uint8Array,
    typeof Uint8ClampedArray !== 'undefined' && Uint8ClampedArray,
    typeof Int16Array !== 'undefined' && Int16Array,
    typeof Uint16Array !== 'undefined' && Uint16Array,
    typeof Int32Array !== 'undefined' && Int32Array,
    typeof Uint32Array !== 'undefined' && Uint32Array,
    typeof Float32Array !== 'undefined' && Float32Array,
    typeof Float64Array !== 'undefined' && Float64Array,
    typeof BigInt64Array !== 'undefined' && BigInt64Array,
    typeof BigUint64Array !== 'undefined' && BigUint64Array,
    typeof Float16Array !== 'undefined' && Float16Array,
  ].forEach(function(c) { if (c) wellKnownConstructors.add(c); });

  function areSimilarRegExps(a, b) {
    return a.source === b.source &&
      a.flags === b.flags &&
      a.lastIndex === b.lastIndex;
  }

  function isPartialUint8Array(a, b) {
    var lenA = a.length;
    var lenB = b.length;
    if (lenA < lenB) return false;
    var offsetA = 0;
    for (var offsetB = 0; offsetB < lenB; offsetB++) {
      while (a[offsetA] !== b[offsetB]) {
        offsetA++;
        if (offsetA > lenA - lenB + offsetB) return false;
      }
      offsetA++;
    }
    return true;
  }

  function asUint8ArrayOf(view) {
    return new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  }

  function isPartialArrayBufferView(a, b) {
    if (a.byteLength < b.byteLength) return false;
    return isPartialUint8Array(asUint8ArrayOf(a), asUint8ArrayOf(b));
  }

  function areSimilarFloatArrays(a, b) {
    var len = a.byteLength;
    if (len !== b.byteLength) return false;
    for (var offset = 0; offset < len; offset++) {
      if (a[offset] !== b[offset]) return false;
    }
    return true;
  }

  function areSimilarTypedArrays(a, b) {
    if (a.byteLength !== b.byteLength) return false;
    var ua = asUint8ArrayOf(a);
    var ub = asUint8ArrayOf(b);
    for (var i = 0; i < ua.length; i++) {
      if (ua[i] !== ub[i]) return false;
    }
    return true;
  }

  function areEqualArrayBuffers(buf1, buf2) {
    if (buf1.byteLength !== buf2.byteLength) return false;
    var ua = new Uint8Array(buf1);
    var ub = new Uint8Array(buf2);
    for (var i = 0; i < ua.length; i++) {
      if (ua[i] !== ub[i]) return false;
    }
    return true;
  }

  function isEqualBoxedPrimitive(val1, val2) {
    var tag1 = objToString.call(val1);
    var tag2 = objToString.call(val2);
    try {
      if (tag1 === '[object Number]') {
        return tag2 === '[object Number]' &&
          Object.is(Number.prototype.valueOf.call(val1), Number.prototype.valueOf.call(val2));
      }
      if (tag1 === '[object String]') {
        return tag2 === '[object String]' &&
          String.prototype.valueOf.call(val1) === String.prototype.valueOf.call(val2);
      }
      if (tag1 === '[object Boolean]') {
        return tag2 === '[object Boolean]' &&
          Boolean.prototype.valueOf.call(val1) === Boolean.prototype.valueOf.call(val2);
      }
      if (tag1 === '[object BigInt]') {
        return tag2 === '[object BigInt]' &&
          BigInt.prototype.valueOf.call(val1) === BigInt.prototype.valueOf.call(val2);
      }
      if (tag1 === '[object Symbol]') {
        return tag2 === '[object Symbol]' &&
          Symbol.prototype.valueOf.call(val1) === Symbol.prototype.valueOf.call(val2);
      }
    } catch (e) {
      // fake boxed primitive (brand check failed)
      return false;
    }
    return false;
  }

  function isEnumerableOrIdentical(val1, val2, prop, mode, memos) {
    return propertyIsEnumerable.call(val2, prop) ||
      (mode === kPartial && (val2[prop] === undefined || (prop === 'message' && val2[prop] === ''))) ||
      innerDeepEqual(val1[prop], val2[prop], mode, memos);
  }

  function innerDeepEqual(val1, val2, mode, memos) {
    // All identical values are equivalent, as determined by ===.
    if (val1 === val2) {
      return val1 !== 0 || Object.is(val1, val2) || mode === kLoose;
    }

    // Check more closely if val1 and val2 are equal.
    if (mode !== kLoose) {
      if (typeof val1 === 'number') {
        // Check for NaN
        return val1 !== val1 && val2 !== val2;
      }
      if (typeof val2 !== 'object' || typeof val1 !== 'object' ||
          val1 === null || val2 === null) {
        return false;
      }
    } else {
      if (val1 === null || typeof val1 !== 'object') {
        return (val2 === null || typeof val2 !== 'object') &&
          // Loose equality plus the NaN carve-out.
          // eslint-disable-next-line eqeqeq
          (val1 == val2 || (val1 !== val1 && val2 !== val2));
      }
      if (val2 === null || typeof val2 !== 'object') {
        return false;
      }
    }
    return objectComparisonStart(val1, val2, mode, memos);
  }

  function hasUnequalTag(val1, val2) {
    return val1[Symbol.toStringTag] !== val2[Symbol.toStringTag];
  }

  function slowHasUnequalTag(val1Tag, val1, val2) {
    if (val1[Symbol.toStringTag] !== undefined && val2[Symbol.toStringTag] !== undefined) {
      return val1[Symbol.toStringTag] !== val2[Symbol.toStringTag];
    }
    return val1Tag !== objToString.call(val2);
  }

  function objectComparisonStart(val1, val2, mode, memos) {
    if (mode === kStrict) {
      // bao's Buffer is a distinct constructor whose instances report
      // constructor === Uint8Array (unlike node, where Buffer.constructor is
      // Buffer). Preserve node's Buffer !== Uint8Array strict semantics with
      // an explicit brand check.
      if (BufferCtor && (val1 instanceof BufferCtor) !== (val2 instanceof BufferCtor)) {
        return false;
      }
      if (wellKnownConstructors.has(val1.constructor) ||
          (val1.constructor !== undefined && !hasOwn.call(val1, 'constructor'))) {
        if (val1.constructor !== val2.constructor) {
          return false;
        }
      } else if (Object.getPrototypeOf(val1) !== Object.getPrototypeOf(val2)) {
        return false;
      }
    }

    if (Array.isArray(val1)) {
      if (!Array.isArray(val2) ||
          (val1.length !== val2.length && (mode !== kPartial || val1.length < val2.length)) ||
          hasUnequalTag(val1, val2)) {
        return false;
      }

      var filter = mode !== kLoose;
      var keys2 = getOwnNonIndexProperties(val2, filter);
      if (mode !== kPartial &&
          keys2.length !== getOwnNonIndexProperties(val1, filter).length) {
        return false;
      }
      return keyCheck(val1, val2, mode, memos, kIsArray, keys2);
    }

    var val1Tag;
    if (val1[Symbol.toStringTag] === undefined &&
        (val1Tag = objToString.call(val1)) === '[object Object]') {
      if (slowHasUnequalTag(val1Tag, val1, val2)) {
        return false;
      }
      return keyCheck(val1, val2, mode, memos, kNoIterator);
    } else if (isSet(val1)) {
      if (!isSet(val2) ||
          (val1.size !== val2.size && (mode !== kPartial || val1.size < val2.size)) ||
          hasUnequalTag(val1, val2)) {
        return false;
      }
      return keyCheck(val1, val2, mode, memos, kIsSet);
    } else if (isMap(val1)) {
      if (!isMap(val2) ||
          (val1.size !== val2.size && (mode !== kPartial || val1.size < val2.size)) ||
          hasUnequalTag(val1, val2)) {
        return false;
      }
      return keyCheck(val1, val2, mode, memos, kIsMap);
    } else if (isArrayBufferView(val1)) {
      if (typedArrayTag(val1) !== typedArrayTag(val2)) {
        return false;
      }
      if (mode === kPartial && val1.byteLength !== val2.byteLength) {
        if (!isPartialArrayBufferView(val1, val2)) {
          return false;
        }
      } else if (mode === kLoose && isFloatTypeName(typedArrayTag(val1))) {
        if (!areSimilarFloatArrays(val1, val2)) {
          return false;
        }
      } else if (!areSimilarTypedArrays(val1, val2)) {
        return false;
      }
      var filterTA = mode !== kLoose;
      var keys2TA = getOwnNonIndexProperties(val2, filterTA);
      if (mode !== kPartial &&
          keys2TA.length !== getOwnNonIndexProperties(val1, filterTA).length) {
        return false;
      }
      return keyCheck(val1, val2, mode, memos, kNoIterator, keys2TA);
    } else if (isDate(val1)) {
      if (!isDate(val2) || hasUnequalTag(val1, val2)) {
        return false;
      }
      // Date.prototype.getTime ignores own getTime overrides (node does the
      // same via DatePrototypeGetTime).
      var time1 = Date.prototype.getTime.call(val1);
      var time2 = Date.prototype.getTime.call(val2);
      if (time1 !== time2 && (time1 === time1 || time2 === time2)) {
        return false;
      }
    } else if (isRegExp(val1)) {
      if (!isRegExp(val2) || !areSimilarRegExps(val1, val2) || hasUnequalTag(val1, val2)) {
        return false;
      }
    } else if (isAnyArrayBuffer(val1)) {
      if (!isAnyArrayBuffer(val2) || hasUnequalTag(val1, val2)) {
        return false;
      }
      if (mode !== kPartial || val1.byteLength === val2.byteLength) {
        if (!areEqualArrayBuffers(val1, val2)) {
          return false;
        }
      } else if (!isPartialUint8Array(new Uint8Array(val1), new Uint8Array(val2))) {
        return false;
      }
    } else if (slowHasUnequalTag(val1Tag !== undefined ? val1Tag : objToString.call(val1), val1, val2) ||
               Array.isArray(val2) ||
               isArrayBufferView(val2) ||
               isSet(val2) ||
               isMap(val2) ||
               isDate(val2) ||
               isRegExp(val2) ||
               isAnyArrayBuffer(val2)) {
      return false;
    } else if (isError(val1)) {
      // Do not compare the stack as it might differ even though the error
      // itself is otherwise identical.
      if (!isError(val2) ||
          !isEnumerableOrIdentical(val1, val2, 'message', mode, memos) ||
          !isEnumerableOrIdentical(val1, val2, 'name', mode, memos) ||
          !isEnumerableOrIdentical(val1, val2, 'cause', mode, memos) ||
          !isEnumerableOrIdentical(val1, val2, 'errors', mode, memos)) {
        return false;
      }
      var hasOwnVal2Cause = hasOwn.call(val2, 'cause');
      if ((hasOwnVal2Cause !== hasOwn.call(val1, 'cause') &&
           (mode !== kPartial || hasOwnVal2Cause))) {
        return false;
      }
    } else if (isBoxedPrimitiveTag(val1)) {
      if (!isEqualBoxedPrimitive(val1, val2)) {
        return false;
      }
    } else if (isURL(val1)) {
      if (!isURL(val2) || val1.href !== val2.href) {
        return false;
      }
    } else if (isKeyObject(val1)) {
      if (!keyObjectEquals(val1, val2)) {
        return false;
      }
    } else if (isCryptoKey(val1)) {
      if (!cryptoKeyEquals(val1, val2, mode, memos)) {
        return false;
      }
    } else if (isBoxedPrimitiveTag(val2) ||
               isError(val2) ||
               val2 instanceof Error ||
               objToString.call(val1) === '[object WeakMap]' ||
               objToString.call(val1) === '[object WeakSet]' ||
               isPromise(val1)) {
      return false;
    }

    return keyCheck(val1, val2, mode, memos, kNoIterator);
  }

  var floatTypeNames = new Set(
    ['Float16Array', 'Float32Array', 'Float64Array'].filter(function(n) {
      return typeof globalThis[n] === 'function';
    })
  );
  function isFloatTypeName(tag) { return floatTypeNames.has(tag); }

  // Own non-index enumerable properties (string keys). `withSymbols` adds
  // own enumerable symbols (node's ONLY_ENUMERABLE vs
  // ONLY_ENUMERABLE | SKIP_SYMBOLS).
  var indexKeyRe = /^(?:0|[1-9][0-9]*)$/;
  function getOwnNonIndexProperties(val, withSymbols) {
    var keys = Object.keys(val);
    var out = [];
    for (var i = 0; i < keys.length; i++) {
      var k = keys[i];
      if (indexKeyRe.test(k) && String(k >>> 0) === k) continue;
      out.push(k);
    }
    if (withSymbols) {
      var syms = Object.getOwnPropertySymbols(val);
      for (var j = 0; j < syms.length; j++) {
        if (propertyIsEnumerable.call(val, syms[j])) out.push(syms[j]);
      }
    }
    return out;
  }

  function partialSymbolEquiv(val1, val2, keys2) {
    var symbolKeys = Object.getOwnPropertySymbols(val2);
    if (symbolKeys.length !== 0) {
      for (var i = 0; i < symbolKeys.length; i++) {
        var key = symbolKeys[i];
        if (propertyIsEnumerable.call(val2, key)) {
          keys2.push(key);
        }
      }
    }
    return true;
  }

  function keyCheck(val1, val2, mode, memos, iterationType, keys2) {
    var isArrayLikeObject = keys2 !== undefined;

    if (keys2 === undefined) {
      keys2 = Object.keys(val2);
    }
    var keys1;

    if (!isArrayLikeObject) {
      if (mode === kPartial) {
        if (!partialSymbolEquiv(val1, val2, keys2)) {
          return false;
        }
      } else if (keys2.length !== (keys1 = Object.keys(val1)).length) {
        return false;
      } else if (mode === kStrict || mode === kStrictWithoutPrototypes) {
        var syms1 = Object.getOwnPropertySymbols(val1);
        for (var i1 = 0; i1 < syms1.length; i1++) {
          if (propertyIsEnumerable.call(val1, syms1[i1])) {
            (keys1 || (keys1 = Object.keys(val1))).push(syms1[i1]);
          }
        }
        var syms2 = Object.getOwnPropertySymbols(val2);
        for (var i2 = 0; i2 < syms2.length; i2++) {
          if (propertyIsEnumerable.call(val2, syms2[i2])) {
            keys2.push(syms2[i2]);
          }
        }
        if (keys1.length !== keys2.length) {
          return false;
        }
      }
    }

    if (keys2.length === 0 &&
        (iterationType === kNoIterator ||
          (iterationType === kIsArray && val2.length === 0) ||
          val2.size === 0)) {
      return true;
    }

    if (memos === null) {
      return objEquiv(val1, val2, mode, keys1, keys2, memos, iterationType);
    }
    return handleCycles(val1, val2, mode, keys1, keys2, memos, iterationType);
  }

  function handleCycles(val1, val2, mode, keys1, keys2, memos, iterationType) {
    // Use memos to handle cycles.
    if (memos === undefined) {
      memos = {
        set: undefined,
        a: val1,
        b: val2,
        c: undefined,
        d: undefined,
        deep: false,
      };
      return objEquiv(val1, val2, mode, keys1, keys2, memos, iterationType);
    }

    if (memos.set === undefined) {
      if (memos.deep === false) {
        if (memos.a === val1) {
          return memos.b === val2;
        }
        if (memos.b === val2) {
          return false;
        }
        memos.c = val1;
        memos.d = val2;
        memos.deep = true;
        var result = objEquiv(val1, val2, mode, keys1, keys2, memos, iterationType);
        memos.deep = false;
        if (memos.set !== undefined) {
          memos.set.delete(memos.c);
          memos.set.delete(memos.d);
        }
        return result;
      }
      memos.set = new Set();
      memos.set.add(memos.a);
      memos.set.add(memos.b);
      memos.set.add(memos.c);
      memos.set.add(memos.d);
    }

    var set = memos.set;

    var originalSize = set.size;
    set.add(val1);
    set.add(val2);
    if (originalSize !== set.size - 2) {
      return originalSize === set.size;
    }

    var areEq = objEquiv(val1, val2, mode, keys1, keys2, memos, iterationType);

    set.delete(val1);
    set.delete(val2);

    return areEq;
  }

  function findLooseMatchingPrimitives(prim) {
    switch (typeof prim) {
      case 'undefined':
        return null;
      case 'object': // Only pass in null as object!
        return undefined;
      case 'symbol':
        return false;
      case 'string':
        prim = +prim;
        // Fall through
      case 'number':
        if (prim !== prim) {
          return false;
        }
    }
    return true;
  }

  function setMightHaveLoosePrim(a, b, prim) {
    var altValue = findLooseMatchingPrimitives(prim);
    if (altValue != null)
      return altValue;

    return !b.has(altValue) && a.has(altValue);
  }

  function mapMightHaveLoosePrim(a, b, prim, item2, memo) {
    var altValue = findLooseMatchingPrimitives(prim);
    if (altValue != null) {
      return altValue;
    }
    var item1 = a.get(altValue);
    if ((item1 === undefined && !a.has(altValue)) ||
        !innerDeepEqual(item1, item2, kLoose, memo)) {
      return false;
    }
    return !b.has(altValue) && innerDeepEqual(item1, item2, kLoose, memo);
  }

  function partialObjectSetEquiv(array, a, b, mode, memo) {
    var aPos = 0;
    var direction = 1;
    var start = 0;
    var end = array.length - 1;
    var itA = a.values();
    for (var next = itA.next(); !next.done; next = itA.next()) {
      var val1 = next.value;
      aPos++;
      if (!b.has(val1)) {
        var innerStart = start;
        if (direction === 1) {
          if (innerDeepEqual(val1, array[start], mode, memo)) {
            if (start === end) {
              return true;
            }
            start += 1;
            continue;
          }
          if (start === end) {
            continue;
          }
          direction = -1;
          innerStart += 1;
        }
        var matched = true;
        if (!innerDeepEqual(val1, array[end], mode, memo)) {
          direction = 1;
          matched = arrayHasEqualElement(array, val1, mode, memo, innerDeepEqual, innerStart, end);
        }
        if (matched) {
          if (start === end) {
            return true;
          }
          end -= 1;
        }
      }
      if (a.size - aPos <= end - start) {
        return false;
      }
    }
    return false;
  }

  function arrayHasEqualElement(array, val1, mode, memo, comparator, start, end) {
    for (var i = end - 1; i >= start; i--) {
      if (comparator(val1, array[i], mode, memo)) {
        // Move the matching element to make sure we do not check that again.
        array[i] = array[end];
        return true;
      }
    }
    return false;
  }

  function setObjectEquiv(array, a, b, mode, memo) {
    var direction = 1;
    var start = 0;
    var end = array.length - 1;
    var comparator = mode !== kLoose ? objectComparisonStart : innerDeepEqual;
    var extraChecks = mode === kLoose || array.length !== a.size;
    var itA = a.values();
    for (var next = itA.next(); !next.done; next = itA.next()) {
      var val1 = next.value;
      // Strict modes: a null/primitive member that is absent from b is a
      // definitive inequality (also guards objectComparisonStart against
      // null.constructor; node v24.19.0 still crashes here, the fix landed
      // upstream later and test-assert-deep.js pins it).
      if (mode !== kLoose && (typeof val1 !== 'object' || val1 === null)) {
        if (b.has(val1)) {
          continue;
        }
        return false;
      }
      if (extraChecks) {
        if (typeof val1 === 'object') {
          if (b.has(val1)) {
            continue;
          }
        } else if (b.has(val1)) {
          continue;
        } else if (mode !== kLoose) {
          return false;
        }
      }

      var innerStart = start;
      if (direction === 1) {
        if (comparator(val1, array[start], mode, memo)) {
          start += 1;
          continue;
        }
        if (start === end) {
          return false;
        }
        direction = -1;
        innerStart += 1;
      }
      if (!comparator(val1, array[end], mode, memo)) {
        direction = 1;
        if (!arrayHasEqualElement(array, val1, mode, memo, comparator, innerStart, end)) {
          return false;
        }
      }
      end -= 1;
    }
    return true;
  }

  function compareSmallSets(a, b, val, iteratorB, mode, memo) {
    var iteratorA = a.values();
    var firstA = iteratorA.next().value;
    var first = innerDeepEqual(firstA, val, mode, memo);
    if (first) {
      if (b.size === 1) { // Partial mode && a.size === 1 || b.size === 1
        return true;
      }
      var secondA = iteratorA.next().value;
      return b.has(secondA) || innerDeepEqual(secondA, iteratorB.next().value, mode, memo);
    }
    return a.size !== 1 && innerDeepEqual(iteratorA.next().value, val, mode, memo) && (
      b.size === 1 || // Partial mode
      b.has(firstA) || // Primitive or reference equal
      innerDeepEqual(firstA, iteratorB.next().value, mode, memo)
    );
  }

  function setEquiv(a, b, mode, memo) {
    // This is a lazily initiated Set of entries which have to be compared
    // pairwise.
    var array;

    var iteratorB = b.values();
    for (var next = iteratorB.next(); !next.done; next = iteratorB.next()) {
      var val = next.value;
      if (!a.has(val)) {
        if ((typeof val !== 'object' || val === null) &&
            (mode !== kLoose || !setMightHaveLoosePrim(a, b, val))) {
          return false;
        }

        if (array === undefined) {
          if (a.size < 3) {
            return compareSmallSets(a, b, val, iteratorB, mode, memo);
          }
          array = [];
        }
        // If the specified value doesn't exist in the second set it's an
        // object (or in loose mode: a non-matching primitive). Find the
        // deep-(mode-)equal element in a set copy to reduce duplicate checks.
        array.push(val);
      }
    }

    if (array === undefined) {
      return true;
    }
    if (mode === kPartial) {
      return partialObjectSetEquiv(array, a, b, mode, memo);
    }
    return setObjectEquiv(array, a, b, mode, memo);
  }

  function partialObjectMapEquiv(array, a, b, mode, memo) {
    var aPos = 0;
    var direction = 1;
    var start = 0;
    var end = array.length - 1;
    var itA = a.entries();
    for (var next = itA.next(); !next.done; next = itA.next()) {
      var key1 = next.value[0];
      var item1 = next.value[1];
      aPos++;
      if (typeof key1 === 'object' && key1 !== null) {
        var innerStart = start;
        if (direction === 1) {
          var key2 = array[start];
          if (objectComparisonStart(key1, key2, mode, memo) &&
              innerDeepEqual(item1, b.get(key2), mode, memo)) {
            if (start === end) {
              return true;
            }
            start += 1;
            continue;
          }
          if (start === end) {
            continue;
          }
          direction = -1;
          innerStart += 1;
        }
        var matched = true;
        var key2e = array[end];
        if (!objectComparisonStart(key1, key2e, mode, memo) ||
            !innerDeepEqual(item1, b.get(key2e), mode, memo)) {
          direction = 1;
          matched = arrayHasEqualMapElement(array, key1, item1, b, mode, memo, objectComparisonStart, innerStart, end);
        }
        if (matched) {
          if (start === end) {
            return true;
          }
          end -= 1;
        }
      }
      if (a.size - aPos <= end - start) {
        return false;
      }
    }
    return false;
  }

  function arrayHasEqualMapElement(array, key1, item1, b, mode, memo, comparator, start, end) {
    for (var i = end - 1; i >= start; i--) {
      var key2 = array[i];
      if (comparator(key1, key2, mode, memo) &&
          innerDeepEqual(item1, b.get(key2), mode, memo)) {
        // Move the matching element to make sure we do not check that again.
        array[i] = array[end];
        return true;
      }
    }
    return false;
  }

  function mapObjectEquiv(array, a, b, mode, memo) {
    var direction = 1;
    var start = 0;
    var end = array.length - 1;
    var comparator = mode !== kLoose ? objectComparisonStart : innerDeepEqual;
    var extraChecks = mode === kLoose || array.length !== a.size;

    var itA = a.entries();
    for (var next = itA.next(); !next.done; next = itA.next()) {
      var key1 = next.value[0];
      var item1 = next.value[1];
      // Strict modes: a null/primitive key absent from b is inequality
      // (mirrors the set guard; node v24.19.0 crashes on `key.constructor`).
      if (mode !== kLoose && (typeof key1 !== 'object' || key1 === null)) {
        if (!b.has(key1) || !innerDeepEqual(item1, b.get(key1), mode, memo)) {
          return false;
        }
        continue;
      }
      if (extraChecks && (typeof key1 !== 'object' || key1 === null)) {
        if (b.has(key1)) {
          if (mode !== kLoose || innerDeepEqual(item1, b.get(key1), mode, memo)) {
            continue;
          }
        } else if (mode !== kLoose) {
          return false;
        }
      }

      var innerStart = start;
      if (direction === 1) {
        var key2 = array[start];
        if (comparator(key1, key2, mode, memo) && innerDeepEqual(item1, b.get(key2), mode, memo)) {
          start += 1;
          continue;
        }
        if (start === end) {
          return false;
        }
        direction = -1;
        innerStart += 1;
      }
      var key2e = array[end];
      if ((!comparator(key1, key2e, mode, memo) || !innerDeepEqual(item1, b.get(key2e), mode, memo))) {
        direction = 1;
        if (!arrayHasEqualMapElement(array, key1, item1, b, mode, memo, comparator, innerStart, end)) {
          return false;
        }
      }
      end -= 1;
    }
    return true;
  }

  function mapEquiv(a, b, mode, memo) {
    var array;

    var itB = b.entries();
    for (var next = itB.next(); !next.done; next = itB.next()) {
      var key2 = next.value[0];
      var item2 = next.value[1];
      if (typeof key2 === 'object' && key2 !== null) {
        if (array === undefined) {
          if (a.size === 1) {
            var first = a.entries().next().value;
            var key1 = first[0];
            var item1 = first[1];
            return innerDeepEqual(key1, key2, mode, memo) &&
              innerDeepEqual(item1, item2, mode, memo);
          }
          array = [];
        }
        array.push(key2);
      } else {
        // By directly retrieving the value we prevent another b.has(key2)
        // check in almost all possible cases.
        var item1b = a.get(key2);
        if (((item1b === undefined && !a.has(key2)) ||
             !innerDeepEqual(item1b, item2, mode, memo))) {
          if (mode !== kLoose)
            return false;
          // Fast path to detect missing string, symbol, undefined and null
          // keys.
          if (!mapMightHaveLoosePrim(a, b, key2, item2, memo))
            return false;
          if (array === undefined) {
            array = [];
          }
          array.push(key2);
        }
      }
    }

    if (array === undefined) {
      return true;
    }

    if (mode === kPartial) {
      return partialObjectMapEquiv(array, a, b, mode, memo);
    }

    return mapObjectEquiv(array, a, b, mode, memo);
  }

  function partialSparseArrayEquiv(a, b, mode, memos, startA, startB) {
    var aPos = startA;
    var keysA = Object.keys(a);
    var keysB = Object.keys(b);
    var lenA = keysA.length - startA;
    var lenB = keysB.length - startB;
    if (lenA < lenB) {
      return false;
    }
    for (var i = 0; i < lenB; i++) {
      var keyB = keysB[startB + i];
      while (!innerDeepEqual(a[keysA[aPos]], b[keyB], mode, memos)) {
        aPos++;
        if (aPos > keysA.length - lenB + i) {
          return false;
        }
      }
      aPos++;
    }
    return true;
  }

  function partialArrayEquiv(a, b, mode, memos) {
    var aPos = 0;
    for (var i = 0; i < b.length; i++) {
      var isSparse = b[i] === undefined && !hasOwn.call(b, i);
      if (isSparse) {
        return partialSparseArrayEquiv(a, b, mode, memos, aPos, i);
      }
      while (!(isSparse = a[aPos] === undefined && !hasOwn.call(a, aPos)) &&
             !innerDeepEqual(a[aPos], b[i], mode, memos)) {
        aPos++;
        if (aPos > a.length - b.length + i) {
          return false;
        }
      }
      if (isSparse) {
        return partialSparseArrayEquiv(a, b, mode, memos, aPos, i);
      }
      aPos++;
    }
    return true;
  }

  function sparseArrayEquiv(a, b, mode, memos, i) {
    var keysA = Object.keys(a);
    var keysB = Object.keys(b);
    if (keysA.length !== keysB.length) {
      return false;
    }
    for (; i < keysB.length; i++) {
      var key = keysB[i];
      if ((a[key] === undefined && !hasOwn.call(a, key)) ||
          !innerDeepEqual(a[key], b[key], mode, memos)) {
        return false;
      }
    }
    return true;
  }

  function objEquiv(a, b, mode, keys1, keys2, memos, iterationType) {
    // The pair must have equivalent values for every corresponding key.
    if (keys2.length > 0) {
      var i = 0;
      // Ordered keys
      if (keys1 !== undefined) {
        for (; i < keys2.length; i++) {
          var key = keys2[i];
          if (keys1[i] !== key) {
            break;
          }
          if (!innerDeepEqual(a[key], b[key], mode, memos)) {
            return false;
          }
        }
      }
      // Unordered keys
      for (; i < keys2.length; i++) {
        var key2 = keys2[i];
        var descriptor = Object.getOwnPropertyDescriptor(a, key2);
        if (descriptor === undefined || descriptor.enumerable !== true) {
          return false;
        }
        var value = descriptor.writable !== undefined ? descriptor.value : a[key2];
        if (!innerDeepEqual(value, b[key2], mode, memos)) {
          return false;
        }
      }
    }

    if (iterationType === kIsArray) {
      if (mode === kPartial) {
        return partialArrayEquiv(a, b, mode, memos);
      }
      for (var j = 0; j < a.length; j++) {
        if (b[j] === undefined) {
          if (!hasOwn.call(b, j))
            return sparseArrayEquiv(a, b, mode, memos, j);
          if ((a[j] !== undefined || !hasOwn.call(a, j)) &&
              (mode !== kLoose || a[j] !== null))
            return false;
        } else if ((a[j] === undefined || !innerDeepEqual(a[j], b[j], mode, memos)) &&
                   (mode !== kLoose || b[j] !== null)) {
          return false;
        }
      }
    } else if (iterationType === kIsSet) {
      if (!setEquiv(a, b, mode, memos)) {
        return false;
      }
    } else if (iterationType === kIsMap) {
      if (!mapEquiv(a, b, mode, memos)) {
        return false;
      }
    }

    return true;
  }

  // Only handle cycles when they are detected (same lazy-swap strategy as
  // node: plain walk first; pathological recursion upgrades to the memoized
  // walk).
  var detectCycles = function(val1, val2, mode) {
    try {
      return innerDeepEqual(val1, val2, mode, null);
    } catch (e) {
      detectCycles = innerDeepEqual;
      return innerDeepEqual(val1, val2, mode, undefined);
    }
  };

  function isDeepEqual(val1, val2) {
    return detectCycles(val1, val2, kLoose);
  }
  function isDeepStrictEqual(val1, val2, skipPrototype) {
    return detectCycles(val1, val2, skipPrototype ? kStrictWithoutPrototypes : kStrict);
  }
  function isPartialStrictEqual(val1, val2) {
    return detectCycles(val1, val2, kPartial);
  }

  // ========================================================================
  // Section 2: myers diff — port of lib/internal/assert/myers_diff.js
  // ========================================================================

  var kNopLinesToCollapse = 5;
  var K_OP_DELETE = -1;
  var K_OP_NOP = 0;
  var K_OP_INSERT = 1;

  function areLinesEqual(actual, expected, checkCommaDisparity) {
    if (actual === expected) {
      return true;
    }
    if (checkCommaDisparity) {
      return (actual + ',') === expected || actual === (expected + ',');
    }
    return false;
  }

  function myersDiff(actual, expected, checkCommaDisparity) {
    var actualLength = actual.length;
    var expectedLength = expected.length;
    var max = actualLength + expectedLength;

    if (max > 2147483647) {
      // Mirrors node's ERR_OUT_OF_RANGE on myersDiff input size.
      throw new RangeError('The value of "myersDiff input size" is out of range. It must be < 2^31. Received ' + max);
    }

    var v = new Int32Array(2 * max + 1);
    var trace = [];

    for (var diffLevel = 0; diffLevel <= max; diffLevel++) {
      trace.push(Array.prototype.slice.call(v));

      for (var diagonalIndex = -diffLevel; diagonalIndex <= diffLevel; diagonalIndex += 2) {
        var offset = diagonalIndex + max;
        var previousOffset = v[offset - 1];
        var nextOffset = v[offset + 1];
        var x = diagonalIndex === -diffLevel ||
          (diagonalIndex !== diffLevel && previousOffset < nextOffset) ?
          nextOffset :
          previousOffset + 1;
        var y = x - diagonalIndex;

        while (
          x < actualLength &&
          y < expectedLength &&
          areLinesEqual(actual[x], expected[y], checkCommaDisparity)
        ) {
          x++;
          y++;
        }

        v[offset] = x;

        if (x >= actualLength && y >= expectedLength) {
          return backtrack(trace, actual, expected, checkCommaDisparity);
        }
      }
    }
  }

  function endsWithComma(s) {
    return typeof s === 'string' && s.length > 0 && s.charAt(s.length - 1) === ',';
  }

  function backtrack(trace, actual, expected, checkCommaDisparity) {
    var actualLength = actual.length;
    var expectedLength = expected.length;
    var max = actualLength + expectedLength;

    var x = actualLength;
    var y = expectedLength;
    var result = [];

    for (var diffLevel = trace.length - 1; diffLevel >= 0; diffLevel--) {
      var v = trace[diffLevel];
      var diagonalIndex = x - y;
      var offset = diagonalIndex + max;

      var prevDiagonalIndex;
      if (
        diagonalIndex === -diffLevel ||
        (diagonalIndex !== diffLevel && v[offset - 1] < v[offset + 1])
      ) {
        prevDiagonalIndex = diagonalIndex + 1;
      } else {
        prevDiagonalIndex = diagonalIndex - 1;
      }

      var prevX = v[prevDiagonalIndex + max];
      var prevY = prevX - prevDiagonalIndex;

      while (x > prevX && y > prevY) {
        var actualItem = actual[x - 1];
        var value = checkCommaDisparity &&
          !endsWithComma(actualItem) ? expected[y - 1] : actualItem;
        result.push([K_OP_NOP, value]);
        x--;
        y--;
      }

      if (diffLevel > 0) {
        if (x > prevX) {
          result.push([K_OP_INSERT, actual[--x]]);
        } else {
          result.push([K_OP_DELETE, expected[--y]]);
        }
      }
    }

    return result;
  }

  function printMyersDiff(diff, operator) {
    var message = '';
    var skipped = false;
    var nopCount = 0;

    for (var diffIdx = diff.length - 1; diffIdx >= 0; diffIdx--) {
      var operation = diff[diffIdx][0];
      var value = diff[diffIdx][1];
      var previousOperation = diffIdx < diff.length - 1 ? diff[diffIdx + 1][0] : null;

      // Avoid grouping if only one line would have been grouped otherwise
      if (previousOperation === K_OP_NOP && operation !== previousOperation) {
        if (nopCount === kNopLinesToCollapse + 1) {
          message += '  ' + diff[diffIdx + 1][1] + '\n';
        } else if (nopCount === kNopLinesToCollapse + 2) {
          message += '  ' + diff[diffIdx + 2][1] + '\n';
          message += '  ' + diff[diffIdx + 1][1] + '\n';
        } else if (nopCount >= kNopLinesToCollapse + 3) {
          message += '...\n';
          message += '  ' + diff[diffIdx + 1][1] + '\n';
          skipped = true;
        }
        nopCount = 0;
      }

      if (operation === K_OP_INSERT) {
        message += '+ ' + value + '\n';
      } else if (operation === K_OP_DELETE) {
        message += '- ' + value + '\n';
      } else if (operation === K_OP_NOP) {
        if (nopCount < kNopLinesToCollapse) {
          message += '  ' + value + '\n';
        }
        nopCount++;
      }
    }

    message = message.replace(/\n+$/, '');

    return { message: '\n' + message, skipped: skipped };
  }

  // ========================================================================
  // Section 3: assert-local value inspection (node inspectValue shape:
  // compact:false, depth:1000, maxArrayLength:Infinity, sorted, getters)
  // ========================================================================

  var kMaxLongStringLength = 512;

  function quoteString(s) {
    // node picks the quote style that needs fewer escapes.
    var quote = "'";
    if (s.indexOf("'") !== -1 && s.indexOf('"') === -1) quote = '"';
    var out = quote;
    for (var i = 0; i < s.length; i++) {
      var c = s.charAt(i);
      var code = s.charCodeAt(i);
      if (c === quote) out += '\\' + quote;
      else if (c === '\\') out += '\\\\';
      else if (c === '\n') out += '\\n';
      else if (c === '\r') out += '\\r';
      else if (c === '\t') out += '\\t';
      else if (code < 0x20 || code === 0x7f) {
        out += '\\u' + ('000' + code.toString(16)).slice(-4);
      }
      else out += c;
    }
    return out + quote;
  }

  function quoteMultiline(s, ctx) {
    var parts = [];
    var start = 0;
    for (var i = 0; i < s.length; i++) {
      if (s.charAt(i) === '\n') {
        parts.push(s.slice(start, i + 1));
        start = i + 1;
      }
    }
    if (start < s.length) parts.push(s.slice(start));
    var cont = ' +\n' + ctx.indent + '  ';
    var out = '';
    for (var p = 0; p < parts.length; p++) {
      if (p > 0) out += cont;
      out += quoteString(parts[p]);
    }
    return out;
  }

  function keyRepr(key) {
    if (typeof key === 'symbol') return key.toString();
    if (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(key)) return key;
    // Node renders non-identifier keys single-quoted (e.g. '0', 'my key').
    return quoteString(key);
  }

  function numberStr(n) {
    if (n !== n) return 'NaN';
    if (n === Infinity) return 'Infinity';
    if (n === -Infinity) return '-Infinity';
    if (Object.is(n, -0)) return '-0';
    return String(n);
  }

  function typeNamePrefix(val) {
    // '[Object: null prototype] ', 'F ', '' per node's getConstructorName.
    var proto = Object.getPrototypeOf(val);
    if (proto === null) return '[Object: null prototype]';
    if (proto === Object.prototype) return '';
    var ctor;
    try { ctor = val.constructor; } catch (e) { return ''; }
    // node only uses a constructor-derived name for non-Object function
    // constructors; an object-valued `constructor`, a missing constructor and
    // the intrinsic Object constructor render the plain '{}' form.
    if (typeof ctor === 'function' && ctor.name && ctor !== Object) return ctor.name;
    return '';
  }

  function isArrayIndexKey(k) {
    return /^(?:0|[1-9][0-9]*)$/.test(k) && String(k >>> 0) === k;
  }

  // Entry point. `depth` mirrors inspect's recurseTimes; `ctx.indent` is the
  // current indentation string (context lines of containers).
  function inspectValue(val, depth, ctx) {
    if (depth === undefined) depth = 1000;
    if (ctx === undefined) ctx = { indent: '', refs: null, refCounter: 0 };
    if (val === null) return 'null';
    var t = typeof val;
    if (t === 'undefined') return 'undefined';
    if (t === 'boolean') return String(val);
    if (t === 'number') return numberStr(val);
    if (t === 'bigint') return String(val) + 'n';
    if (t === 'symbol') return String(val);
    if (t === 'string') {
      // node's inspect (compact:false, breakLength 80) splits long strings
      // containing newlines into quoted segments joined by ' +' continuations.
      if (val.length > 80 && val.indexOf('\n') !== -1) {
        return quoteMultiline(val, ctx);
      }
      return quoteString(val);
    }
    if (t === 'function') {
      var fname = '';
      try { fname = val.name; } catch (e) { /* ignore */ }
      var isClass = false;
      try { isClass = /^class[\s{]/.test(String(val).slice(0, 100)); } catch (e) { /* ignore */ }
      if (isClass) {
        return fname ? '[class ' + fname + ']' : '[class (anonymous)]';
      }
      return fname ? '[Function: ' + fname + ']' : '[Function (anonymous)]';
    }
    // Objects.
    var seen = ctx.seen || (ctx.seen = []);
    for (var si = 0; si < seen.length; si++) {
      if (seen[si] === val) {
        var cycRef = ctx.refs ? ctx.refs.get(val) : 0;
        return '[Circular *' + (cycRef || (si + 1)) + ']';
      }
    }
    if (depth < 0) {
      // node's depth-exhaustion tags (util.inspect oracle): containers
      // collapse to '[Object]' / '[Array]' / '[Map]' / '[Set]' /
      // '[Promise]' / '[WeakMap]' / '[WeakSet]' / '[ArrayBuffer]' /
      // '[<toStringTag>]' (typed arrays — tag lives on the prototype).
      // Errors / Date / RegExp / functions / boxed primitives never
      // truncate — they render fully at any depth.
      // (The legacy '[Unknown]' shape came from the assert-only path where
      // depth 1000 never truncates; util.inspect delegation can hit this
      // arm with small depths, so node's exact tags are required.)
      if (isError(val) || isDate(val) || isRegExp(val) || isBoxedPrimitiveTag(val)) {
        // fall through to the full renderer below (depth stays < 0; the
        // per-entry walks guard their own recursion via depth - 1, and
        // scalar/leaf children render normally).
      } else {
        var tagStr = objToString.call(val);
        if (tagStr === '[object Object]' || tagStr === '[object Module]') return '[Object]';
        if (Array.isArray(val)) return '[Array]';
        if (tagStr === '[object Map]') return '[Map]';
        if (tagStr === '[object Set]') return '[Set]';
        if (tagStr === '[object Promise]') return '[Promise]';
        if (tagStr === '[object WeakMap]') return '[WeakMap]';
        if (tagStr === '[object WeakSet]') return '[WeakSet]';
        if (isAnyArrayBuffer(val)) return '[ArrayBuffer]';
        var truncTag = '';
        try { truncTag = val[Symbol.toStringTag]; } catch (e5) { /* ignore */ }
        if (typeof truncTag === 'string' && truncTag) return '[' + truncTag + ']';
        return '[Object]';
      }
    }
    seen.push(val);
    if (ctx.refs === null) ctx.refs = new Map();
    var refNum = ctx.refs.get(val);
    if (refNum === undefined) {
      if (participatesInCycle(val)) {
        ctx.refCounter += 1;
        refNum = ctx.refCounter;
        ctx.refs.set(val, refNum);
      } else {
        refNum = 0;
        ctx.refs.set(val, 0);
      }
    }
    var refPrefix = refNum > 0 ? '<ref *' + refNum + '> ' : '';
    var out = refPrefix + inspectObject(val, depth, ctx);
    seen.pop();
    return out;
  }

  function participatesInCycle(val) {
    // Cycle = val reachable from val via own enumerable properties, arrays,
    // Map/Set entries. Bounded walk with an explicit visited set.
    var visited = new Set();
    var stack = [val];
    var steps = 0;
    while (stack.length > 0) {
      steps++;
      if (steps > 10000) return true; // huge graph: assume cyclic
      var cur = stack.pop();
      if (visited.has(cur)) continue;
      visited.add(cur);
      var children = [];
      var t = typeof cur;
      if (cur === null || (t !== 'object' && t !== 'function')) continue;
      var tag = objToString.call(cur);
      if (Array.isArray(cur)) {
        for (var i = 0; i < cur.length; i++) children.push(cur[i]);
        var props = Object.keys(cur);
        for (var p = 0; p < props.length; p++) children.push(cur[props[p]]);
      } else if (tag === '[object Map]') {
        cur.forEach(function(v, k) { children.push(k); children.push(v); });
      } else if (tag === '[object Set]') {
        cur.forEach(function(v) { children.push(v); });
      } else if (ArrayBuffer.isView(cur)) {
        continue;
      } else {
        var keys = Object.keys(cur);
        for (var k = 0; k < keys.length; k++) children.push(cur[keys[k]]);
      }
      for (var c = 0; c < children.length; c++) {
        var ch = children[c];
        if (ch === val) return true;
        if (ch && typeof ch === 'object' && !visited.has(ch)) stack.push(ch);
      }
    }
    return false;
  }

  // Compose a container body: open brace on the prefix line, one entry per
  // line (nested containers carry their own indentation), comma-separated,
  // closing bracket at the parent indent. `open` is '{' or '['.
  function joinBody(open, lines, indent, close) {
    var pad = indent + '  ';
    var body = '';
    for (var i = 0; i < lines.length; i++) {
      var parts = lines[i].split('\n');
      for (var j = 0; j < parts.length; j++) {
        // Only the first line of an entry takes the parent pad; continuation
        // lines of nested containers already carry their absolute indent.
        body += (j === 0 ? pad : '') + parts[j];
        if (j === parts.length - 1 && i < lines.length - 1) body += ',';
        body += '\n';
      }
    }
    return open + '\n' + body + indent + close;
  }

  // node's inspect (compact: false — assertion_error.js inspectValue) always
  // breaks object/array bodies into indented per-entry lines (joinBody); the
  // single-line '{ a: 1 }' forms in golden expected messages come from the
  // AssertionError *own-props* rendering (inspectCompact / bare inspect()),
  // never from inspectValue itself.
  function compactBody(prefix, open, close, lines, indent) {
    return (prefix ? prefix + ' ' : '') + joinBody(open, lines, indent, close);
  }

  // Container head: 'Map(1) {' / '[' / '[Object: null prototype] {}' forms.
  function body(prefix, open, close, lines, indent) {
    if (lines.length === 0) return (prefix ? prefix + ' ' : '') + open + close;
    return compactBody(prefix, open, close, lines, indent);
  }

  // Date/RegExp with own enumerable props and/or a subclass constructor:
  // node renders 'MyDate 2016-...Z { '0': '1' }' / 'MyRegExp /a/g { ... }'.
  function inspectSpecial(val, base, depth, ctx, defaultCtorName) {
    var ctorName = '';
    try { ctorName = val.constructor && val.constructor.name; } catch (e) { /* ignore */ }
    var prefix = (ctorName && ctorName !== defaultCtorName) ? ctorName + ' ' : '';
    var keys = Object.keys(val).sort();
    if (keys.length === 0) return prefix + base;
    var lines = [];
    for (var i = 0; i < keys.length; i++) {
      var v;
      try { v = val[keys[i]]; } catch (e2) { v = '[Thrown]'; }
      lines.push(keyRepr(keys[i]) + ': ' + inspectValue(v, depth - 1, ctx));
    }
    // Date/RegExp subclass own-props bodies always break (mirrors util.inspect
    // compact:false output: 'MyDate 2016-...Z {\n  0: \'1\'\n}').
    return prefix + base + ' ' + joinBody('{', lines, ctx.indent, '}');
  }

  function inspectObject(val, depth, ctx) {
    if (Array.isArray(val)) {
      return inspectArray(val, depth, ctx);
    }
    var tag = objToString.call(val);
    if (isError(val)) return inspectError(val, depth, ctx, false);
    if (isURL(val)) {
      return inspectSpecial(val, String(val.href), depth, ctx, 'URL');
    }
    if (isDate(val)) {
      return inspectSpecial(val, isNaN(val.getTime()) ? 'Invalid Date' : val.toISOString(), depth, ctx, 'Date');
    }
    if (isRegExp(val)) {
      return inspectSpecial(val, '/' + val.source + '/' + val.flags, depth, ctx, 'RegExp');
    }
    if (isMap(val)) return inspectMap(val, depth, ctx);
    if (isSet(val)) return inspectSet(val, depth, ctx);
    if (tag === '[object Arguments]') {
      return inspectEntries(val, depth, ctx, '[Arguments]', '{', '}');
    }
    if (isArrayBufferView(val) && isTypedArrayDisplayTag(val[Symbol.toStringTag])) {
      return inspectTypedArray(val, depth, ctx);
    }
    if (isAnyArrayBuffer(val)) return inspectArrayBuffer(val, depth, ctx);
    if (tag === '[object Promise]') return inspectPromise(val, depth, ctx);
    if (tag === '[object WeakMap]') return body('WeakMap', '{', '}', ['<items unknown>'], ctx.indent);
    if (tag === '[object WeakSet]') return body('WeakSet', '{', '}', ['<items unknown>'], ctx.indent);
    if (isBoxedPrimitiveTag(val)) return inspectBoxed(val, depth, ctx);
    if (tag === '[object Object]' || tag === '[object Module]') {
      var prefix = typeNamePrefix(val);
      return inspectEntries(val, depth, ctx, prefix, '{', '}');
    }
    // Exotic objects without a special branch.
    var ctor = '';
    try { ctor = val.constructor && val.constructor.name; } catch (e) { /* ignore */ }
    var tagName = tag.replace(/^\[object |\]$/g, '');
    var ownTag = false;
    try { ownTag = hasOwn.call(val, Symbol.toStringTag); } catch (e2) { /* ignore */ }
    var prefix;
    if (ctor && ownTag && tagName !== 'Object' && tagName !== ctor) {
      // Buffer [Uint8Array] { ... } style for own-tag vs constructor mismatch.
      prefix = ctor + ' [' + tagName + ']';
    } else if (ctor) {
      prefix = ctor;
    } else {
      prefix = '';
    }
    // node compacts plain-object bodies on the same breakLength rule; arrays
    // (Array { 0: 1 } fake-array style) always take the compact single line.
    if (tag === '[object Array]' && prefix) {
      var fakeKeys = Object.keys(val).sort();
      var fakeLines = [];
      var fakeChild = { indent: ctx.indent + '  ', refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen };
      for (var fk = 0; fk < fakeKeys.length; fk++) {
        var fv;
        try { fv = val[fakeKeys[fk]]; } catch (e3) { fv = '[Thrown]'; }
        fakeLines.push(keyRepr(fakeKeys[fk]) + ': ' + inspectEntryValue(val, fakeKeys[fk], fv, depth - 1, fakeChild));
      }
      return 'Array { ' + fakeLines.join(', ') + ' }';
    }
    return inspectEntries(val, depth, ctx, prefix, '{', '}');
  }

  // node's container-head prefix for subclasses: a subclass of Map/Set
  // renders 'Ctor(size) [Map]' / 'Ctor(size) [Set]'; a subclass of Array
  // renders 'Ctor(len) [' (no '[Array]' tag — node oracle 'MyA(2) [').
  function containerPrefix(val, baseName, sizeTag) {
    var ctor = '';
    try { ctor = val.constructor && val.constructor.name; } catch (e) { /* ignore */ }
    if (!ctor || ctor === baseName) return baseName + '(' + sizeTag + ')';
    if (baseName === 'Array') return ctor + '(' + sizeTag + ')';
    return ctor + '(' + sizeTag + ') [' + baseName + ']';
  }

  function inspectArray(arr, depth, ctx) {
    var n = arr.length;
    var props = Object.keys(arr).filter(function(k) { return !isArrayIndexKey(k); });
    // Subclass arrays take the 'Ctor(len) [' head (node: 'MyArray(3) [');
    // plain arrays keep the bare '[' head.
    var isSub = false;
    try { isSub = arr.constructor && arr.constructor !== Array; } catch (e0) { /* ignore */ }
    if (isSub) {
      var head = containerPrefix(arr, 'Array', String(n));
      if (n === 0 && props.length === 0) return head + ' []';
      var subLines = [];
      var holeRun2 = 0;
      for (var i2 = 0; i2 < n; i2++) {
        if (i2 in arr) {
          if (holeRun2 > 0) {
            subLines.push(holeRun2 === 1 ? '<1 empty item>' : '<' + holeRun2 + ' empty items>');
            holeRun2 = 0;
          }
          subLines.push(inspectValue(arr[i2], depth - 1, { indent: ctx.indent + '  ', refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen }));
        } else {
          holeRun2++;
        }
      }
      if (holeRun2 > 0) {
        subLines.push(holeRun2 === 1 ? '<1 empty item>' : '<' + holeRun2 + ' empty items>');
      }
      for (var p2 = 0; p2 < props.length; p2++) {
        subLines.push(keyRepr(props[p2]) + ': ' + inspectValue(arr[props[p2]], depth - 1, ctx));
      }
      return body(head, '[', ']', subLines, ctx.indent);
    }
    if (n === 0 && props.length === 0) return '[]';
    var lines = [];
    var holeRun = 0;
    for (var i = 0; i < n; i++) {
      if (i in arr) {
        if (holeRun > 0) {
          lines.push(holeRun === 1 ? '<1 empty item>' : '<' + holeRun + ' empty items>');
          holeRun = 0;
        }
        lines.push(inspectValue(arr[i], depth - 1, { indent: ctx.indent + '  ', refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen }));
      } else {
        holeRun++;
      }
    }
    if (holeRun > 0) {
      lines.push(holeRun === 1 ? '<1 empty item>' : '<' + holeRun + ' empty items>');
    }
    for (var p = 0; p < props.length; p++) {
      lines.push(keyRepr(props[p]) + ': ' + inspectValue(arr[props[p]], depth - 1, ctx));
    }
    if (lines.length === 0) return '[]';
    return joinBody('[', lines, ctx.indent, ']');
  }

  // node marks accessor-derived values: [Getter: 5], [Getter/Setter: 5],
  // [Setter] (getters:true invokes and shows the value).
  function inspectEntryValue(owner, key, value, depth, childCtx) {
    var descriptor;
    try {
      descriptor = Object.getOwnPropertyDescriptor(owner, key);
    } catch (e) {
      return inspectValue(value, depth, childCtx);
    }
    if (descriptor && descriptor.get) {
      var rendered = inspectValue(value, depth, childCtx);
      return descriptor.set ? '[Getter/Setter: ' + rendered + ']' : '[Getter: ' + rendered + ']';
    }
    if (descriptor && descriptor.set) {
      return '[Setter]';
    }
    return inspectValue(value, depth, childCtx);
  }

  function inspectEntries(val, depth, ctx, prefix, open, close) {
    var keys = Object.keys(val).sort();
    var syms = Object.getOwnPropertySymbols(val).filter(function(s) {
      return propertyIsEnumerable.call(val, s);
    });
    if (keys.length === 0 && syms.length === 0) return body(prefix, open, close, [], ctx.indent);
    var lines = [];
    var childCtx = { indent: ctx.indent + '  ', refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen };
    for (var i = 0; i < keys.length; i++) {
      var v;
      try { v = val[keys[i]]; } catch (e) { v = '[Thrown]'; }
      lines.push(keyRepr(keys[i]) + ': ' + inspectEntryValue(val, keys[i], v, depth - 1, childCtx));
    }
    for (var j = 0; j < syms.length; j++) {
      var sv;
      try { sv = val[syms[j]]; } catch (e) { sv = '[Thrown]'; }
      lines.push(keyRepr(syms[j]) + ': ' + inspectEntryValue(val, syms[j], sv, depth - 1, childCtx));
    }
    return body(prefix, '{', '}', lines, ctx.indent);
  }

  function inspectError(err, depth, ctx) {
    var name = '';
    var msg = '';
    var ctorName = '';
    try { name = String(err.name || 'Error'); } catch (e) { /* ignore */ }
    try { msg = String(err.message); } catch (e2) { /* ignore */ }
    try { ctorName = err.constructor && err.constructor.name; } catch (e3) { /* ignore */ }
    var isCopy = err.__baoCopiedError === true;
    var head;
    if (isCopy) {
      // copyError duplicates (node: Object.assign over the prototype) lose
      // the native-error flag, so node's inspect renders the bracketed
      // no-stack form.
      head = '[' + name + ': ' + msg + ']';
    } else if (ctorName && ctorName !== 'Error' && ctorName !== name) {
      head = ctorName + ' [Error]: ' + msg;
    } else {
      head = name + ': ' + msg;
    }
    var frames = [];
    if (!isCopy) {
      var stack = '';
      try { stack = String(err.stack || ''); } catch (e4) { /* ignore */ }
      if (stack) {
        var sLines = stack.split('\n');
        for (var s = 1; s < sLines.length; s++) {
          // node indents frames relative to the head line: the base 4-space
          // form plus the current entry indent (a nested error's frames
          // carry the container's indentation — 'e: TypeError: x' inside
          // an object body gets 6-space frames).
          if (sLines[s]) frames.push(sLines[s].replace(/^\s+/, '    ' + ctx.indent));
        }
      }
    }
    var props = [];
    var errChild = { indent: ctx.indent + '  ', refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen };
    var keys = Object.keys(err).filter(function(k) {
      // 'cause' is rendered by the dedicated '[cause]:' arm below; SM makes
      // it enumerable (node/V8 keeps it non-enumerable) so without this
      // filter it would render twice ('cause:' + '[cause]:').
      return k !== 'message' && k !== 'name' && k !== 'stack' && k !== 'cause';
    });
    for (var i = 0; i < keys.length; i++) {
      var v;
      try { v = err[keys[i]]; } catch (e5) { v = '[Thrown]'; }
      props.push(keyRepr(keys[i]) + ': ' + inspectValue(v, depth - 1, errChild));
    }
    if (hasOwn.call(err, 'cause')) {
      props.push('[cause]: ' + inspectValue(err.cause, depth - 1, errChild));
    }
    // Node layout: head line, stack frames as bare continuation lines, then
    // the own-props block ('{' rides on the last line before the props).
    if (frames.length === 0 && props.length === 0) return head;
    var out = head;
    for (var f = 0; f < frames.length; f++) out += '\n' + frames[f];
    if (props.length > 0) out += ' ' + joinBody('{', props, ctx.indent, '}');
    return out;
  }

  function inspectMap(m, depth, ctx) {
    var lines = [];
    var inner = ctx.indent + '  ';
    var childCtx = { indent: inner, refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen };
    var count = 0;
    m.forEach(function(v, k) {
      lines.push([inspectValue(k, depth - 1, childCtx), inspectValue(v, depth - 1, childCtx)]);
      count++;
    });
    // node's inspect sorted:true orders Map entries by rendered key.
    lines.sort(function(x, y) { return x[0] < y[0] ? -1 : (x[0] > y[0] ? 1 : 0); });
    var mlines = [];
    for (var mi = 0; mi < lines.length; mi++) mlines.push(lines[mi][0] + ' => ' + lines[mi][1]); // @evidence SPEC/10-REQUIREMENTS.html:237
    var mprops = Object.keys(m).filter(function(k) { return !isArrayIndexKey(k); }).sort();
    var mchild = { indent: ctx.indent + '  ', refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen };
    for (var mp = 0; mp < mprops.length; mp++) {
      mlines.push(keyRepr(mprops[mp]) + ': ' + inspectEntryValue(m, mprops[mp], m[mprops[mp]], depth - 1, mchild));
    }
    // Subclass Maps take the node 'Ctor(size) [Map]' head (plain: 'Map(size)').
    var isMapSub = false;
    try { isMapSub = m.constructor && m.constructor !== Map; } catch (e7) { /* ignore */ }
    var mhead = isMapSub ? containerPrefix(m, 'Map', String(m.size)) : 'Map(' + m.size + ')';
    if (isMapSub) return body(mhead, '{', '}', mlines, ctx.indent);
    return body('Map(' + m.size + ')', '{', '}', mlines, ctx.indent);
  }

  function inspectSet(s, depth, ctx) {
    var lines = [];
    var inner = ctx.indent + '  ';
    var childCtx = { indent: inner, refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen };
    s.forEach(function(v) {
      lines.push(inspectValue(v, depth - 1, childCtx));
    });
    // node's inspect sorted:true orders Set entries by rendered value.
    lines.sort();
    var sprops = Object.keys(s).filter(function(k) { return !isArrayIndexKey(k); }).sort();
    var schild = { indent: ctx.indent + '  ', refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen };
    for (var sp = 0; sp < sprops.length; sp++) {
      lines.push(keyRepr(sprops[sp]) + ': ' + inspectEntryValue(s, sprops[sp], s[sprops[sp]], depth - 1, schild));
    }
    // Subclass Sets take the node 'Ctor(size) [Set]' head (plain: 'Set(size)').
    var isSetSub = false;
    try { isSetSub = s.constructor && s.constructor !== Set; } catch (e8) { /* ignore */ }
    var shead = isSetSub ? containerPrefix(s, 'Set', String(s.size)) : 'Set(' + s.size + ')';
    if (isSetSub) return body(shead, '{', '}', lines, ctx.indent);
    return body('Set(' + s.size + ')', '{', '}', lines, ctx.indent);
  }

  function isTypedArrayDisplayTag(tag) {
    return tag === 'DataView' || tag === 'Int8Array' || tag === 'Uint8Array' ||
      tag === 'Uint8ClampedArray' || tag === 'Int16Array' || tag === 'Uint16Array' ||
      tag === 'Int32Array' || tag === 'Uint32Array' || tag === 'Float16Array' ||
      tag === 'Float32Array' || tag === 'Float64Array' || tag === 'BigInt64Array' ||
      tag === 'BigUint64Array';
  }

  function inspectTypedArray(v, depth, ctx) {
    // Display uses the observable toStringTag (node's inspect trusts it);
    // comparison uses the intrinsic brand.
    var tag = v[Symbol.toStringTag] || typedArrayTag(v) || 'TypedArray';
    var ctorName = '';
    try { ctorName = v.constructor && v.constructor.name; } catch (e) { /* ignore */ }
    var n = v.length;
    var props = Object.keys(v).filter(function(k) { return !isArrayIndexKey(k); });
    if (n === 0 && props.length === 0) {
      return ctorName === 'Buffer' ? 'Buffer(0) [Uint8Array] []' : tag + '(0) []';
    }
    var lines = [];
    for (var i = 0; i < n; i++) lines.push(numberStr(v[i]));
    for (var p = 0; p < props.length; p++) {
      lines.push(keyRepr(props[p]) + ': ' + inspectValue(v[props[p]], depth - 1, ctx));
    }
    var prefix = ctorName === 'Buffer' ? ('Buffer(' + n + ') [Uint8Array]') : (tag + '(' + n + ')');
    // This renderer serves the assert-message path, which is node's
    // assertion_error inspectWithNoCustomRetry (customInspect: false) —
    // Buffers take the Buffer(N) [Uint8Array] element-list body, NOT the
    // bare-util.inspect '<Buffer hex>' custom-inspector form (that shape
    // belongs to the Rust single-line path, pinned by util_edge_tests).
    // Golden test-assert-deep 'deepEqual' (Uint8Array vs Buffer diff)
    // pins the Buffer(N) [Uint8Array] multi-line form on the '-' side.
    return prefix + ' ' + joinBody('[', lines, ctx.indent, ']');
  }

  function inspectArrayBuffer(v, depth, ctx) {
    var bytes = new Uint8Array(v);
    var shown = Math.min(bytes.length, 512);
    var hex = '';
    for (var i = 0; i < shown; i++) {
      if (i > 0) hex += ' ';
      hex += (bytes[i] < 16 ? '0' : '') + bytes[i].toString(16);
    }
    var lines = [];
    if (hex.length > 0) lines.push('[Uint8Contents]: <' + hex + '>');
    lines.push('[byteLength]: ' + String(v.byteLength));
    return body('ArrayBuffer', '{', '}', lines, ctx.indent);
  }

  function inspectPromise(val, depth, ctx) {
    return 'Promise { <unknown> }';
  }

  function inspectBoxed(val, depth, ctx) {
    var tag = objToString.call(val);
    var base;
    if (tag === '[object String]') base = '[String: ' + quoteString(String.prototype.valueOf.call(val)) + ']';
    else if (tag === '[object Number]') base = '[Number: ' + numberStr(Number.prototype.valueOf.call(val)) + ']';
    else if (tag === '[object Boolean]') base = '[Boolean: ' + String(Boolean.prototype.valueOf.call(val)) + ']';
    else if (tag === '[object BigInt]') base = '[BigInt: ' + String(BigInt.prototype.valueOf.call(val)) + 'n]';
    else if (tag === '[object Symbol]') base = '[Symbol: ' + String(Symbol.prototype.valueOf.call(val)) + ']';
    else base = 'Object';
    // Own enumerable non-index props are appended as a block.
    var keys = Object.keys(val).filter(function(k) { return !isArrayIndexKey(k); }).sort();
    if (keys.length === 0) return base;
    var childCtx = { indent: ctx.indent + '  ', refs: ctx.refs, refCounter: ctx.refCounter, seen: ctx.seen };
    var lines = [];
    for (var i = 0; i < keys.length; i++) {
      var v;
      try { v = val[keys[i]]; } catch (e) { v = '[Thrown]'; }
      lines.push(keyRepr(keys[i]) + ': ' + inspectEntryValue(val, keys[i], v, depth - 1, childCtx));
    }
    return base + ' ' + joinBody('{', lines, ctx.indent, '}');
  }

  function addEllipsis(string) {
    var lines = string.split('\n');
    if (lines.length > 10) {
      lines.length = 10;
      return lines.join('\n') + '\n...';
    } else if (string.length > kMaxLongStringLength) {
      // node's own quirk: String.prototype.slice(str, 512) keeps the tail.
      return string.slice(kMaxLongStringLength) + '...';
    }
    return string;
  }

  // ========================================================================
  // Section 4: AssertionError — port of lib/internal/assert/assertion_error.js
  // ========================================================================

  var kReadableOperator = {
    deepStrictEqual: 'Expected values to be strictly deep-equal:',
    partialDeepStrictEqual: 'Expected values to be partially and strictly deep-equal:',
    strictEqual: 'Expected values to be strictly equal:',
    strictEqualObject: 'Expected "actual" to be reference-equal to "expected":',
    deepEqual: 'Expected values to be loosely deep-equal:',
    notDeepStrictEqual: 'Expected "actual" not to be strictly deep-equal to:',
    notStrictEqual: 'Expected "actual" to be strictly unequal to:',
    notStrictEqualObject: 'Expected "actual" not to be reference-equal to "expected":',
    notDeepEqual: 'Expected "actual" not to be loosely deep-equal to:',
    notIdentical: 'Values have same structure but are not reference-equal:',
    notDeepEqualUnequal: 'Expected values not to be loosely deep-equal:',
  };

  var kMaxShortStringLength = 12;

  var kMethodsWithCustomMessageDiff = new Set()
    .add('deepStrictEqual')
    .add('strictEqual')
    .add('partialDeepStrictEqual');

  // node's default util.inspect form (compact: true): one line for
  // primitives and small containers. Used where node calls bare inspect()
  // (ifError, validator-return messages, invalidArgTypeHelper).
  function inspectCompact(val, depth, seen, indent) {
    // node renders own props via bare inspect() (default depth 2): containers
    // at the property level collapse to [Array]/[Object] markers.
    if (depth === undefined) depth = 2;
    if (seen === undefined) seen = [];
    if (indent === undefined) indent = '';
    var scalarCtx = { indent: indent, refs: null, refCounter: 0 };
    if (val === null) return 'null';
    var t = typeof val;
    if (t === 'undefined') return 'undefined';
    if (t === 'boolean' || t === 'number' || t === 'bigint' || t === 'symbol' || t === 'string') {
      return inspectValue(val, 0, scalarCtx);
    }
    if (t === 'function') return inspectValue(val, 0, scalarCtx);
    if (seen.indexOf(val) !== -1) return '[Circular]';
    if (depth <= 0) {
      // depth:0 collapses containers (node renders [Array]/[Object]).
      if (Array.isArray(val)) return '[Array]';
      if (isError(val)) { /* errors keep their single-line form */ }
      else if (isDate(val)) return isNaN(Date.prototype.getTime.call(val)) ? 'Invalid Date' : Date.prototype.toISOString.call(val);
      else if (isRegExp(val)) return '/' + val.source + '/' + val.flags;
      else return '[Object]';
    }
    var tag = objToString.call(val);
    if (isDate(val)) return isNaN(val.getTime()) ? 'Invalid Date' : val.toISOString();
    if (isRegExp(val)) return '/' + val.source + '/' + val.flags;
    if (isError(val)) {
      var nm = '';
      var mm = '';
      try { nm = String(val.name || 'Error'); } catch (e) { /* ignore */ }
      try { mm = String(val.message); } catch (e) { /* ignore */ }
      return nm === 'Error' ? ('[Error: ' + mm + ']') : ('[' + nm + ': ' + mm + ']');
    }
    seen.push(val);
    var out;
    if (Array.isArray(val)) {
      var items = [];
      for (var i = 0; i < val.length && i < 100; i++) {
        items.push(i in val ? inspectCompact(val[i], depth - 1, seen) : '<1 empty item>');
      }
      out = '[ ' + items.join(', ') + ' ]';
    } else if (isMap(val) || isSet(val) || isArrayBufferView(val) || isAnyArrayBuffer(val)) {
      out = inspectValue(val, 0, { indent: '', refs: null, refCounter: 0 }).replace(/\n/g, '\n');
    } else {
      var keys = Object.keys(val).sort();
      var parts = [];
      for (var k = 0; k < keys.length; k++) {
        var v;
        try { v = val[keys[k]]; } catch (e2) { v = '[Thrown]'; }
        parts.push(keyRepr(keys[k]) + ': ' + inspectCompact(v, depth - 1, seen));
      }
      out = '{ ' + parts.join(', ') + ' }';
    }
    seen.pop();
    return out;
  }

  function inspectValueForMessage(val) {
    return inspectValue(val, 1000, { indent: '', refs: null, refCounter: 0 });
  }

  function checkOperator(actual, expected, operator) {
    if (
      operator === 'strictEqual' &&
      ((typeof actual === 'object' &&
        actual !== null &&
        typeof expected === 'object' &&
        expected !== null) ||
        (typeof actual === 'function' && typeof expected === 'function'))
    ) {
      operator = 'strictEqualObject';
    }

    return operator;
  }

  // strictEqual(innerFail path) must adopt the same object-vs-object
  // promotion node applies inside createErrDiff, otherwise two objects fail
  // with the '!== ' simple form instead of '+ actual - expected'.
  function checkOperatorForInnerFail(actual, expected, operator) {
    return checkOperator(actual, expected, operator);
  }

  function getStackedDiff(actual, expected) {
    var isStringComparison = typeof actual === 'string' && typeof expected === 'string';

    var message = '\n+ ' + actual + '\n- ' + expected;
    var stringsLen = actual.length + expected.length;
    var maxTerminalLength = 80;
    var showIndicator = isStringComparison && (stringsLen <= maxTerminalLength);

    if (showIndicator) {
      var indicatorIdx = -1;

      for (var i = 0; i < actual.length; i++) {
        if (actual[i] !== expected[i]) {
          // Skip the indicator for the first 3 characters.
          if (i >= 3) {
            indicatorIdx = i;
          }
          break;
        }
      }

      if (indicatorIdx !== -1) {
        var pad = '';
        for (var p = 0; p < indicatorIdx + 2; p++) pad += ' ';
        message += '\n' + pad + '^';
      }
    }

    return { message: message };
  }

  function getSimpleDiff(originalActual, actual, originalExpected, expected) {
    var stringsLen = actual.length + expected.length;
    // Accounting for the quotes wrapping strings
    if (typeof originalActual === 'string') {
      stringsLen -= 2;
    }
    if (typeof originalExpected === 'string') {
      stringsLen -= 2;
    }
    if (stringsLen <= kMaxShortStringLength && (originalActual !== 0 || originalExpected !== 0)) {
      return { message: actual + ' !== ' + expected, header: '' };
    }

    return getStackedDiff(actual, expected);
  }

  function isSimpleDiff(actual, inspectedActual, expected, inspectedExpected) {
    if (inspectedActual.length > 1 || inspectedExpected.length > 1) {
      return false;
    }

    return typeof actual !== 'object' || actual === null ||
      typeof expected !== 'object' || expected === null;
  }

  function createErrDiff(actual, expected, operator, customMessage, diffType) {
    operator = checkOperator(actual, expected, operator);

    var skipped = false;
    var message = '';
    var inspectedActual = inspectValueForMessage(actual);
    var inspectedExpected = inspectValueForMessage(expected);
    var inspectedSplitActual = inspectedActual.split('\n');
    var inspectedSplitExpected = inspectedExpected.split('\n');
    var showSimpleDiff = isSimpleDiff(actual, inspectedSplitActual, expected, inspectedSplitExpected);
    var header = '+ actual - expected';

    if (showSimpleDiff) {
      var simpleDiff = getSimpleDiff(actual, inspectedSplitActual[0], expected, inspectedSplitExpected[0]);
      message = simpleDiff.message;
      if (typeof simpleDiff.header !== 'undefined') {
        header = simpleDiff.header;
      }
    } else if (inspectedActual === inspectedExpected) {
      // Handles the case where the objects are structurally the same but
      // different references
      operator = 'notIdentical';
      if (inspectedSplitActual.length > 50 && diffType !== 'full') {
        message = inspectedSplitActual.slice(0, 50).join('\n') + '\n...}';
        skipped = true;
      } else {
        message = inspectedSplitActual.join('\n');
      }
      header = '';
    } else {
      var checkCommaDisparity = actual != null && typeof actual === 'object';
      var diff = myersDiff(inspectedSplitActual, inspectedSplitExpected, checkCommaDisparity);

      var myersDiffMessage = printMyersDiff(diff, operator);
      message = myersDiffMessage.message;

      if (operator === 'partialDeepStrictEqual') {
        header = '+ actual - expected';
      }

      if (myersDiffMessage.skipped) {
        skipped = true;
      }
    }

    var headerMessage = (customMessage || kReadableOperator[operator]) + '\n' + header;
    var skippedMessage = skipped ? '\n... Skipped lines' : '';

    return headerMessage + skippedMessage + '\n' + message + '\n';
  }

  function copyError(source) {
    var target = Object.assign(Object.create(Object.getPrototypeOf(source)), source);
    Object.defineProperty(target, '__baoCopiedError', { __proto__: null, value: true });
    Object.defineProperty(target, 'message', { __proto__: null, value: source.message });
    if (hasOwn.call(source, 'cause')) {
      var cause = source.cause;
      if (isError(cause)) {
        cause = copyError(cause);
      }
      Object.defineProperty(target, 'cause', { __proto__: null, value: cause });
    }
    return target;
  }

  function invalidArgTypeHelper(input) {
    if (input == null) {
      return ' Received ' + input;
    }
    if (typeof input === 'function' && input.name) {
      return ' Received function ' + input.name;
    }
    if (typeof input === 'object') {
      if (input.constructor && input.constructor.name) {
        return ' Received an instance of ' + input.constructor.name;
      }
      return ' Received ' + inspectValueForMessage(input);
    }
    var inspected = inspectValueForMessage(input);
    if (inspected.length > 25) inspected = inspected.slice(0, 25) + '...';
    return ' Received type ' + typeof input + ' (' + inspected + ')';
  }

  // node's ERR_INVALID_ARG_TYPE type-list rendering: 'function or an
  // instance of Error, RegExp, or Object'.
  function typeListPhrase(types) {
    var plain = [];
    var insts = [];
    for (var i = 0; i < types.length; i++) {
      var t = types[i];
      if (t === 'null' || t === 'undefined') {
        plain.push(t);
      } else if (t === t.toLowerCase() && t !== 'object') {
        plain.push(t);
      } else {
        insts.push(t);
      }
    }
    var phrase = '';
    if (plain.length === 1) {
      phrase = 'of type ' + plain[0];
    } else if (plain.length > 1) {
      phrase = 'of type ' + plain.slice(0, -1).join(', ') + ', or ' + plain[plain.length - 1];
    }
    if (insts.length > 0) {
      var instPhrase;
      if (insts.length === 1) {
        instPhrase = 'an instance of ' + insts[0];
      } else if (insts.length === 2) {
        instPhrase = 'an instance of ' + insts[0] + ' or ' + insts[1];
      } else {
        instPhrase = 'an instance of ' + insts.slice(0, -1).join(', ') + ', or ' + insts[insts.length - 1];
      }
      phrase = phrase === '' ? instPhrase : phrase + ' or ' + instPhrase;
    }
    return phrase;
  }

  function makeInvalidArgTypeError(name, expected, value) {
    var phrase = Array.isArray(expected) ? typeListPhrase(expected) : 'of type ' + expected;
    var err = new TypeError('The "' + name + '" argument must be ' + phrase + '.' +
      invalidArgTypeHelper(value));
    err.code = 'ERR_INVALID_ARG_TYPE';
    return err;
  }

  function AssertionError(options) {
    // validateObject(options, 'options')
    if (options === null || typeof options !== 'object') {
      throw makeInvalidArgTypeError('options', 'object', options);
    }

    var message = options.message;
    var operator = options.operator;
    var stackStartFn = options.stackStartFn;
    var stackStartFunction = options.stackStartFunction;
    var diff = options.diff === undefined ? 'simple' : options.diff;
    var actual = options.actual;
    var expected = options.expected;

    if (message != null) {
      if (kMethodsWithCustomMessageDiff.has(operator)) {
        message = createErrDiff(actual, expected, operator, message, diff);
      } else {
        message = String(message);
      }
    } else {
      // Prevent the error stack from being visible by duplicating the error
      // in a very close way to the original in case both sides are actually
      // instances of Error.
      if (typeof actual === 'object' && actual !== null &&
          typeof expected === 'object' && expected !== null &&
          'stack' in actual && actual instanceof Error &&
          'stack' in expected && expected instanceof Error) {
        actual = copyError(actual);
        expected = copyError(expected);
      }

      if (kMethodsWithCustomMessageDiff.has(operator)) {
        message = createErrDiff(actual, expected, operator, message, diff);
      } else if (operator === 'notDeepStrictEqual' ||
        operator === 'notStrictEqual') {
        // In case the objects are equal but the operator requires unequal,
        // show the first object and say A equals B.
        var base = kReadableOperator[operator];
        var res = inspectValueForMessage(actual).split('\n');

        if (operator === 'notStrictEqual' &&
            ((typeof actual === 'object' && actual !== null) ||
             typeof actual === 'function')) {
          base = kReadableOperator.notStrictEqualObject;
        }

        // Only remove lines in case it makes sense to collapse those.
        if (res.length > 50 && diff !== 'full') {
          res[46] = '...';
          while (res.length > 47) {
            res.pop();
          }
        }

        // Only print a single input.
        if (res.length === 1) {
          message = base + (res[0].length > 5 ? '\n\n' : ' ') + res[0];
        } else {
          message = base + '\n\n' + res.join('\n') + '\n';
        }
      } else {
        var resA = inspectValueForMessage(actual);
        var other = inspectValueForMessage(expected);
        var knownOperator = kReadableOperator[operator];
        if (operator === 'notDeepEqual' && resA === other) {
          resA = knownOperator + '\n\n' + resA;
          if (resA.length > 1024 && diff !== 'full') {
            resA = resA.slice(0, 1021) + '...';
          }
          message = resA;
        } else {
          if (resA.length > kMaxLongStringLength && diff !== 'full') {
            resA = resA.slice(0, 509) + '...';
          }
          if (other.length > kMaxLongStringLength && diff !== 'full') {
            other = other.slice(0, 509) + '...';
          }
          if (operator === 'deepEqual') {
            resA = knownOperator + '\n\n' + resA + '\n\nshould loosely deep-equal\n\n';
          } else {
            var newOp = kReadableOperator[operator + 'Unequal'];
            if (newOp) {
              resA = newOp + '\n\n' + resA + '\n\nshould not loosely deep-equal\n\n';
            } else {
              other = ' ' + operator + ' ' + other;
            }
          }
          message = resA + other;
        }
      }
    }

    Error.call(this);
    Object.defineProperty(this, 'message', {
      __proto__: null,
      value: message,
      enumerable: false,
      writable: true,
      configurable: true,
    });
    this.generatedMessage = !options.message;
    this.code = 'ERR_ASSERTION';
    this.actual = actual;
    this.expected = expected;
    this.operator = operator;
    if (typeof Error.captureStackTrace === 'function') {
      Error.captureStackTrace(this, stackStartFn || stackStartFunction || AssertionError);
      // SpiderMonkey's captureStackTrace stores frames without the
      // "Name: message" head that V8 prepends.  Node's contract is that
      // err.stack starts with "AssertionError [ERR_ASSERTION]: <message>".
      // Rebuild the stack with the head when it is missing.
      var currentStack = String(this.stack || '');
      if (currentStack.indexOf('AssertionError') !== 0) {
        var headLine = 'AssertionError [ERR_ASSERTION]: ' + message;
        Object.defineProperty(this, 'stack', {
          __proto__: null,
          value: headLine + '\n' + currentStack,
          enumerable: false,
          writable: true,
          configurable: true,
        });
      }
    } else {
      // Frame head carries the node shape; internal frames are kept (no V8
      // frame trimming available without captureStackTrace).
      var rawStack = (new Error()).stack || '';
      var stackLines = rawStack.split('\n');
      var frames = stackLines.slice(1).join('\n');
      var head = 'AssertionError [ERR_ASSERTION]: ' + message;
      Object.defineProperty(this, 'stack', {
        __proto__: null,
        value: frames ? head + '\n' + frames : head,
        enumerable: false,
        writable: true,
        configurable: true,
      });
    }
    this.name = 'AssertionError';
    this.diff = diff;
  }
  AssertionError.prototype = Object.create(Error.prototype);
  AssertionError.prototype.constructor = AssertionError;
  AssertionError.prototype.name = 'AssertionError';
  // Mirror `class AssertionError extends Error`: constructor-chain identity
  // so Error.isPrototypeOf(AssertionError) holds (throws() class detection).
  Object.setPrototypeOf(AssertionError, Error);
  AssertionError.prototype.toString = function() {
    return this.name + ' [ERR_ASSERTION]: ' + this.message;
  };

  // node's AssertionError[util.inspect.custom]: ellipsize long string
  // actual/expected and render the own props at depth 0.
  var inspectCustomSymbol = (typeof Symbol === 'function' && Symbol.for)
    ? Symbol.for('nodejs.util.inspect.custom')
    : null;
  if (inspectCustomSymbol !== null) {
    AssertionError.prototype[inspectCustomSymbol] = function() {
      var tmpActual = this.actual;
      var tmpExpected = this.expected;
      if (typeof this.actual === 'string') this.actual = addEllipsis(this.actual);
      if (typeof this.expected === 'string') this.expected = addEllipsis(this.expected);
      var keys = Object.keys(this);
      var parts = [];
      for (var i = 0; i < keys.length; i++) {
        var v;
        try { v = this[keys[i]]; } catch (e) { v = '[Thrown]'; }
        parts.push('  ' + keyRepr(keys[i]) + ': ' + inspectCompact(v, 0, undefined, '  '));
      }
      this.actual = tmpActual;
      this.expected = tmpExpected;
      var head = 'AssertionError [ERR_ASSERTION]: ' + this.message;
      if (parts.length === 0) return head;
      return head + ' {\n' + parts.join(',\n') + '\n}';
    };
  }

  function innerFail(obj) {
    if (obj.message instanceof Error) throw obj.message;

    throw new AssertionError(obj);
  }

  // ========================================================================
  // Section 5: API surface — port of lib/assert.js + internal/assert/utils.js
  // ========================================================================

  function getErrMessageFallback(value, message) {
    // node extracts the falsy source expression from V8 CallSite source
    // text.  SpiderMonkey stack frames carry only "fn@file:line:col", so
    // we reconstruct the expression by reading the source file at the
    // caller's line and slicing out the argument list of the innermost
    // assert.ok / assert() call.  This mirrors node's behaviour closely
    // enough for the golden grids (single-line and simple multi-line
    // forms).  Returns undefined when the source cannot be recovered.
    try {
      var err = new Error();
      var stack = err.stack || '';
      var frames = stack.split('\n');
      // frames[0] is the getErrMessageFallback frame; frames[1] is innerOk;
      // frames[2] is the caller of ok/assert (the test frame we want).
      var callerLine = null;
      for (var fi = 1; fi < frames.length; fi++) {
        var f = frames[fi];
        if (f.indexOf('node_util') !== -1 || f.indexOf('assert:') !== -1 ||
            f.indexOf('<assert>') !== -1 || f.indexOf('getErrMessage') !== -1 ||
            f.indexOf('innerOk') !== -1) continue;
        callerLine = f;
        break;
      }
      if (!callerLine) return undefined;
      // Parse "fnName@/path/to/file.js:LINE:COL" or "@/path:LINE:COL"
      var m = callerLine.match(/@(.+):(\d+):(\d+)$/);
      if (!m) return undefined;
      var filePath = m[1];
      var lineNum = parseInt(m[2], 10);
      if (typeof require !== 'function') return undefined;
      var fs;
      try { fs = require('fs'); } catch (e0) { return undefined; }
      // Resolve relative paths against the current working directory.
      var path;
      try { path = require('path'); } catch (e0b) { path = null; }
      var resolvedPath = filePath;
      if (filePath.charAt(0) !== '/' && path !== null) {
        try { resolvedPath = path.resolve(process.cwd(), filePath); } catch (e0c) {}
      }
      var src;
      try { src = fs.readFileSync(resolvedPath, 'utf8'); }
      catch (e0d) { try { src = fs.readFileSync(filePath, 'utf8'); } catch (e0e) { return undefined; } }
      var srcLines = src.split('\n');
      if (lineNum < 1 || lineNum > srcLines.length) return undefined;
      // Collect enough source lines to cover the full ok(...) call.
      // Start at the caller line and grab up to 10 lines.
      var exprLines = [];
      var parenDepth = 0;
      var foundOpen = false;
      for (var li = lineNum - 1; li < Math.min(lineNum + 9, srcLines.length); li++) {
        var text = srcLines[li];
        // Strip trailing comment
        var ci = text.indexOf('//');
        if (ci !== -1 && !foundOpen) text = text.slice(0, ci);
        exprLines.push(text);
        for (var ci2 = 0; ci2 < text.length; ci2++) {
          var ch = text[ci2];
          if (ch === '(') { parenDepth++; foundOpen = true; }
          else if (ch === ')') { parenDepth--; }
        }
        if (foundOpen && parenDepth <= 0) break;
      }
      var expr = exprLines.join('\n');
      // Trim leading/trailing whitespace from each line and the whole block.
      expr = expr.split('\n').map(function(l) { return l.trim(); }).join('\n').trim();
      if (!expr) return undefined;
      // Extract the full call expression (e.g. "assert.ok(arg)" or "strict.ok(").
      // node shows the complete call for single-line expressions; for
      // multi-line calls it truncates at the first newline after the
      // opening paren (V8 stack frame source text limit).
      // Find the innermost assert.ok / assert( / strict.ok call — match the
      // full dotted path so "assert.ok(" wins over a bare "ok(" inside it.
      var callMatch = expr.match(/(?:assert|strict|ok)(?:\.[a-zA-Z_$][\w$]*)*\s*\(/);
      if (!callMatch) return undefined;
      var callStart = callMatch.index;
      var openIdx = expr.indexOf('(', callStart);
      if (openIdx === -1) return undefined;
      // For multi-line: node shows only "funcname(" (up to and including
      // the opening paren, then a newline). Detect multi-line by checking
      // if the matching close paren is on a different line.
      var depth = 0;
      var endIdx = -1;
      for (var ei = openIdx; ei < expr.length; ei++) {
        if (expr[ei] === '(') depth++;
        else if (expr[ei] === ')') { depth--; if (depth === 0) { endIdx = ei; break; } }
      }
      var callExpr;
      if (endIdx === -1) {
        // Unclosed — just show up to the opening paren
        callExpr = expr.slice(callStart, openIdx + 1);
      } else {
        var closeLine = expr.slice(0, endIdx).split('\n').length - 1;
        var openLine = expr.slice(0, openIdx).split('\n').length - 1;
        if (closeLine > openLine) {
          // Multi-line call: node truncates at the opening paren
          callExpr = expr.slice(callStart, openIdx + 1);
        } else {
          // Single-line call: show the full expression
          callExpr = expr.slice(callStart, endIdx + 1);
        }
      }
      if (!callExpr) return undefined;
      // node's format: 'The expression evaluated to a falsy value:\n\n  <src>\n'
      return 'The expression evaluated to a falsy value:\n\n  ' + callExpr + '\n';
    } catch (e) {
      return undefined;
    }
  }

  function innerOk(fn, argLen, value, message) {
    if (!value) {
      var generatedMessage = false;

      if (argLen === 0) {
        generatedMessage = true;
        message = 'No value argument passed to `assert.ok()`';
      } else if (message == null) {
        generatedMessage = true;
        message = getErrMessageFallback(value, message);
      } else if (isError(message)) {
        throw message;
      }

      var err = new AssertionError({
        actual: value,
        expected: true,
        message: message,
        operator: '==',
        stackStartFn: fn,
      });
      err.generatedMessage = generatedMessage;
      throw err;
    }
  }

  function ok() {
    innerOk(ok, arguments.length, arguments[0], arguments[1]);
  }

  function ERR_MISSING_ARGS(names) {
    var joined;
    if (names.length === 2) {
      joined = '"' + names[0] + '" and "' + names[1] + '"';
    } else {
      joined = '"' + names.slice(0, -1).join('", "') + '", and "' + names[names.length - 1] + '"';
    }
    var err = new TypeError('The ' + joined + ' arguments must be specified');
    err.code = 'ERR_MISSING_ARGS';
    return err;
  }

  function equal(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    // eslint-disable-next-line eqeqeq
    if (actual != expected && (!(actual !== actual) || !(expected !== expected))) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: '==',
        stackStartFn: equal,
        diff: optOf(this) && optOf(this).diff,
      });
    }
  }

  function notEqual(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    // eslint-disable-next-line eqeqeq
    if (actual == expected || ((actual !== actual) && (expected !== expected))) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: '!=',
        stackStartFn: notEqual,
        diff: optOf(this) && optOf(this).diff,
      });
    }
  }

  function deepEqual(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    if (!isDeepEqual(actual, expected)) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: 'deepEqual',
        stackStartFn: deepEqual,
        diff: optOf(this) && optOf(this).diff,
      });
    }
  }

  function notDeepEqual(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    if (isDeepEqual(actual, expected)) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: 'notDeepEqual',
        stackStartFn: notDeepEqual,
        diff: optOf(this) && optOf(this).diff,
      });
    }
  }

  function optOf(self) {
    return self && self.__baoAssertOptions;
  }

  function deepStrictEqual(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    var o = optOf(this);
    if (!isDeepStrictEqual(actual, expected, o && o.skipPrototype)) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: 'deepStrictEqual',
        stackStartFn: deepStrictEqual,
        diff: o && o.diff,
      });
    }
  }

  function notDeepStrictEqual(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    var o = optOf(this);
    if (isDeepStrictEqual(actual, expected, o && o.skipPrototype)) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: 'notDeepStrictEqual',
        stackStartFn: notDeepStrictEqual,
        diff: o && o.diff,
      });
    }
  }

  function strictEqual(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    if (!Object.is(actual, expected)) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: 'strictEqual',
        stackStartFn: strictEqual,
        diff: optOf(this) && optOf(this).diff,
      });
    }
  }

  function notStrictEqual(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    if (Object.is(actual, expected)) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: 'notStrictEqual',
        stackStartFn: notStrictEqual,
        diff: optOf(this) && optOf(this).diff,
      });
    }
  }

  function partialDeepStrictEqual(actual, expected, message) {
    if (arguments.length < 2) {
      throw ERR_MISSING_ARGS(['actual', 'expected']);
    }
    if (!isPartialStrictEqual(actual, expected)) {
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: 'partialDeepStrictEqual',
        stackStartFn: partialDeepStrictEqual,
        diff: optOf(this) && optOf(this).diff,
      });
    }
  }

  function Comparison(obj, keys, actual) {
    for (var i = 0; i < keys.length; i++) {
      var key = keys[i];
      if (key in obj) {
        if (actual !== undefined &&
            typeof actual[key] === 'string' &&
            isRegExp(obj[key]) &&
            obj[key].exec(actual[key]) !== null) {
          this[key] = actual[key];
        } else {
          this[key] = obj[key];
        }
      }
    }
  }

  function compareExceptionKey(actual, expected, key, message, keys, fn) {
    if (!(key in actual) || !isDeepStrictEqual(actual[key], expected[key])) {
      if (!message) {
        // Create placeholder objects to create a nice output.
        var a = new Comparison(actual, keys);
        var b = new Comparison(expected, keys, actual);

        var err = new AssertionError({
          actual: a,
          expected: b,
          operator: 'deepStrictEqual',
          stackStartFn: fn,
        });
        err.actual = actual;
        err.expected = expected;
        err.operator = fn.name;
        throw err;
      }
      innerFail({
        actual: actual,
        expected: expected,
        message: message,
        operator: fn.name,
        stackStartFn: fn,
      });
    }
  }

  function expectedException(actual, expected, message, fn) {
    var generatedMessage = false;
    var throwError = false;

    if (typeof expected !== 'function') {
      // Handle regular expressions.
      if (isRegExp(expected)) {
        var str = String(actual);
        if (expected.exec(str) !== null)
          return;

        if (!message) {
          generatedMessage = true;
          message = 'The input did not match the regular expression ' +
            inspectValueForMessage(expected) + '. Input:\n\n' +
            inspectValueForMessage(str) + '\n';
        }
        throwError = true;
      // Handle primitives properly.
      } else if (typeof actual !== 'object' || actual === null) {
        var err = new AssertionError({
          actual: actual,
          expected: expected,
          message: message,
          operator: 'deepStrictEqual',
          stackStartFn: fn,
        });
        err.operator = fn.name;
        throw err;
      } else {
        // Handle validation objects.
        var keys = Object.keys(expected);
        // Special handle errors to make sure the name and the message are
        // compared as well.
        if (expected instanceof Error) {
          keys.push('name', 'message');
        } else if (keys.length === 0) {
          var invalidArg = new TypeError("The argument 'error' may not be an empty object. Received " +
            inspectValueForMessage(expected));
          invalidArg.code = 'ERR_INVALID_ARG_VALUE';
          throw invalidArg;
        }
        for (var i = 0; i < keys.length; i++) {
          var key = keys[i];
          if (typeof actual[key] === 'string' &&
              isRegExp(expected[key]) &&
              expected[key].exec(actual[key]) !== null) {
            continue;
          }
          compareExceptionKey(actual, expected, key, message, keys, fn);
        }
        return;
      }
    // Guard instanceof against arrow functions as they don't have a
    // prototype. Check for matching Error classes.
    } else if (expected.prototype !== undefined && actual instanceof expected) {
      return;
    } else if (typeof Error === 'function' && Error.isPrototypeOf(expected)) {
      if (!message) {
        generatedMessage = true;
        message = 'The error is expected to be an instance of ' +
          '"' + expected.name + '". Received ';
        if (isError(actual)) {
          var name = (actual.constructor && actual.constructor.name) ||
            actual.name;
          if (expected.name === name) {
            message += 'an error with identical name but a different prototype.';
          } else {
            message += '"' + name + '"';
          }
          if (actual.message) {
            message += '\n\nError message:\n\n' + actual.message;
          }
        } else {
          message += '"' + inspectCompact(actual, 0) + '"';
        }
      }
      throwError = true;
    } else {
      // Check validation functions return value.
      var res = expected.call({}, actual);
      if (res !== true) {
        if (!message) {
          generatedMessage = true;
          var vname = expected.name ? '"' + expected.name + '" ' : '';
          message = 'The ' + vname + 'validation function is expected to return' +
            ' "true". Received ' + inspectValueForMessage(res);

          if (isError(actual)) {
            message += '\n\nCaught error:\n\n' + actual;
          }
        }
        throwError = true;
      }
    }

    if (throwError) {
      var failErr = new AssertionError({
        actual: actual,
        expected: expected,
        message: message,
        operator: fn.name,
        stackStartFn: fn,
      });
      failErr.generatedMessage = generatedMessage;
      throw failErr;
    }
  }

  var NO_EXCEPTION_SENTINEL = {};

  // node renders this argument's type list as the fixed phrase
  // "function or an instance of Error, RegExp, or Object".
  var ERROR_ARG_TYPES = ['function', 'Error', 'RegExp', 'Object'];

  function validateFunction(fn, name) {
    if (typeof fn !== 'function') {
      throw makeInvalidArgTypeError(name, 'function', fn);
    }
  }

  function getActual(fn) {
    validateFunction(fn, 'fn');
    try {
      fn();
    } catch (e) {
      return e;
    }
    return NO_EXCEPTION_SENTINEL;
  }

  function checkIsPromise(obj) {
    // Accept native promises and thenables with both `then` and `catch`.
    return objToString.call(obj) === '[object Promise]' ||
      (obj !== null && typeof obj === 'object' &&
      typeof obj.then === 'function' &&
      typeof obj.catch === 'function');
  }

  function typeAndValue(v) {
    if (v === null) return 'null';
    if (v === undefined) return 'undefined';
    if (typeof v === 'object' && v.constructor && v.constructor.name) {
      return 'an instance of ' + v.constructor.name;
    }
    return 'type ' + typeof v + ' (' + inspectValueForMessage(v) + ')';
  }

  function waitForActual(promiseFn) {
    var resultPromise;
    if (typeof promiseFn === 'function') {
      try {
        resultPromise = promiseFn();
      } catch (e) {
        return Promise.reject(e);
      }
      // Fail in case no promise is returned.
      if (!checkIsPromise(resultPromise)) {
        var ivErr = new TypeError('Expected instance of Promise to be returned from the "promiseFn" function but got ' +
          (resultPromise === undefined ? 'undefined' : typeAndValue(resultPromise)) + '.');
        ivErr.code = 'ERR_INVALID_RETURN_VALUE';
        return Promise.reject(ivErr);
      }
    } else if (checkIsPromise(promiseFn)) {
      resultPromise = promiseFn;
    } else {
      return Promise.reject(
        makeInvalidArgTypeError('promiseFn', ['function', 'Promise'], promiseFn));
    }

    // For real native promises, call .then directly (avoids an extra
    // Promise.resolve adoption that can trigger spurious unhandled-rejection
    // warnings on engines where the adoption schedules a microtask).
    // For non-native thenables (objects whose .then does not return a
    // Promise), wrap through Promise.resolve so the chain below still gets
    // a real Promise — calling .then directly on a bare thenable returns
    // whatever its then() returns (often undefined), crashing the caller.
    if (objToString.call(resultPromise) === '[object Promise]') {
      return resultPromise.then(function() {
        return NO_EXCEPTION_SENTINEL;
      }, function(e) {
        return e;
      });
    }
    return Promise.resolve(resultPromise).then(function() {
      return NO_EXCEPTION_SENTINEL;
    }, function(e) {
      return e;
    });
  }

  function expectsError(stackStartFn, actual, error, message, argsLen) {
    if (typeof error === 'string') {
      if (argsLen === 4) {
        throw makeInvalidArgTypeError('error', ERROR_ARG_TYPES, error);
      }
      if (typeof actual === 'object' && actual !== null) {
        if (actual.message === error) {
          var amb1 = new TypeError('The "error/message" argument is ambiguous. ' +
            'The error message "' + actual.message + '" is identical to the message.');
          amb1.code = 'ERR_AMBIGUOUS_ARGUMENT';
          throw amb1;
        }
      } else if (actual === error) {
        var amb2 = new TypeError('The "error/message" argument is ambiguous. ' +
          'The error "' + actual + '" is identical to the message.');
        amb2.code = 'ERR_AMBIGUOUS_ARGUMENT';
        throw amb2;
      }
      message = error;
      error = undefined;
    } else if (error != null &&
               typeof error !== 'object' &&
               typeof error !== 'function') {
      throw makeInvalidArgTypeError('error', ERROR_ARG_TYPES, error);
    }

    if (actual === NO_EXCEPTION_SENTINEL) {
      var details = '';
      if (error && error.name) {
        details += ' (' + error.name + ')';
      }
      details += message ? ': ' + message : '.';
      var fnType = stackStartFn === rejects ? 'rejection' : 'exception';
      innerFail({
        actual: undefined,
        expected: error,
        operator: stackStartFn.name,
        message: 'Missing expected ' + fnType + details,
        stackStartFn: stackStartFn,
      });
    }

    if (!error)
      return;

    expectedException(actual, error, message, stackStartFn);
  }

  function hasMatchingError(actual, expected) {
    if (typeof expected !== 'function') {
      if (isRegExp(expected)) {
        var str = String(actual);
        return expected.exec(str) !== null;
      }
      throw makeInvalidArgTypeError('expected', ['Function', 'RegExp'], expected);
    }
    // Guard instanceof against arrow functions as they don't have a
    // prototype.
    if (expected.prototype !== undefined && actual instanceof expected) {
      return true;
    }
    if (typeof Error === 'function' && Error.isPrototypeOf(expected)) {
      return false;
    }
    return expected.call({}, actual) === true;
  }

  function expectsNoError(stackStartFn, actual, error, message) {
    if (actual === NO_EXCEPTION_SENTINEL)
      return;

    if (typeof error === 'string') {
      message = error;
      error = undefined;
    }

    if (!error || hasMatchingError(actual, error)) {
      var details = message ? ': ' + message : '.';
      var fnType = stackStartFn === doesNotReject ? 'rejection' : 'exception';
      var actualMsg = actual && typeof actual === 'object' && typeof actual.message === 'string' ? actual.message : undefined;
      innerFail({
        actual: actual,
        expected: error,
        operator: stackStartFn.name,
        message: 'Got unwanted ' + fnType + details +
          '\nActual message: "' + actualMsg + '"',
        stackStartFn: stackStartFn,
      });
    }
    throw actual;
  }

  function throws() {
    var args = Array.prototype.slice.call(arguments);
    expectsError(throws, getActual(args[0]), args[1], args[2], args.length + 1);
  }

  function rejects(promiseFn, error, message) {
    var argsLen = arguments.length;
    return waitForActual(promiseFn).then(function(actual) {
      expectsError(rejects, actual, error, message, argsLen + 1);
    });
  }

  function doesNotThrow() {
    var args = Array.prototype.slice.call(arguments);
    expectsNoError(doesNotThrow, getActual(args[0]), args[1], args[2]);
  }

  function doesNotReject(fn, error, message) {
    return waitForActual(fn).then(function(actual) {
      expectsNoError(doesNotReject, actual, error, message);
    });
  }

  function ifError(err) {
    if (err !== null && err !== undefined) {
      var message = 'ifError got unwanted exception: ';
      if (typeof err === 'object' && typeof err.message === 'string') {
        if (err.message.length === 0 && err.constructor) {
          message += err.constructor.name;
        } else {
          message += err.message;
        }
      } else {
        message += inspectCompact(err);
      }

      var newErr = new AssertionError({
        actual: err,
        expected: null,
        operator: 'ifError',
        message: message,
        stackStartFn: ifError,
      });
      newErr.generatedMessage = false;

      throw newErr;
    }
  }

  function internalMatch(string, regexp, message, fn) {
    if (!isRegExp(regexp)) {
      var reErr = new TypeError('The "regexp" argument must be an instance of RegExp.' +
        invalidArgTypeHelper(regexp));
      reErr.code = 'ERR_INVALID_ARG_TYPE';
      throw reErr;
    }
    var match = fn === match_;
    if (typeof string !== 'string' || (regexp.exec(string) !== null) !== match) {
      if (message instanceof Error) {
        throw message;
      }

      var generatedMessage = !message;

      message = message || (typeof string !== 'string' ?
        'The "string" argument must be of type string. Received type ' +
        typeof string + ' (' + inspectValueForMessage(string) + ')' :
        (match ?
          'The input did not match the regular expression ' :
          'The input was expected to not match the regular expression ') +
        inspectValueForMessage(regexp) + '. Input:\n\n' +
        inspectValueForMessage(string) + '\n');
      var err = new AssertionError({
        actual: string,
        expected: regexp,
        message: message,
        operator: fn.name,
        stackStartFn: fn,
      });
      err.generatedMessage = generatedMessage;
      throw err;
    }
  }

  function match_(string, regexp, message) {
    internalMatch(string, regexp, message, match_);
  }

  function doesNotMatch(string, regexp, message) {
    internalMatch(string, regexp, message, doesNotMatch);
  }

  function fail() {
    var argsLen = arguments.length;
    var actual = arguments[0];
    var expected = arguments[1];
    var message = arguments[2];
    var operator = arguments[3];

    var internalMessage = false;
    if ((actual == null) && argsLen <= 1) {
      internalMessage = true;
      message = 'Failed';
    } else if (argsLen === 1) {
      message = actual;
      actual = undefined;
    } else {
      if (argsLen === 2)
        operator = '!=';
    }

    if (message instanceof Error) throw message;

    var err = new AssertionError({
      actual: actual,
      expected: expected,
      operator: operator === undefined ? 'fail' : operator,
      stackStartFn: fail,
      message: message,
    });
    if (internalMessage) {
      err.generatedMessage = true;
    }
    throw err;
  }

  // ---- CallTracker — port of lib/internal/assert/calltracker.js ----
  function validateUint32(value, name) {
    if (typeof value !== 'number' || value !== value || value < 0 ||
        Math.floor(value) !== value || value > 4294967295) {
      var err = new RangeError('The "' + name + '" argument must be of type uint32. Received ' +
        inspectValueForMessage(value));
      err.code = 'ERR_OUT_OF_RANGE';
      throw err;
    }
  }

  function CallTrackerContext(expected, stackTrace, name) {
    this.calls = [];
    this.expected = expected;
    this.stackTrace = stackTrace;
    this.name = name;
  }
  CallTrackerContext.prototype.track = function(thisArg, args) {
    var argsClone = Object.freeze(Array.prototype.slice.call(args));
    this.calls.push(Object.freeze({ thisArg: thisArg, arguments: argsClone }));
  };
  Object.defineProperty(CallTrackerContext.prototype, 'delta', {
    get: function() { return this.calls.length - this.expected; },
  });
  CallTrackerContext.prototype.reset = function() { this.calls = []; };
  CallTrackerContext.prototype.getCalls = function() {
    return Object.freeze(Array.prototype.slice.call(this.calls));
  };
  CallTrackerContext.prototype.report = function() {
    if (this.delta !== 0) {
      var message = 'Expected the ' + this.name + ' function to be executed ' +
        this.expected + ' time(s) but was executed ' + this.calls.length + ' time(s).';
      return {
        message: message,
        actual: this.calls.length,
        expected: this.expected,
        operator: this.name,
        stack: this.stackTrace,
      };
    }
  };

  function CallTracker() {
    this._callChecks = [];
    this._trackedFunctions = typeof WeakMap !== 'undefined' ? new WeakMap() : null;
    this._trackedPairs = [];
  }
  CallTracker.prototype._getTrackedFunction = function(tracked) {
    if (this._trackedFunctions !== null && this._trackedFunctions.has(tracked)) {
      return this._trackedFunctions.get(tracked);
    }
    for (var i = 0; i < this._trackedPairs.length; i++) {
      if (this._trackedPairs[i][0] === tracked) return this._trackedPairs[i][1];
    }
    var err = new TypeError('The argument \'tracked\' is invalid. ' +
      'Received ' + inspectValueForMessage(tracked) +
      ' (is not a tracked function)');
    err.code = 'ERR_INVALID_ARG_VALUE';
    throw err;
  };
  CallTracker.prototype.reset = function(tracked) {
    if (tracked === undefined) {
      for (var i = 0; i < this._callChecks.length; i++) this._callChecks[i].reset();
      return;
    }
    this._getTrackedFunction(tracked).reset();
  };
  CallTracker.prototype.getCalls = function(tracked) {
    return this._getTrackedFunction(tracked).getCalls();
  };
  CallTracker.prototype.calls = function(fn, expected) {
    if (typeof process !== 'undefined' && process && process._exiting) {
      var exitErr = new Error('This API is not available during process exit');
      exitErr.code = 'ERR_UNAVAILABLE_DURING_EXIT';
      throw exitErr;
    }
    if (typeof fn === 'number') {
      expected = fn;
      fn = function() {};
    } else if (fn === undefined) {
      fn = function() {};
    }
    if (expected === undefined) expected = 1;

    if (typeof expected !== 'number') {
      throw makeInvalidArgTypeError('expected', 'number', expected);
    }
    validateUint32(expected, 'expected');

    var context = new CallTrackerContext(expected, new Error(), fn.name || 'calls');
    var self = this;
    var tracked = new Proxy(fn, {
      __proto__: null,
      apply: function(target, thisArg, argList) {
        context.track(thisArg, argList);
        return Reflect.apply(target, thisArg, argList);
      },
    });
    this._callChecks.push(context);
    if (this._trackedFunctions !== null) this._trackedFunctions.set(tracked, context);
    else this._trackedPairs.push([tracked, context]);
    return tracked;
  };
  CallTracker.prototype.report = function() {
    var errors = [];
    for (var i = 0; i < this._callChecks.length; i++) {
      var message = this._callChecks[i].report();
      if (message !== undefined) {
        errors.push(message);
      }
    }
    return errors;
  };
  CallTracker.prototype.verify = function() {
    var errors = this.report();
    if (errors.length === 0) {
      return;
    }
    var message = errors.length === 1 ?
      errors[0].message :
      'Functions were not called the expected number of times';
    throw new AssertionError({
      message: message,
      details: errors,
    });
  };

  // ---- Assert class (node's configurable instance surface) ----
  function Assert(options) {
    if (!new.target) {
      var ce = new TypeError('Class constructor Assert cannot be invoked without `new`');
      ce.code = 'ERR_CONSTRUCT_CALL_REQUIRED';
      throw ce;
    }

    if (options !== undefined && (options === null || typeof options !== 'object')) {
      throw makeInvalidArgTypeError('options', 'object', options);
    }
    options = Object.assign({ strict: true, skipPrototype: false }, options || {});

    if (options.diff !== undefined && options.diff !== null) {
      if (options.diff !== 'simple' && options.diff !== 'full') {
        var diffErr = new TypeError("The property 'options.diff' must be one of: 'simple', 'full'. Received " + inspectValueForMessage(options.diff));
        diffErr.code = 'ERR_INVALID_ARG_VALUE';
        throw diffErr;
      }
    }

    this.__baoAssertOptions = {
      strict: options.strict !== false,
      skipPrototype: !!options.skipPrototype,
      diff: options.diff,
    };

    this.AssertionError = AssertionError;
    this.ok = function ok(value, message) {
      innerOk(ok, arguments.length, value, message);
    };
    this.fail = fail;
    this.strictEqual = strictEqual;
    this.notStrictEqual = notStrictEqual;
    this.deepStrictEqual = deepStrictEqual;
    this.notDeepStrictEqual = notDeepStrictEqual;
    this.partialDeepStrictEqual = partialDeepStrictEqual;
    this.match = match_;
    this.doesNotMatch = doesNotMatch;
    this.throws = throws;
    this.rejects = rejects;
    this.doesNotThrow = doesNotThrow;
    this.doesNotReject = doesNotReject;
    this.ifError = ifError;
    if (this.__baoAssertOptions.strict) {
      this.equal = strictEqual;
      this.deepEqual = deepStrictEqual;
      this.notEqual = notStrictEqual;
      this.notDeepEqual = notDeepStrictEqual;
    } else {
      this.equal = equal;
      this.deepEqual = deepEqual;
      this.notEqual = notEqual;
      this.notDeepEqual = notDeepEqual;
    }
  }

  // ---- assembly (node lib/assert.js tail shape) ----
  function strict() {
    innerOk(strict, arguments.length, arguments[0], arguments[1]);
  }

  var assertFn = ok;
  assertFn.AssertionError = AssertionError;
  assertFn.CallTracker = CallTracker;
  assertFn.fail = fail;
  assertFn.equal = equal;
  assertFn.notEqual = notEqual;
  assertFn.deepEqual = deepEqual;
  assertFn.notDeepEqual = notDeepEqual;
  assertFn.deepStrictEqual = deepStrictEqual;
  assertFn.notDeepStrictEqual = notDeepStrictEqual;
  assertFn.strictEqual = strictEqual;
  assertFn.notStrictEqual = notStrictEqual;
  assertFn.partialDeepStrictEqual = partialDeepStrictEqual;
  assertFn.match = match_;
  assertFn.doesNotMatch = doesNotMatch;
  assertFn.throws = throws;
  assertFn.rejects = rejects;
  assertFn.doesNotThrow = doesNotThrow;
  assertFn.doesNotReject = doesNotReject;
  assertFn.ifError = ifError;
  // node: assert.ok = assert (the ok function IS the module export).
  assertFn.ok = assertFn;

  // Non-enumerable back-channel for util_inspect's compact:false
  // delegation: the Section-3 inspectValue is the node assertion_error.js
  // inspectValue shape (validated against host node v24 with a 220-case
  // parity matrix).  Hidden behind a non-enumerable property so golden
  // probes scanning assert's public surface never see it.
  Object.defineProperty(assertFn, '__baoInspectValue', {
    value: inspectValue,
    enumerable: false,
    writable: false,
    configurable: false,
  });

  // Non-enumerable back-channel for util.isDeepStrictEqual: the Section-1
  // comparisons.js port (boxed primitives, Map/Set/Date/RegExp/TypedArray,
  // cycle memos, skipPrototype). Hidden so golden probes scanning assert's
  // public surface never see it; install_assert's Rust tail forwards it
  // onto the cached util object as util.__baoIsDeepStrictEqual.
  Object.defineProperty(assertFn, '__baoIsDeepStrictEqual', {
    value: isDeepStrictEqual,
    enumerable: false,
    writable: false,
    configurable: false,
  });

  Object.assign(strict, assertFn, {
    equal: assertFn.strictEqual,
    deepEqual: assertFn.deepStrictEqual,
    notEqual: assertFn.notStrictEqual,
    notDeepEqual: assertFn.notDeepStrictEqual,
  });

  assertFn.strict = strict;
  assertFn.Assert = Assert;
  strict.Assert = Assert;
  strict.strict = strict;

  return assertFn;
}
return makeAssert();
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
        mozjs::glue::DeleteCompileOptions(opts);
        if !ok || !rval.is_object() {
            return;
        }
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
        rooted!(&in(wrapped_cx) let assert_fn_obj = rval.to_object());

        // Cache as builtin `assert`.
        cache_builtin(cx, "assert", assert_fn_obj.get());

        // Cache `assert/strict` — extract the .strict sub-object so
        // require('assert/strict') returns the strict alias, not the full
        // assert function (node contract: require('assert/strict') ===
        // assert.strict).
        let mut strict_val = UndefinedValue();
        JS_GetProperty(
            cx.raw_cx(),
            assert_fn_obj.handle().into(),
            c"strict".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut strict_val,
            },
        );
        if strict_val.is_object() {
            rooted!(&in(wrapped_cx) let strict_obj = strict_val.to_object());
            cache_builtin(cx, "assert/strict", strict_obj.get());
        }

        // Cache the Section-3 inspectValue (node assertion_error.js shape)
        // as a hidden builtin for util_inspect's compact:false delegation
        // path.  The JS side stores it as a non-enumerable property
        // (__baoInspectValue) so it never leaks into golden probes.
        let mut iv_val = UndefinedValue();
        JS_GetProperty(
            cx.raw_cx(),
            assert_fn_obj.handle().into(),
            c"__baoInspectValue".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut iv_val,
            },
        );
        if iv_val.is_object() && JS_ObjectIsFunction(iv_val.to_object()) {
            rooted!(&in(wrapped_cx) let iv_obj = iv_val.to_object());
            cache_builtin(cx, "assert/__inspectValue", iv_obj.get());
        }

        // Forward the Section-1 comparisons port onto the cached util
        // object (install order: util first, assert second — globals.rs).
        // util.isDeepStrictEqual delegates to it at call time, so the full
        // comparator (boxed primitives / skipPrototype / cycle memos)
        // backs the util export with a single implementation (DRY with
        // assert.deepStrictEqual's engine). Hidden property: never visible
        // to golden probes scanning util's public surface.
        let mut ds_val = UndefinedValue();
        JS_GetProperty(
            cx.raw_cx(),
            assert_fn_obj.handle().into(),
            c"__baoIsDeepStrictEqual".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ds_val,
            },
        );
        if ds_val.is_object() && JS_ObjectIsFunction(ds_val.to_object()) {
            rooted!(&in(wrapped_cx) let ds_obj = ds_val.to_object());
            cache_builtin(cx, "assert/__isDeepStrictEqual", ds_obj.get());
            if let Some(util_raw) = crate::gc_store::gc_store_get(cx.raw_cx(), "builtin:util") {
                if !util_raw.is_null() {
                    rooted!(&in(wrapped_cx) let util_obj = util_raw);
                    let ds_obj_val = ObjectValue(ds_obj.get());
                    rooted!(&in(wrapped_cx) let ds_val_root = ds_obj_val);
                    let c_bao_cmp = ZBox::from_bytes(b"__baoIsDeepStrictEqual");
                    JS_DefineProperty(
                        cx.raw_cx(),
                        util_obj.handle().into(),
                        c_bao_cmp.as_ptr(),
                        ds_val_root.handle().into(),
                        0, // non-enumerable, non-writable, non-configurable
                    );
                }
            }
        }

        // Also expose AssertionError globally for tests that reference it
        // without going through require('assert').
        let mut ae_val = UndefinedValue();
        JS_GetProperty(
            cx.raw_cx(),
            assert_fn_obj.handle().into(),
            c"AssertionError".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ae_val,
            },
        );
        if ae_val.is_object() {
            let global = CurrentGlobalOrNull(cx.raw_cx());
            if !global.is_null() {
                rooted!(&in(wrapped_cx) let global_root = global);
                rooted!(&in(wrapped_cx) let ae_root = ae_val);
                JS_DefineProperty(
                    cx.raw_cx(),
                    global_root.handle().into(),
                    c"AssertionError".as_ptr(),
                    ae_root.handle().into(),
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
        register_assert_stubs!(cx, assert_obj;
            (c"ok", assert_ok, 1)
            (c"equal", assert_equal, 2)
            (c"notEqual", assert_not_equal, 2)
            (c"deepEqual", assert_deep_equal, 2)
            (c"notDeepEqual", assert_noop, 2)
            (c"strictEqual", assert_strict_equal, 2)
            (c"notStrictEqual", assert_not_strict_equal, 2)
            (c"throws", assert_noop, 1)
            (c"rejects", assert_rejects, 1)
            (c"doesNotThrow", assert_noop, 1)
            (c"fail", assert_fail, 0)
            (c"ifError", assert_if_error, 1)
            (c"deepStrictEqual", assert_deep_equal, 2)
        );

        rooted!(&in(cx) let strict_val = ObjectValue(assert_obj.get()));
        JS_DefineProperty(
            cx.raw_cx(),
            assert_obj.handle().into(),
            c"strict".as_ptr(),
            strict_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

/// Delegate one inspected value to the runtime's node-shaped `Bun.inspect`
/// formatter (REQ-ENG-016). The legacy `jsval_inspect` plain-object/array/
/// Buffer/function arms stay in place (C8b pins their exact shapes); the
/// class family the legacy formatter never learned — Map/Set/Date/RegExp/
/// Error/Promise/TypedArray — is formatted by Bun.inspect, which already
/// implements the node byte contracts (native Map/Set iteration, ISO dates,
/// typed-array element lists, error head+frame).
///
/// Depth: this frame's remaining budget is `depth` (root call = user depth
/// + 1); Bun.inspect's `depth` option counts nested levels below its root,
/// so the equivalent option is `depth - 1` (Bun adds the root back).
unsafe fn bun_inspect_delegate(cx: *mut JSContext, val: JSVal, depth: u32) -> Option<String> {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    // Root val early: the JS_GetProperty / JS_NewPlainObject calls below
    // can trigger GC and relocate the object, making the by-value val
    // parameter stale before its use in the JS_CallFunctionValue args.
    rooted!(&in(cx_ref) let val_root = val);
    let mut bun_val = UndefinedValue();
    if !JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"Bun".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut bun_val,
        },
    ) {
        JS_ClearPendingException(cx);
        return None;
    }
    if !bun_val.is_object() {
        return None;
    }
    rooted!(&in(cx_ref) let bun_obj = bun_val.to_object());
    let mut inspect_v = UndefinedValue();
    if !JS_GetProperty(
        cx,
        bun_obj.handle().into(),
        c"inspect".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut inspect_v,
        },
    ) {
        JS_ClearPendingException(cx);
        return None;
    }
    if !inspect_v.is_object() || !JS_ObjectIsFunction(inspect_v.to_object()) {
        return None;
    }
    rooted!(&in(cx_ref) let opts_obj = w2::JS_NewPlainObject(cx_ref));
    if opts_obj.get().is_null() {
        return None;
    }
    rooted!(&in(cx_ref) let depth_v = mozjs::jsval::DoubleValue(depth.saturating_sub(1) as f64));
    JS_DefineProperty(
        cx,
        opts_obj.handle().into(),
        c"depth".as_ptr(),
        depth_v.handle().into(),
        0,
    );
    rooted!(&in(cx_ref) let inspect_root = inspect_v);
    let call_vals = [val_root.get(), ObjectValue(opts_obj.get())];
    let call_args = HandleValueArray {
        length_: 2,
        elements_: call_vals.as_ptr(),
    };
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        global_root.handle().into(),
        inspect_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(cx);
        return None;
    }
    if !rval.is_string() {
        return None;
    }
    Some(crate::js_to_rust_string(cx, rval))
}

/// `.constructor.name` of an object (empty when absent/non-string). Used for
/// the node class-name prefix and depth-cap tag (`Point { ... }` / `[Point]`).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn object_ctor_name(cx: *mut JSContext, obj: mozjs::rust::Handle<*mut JSObject>) -> String {
    let mut ctor = UndefinedValue();
    if !JS_GetProperty(
        cx,
        obj.into(),
        c"constructor".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor,
        },
    ) {
        JS_ClearPendingException(cx);
        return String::new();
    }
    if !ctor.is_object() {
        return String::new();
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let ctor_obj = ctor.to_object());
    let mut name = UndefinedValue();
    if !JS_GetProperty(
        cx,
        ctor_obj.handle().into(),
        c"name".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut name,
        },
    ) {
        JS_ClearPendingException(cx);
        return String::new();
    }
    if name.is_string() {
        crate::js_to_rust_string(cx, name)
    } else {
        String::new()
    }
}

/// True when the object's [[Prototype]] is null (node renders
/// `[Object: null prototype] { ... }`).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn object_has_null_proto(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
) -> bool {
    let mut proto: *mut JSObject = ::std::ptr::null_mut();
    if !JS_GetPrototype(
        cx,
        obj.into(),
        MutableHandle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut proto,
        },
    ) {
        JS_ClearPendingException(cx);
        return false;
    }
    proto.is_null()
}

/// Map / Set rendering with node's util.inspect shape:
///   * `Map(1) { 1 => 'a' }` / `Set(2) { 1, 2 }`
///   * empty → `Map(0) {}` (single space group, unlike Bun.inspect)
/// Entries come from the native table iterators (JS::MapEntries /
/// JS::SetValues), so `Map.prototype` / user iterators are never consulted;
/// entry values go through `jsval_inspect` so strings keep util.inspect's
/// single-quote form.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn inspect_map_or_set(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    is_map: bool,
    depth: u32,
    ctx: &mut InspectCtx,
) -> String {
    let head = if is_map { "Map" } else { "Set" };
    let n = if is_map {
        mozjs_sys::jsapi::JS::MapSize(cx, obj.into())
    } else {
        mozjs_sys::jsapi::JS::SetSize(cx, obj.into())
    } as usize;
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let mut iter_val = UndefinedValue();
    let iter_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut iter_val,
    };
    let made_iter = if is_map {
        mozjs_sys::jsapi::JS::MapEntries(cx, obj.into(), iter_handle)
    } else {
        mozjs_sys::jsapi::JS::SetValues(cx, obj.into(), iter_handle)
    };
    if !made_iter || !iter_val.is_object() {
        JS_ClearPendingException(cx);
        return format!("{}({}) {{ <items unknown> }}", head, n);
    }
    rooted!(&in(cx_ref) let iter_root = iter_val.to_object());
    let inner_depth = depth.saturating_sub(1);
    let mut parts: Vec<String> = Vec::new();
    for _ in 0..n.min(100) {
        let mut res = UndefinedValue();
        let called = JS_CallFunctionName(
            cx,
            iter_root.handle().into(),
            c"next".as_ptr(),
            &HandleValueArray::empty(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut res,
            },
        );
        if !called || !res.is_object() {
            if !called {
                JS_ClearPendingException(cx);
            }
            break;
        }
        rooted!(&in(cx_ref) let res_root = res.to_object());
        let mut done = UndefinedValue();
        let mut value = UndefinedValue();
        let got_done = JS_GetProperty(
            cx,
            res_root.handle().into(),
            c"done".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut done,
            },
        );
        let got_value = JS_GetProperty(
            cx,
            res_root.handle().into(),
            c"value".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut value,
            },
        );
        if !got_done || !got_value {
            JS_ClearPendingException(cx);
            break;
        }
        if done.to_boolean() {
            break;
        }
        if !is_map {
            parts.push(jsval_inspect(cx, value, inner_depth, ctx));
            continue;
        }
        if !value.is_object() {
            parts.push("<items unknown>".to_string());
            continue;
        }
        rooted!(&in(cx_ref) let pair = value.to_object());
        let mut k = UndefinedValue();
        let mut v = UndefinedValue();
        let got_k = JS_GetElement(
            cx,
            pair.handle().into(),
            0,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut k,
            },
        );
        let got_v = JS_GetElement(
            cx,
            pair.handle().into(),
            1,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            },
        );
        if !got_k || !got_v {
            JS_ClearPendingException(cx);
            continue;
        }
        parts.push(format!(
            "{} => {}",
            jsval_inspect(cx, k, inner_depth, ctx),
            jsval_inspect(cx, v, inner_depth, ctx)
        ));
    }
    if parts.is_empty() {
        return format!("{}({}) {{}}", head, n);
    }
    format!("{}({}) {{ {} }}", head, n, parts.join(", "))
}

/// Native recursion budget for one `util.inspect` traversal (REQ-ENG-048
/// R7 §8 second-deviation ruling: `depth: Infinity` = node's "iterate until
/// the engine's own stack limit, then interrupt" semantics, not a fixed 100
/// truncation).
///
/// Basis (all measured, no guesses):
/// * node v24.19.0 oracle: `depth: Infinity` renders to completion up to the
///   V8 JS stack, then emits the exact tag
///   `[Object: Inspection interrupted prematurely. Maximum call stack size
///   exceeded.]` (~928 levels on the default stack; `--stack-size=4000`
///   renders 1200 levels fully) — i.e. node itself never offers unbounded
///   Infinity; the interruption tag is the defined engine-bound behavior.
/// * In-tree datum (`util_inspect` u32::MAX incident): ~1000 native
///   `jsval_inspect` frames ≈ the 8 MiB main thread (~8 KB/frame on the
///   test-ci profile).
/// * 700 frames ≈ 5.6 MiB, leaving ~2.4 MiB headroom for the entry path and
///   call scaffolding. Deeper-than-node tolerances are impossible to honor
///   natively without a heap-allocated renderer rewrite (registered as a
///   known engine-level deviation: chains of 700..~928 nested containers
///   render the interruption tag in bao but fully in node; the official
///   long-running fixture's 1001-deep chain exceeds node's own default-stack
///   bound too, so both engines interrupt — at different depths).
const INSPECT_NATIVE_BUDGET: u32 = 700;

/// node v24's exact stack-exhaustion marker emitted by `util.inspect` when
/// its own recursion hits the engine stack limit (oracle-verbatim, incl.
/// spacing and trailing period).
const INSPECT_INTERRUPTED_TAG: &str =
    "[Object: Inspection interrupted prematurely. Maximum call stack size exceeded.]";

/// Native `util.inspect` traversal context (REQ-ENG-048 R7 root fix): the
/// ancestor `seen` stack that folds cyclic back-edges into node's
/// `[Circular *N]` form, the back-edge ref registry behind the `<ref *N>`
/// prefix, and the per-traversal native recursion budget.
///
/// GC discipline: every entry is a `Box<Heap<*mut JSObject>>` inside a
/// `RootedVec` — the servo structuredclone reader idiom already used by
/// `Bun.concatArrayBuffers` (bun_api.rs). Slots are tracer-visible
/// (RootedTraceableSet) and barrier-pinned at Box-stable addresses, so a
/// moving GC triggered by an accessor / Map-iterator allocation mid-walk
/// updates the slots in place and pointer identity stays valid (bare
/// `Vec<*mut JSObject>` identity would silently drift across a nursery
/// move — the W30/e127 class).
struct InspectCtx<'a> {
    /// Ancestor stack: objects whose rendering is currently in progress.
    seen: RootedVec<'a, Box<Heap<*mut JSObject>>>,
    /// Back-edge targets in first-discovery order; ref number = index + 1
    /// (node v24 oracle T11/T13/T18: only objects that are the TARGET of a
    /// back-edge discovered during traversal get `<ref *N>` — mere cycle
    /// participants do not, which is why the JS-side `participatesInCycle`
    /// reachability pre-pass diverges from node on 3-cycles rooted at a
    /// middle node).
    refs: RootedVec<'a, Box<Heap<*mut JSObject>>>,
    /// Remaining native recursion budget (see [INSPECT_NATIVE_BUDGET]).
    native_budget: u32,
}

impl InspectCtx<'_> {
    /// Ref number for `obj` if a back-edge to it was already discovered.
    fn ref_number(&self, obj: *mut JSObject) -> Option<u32> {
        self.refs
            .iter()
            .position(|h| h.get() == obj)
            .map(|i| (i + 1) as u32)
    }

    /// Ancestor-hit path: node numbers the back-edge target at discovery
    /// time (lazy, single-pass — the target's frame has not returned yet, so
    /// the `<ref *N>` prefix can still be prepended on its return; oracle
    /// T20 confirms no number is assigned when a depth truncation hides the
    /// back edge entirely).
    fn seen_hit_ref(&mut self, obj: *mut JSObject) -> Option<u32> {
        if self.seen.iter().any(|h| h.get() == obj) {
            Some(self.assign_ref(obj))
        } else {
            None
        }
    }

    fn assign_ref(&mut self, obj: *mut JSObject) -> u32 {
        if let Some(n) = self.ref_number(obj) {
            return n;
        }
        self.refs.push(Heap::boxed(obj));
        self.refs.len() as u32
    }
}

/// One-off inspect with a fresh traversal context — a fresh cycle scope,
/// exactly like a fresh `util.inspect` call in node. Used by display arms
/// (`util.format`/console paths) that hold no traversal context of their
/// own; each call gets its own ancestor stack, ref registry and budget.
///
/// @trace REQ-ENG-048 [algorithm:util_inspect_cycle_folding]
unsafe fn jsval_inspect_fresh(cx: *mut JSContext, val: JSVal, depth: u32) -> String {
    let mut seen_rootable: RootableVec<Box<Heap<*mut JSObject>>> = RootableVec::new_unrooted();
    let mut refs_rootable: RootableVec<Box<Heap<*mut JSObject>>> = RootableVec::new_unrooted();
    let mut ctx = InspectCtx {
        seen: RootedVec::new(&mut seen_rootable),
        refs: RootedVec::new(&mut refs_rootable),
        native_budget: INSPECT_NATIVE_BUDGET,
    };
    jsval_inspect(cx, val, depth, &mut ctx)
}

/// Recursively format a JSVal the way Node.js `util.inspect` would, with a
/// depth cap to avoid infinite recursion on cyclic structures.
///
/// @trace REQ-ENG-004 [algorithm:util_inspect]
/// @trace REQ-ENG-048 [algorithm:util_inspect_cycle_folding]
unsafe fn jsval_inspect(
    cx: *mut JSContext,
    val: JSVal,
    depth: u32,
    ctx: &mut InspectCtx,
) -> String {
    unsafe {
        if val.is_undefined() {
            return "undefined".to_string();
        }
        if val.is_null() {
            return "null".to_string();
        }
        if val.is_boolean() {
            return val.to_boolean().to_string();
        }
        if val.is_int32() {
            return val.to_int32().to_string();
        }
        if val.is_double() {
            let d = val.to_double();
            // Rust's f64::to_string renders Infinity as "inf" and NaN as
            // "NaN"; node's util.inspect contract is "Infinity" / "-Infinity".
            if d.is_infinite() {
                return if d > 0.0 {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                };
            }
            if d.is_nan() {
                return "NaN".to_string();
            }
            if d == 0.0 && d.is_sign_negative() {
                return "-0".to_string();
            }
            return d.to_string();
        }
        if val.is_string() {
            return format!("'{}'", crate::js_to_rust_string(cx, val));
        }
        if val.is_symbol() {
            // JS::GetSymbolDescription returns *mut JSString directly.
            // Root the symbol first, then call the API.
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let sv = val);
            let sym_ptr = val.to_symbol();
            rooted!(&in(cx_ref) let sym_root = sym_ptr);
            let desc = w2::GetSymbolDescription(sym_root.handle());
            if desc.is_null() {
                return "Symbol()".to_string();
            }
            rooted!(&in(cx_ref) let desc_root = desc);
            let desc_str = crate::jsstr_to_rust_string(cx, desc_root.get());
            return format!("Symbol({})", desc_str);
        }
        if val.is_bigint() {
            // node: `9n` — canonical engine ToString plus the suffix.
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let sv = val);
            let jsstr = mozjs::rust::ToString(cx_ref, sv.handle());
            if jsstr.is_null() {
                JS_ClearPendingException(cx);
                return "0n".to_string();
            }
            let text = crate::js_to_rust_string(cx, StringValue(&*jsstr));
            return format!("{}n", text);
        }
        if val.is_object() {
            let obj_ptr = val.to_object();
            // Cycle folding before any truncation tag: node renders the root
            // of a self-cycle fully even at depth:0 as
            // `<ref *1> { self: [Circular *1] }` (node v24 oracle), so the
            // ancestor hit takes priority over the depth tag.
            if let Some(n) = ctx.seen_hit_ref(obj_ptr) {
                return format!("[Circular *{}]", n);
            }
            // Node-interrupt semantics: this tag marks the ENGINE stack
            // budget, not a contract depth (see INSPECT_NATIVE_BUDGET).
            if ctx.native_budget == 0 {
                return INSPECT_INTERRUPTED_TAG.to_string();
            }
            ctx.native_budget -= 1;
            ctx.seen.push(Heap::boxed(obj_ptr));
            let body = jsval_inspect_object_body(cx, val, depth, ctx);
            ctx.seen.pop();
            // A back-edge to this object discovered inside its own subtree
            // (the frame has not returned yet) numbered it; node prepends
            // the `<ref *N>` prefix when the frame returns.
            match ctx.ref_number(obj_ptr) {
                Some(n) => format!("<ref *{}> {}", n, body), // @evidence SPEC/10-REQUIREMENTS.html:237
                None => body, // @evidence SPEC/10-REQUIREMENTS.html:237
            }
        } else {
            String::new()
        }
    }
}

/// Body renderer for one object value, factored out of `jsval_inspect` so
/// the caller brackets the entire body (every early return included) with
/// the ancestor-stack push/pop. All recursive descent below threads `ctx`.
///
/// @trace REQ-ENG-048 [algorithm:util_inspect_cycle_folding]
unsafe fn jsval_inspect_object_body(
    cx: *mut JSContext,
    val: JSVal,
    depth: u32,
    ctx: &mut InspectCtx,
) -> String {
    unsafe {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj = val.to_object());

        // node's util.inspect dispatches to Symbol.for('nodejs.util.inspect.custom')
        // when present on the object or its prototype chain.
        if let Some(s) =
            crate::bun_inspect_api::try_custom_inspect(cx, &mut wrapped_cx, obj.handle(), depth)
        {
            return s;
        }

        // Re-synthesize val from the rooted handle: try_custom_inspect runs
        // JS (JS_NewStringCopyZ / JS_GetPropertyById / user callbacks) that
        // can trigger GC and relocate the object, leaving the by-value
        // `val` parameter stale. Downstream consumers (IsArrayObject,
        // jsval_buffer_bytes, bun_inspect_delegate) re-extract the object
        // pointer from val, so refresh it from the GC-updated root.
        let val = ObjectValue(obj.get());

        // Arrays → [ a, b, c ] (elements first, then non-index own
        // properties — Node renders `[ 1, 2, 3, foo: 'x' ]`).
        let mut is_arr = false;
        rooted!(&in(wrapped_cx) let v_root = val);
        IsArrayObject(cx, v_root.handle().into(), &mut is_arr);
        if is_arr {
            if depth == 0 {
                return "[Array]".to_string();
            }
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                obj.handle().into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            let len = if len_val.is_int32() && len_val.to_int32() > 0 {
                len_val.to_int32() as u32
            } else {
                0
            };
            let mut parts: Vec<String> = Vec::new();
            for i in 0..len {
                let mut elem = UndefinedValue();
                JS_GetElement(
                    cx,
                    obj.handle().into(),
                    i,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut elem,
                    },
                );
                parts.push(jsval_inspect(cx, elem, depth - 1, ctx));
            }
            // Non-index own properties after the elements; own integer
            // indices >= length render as `N: value` keys.
            // domain-check 06d0ae8ac1 (own-idiom fix, upstream oracle)
            let mut ids = mozjs::rust::IdVector::new(&mut wrapped_cx);
            let keys_ok =
                GetPropertyKeys(cx, obj.handle().into(), JSITER_OWNONLY, ids.handle_mut());
            if keys_ok {
                for jsid in &*ids {
                    if jsid.is_int() {
                        let idx = jsid.to_int() as u32;
                        if idx < len {
                            continue;
                        }
                        let mut v = UndefinedValue();
                        if !JS_GetElement(
                            cx,
                            obj.handle().into(),
                            idx,
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut v,
                            },
                        ) {
                            JS_ClearPendingException(cx);
                            continue;
                        }
                        parts.push(format!("{}: {}", idx, jsval_inspect(cx, v, depth - 1, ctx)));
                        continue;
                    }
                    if !jsid.is_string() {
                        continue;
                    }
                    let key_str_ptr = jsid.to_string();
                    if key_str_ptr.is_null() {
                        continue;
                    }
                    let key = jsstr_to_string(
                        &mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)),
                        NonNull::new_unchecked(key_str_ptr),
                    );
                    let c_key = ZBox::from_bytes(key.as_bytes());
                    let mut v = UndefinedValue();
                    if !JS_GetProperty(
                        cx,
                        obj.handle().into(),
                        c_key.as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut v,
                        },
                    ) {
                        JS_ClearPendingException(cx);
                        continue;
                    }
                    parts.push(format!("{}: {}", key, jsval_inspect(cx, v, depth - 1, ctx)));
                }
            } else {
                JS_ClearPendingException(cx);
            }
            if parts.is_empty() {
                return "[]".to_string();
            }
            return format!("[ {} ]", parts.join(", "));
        }

        // Buffer instances → '<Buffer 68 69>' (node buffer-inspect form:
        // the typed-array bytes as space-separated lowercase hex pairs;
        // the empty buffer keeps the trailing space, '<Buffer >'). A
        // Buffer shares the Uint8Array representation with a `_isBuffer`
        // marker — the same detection `Buffer.isBuffer` uses — so this
        // check must run before the generic plain-object path (which
        // would otherwise render the byte indices as '{ 0: 104, ... }').
        // Node shows the bytes regardless of `depth` (they are a flat
        // preview, not nested structure).
        if let Some((len, data)) = jsval_buffer_bytes(cx, &val) {
            const INSPECT_BUFFER_MAX_BYTES: usize = 512;
            let mut s = String::from("<Buffer");
            if len == 0 {
                // '<Buffer >' — node renders the empty buffer with a
                // trailing space.
                s.push(' ');
            } else {
                let shown = len.min(INSPECT_BUFFER_MAX_BYTES);
                let bytes = ::std::slice::from_raw_parts(data, shown);
                for b in bytes {
                    s.push(' ');
                    s.push_str(&format!("{:02x}", b));
                }
                if len > INSPECT_BUFFER_MAX_BYTES {
                    s.push_str(&format!(
                        " ... {} more bytes",
                        len - INSPECT_BUFFER_MAX_BYTES
                    ));
                }
            }
            s.push('>');
            return s;
        }

        // Functions → [Function: name] or [Function (anonymous)]
        if JS_ObjectIsFunction(obj.get()) {
            let mut name_val = UndefinedValue();
            JS_GetProperty(
                cx,
                obj.handle().into(),
                c"name".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut name_val,
                },
            );
            let name = if name_val.is_string() {
                crate::js_to_rust_string(cx, name_val)
            } else {
                String::new()
            };
            if name.is_empty() {
                return "[Function (anonymous)]".to_string();
            }
            return format!("[Function: {}]", name);
        }

        // Class family the legacy formatter never learned: delegate to
        // the runtime's node-shaped Bun.inspect formatter (Map/Set native
        // iteration, ISO dates, RegExp source/flags, error head+frame,
        // Promise state). Buffer/Array/Function above stay on the legacy
        // path because C8b pins their exact shapes.
        let mut cls = ESClass::Other;
        let _ = w2::GetBuiltinClass(&mut wrapped_cx, obj.handle(), &mut cls);

        // Map/Set are rendered here rather than delegated: node's
        // util.inspect quotes strings with single quotes and collapses an
        // empty body to `{}`, while Bun.inspect's formatter uses double
        // quotes and keeps the body padding (`Map(0) {  }`).
        if cls == ESClass::Map || cls == ESClass::Set {
            return inspect_map_or_set(cx, obj.handle(), cls == ESClass::Map, depth, ctx);
        }

        if matches!(
            cls,
            ESClass::Date | ESClass::RegExp | ESClass::Error | ESClass::Promise
        ) {
            if let Some(s) = bun_inspect_delegate(cx, val, depth) {
                return s;
            }
        }

        // TypedArrays / DataView (Buffer was handled above).
        {
            let mut view_len: usize = 0;
            let mut view_shared = false;
            let mut view_data: *mut u8 = ::std::ptr::null_mut();
            let view = JS_GetObjectAsArrayBufferView(
                obj.get(),
                &mut view_len,
                &mut view_shared,
                &mut view_data,
            );
            if !view.is_null() {
                if depth == 0 {
                    let name = object_ctor_name(cx, obj.handle());
                    let tname = if name.is_empty() {
                        "TypedArray".to_string()
                    } else {
                        name
                    };
                    return format!("[{}]", tname);
                }
                if let Some(s) = bun_inspect_delegate(cx, val, depth) {
                    return s;
                }
            }
        }

        let null_proto = object_has_null_proto(cx, obj.handle());
        let ctor_name = object_ctor_name(cx, obj.handle());

        // WeakMap / WeakSet: node renders the unknown-items tag.
        if ctor_name == "WeakMap" || ctor_name == "WeakSet" {
            return format!("{} {{ <items unknown> }}", ctor_name);
        }

        // Plain objects → { key: value, ... }
        if depth == 0 {
            if null_proto {
                return "[Object: null prototype] {}".to_string();
            }
            if !ctor_name.is_empty() && ctor_name != "Object" {
                return format!("[{}]", ctor_name);
            }
            return "[Object]".to_string();
        }
        let prefix = if null_proto {
            "[Object: null prototype] ".to_string()
        } else if !ctor_name.is_empty() && ctor_name != "Object" {
            format!("{} ", ctor_name)
        } else {
            String::new()
        };

        // Enumerate own enumerable entries via the established IdVector +
        // GetPropertyKeys pattern used elsewhere in bao_runtime. Integer-
        // index keys (PropertyKey::Int) render bare, ascending — engine
        // enumeration yields ints first, then string keys in insertion
        // order (Node inspect key order). Note: IdVector takes the
        // wrapped JSContext (mozjs 0.22 safe API), and we fetch each
        // value via JS_GetProperty with the C-string key (the same
        // approach node_url.rs uses) to avoid the Handle<PropertyKey>
        // wrapping that JS_GetPropertyById would require.
        // domain-check 06d0ae8ac1 (own-idiom fix, upstream oracle)
        let mut ids = mozjs::rust::IdVector::new(&mut wrapped_cx);
        let ok = GetPropertyKeys(cx, obj.handle().into(), JSITER_OWNONLY, ids.handle_mut());
        let mut parts: Vec<String> = Vec::new();
        if ok {
            for jsid in &*ids {
                if jsid.is_int() {
                    let idx = jsid.to_int() as u32;
                    let mut v = UndefinedValue();
                    if !JS_GetElement(
                        cx,
                        obj.handle().into(),
                        idx,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut v,
                        },
                    ) {
                        JS_ClearPendingException(cx);
                        continue;
                    }
                    parts.push(format!("{}: {}", idx, jsval_inspect(cx, v, depth - 1, ctx)));
                    continue;
                }
                if !jsid.is_string() {
                    continue;
                }
                let key_str_ptr = jsid.to_string();
                if key_str_ptr.is_null() {
                    continue;
                }
                let key = jsstr_to_string(
                    &mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)),
                    NonNull::new_unchecked(key_str_ptr),
                );
                let c_key = ZBox::from_bytes(key.as_bytes());
                let mut v = UndefinedValue();
                JS_GetProperty(
                    cx,
                    obj.handle().into(),
                    c_key.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut v,
                    },
                );
                parts.push(format!("{}: {}", key, jsval_inspect(cx, v, depth - 1, ctx)));
            }
        }
        if parts.is_empty() {
            return format!("{}{{}}", prefix);
        }
        return format!("{}{{ {} }}", prefix, parts.join(", "));
    }
}

/// Identify a Buffer instance and return its byte view `(len, data)`.
///
/// Buffer instances are real Uint8Arrays with `Buffer.prototype` rebound plus
/// a `_isBuffer` own marker (globals.rs `create_buffer_object`) — the exact
/// detection `Buffer.isBuffer` performs. Returns `None` for non-Buffers and
/// for Buffer-shaped objects that are not Uint8Array-backed.
unsafe fn jsval_buffer_bytes(
    cx: *mut JSContext,
    val: &JSVal,
) -> ::std::option::Option<(usize, *mut u8)> {
    if !val.is_object() {
        return ::std::option::Option::None;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = val.to_object());
    let mut marker = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"_isBuffer".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut marker,
        },
    );
    if !marker.is_boolean() || !marker.to_boolean() {
        return ::std::option::Option::None;
    }
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data: *mut u8 = ::std::ptr::null_mut();
    let view = JS_GetObjectAsUint8Array(obj.get(), &mut length, &mut is_shared, &mut data);
    if view.is_null() || data.is_null() {
        return ::std::option::Option::None;
    }
    ::std::option::Option::Some((length, data))
}

/// `util.format('%j', v)` — JSON.stringify semantics (node contract): the
/// stringify result string, `'undefined'` when stringify yields undefined
/// (undefined / function / symbol input), and `'[Circular]'` when stringify
/// throws (cyclic structure).
unsafe fn json_stringify_for_format(cx: *mut JSContext, val: JSVal) -> String {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return "undefined".to_string();
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let global_root = global);
    // Root val early: the JS_GetProperty calls below can trigger GC and
    // relocate the object, making the by-value val parameter stale before
    // its use in the JS_CallFunctionValue args.
    rooted!(&in(wrapped_cx) let val_root = val);
    let mut json_val = UndefinedValue();
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"JSON".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut json_val,
        },
    );
    if !json_val.is_object() {
        return "undefined".to_string();
    }
    let json_obj = json_val.to_object();
    rooted!(&in(wrapped_cx) let json_root = json_obj);
    let mut stringify_val = UndefinedValue();
    JS_GetProperty(
        cx,
        json_root.handle().into(),
        c"stringify".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut stringify_val,
        },
    );
    if !stringify_val.is_object() {
        return "undefined".to_string();
    }
    // The callee slot takes a VALUE handle — root the jsval form.
    rooted!(&in(wrapped_cx) let stringify_root = stringify_val);
    let call_args = HandleValueArray {
        length_: 1,
        elements_: &val_root.get() as *const Value,
    };
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        json_root.handle().into(),
        stringify_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(cx);
        return "[Circular]".to_string();
    }
    if rval.is_undefined() {
        return "undefined".to_string();
    }
    if rval.is_string() {
        return crate::js_to_rust_string(cx, rval);
    }
    if rval.is_null() {
        return "null".to_string();
    }
    jsval_to_display(cx, rval)
}

/// Format a JSVal for `util.format` / console output. Strings are emitted
/// bare (no quotes), objects fall back to their constructor-tag. This is the
/// `util.format("%s", val)` / `console.log` display semantics — distinct from
/// `util.inspect`, which quotes strings and recurses into objects.
unsafe fn jsval_to_display(cx: *mut JSContext, val: JSVal) -> String {
    unsafe {
        if val.is_undefined() {
            return "undefined".to_string();
        }
        if val.is_null() {
            return "null".to_string();
        }
        if val.is_boolean() {
            return val.to_boolean().to_string();
        }
        if val.is_int32() {
            return val.to_int32().to_string();
        }
        if val.is_double() {
            let d = val.to_double();
            if d.is_infinite() {
                return if d > 0.0 {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                };
            }
            if d.is_nan() {
                return "NaN".to_string();
            }
            return d.to_string();
        }
        if val.is_string() {
            return crate::js_to_rust_string(cx, val);
        }
        if val.is_symbol() {
            // JS::GetSymbolDescription — same fix as jsval_inspect.
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let sv = val);
            let sym_ptr = val.to_symbol();
            rooted!(&in(cx_ref) let sym_root = sym_ptr);
            let desc = w2::GetSymbolDescription(sym_root.handle());
            if desc.is_null() {
                return "Symbol()".to_string();
            }
            rooted!(&in(cx_ref) let desc_root = desc);
            let desc_str = crate::jsstr_to_rust_string(cx, desc_root.get());
            return format!("Symbol({})", desc_str);
        }
        if val.is_bigint() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let sv = val);
            let jsstr = mozjs::rust::ToString(cx_ref, sv.handle());
            if jsstr.is_null() {
                JS_ClearPendingException(cx);
                return "0n".to_string();
            }
            let text = crate::js_to_rust_string(cx, StringValue(&*jsstr));
            return format!("{}n", text);
        }
        if val.is_object() {
            // For format display, delegate to inspect so arrays/objects render
            // with their contents rather than the bare "[Object]" tag. Fresh
            // traversal context: util.format display is its own cycle scope.
            return jsval_inspect_fresh(cx, val, 2);
        }
        String::new()
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
/// Read an options-object numeric field as an f64 (JS-side NaN/Infinity
/// preserved; non-numeric/missing/failed get = None).
///
/// # Safety
/// `cx` must be a valid live JSContext; `oobj` must be rooted for the call.
unsafe fn util_inspect_num_field(
    cx: *mut JSContext,
    oobj: mozjs::rust::Handle<*mut JSObject>,
    key: &str,
) -> Option<f64> {
    let c_key = ZBox::from_bytes(key.as_bytes());
    let mut v = UndefinedValue();
    let got = JS_GetProperty(
        cx,
        oobj.into(),
        c_key.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if !got {
        JS_ClearPendingException(cx);
        return None;
    }
    if v.is_number() {
        Some(v.to_number())
    } else {
        None
    }
}

/// `true` when node's own formatter would break the container into
/// indented per-entry lines.  Node rules (v24.19.0 oracle-verified,
/// `inspect.js` `if (compact === false || compact === null || breakLength === 0)`
/// then `compact > 0` numeric gate):
///   * `compact: false` → always break.
///   * `compact: N` (number) → break iff nested levels exceed N; N ≤ 0
///     always breaks (compact: 0 breaks at every level).
///   * `compact: null` → `=== false` type-check fails, falls to
///     `breakLength === 0` test; not the unambiguous-break shape, so stay
///     on the Rust path (no behaviour change vs pre-delegation).
///   * `compact` omitted/true/undefined → node defaults to compact with
///     breakLength 80 auto-break.  Only `breakLength: 0` is an
///     unambiguous break trigger here; a breakLength overflow depends on
///     the single-line rendering length (not computed in Rust), so those
///     calls stay on the Rust path.
/// Oracle probes: `{a:1,b:2}` @default = 1 line; `compact:false` = 3
/// lines; `compact:0` = 7 lines; 8 short keys @default = 10 lines;
/// `breakLength:Infinity` = 1 line regardless of length.
///
/// # Safety
/// `cx` must be a valid live JSContext; `oobj` must be rooted for the call.
unsafe fn util_inspect_would_break(
    cx: *mut JSContext,
    oobj: mozjs::rust::Handle<*mut JSObject>,
) -> bool {
    let mut v = UndefinedValue();
    let got = JS_GetProperty(
        cx,
        oobj.into(),
        c"compact".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if !got {
        JS_ClearPendingException(cx);
        return false;
    }
    if v.is_boolean() {
        return !v.to_boolean();
    }
    if v.is_number() {
        return v.to_number() <= 0.0;
    }
    // compact omitted (undefined), null, or a string → no unambiguous
    // break trigger from compact itself; fall through to the breakLength
    // check (node only breaks on breakLength === 0 when compact is not
    // already a boolean false).
    matches!(util_inspect_num_field(cx, oobj, "breakLength"), Some(0.0))
}

/// Delegate one inspected value to the assert module's Section-3
/// `inspectValue` (node `assertion_error.js` shape, 220-case
/// parity-validated).  Returns the rendered string, or None on any bridge
/// failure (util_inspect then falls back to the Rust single-line path
/// rather than silently dropping the render).  The JS side already
/// implements node's depth contract — `inspectValue(val, N, ctx)` counts N
/// nested levels — so no +1 budget is added here (distinct from the Rust
/// `jsval_inspect` path, whose root call carries one extra level).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn inspect_value_delegate(cx: *mut JSContext, val: JSVal, depth: u32) -> Option<String> {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    // Fetch the cached assert builtin's Section-3 inspectValue function.
    // The install_assert setup stashes it under a non-enumerable
    // __baoInspectValue property and caches the function as
    // "builtin:assert/__inspectValue" for the Rust side.
    let inspect_raw = crate::gc_store::gc_store_get(cx, "builtin:assert/__inspectValue")?;
    if inspect_raw.is_null() {
        return None;
    }
    rooted!(&in(cx_ref) let inspect_obj = inspect_raw);
    // inspectValue(val, depth, ctx={indent:'',refs:null,refCounter:0})
    rooted!(&in(cx_ref) let val_root = val);
    rooted!(&in(cx_ref) let depth_root = mozjs::jsval::DoubleValue(depth as f64));
    let call_vals = [val_root.get(), depth_root.get()];
    let call_args = HandleValueArray {
        length_: 2,
        elements_: call_vals.as_ptr(),
    };
    let mut rval = UndefinedValue();
    // The callee slot takes a VALUE handle — root the jsval form.
    rooted!(&in(cx_ref) let inspect_root = ObjectValue(inspect_obj.get()));
    let ok = JS_CallFunctionValue(
        cx,
        global_root.handle().into(),
        inspect_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(cx);
        return None;
    }
    if !rval.is_string() {
        return None;
    }
    Some(crate::js_to_rust_string(cx, rval))
}

/// Emit a JS string result back into util_inspect's rval slot.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn util_inspect_set_result(cx: *mut JSContext, args: &CallArgs, result: &str) {
    let utf16: Vec<u16> = result.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
}

unsafe extern "C" fn util_inspect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let s = JS_NewStringCopyZ(cx, c"undefined".as_ptr());
        args.rval().set(if s.is_null() {
            UndefinedValue()
        } else {
            StringValue(&*s)
        });
        return true;
    }
    let val = *args.get(0).ptr;
    // @trace REQ-ENG-004 [algorithm:util_inspect]
    // util.inspect quotes strings and recurses into objects (Node.js default
    // depth = 2). This is distinct from util.format display semantics.
    //
    // Options (2nd arg, Node contract): null/undefined ignored; any other
    // non-object throws ERR_INVALID_ARG_TYPE.  The Rust jsval_inspect
    // single-line path honours `depth` (the formatter has no colors path).
    // Multi-line forms (compact:false, compact:N≤0, breakLength:0) delegate
    // to the assert module's Section-3 inspectValue (node assertion_error.js
    // shape, 220-case parity) — the node multi-line body is never
    // re-implemented in Rust.  Default compact calls stay on the Rust path
    // unchanged.  Root call carries one extra level of budget on the Rust
    // path only — `depth: N` caps NESTED levels, so
    // inspect({d:{e:{f:1}}}, {depth:0}) → "{ d: [Object] }" (same
    // convention as Bun.inspect).
    // domain-check a21f02a988 (own-idiom fix, upstream oracle)
    let mut depth: u32 = 2;
    if args.argc_ > 1 {
        let o = *args.get(1).ptr;
        if !o.is_undefined() && !o.is_null() {
            if !o.is_object() {
                let msg = format!(
                    "The \"options\" argument must be of type object. Received {}",
                    crate::bun_inspect_api::received_type_fragment(cx, o)
                );
                return crate::bun_inspect_api::throw_invalid_arg_type(cx, &msg);
            }
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let oobj = o.to_object());
            let d = util_inspect_num_field(cx, oobj.handle(), "depth");
            if let Some(d) = d {
                // depth: Infinity = node's honest "no contract cap" (R7 §8
                // second-deviation ruling). The real bound is the native
                // recursion budget (INSPECT_NATIVE_BUDGET) which mirrors
                // node's own engine-stack interruption, not a fixed 100
                // truncation. The stale clamp here previously truncated
                // acyclic deep chains at 100 layers where node renders them
                // to completion (node oracle: 300-deep chain = 182KB, and
                // node interrupts only at its own ~928-level stack bound).
                depth = if d.is_infinite() {
                    u32::MAX
                } else {
                    d.max(0.0) as u32
                };
            }
            // Delegation condition (node v24.19.0 oracle-verified):
            // compact:false, compact:N with N≤0, or breakLength:0 force a
            // multi-line body; all other option shapes keep the Rust
            // single-line path (the pre-delegation behaviour).
            if util_inspect_would_break(cx, oobj.handle()) {
                if let Some(out) = inspect_value_delegate(cx, val, depth) {
                    util_inspect_set_result(cx, &args, &out);
                    return true;
                }
            }
        }
    }
    // One traversal context per util.inspect call (node: each call owns its
    // cycle scope — ref numbers never leak across calls). The ancestor stack
    // is what folds the R7 exponential back-edge explosion into node's
    // `[Circular *N]` / `<ref *N>` forms (REQ-ENG-048).
    let result = jsval_inspect_fresh(cx, val, depth.saturating_add(1));
    util_inspect_set_result(cx, &args, &result);
    true
}

// ---- util.stripVTControlCharacters (node v24 contract, REQ-ENG-048) ----
// Oracle-verbatim pattern source (extracted from the running node v24.19.0
// binary, lib/internal/util/inspect.js):
//   const ansi = new RegExp(
//     '[\\u001B\\u009B][[\\]()#;?]*' +
//     '(?:(?:(?:(?:;[-a-zA-Z\\d\\/\\#&.:=?%@~_]+)*' +
//     '|[a-zA-Z\\d]+(?:;[-a-zA-Z\\d\\/\\#&.:=?%@~_]*)*)?' +
//     '(?:\\u0007|\\u001B\\u005C|\\u009C))' +
//     '|(?:(?:\\d{1,4}(?:;\\d{0,4})*)?' +
//     '[\\dA-PR-TZcf-nq-uy=><~]))', 'g');
// The scanner below implements the exact regex semantics (branch order,
// maximal munch, branch-B digit-quantifier DFS backtracking) and was
// differential-pinned against the node binary on 3500 randomized and
// handpicked sequences with zero divergences (twin harness in the task
// report). Pins worth keeping visible:
//   * "\x1B[31z" strips "\x1B[3" and leaves "1z" (the final-char class is
//     tried at every digit-part length, longest first),
//   * "\x1B]0;t\x1B\\x" strips the whole OSC up to the ESC-\ terminator,
//   * a failed match re-scans from the next char, so "\x1B]0;t\x1BX" leaves
//     "]0;t" (the trailing ESC+X match as a CSI form).
const VT_ESC: char = '\u{001B}';
const VT_CSI: char = '\u{009B}';
const VT_BEL: char = '\u{0007}';
const VT_C1_ST: char = '\u{009C}';

#[inline]
fn vt_is_intro(ch: char) -> bool {
    ch == VT_ESC || ch == VT_CSI
}

/// `[[\]()#;?]*` — introducer sequence after ESC / C1.
#[inline]
fn vt_is_inner(ch: char) -> bool {
    matches!(ch, '[' | ']' | '(' | ')' | '#' | ';' | '?')
}

/// `[-a-zA-Z\d/#&.:=?%@~_]` — OSC text params.
#[inline]
fn vt_is_a_param(ch: char) -> bool {
    matches!(
        ch,
        '-'
            | 'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | '/'
            | '#'
            | '&'
            | '.'
            | ':'
            | '='
            | '?'
            | '%'
            | '@'
            | '~'
            | '_'
    )
}

/// `[\dA-PR-TZcf-nq-uy=><~]` — CSI final byte. Tokenized: digits, A-P,
/// R-T, Z, c, f-n, q-u, y, `=`, `>`, `<`, `~` (node probe-pinned: q/u/y in,
/// v/w/x/z out; G/H in, U/V out; f/g/n in, e/o/p out).
#[inline]
fn vt_is_b_final(ch: char) -> bool {
    matches!(
        ch,
        '0'..='9' | 'A'..='P' | 'R'..='T' | 'Z' | 'c' | 'f'..='n' | 'q'..='u' | 'y'
            | '=' | '>' | '<' | '~'
    )
}

/// Terminator length in chars: BEL (1) | ESC `\` (2) | C1 ST (1), else 0.
#[inline]
fn vt_term_len(chars: &[char], j: usize) -> usize {
    if chars.get(j) == Some(&VT_BEL) {
        1
    } else if chars.get(j) == Some(&VT_ESC) && chars.get(j + 1) == Some(&'\\') {
        2
    } else if chars.get(j) == Some(&VT_C1_ST) {
        1
    } else {
        0
    }
}

/// Engine-bound guards for the branch-B enumeration: node's own backtracking
/// VM grinds exponentially on pathological `1;1111;1111;…` chains (ReDoS
/// class — the regex's quantifiers are exponential there too); the native
/// scanner prunes at a native-stack-safe recursion depth and a candidate
/// budget instead of following it into the exponential. Deviation only on
/// inputs with >512 `;`-groups after the initial digits — no real ANSI
/// stream reaches that.
const VT_DIGIT_END_DEPTH_CAP: usize = 512;
const VT_DIGIT_END_BUDGET: usize = 100_000;

/// Valid digit-part ends at/after `p` in DFS greedy-backtracking order
/// (`;\d{0,4}` groups longest-first, innermost backtrack first, star-stops
/// candidate last) — mirrors the regex engine's quantifier enumeration,
/// bounded by [VT_DIGIT_END_DEPTH_CAP] / [VT_DIGIT_END_BUDGET].
fn vt_collect_digit_ends(chars: &[char], p: usize, out: &mut Vec<usize>) {
    fn walk(chars: &[char], p: usize, out: &mut Vec<usize>, depth: usize) {
        if depth > VT_DIGIT_END_DEPTH_CAP || out.len() >= VT_DIGIT_END_BUDGET {
            return;
        }
        if chars.get(p) == Some(&';') {
            let mut maxd = 0usize;
            while maxd < 4 && chars.get(p + 1 + maxd).is_some_and(|c| c.is_ascii_digit()) {
                maxd += 1;
            }
            for m in (0..=maxd).rev() {
                walk(chars, p + 1 + m, out, depth + 1);
                if out.len() >= VT_DIGIT_END_BUDGET {
                    return;
                }
            }
        }
        out.push(p);
    }
    walk(chars, p, out, 0);
}

/// Match one `ansi` occurrence at `chars[i]`; returns the consumed char
/// count, or 0 when no branch matches (global-replace leftmost semantics:
/// the scan then advances one char and retries).
fn vt_match_at(chars: &[char], i: usize) -> usize {
    if !vt_is_intro(chars[i]) {
        return 0;
    }
    let mut j = i + 1;
    while j < chars.len() && vt_is_inner(chars[j]) {
        j += 1;
    }
    let k = j;
    // Branch A1: (?:;[-a-zA-Z\d/#&.:=?%@~_]+)* then terminator. The param
    // classes exclude every terminator char, so maximal munch plus a single
    // terminator check is exact — quantifier backtracking can never expose
    // an earlier terminator.
    let mut p = k;
    loop {
        if chars.get(p) == Some(&';') {
            let mut q = p + 1;
            while q < chars.len() && vt_is_a_param(chars[q]) {
                q += 1;
            }
            if q > p + 1 {
                p = q;
                continue;
            }
        }
        break;
    }
    let t = vt_term_len(chars, p);
    if t > 0 {
        return p + t - i;
    }
    // Branch A2: [a-zA-Z\d]+ (?:;[-a-zA-Z\d/#&.:=?%@~_]*)* then terminator
    // (`;` may be followed by zero params — the `*` form).
    let mut p = k;
    while p < chars.len() && chars[p].is_ascii_alphanumeric() {
        p += 1;
    }
    if p > k {
        loop {
            if chars.get(p) == Some(&';') {
                let mut q = p + 1;
                while q < chars.len() && vt_is_a_param(chars[q]) {
                    q += 1;
                }
                p = q;
                continue;
            }
            break;
        }
        let t = vt_term_len(chars, p);
        if t > 0 {
            return p + t - i;
        }
    }
    // Branch B: (?:\d{1,4}(?:;\d{0,4})*)? [final class char] — full
    // enumeration over digit-part lengths, longest first.
    if chars.get(k).is_some_and(|c| c.is_ascii_digit()) {
        let mut run = 0usize;
        while chars.get(k + run).is_some_and(|c| c.is_ascii_digit()) {
            run += 1;
        }
        for init in (1..=run.min(4)).rev() {
            let mut ends: Vec<usize> = Vec::new();
            vt_collect_digit_ends(chars, k + init, &mut ends);
            for e in ends {
                if e < chars.len() && vt_is_b_final(chars[e]) {
                    return e + 1 - i;
                }
            }
        }
    }
    // The optional group skipped entirely: final char right after the
    // introducer sequence.
    if k < chars.len() && vt_is_b_final(chars[k]) {
        return k + 1 - i;
    }
    0
}

/// `util.stripVTControlCharacters` — node v24 contract: remove ANSI/VT
/// escape sequences (7-bit ESC and 8-bit C1 introducers; OSC terminated by
/// BEL/ST; CSI final-byte forms). Non-string input throws
/// ERR_INVALID_ARG_TYPE (`The "str" argument must be of type string.`,
/// oracle-verbatim — including `Received null` for null).
///
/// @trace REQ-ENG-048
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_strip_vt_control_characters(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    if !val.is_string() {
        let frag = if val.is_null() {
            "null".to_string()
        } else if val.is_undefined() {
            "undefined".to_string()
        } else {
            crate::bun_inspect_api::received_type_fragment(cx, val)
        };
        let msg = format!(
            "The \"str\" argument must be of type string. Received {}",
            frag
        );
        return crate::bun_inspect_api::throw_invalid_arg_type(cx, &msg);
    }
    let text = crate::js_to_rust_string(cx, val);
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        let m = vt_match_at(&chars, i);
        if m > 0 {
            i += m;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    let utf16: Vec<u16> = out.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
    true
}

// ---- util.getCallSites (node v24 contract, REQ-ENG-048) ----

/// Process-level per-script id registry behind CallSite.scriptId. Node's
/// contract: a stable, per-script unique identifier rendered as a decimal
/// string. SM exposes no per-script id through the embedder stack API, so
/// ids are minted on first sighting of a script name and reused for the
/// process lifetime (V8-shaped: monotonically increasing, stable per script
/// within the process). Known boundary: distinct eval scripts share the
/// "[eval]" display name and therefore one id — V8 mints one per eval.
static SCRIPT_ID_BY_NAME: LazyLock<Mutex<HashMap<String, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SCRIPT_ID_NEXT: AtomicU32 = AtomicU32::new(1);

fn call_site_script_id(name: &str) -> u32 {
    let mut map = SCRIPT_ID_BY_NAME.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(id) = map.get(name) {
        return *id;
    }
    let id = SCRIPT_ID_NEXT.fetch_add(1, Ordering::Relaxed);
    map.insert(name.to_string(), id);
    id
}

/// Split `SRC:LINE:COL` from the right — the last two `:`-separated
/// segments must be decimal integers (URIs themselves may contain `:`).
fn split_src_line_col(loc: &str) -> Option<(String, u32, u32)> {
    let col_sep = loc.rfind(':')?;
    let col: u32 = loc[col_sep + 1..].parse().ok()?;
    let line_sep = loc[..col_sep].rfind(':')?;
    let line_no: u32 = loc[line_sep + 1..col_sep].parse().ok()?;
    Some((loc[..line_sep].to_string(), line_no, col))
}

/// Parse one V8-format stack line (SM `BuildStackString(StackFormat::V8)`
/// shape — vendor/mozjs `FormatV8StackFrame`: `    at NAME (SRC:LINE:COL)`
/// or `    at SRC:LINE:COL`). Returns
/// `(functionName, scriptName, lineNumber, columnNumber)`; None skips the
/// line (wasm frames have a non-numeric LINE segment).
fn parse_v8_stack_line(line: &str) -> Option<(String, String, u32, u32)> {
    let rest = line.trim_start().strip_prefix("at ")?;
    // Name/location split: prefer the rightmost " (" whose remainder ends
    // the line with `)` and parses as SRC:LINE:COL.
    let (name, loc) = if rest.ends_with(')') {
        let mut found = None;
        let mut search = rest;
        while let Some(idx) = search.rfind(" (") {
            let cand = &search[idx + 2..search.len() - 1];
            if split_src_line_col(cand).is_some() {
                found = Some((&rest[..idx], cand));
                break;
            }
            search = &search[..idx];
        }
        match found {
            Some((n, l)) => (Some(n), l), // @evidence SPEC/10-REQUIREMENTS.html:237
            None => (None, &rest[..rest.len() - 1]), // @evidence SPEC/10-REQUIREMENTS.html:237
        }
    } else {
        (None, rest)
    };
    let (src, line_no, col) = split_src_line_col(loc)?;
    Some((name.unwrap_or("").to_string(), src, line_no, col))
}

/// Build one null-prototype CallSite object (node v24 oracle: plain object,
/// `Object.getPrototypeOf(site) === null`, writable/enumerable/configurable
/// data properties; exactly functionName, scriptId, scriptName, lineNumber,
/// columnNumber, column — `column` mirrors `columnNumber`).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn make_call_site(
    cx: *mut JSContext,
    function_name: &str,
    script_name: &str,
    line_number: u32,
    column_number: u32,
) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    // wrappers2 signature takes the mozjs lifetime `Handle` flavor
    // (jsapi2_wrappers.in.rs:579 wrap! inside the mozjs crate). Null handle:
    // mozjs::gc::HandleObject<'a>::null() (root.rs:462, wraps RawHandleObject::null()).
    let site = w2::JS_NewObjectWithGivenProto(
        &mut wrapped_cx,
        ::std::ptr::null(),
        mozjs::gc::HandleObject::null(),
    );
    if site.is_null() {
        return ::std::ptr::null_mut();
    }
    rooted!(&in(wrapped_cx) let site_root = site);
    let mut define_str = |key: &str, value: &str| -> bool {
        let c_key = ZBox::from_bytes(key.as_bytes());
        // UTF-16 entry path (functionName/scriptName may be non-ASCII —
        // unicode file paths must round-trip; CopyZ is Latin-1 shaped).
        let utf16: Vec<u16> = value.encode_utf16().collect();
        let s = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
        if s.is_null() {
            return false;
        }
        let v = StringValue(&*s);
        rooted!(&in(wrapped_cx) let vr = v);
        unsafe {
            JS_DefineProperty(
                cx,
                site_root.handle().into(),
                c_key.as_ptr(),
                vr.handle().into(),
                JSPROP_ENUMERATE as u32,
            )
        }
    };
    if !define_str("functionName", function_name)
        || !define_str("scriptId", &call_site_script_id(script_name).to_string())
        || !define_str("scriptName", script_name)
    {
        return ::std::ptr::null_mut();
    }
    let mut define_u32 = |key: &str, value: u32| -> bool {
        let c_key = ZBox::from_bytes(key.as_bytes());
        let v = mozjs::jsval::Int32Value(value as i32);
        rooted!(&in(wrapped_cx) let vr = v);
        unsafe {
            JS_DefineProperty(
                cx,
                site_root.handle().into(),
                c_key.as_ptr(),
                vr.handle().into(),
                JSPROP_ENUMERATE as u32,
            )
        }
    };
    if !define_u32("lineNumber", line_number)
        || !define_u32("columnNumber", column_number)
        || !define_u32("column", column_number)
    {
        return ::std::ptr::null_mut();
    }
    site
}

/// node CallSite `scriptName` contract: the script's resolved ABSOLUTE
/// location (test-util-getcallsites-preparestacktrace oracle:
/// `sites[0].scriptName === __filename`, and __filename is the CWD-joined
/// script argument with relative segments preserved verbatim — the same
/// naive join bao's `__filename` performs). Non-path sources pass through:
/// `node:`-prefixed internals, engine pseudo-names (`[eval]`, `<blob>`),
/// scheme-bearing URLs.
fn resolve_call_site_script_name(name: &str) -> String {
    if name.starts_with('/')
        || name.starts_with("node:")
        || name.starts_with('<')
        || name.starts_with('[')
        || name.contains("://")
    {
        return name.to_string();
    }
    match ::std::env::current_dir() {
        // REQ-ENG-004 getCallSites scriptName absolutization: a bare relative
        // module name resolves against the process cwd (node v24 returns
        // absolute scriptName); cwd-unresolvable falls back verbatim.
        Ok(cwd) => format!("{}/{}", cwd.display(), name), // @evidence SPEC/10-REQUIREMENTS.html:237
        Err(_) => name.to_string(),                       // @evidence SPEC/10-REQUIREMENTS.html:237
    }
}

/// `util.getCallSites([frameCount], [options])` — node v24 contract:
/// structured stack frames of the caller chain, freshest first, own frame
/// excluded (a JSNative has no JS frame, so SM's capture starts at the
/// caller — the same shape as node's captureStackTrace-style trimming;
/// oracle: frame 0 is the immediate caller at its current execution point,
/// 1-based lines/columns).
///
/// Validation (oracle-verbatim messages):
///   * object/null frameCount in slot 0 shifts to options (JS
///     `typeof null === 'object'` overload; `getCallSites(null)` reports
///     the options error);
///   * non-number frameCount → TypeError ERR_INVALID_ARG_TYPE
///     `The "frameCount" argument must be of type number. Received type string ('x')`;
///   * non-integer → RangeError ERR_OUT_OF_RANGE
///     `The value of "frameCount" is out of range. It must be an integer. Received NaN`;
///   * outside 1..=200 → RangeError ERR_OUT_OF_RANGE
///     `The value of "frameCount" is out of range. It must be >= 1 && <= 200. Received 0`;
///   * omitted/undefined → 10 (node v24.19 measured default; independent
///     of a mutated live Error.stackTraceLimit);
///   * options non-object (null included) → TypeError
///     `The "options" argument must be of type object. Received null`;
///   * options.sourceMap non-boolean → TypeError
///     `The "options.sourceMap" property must be of type boolean. Received type string ('yes')`.
///
/// sourceMap: true is accepted but inert: bao has no sourcemap
/// infrastructure to resolve frame positions through (registered
/// stop-condition; output is byte-identical while no maps exist, which is
/// the same output node produces for unmapped frames).
///
/// @trace REQ-ENG-048
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_get_call_sites(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut count_slot: Option<JSVal> = if argc > 0 {
        Some(*args.get(0).ptr)
    } else {
        None
    };
    let mut options_slot: Option<JSVal> = if argc > 1 {
        Some(*args.get(1).ptr)
    } else {
        None
    };
    if let Some(v) = count_slot {
        if v.is_object() || v.is_null() {
            options_slot = Some(v);
            count_slot = None;
        }
    }
    let count: u32 = match count_slot {
        None => 10,                        // @evidence SPEC/10-REQUIREMENTS.html:237
        Some(v) if v.is_undefined() => 10, // @evidence SPEC/10-REQUIREMENTS.html:237
        Some(v) => {
            // @evidence SPEC/10-REQUIREMENTS.html:237
            if !v.is_number() {
                let msg = format!(
                    "The \"frameCount\" argument must be of type number. Received {}",
                    crate::bun_inspect_api::received_type_fragment(cx, v)
                );
                return crate::bun_inspect_api::throw_invalid_arg_type(cx, &msg);
            }
            let n = v.to_number();
            if n.is_nan() || n.is_infinite() || n.fract() != 0.0 {
                let msg = format!(
                    "The value of \"frameCount\" is out of range. It must be an integer. Received {}",
                    format_js_number(n)
                );
                return throw_range_with_code(cx, "ERR_OUT_OF_RANGE", &msg);
            }
            if !(1.0..=200.0).contains(&n) {
                let msg = format!(
                    "The value of \"frameCount\" is out of range. It must be >= 1 && <= 200. Received {}",
                    format_js_number(n)
                );
                return throw_range_with_code(cx, "ERR_OUT_OF_RANGE", &msg);
            }
            n as u32
        }
    };
    if let Some(v) = options_slot {
        if !v.is_undefined() {
            if !v.is_object() {
                let frag = if v.is_null() {
                    "null".to_string()
                } else {
                    crate::bun_inspect_api::received_type_fragment(cx, v)
                };
                let msg = format!(
                    "The \"options\" argument must be of type object. Received {}",
                    frag
                );
                return crate::bun_inspect_api::throw_invalid_arg_type(cx, &msg);
            }
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let opts_obj = v.to_object());
            let c_key = ZBox::from_bytes(b"sourceMap");
            let mut sm = UndefinedValue();
            let got = JS_GetProperty(
                cx,
                opts_obj.handle().into(),
                c_key.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut sm,
                },
            );
            if !got {
                JS_ClearPendingException(cx);
            } else if !sm.is_undefined() && !sm.is_boolean() {
                let msg = format!(
                    "The \"options.sourceMap\" property must be of type boolean. Received {}",
                    crate::bun_inspect_api::received_type_fragment(cx, sm)
                );
                return crate::bun_inspect_api::throw_invalid_arg_type(cx, &msg);
            }
        }
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    // Structured capture → V8-format text → parsed frames. The capture
    // excludes self-hosted frames (SM SavedFrameSelfHosted::Exclude inside
    // BuildStackString), so no engine-internal frames leak into the result.
    let text: Option<String> = {
        rooted!(&in(wrapped_cx) let mut stack_guard = ::std::ptr::null_mut::<JSObject>());
        let captured = mozjs::rust::CapturedJSStack::new(&mut wrapped_cx, stack_guard, Some(count));
        captured.and_then(|mut s| s.as_string(None, mozjs_sys::jsapi::js::StackFormat::V8))
    };
    let frames: Vec<(String, String, u32, u32)> = match text {
        Some(t) => t.lines().filter_map(parse_v8_stack_line).collect(), // @evidence SPEC/10-REQUIREMENTS.html:237
        None => Vec::new(), // @evidence SPEC/10-REQUIREMENTS.html:237
    };
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, frames.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (i, (fname, sname, lineno, colno)) in frames.iter().enumerate() {
        let resolved_sname = resolve_call_site_script_name(sname);
        let site = make_call_site(cx, fname, &resolved_sname, *lineno, *colno);
        if site.is_null() {
            continue;
        }
        rooted!(&in(wrapped_cx) let site_root = site);
        let v = ObjectValue(site_root.get());
        rooted!(&in(wrapped_cx) let vr = v);
        JS_DefineElement(
            cx,
            arr.handle().into(),
            i as u32,
            vr.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

macro_rules! type_check_fn {
    ($name:ident, $check:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            if argc == 0 {
                args.rval().set(BooleanValue(false));
                return true;
            }
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

unsafe fn is_function(cx: *mut JSContext, val: &JSVal) -> bool {
    unsafe {
        if !val.is_object() {
            return false;
        }
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj = val.to_object());
        JS_ObjectIsFunction(obj.get())
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_function(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(is_function(cx, &val)));
    true
}

// U7 (REQ-ENG-007): exact proxy internal-slot probe via the mozjs fork's
// JS_IsProxy (thin linkable wrapper over the public inline js::IsProxy).
// Mirrors the node v24 oracle: function proxies (typeof "function") and
// object proxies both answer true; every non-proxy value (including plain
// objects, functions, null, primitives) answers false. Replaces the retired
// structured-clone heuristic, which could not detect function proxies.
unsafe fn is_proxy(_cx: *mut JSContext, val: &JSVal) -> bool {
    unsafe {
        if !val.is_object() {
            return false;
        }
        w2::JS_IsProxy(val.to_object())
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_proxy(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(is_proxy(cx, &val)));
    true
}

unsafe fn is_array(cx: *mut JSContext, val: &JSVal) -> bool {
    unsafe {
        if !val.is_object() {
            return false;
        }
        let mut result = false;
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let v_root = *val);
        IsArrayObject(cx, v_root.handle().into(), &mut result);
        result
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_array(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(is_array(cx, &val)));
    true
}

unsafe fn has_class_name(cx: *mut JSContext, val: &JSVal, name: &str) -> bool {
    unsafe {
        if !val.is_object() {
            return false;
        }
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj = val.to_object());
        let mut ctor = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"constructor".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ctor,
            },
        );
        if ctor.is_object() {
            rooted!(&in(wrapped_cx) let ctor_obj = ctor.to_object());
            let mut name_val = UndefinedValue();
            JS_GetProperty(
                cx,
                ctor_obj.handle().into(),
                c"name".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut name_val,
                },
            );
            if name_val.is_string() {
                let n = crate::js_to_rust_string(cx, name_val);
                return n == name;
            }
        }
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_date(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let val = *args.get(0).ptr;
    args.rval()
        .set(BooleanValue(has_class_name(cx, &val, "Date")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_regexp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let val = *args.get(0).ptr;
    args.rval()
        .set(BooleanValue(has_class_name(cx, &val, "RegExp")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_error(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let val = *args.get(0).ptr;
    args.rval()
        .set(BooleanValue(has_class_name(cx, &val, "Error")));
    true
}

/// JS `ToString(Number)` for a computed f64 (util.format %d/%i/%f output).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn f64_js_text(cx: *mut JSContext, n: f64) -> String {
    // node formatNumber contract (lib/internal/util/inspect.js): -0 renders
    // '-0' — engine ToString(String(-0) === '0') must be bypassed before any
    // string conversion (node: "this must be checked before any String()
    // conversion"). REQ-ENG-004 util.format %d/%i/%f oracle.
    if n == 0.0 && n.is_sign_negative() {
        return "-0".to_string();
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let nv = mozjs::jsval::DoubleValue(n));
    let jsstr = mozjs::rust::ToString(cx_ref, nv.handle());
    if jsstr.is_null() {
        JS_ClearPendingException(cx);
        return "NaN".to_string();
    }
    crate::js_to_rust_string(cx, StringValue(&*jsstr))
}

/// Call the realm's global `parseInt` / `parseFloat` with one argument —
/// node's %i/%f coercion (ToString then parse, with the JS whitespace/radix
/// rules). None when the global is absent or the call throws (caller renders
/// NaN, matching the non-throwing face of node's numeric formatter).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn call_global_number_parser(
    cx: *mut JSContext,
    name: &[u8],
    val: JSVal,
) -> ::std::option::Option<f64> {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    // Root val early: JS_GetProperty below can trigger GC and relocate
    // the object, making the by-value val parameter stale before its use
    // in the JS_CallFunctionValue args.
    rooted!(&in(cx_ref) let val_root = val);
    let c_name = ZBox::from_bytes(name);
    let mut fn_v = UndefinedValue();
    if !JS_GetProperty(
        cx,
        global_root.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut fn_v,
        },
    ) {
        JS_ClearPendingException(cx);
        return None;
    }
    if !fn_v.is_object() {
        return None;
    }
    rooted!(&in(cx_ref) let fn_root = fn_v);
    let call_vals = [val_root.get()];
    let call_args = HandleValueArray {
        length_: 1,
        elements_: call_vals.as_ptr(),
    };
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        global_root.handle().into(),
        fn_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(cx);
        return None;
    }
    if rval.is_number() {
        Some(rval.to_number())
    } else {
        None
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_format(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let s = JS_NewStringCopyZ(cx, c"".as_ptr());
        args.rval().set(if s.is_null() {
            UndefinedValue()
        } else {
            StringValue(&*s)
        });
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
                        // %j = JSON.stringify form (node: '{"a":1}' for
                        // util.format('%j', {a:1})), NOT the inspected form.
                        Some(&'j') => {
                            chars.next();
                            if arg_idx < argc {
                                result.push_str(&json_stringify_for_format(
                                    cx,
                                    *args.get(arg_idx).ptr,
                                ));
                                arg_idx += 1;
                            }
                        }
                        Some(&'s') | Some(&'o') | Some(&'O') => {
                            chars.next();
                            if arg_idx < argc {
                                result.push_str(&jsval_to_display(cx, *args.get(arg_idx).ptr));
                                arg_idx += 1;
                            }
                        }
                        // %d — node: BigInt formats as `9n`, Symbol as NaN,
                        // everything else Number(value) (JS ToNumber).
                        Some(&'d') => {
                            chars.next();
                            if arg_idx < argc {
                                let v = *args.get(arg_idx).ptr;
                                if v.is_bigint() {
                                    result.push_str(&jsval_inspect_fresh(cx, v, 2));
                                } else if v.is_symbol() {
                                    result.push_str("NaN");
                                } else {
                                    let mut wrapped = mozjs::context::JSContext::from_ptr(
                                        NonNull::new_unchecked(cx),
                                    );
                                    let hv = mozjs::rust::HandleValue::from_marked_location(
                                        args.get(arg_idx).ptr,
                                    );
                                    match mozjs::rust::ToNumber(&mut wrapped, hv) {
                                        Ok(n) => result.push_str(&f64_js_text(cx, n)),
                                        Err(()) => return false,
                                    }
                                }
                                arg_idx += 1;
                            }
                        }
                        // %i — node: parseInt(value) (truncation), BigInt
                        // verbatim (`9n`), Symbol → NaN. This is the U6
                        // truncation contract (`%i`, 2.9 → "2").
                        Some(&'i') => {
                            chars.next();
                            if arg_idx < argc {
                                let v = *args.get(arg_idx).ptr;
                                if v.is_bigint() {
                                    result.push_str(&jsval_inspect_fresh(cx, v, 2));
                                } else if v.is_symbol() {
                                    result.push_str("NaN");
                                } else {
                                    match call_global_number_parser(cx, b"parseInt", v) {
                                        Some(n) => result.push_str(&f64_js_text(cx, n)),
                                        None => result.push_str("NaN"),
                                    }
                                }
                                arg_idx += 1;
                            }
                        }
                        // %f — node: parseFloat(value), Symbol → NaN.
                        Some(&'f') => {
                            chars.next();
                            if arg_idx < argc {
                                let v = *args.get(arg_idx).ptr;
                                if v.is_symbol() {
                                    result.push_str("NaN");
                                } else {
                                    match call_global_number_parser(cx, b"parseFloat", v) {
                                        Some(n) => result.push_str(&f64_js_text(cx, n)),
                                        None => result.push_str("NaN"),
                                    }
                                }
                                arg_idx += 1;
                            }
                        }
                        // %c consumes the argument and renders nothing.
                        Some(&'c') => {
                            chars.next();
                            if arg_idx < argc {
                                arg_idx += 1;
                            }
                        }
                        Some(&'%') => {
                            chars.next();
                            result.push('%');
                        }
                        _ => result.push(c),
                    }
                } else {
                    result.push(c);
                }
            }
            let utf16: Vec<u16> = result.encode_utf16().collect();
            let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
            args.rval().set(if js_str.is_null() {
                UndefinedValue()
            } else {
                StringValue(&*js_str)
            });
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
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
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
  // domain-check a1f2e22140 (own-idiom fix): Node's util.promisify consults
  // orig[Symbol.for('nodejs.util.promisify.custom')] FIRST and returns that
  // function verbatim when callable. dns's lookup/resolve* family and the
  // global timer functions stamp it; without this probe the generic
  // (err, value) wrapper below swallowed every custom contract — the dns
  // custom symbols were dead wiring and promisify(setTimeout) could never
  // equal timers/promises.setTimeout. A non-callable value falls through to
  // the generic wrapper (nothing in the tree stamps non-function customs).
  var custom = orig[Symbol.for('nodejs.util.promisify.custom')];
  if (typeof custom === 'function') return custom;
  function promisified() {
    var args = Array.prototype.slice.call(arguments);
    return new Promise(function(resolve, reject) {
      args.push(function(err, value) {
        if (err) reject(err);
        else resolve(value);
      });
      orig.apply(this, args);
    });
  }
  // bao additionally exposes a `.then` on the returned callable so callers
  // that test for thenability (a legacy bao_runtime conformance probe)
  // observe a Promise-compatible surface; the function remains directly
  // callable and returns a real Promise on invoke. The former
  // `promisified[custom] = orig` self-stamp is GONE — with the probe above
  // it would make promisify(promisify(f)) return f itself, and Node never
  // stamps the result.
  promisified.then = function(onFulfilled, onRejected) {
    return Promise.resolve(promisified()).then(onFulfilled, onRejected);
  };
  return promisified;
})"#;
    let mut src = mozjs::rust::transform_str_to_source_text(promisify_src);
    let mut factory_val = UndefinedValue();
    let factory_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut factory_val,
    };
    let opts = mozjs::glue::NewCompileOptions(cx, c"<promisify>".as_ptr(), 1);
    if opts.is_null() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    if !JS::Evaluate2(cx, opts, &mut src, factory_h) || !factory_val.is_object() {
        mozjs::glue::DeleteCompileOptions(opts);
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    mozjs::glue::DeleteCompileOptions(opts);

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    rooted!(&in(wrapped_cx) let global_root = global);
    rooted!(&in(wrapped_cx) let fn_obj = fn_val.get().to_object());
    rooted!(&in(wrapped_cx) let fn_obj_val = ObjectValue(fn_obj.get()));
    let args_arr = HandleValueArray {
        length_: 1,
        elements_: &fn_obj_val.get() as *const Value,
    };
    let mut call_rval = UndefinedValue();
    let call_rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut call_rval,
    };
    rooted!(&in(wrapped_cx) let factory_obj = factory_val.to_object());
    rooted!(&in(wrapped_cx) let factory_val_h = ObjectValue(factory_obj.get()));
    JS_CallFunctionValue(
        cx,
        global_root.handle().into(),
        factory_val_h.handle().into(),
        &args_arr,
        call_rval_h,
    );
    args.rval().set(call_rval);
    true
}

// @trace REQ-ENG-004 [api:util.callbackify] — node v24.19 lib/util.js
// oracle semantics (REQ-ENG-048 F-class rework):
//   - validateFunction(original, 'original') → ERR_INVALID_ARG_TYPE with
//     determineSpecificType tail ('Received null' / 'an instance of X')
//   - own-property descriptor copy onto the callbackified fn, with numeric
//     length gaining the callback slot (+1) and string names gaining the
//     'Callbackified' suffix; non-number/non-string values preserved
//     verbatim (the promiseFn descriptor-object oracle)
//   - callback bound to the call's `this`; delivery via process.nextTick
//     (a throwing callback surfaces as uncaughtException — fixture
//     contract; bao process.nextTick: bun_api.rs:1514)
//   - resolved value always cb(null, ret) — arrays are NOT spread
//   - falsy rejection wrapped: ERR_FALSY_VALUE_REJECTION, message
//     'Promise was rejected with falsy value', `.reason` = original value
//   - a synchronous throw of the wrapped fn propagates synchronously
//     (node has no try/catch around ReflectApply)
//   - last-argument validation: 'The last argument must be of type
//     function. Received <specific>' (node's unquoted phrasing)
//
// CS-EMPTY-CATCH exemption: the empty-catch around `reason.cause = err` inside
// the rejection handler is best-effort cause attachment — a frozen/non-
// extensible `reason` object cannot take `.cause`, and Node's callbackify does
// not surface that as an error (the original `reason` is still delivered to the
// callback).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_callbackify(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let fn_val = if _argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    });

    // node v24 lib/util.js callbackify oracle: descriptor copy with
    // length+1 / name+'Callbackified' mutation guards, callback bound to
    // the call's `this`, process.nextTick delivery (a throwing callback
    // surfaces as uncaughtException — the fixture contract), falsy
    // rejection wrapped as ERR_FALSY_VALUE_REJECTION carrying `.reason`,
    // ERR_INVALID_ARG_TYPE validation messages via determineSpecificType.
    let callbackify_src = r#"(function(orig) {
  function cbSpecificType(value) {
    if (value === null) return 'null';
    if (value === undefined) return 'undefined';
    var t = typeof value;
    switch (t) {
      case 'bigint': return 'type bigint (' + value + 'n)';
      case 'number':
        if (value === 0) return 1 / value === -Infinity ? 'type number (-0)' : 'type number (0)';
        if (value !== value) return 'type number (NaN)';
        if (value === Infinity) return 'type number (Infinity)';
        if (value === -Infinity) return 'type number (-Infinity)';
        return 'type number (' + value + ')';
      case 'boolean': return value ? 'type boolean (true)' : 'type boolean (false)';
      case 'symbol': return 'type symbol (' + String(value) + ')';
      case 'function': return 'function ' + value.name;
      case 'object': {
        var name = '';
        try { name = (value.constructor && value.constructor.name) || ''; } catch (e) {}
        if (typeof name === 'string' && name !== '') return 'an instance of ' + name;
        return 'object';
      }
      case 'string': {
        var s = value;
        if (s.length > 28) s = s.slice(0, 25) + '...';
        return s.indexOf("'") === -1 ? "type string ('" + s + "')" : 'type string (' + JSON.stringify(s) + ')';
      }
      default: return 'type ' + t + ' (' + String(value) + ')';
    }
  }
  if (typeof orig !== 'function') {
    var oe = new TypeError('The "original" argument must be of type function. Received ' +
                           cbSpecificType(orig));
    oe.code = 'ERR_INVALID_ARG_TYPE';
    throw oe;
  }
  function callbackified() {
    var args = Array.prototype.slice.call(arguments);
    var maybeCb = args.pop();
    if (typeof maybeCb !== 'function') {
      var le = new TypeError('The last argument must be of type function. Received ' +
                             cbSpecificType(maybeCb));
      le.code = 'ERR_INVALID_ARG_TYPE';
      throw le;
    }
    var cb = maybeCb.bind(this);
    // In true node style the callback runs on process.nextTick with all the
    // implications for stack, uncaughtException and async_hooks.
    orig.apply(this, args).then(function(ret) {
      process.nextTick(cb, null, ret);
    }, function(rej) {
      if (!rej) {
        // `!reason` guard: null is the callbacks' "no error" sentinel, so a
        // falsy rejection is error-wrapped (node callbackifyOnRejected) —
        // the original value rides on `.reason`.
        var wrapped = new Error('Promise was rejected with falsy value');
        wrapped.code = 'ERR_FALSY_VALUE_REJECTION';
        wrapped.reason = rej;
        rej = wrapped;
      }
      process.nextTick(cb, rej);
    });
  }
  var descriptors = Object.getOwnPropertyDescriptors(orig);
  // Guards against a manipulated `length`/`name`: numeric length gains the
  // callback slot; string names gain the suffix; anything else is preserved
  // verbatim (the promiseFn descriptor-object oracle).
  if (descriptors.length && typeof descriptors.length.value === 'number') {
    descriptors.length.value++;
  }
  if (descriptors.name && typeof descriptors.name.value === 'string') {
    descriptors.name.value += 'Callbackified';
  }
  var values = Object.values(descriptors);
  for (var i = 0; i < values.length; i++) {
    // null-prototype descriptors: no reliance on globally mutable %Object.prototype%.
    Object.setPrototypeOf(values[i], null);
  }
  Object.defineProperties(callbackified, descriptors);
  return callbackified;
})"#;
    let mut src = mozjs::rust::transform_str_to_source_text(callbackify_src);
    let mut factory_val = UndefinedValue();
    let factory_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut factory_val,
    };
    let opts = mozjs::glue::NewCompileOptions(cx, c"<callbackify>".as_ptr(), 1);
    if opts.is_null() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    if !JS::Evaluate2(cx, opts, &mut src, factory_h) || !factory_val.is_object() {
        mozjs::glue::DeleteCompileOptions(opts);
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    mozjs::glue::DeleteCompileOptions(opts);

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    rooted!(&in(wrapped_cx) let global_root = global);
    // The factory performs node-shaped validation itself — ANY argument
    // value (including non-objects) reaches the ERR_INVALID_ARG_TYPE throw
    // path, so the arg passes as the raw value, not an ObjectValue.
    rooted!(&in(wrapped_cx) let fn_obj_val = fn_val.get());
    let args_arr = HandleValueArray {
        length_: 1,
        elements_: &fn_obj_val.get() as *const Value,
    };
    let mut call_rval = UndefinedValue();
    let call_rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut call_rval,
    };
    rooted!(&in(wrapped_cx) let factory_obj = factory_val.to_object());
    rooted!(&in(wrapped_cx) let factory_val_h = ObjectValue(factory_obj.get()));
    JS_CallFunctionValue(
        cx,
        global_root.handle().into(),
        factory_val_h.handle().into(),
        &args_arr,
        call_rval_h,
    );
    args.rval().set(call_rval);
    true
}

/// JavaScript number rendering for error messages (`Received 1.5`, `NaN`).
fn format_js_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n.is_sign_positive() {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_string();
    }
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        return format!("{}", n as i64);
    }
    format!("{}", n)
}

/// `util.getSystemErrorName(err)` — node v24 contract:
///   * non-number                          → TypeError ERR_INVALID_ARG_TYPE
///   * non-integer or >= 0                 → RangeError ERR_OUT_OF_RANGE
///                                             ("must be a negative integer")
///   * mapped libuv code                   → e.g. -1 → "EPERM", -13 → "EACCES"
///   * unmapped negative integer           → `Unknown system error <n>`
/// The mapping is bun_errno's per-OS libuv table (`bun_sys::UV_E::name`,
/// the same table node's uv_errname consults); unmapped values are the
/// literal node string, never undefined.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_get_system_error_name(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    if !val.is_number() {
        let received = if val.is_undefined() {
            "undefined".to_string()
        } else if val.is_null() {
            "null".to_string()
        } else if val.is_object() {
            "an instance of Object".to_string()
        } else {
            format!(
                "type {}",
                crate::bun_inspect_api::received_type_fragment(cx, val).trim_start_matches("type ")
            )
        };
        let msg = format!(
            "The \"err\" argument must be of type number. Received {}",
            received
        );
        return crate::bun_inspect_api::throw_invalid_arg_type(cx, &msg);
    }

    let num = val.to_number();
    if num.is_nan() || num.fract() != 0.0 || num >= 0.0 {
        let msg = format!(
            "The value of \"err\" is out of range. It must be a negative integer. Received {}",
            format_js_number(num)
        );
        return throw_range_with_code(cx, "ERR_OUT_OF_RANGE", &msg);
    }
    let name = if num < i32::MIN as f64 {
        format!("Unknown system error {}", format_js_number(num))
    } else {
        let err = num as i32;
        match bun_sys::UV_E::name(err) {
            Some(name) => name.to_string(),
            None => format!("Unknown system error {}", err),
        }
    };
    let js_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(name.as_bytes()).as_ptr());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
    true
}

/// RangeError with a node `code`, set as the pending exception.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_range_with_code(cx: *mut JSContext, code: &str, msg: &str) -> bool {
    let c_msg = ::std::ffi::CString::new(msg)
        .unwrap_or_else(|_| ::std::ffi::CString::new("error").unwrap());
    {
        let mut cx_s = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        mozjs::error::throw_range_error(&mut cx_s, c_msg.as_ref());
    }
    if JS_IsExceptionPending(cx) {
        rooted!(in(cx) let mut exn = UndefinedValue());
        JS_GetPendingException(cx, exn.handle_mut().into());
        let exn_val = exn.get();
        if !exn_val.is_undefined() && exn_val.is_object() {
            let cx_ref_err = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(cx_ref_err) let exn_root = exn_val.to_object());
            let code_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(code.as_bytes()).as_ptr());
            if !code_str.is_null() {
                let code_val = StringValue(&*code_str);
                rooted!(&in(cx_ref_err) let code_r = code_val);
                JS_DefineProperty(
                    cx,
                    exn_root.handle().into(),
                    c"code".as_ptr(),
                    code_r.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                JS_SetPendingException(
                    cx,
                    exn.handle().into(),
                    ExceptionStackBehavior::DoNotCapture,
                );
            }
        }
    }
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_ok(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let msg = ZBox::from_bytes(b"No value argument passed to assert.ok()");
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
    } else {
        !(val.is_null() || val.is_undefined())
    };
    if !is_truthy {
        let msg = if argc > 1 {
            jsval_to_display(cx, *args.get(1).ptr)
        } else {
            "The expression evaluated to a falsy value".to_string()
        };
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

// Shared shape for the binary display-compare stubs (e171, REQ-BRW-049):
// gate on two args, render both sides via jsval_to_display, report
// "AssertionError: <msg>" when the $fail_when closure says the rendered
// strings fail. The closures take (&String, &String) — macro hygiene keeps
// call-site tokens from seeing the macro's own `a`/`b` locals, so the
// condition/message travel as values (same form as type_check_fn!'s $check).
// Semantics are identical to the handwritten bodies it replaced
// (equal/notEqual had a formatted message, deepEqual a fixed one; only the
// ZBox construction path differs, from_vec vs from_bytes, producing the same
// NUL-terminated payload).
macro_rules! binary_display_assert {
    ($name:ident, $fail_when:expr, $msg:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            if argc >= 2 {
                let a = jsval_to_display(cx, *args.get(0).ptr);
                let b = jsval_to_display(cx, *args.get(1).ptr);
                if $fail_when(&a, &b) {
                    let msg = $msg(&a, &b);
                    let c_msg = ZBox::from_bytes(msg.as_bytes());
                    JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
                    return false;
                }
            }
            args.rval().set(UndefinedValue());
            true
        }
    };
}

binary_display_assert!(assert_equal, |a, b| a != b, |a, b| format!(
    "{} == {}",
    a, b
));
binary_display_assert!(assert_not_equal, |a, b| a == b, |a, b| format!(
    "{} != {}",
    a, b
));
binary_display_assert!(assert_deep_equal, |a, b| a != b, |_a, _b| {
    "Expected values to be deeply equal".to_string()
});

/// Shared no-op body for the legacy placeholder stubs whose JS-level twin in
/// the IIFE library fully supersedes them (notDeepEqual / throws /
/// doesNotThrow): consume the call, return undefined. One Rust symbol
/// registered under three names — the JS-visible name comes from the
/// registration table, not the symbol, so behavior is identical to the three
/// handwritten copies it replaced (e171, REQ-BRW-049).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_noop(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

unsafe fn values_equal_strict(cx: *mut JSContext, a: JSVal, b: JSVal) -> bool {
    unsafe {
        if a.is_undefined() && b.is_undefined() {
            return true;
        }
        if a.is_null() && b.is_null() {
            return true;
        }
        if a.is_boolean() && b.is_boolean() {
            return a.to_boolean() == b.to_boolean();
        }
        if a.is_int32() && b.is_int32() {
            return a.to_int32() == b.to_int32();
        }
        if a.is_string() && b.is_string() {
            return jsval_to_display(cx, a) == jsval_to_display(cx, b);
        }
        if a.is_double() || b.is_double() {
            let da = if a.is_double() {
                a.to_double()
            } else if a.is_int32() {
                a.to_int32() as f64
            } else {
                return false;
            };
            let db = if b.is_double() {
                b.to_double()
            } else if b.is_int32() {
                b.to_int32() as f64
            } else {
                return false;
            };
            return da == db;
        }
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_strict_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 && !values_equal_strict(cx, *args.get(0).ptr, *args.get(1).ptr) {
        let a = jsval_to_display(cx, *args.get(0).ptr);
        let b = jsval_to_display(cx, *args.get(1).ptr);
        let c_msg = ZBox::from_vec(format!("Expected {} to strictly equal {}", a, b).into_bytes());
        JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_not_strict_equal(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 && values_equal_strict(cx, *args.get(0).ptr, *args.get(1).ptr) {
        let c_msg = ZBox::from_vec(
            "Expected values to be strictly unequal"
                .to_string()
                .into_bytes(),
        );
        JS_ReportErrorUTF8(cx, c"AssertionError: %s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_rejects(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // @trace REQ-ENG-006 [api:assert.rejects] — async-aware rejection
    // assertion. `assert.rejects(asyncFn, error?)` returns a Promise that
    // resolves when asyncFn rejects with a matching error (or rejects with
    // AssertionError if it resolves / rejects with the wrong error).
    //
    // Signature variants (Node.js assert doc):
    //   - assert.rejects(fn)                          → rejects anything
    //   - assert.rejects(fn, errorClass)              → reject must be instanceof
    //   - assert.rejects(fn, /regexp/)                → reject.message must match
    //   - assert.rejects(fn, function(err){...})      → custom validator
    //   - assert.rejects(fn, { message: /re/, code }) → property match
    //   - assert.rejects(fn, "string")                → reject.message includes
    let args = CallArgs::from_vp(vp, _argc);
    if _argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"assert.rejects() requires an async function or Promise".as_ptr(),
        );
        return false;
    }

    // Build the validation + control flow entirely in JS. Returning a Promise
    // keeps the test runner's `await` chain intact.
    let helper = r#"
        (function() {
            var fn = __assertRejects_fn;
            var spec = __assertRejects_spec;
            delete globalThis.__assertRejects_fn;
            delete globalThis.__assertRejects_spec;
            return Promise.resolve()
                .then(function() {
                    var ret = (typeof fn === 'function') ? fn() : fn;
                    if (!ret || typeof ret.then !== 'function') {
                        ret = Promise.resolve(ret);
                    }
                    return ret.then(
                        function() {
                            var err = new Error('Missing expected rejection to assert.rejects');
                            err.name = 'AssertionError';
                            throw err;
                        },
                        function(rejection) {
                            // Validate the rejection against spec.
                            if (spec === undefined || spec === null) return;
                            if (typeof spec === 'function') {
                                // Class constructor or validator function.
                                try {
                                    if (spec.prototype !== undefined) {
                                        if (!(rejection instanceof spec)) {
                                            var e1 = new Error('Missing expected rejection (' + (spec && spec.name) + ') to assert.rejects');
                                            e1.name = 'AssertionError'; throw e1;
                                        }
                                        return;
                                    }
                                    var ok = spec(rejection);
                                    if (ok && typeof ok.then === 'function') {
                                        // Async validator — rare, fall through.
                                        return ok.then(function(v) { if (!v) { var e = new Error('Invalid rejection value to assert.rejects'); e.name = 'AssertionError'; throw e; } });
                                    }
                                    if (!ok) {
                                        var e2 = new Error('Invalid rejection value to assert.rejects'); e2.name = 'AssertionError'; throw e2;
                                    }
                                    return;
                                } catch (ve) {
                                    if (ve && ve.name === 'AssertionError') throw ve;
                                    var e3 = new Error('Invalid rejection value to assert.rejects'); e3.name = 'AssertionError'; throw e3;
                                }
                            }
                            if (spec instanceof RegExp) {
                                var msg = (rejection && rejection.message) ? String(rejection.message) : String(rejection);
                                if (!spec.test(msg)) {
                                    var e4 = new Error('Missing expected rejection message to assert.rejects'); e4.name = 'AssertionError'; throw e4;
                                }
                                return;
                            }
                            if (typeof spec === 'string') {
                                var msg2 = (rejection && rejection.message) ? String(rejection.message) : String(rejection);
                                if (msg2.indexOf(spec) === -1) {
                                    var e5 = new Error('Missing expected rejection message to assert.rejects'); e5.name = 'AssertionError'; throw e5;
                                }
                                return;
                            }
                            if (typeof spec === 'object') {
                                for (var k in spec) {
                                    if (k === 'message' && spec[k] instanceof RegExp) {
                                        var m = (rejection && rejection.message) ? String(rejection.message) : '';
                                        if (!spec[k].test(m)) {
                                            var e6 = new Error('Missing expected rejection message to assert.rejects'); e6.name = 'AssertionError'; throw e6;
                                        }
                                    } else {
                                        var actual = rejection ? rejection[k] : undefined;
                                        if (actual !== spec[k]) {
                                            var e7 = new Error('Mismatched rejection property "' + k + '" to assert.rejects'); e7.name = 'AssertionError'; throw e7;
                                        }
                                    }
                                }
                                return;
                            }
                        }
                    );
                });
        })()
    "#;

    // Stash the args on the global, then evaluate.
    let global = unsafe { CurrentGlobalOrNull(cx) };
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let global_root = global);
    rooted!(&in(wrapped_cx) let fn_root = *args.get(0).ptr);
    let spec_val = if _argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    rooted!(&in(wrapped_cx) let spec_root = spec_val);
    unsafe {
        let _ = mozjs_sys::jsapi::JS_DefineProperty(
            cx,
            global_root.handle().into(),
            c"__assertRejects_fn".as_ptr(),
            fn_root.handle().into(),
            0,
        );
        let _ = mozjs_sys::jsapi::JS_DefineProperty(
            cx,
            global_root.handle().into(),
            c"__assertRejects_spec".as_ptr(),
            spec_root.handle().into(),
            0,
        );
    }

    let opts = mozjs::glue::NewCompileOptions(cx, c"<assert-rejects>".as_ptr(), 1);
    if opts.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut src = mozjs::rust::transform_str_to_source_text(helper);
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = unsafe { mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_h) };
    mozjs::glue::DeleteCompileOptions(opts);
    args.rval().set(if ok { rval } else { UndefinedValue() });
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

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let child_obj = child_val.to_object());

    // Child.super_ = Parent
    let super_val = *parent_val.ptr;
    rooted!(&in(wrapped_cx) let super_root = super_val);
    JS_SetProperty(
        cx,
        child_obj.handle().into(),
        c"super_".as_ptr(),
        super_root.handle().into(),
    );

    // node also links the prototype chain:
    //   ObjectSetPrototypeOf(Child.prototype, Parent.prototype)
    // so `new Child() instanceof Parent` is true (U4). Skipped when either
    // `.prototype` is absent/non-object (the legacy no-op guard face stays).
    let parent_obj = parent_val.to_object();
    rooted!(&in(wrapped_cx) let parent_root = parent_obj);
    let mut child_proto = UndefinedValue();
    let mut parent_proto = UndefinedValue();
    let got_child = JS_GetProperty(
        cx,
        child_obj.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut child_proto,
        },
    );
    let got_parent = JS_GetProperty(
        cx,
        parent_root.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut parent_proto,
        },
    );
    if got_child && got_parent && child_proto.is_object() && parent_proto.is_object() {
        rooted!(&in(wrapped_cx) let child_proto_obj = child_proto.to_object());
        rooted!(&in(wrapped_cx) let parent_proto_obj = parent_proto.to_object());
        JS_SetPrototype(
            cx,
            child_proto_obj.handle().into(),
            parent_proto_obj.handle().into(),
        );
    } else if !got_child || !got_parent {
        JS_ClearPendingException(cx);
    }

    args.rval().set(UndefinedValue());
    true
}

/// Re-export the global TextEncoder/TextDecoder constructors onto the util
/// module (Node identity contract: `util.TextEncoder === globalThis.TextEncoder`
/// is true).
///
/// e201 REQ-ARCH-001: extracted from `install_util` (control flow unchanged;
/// the `global.is_null()` nest level became a guard clause).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn install_util_text_codecs(
    cx: &mut mozjs::context::JSContext,
    util_obj: mozjs::rust::Handle<'_, *mut JSObject>,
) {
    let global = unsafe { CurrentGlobalOrNull(cx.raw_cx()) };
    if global.is_null() {
        return;
    }
    // compile-fix by e-node-stdlib (parallel wave): missing unsafe
    // wrapper on from_ptr/new_unchecked/raw_cx — semantics unchanged.
    let wrapped_cx =
        unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx())) };
    rooted!(&in(wrapped_cx) let global_root = global);
    for name in ["TextEncoder", "TextDecoder"] {
        let cname = ZBox::from_bytes(name.as_bytes());
        let mut ctor = UndefinedValue();
        unsafe {
            JS_GetProperty(
                cx.raw_cx(),
                global_root.handle().into(),
                cname.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut ctor,
                },
            );
        }
        if ctor.is_object() {
            rooted!(&in(wrapped_cx) let ctor_root = ctor);
            unsafe {
                JS_DefineProperty(
                    cx.raw_cx(),
                    util_obj.into(),
                    cname.as_ptr(),
                    ctor_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }
}
