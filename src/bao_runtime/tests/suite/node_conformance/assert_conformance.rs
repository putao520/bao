// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:assert against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/assert/assert.test.cjs (MIT, Bun project)

#[path = "../conformance_common.rs"]
mod common;

use common::{CHECK_SCAFFOLD, make_ctx, run_checks};

#[test]
fn test_assert_conformance_suite() {
    let mut ctx = make_ctx();

    // ===== ok =====
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        check("ok_truthy", function() {{
            try {{ assert.ok(1); assert.ok("nonempty"); return true; }}
            catch(e) {{ return false; }}
        }});
        check("ok_throws_on_falsy", function() {{
            try {{ assert.ok(0); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        check("ok_throws_on_false", function() {{
            try {{ assert.ok(false); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        check("ok_throws_on_null", function() {{
            try {{ assert.ok(null); return false; }}
            catch(e) {{ return true; }}
        }});
        check("ok_throws_on_undefined", function() {{
            try {{ assert.ok(undefined); return false; }}
            catch(e) {{ return true; }}
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== equal / strictEqual =====
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        check("equal_loose_coercion", function() {{
            try {{ assert.equal(1, "1"); return true; }} catch(e) {{ return false; }}
        }});
        check("equal_strict_no_coercion", function() {{
            try {{ assert.strictEqual(1, "1"); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        check("equal_throws_on_diff", function() {{
            try {{ assert.equal(1, 2); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        check("notEqual_passes_diff", function() {{
            try {{ assert.notEqual(1, 2); return true; }} catch(e) {{ return false; }}
        }});
        check("notStrictEqual_passes", function() {{
            try {{ assert.notStrictEqual(1, "1"); return true; }} catch(e) {{ return false; }}
        }});
        check("strictEqual_same_string", function() {{
            try {{ assert.strictEqual("abc", "abc"); return true; }} catch(e) {{ return false; }}
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== deepEqual / deepStrictEqual =====
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        check("deepEqual_same_obj", function() {{
            try {{ assert.deepEqual({{a: 1}}, {{a: 1}}); return true; }}
            catch(e) {{ return false; }}
        }});
        check("deepEqual_throws_on_diff", function() {{
            try {{ assert.deepEqual({{a: 1}}, {{a: 2}}); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        check("deepStrictEqual_strict", function() {{
            try {{ assert.deepStrictEqual({{a: 1}}, {{a: 1}}); return true; }}
            catch(e) {{ return false; }}
        }});
        check("deepStrictEqual_strict_throws_coercion", function() {{
            try {{ assert.deepStrictEqual({{a: 1}}, {{a: "1"}}); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== throws / doesNotThrow =====
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        check("throws_passes_when_thrown", function() {{
            try {{ assert.throws(function() {{ throw new Error("x"); }}); return true; }}
            catch(e) {{ return false; }}
        }});
        check("throws_fails_when_not_thrown", function() {{
            try {{ assert.throws(function() {{ }}); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        check("throws_matches_class", function() {{
            try {{ assert.throws(function() {{ throw new TypeError("oops"); }}, TypeError); return true; }}
            catch(e) {{ return false; }}
        }});
        check("throws_matches_regex", function() {{
            try {{ assert.throws(function() {{ throw new Error("specific message"); }}, /specific/); return true; }}
            catch(e) {{ return false; }}
        }});
        check("doesNotThrow_passes", function() {{
            try {{ assert.doesNotThrow(function() {{ }}); return true; }}
            catch(e) {{ return false; }}
        }});
        check("doesNotThrow_fails_on_throw", function() {{
            try {{ assert.doesNotThrow(function() {{ throw new Error("x"); }}); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== fail / ifError =====
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        check("fail_throws", function() {{
            try {{ assert.fail("explicit"); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        check("ifError_passes_on_null", function() {{
            try {{ assert.ifError(null); return true; }} catch(e) {{ return false; }}
        }});
        check("ifError_passes_on_undefined", function() {{
            try {{ assert.ifError(undefined); return true; }} catch(e) {{ return false; }}
        }});
        check("ifError_throws_on_error", function() {{
            try {{ assert.ifError(new Error("x")); return false; }}
            catch(e) {{ return e instanceof Error; }}
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== strict submodule =====
    let src = format!(
        r##"
        {scaffold}
        check("strict_module_exists", function() {{
            try {{ var s = require('assert/strict'); return typeof s === "object" && typeof s.ok === "function"; }}
            catch(e) {{ return false; }}
        }});
        check("strict_property_alias", function() {{
            var assert = require('assert');
            return typeof assert.strict === "object";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_assert_conformance_match() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        check("match_is_function", function() {{
            return typeof assert.match === "function";
        }});
        check("doesNotMatch_is_function", function() {{
            return typeof assert.doesNotMatch === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_assert_conformance_rejects() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        check("rejects_is_function", function() {{
            return typeof assert.rejects === "function";
        }});
        check("doesNotReject_is_function", function() {{
            return typeof assert.doesNotReject === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_assert_conformance_call_tracker() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        check("CallTracker_constructor", function() {{
            return typeof assert.CallTracker === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

// ===== failure diff rendering (domain-check 300f3a0b29, own idiom) =====
// Equality/deep-equality failures must carry a line-level +/- Myers diff of
// actual vs expected in the message. Exact rendered lines are asserted (not
// just "message exists") so regressions in marker/indent layout fail loudly.
#[test]
fn test_assert_diff_rendering_conformance() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var assert = require('assert');
        function msgOf(fn) {{
            try {{ fn(); }} catch(e) {{ return e.message || ''; }}
            return '__NOT_THROWN__';
        }}

        // --- object diff: deepEqual({{a:1,b:2}}, {{a:1,b:3}}) ---
        check("obj_diff_markers", function() {{
            var m = msgOf(function() {{ assert.deepEqual({{a: 1, b: 2}}, {{a: 1, b: 3}}); }});
            return m.indexOf('Expected values to be loosely deeply equal:') >= 0
                && m.indexOf('+ actual - expected') >= 0
                && m.indexOf('\n  {{') >= 0
                && m.indexOf('\n    a: 1\n') >= 0
                && m.indexOf('\n+   b: 2\n') >= 0
                && m.indexOf('\n-   b: 3\n') >= 0
                && m.indexOf('\n  }}') >= 0;
        }});

        // --- array diff: deepStrictEqual([1,2,3,4], [1,5,3,9]) ---
        check("array_diff_markers", function() {{
            var m = msgOf(function() {{ assert.deepStrictEqual([1, 2, 3, 4], [1, 5, 3, 9]); }});
            return m.indexOf('Expected values to be strictly deeply equal:') >= 0
                && m.indexOf('\n    1\n') >= 0
                && m.indexOf('\n+   2\n') >= 0
                && m.indexOf('\n-   5\n') >= 0
                && m.indexOf('\n    3\n') >= 0
                && m.indexOf('\n+   4\n') >= 0
                && m.indexOf('\n-   9\n') >= 0;
        }});

        // --- string diff: multi-line strings split and diffed by line ---
        check("string_diff_markers", function() {{
            var m = msgOf(function() {{
                assert.deepStrictEqual('one\ntwo\nthree', 'one\nTWO\nthree');
            }});
            return m.indexOf("+ actual - expected") >= 0
                && m.indexOf("\n  'one\n") >= 0
                && m.indexOf('\n+   two\n') >= 0
                && m.indexOf('\n-   TWO\n') >= 0
                && m.indexOf("\n    three'") >= 0;
        }});

        // --- scalar inequality: deep* renders +/- line diff even for scalars ---
        check("scalar_diff_markers", function() {{
            var m = msgOf(function() {{ assert.deepStrictEqual(1, 2); }});
            return m.indexOf('Expected values to be strictly deeply equal:') >= 0
                && m.indexOf('\n+ 1\n') >= 0
                && m.indexOf('\n- 2') >= 0;
        }});

        // --- scalar inequality: equal() keeps short primitives inline ---
        check("scalar_equal_inline", function() {{
            var m = msgOf(function() {{ assert.equal(1, 2); }});
            return m.indexOf('1 == 2') >= 0
                && m.indexOf('+ actual - expected') === -1;
        }});

        // --- equal() on objects: no more "[object Object]", full diff body ---
        check("equal_objects_diff_not_opaque", function() {{
            var m = msgOf(function() {{ assert.equal({{a: 1}}, {{a: 2}}); }});
            return m.indexOf('Expected values to be loosely equal:') >= 0
                && m.indexOf('[object Object]') === -1
                && m.indexOf('\n+   a: 1\n') >= 0
                && m.indexOf('\n-   a: 2\n') >= 0;
        }});

        // --- strictEqual() on objects goes through the diff too ---
        check("strictEqual_objects_diff", function() {{
            var m = msgOf(function() {{ assert.strictEqual({{x: 1}}, {{x: 2}}); }});
            return m.indexOf('Expected values to be strictly equal:') >= 0
                && m.indexOf('\n+   x: 1\n') >= 0
                && m.indexOf('\n-   x: 2\n') >= 0;
        }});

        // --- long/multi-line string operands in equal() leave the inline path ---
        check("equal_long_string_diff", function() {{
            var long1 = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1';
            var long2 = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2';
            var m = msgOf(function() {{ assert.strictEqual(long1, long2); }});
            return m.indexOf("+ actual - expected") >= 0
                && m.indexOf("+ '" + long1) >= 0
                && m.indexOf("- '" + long2) >= 0;
        }});

        // --- nested containers diff at their own depth ---
        check("nested_diff_indent", function() {{
            var m = msgOf(function() {{
                assert.deepStrictEqual({{x: {{y: 1}}, k: 'same'}}, {{x: {{y: 2}}, k: 'same'}});
            }});
            return m.indexOf('\n+     y: 1\n') >= 0
                && m.indexOf('\n-     y: 2\n') >= 0
                && m.indexOf("\n    k: 'same'\n") >= 0;
        }});

        // --- Latin-1 / UTF-16 text survives rendering byte-exact ---
        // (the corruption class the upstream 300f3a0b29 oracle fixes natively;
        //  here values never leave JS strings, so no mojibake is possible)
        check("unicode_survives", function() {{
            var m = msgOf(function() {{
                assert.deepStrictEqual({{s: 'línea\nütf'}}, {{s: 'linea\nutf'}});
            }});
            return m.indexOf('+   s: \'línea') >= 0
                && m.indexOf('ütf\'') >= 0
                && m.indexOf('-   s: \'linea') >= 0
                && m.indexOf('utf\'') >= 0;
        }});
        check("utf16_emoji_survives", function() {{
            var m = msgOf(function() {{
                assert.deepStrictEqual({{s: '😀\nx'}}, {{s: '😺\nx'}});
            }});
            return m.indexOf('😀') >= 0 && m.indexOf('😺') >= 0;
        }});

        // --- long equal runs collapse with an explicit marker ---
        check("long_run_collapsed", function() {{
            var a = Array.from({{length: 30}}, function(_, i) {{ return 'v' + i; }});
            var b = a.map(function(v, i) {{ return (i === 2 || i === 27) ? 'x' : v; }});
            var m = msgOf(function() {{ assert.deepStrictEqual(a, b); }});
            return m.indexOf('\n+   \'v2\',\n') === -1 // no comma in own layout
                && m.indexOf("\n+   'v2'\n") >= 0
                && m.indexOf("\n+   'v27'\n") >= 0
                && m.indexOf('\n-   \'x\'\n') >= 0
                && m.indexOf('... Skipped 18 identical lines') >= 0;
        }});

        // --- cycles render [Circular] instead of recursing forever ---
        check("cycle_no_hang", function() {{
            var o = {{}}; o.self = o;
            var m = msgOf(function() {{ assert.deepEqual(o, {{a: 1}}); }});
            return m.indexOf('[Circular]') >= 0;
        }});

        // --- custom message still replaces the generated diff body ---
        check("custom_message_wins", function() {{
            var m = msgOf(function() {{ assert.deepEqual(1, 2, 'my custom reason'); }});
            return m.indexOf('my custom reason') >= 0
                && m.indexOf('+ actual - expected') === -1;
        }});

        // --- err contract preserved: actual/expected/operator fields intact ---
        check("err_fields_intact", function() {{
            try {{ assert.deepEqual({{a: 1}}, {{a: 2}}); return false; }}
            catch(e) {{
                return e.name === 'AssertionError'
                    && e.actual !== null && typeof e.actual === 'object' && e.actual.a === 1
                    && e.expected !== null && typeof e.expected === 'object' && e.expected.a === 2
                    && e.operator === 'deepEqual';
            }}
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}
