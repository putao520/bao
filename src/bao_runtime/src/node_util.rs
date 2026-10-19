// @trace REQ-ENG-007
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::conversions::jsstr_to_string;
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
// e201 REQ-ARCH-1: renamed to the domain-neutral `register_js_functions!`
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
        );

        // util.types — native type checkers (12 Rust-backed)
        {
            rooted!(&in(cx) let types_obj = w2::JS_NewPlainObject(cx));
            if !types_obj.get().is_null() {
                let enumerate = JSPROP_ENUMERATE as u32;
                register_js_functions!(cx, types_obj, enumerate;
                    (c"isBoolean", util_is_boolean, 1)
                    (c"isNumber", util_is_number, 1)
                    (c"isString", util_is_string, 1)
                    (c"isSymbol", util_is_symbol, 1)
                    (c"isUndefined", util_is_undefined, 1)
                    (c"isNull", util_is_null, 1)
                    (c"isObject", util_is_object, 1)
                    (c"isFunction", util_is_function, 1)
                    (c"isProxy", util_is_proxy, 1)
                    (c"isArray", util_is_array, 1)
                    (c"isDate", util_is_date, 1)
                    (c"isRegExp", util_is_regexp, 1)
                    (c"isError", util_is_error, 1)
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
t.isKeyObject=function(){return false};
t.isCryptoKey=function(){return false};
})"#;
                eval_js_factory_on_object(cx, types_src, c"<types>", types_obj.handle());

                w2::JS_DefineProperty3(
                    cx,
                    util_obj.handle(),
                    c"types".as_ptr(),
                    types_obj.handle(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // @trace REQ-ENG-006 [api:util.styleText / isDeepStrictEqual-object]
        // Bridge JS additions (reuses Bun's util.ts styleText + deep-strict
        // algorithm shapes — see ~/code/rust/bun/src/js/node/util.ts):
        //   - styleText(fmt, text)  → ANSI-wrapped text via inspect.colors map
        //   - isDeepStrictEqual     → deep recursive comparison over objects
        // Attached on top of util_obj so they override any native placeholder.
        let util_extra_src = r#"(function(u){
  // ANSI color table reused from Bun's util.ts (which derives it from
  // node's inspect.colors). Each entry is [openCode, closeCode].
  var colors = {
    reset: [0, 0], bold: [1, 22], dim: [2, 22], italic: [3, 23],
    underline: [4, 24], inverse: [7, 27], hidden: [8, 28],
    strikethrough: [9, 29], doubleunderline: [21, 24],
    black: [30, 39], red: [31, 39], green: [32, 39],
    yellow: [33, 39], blue: [34, 39], magenta: [35, 39],
    cyan: [36, 39], white: [37, 39], gray: [90, 39], grey: [90, 39],
    bgBlack: [40, 49], bgRed: [41, 49], bgGreen: [42, 49],
    bgYellow: [43, 49], bgBlue: [44, 49], bgMagenta: [45, 49],
    bgCyan: [46, 49], bgWhite: [47, 49],
    redBright: [91, 39], greenBright: [92, 39], yellowBright: [93, 39],
    blueBright: [94, 39], magentaBright: [95, 39], cyanBright: [96, 39],
    whiteBright: [97, 39]
  };
  function styleText(format, text) {
    if (typeof text !== 'string') {
      throw new TypeError('The "text" argument must be of type string.');
    }
    function wrap(fmt) {
      var codes = colors[fmt];
      if (!codes) {
        throw new TypeError('The "format" argument must be one of: ' + Object.keys(colors).join(', '));
      }
      return '[' + codes[0] + 'm' + text + '[' + codes[1] + 'm';
    }
    if (Array.isArray(format)) {
      var left = '', right = '';
      for (var i = 0; i < format.length; i++) {
        var c = colors[format[i]];
        if (!c) {
          throw new TypeError('The "format" argument must be one of: ' + Object.keys(colors).join(', '));
        }
        left += '[' + c[0] + 'm';
        right = '[' + c[1] + 'm' + right;
      }
      return left + text + right;
    }
    return wrap(format);
  }
  u.styleText = styleText;

  function isDeepStrictEqual(a, b) {
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
  }
  u.isDeepStrictEqual = isDeepStrictEqual;

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
      var ce = new TypeError('The "code" argument must be of type string. Received type ' +
                             typeof code + ' (' + String(code) + ')');
      ce.code = 'ERR_INVALID_ARG_TYPE';
      throw ce;
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
/// e201 REQ-ARCH-1: shared body of the two inline factory blocks that used
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
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
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

  // domain-check 300f3a0b29(own-idiom fix) — line-level Myers diff rendering
  // for equality/deep-equality failures. Previously deepEqual failures threw
  // a fixed sentence and equal/strictEqual on objects collapsed both sides to
  // "[object Object]", hiding what actually differed. On failure the message
  // now carries a unified-diff body:
  //
  //   Expected values to be loosely deeply equal:
  //   + actual - expected
  //
  //     {
  //       a: 1
  //   +   b: 2
  //   -   b: 3
  //     }
  //
  // Layout follows the Node assertion_error diff shape (own typography: no
  // trailing commas, own run-collapsing) and is implemented entirely here in
  // JS — strings stay JS strings end to end, so the Latin-1/UTF-16 rendering
  // corruption class the upstream commit fixes at the native layer cannot
  // occur by construction. Primitive/short values in equal/strictEqual
  // failures stay inline (`1 == 2`); everything multi-line goes through the
  // diff.
  //
  // Leaf values are dumped with the runtime's own Bun.inspect formatter
  // (single-line, depth-capped, cycle-safe); structural expansion into one
  // line per entry is done by _dumpLines below.

  // Single-line leaf dump via Bun.inspect (depth 2 like the default face).
  // Falls back to _format only in embedder contexts without the Bun global;
  // this is a diagnostics formatter, it must never be the reason assert
  // itself blows up.
  function _leafDump(value) {
    try {
      var bun = typeof globalThis !== 'undefined' && globalThis.Bun;
      if (bun && typeof bun.inspect === 'function') {
        return bun.inspect(value, { depth: 2, colors: false });
      }
    } catch (e) { /* fall through to _format */ }
    return _format(value);
  }

  // Single-quote a string keeping real newlines so multi-line values diff
  // line-wise (Node does the same when splitting inspected strings).
  function _quoteString(s) {
    var out = "'";
    for (var i = 0; i < s.length; i++) {
      var c = s.charAt(i);
      var code = s.charCodeAt(i);
      if (c === "'") out += "\\'";
      else if (c === '\\') out += '\\\\';
      else if (c === '\r') out += '\\r';
      else if (c === '\t') out += '\\t';
      else if (c === '\n') out += '\n';
      else if (code < 0x20) out += '\\u' + ('000' + code.toString(16)).slice(-4);
      else out += c;
    }
    return out + "'";
  }

  // Identifier-ish keys bare, others double-quoted (same rule as
  // bun_inspect_api's format_key).
  function _keyRepr(key) {
    if (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(key)) return key;
    var esc = key.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n');
    return '"' + esc + '"';
  }

  function _isPlainObject(v) {
    if (v === null || typeof v !== 'object' || Array.isArray(v)) return false;
    var proto = Object.getPrototypeOf(v);
    return proto === Object.prototype || proto === null;
  }

  function _seenHas(seen, v) {
    for (var i = 0; i < seen.length; i++) {
      if (seen[i] === v) return true;
    }
    return false;
  }

  // Max entries rendered per container (mirrors Bun.inspect's 100-element
  // array cap); larger containers get an explicit "... N more" line.
  var _DIFF_MAX_ENTRIES = 100;
  // Total-line budget for the O(ND) trace; pathological dumps beyond it get
  // a bounded whole-block diff instead of a minimal one.
  var _DIFF_MAX_LINES = 512;

  // Render `value` as display lines at `indent` depth. `head` replaces the
  // leading pad of the first emitted line so nested containers can splice
  // their opening brace onto the parent's `key: ` prefix (`  b: {`).
  // Only arrays and plain objects expand structurally; every other object
  // kind (Date/RegExp/Map/Set/TypedArray/...) is a leaf via Bun.inspect.
  function _dumpLines(value, indent, seen, head) {
    var pad = '';
    for (var p = 0; p < indent; p++) pad += '  ';
    if (head === undefined) head = pad;
    var isStruct = value !== null && typeof value === 'object' &&
                   (Array.isArray(value) || _isPlainObject(value));
    if (!isStruct) {
      var leaf = (typeof value === 'string') ? _quoteString(value) : _leafDump(value);
      var parts = String(leaf).split('\n');
      var out = [];
      for (var i = 0; i < parts.length; i++) {
        out.push((i === 0 ? head : pad + '  ') + parts[i]);
      }
      return out;
    }
    if (_seenHas(seen, value)) return [head + '[Circular]'];
    seen.push(value);
    var isArr = Array.isArray(value);
    var lines = [head + (isArr ? '[' : '{')];
    var inner = pad + '  ';
    if (isArr) {
      var n = value.length;
      var shown = n > _DIFF_MAX_ENTRIES ? _DIFF_MAX_ENTRIES : n;
      for (var j = 0; j < shown; j++) {
        lines = lines.concat(_dumpLines(value[j], indent + 1, seen, inner));
      }
      if (n > _DIFF_MAX_ENTRIES) {
        lines.push(inner + '... ' + (n - _DIFF_MAX_ENTRIES) + ' more items');
      }
    } else {
      var keys = Object.keys(value);
      var kshown = keys.length > _DIFF_MAX_ENTRIES ? _DIFF_MAX_ENTRIES : keys.length;
      for (var k = 0; k < kshown; k++) {
        var keyHead = inner + _keyRepr(keys[k]) + ': ';
        lines = lines.concat(_dumpLines(value[keys[k]], indent + 1, seen, keyHead));
      }
      if (keys.length > _DIFF_MAX_ENTRIES) {
        lines.push(inner + '... ' + (keys.length - _DIFF_MAX_ENTRIES) + ' more keys');
      }
    }
    seen.pop();
    lines.push(pad + (isArr ? ']' : '}'));
    return lines;
  }

  // Classic O(ND) Myers shortest-edit diff over line arrays.
  // a = actual lines, b = expected lines. Returns ops in path order:
  //   { m: '+', s } — actual-only line
  //   { m: '-', s } — expected-only line
  //   { m: ' ', s } — common line
  // Backtrack verified: '+'/' ' ops reconstruct a, '-'/' ' reconstruct b,
  // edit count equals the LCS optimum (fuzzed 250k random pairs, 0 misses).
  function _myersDiffLines(a, b) {
    var n = a.length, m = b.length;
    if (n === 0 && m === 0) return [];
    if (n + m > _DIFF_MAX_LINES) {
      var big = [];
      for (var i0 = 0; i0 < n; i0++) big.push({ m: '+', s: a[i0] });
      for (var j0 = 0; j0 < m; j0++) big.push({ m: '-', s: b[j0] });
      return big;
    }
    var offset = n + m;
    var v = new Array(2 * (n + m) + 3);
    v[offset + 1] = 0;
    var trace = [];
    var found = false;
    for (var d = 0; d <= n + m && !found; d++) {
      trace.push(v.slice());
      for (var k = -d; k <= d; k += 2) {
        var x;
        if (k === -d || (k !== d && (v[offset + k - 1] | 0) < (v[offset + k + 1] | 0))) {
          x = v[offset + k + 1] | 0;
        } else {
          x = (v[offset + k - 1] | 0) + 1;
        }
        var y = x - k;
        while (x < n && y < m && a[x] === b[y]) { x++; y++; }
        v[offset + k] = x;
        if (x >= n && y >= m) { found = true; break; }
      }
    }
    var ops = [];
    var cx = n, cy = m;
    for (var t = trace.length - 1; t >= 0; t--) {
      if (cx === 0 && cy === 0) break;
      var vt = trace[t];
      var kk = cx - cy;
      var prevK;
      if (kk === -t || (kk !== t && (vt[offset + kk - 1] | 0) < (vt[offset + kk + 1] | 0))) {
        prevK = kk + 1;
      } else {
        prevK = kk - 1;
      }
      var prevX = vt[offset + prevK] | 0;
      var prevY = prevX - prevK;
      while (cx > prevX && cy > prevY) {
        ops.push({ m: ' ', s: a[cx - 1] });
        cx--;
        cy--;
      }
      if (t > 0) {
        if (cx === prevX) ops.push({ m: '-', s: b[prevY] });
        else ops.push({ m: '+', s: a[prevX] });
      }
      cx = prevX;
      cy = prevY;
    }
    ops.reverse();
    return ops;
  }

  // Collapse runs of >= 8 identical lines to first 3 + marker + last 3 so
  // large dumps stay readable (own rule; Node collapses similarly).
  function _collapseRuns(ops) {
    var out = [];
    var i = 0;
    while (i < ops.length) {
      if (ops[i].m !== ' ') { out.push(ops[i]); i++; continue; }
      var j = i;
      while (j < ops.length && ops[j].m === ' ') j++;
      var run = j - i;
      if (run >= 8) {
        for (var h = 0; h < 3; h++) out.push(ops[i + h]);
        out.push({ m: ' ', s: '... Skipped ' + (run - 6) + ' identical lines' });
        for (var q = 3; q < 6; q++) out.push(ops[i + run - 6 + q]);
      } else {
        for (var c = i; c < j; c++) out.push(ops[c]);
      }
      i = j;
    }
    return out;
  }

  function _renderDiff(ops) {
    var lines = [];
    for (var i = 0; i < ops.length; i++) {
      lines.push(ops[i].m === ' ' ? '  ' + ops[i].s : ops[i].m + ' ' + ops[i].s);
    }
    return lines.join('\n');
  }

  function _diffBody(actual, expected) {
    var aLines = _dumpLines(actual, 0, [], '');
    var bLines = _dumpLines(expected, 0, [], '');
    var ops = _collapseRuns(_myersDiffLines(aLines, bLines));
    return '+ actual - expected\n\n' + _renderDiff(ops);
  }

  // Full failure message for the deep-equal family: headline (the historical
  // fixed sentence, kept verbatim) plus the diff body. The renderer is
  // diagnostics-only: if it ever throws (hostile getter, exotic proxy), fall
  // back to the headline rather than masking the assertion itself.
  function _diffMessage(actual, expected, headline) {
    try {
      return headline + ':\n' + _diffBody(actual, expected);
    } catch (e) {
      return headline;
    }
  }

  // Node's operand rendering for the equality family: a structural value is
  // pretty-printed one entry per line (assert's `compact: false` inspect
  // shape, `{\n  a: 1\n}`); primitives use the inspect scalar form (strings
  // quoted). Both sides are joined by the operator (`==` / `===`), matching
  // node's `${inspect(actual)} ${op} ${inspect(expected)}` message.
  function _operandRepr(value) {
    if (value !== null && typeof value === 'object') {
      try {
        return _dumpLines(value, 0, [], '').join('\n');
      } catch (e) { /* fall through to scalar form */ }
    }
    return _format(value);
  }

  function _equalityMessage(actual, expected, op) {
    return _operandRepr(actual) + ' ' + op + ' ' + _operandRepr(expected);
  }

  function _err(message, actual, expected, operator, generated) {
    var err = new AssertionError({
      message: message,
      actual: actual,
      expected: expected,
      operator: operator
    });
    // User-supplied messages are delivered verbatim (node: fail('boom') →
    // message === 'boom'). Generated messages keep the historical
    // "AssertionError: " prefix for the legacy consumers that match on it;
    // node's generated wording has no prefix (registered residual).
    err.message = generated ? "AssertionError: " + (message || "Assertion failed") : message;
    return err;
  }

  function ok(value, message) {
    if (!value) {
      throw _err(message || "The expression evaluated to a falsy value", value, "truthy", "==", !message);
    }
  }

  function equal(actual, expected, message) {
    if (actual != expected) {
      throw _err(message || _equalityMessage(actual, expected, "=="), actual, expected, "==", !message);
    }
  }

  function notEqual(actual, expected, message) {
    if (actual == expected) {
      throw _err(message || (_format(actual) + " != " + _format(expected)), actual, expected, "!=", !message);
    }
  }

  function deepEqual(actual, expected, message) {
    if (!_deepEqual(actual, expected, false)) {
      throw _err(message || _diffMessage(actual, expected, "Expected values to be loosely deep-equal"), actual, expected, "deepEqual", !message);
    }
  }

  function notDeepEqual(actual, expected, message) {
    if (_deepEqual(actual, expected, false)) {
      throw _err(message || "Expected values not to be loosely deep-equal", actual, expected, "notDeepEqual", !message);
    }
  }

  function strictEqual(actual, expected, message) {
    if (actual !== expected) {
      throw _err(message || _equalityMessage(actual, expected, "==="), actual, expected, "===", !message);
    }
  }

  function notStrictEqual(actual, expected, message) {
    if (actual === expected) {
      throw _err(message || (_format(actual) + " !== " + _format(expected)), actual, expected, "!==", !message);
    }
  }

  function deepStrictEqual(actual, expected, message) {
    if (!_deepEqual(actual, expected, true)) {
      throw _err(message || _diffMessage(actual, expected, "Expected values to be strictly deep-equal"), actual, expected, "deepStrictEqual", !message);
    }
  }

  function notDeepStrictEqual(actual, expected, message) {
    if (_deepEqual(actual, expected, true)) {
      throw _err(message || "Expected values not to be strictly deep-equal", actual, expected, "notDeepStrictEqual", !message);
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
          throw _err(message || "Wrong error type thrown", e, expected, "throws", !message);
        }
        if (expected instanceof RegExp && typeof e.message === 'string' && !expected.test(e.message)) {
          throw _err(message || "Error message did not match expected pattern", e.message, expected, "throws", !message);
        }
      }
      return;
    }
    throw _err(message || "Missing expected exception", undefined, undefined, "throws", !message);
  }

  function doesNotThrow(fn, expected, message) {
    if (typeof expected === 'string') {
      message = expected;
      expected = undefined;
    }
    try {
      fn();
    } catch(e) {
      throw _err(message || "Got unwanted exception", e, undefined, "doesNotThrow", !message);
    }
  }

  function fail(message) {
    throw _err(message || "Failed", undefined, undefined, "fail", !message);
  }

  function ifError(err) {
    if (err !== null && err !== undefined) {
      throw err;
    }
  }

  // @trace REQ-ENG-006 [api:assert.spec-match] — shared rejection/error
  // validator used by both assert.rejects (asserts the rejection matches)
  // and assert.doesNotReject (asserts the rejection does NOT match).
  // Returns a sentinel:
  //   '__MATCH__'  — rejection matches the spec (assert.rejects passes,
  //                  assert.doesNotReject fails)
  //   '__NOMATCH__' — rejection does NOT match the spec (assert.rejects
  //                   fails with AssertionError, assert.doesNotReject
  //                   rethrows the original rejection)
  //   '__NOSPEC__' — spec is null/undefined; no matching performed
  // Throws AssertionError only for validator functions that themselves throw
  // a non-AssertionError (treated as "does not match" per Node.js semantics).
  function _matchSpec(rejection, spec) {
    if (spec === undefined || spec === null) return '__NOSPEC__';
    if (typeof spec === 'function') {
      try {
        if (spec.prototype !== undefined) {
          return (rejection instanceof spec) ? '__MATCH__' : '__NOMATCH__';
        }
        return spec(rejection) ? '__MATCH__' : '__NOMATCH__';
      } catch (ve) {
        if (ve && ve.name === 'AssertionError') throw ve;
        return '__NOMATCH__';
      }
    }
    if (spec instanceof RegExp) {
      var msg = (rejection && rejection.message) ? String(rejection.message) : String(rejection);
      return spec.test(msg) ? '__MATCH__' : '__NOMATCH__';
    }
    if (typeof spec === 'string') {
      var msg2 = (rejection && rejection.message) ? String(rejection.message) : String(rejection);
      return (msg2.indexOf(spec) !== -1) ? '__MATCH__' : '__NOMATCH__';
    }
    if (typeof spec === 'object') {
      for (var k in spec) {
        if (k === 'message' && spec[k] instanceof RegExp) {
          var m = (rejection && rejection.message) ? String(rejection.message) : '';
          if (!spec[k].test(m)) return '__NOMATCH__';
        } else {
          var actual = rejection ? rejection[k] : undefined;
          if (actual !== spec[k]) return '__NOMATCH__';
        }
      }
      return '__MATCH__';
    }
    return '__NOMATCH__';
  }

  // Normalise asyncFn into a thenable; coerce non-thenable results to a
  // resolved Promise so both assert.rejects and assert.doesNotReject share
  // the same call site.
  function _toThenable(fn) {
    var ret = (typeof fn === 'function') ? fn() : fn;
    if (ret && typeof ret.then === 'function') return ret;
    return Promise.resolve(ret);
  }

  // @trace REQ-ENG-006 [api:assert.rejects] — async-aware rejection assertion.
  // Returns a Promise that resolves when asyncFn rejects with a matching
  // error, or rejects with AssertionError if asyncFn resolves / rejects with
  // the wrong error.
  function rejects(asyncFn, error) {
    var spec = error;
    return Promise.resolve().then(function() {
      return _toThenable(asyncFn).then(
        function() {
          var err = new Error('Missing expected rejection to assert.rejects');
          err.name = 'AssertionError'; throw err;
        },
        function(rejection) {
          var verdict = _matchSpec(rejection, spec);
          if (verdict === '__NOMATCH__') {
            var e = new Error('Missing expected rejection (' + (spec && spec.name) + ') to assert.rejects');
            e.name = 'AssertionError'; throw e;
          }
          // __MATCH__ or __NOSPEC__ → success
        }
      );
    });
  }

  // @trace REQ-ENG-006 [api:assert.doesNotReject] — async-aware inverse of
  // assert.rejects. Awaits asyncFn and:
  //   - resolves (no-op) when asyncFn resolves
  //   - rethrows the original rejection when it does NOT match `error`
  //     (per Node.js: "If the error does not match the expected error, the
  //     caught error is re-thrown")
  //   - rejects with AssertionError ("Got unwanted rejection") when the
  //     rejection DOES match `error`
  function doesNotReject(asyncFn, error) {
    var spec = error;
    return Promise.resolve().then(function() {
      return _toThenable(asyncFn).then(
        function() {
          // resolved — no rejection, assertion passes
          return;
        },
        function(rejection) {
          var verdict = _matchSpec(rejection, spec);
          if (verdict === '__MATCH__') {
            var e = new Error('Got unwanted rejection' + (rejection && rejection.message ? ': ' + rejection.message : ''));
            e.name = 'AssertionError';
            e.actual = rejection;
            e.expected = spec;
            e.operator = 'doesNotReject';
            throw e;
          }
          // __NOMATCH__ → rethrow original rejection (Node.js semantics)
          if (verdict === '__NOMATCH__') throw rejection;
          // __NOSPEC__ → spec was null/undefined, ANY rejection is unwanted
          var e2 = new Error('Got unwanted rejection' + (rejection && rejection.message ? ': ' + rejection.message : ''));
          e2.name = 'AssertionError';
          e2.actual = rejection;
          e2.operator = 'doesNotReject';
          throw e2;
        }
      );
    });
  }

  function match(value, regex, message) {
    if (typeof regex === 'string') regex = new RegExp(regex);
    if (!regex.test(value)) {
      throw _err(message || "Value did not match pattern", value, regex, "match", !message);
    }
  }

  function doesNotMatch(value, regex, message) {
    if (typeof regex === 'string') regex = new RegExp(regex);
    if (regex.test(value)) {
      throw _err(message || "Value unexpectedly matched pattern", value, regex, "doesNotMatch", !message);
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
              doesNotReject: doesNotReject,
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
    doesNotReject: doesNotReject,
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
        mozjs::glue::DeleteCompileOptions(opts);
        if !ok || !rval.is_object() {
            return;
        }
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
        rooted!(&in(wrapped_cx) let assert_fn_obj = rval.to_object());

        // Cache as builtin `assert` and `assert/strict`.
        cache_builtin(cx, "assert", assert_fn_obj.get());
        cache_builtin(cx, "assert/strict", assert_fn_obj.get());

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
    rooted!(&in(cx_ref) let val_root = val);
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
unsafe fn object_has_null_proto(cx: *mut JSContext, obj: mozjs::rust::Handle<*mut JSObject>) -> bool {
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
            parts.push(jsval_inspect(cx, value, inner_depth));
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
            jsval_inspect(cx, k, inner_depth),
            jsval_inspect(cx, v, inner_depth)
        ));
    }
    if parts.is_empty() {
        return format!("{}({}) {{}}", head, n);
    }
    format!("{}({}) {{ {} }}", head, n, parts.join(", "))
}

