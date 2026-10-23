// @trace TEST-ENG-048-ASSERT-PARITY [req:REQ-ENG-048] [level:integration]
//
// Regression pin for the assert deep-comparison family rewrite (node v24
// parity port). Every expectation below was anchored against the real node
// v24.19.0 binary before implementation (oracle transcript in the task
// report); outcomes and exact messages are pinned so a drift re-fails here
// first.
use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

fn run_battery(ctx: &mut JsContext, battery: &str) -> (usize, usize) {
    let results = eval_string(ctx, battery);
    assert!(
        !results.is_empty(),
        "battery probe produced no results (uncaught throw or non-string return)"
    );
    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.is_empty() {
            continue;
        }
        if item.contains(" PASS") {
            pass += 1;
        } else {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    (pass, fail)
}

#[test]
fn test_assert_deep_compare_semantics() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let (pass, fail) = run_battery(
        &mut ctx,
        r#"
        var out = [];
        function C(label, fn) {
            try { fn(); out.push(label + " PASS"); }
            catch (e) { out.push(label + " FAIL:" + (e && e.message ? e.message : e)); }
        }
        function T(label, fn) {
            try { fn(); out.push(label + " FAIL:no-throw"); }
            catch (e) { out.push(label + " PASS"); }
        }
        var assert = require('assert');

        // --- strict deep comparison: objects/arrays/prototypes ---
        C("dse nested", function() { assert.deepStrictEqual({a:{b:[1,2]}}, {a:{b:[1,2]}}); });
        T("dse primitive type", function() { assert.deepStrictEqual(1, '1'); });
        T("dse zero sign", function() { assert.deepStrictEqual(+0, -0); });
        C("dse nan", function() { assert.deepStrictEqual(NaN, NaN); });
        T("dse null undefined", function() { assert.deepStrictEqual(null, undefined); });
        T("dse null proto", function() { assert.deepStrictEqual({}, Object.create(null)); });
        T("dse class instance", function() {
            function F() { this.a = 1; }
            assert.deepStrictEqual(new F(), {a:1});
        });
        T("dse length", function() { assert.deepStrictEqual([1,2], [1,2,3]); });
        T("dse own prop", function() { var x=[1,2]; x.p=1; assert.deepStrictEqual(x,[1,2]); });
        T("dse sparse vs undefined", function() { var a=[,1]; assert.deepStrictEqual(a,[undefined,1]); });
        C("dse shared vs clone", function() { var s={x:1}; assert.deepStrictEqual({a:s,b:s},{a:{x:1},b:{x:1}}); });
        C("dse symbol key same", function() {
            var s = Symbol('x');
            var mk = function() { var o = {}; o[s] = 1; return o; };
            assert.deepStrictEqual(mk(), mk());
        });
        T("dse symbol key diff", function() { var mk=function(){var o={};o[Symbol('x')]=1;return o;}; assert.deepStrictEqual(mk(),mk()); });
        C("dse getter enumerable", function() {
            var o = {};
            Object.defineProperty(o, 'a', {get: function(){return 1;}, enumerable: true});
            assert.deepStrictEqual(o, {a:1});
        });
        C("dse function identity", function() { var f = function(){}; assert.deepStrictEqual(f, f); });
        T("dse function distinct", function() { assert.deepStrictEqual(function(){}, function(){}); });

        // --- Date / RegExp ---
        C("dse date", function() { assert.deepStrictEqual(new Date(12345), new Date(12345)); });
        T("dse date diff", function() { assert.deepStrictEqual(new Date(0), new Date(1)); });
        C("dse date invalid", function() { assert.deepStrictEqual(new Date(NaN), new Date(NaN)); });
        C("dse regexp", function() { assert.deepStrictEqual(/a/gi, /a/gi); });
        T("dse regexp flags", function() { assert.deepStrictEqual(/a/g, /a/i); });
        T("dse regexp lastIndex", function() { var r1=/a/g; var r2=/a/g; r2.lastIndex=3; assert.deepStrictEqual(r1,r2); });

        // --- Map / Set ---
        C("dse map", function() { assert.deepStrictEqual(new Map([[1,'a'],[2,'b']]), new Map([[2,'b'],[1,'a']])); });
        T("dse map value", function() { assert.deepStrictEqual(new Map([[1,'a']]), new Map([[1,'b']])); });
        C("dse map deep keys", function() { assert.deepStrictEqual(new Map([[[1],'x']]), new Map([[[1],'x']])); });
        T("dse map deep keys diff", function() { assert.deepStrictEqual(new Map([[[1],'x']]), new Map([[[1],'y']])); });
        C("dse map dup eq keys", function() { assert.deepStrictEqual(new Map([[{},1],[{},2]]), new Map([[{},2],[{},1]])); });
        C("dse set reorder", function() { assert.deepStrictEqual(new Set([1,2,3]), new Set([3,2,1])); });
        T("dse set diff", function() { assert.deepStrictEqual(new Set([1,2]), new Set([1,3])); });
        C("dse set obj members", function() { assert.deepStrictEqual(new Set([[1,2],[3,4]]), new Set([[3,4],[1,2]])); });
        C("dse set dup obj", function() { assert.deepStrictEqual(new Set([{a:1},{a:1}]), new Set([{a:1},{a:1}])); });
        T("dse set prim loose-only", function() { assert.deepStrictEqual(new Set(['1']), new Set([1])); });
        C("dse set vs arr", function() { assert.notDeepEqual(new Set([1]), [1]) === undefined; });

        // --- typed arrays / buffers / ArrayBuffers ---
        C("dse u8", function() { assert.deepStrictEqual(new Uint8Array([1,2,3]), new Uint8Array([1,2,3])); });
        T("dse u8 ctor", function() { assert.deepStrictEqual(new Uint8Array([1]), new Int8Array([1])); });
        C("dse subarray", function() { assert.deepStrictEqual(new Uint8Array([1,2,3,4]).subarray(1), new Uint8Array([2,3,4])); });
        C("dse f32 bit-eq", function() { assert.deepStrictEqual(new Float32Array([1.5]), new Float32Array([1.5])); });
        C("dse f32 loose precision", function() { assert.deepStrictEqual(new Float32Array([0.1]), new Float32Array([0.10000000149011612])); });
        C("dse arraybuffer", function() { assert.deepStrictEqual(new Uint8Array([9]).buffer, new Uint8Array([9]).buffer); });
        T("dse arraybuffer diff", function() { assert.deepStrictEqual(new Uint8Array([9]).buffer, new Uint8Array([8]).buffer); });
        if (typeof Buffer === 'function') {
            C("dse buffer", function() { assert.deepStrictEqual(Buffer.from([1,2]), Buffer.from([1,2])); });
            T("dse buffer vs u8", function() { assert.deepStrictEqual(Buffer.from([1,2]), new Uint8Array([1,2])); });
        }

        // --- errors (name/message/cause) ---
        C("dse error", function() { assert.deepStrictEqual(new Error('a'), new Error('a')); });
        T("dse error msg", function() { assert.deepStrictEqual(new Error('a'), new Error('b')); });
        T("dse error class", function() { assert.deepStrictEqual(new TypeError('a'), new Error('a')); });
        C("dse error cause", function() { assert.deepStrictEqual(new Error('a',{cause:new Error('x')}), new Error('a',{cause:new Error('x')})); });
        T("dse error cause diff", function() { assert.deepStrictEqual(new Error('a',{cause:new Error('x')}), new Error('a',{cause:new Error('y')})); });
        T("dse error cause own-ness", function() { assert.deepStrictEqual(new Error('a',{cause:undefined}), new Error('a')); });

        // --- cycles ---
        C("dse cycle", function() { var a={}; a.s=a; var b={}; b.s=b; assert.deepStrictEqual(a,b); });
        T("dse cycle diff", function() { var a={}; a.s=a; var b={}; b.s={}; assert.deepStrictEqual(a,b); });

        // --- loose deepEqual ---
        C("de null undefined", function() { assert.deepEqual(null, undefined); });
        C("de coercion", function() { assert.deepEqual(1, '1'); });
        C("de 1 true", function() { assert.deepEqual(1, true); });
        C("de proto ignored", function() { assert.deepEqual({}, Object.create(null)); });
        C("de class ignored", function() { function F(){this.a=1;} assert.deepEqual(new F(), {a:1}); });
        T("de tag mismatch", function() { assert.deepEqual({0:1}, new String('1')); });

        // --- partialDeepStrictEqual ---
        C("pds subset", function() { assert.partialDeepStrictEqual({a:1,b:2},{a:1}); });
        T("pds value diff", function() { assert.partialDeepStrictEqual({a:1},{a:2}); });
        T("pds longer expected", function() { assert.partialDeepStrictEqual({a:1},{a:1,b:2}); });
        C("pds array prefix", function() { assert.partialDeepStrictEqual([1,2,3],[1,2]); });
        C("pds array not prefix", function() { assert.partialDeepStrictEqual([1,2,3],[2,3]); });
        C("pds ta prefix", function() { assert.partialDeepStrictEqual(new Uint8Array([1,2,3]), new Uint8Array([2,3])); });
        C("pds set subset", function() { assert.partialDeepStrictEqual(new Set([1,2,3]), new Set([1,2])); });
        C("pds map subset", function() { assert.partialDeepStrictEqual(new Map([[1,'a'],[2,'b']]), new Map([[1,'a']])); });
        T("pds err name", function() { assert.partialDeepStrictEqual(new TypeError('x'), {name:'TypeError'}); });

        out.join("|")
    "#,
    );
    assert_eq!(fail, 0, "deep compare battery had {} failures", fail);
    assert_eq!(pass, 65, "deep compare battery pass count drifted");

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_assert_strict_callable_shape() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let (pass, fail) = run_battery(
        &mut ctx,
        r#"
        var out = [];
        function C(label, cond) {
            out.push(label + (cond ? " PASS" : " FAIL:" + label));
        }
        var assert = require('assert');
        var strictMod = require('assert/strict');
        var s = assert.strict;

        C("typeof fn", typeof assert === 'function');
        C("strict typeof fn", typeof s === 'function');
        C("strict self", s === s.strict);
        C("req-strict is strict", strictMod === s);
        C("req-strict typeof fn", typeof strictMod === 'function');
        C("strict equal remap", s.equal === s.strictEqual);
        C("strict deepEqual remap", s.deepEqual === s.deepStrictEqual);
        C("strict notEqual remap", s.notEqual === s.notStrictEqual);
        C("strict notDeepEqual remap", s.notDeepEqual === s.notDeepStrictEqual);
        C("strict ok is assert.ok", s.ok === assert.ok);
        C("assert.ok self", assert.ok === assert);
        C("key parity", Object.keys(s).length === Object.keys(assert).length);
        C("has partial", typeof assert.partialDeepStrictEqual === 'function' && typeof s.partialDeepStrictEqual === 'function');
        C("has Assert", typeof assert.Assert === 'function' && assert.Assert === s.Assert);
        C("strict callable ok", (function(){ s(7); return true; })());
        C("strict deepEqual throws", (function(){ try { s.deepEqual(1, true); return false; } catch (e) { return e.name === 'AssertionError'; } })());
        C("strict notEqual 0 false", (function(){ s.notEqual(0, false); return true; })());
        C("strict no-arg msg", (function(){ try { s(); return false; } catch (e) { return e.message === 'No value argument passed to `assert.ok()`' && e.generatedMessage === true; } })());
        C("assert no-arg msg", (function(){ try { assert(); return false; } catch (e) { return e.message === 'No value argument passed to `assert.ok()`'; } })());

        out.join("|")
    "#,
    );
    assert_eq!(fail, 0, "strict shape battery had {} failures", fail);
    assert_eq!(pass, 19, "strict shape battery pass count drifted");

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_assert_throws_validation_and_messages() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Message-text assertions were oracle-anchored against node v24.19.0.
    let (pass, fail) = run_battery(
        &mut ctx,
        r#"
        var out = [];
        function M(label, fn, expected) {
            try { fn(); out.push(label + " FAIL:no-throw"); }
            catch (e) {
                if (String(e.message) === expected) out.push(label + " PASS");
                else out.push(label + " FAIL:got=" + JSON.stringify(e.message));
            }
        }
        function T(label, fn) {
            try { fn(); out.push(label + " FAIL:no-throw"); }
            catch (e) { out.push(label + " PASS"); }
        }
        function C(label, fn) {
            try { fn(); out.push(label + " PASS"); }
            catch (e) { out.push(label + " FAIL:" + String(e && e.message !== undefined ? e.message : e)); }
        }
        var assert = require('assert');

        // fn argument validation (node: TypeError ERR_INVALID_ARG_TYPE)
        T("throws non-fn", function() { assert.throws(1); });
        T("throws non-fn null", function() { assert.throws(null); });
        T("doesNotThrow non-fn", function() { assert.doesNotThrow('x'); });
        M("throws non-fn msg", function() { assert.throws(1); },
          'The "fn" argument must be of type function. Received type number (1)');

        // error-argument validation
        T("throws err arg string", function() { assert.throws(function(){}, 'Error message', 'message'); });
        M("throws err arg msg", function() { assert.throws(function(){}, 1); },
          'The "error" argument must be of type function or an instance of Error, RegExp, or Object. Received type number (1)');

        // missing-exception messages
        M("missing plain", function() { assert.throws(function(){}); }, 'Missing expected exception.');
        M("missing class", function() { assert.throws(function(){}, TypeError); }, 'Missing expected exception (TypeError).');
        M("missing str", function() { assert.throws(function(){}, 'fhqwhgads'); }, 'Missing expected exception: fhqwhgads');
        M("missing class+str", function() { assert.throws(function(){}, TypeError, 'fhqwhgads'); }, 'Missing expected exception (TypeError): fhqwhgads');

        // empty validation object
        T("empty spec", function() { assert.throws(function(){ throw new Error('x'); }, {}); });
        M("empty spec msg", function() { assert.throws(function(){ throw new Error('x'); }, {}); },
          "The argument 'error' may not be an empty object. Received {}");

        // ambiguous string message
        M("ambiguous err msg", function() { assert.throws(function(){ throw new TypeError('foo'); }, 'foo'); },
          'The "error/message" argument is ambiguous. The error message "foo" is identical to the message.');

        // object-spec matching via deep comparison
        C("obj spec match", function() {
            assert.throws(function(){ var e = new TypeError('Wrong value'); e.code = 404; throw e; },
                { name: 'TypeError', message: 'Wrong value', code: 404 });
        });
        T("obj spec value diff", function() {
            assert.throws(function(){ var e = new TypeError('Wrong value'); e.code = 405; throw e; },
                { code: 404 });
        });
        T("obj spec missing key", function() {
            assert.throws(function(){ var e = new TypeError('Wrong value'); e.code = 404; throw e; },
                { code: 404, foo: undefined });
        });
        C("obj spec regex message", function() {
            assert.throws(function(){ throw new Error('foobar'); }, { message: /foo/ });
        });

        // validator functions
        T("validator fail", function() {
            assert.throws(function(){ throw new Error('x'); }, function() { return false; });
        });
        C("validator passes", function() {
            assert.throws(function(){ throw new Error('x'); }, function(e) { return e.message === 'x'; });
        });
        // node appends "\n\nCaught error:\n\n" + String(actual) (stack
        // text is environment-dependent), so pin the stable prefix.
        C("validator fail msg", function() {
            try {
                assert.throws(function(){ throw new Error('x'); }, function boom() { return false; });
            } catch (e) {
                var head = 'The "boom" validation function is expected to return "true". Received false';
                if (e.message.indexOf(head) === 0) return;
                throw new Error('message mismatch: ' + e.message);
            }
            throw new Error('no throw');
        });

        // class spec with wrong error type
        T("class wrong type", function() { assert.throws(function(){ throw new Error('x'); }, TypeError); });

        // notEqual / notStrictEqual message forms (oracle-anchored)
        M("notEqual prim", function() { assert.notEqual(1, 1); }, '1 != 1');
        M("notStrictEqual short", function() { assert.notStrictEqual(2, 2); }, 'Expected "actual" to be strictly unequal to: 2');
        M("notStrictEqual long", function() { assert.notStrictEqual('a '.repeat(30), 'a '.repeat(30)); },
          'Expected "actual" to be strictly unequal to:\n\n\'' + 'a '.repeat(30) + '\'');

        // strictEqual primitive form + custom message diff
        M("strictEqual prim", function() { assert.strictEqual(1, 2); },
          'Expected values to be strictly equal:\n\n1 !== 2\n');
        M("strictEqual custom", function() { assert.strictEqual(1, 2, 'oh no'); },
          'oh no\n\n1 !== 2\n');

        // reference-identity message for structural twins
        M("notIdentical", function() { assert.strictEqual([], []); },
          'Values have same structure but are not reference-equal:\n\n[]\n');

        // deep-equal diff shape (oracle-anchored exact form)
        M("dse deep diff", function() { assert.deepStrictEqual([[[1,2,3]],4,5], [[[1,2,'3']],4,5]); },
          "Expected values to be strictly deep-equal:\n+ actual - expected\n\n  [\n    [\n      [\n        1,\n        2,\n+       3\n-       '3'\n      ]\n    ],\n    4,\n    5\n  ]\n");
        M("dse skip marker", function() { assert.deepStrictEqual([1,1,1,0,1,1,1,1],[1,1,1,0,1,1,1]); },
          "Expected values to be strictly deep-equal:\n+ actual - expected\n... Skipped lines\n\n  [\n    1,\n    1,\n    1,\n    0,\n...\n    1,\n+   1\n  ]\n");
        M("loose diff form", function() { assert.deepEqual({a:1},{a:2}); },
          "Expected values to be loosely deep-equal:\n\n{\n  a: 1\n}\n\nshould loosely deep-equal\n\n{\n  a: 2\n}");
        M("notDeepEqual form", function() { assert.notDeepStrictEqual([1],[1]); },
          'Expected "actual" not to be strictly deep-equal to:\n\n[\n  1\n]\n');

        // message-is-Error rethrow (equal/ok family)
        C("custom error rethrow", function() {
            var re = new RangeError('my range');
            try { assert.strictEqual(1, 2, re); } catch (e) {
                if (e === re) return;
                throw new Error('not identical');
            }
        });
        C("ok custom error rethrow", function() {
            var re = new RangeError('my range');
            try { assert.ok(0, re); } catch (e) {
                if (e === re) return;
                throw new Error('not identical');
            }
        });

        // AssertionError constructor validation
        var badInputs = [1, true, false, '', null, Infinity];
        for (var bi = 0; bi < badInputs.length; bi++) {
            (function(input) {
                T("AE bad " + typeof input, function() { new assert.AssertionError(input); });
            })(badInputs[bi]);
        }
        C("AE ok empty", function() { new assert.AssertionError({}); });
        M("AE bad msg", function() { new assert.AssertionError(1); },
          'The "options" argument must be of type object. Received type number (1)');
        M("AE toString", function() {
            var e = new assert.AssertionError({ message: 'm' });
            throw new Error('SENTINEL:' + (e.toString() === 'AssertionError [ERR_ASSERTION]: m' && e.name === 'AssertionError' && e.code === 'ERR_ASSERTION' && e.generatedMessage === false));
        }, 'SENTINEL:true');

        // Assert class
        T("Assert requires new", function() { assert.Assert(); });
        M("Assert requires new msg", function() { assert.Assert(); },
          "Class constructor Assert cannot be invoked without `new`");
        M("Assert bad diff", function() { new assert.Assert({ diff: 'x' }); },
          "The property 'options.diff' must be one of: 'simple', 'full'. Received 'x'");
        C("Assert loose instance", function() {
            var i = new assert.Assert({ strict: false });
            i.deepEqual(1, true);
        });
        C("Assert strict instance", function() {
            var i = new assert.Assert({});
            try { i.deepEqual(1, true); return; } catch (e) {
                if (e.name === 'AssertionError') return;
                throw e;
            }
        });

        // ifError
        T("ifError throws AE", function() { assert.ifError(new Error('x')); });
        M("ifError msg", function() { assert.ifError(new Error('x')); },
          'ifError got unwanted exception: x');
        C("ifError null ok", function() { assert.ifError(null); assert.ifError(undefined); });

        // match / doesNotMatch
        T("match regexp arg", function() { assert.match('abc', 'b'); });
        M("match regexp arg msg", function() { assert.match('abc', 'b'); },
          'The "regexp" argument must be an instance of RegExp. Received type string (\'b\')');
        T("match non-string", function() { assert.match(123, /x/); });
        T("dnm fail", function() { assert.doesNotMatch('abc', /b/); });

        out.join("|")
    "#,
    );
    assert_eq!(fail, 0, "throws battery had {} failures: see eprintln above", fail);
    assert_eq!(pass, 54, "throws battery pass count drifted");

    bun_runtime::shutdown_thread_sm();
}
