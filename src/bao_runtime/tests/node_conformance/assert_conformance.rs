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
