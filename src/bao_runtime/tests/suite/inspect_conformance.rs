// @trace REQ-ENG-006 [api:Bun.inspect] [req:REQ-ENG-004 util.inspect] [level:integration]
// Conformance suite for the inspect serialisation domain fixes
// (domain-check 06d0ae8ac1/abe2ad4f00/a21f02a988, own-idiom fixes with
// upstream oracle semantics):
//   1. integer-index keys render (ascending numeric order first, string
//      keys in insertion order after — Node/Bun inspect key order),
//   2. array non-index own properties render after the elements,
//   3. Headers record form (fetch init.headers) carries numeric keys,
//   4. built-in classes read through native SM paths — Date time value,
//      Map/Set tables, Error prototype identity — prototype pollution
//      cannot reach the output; custom inspect stays the user entry,
//   5. no silent blanks: swallowed exceptions render as `<exception>`,
//   6. options validation: non-object 2nd arg throws ERR_INVALID_ARG_TYPE
//      (canonical code+message); util.inspect passes depth through.
//
// Assertions are exact-output (===) — no truthiness checks.

#[path = "conformance_common.rs"]
mod common;

use bao_engine::context::JsContext;
use common::{eval_string, make_ctx, run_checks};

/// check() scaffold + the exact-output helpers (same shape as the argon2
/// conformance suite).
const INSPECT_PRELUDE: &str = r#"
var results = [];
function check(label, fn) {
    try {
        var ok = fn();
        results.push(label + ":" + (ok === true ? "PASS" : ("FAIL" + (ok === false ? "" : ":" + ok))));
    } catch (e) { results.push(label + ":ERROR:" + (e && e.message ? e.message : e)); }
}
// Exact-equality helper: returns true or a "got<want>" mismatch fragment.
function eq(got, want) {
    return got === want ? true : ("got<" + JSON.stringify(got) + "> want<" + JSON.stringify(want) + ">");
}
// Node-canonical ERR_INVALID_ARG_TYPE assertion: instanceof TypeError +
// .code + .message verbatim.
function expectInvalidArgType(fn, message) {
    var e;
    var threw = false;
    try { fn(); } catch (caught) { e = caught; threw = true; }
    if (!threw) return "no-throw";
    var problems = [];
    if (!(e instanceof TypeError)) problems.push("ctor:" + ((e && e.constructor && e.constructor.name) ? e.constructor.name : String(e)));
    if (!e || e.code !== "ERR_INVALID_ARG_TYPE") problems.push("code:" + (e ? String(e.code) : "n/a"));
    if (!e || e.message !== message) problems.push("message:<" + (e ? String(e.message) : "n/a") + ">");
    return problems.length === 0 ? true : problems.join(" / ");
}
var util = require("util");
"#;

fn bundle(body: &str) -> String {
    format!("{prelude}\n{body}", prelude = INSPECT_PRELUDE, body = body)
}

/// Integer-index keys render, in ascending numeric order, before the
/// insertion-order string keys (engine OrdinaryOwnPropertyKeys order ==
/// Node/Bun inspect order).
#[test]
fn inspect_conformance_numeric_keys() {
    let mut ctx = make_ctx();
    run_checks(
        &mut ctx,
        &bundle(
            r#"
        check("numeric_keys_all_render_ints_first", function() {
            return eq(Bun.inspect({ 0: "a", foo: "b", 2: "c", 1: "d", bar: "e" }),
                      '{ 0: "a", 1: "d", 2: "c", foo: "b", bar: "e" }');
        });
        check("numeric_order_is_numeric_not_lexicographic", function() {
            return eq(Bun.inspect({ 10: "x", 9: "y" }), '{ 9: "y", 10: "x" }');
        });
        check("non_index_string_keys_stay_quoted", function() {
            return eq(Bun.inspect({ "-1": "x", "a-b": "y" }), '{ "-1": "x", "a-b": "y" }');
        });
        check("util_inspect_numeric_keys", function() {
            return eq(util.inspect({ 0: "a", foo: "b" }), "{ 0: 'a', foo: 'b' }");
        });
        results.join("|")
        "#,
        ),
    );
    bun_runtime::shutdown_thread_sm();
}

