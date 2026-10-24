// @trace TEST-ENG-7-UTIL-EDGE [req:REQ-ENG-007] [level:integration]
//
// C8b line-coverage wave: node_util.rs uncovered branches — util.inspect
// (Buffer/array-props/function/depth-cap/options-error arms), util.format
// (%j/%o/%O/i/f/%%/starvation arms), util.styleText (table/array/error arms),
// util.types extended-checker matrix, util.promisify / util.callbackify
// (every branch incl. custom-symbol + error arms), util.inherits, and the
// assert IIFE spec forms (throws spec classes, rejects/doesNotReject spec
// matrix, match/doesNotMatch, AssertionError property shape, strict variant,
// CallTracker).

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full;

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Run `body` and return the joined per-check results; every label must
/// report PASS for the caller's assert to hold.
fn run_checks(ctx: &mut JsContext, body: &str) -> String {
    let wrapped = format!(
        r#"
        var __results = [];
        function check(label, fn) {{
            try {{ var ok = fn(); __results.push(label + (ok ? " PASS" : " FAIL")); }}
            catch(e) {{ __results.push(label + " ERR:" + (e.message || e).toString().substring(0, 90)); }}
        }}
        {body}
        __results.join("\n");
        "#
    );
    eval_string_full(ctx, &wrapped)
}

fn assert_all_pass(results: &str) {
    assert!(
        !results.contains("FAIL") && !results.contains("ERR:"),
        "util edge checks failed:\n{}",
        results
    );
}

#[test]
fn util_inspect_shape_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // Buffer form: '<Buffer 68 69>' — byte view short-circuit before the
        // plain-object path.
        check("inspect_buffer_hex", function() {
            return util.inspect(Buffer.from([0x68, 0x69])) === '<Buffer 68 69>';
        });
        check("inspect_buffer_empty_trailing_space", function() {
            return util.inspect(Buffer.alloc(0)) === '<Buffer >';
        });
        check("inspect_buffer_overflow_marker", function() {
            var big = Buffer.alloc(600, 0x41);
            var s = util.inspect(big);
            return s.indexOf('<Buffer ') === 0 && s.indexOf('... 88 more bytes') !== -1;
        });

        // Arrays: elements first, then non-index own props, then integer
        // indices >= length rendered as 'N: value' keys.
        check("inspect_array_extra_props", function() {
            var a = [1, 2];
            a.foo = 'x';
            return util.inspect(a) === "[ 1, 2, foo: 'x' ]";
        });
        check("inspect_plain_object_int_keys_prefixed", function() {
            // A non-array with integer keys renders them bare-ascending and
            // prefixed (plain-object int-jsid arm). The array-branch
            // `idx >= length` twin arm is defensive-only: an index write
            // always extends `length`, so the key loop skips every own index
            // of a real array (verified: [1,2] + a[5]='y' renders as
            // "[ 1, 2, undefined, undefined, undefined, 'y' ]" via the
            // elements loop, length 6).
            var o = {length: 2, 0: 'a', 5: 'b'};
            var s = util.inspect(o);
            if (s !== "{ 0: 'a', 5: 'b', length: 2 }" && s !== "{ length: 2, 0: 'a', 5: 'b' }") {
                throw new Error('GOT:' + s);
            }
            return true;
        });
        check("inspect_array_strings_quoted", function() {
            return util.inspect(['ab']) === "[ 'ab' ]";
        });
        check("inspect_array_empty", function() {
            return util.inspect([]) === '[]';
        });

        // Functions: [Function: name] / [Function (anonymous)].
        check("inspect_function_named", function() {
            function myFn() {}
            return util.inspect(myFn) === '[Function: myFn]';
        });
        check("inspect_function_anonymous", function() {
            return util.inspect(function() {}) === '[Function (anonymous)]';
        });

        // Depth caps: root call carries depth+1 budget, so depth:0 still
        // expands the root and caps the FIRST nested level.
        check("inspect_depth0_object_cap", function() {
            return util.inspect({a: {b: 1}}, {depth: 0}) === '{ a: [Object] }';
        });
        check("inspect_depth0_array_cap", function() {
            return util.inspect({a: [1]}, {depth: 0}) === '{ a: [Array] }';
        });
        check("inspect_depth_default_expands_two_levels", function() {
            // default depth=2: root + 2 nested levels expand, third caps.
            return util.inspect({a: {b: {c: {d: 1}}}}) === '{ a: { b: { c: [Object] } } }';
        });
        check("inspect_depth_infinity_expands_all", function() {
            var s = util.inspect({a: {b: {c: {d: 1}}}}, {depth: Infinity});
            return s === '{ a: { b: { c: { d: 1 } } } }';
        });
        check("inspect_depth_negative_clamps_to_zero", function() {
            return util.inspect({a: {b: 1}}, {depth: -5}) === '{ a: [Object] }';
        });
        check("inspect_circular_object_depth_capped", function() {
            var o = {name: 'root'};
            o.self = o;
            // node v24.19.0 oracle (2026-10-23, local binary): self-cycles
            // fold at EVERY depth — "<ref *1> { name: 'root', self:
            // [Circular *1] }" — the R7 ancestor-seen folding IS node
            // semantics. The former pin (re-expansion until the depth cap
            // renders '[Object]') predates the R7 root fix.
            return util.inspect(o) === "<ref *1> { name: 'root', self: [Circular *1] }";
        });

        // Primitives: strings quoted, others bare.
        check("inspect_string_quoted", function() {
            return util.inspect('hi') === "'hi'";
        });
        check("inspect_null_undefined_bool_num", function() {
            return util.inspect(null) === 'null'
                && util.inspect(undefined) === 'undefined'
                && util.inspect(true) === 'true'
                && util.inspect(7) === '7'
                && util.inspect(2.5) === '2.5';
        });
        check("inspect_plain_object_form", function() {
            return util.inspect({a: 1, b: 'two'}) === "{ a: 1, b: 'two' }";
        });
        check("inspect_empty_object", function() {
            return util.inspect({}) === '{}';
        });
        "#,
    );
    assert_all_pass(&results);
}

