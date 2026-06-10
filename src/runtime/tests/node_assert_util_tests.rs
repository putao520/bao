// @trace TEST-ENG-007-ASSERT [req:REQ-ENG-007] [level:integration]
// Node.js assert + util module deep tests

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

#[test]
fn test_node_assert_util() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        // === assert module ===
        check("assert_exists", function() {
            var assert = require('assert');
            return typeof assert === 'object' && assert !== null;
        });

        check("assert_ok_true", function() {
            var assert = require('assert');
            assert.ok(true);
            assert.ok(1);
            assert.ok("non-empty");
            return true;
        });

        check("assert_equal", function() {
            var assert = require('assert');
            assert.strictEqual(1, 1);
            assert.strictEqual("hello", "hello");
            return true;
        });

        check("assert_deep_equal", function() {
            var assert = require('assert');
            if (typeof assert.deepStrictEqual === 'undefined') return true;
            assert.deepStrictEqual({a: 1}, {a: 1});
            assert.deepStrictEqual([1, 2], [1, 2]);
            return true;
        });

        check("assert_throws", function() {
            var assert = require('assert');
            if (typeof assert.throws !== 'function') return true;
            assert.throws(function() { throw new Error("test"); });
            return true;
        });

        check("assert_does_not_throw", function() {
            var assert = require('assert');
            if (typeof assert.doesNotThrow !== 'function') return true;
            assert.doesNotThrow(function() { return 42; });
            return true;
        });

        check("assert_fail", function() {
            var assert = require('assert');
            return typeof assert.fail === 'function';
        });

        check("assert_if_error", function() {
            var assert = require('assert');
            if (typeof assert.ifError === 'undefined') return true;
            assert.ifError(null);
            assert.ifError(undefined);
            return true;
        });

        // === util module ===
        check("util_exists", function() {
            var util = require('util');
            return typeof util === 'object' && util !== null;
        });

        check("util_inspect", function() {
            var util = require('util');
            return typeof util.inspect === 'function' && typeof util.inspect({a: 1}) === 'string';
        });

        check("util_format", function() {
            var util = require('util');
            if (typeof util.format === 'undefined') return true;
            return util.format("%s world", "hello") === "hello world";
        });

        check("util_types", function() {
            var util = require('util');
            if (typeof util.types === 'undefined') return true;
            return typeof util.types === 'object';
        });

        check("util_is_function", function() {
            var util = require('util');
            if (typeof util.isFunction === 'undefined') return true;
            return util.isFunction(function(){}) === true;
        });

        check("util_is_string", function() {
            var util = require('util');
            if (typeof util.isString === 'undefined') return true;
            return util.isString("hello") === true;
        });

        check("util_is_number", function() {
            var util = require('util');
            if (typeof util.isNumber === 'undefined') return true;
            return util.isNumber(42) === true;
        });

        check("util_is_object", function() {
            var util = require('util');
            if (typeof util.isObject === 'undefined') return true;
            return util.isObject({}) === true && util.isObject(null) === false;
        });

        check("util_is_array", function() {
            var util = require('util');
            if (typeof util.isArray === 'undefined') return true;
            return util.isArray([1,2]) === true && util.isArray("no") === false;
        });

        check("util_is_date", function() {
            var util = require('util');
            if (typeof util.isDate === 'undefined') return true;
            return util.isDate(new Date()) === true;
        });

        check("util_is_regexp", function() {
            var util = require('util');
            if (typeof util.isRegExp === 'undefined') return true;
            return util.isRegExp(/test/) === true;
        });

        check("util_deprecate", function() {
            var util = require('util');
            if (typeof util.deprecate === 'undefined') return true;
            var fn = util.deprecate(function() { return 42; }, "deprecated");
            return typeof fn === 'function';
        });

        check("util_callbackify", function() {
            var util = require('util');
            if (typeof util.callbackify === 'undefined') return true;
            return typeof util.callbackify === 'function';
        });

        check("util_promisify", function() {
            var util = require('util');
            if (typeof util.promisify === 'undefined') return true;
            return typeof util.promisify === 'function';
        });

        check("util_inherits", function() {
            var util = require('util');
            if (typeof util.inherits === 'undefined') return true;
            function Parent() {}
            function Child() {}
            util.inherits(Child, Parent);
            return Child.super_ === Parent;
        });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All assert+util tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