/// Arrays render elements first, then non-index own properties (Node:
/// `[ 1, 2, 3, foo: 'x' ]`); own integer indices >= length render as keys.
#[test]
fn inspect_conformance_array_own_props() {
    let mut ctx = make_ctx();
    run_checks(
        &mut ctx,
        &bundle(
            r#"
        var a = [1, 2, 3];
        a.foo = "x";
        check("array_non_index_prop_renders_after_elements", function() {
            return eq(Bun.inspect(a), '[ 1, 2, 3, foo: "x" ]');
        });
        var e = [];
        e.foo = "x";
        check("empty_array_with_own_prop", function() {
            return eq(Bun.inspect(e), '[ foo: "x" ]');
        });
        var b = [1, 2];
        b[3] = 9;
        b.z = "w";
        check("array_beyond_length_index_renders_as_key", function() {
            return eq(Bun.inspect(b), '[ 1, 2, undefined, 9, z: "w" ]');
        });
        check("util_inspect_array_own_prop", function() {
            var u = [1, 2, 3];
            u.foo = "x";
            return eq(util.inspect(u), "[ 1, 2, 3, foo: 'x' ]");
        });
        results.join("|")
        "#,
        ),
    );
    bun_runtime::shutdown_thread_sm();
}

/// Built-in classes render through native SM paths (Date time value + spec
/// ISO math, Map/Set native table reads, Error prototype identity), so
/// user-replaced prototype members cannot reach the output.
#[test]
fn inspect_conformance_builtin_native_reads() {
    let mut ctx = make_ctx();
    run_checks(
        &mut ctx,
        &bundle(
            r#"
        check("date_epoch_exact_iso", function() {
            return eq(Bun.inspect(new Date(0)), "1970-01-01T00:00:00.000Z");
        });
        check("date_leap_day_exact_iso", function() {
            return eq(Bun.inspect(new Date(Date.UTC(2024, 1, 29, 12, 34, 56, 789))),
                      "2024-02-29T12:34:56.789Z");
        });
        check("date_year_zero_four_digit", function() {
            // Astronomical year 0 (= 1 BC, a 400-divisible leap year) starts
            // 719528 days before the epoch: -62167219200000 ms. (The earlier
            // constant -62135596800000 was 0001-01-01 — 719162 days — the
            // engine rendered that one correctly too.)
            return eq(Bun.inspect(new Date(-62167219200000)), "0000-01-01T00:00:00.000Z");
        });
        check("date_extended_year_signed_six_digit", function() {
            return eq(Bun.inspect(new Date(Date.UTC(10000, 0, 1))), "+010000-01-01T00:00:00.000Z");
        });
        check("date_invalid", function() {
            return eq(Bun.inspect(new Date(NaN)), "Invalid Date");
        });
        check("map_exact_shape", function() {
            return eq(Bun.inspect(new Map([["a", 1], ["b", 2]])), 'Map(2) { "a" => 1, "b" => 2 }');
        });
        check("set_exact_shape", function() {
            return eq(Bun.inspect(new Set([1, 2])), "Set(2) { 1, 2 }");
        });
        check("error_typeerror_first_line", function() {
            return eq(Bun.inspect(new TypeError("m")).split("\n")[0], "TypeError: m");
        });
        check("error_own_name_wins", function() {
            var err = new Error("boom");
            err.name = "Custom";
            return eq(Bun.inspect(err).split("\n")[0], "Custom: boom");
        });
        check("error_non_string_message_coerced", function() {
            var err = new Error("m");
            err.message = 42;
            return eq(Bun.inspect(err).split("\n")[0], "Error: 42");
        });
        check("arraybuffer_bytelength_number", function() {
            return eq(Bun.inspect(new ArrayBuffer(8)), "ArrayBuffer { byteLength: 8 }");
        });
        check("typedarray_constructor_name", function() {
            return eq(Bun.inspect(new Uint8Array([1, 2, 3])), "Uint8Array(3) [ 1, 2, 3 ]");
        });
        results.join("|")
        "#,
        ),
    );
    bun_runtime::shutdown_thread_sm();
}