#[test]
fn util_inspect_options_validation() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');
        function errCode(fn) {
            try { fn(); return null; }
            catch (e) { return e; }
        }

        // Non-object options → ERR_INVALID_ARG_TYPE TypeError (Node contract).
        check("inspect_options_number_throws", function() {
            var e = errCode(function() { util.inspect(1, 5); });
            return e instanceof TypeError && e.code === 'ERR_INVALID_ARG_TYPE'
                && e.message.indexOf('options') !== -1 && e.message.indexOf('number') !== -1;
        });
        check("inspect_options_bool_throws", function() {
            var e = errCode(function() { util.inspect(1, true); });
            return e !== null && e.code === 'ERR_INVALID_ARG_TYPE'
                && e.message.indexOf('boolean') !== -1;
        });
        // null/undefined options are ignored (Node semantics).
        check("inspect_options_null_ignored", function() {
            return util.inspect({a: 1}, null) === '{ a: 1 }'
                && util.inspect({a: 1}, undefined) === '{ a: 1 }';
        });
        check("inspect_no_options_depth_default", function() {
            return util.inspect({a: {b: 1}}) === '{ a: { b: 1 } }';
        });
        "#,
    );
    assert_all_pass(&results);
}

#[test]
fn util_format_specifier_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // %s / %d / %f / %i via the display path.
        check("format_s_string", function() {
            return util.format('%s!', 'hi') === 'hi!';
        });
        check("format_s_object_delegates_to_inspect", function() {
            return util.format('%s', {a: 1}) === '{ a: 1 }';
        });
        check("format_d_number", function() {
            return util.format('%d + %d = %d', 1, 2, 3) === '1 + 2 = 3';
        });
        check("format_i_and_f", function() {
            return util.format('%i %f', 42, 1.5) === '42 1.5';
        });

        // %j = JSON.stringify semantics, not inspect.
        check("format_j_object", function() {
            return util.format('%j', {a: 1}) === '{"a":1}';
        });
        check("format_j_undefined_value", function() {
            return util.format('%j', undefined) === 'undefined';
        });
        check("format_j_function_value", function() {
            return util.format('%j', function f() {}) === 'undefined';
        });
        check("format_j_null_value", function() {
            return util.format('%j', null) === 'null';
        });
        check("format_j_circular_is_marker", function() {
            var o = {};
            o.self = o;
            return util.format('%j', o) === '[Circular]';
        });

        // %o / %O route to the inspect formatter.
        check("format_o_inspect_form", function() {
            return util.format('%o', {x: 1}) === '{ x: 1 }';
        });
        check("format_big_o_inspect_form", function() {
            return util.format('%O', [1]) === '[ 1 ]';
        });

        // %% escape consumes both percent signs; the %-format path returns
        // the rendered format string WITHOUT appending unused args.
        check("format_percent_escape", function() {
            return util.format('%%s', 1) === '%s';
        });

        // Placeholder starvation: an exhausted specifier is consumed and
        // renders nothing (trailing space from the literal stays).
        check("format_starved_specifier", function() {
            return util.format('%s %s', 'a') === 'a ';
        });
        // Specifier with no following arg at all: falls to the plain-join
        // path, which displays the raw string.
        check("format_dangling_specifier", function() {
            return util.format('%s') === '%s';
        });
        // Extra args beyond the format string are dropped on the %-path
        // (node appends them — divergence registered in the C8b report).
        check("format_extra_args_dropped_on_format_path", function() {
            return util.format('%s', 'a', 'b', 3) === 'a';
        });
        // Non-format first arg: everything displayed space-joined.
        check("format_plain_join", function() {
            return util.format(1, 'x', null) === '1 x null';
        });
        check("format_no_args_empty", function() {
            return util.format() === '';
        });
        // Literal string without % passes through even with args.
        check("format_literal_with_args", function() {
            return util.format('hello', 'world') === 'hello world';
        });
        // Unknown specifier: '%' + char pass through; the next recognized
        // specifier consumes the FIRST vararg (arg_idx starts at the first
        // variadic slot).
        check("format_unknown_specifier_passthrough", function() {
            var s = util.format('%q %s', 1, 2);
            if (s !== '%q 1') throw new Error('GOT:' + s);
            return true;
        });
        "#,
    );
    assert_all_pass(&results);
}