/// Recursively format a JSVal the way Node.js `util.inspect` would, with a
/// depth cap to avoid infinite recursion on cyclic structures.
///
/// @trace REQ-ENG-004 [algorithm:util_inspect]
unsafe fn jsval_inspect(cx: *mut JSContext, val: JSVal, depth: u32) -> String {
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
            return val.to_double().to_string();
        }
        if val.is_string() {
            return format!("'{}'", crate::js_to_rust_string(cx, val));
        }
        if val.is_symbol() {
            // `ToString(symbol)` throws (abstract op); the node form is
            // `Symbol(description)`, read through the public symbol API.
            let sym = val.to_symbol();
            let desc = w2::GetSymbolDescription(mozjs::rust::Handle::from_marked_location(&sym));
            if desc.is_null() {
                return "Symbol()".to_string();
            }
            return format!("Symbol({})", crate::js_to_rust_string(cx, StringValue(&*desc)));
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
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let obj = val.to_object());

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
                    parts.push(jsval_inspect(cx, elem, depth - 1));
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
                            parts.push(format!("{}: {}", idx, jsval_inspect(cx, v, depth - 1)));
                            continue;
                        }
                        if !jsid.is_string() {
                            continue;
                        }
                        let key_str_ptr = jsid.to_string();
                        if key_str_ptr.is_null() {
                            continue;
                        }
                        let key = jsstr_to_string(&mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)), NonNull::new_unchecked(key_str_ptr));
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
                        parts.push(format!("{}: {}", key, jsval_inspect(cx, v, depth - 1)));
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
                return inspect_map_or_set(cx, obj.handle(), cls == ESClass::Map, depth);
            }

            if matches!(cls, ESClass::Date | ESClass::RegExp | ESClass::Error | ESClass::Promise) {
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
                        let tname = if name.is_empty() { "TypedArray".to_string() } else { name };
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
                        parts.push(format!("{}: {}", idx, jsval_inspect(cx, v, depth - 1)));
                        continue;
                    }
                    if !jsid.is_string() {
                        continue;
                    }
                    let key_str_ptr = jsid.to_string();
                    if key_str_ptr.is_null() {
                        continue;
                    }
                    let key = jsstr_to_string(&mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)), NonNull::new_unchecked(key_str_ptr));
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
                    parts.push(format!("{}: {}", key, jsval_inspect(cx, v, depth - 1)));
                }
            }
            if parts.is_empty() {
                return format!("{}{{}}", prefix);
            }
            return format!("{}{{ {} }}", prefix, parts.join(", "));
        }
        String::new()
    }
}