/// Prototype pollution cannot reach inspect output: replaced
/// Date.prototype.toISOString / Error.prototype.name / Map.prototype.entries
/// / Set.prototype.values are never invoked.
#[test]
fn inspect_conformance_prototype_pollution() {
    let mut ctx = make_ctx();
    run_checks(
        &mut ctx,
        &bundle(
            r#"
        Date.prototype.toISOString = function() { return "PWNED"; };
        Error.prototype.name = "PWN";
        Object.defineProperty(TypeError.prototype, "name", { value: "PWN2", configurable: true });
        Map.prototype.entries = function() { return [][Symbol.iterator](); };
        Set.prototype.values = function() { return [][Symbol.iterator](); };
        check("poisoned_toISOString_not_used", function() {
            return eq(Bun.inspect(new Date(0)), "1970-01-01T00:00:00.000Z");
        });
        check("poisoned_error_name_not_used", function() {
            return eq(Bun.inspect(new TypeError("m")).split("\n")[0], "TypeError: m");
        });
        check("plain_error_name_still_error", function() {
            return eq(Bun.inspect(new Error("m")).split("\n")[0], "Error: m");
        });
        check("own_name_survives_pollution", function() {
            var err = new Error("boom");
            err.name = "Custom";
            return eq(Bun.inspect(err).split("\n")[0], "Custom: boom");
        });
        check("poisoned_map_entries_not_used", function() {
            return eq(Bun.inspect(new Map([["a", 1]])), 'Map(1) { "a" => 1 }');
        });
        check("poisoned_set_values_not_used", function() {
            return eq(Bun.inspect(new Set([1, 2])), "Set(2) { 1, 2 }");
        });
        results.join("|")
        "#,
        ),
    );
    bun_runtime::shutdown_thread_sm();
}

/// nodejs.util.inspect.custom stays the user entry: the registered symbol
/// (Node/Bun semantics, hook receives the levels-from-root depth) and the
/// legacy plain-string property; non-string/throwing hooks fall through to
/// normal rendering (never a silent blank).
#[test]
fn inspect_conformance_custom_inspect_entry() {
    let mut ctx = make_ctx();
    run_checks(
        &mut ctx,
        &bundle(
            r#"
        var sym = {};
        sym[Symbol.for("nodejs.util.inspect.custom")] = function(d) { return "CUSTOM:" + d; };
        check("symbol_custom_dispatch_with_node_depth_arg", function() {
            return eq(Bun.inspect(sym), "CUSTOM:2");
        });
        check("symbol_custom_depth_follows_option", function() {
            return eq(Bun.inspect(sym, { depth: 0 }), "CUSTOM:0");
        });
        var legacy = {};
        legacy["nodejs.util.inspect.custom"] = function() { return "LEGACY"; };
        check("legacy_string_custom_dispatch", function() {
            return eq(Bun.inspect(legacy), "LEGACY");
        });
        var nonstr = {};
        nonstr[Symbol.for("nodejs.util.inspect.custom")] = function() { return 42; };
        check("non_string_custom_falls_through", function() {
            return eq(Bun.inspect(nonstr), "{}");
        });
        var throwing = {};
        throwing[Symbol.for("nodejs.util.inspect.custom")] = function() { throw new Error("hook"); };
        check("throwing_custom_falls_through_non_silent", function() {
            return eq(Bun.inspect(throwing), "{}");
        });
        results.join("|")
        "#,
        ),
    );
    bun_runtime::shutdown_thread_sm();
}