#[test]
fn util_styletext_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // Forced-ANSI option: node's --color=auto suppresses color on a pipe
        // (styleText('red','x') returns the bare "x" when stdout is not a TTY),
        // so the old indexOf-based pins were environment-dependent and failed
        // under a pipe. `{validateStream:false}` is node's own force-color
        // escape hatch — it skips the stream colorization check entirely, so
        // the assertion holds in both pipe and TTY. node v24.19.0 oracle:
        //   styleText('red','x',{validateStream:false}) === "\u001b[31mx\u001b[39m"
        check("styletext_single_format", function() {
            var s = util.styleText('red', 'x', {validateStream: false});
            return s === '\u001b[31mx\u001b[39m';
        });
        // bold opens first, red second; closes unwound (39 before 22).
        // node v24.19.0 oracle: "\u001b[1m\u001b[31mx\u001b[39m\u001b[22m"
        check("styletext_array_format_orders_wraps", function() {
            var s = util.styleText(['bold', 'red'], 'x', {validateStream: false});
            return s === '\u001b[1m\u001b[31mx\u001b[39m\u001b[22m';
        });
        check("styletext_unknown_format_throws", function() {
            try { util.styleText('nope', 'x'); return false; }
            catch (e) {
                return e instanceof TypeError
                    && e.message.indexOf('format') !== -1;
            }
        });
        check("styletext_array_unknown_entry_throws", function() {
            try { util.styleText(['red', 'nope'], 'x'); return false; }
            catch (e) { return e instanceof TypeError; }
        });
        check("styletext_non_string_text_throws", function() {
            try { util.styleText('red', 5); return false; }
            catch (e) {
                return e instanceof TypeError
                    && e.message.indexOf('text') !== -1;
            }
        });
        "#,
    );
    assert_all_pass(&results);
}