/// Identify a Buffer instance and return its byte view `(len, data)`.
///
/// Buffer instances are real Uint8Arrays with `Buffer.prototype` rebound plus
/// a `_isBuffer` own marker (globals.rs `create_buffer_object`) — the exact
/// detection `Buffer.isBuffer` performs. Returns `None` for non-Buffers and
/// for Buffer-shaped objects that are not Uint8Array-backed.
unsafe fn jsval_buffer_bytes(cx: *mut JSContext, val: &JSVal) -> ::std::option::Option<(usize, *mut u8)> {
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
    rooted!(&in(wrapped_cx) let val_root = val);
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
            return val.to_double().to_string();
        }
        if val.is_string() {
            return crate::js_to_rust_string(cx, val);
        }
        if val.is_symbol() {
            let sym = val.to_symbol();
            let desc = w2::GetSymbolDescription(mozjs::rust::Handle::from_marked_location(&sym));
            if desc.is_null() {
                return "Symbol()".to_string();
            }
            return format!("Symbol({})", crate::js_to_rust_string(cx, StringValue(&*desc)));
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
            // with their contents rather than the bare "[Object]" tag.
            return jsval_inspect(cx, val, 2);
        }
        String::new()
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
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
    // non-object throws ERR_INVALID_ARG_TYPE. Supports `depth` (the
    // jsval_inspect formatter has no colors path). Root call carries one
    // extra level of budget — `depth: N` caps NESTED levels, so
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
            let mut wrapped_cx =
                mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let oobj = o.to_object());
            let c_key = ZBox::from_bytes(b"depth");
            let mut dv = UndefinedValue();
            let got = JS_GetProperty(
                cx,
                oobj.handle().into(),
                c_key.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut dv,
                },
            );
            if !got {
                JS_ClearPendingException(cx);
            } else if dv.is_number() {
                let d = dv.to_number();
                depth = if d.is_infinite() { u32::MAX } else { d.max(0.0) as u32 };
            }
        }
    }
    let result = jsval_inspect(cx, val, depth.saturating_add(1));
    let utf16: Vec<u16> = result.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
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
    rooted!(&in(cx_ref) let val_root = val);
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
                                    result.push_str(&jsval_inspect(cx, v, 2));
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
                                    result.push_str(&jsval_inspect(cx, v, 2));
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