/// Swallowed exceptions render as the `<exception>` placeholder — output is
/// never a silent blank (poisoned RegExp.prototype.source getter throws).
#[test]
fn inspect_conformance_no_silent_blanks() {
    let mut ctx = make_ctx();
    run_checks(
        &mut ctx,
        &bundle(
            r#"
        Object.defineProperty(RegExp.prototype, "source", {
            get: function() { throw new Error("poison"); },
            configurable: true,
        });
        check("throwing_source_renders_placeholder", function() {
            var out = Bun.inspect(/re/g);
            return out.indexOf("<exception>") !== -1 ? true : "got<" + out + ">";
        });
        results.join("|")
        "#,
        ),
    );
    bun_runtime::shutdown_thread_sm();
}

/// Options (2nd arg): null/undefined ignored; any other non-object throws
/// ERR_INVALID_ARG_TYPE with the canonical code+message; depth (incl.
/// Infinity) and the depth:0 root-keys contract pass through.
#[test]
fn inspect_conformance_options_validation() {
    let mut ctx = make_ctx();
    run_checks(
        &mut ctx,
        &bundle(
            r#"
        check("bun_inspect_number_options_throws", function() {
            return expectInvalidArgType(function() { Bun.inspect({}, 5); },
                'The "options" argument must be of type object. Received type number (5)');
        });
        check("bun_inspect_string_options_throws", function() {
            return expectInvalidArgType(function() { Bun.inspect({}, "x"); },
                'The "options" argument must be of type object. Received type string (\'x\')');
        });
        check("bun_inspect_boolean_options_throws", function() {
            return expectInvalidArgType(function() { Bun.inspect({}, true); },
                'The "options" argument must be of type object. Received type boolean (true)');
        });
        check("bun_inspect_null_options_ignored", function() {
            return eq(Bun.inspect("x", null), '"x"');
        });
        check("bun_inspect_undefined_options_ignored", function() {
            return eq(Bun.inspect("x", undefined), '"x"');
        });
        check("bun_inspect_depth_zero_root_keys_visible", function() {
            return eq(Bun.inspect({ d: { e: { f: 1 } } }, { depth: 0 }), "{ d: [Object] }");
        });
        check("bun_inspect_depth_infinity_full", function() {
            return eq(Bun.inspect({ a: { b: { c: { d: 1 } } } }, { depth: Infinity }),
                      '{ a: { b: { c: { d: 1 } } } }');
        });
        check("util_inspect_number_options_throws", function() {
            return expectInvalidArgType(function() { util.inspect({}, 5); },
                'The "options" argument must be of type object. Received type number (5)');
        });
        check("util_inspect_null_options_ignored", function() {
            return eq(util.inspect("x", null), "'x'");
        });
        results.join("|")
        "#,
        ),
    );
    bun_runtime::shutdown_thread_sm();
}

/// util.inspect passes `depth` through (Node budget semantics: depth:N caps
/// nested levels — root always renders its keys).
#[test]
fn inspect_conformance_util_inspect_depth_passthrough() {
    let mut ctx = make_ctx();
    run_checks(
        &mut ctx,
        &bundle(
            r#"
        check("util_inspect_default_depth_two", function() {
            return eq(util.inspect({ a: { b: { c: { d: 1 } } } }), "{ a: { b: { c: [Object] } } }");
        });
        check("util_inspect_depth_zero", function() {
            return eq(util.inspect({ a: { b: { c: { d: 1 } } } }, { depth: 0 }), "{ a: [Object] }");
        });
        check("util_inspect_depth_one", function() {
            return eq(util.inspect({ a: { b: { c: { d: 1 } } } }, { depth: 1 }), "{ a: { b: [Object] } }");
        });
        check("util_inspect_depth_infinity", function() {
            return eq(util.inspect({ a: { b: { c: { d: 1 } } } }, { depth: Infinity }),
                      "{ a: { b: { c: { d: 1 } } } }");
        });
        results.join("|")
        "#,
        ),
    );
    bun_runtime::shutdown_thread_sm();
}

/// Sentinel proving the harness evals at all (fresh-context smoke).
#[test]
fn inspect_conformance_harness_smoke() {
    let mut ctx = make_ctx();
    let out = eval_string(&mut ctx, r#"Bun.inspect(1)"#);
    assert_eq!(out, "1");
    bun_runtime::shutdown_thread_sm();
}