#[test]
fn util_types_extended_matrix() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');
        var t = util.types;

        check("types_promise", function() {
            return t.isPromise(new Promise(function() {})) === true
                && t.isPromise({}) === false;
        });
        check("types_map_set", function() {
            return t.isMap(new Map()) && !t.isMap(new Set())
                && t.isSet(new Set()) && !t.isSet(new Map());
        });
        check("types_weak", function() {
            return t.isWeakMap(new WeakMap()) && t.isWeakSet(new WeakSet())
                && !t.isWeakMap({}) && !t.isWeakSet({});
        });
        check("types_iterators", function() {
            return t.isMapIterator(new Map().entries())
                && t.isSetIterator(new Set().values())
                && !t.isMapIterator(new Map())
                && !t.isSetIterator([]);
        });
        check("types_arraybuffer_family", function() {
            return t.isArrayBuffer(new ArrayBuffer(1))
                && t.isAnyArrayBuffer(new ArrayBuffer(1))
                && t.isArrayBufferView(new Uint8Array(1))
                && t.isArrayBufferView(new DataView(new ArrayBuffer(1)))
                && !t.isDataView(new Uint8Array(1))
                && t.isDataView(new DataView(new ArrayBuffer(1)));
        });
        check("types_typed_array_matrix", function() {
            return t.isInt8Array(new Int8Array(1)) && t.isUint8Array(new Uint8Array(1))
                && t.isUint8ClampedArray(new Uint8ClampedArray(1))
                && t.isInt16Array(new Int16Array(1)) && t.isUint16Array(new Uint16Array(1))
                && t.isInt32Array(new Int32Array(1)) && t.isUint32Array(new Uint32Array(1))
                && t.isFloat32Array(new Float32Array(1)) && t.isFloat64Array(new Float64Array(1))
                && t.isTypedArray(new Uint8Array(1)) && !t.isTypedArray(new DataView(new ArrayBuffer(1)))
                && !t.isInt32Array([]);
        });
        check("types_bigint_arrays", function() {
            return t.isBigInt64Array(new BigInt64Array(1))
                && t.isBigUint64Array(new BigUint64Array(1));
        });
        check("types_boxed_primitives", function() {
            return t.isBooleanObject(new Boolean(true)) && t.isNumberObject(new Number(5))
                && t.isStringObject(new String('x')) && t.isSymbolObject(Object(Symbol('s')))
                && t.isBoxedPrimitive(new Number(1)) && !t.isBoxedPrimitive(1)
                && !t.isBooleanObject(true) && !t.isStringObject('x');
        });
        check("types_native_error", function() {
            return t.isNativeError(new Error('e')) && !t.isNativeError('Error');
        });
        check("types_async_generator_functions", function() {
            return t.isAsyncFunction(async function() {})
                && !t.isAsyncFunction(function() {})
                && t.isGeneratorFunction(function*() {})
                && !t.isGeneratorFunction(function() {})
                && t.isGeneratorObject((function*() {})());
        });
        check("types_arguments_object", function() {
            function f() { return t.isArgumentsObject(arguments); }
            return f(1, 2) === true && t.isArgumentsObject([1]) === false;
        });
        check("types_stub_faces_false", function() {
            return t.isExternal() === false && t.isKeyObject() === false
                && t.isCryptoKey() === false;
        });
        check("types_shared_array_buffer", function() {
            if (typeof SharedArrayBuffer === 'undefined') return true;
            return t.isSharedArrayBuffer(new SharedArrayBuffer(1))
                && t.isAnyArrayBuffer(new SharedArrayBuffer(1));
        });
        check("types_basic_negatives", function() {
            return !t.isPromise(1) && !t.isMap('m') && !t.isSet(null)
                && !t.isArrayBufferView({length: 1}) && !t.isTypedArray([1]);
        });
        "#,
    );
    assert_all_pass(&results);
}

#[test]
fn util_promisify_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // (Success path resolution is asserted via the __presult side-channel
        // after microtask pumping in the Rust half of this test.)
        // Error path: truthy err → reject(err).
        check("promisify_rejects_err", function() {
            function boom(cb) { cb(new Error('boom')); }
            return util.promisify(boom)().then(
                function() { return false; },
                function(e) { return e.message === 'boom'; }
            );
        });
        // Custom promisify symbol returns the custom function verbatim.
        check("promisify_custom_symbol", function() {
            var custom = function() { return 42; };
            function orig(cb) { cb(null, 1); }
            orig[Symbol.for('nodejs.util.promisify.custom')] = custom;
            return util.promisify(orig) === custom;
        });
        // The returned callable exposes a working .then forwarder.
        check("promisify_then_forwarder", function() {
            function cbStyle(cb) { cb(null, 'v'); }
            var p = util.promisify(cbStyle);
            return typeof p.then === 'function'
                && p.then(function(v) { return v === 'v'; });
        });
        // Non-function argument throws.
        check("promisify_non_function_throws", function() {
            try { util.promisify(5); return false; }
            catch (e) { return String(e.message).indexOf('promisify') !== -1; }
        });
        "#,
    );
    assert_all_pass(&results);

    // Success-path resolution observed via a side-channel global after
    // microtask pumping (a promise returned from eval would only stringify
    // as "[object]").
    eval_string_full(
        &mut ctx,
        r#"
        var __presult = 'unset';
        (function() {
            var util = require('util');
            function cbStyle(x, cb) { cb(null, x * 2); }
            util.promisify(cbStyle)(21).then(function(v) { __presult = 'V=' + v; });
        })();
        __presult
        "#,
    );
    for _ in 0..20 {
        let _ = eval_string_full(&mut ctx, "1");
        common::drive_event_loop(&mut ctx, 2);
    }
    let final_value = eval_string_full(&mut ctx, "__presult");
    assert_eq!(final_value, "V=42", "promisify resolve path");
}

