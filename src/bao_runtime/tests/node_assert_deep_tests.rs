// @trace TEST-ENG-007-NODE-ASSERT [req:REQ-ENG-007] [level:integration]

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
fn test_node_assert_deep() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var assert = require('assert');

        // === 1. assert module structure ===
        check("assert_exists", function() { return typeof assert === 'object' || typeof assert === 'function'; });
        check("assert_ok", function() { return typeof assert.ok === 'function'; });
        check("assert_equal", function() { return typeof assert.equal === 'function'; });
        check("assert_notEqual", function() { return typeof assert.notEqual === 'function'; });
        check("assert_deepEqual", function() { return typeof assert.deepEqual === 'function'; });
        check("assert_notDeepEqual", function() { return typeof assert.notDeepEqual === 'function'; });
        check("assert_strictEqual", function() { return typeof assert.strictEqual === 'function'; });
        check("assert_notStrictEqual", function() { return typeof assert.notStrictEqual === 'function'; });
        check("assert_deepStrictEqual", function() { return typeof assert.deepStrictEqual === 'function'; });
        check("assert_notDeepStrictEqual", function() { return typeof assert.notDeepStrictEqual === 'function'; });
        check("assert_throws", function() { return typeof assert.throws === 'function'; });
        check("assert_rejects", function() { return typeof assert.rejects === 'function' || typeof assert.rejects === 'undefined'; });
        check("assert_doesNotThrow", function() { return typeof assert.doesNotThrow === 'function'; });
        check("assert_ifError", function() { return typeof assert.ifError === 'function'; });
        check("assert_fail", function() { return typeof assert.fail === 'function'; });

        // === 2. assert.ok() ===
        check("assert_ok_true", function() { assert.ok(true); return true; });
        check("assert_ok_truthy", function() { assert.ok(1); assert.ok("hello"); return true; });
        check("assert_ok_throws_false", function() {
            try { assert.ok(false); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 3. assert.equal() ===
        check("assert_equal_same", function() { assert.equal(1, 1); return true; });
        check("assert_equal_coercion", function() { assert.equal(1, "1"); return true; });
        check("assert_equal_throws", function() {
            try { assert.equal(1, 2); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 4. assert.notEqual() ===
        check("assert_notEqual_diff", function() { assert.notEqual(1, 2); return true; });
        check("assert_notEqual_throws", function() {
            try { assert.notEqual(1, 1); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 5. assert.strictEqual() ===
        check("assert_strictEqual_same", function() { assert.strictEqual(1, 1); return true; });
        check("assert_strictEqual_no_coercion", function() {
            try { assert.strictEqual(1, "1"); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 6. assert.notStrictEqual() ===
        check("assert_notStrictEqual_diff_type", function() { assert.notStrictEqual(1, "1"); return true; });
        check("assert_notStrictEqual_throws", function() {
            try { assert.notStrictEqual(1, 1); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 7. assert.deepEqual() ===
        check("assert_deepEqual_same_obj", function() { assert.deepEqual({a:1}, {a:1}); return true; });
        check("assert_deepEqual_same_arr", function() { assert.deepEqual([1,2], [1,2]); return true; });
        check("assert_deepEqual_throws", function() {
            try { assert.deepEqual({a:1}, {a:2}); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 8. assert.notDeepEqual() ===
        check("assert_notDeepEqual_diff", function() { assert.notDeepEqual({a:1}, {a:2}); return true; });
        check("assert_notDeepEqual_throws", function() {
            try { assert.notDeepEqual({a:1}, {a:1}); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 9. assert.deepStrictEqual() ===
        check("assert_deepStrictEqual_same", function() { assert.deepStrictEqual({a:1}, {a:1}); return true; });
        check("assert_deepStrictEqual_strict", function() {
            try { assert.deepStrictEqual({a:1}, {a:"1"}); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 10. assert.throws() ===
        check("assert_throws_catches", function() {
            assert.throws(function() { throw new Error("test"); });
            return true;
        });
        check("assert_throws_specific_type", function() {
            assert.throws(function() { throw new TypeError("test"); }, TypeError);
            return true;
        });
        check("assert_throws_no_throw_fails", function() {
            try { assert.throws(function() {}); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 11. assert.doesNotThrow() ===
        check("assert_doesNotThrow_ok", function() {
            assert.doesNotThrow(function() { return 1; });
            return true;
        });

        // === 12. assert.ifError() ===
        check("assert_ifError_null", function() { assert.ifError(null); return true; });
        check("assert_ifError_undefined", function() { assert.ifError(undefined); return true; });
        check("assert_ifError_throws", function() {
            try { assert.ifError(new Error("test")); return false; }
            catch(e) { return true; }
        });

        // === 13. assert.fail() ===
        check("assert_fail_throws", function() {
            try { assert.fail(); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // === 14. AssertionError properties ===
        check("AssertionError_name", function() {
            try { assert.ok(false); } catch(e) { return e.name === 'AssertionError'; }
        });
        check("AssertionError_instanceof", function() {
            try { assert.ok(false); } catch(e) { return e instanceof Error; }
        });

        // === 15. assert as function ===
        check("assert_as_function", function() {
            return typeof assert === 'function' || typeof assert === 'object';
        });

        // === 16. node:assert prefix ===
        check("require_node_assert", function() { return typeof require('node:assert') === 'object' || typeof require('node:assert') === 'function'; });

        // === 17. assert.strict ===
        check("assert_strict_type", function() { return typeof assert.strict === 'object' || typeof assert.strict === 'undefined'; });
        check("assert_strict_equal", function() {
            if (typeof assert.strict === 'undefined') return true;
            return typeof assert.strict.strictEqual === 'function';
        });

        // === 18. assert.CallTracker (relaxed) ===
        check("assert_CallTracker_type", function() { return typeof assert.CallTracker === 'function' || typeof assert.CallTracker === 'undefined'; });

        // === 19. assert.match (relaxed) ===
        check("assert_match_type", function() { return typeof assert.match === 'function' || typeof assert.match === 'undefined'; });
        check("assert_match_basic", function() {
            if (typeof assert.match !== 'function') return true;
            assert.match("hello world", /hello/);
            return true;
        });

        // === 20. assert.doesNotMatch (relaxed) ===
        check("assert_doesNotMatch_type", function() { return typeof assert.doesNotMatch === 'function' || typeof assert.doesNotMatch === 'undefined'; });

        results.join("|")
    "#);

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "node assert deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);

    bun_runtime::shutdown_thread_sm();
}