// @trace REQ-ENG-006 [api:util.callbackify] — real Node semantics. The
// previous implementation returned the argument unchanged, so the callback
// never ran (silent fake). Mirrors Node lib/util.js:
//   - last argument must be a function (else TypeError ERR_INVALID_CALLBACK)
//   - uses `fn[Symbol.for('nodejs.callbackify.custom')]` when present
//   - resolved value: undefined → cb(null); Array → cb(null, ...arr);
//     anything else → cb(null, value)
//   - rejection: falsy reason is wrapped in an Error carrying `.cause`
//   - a synchronous throw of the wrapped fn is delivered to the callback on a
//     microtask (Node defers via process.nextTick)
//
// CS-EMPTY-CATCH exemption: the empty-catch around `reason.cause = err` inside
// the rejection handler is best-effort cause attachment — a frozen/non-
// extensible `reason` object cannot take `.cause`, and Node's callbackify does
// not surface that as an error (the original `reason` is still delivered to the
// callback).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_callbackify(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    if _argc == 0 || !(*args.get(0).ptr).is_object() {
        JS_ReportErrorUTF8(cx, c"callbackify requires a function".as_ptr());
        return false;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let fn_val = *args.get(0).ptr);

    let callbackify_src = r#"(function(orig) {
  var custom = orig[Symbol.for('nodejs.callbackify.custom')];
  var impl = (typeof custom === 'function') ? function() { return custom.apply(this, arguments); } : orig;
  function callbackified() {
    var args = Array.prototype.slice.call(arguments);
    var maybeCb = args.pop();
    if (typeof maybeCb !== 'function') {
      throw new TypeError('The last argument passed to callbackify must be a callback function');
    }
    var cb = maybeCb;
    var ret;
    try {
      ret = impl.apply(this, args);
    } catch (err) {
      // Node defers the synchronous failure to process.nextTick; a microtask
      // is the same "after the current call" delivery without nextTick.
      Promise.resolve().then(function() { cb(err); });
      return;
    }
    Promise.resolve(ret).then(function(value) {
      if (value === undefined) cb(null);
      else if (Array.isArray(value)) cb.apply(null, [null].concat(value));
      else cb(null, value);
    }, function(err) {
      var reason = err;
      if (!reason) {
        reason = new Error('falsy rejection reason');
        if (reason && !reason.cause) { try { reason.cause = err; } catch (e) {/*CS-EMPTY-CATCH exempt: frozen reason cannot take .cause; original reason still delivered*/} }
      }
      cb(reason);
    });
  }
  callbackified[Symbol.for('nodejs.callbackify.custom')] = orig[Symbol.for('nodejs.callbackify.custom')];
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

/// JavaScript number rendering for error messages (`Received 1.5`, `NaN`).
fn format_js_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n.is_sign_positive() { "Infinity" } else { "-Infinity" }.to_string();
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
    let val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
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
                crate::bun_inspect_api::received_type_fragment(cx, val)
                    .trim_start_matches("type ")
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

binary_display_assert!(assert_equal, |a, b| a != b, |a, b| format!("{} == {}", a, b));
binary_display_assert!(assert_not_equal, |a, b| a == b, |a, b| format!("{} != {}", a, b));
binary_display_assert!(
    assert_deep_equal,
    |a, b| a != b,
    |_a, _b| "Expected values to be deeply equal".to_string()
);

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
/// e201 REQ-ARCH-1: extracted from `install_util` (control flow unchanged;
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
    let wrapped_cx = unsafe {
        mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()))
    };
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