#[test]
fn util_callbackify_arms() {
    let mut ctx = make_ctx();
    // callbackify delivers through callbacks; observe via a results global
    // after driving microtasks.
    eval_string_full(
        &mut ctx,
        r#"
        var __cbResults = [];
        var util = require('util');

        // undefined resolve → cb(null)
        var f1 = util.callbackify(async function() { });
        f1(function(err, v) { __cbResults.push('undef:' + err + ',' + v); });

        // value resolve → cb(null, value)
        var f2 = util.callbackify(async function() { return 7; });
        f2(function(err, v) { __cbResults.push('val:' + err + ',' + v); });

        // array resolve → cb(null, ...arr)
        var f3 = util.callbackify(async function() { return ['a', 'b']; });
        f3(function(err, x, y) { __cbResults.push('arr:' + err + ',' + x + ',' + y); });

        // rejection → cb(reason)
        var f4 = util.callbackify(async function() { throw new Error('rej'); });
        f4(function(err) { __cbResults.push('rej:' + (err && err.message)); });

        // falsy rejection reason → wrapped Error carrying .cause
        var f5 = util.callbackify(async function() { return Promise.reject(null); });
        f5(function(err) {
            __cbResults.push('falsy:' + (err instanceof Error) + ',' + err.message + ',' + (err.cause === null));
        });

        // synchronous throw of the wrapped fn → node v24 oracle: the error
        // propagates SYNCHRONOUSLY out of the callbackified call (not
        // deferred into cb). Catch it here so the nofn probe still runs.
        var f6 = util.callbackify(function() { throw new Error('sync-throw'); });
        try {
            f6(function(err) { __cbResults.push('sync:deferred:' + (err && err.message)); });
            __cbResults.push('sync:deferred-unreached');
        }
        catch (e) {
            __cbResults.push('sync:sync-throw:' + (e instanceof Error) + ',' + e.message);
        }

        // non-function last argument → TypeError thrown synchronously
        try { f6(42); __cbResults.push('nofn:FAIL'); }
        catch (e) { __cbResults.push('nofn:' + (e instanceof TypeError)); }
        "#,
    );
    for _ in 0..20 {
        common::drive_event_loop(&mut ctx, 2);
    }
    let results = eval_string_full(&mut ctx, "__cbResults.join('|')");
    // node v24 oracle (verbatim): array resolve does NOT spread into extra
    // cb params (x=["a","b"] joins to "a,b", y=undefined); falsy rejection
    // message is "Promise was rejected with falsy value" with .cause left
    // undefined (cause === null is false); sync throw of the wrapped fn
    // propagates synchronously as the original Error.
    for expected in [
        "undef:null,undefined",
        "val:null,7",
        "arr:null,a,b,undefined",
        "rej:rej",
        "falsy:true,Promise was rejected with falsy value,false",
        "sync:sync-throw:true,sync-throw",
        "nofn:true",
    ] {
        assert!(
            results.contains(expected),
            "callbackify expected fragment `{}` in: {}",
            expected,
            results
        );
    }
}

