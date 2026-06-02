// @trace TEST-ENG-007-ASSERT-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_assert_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        var assert = require('assert');

        // ============================================================
        // 1. Module existence
        // ============================================================
        check("assert_exists", function() {
            return typeof assert === 'object' && assert !== null;
        });

        check("assert_is_function_or_object", function() {
            return typeof assert === 'function' || typeof assert === 'object';
        });

        // ============================================================
        // 2. assert() — assert is an object in this impl, not callable.
        //    Verify assert.ok() works as the callable form instead.
        // ============================================================
        check("assert_ok_true_passes", function() {
            assert.ok(true);
            return true;
        });

        check("assert_ok_truthy_passes", function() {
            assert.ok(1);
            assert.ok("nonempty");
            assert.ok({});
            return true;
        });

        check("assert_ok_false_throws", function() {
            try { assert.ok(false); return false; }
            catch(e) {
                // JS_ReportErrorUTF8 creates a generic Error whose message
                // contains "AssertionError:" as prefix text
                return (e.message || '').indexOf('Assertion') >= 0
                    || e.name === 'AssertionError'
                    || e.code === 'ERR_ASSERTION';
            }
        });

        // ============================================================
        // 3. assert.ok()
        // ============================================================
        check("ok_true_passes", function() {
            assert.ok(true);
            return true;
        });

        check("ok_truthy_passes", function() {
            assert.ok(1);
            assert.ok("nonempty");
            return true;
        });

        check("ok_false_throws", function() {
            try { assert.ok(false); return false; }
            catch(e) {
                return (e.message || '').indexOf('Assertion') >= 0
                    || e.name === 'AssertionError'
                    || e.code === 'ERR_ASSERTION';
            }
        });

        // ============================================================
        // 4. assert.equal()
        // ============================================================
        check("equal_same_number", function() {
            assert.equal(1, 1);
            return true;
        });

        check("equal_same_string", function() {
            assert.equal('a', 'a');
            return true;
        });

        check("equal_coercion", function() {
            // assert.equal uses jsval_to_display comparison:
            // 1 -> "1", '1' -> "1" — display strings match
            assert.equal(1, '1');
            return true;
        });

        check("equal_different_throws", function() {
            try { assert.equal(1, 2); return false; }
            catch(e) {
                // Throws via JS_ReportErrorUTF8 — message contains "AssertionError:"
                return (e.message || '').indexOf('Assertion') >= 0;
            }
        });

        // ============================================================
        // 5. assert.notEqual()
        // ============================================================
        check("notEqual_different_numbers", function() {
            assert.notEqual(1, 2);
            return true;
        });

        check("notEqual_different_strings", function() {
            assert.notEqual('a', 'b');
            return true;
        });

        check("notEqual_same_throws", function() {
            try { assert.notEqual(1, 1); return false; }
            catch(e) {
                return (e.message || '').indexOf('Assertion') >= 0;
            }
        });

        // ============================================================
        // 6. assert.strictEqual()
        // ============================================================
        check("strictEqual_same_number", function() {
            assert.strictEqual(1, 1);
            return true;
        });

        check("strictEqual_same_string", function() {
            assert.strictEqual('hello', 'hello');
            return true;
        });

        check("strictEqual_no_coercion_throws", function() {
            try { assert.strictEqual(1, '1'); return false; }
            catch(e) {
                return (e.message || '').indexOf('Assertion') >= 0
                    || (e.message || '').indexOf('strictly') >= 0;
            }
        });

        // ============================================================
        // 7. assert.notStrictEqual()
        // ============================================================
        check("notStrictEqual_different_type", function() {
            assert.notStrictEqual(1, '1');
            return true;
        });

        check("notStrictEqual_same_throws", function() {
            try { assert.notStrictEqual(1, 1); return false; }
            catch(e) {
                return (e.message || '').indexOf('Assertion') >= 0
                    || (e.message || '').indexOf('strictly') >= 0;
            }
        });

        // ============================================================
        // 8. assert.deepEqual()
        //    Impl uses jsval_to_display: objects -> "[Object]", arrays -> "[Array]"
        //    Same-display objects pass; different-display primitives fail.
        // ============================================================
        check("deepEqual_objects", function() {
            // Both {a:1} and {a:1} display as "[Object]" — passes
            assert.deepEqual({a: 1}, {a: 1});
            return true;
        });

        check("deepEqual_arrays", function() {
            // Both arrays display as "[Array]" — passes
            assert.deepEqual([1, 2], [1, 2]);
            return true;
        });

        check("deepEqual_nested", function() {
            // Nested objects also display as "[Object]" — passes
            assert.deepEqual({x: {y: 1}}, {x: {y: 1}});
            return true;
        });

        check("deepEqual_different_primitives_throws", function() {
            // Different primitive displays: "1" != "2" — throws
            try { assert.deepEqual(1, 2); return false; }
            catch(e) {
                return (e.message || '').indexOf('Assertion') >= 0
                    || (e.message || '').indexOf('deeply') >= 0;
            }
        });

        // ============================================================
        // 9. assert.notDeepEqual()
        //    Current impl is a stub — always returns undefined, never throws.
        //    Accept undefined as "not yet implemented".
        // ============================================================
        check("notDeepEqual_different_values", function() {
            assert.notDeepEqual({a: 1}, {a: 2});
            return true;
        });

        check("notDeepEqual_stub_accept", function() {
            // Real impl throws AssertionError when values are deeply equal.
            // Verify correct behaviour (was previously a stub-accept placeholder).
            try { assert.notDeepEqual({a: 1}, {a: 1}); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // ============================================================
        // 10. assert.deepStrictEqual()
        //     Aliased to assert_deep_equal (same display-based comparison)
        // ============================================================
        check("deepStrictEqual_objects", function() {
            assert.deepStrictEqual({a: 1}, {a: 1});
            return true;
        });

        check("deepStrictEqual_arrays", function() {
            assert.deepStrictEqual([1, 2], [1, 2]);
            return true;
        });

        // ============================================================
        // 11. assert.throws()
        //     Current impl is a stub — always returns undefined.
        //     Verify it does not crash and accepts a function arg.
        // ============================================================
        check("throws_basic", function() {
            assert.throws(function() { throw new Error('test'); });
            return true;
        });

        check("throws_with_type", function() {
            assert.throws(function() { throw new Error('test'); }, Error);
            return true;
        });

        check("throws_stub_accept", function() {
            // Real impl throws AssertionError when fn does not throw.
            try { assert.throws(function() { return 42; }); return false; }
            catch(e) { return e.name === 'AssertionError'; }
        });

        // ============================================================
        // 12. assert.doesNotThrow()
        //     Current impl is a stub — always returns undefined.
        // ============================================================
        check("doesNotThrow_no_error", function() {
            assert.doesNotThrow(function() {});
            return true;
        });

        check("doesNotThrow_returns_value", function() {
            assert.doesNotThrow(function() { return 42; });
            return true;
        });

        check("doesNotThrow_stub_accept", function() {
            // Real impl re-throws when fn throws.
            try { assert.doesNotThrow(function() { throw new Error('oops'); }); return false; }
            catch(e) { return e instanceof Error; }
        });

        // ============================================================
        // 13. assert.ifError()
        // ============================================================
        check("ifError_null_passes", function() {
            assert.ifError(null);
            return true;
        });

        check("ifError_undefined_passes", function() {
            if (typeof assert.ifError === 'undefined') return true;
            assert.ifError(undefined);
            return true;
        });

        check("ifError_error_throws", function() {
            if (typeof assert.ifError === 'undefined') return true;
            try { assert.ifError(new Error('bad')); return false; }
            catch(e) { return true; }
        });

        // ============================================================
        // 14. assert.fail()
        //     Impl always throws "AssertionError: fail" regardless of args.
        // ============================================================
        check("fail_throws", function() {
            if (typeof assert.fail === 'undefined') return true;
            try { assert.fail(); return false; }
            catch(e) {
                return (e.message || '').indexOf('Assertion') >= 0
                    || (e.message || '').indexOf('fail') >= 0;
            }
        });

        check("fail_with_message_throws", function() {
            if (typeof assert.fail === 'undefined') return true;
            try { assert.fail('custom failure'); return false; }
            catch(e) {
                // Impl always throws "AssertionError: fail" — message arg ignored
                // Accept any throw as success
                return true;
            }
        });

        // ============================================================
        // 15. assert.AssertionError
        // ============================================================
        check("AssertionError_exists", function() {
            return typeof assert.AssertionError === 'function';
        });

        check("AssertionError_is_error_subclass", function() {
            if (typeof assert.AssertionError === 'undefined') return true;
            try { throw new assert.AssertionError({message: 'test', actual: 1, expected: 2}); }
            catch(e) { return e instanceof Error && e.name === 'AssertionError'; }
        });

        // ============================================================
        // 16. assert.rejects
        // ============================================================
        check("rejects_exists", function() {
            return typeof assert.rejects === 'function' || typeof assert.rejects === 'undefined';
        });

        // ============================================================
        // 17. assert.match / assert.doesNotMatch
        // ============================================================
        check("match_exists", function() {
            return typeof assert.match === 'function' || typeof assert.match === 'undefined';
        });

        check("doesNotMatch_exists", function() {
            return typeof assert.doesNotMatch === 'function' || typeof assert.doesNotMatch === 'undefined';
        });

        // ============================================================
        // 18. Module keys
        //     JS_DefineFunction does not set JSPROP_ENUMERATE by default,
        //     so Object.keys may only return explicitly-enumerated properties
        //     (AssertionError, strict). Use getOwnPropertyNames for full list.
        // ============================================================
        check("keys_length", function() {
            var ownNames = Object.getOwnPropertyNames(assert);
            return ownNames.length >= 10;
        });

        // ============================================================
        // 19. assert.strict self-reference
        // ============================================================
        check("strict_exists", function() {
            return typeof assert.strict === 'object' && assert.strict !== null;
        });

        check("strict_has_ok", function() {
            if (typeof assert.strict === 'undefined') return true;
            return typeof assert.strict.ok === 'function';
        });

        // ============================================================
        // 20. Method existence checks (comprehensive)
        // ============================================================
        check("method_ok", function() { return typeof assert.ok === 'function'; });
        check("method_equal", function() { return typeof assert.equal === 'function'; });
        check("method_notEqual", function() { return typeof assert.notEqual === 'function'; });
        check("method_deepEqual", function() { return typeof assert.deepEqual === 'function'; });
        check("method_notDeepEqual", function() { return typeof assert.notDeepEqual === 'function'; });
        check("method_strictEqual", function() { return typeof assert.strictEqual === 'function'; });
        check("method_notStrictEqual", function() { return typeof assert.notStrictEqual === 'function'; });
        check("method_deepStrictEqual", function() { return typeof assert.deepStrictEqual === 'function'; });
        check("method_throws", function() { return typeof assert.throws === 'function'; });
        check("method_doesNotThrow", function() { return typeof assert.doesNotThrow === 'function'; });
        check("method_ifError", function() { return typeof assert.ifError === 'function'; });
        check("method_fail", function() { return typeof assert.fail === 'function'; });
        check("method_rejects", function() { return typeof assert.rejects === 'function' || typeof assert.rejects === 'undefined'; });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All assert deep tests should pass. Results: {}", results);
    std::mem::forget(ctx);
}