#[test]
fn util_misc_surface_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // inherits: Child.super_ = Parent.
        check("inherits_sets_super", function() {
            function Parent() {}
            function Child() {}
            util.inherits(Child, Parent);
            return Child.super_ === Parent;
        });
        // Non-object / non-function args THROW a TypeError — the old pin name
        // "noop" (and its === undefined assertions) was wrong: node never
        // returns undefined here, it validates each arg. node v24.19.0 oracle:
        //   inherits(5, {})  -> TypeError: The "superCtor.prototype" property
        //                       must be of type object. Received undefined
        //   inherits({})     -> TypeError: The "superCtor" argument must be
        //                       of type function. Received undefined
        // Only the 'superCtor' key fragment is asserted (both messages carry it)
        // rather than the whole string, which differs per arg.
        check("inherits_non_object_args_throw", function() {
            try { util.inherits(5, {}); return false; }
            catch (e) {
                return e instanceof TypeError && e.message.indexOf('superCtor') !== -1;
            }
        });
        // getSystemErrorName: node v24 validates the code — a positive (or
        // zero) value is a RangeError ERR_OUT_OF_RANGE ("must be a negative
        // integer"), not undefined (the old stub face this pin used to
        // assert; flipped in the C17 fix wave).
        check("getSystemErrorName_rejects_non_negative", function() {
            try { util.getSystemErrorName(1); return false; }
            catch (e) {
                return e.name === 'RangeError' && e.code === 'ERR_OUT_OF_RANGE';
            }
        });
        // parseArgs returns an object (values shape is the stub contract).
        check("parseArgs_returns_object", function() {
            return util.parseArgs([]) !== null && typeof util.parseArgs([]) === 'object';
        });
        // deprecate passes the fn through.
        check("deprecate_passthrough", function() {
            function f() { return 9; }
            return util.deprecate(f, 'gone soon')() === 9;
        });
        // isDeepStrictEqual JS implementation over objects/arrays/dates/regex.
        check("isDeepStrictEqual_shapes", function() {
            return util.isDeepStrictEqual({a: [1, 2]}, {a: [1, 2]}) === true
                && util.isDeepStrictEqual({a: 1}, {a: 2}) === false
                && util.isDeepStrictEqual([1], {0: 1}) === false
                && util.isDeepStrictEqual(new Date(5), new Date(5)) === true
                && util.isDeepStrictEqual(/ab/g, /ab/g) === true
                && util.isDeepStrictEqual(1, '1') === false
                && util.isDeepStrictEqual(null, null) === true
                && util.isDeepStrictEqual(null, undefined) === false;
        });
        // Native isXxx family on the module root.
        check("is_family_matrix", function() {
            return util.isBoolean(true) && !util.isBoolean('true')
                && util.isNumber(3) && !util.isNumber('3')
                && util.isString('s') && !util.isString(1)
                && util.isSymbol(Symbol('x')) && !util.isSymbol('x')
                && util.isUndefined(undefined) && !util.isUndefined(null)
                && util.isNull(null) && !util.isNull(undefined)
                && util.isObject({}) && !util.isObject(1)
                && util.isFunction(function() {}) && !util.isFunction({})
                && util.isArray([]) && !util.isArray({})
                && util.isDate(new Date()) && !util.isDate({})
                && util.isRegExp(/r/) && !util.isRegExp('/')
                && util.isError(new Error()) && !util.isError({name: 'Error'});
        });
        "#,
    );
    assert_all_pass(&results);
}

#[test]
fn assert_spec_forms_matrix() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var assert = require('assert');

        // throws with a class spec: wrong class → AssertionError.
        // node v24.19.0 oracle (2026-10-23, local binary): the generated
        // message is "The error is expected to be an instance of
        // \"RangeError\". Received \"TypeError\"\n\nError message:\n\nt".
        check("throws_wrong_class", function() {
            try { assert.throws(function() { throw new TypeError('t'); }, RangeError); return false; }
            catch (e) {
                return e.name === 'AssertionError'
                    && String(e.message).indexOf('expected to be an instance of') !== -1;
            }
        });
        // throws with a regex spec: message mismatch → AssertionError.
        check("throws_message_regex_mismatch", function() {
            try { assert.throws(function() { throw new Error('alpha'); }, /beta/); return false; }
            catch (e) {
                return e.name === 'AssertionError'
                    && String(e.message).indexOf('did not match') !== -1;
            }
        });
        // throws with no exception at all.
        check("throws_missing_exception", function() {
            try { assert.throws(function() {}); return false; }
            catch (e) {
                return e.name === 'AssertionError'
                    && String(e.message).indexOf('Missing expected exception') !== -1;
            }
        });
        // doesNotThrow re-wraps an escaping exception.
        check("doesNotThrow_unwanted_exception", function() {
            try { assert.doesNotThrow(function() { throw new Error('x'); }); return false; }
            catch (e) {
                return e.name === 'AssertionError'
                    && String(e.message).indexOf('unwanted exception') !== -1;
            }
        });
        // notStrictEqual / strictEqual failure arms.
        check("notStrictEqual_equal_throws", function() {
            try { assert.notStrictEqual(1, 1); return false; }
            catch (e) { return e.name === 'AssertionError'; }
        });
        // node v24.19.0 oracle (2026-10-23, local binary):
        // strictEqual(1, 2) → "Expected values to be strictly equal:\n\n1 !== 2\n"
        // — the generated message joins with the UNEQUAL operator.
        check("strictEqual_diff_message_inline", function() {
            try { assert.strictEqual(1, 2); return false; }
            catch (e) {
                return String(e.message).indexOf('1 !== 2') !== -1;
            }
        });
        // deepStrictEqual object failure renders the diff body.
        check("deepStrictEqual_diff_body", function() {
            try { assert.deepStrictEqual({a: 1}, {a: 2}); return false; }
            catch (e) {
                var m = String(e.message);
                return m.indexOf('+ actual - expected') !== -1 && m.indexOf('a:') !== -1;
            }
        });
        // match / doesNotMatch. node v24.19.0 oracle (2026-10-23, local
        // binary): a string regexp argument is NOT coerced — TypeError
        // ERR_INVALID_ARG_TYPE "The \"regexp\" argument must be an instance
        // of RegExp. Received type string ('b')".
        check("match_ok_and_string_coercion", function() {
            assert.match('abc', /b/);
            try { assert.match('abc', 'b'); return false; }
            catch (e) {
                return e instanceof TypeError && e.code === 'ERR_INVALID_ARG_TYPE';
            }
        });
        // node v24.19.0 oracle (2026-10-23): the generated message reads
        // "The input did not match the regular expression /z/. Input:...".
        check("match_failure_throws", function() {
            try { assert.match('abc', /z/); return false; }
            catch (e) {
                return e.name === 'AssertionError'
                    && String(e.message).indexOf('regular expression') !== -1;
            }
        });
        check("doesNotMatch_failure_throws", function() {
            try { assert.doesNotMatch('abc', /b/); return false; }
            catch (e) { return e.name === 'AssertionError'; }
        });
        // AssertionError property shape.
        check("assertion_error_properties", function() {
            try { assert.equal(1, 2); return false; }
            catch (e) {
                return e instanceof Error
                    && e.actual === 1 && e.expected === 2 && e.operator === '=='
                    && assert.AssertionError.prototype.isPrototypeOf(e);
            }
        });
        // strict namespace maps to strict variants.
        check("strict_namespace_semantics", function() {
            try { assert.strict.equal('1', 1); return false; }
            catch (e) { return e.name === 'AssertionError'; }
        });
        check("strict_namespace_deepEqual", function() {
            try { assert.strict.deepEqual({a: 1}, {a: '1'}); return false; }
            catch (e) { return e.name === 'AssertionError'; }
        });
        // CallTracker registers and verifies without throwing. node v24
        // oracle: calls(fn[, exact]) takes the wrapped FUNCTION (a string
        // arg is a TypeError in both engines — V8: "Cannot create proxy
        // with a non-object as target or handler"); verify() passes when
        // the call count matches.
        check("call_tracker", function() {
            var tracker = new assert.CallTracker();
            var hit = false;
            var fn = tracker.calls(function() { hit = true; }, 1);
            fn();
            tracker.verify();
            return hit && typeof tracker.calls === 'function';
        });
        // ok falsy value message arm. node v24.19.0 (getErrMessage, read
        // from the embedded internal/assert/utils source via
        // --expose-internals): the "The expression evaluated to a falsy
        // value" form requires recovering the CALL-SITE SOURCE EXPRESSION
        // from the V8 stack; when the source is unrecoverable getErrMessage
        // returns undefined and the AssertionError falls back to the
        // operator join — "0 == true". SpiderMonkey frames carry no source
        // text, so the reachable node-parity form on bao is the operator
        // join (node's own no-source fallback).
        check("ok_falsy_message", function() {
            try { assert.ok(0); return false; }
            catch (e) {
                return e.name === 'AssertionError'
                    && String(e.message).indexOf('0 == true') !== -1;
            }
        });
        "#,
    );
    assert_all_pass(&results);
}

#[test]
fn assert_async_rejects_matrix() {
    let mut ctx = make_ctx();
    eval_string_full(
        &mut ctx,
        r#"
        var __rej = [];
        var assert = require('assert');

        function track(label) { return function(v) { __rej.push(label + ':' + v); }; }
        function trackErr(label) {
            return function(e) { __rej.push(label + ':' + (e.name || 'err') + ':' + String(e.message || e).substring(0, 30)); };
        }

        // 1. rejects with no spec — any rejection passes.
        assert.rejects(function() { return Promise.reject(new Error('any')); })
            .then(track('nospec:true'), trackErr('nospec'));

        // 2. rejects with class spec — match.
        assert.rejects(function() { return Promise.reject(new TypeError('ty')); }, TypeError)
            .then(track('class:true'), trackErr('class'));

        // 3. rejects with regex spec — match on message.
        assert.rejects(function() { return Promise.reject(new Error('needle-here')); }, /needle/)
            .then(track('regex:true'), trackErr('regex'));

        // 4. rejects with string spec identical to the rejection message —
        // node v24.19.0 oracle (2026-10-23, local binary): TypeError
        // ERR_INVALID_ARG_TYPE "The \"error/message\" argument is ambiguous.
        // The error message \"str-needle\" is identical to the message."
        // (a string spec DIFFERENT from the message is a valid includes-
        // check; the identical string is the classic misuse node rejects).
        assert.rejects(function() { return Promise.reject(new Error('str-needle')); }, 'str-needle')
            .then(track('string:true'), trackErr('string'));

        // 5. rejects with object spec — property equality.
        assert.rejects(function() { return Promise.reject(Object.assign(new Error('o'), {code: 'E1'})); }, {code: 'E1'})
            .then(track('obj:true'), trackErr('obj'));

        // 6. rejects with validator function spec. node v24.19.0 oracle
        // (2026-10-23, local binary): a validator function returning true
        // PASSES the rejection (the class arm requires an Error-prototype
        // discriminator, so a plain function falls to the validator arm).
        // The former C8b-registered divergence (validator read as class →
        // NOMATCH) no longer exists in the implementation.
        assert.rejects(function() { return Promise.reject(new Error('v')); }, function(e) { return e.message === 'v'; })
            .then(track('validator:ok'), trackErr('validator'));

        // 7. rejects mismatch (regex) → AssertionError.
        assert.rejects(function() { return Promise.reject(new Error('aaa')); }, /zzz/)
            .then(track('mismatch:ok'), trackErr('mismatch'));

        // 8. rejects on a resolved promise → AssertionError 'Missing expected rejection'.
        assert.rejects(function() { return Promise.resolve(1); })
            .then(track('resolved:ok'), trackErr('resolved'));

        // 9. doesNotReject resolves — passes.
        assert.doesNotReject(function() { return Promise.resolve(1); })
            .then(track('dnr-resolve:true'), trackErr('dnr-resolve'));

        // 10. doesNotReject with matching rejection → AssertionError 'unwanted'.
        assert.doesNotReject(function() { return Promise.reject(new Error('unwanted')); }, /unwanted/)
            .then(track('dnr-match:ok'), trackErr('dnr-match'));

        // 11. doesNotReject with mismatching rejection → rethrows the original.
        assert.doesNotReject(function() { return Promise.reject(new Error('original')); }, /nope/)
            .then(track('dnr-mismatch:ok'), trackErr('dnr-mismatch'));

        // 12. doesNotReject with no spec — ANY rejection is unwanted.
        assert.doesNotReject(function() { return Promise.reject(new Error('e')); })
            .then(track('dnr-nospec:ok'), trackErr('dnr-nospec'));
        "#,
    );
    for _ in 0..30 {
        common::drive_event_loop(&mut ctx, 2);
    }
    let results = eval_string_full(&mut ctx, "__rej.sort().join('|')");
    for expected in [
        "class:true",
        "dnr-match:AssertionError",
        "dnr-mismatch:Error:original",
        "dnr-nospec:AssertionError",
        "dnr-resolve:true",
        "mismatch:AssertionError",
        "nospec:true",
        "obj:true",
        "regex:true",
        "resolved:AssertionError",
        // node v24.19.0 oracle 2026-10-23: identical-string spec → the
        // ambiguous-argument TypeError, not a match.
        "string:TypeError",
        // node v24.19.0 oracle 2026-10-23: validator fn returning true
        // passes → the resolve path fires (track pushes `validator:ok:<v>`).
        "validator:ok",
    ] {
        assert!(
            results.contains(expected),
            "rejects matrix expected `{}` in: {}",
            expected,
            results
        );
    }
}
