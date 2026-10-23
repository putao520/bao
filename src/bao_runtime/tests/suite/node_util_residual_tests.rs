// @trace TEST-ENG-048-UTIL-RESIDUAL [req:REQ-ENG-048] [level:integration]
//
// Regression pin for the node_util residual trio (GOLDEN A / REQ-ENG-048):
//   1. util.getCallSites (node v24 public API — CallSite shape, count
//      validation, options.sourceMap validation),
//   2. util.stripVTControlCharacters (ANSI/VT sequence stripper, oracle
//      regex extracted from the node v24.19.0 binary),
//   3. native util.inspect cycle folding — `[Circular *N]` / `<ref *N>`
//      (R7 root fix: ancestor seen stack ends the exponential back-edge
//      explosion) + depth:Infinity interruption semantics (R7 §8: node
//      renders acyclic deep chains fully; its own stack bound produces the
//      interruption tag, not a fixed-100 truncation).
// Every expectation below was anchored against the real node v24.19.0
// binary before implementation (oracle transcripts in the task report);
// exact-string pins make an oracle drift re-fail here first.
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

fn fresh_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

#[test]
fn test_strip_vt_control_characters_table() {
    let mut ctx = fresh_ctx();
    // All expectations node v24.19.0 oracle-verbatim (util.stripVTControlCharacters).
    let (pass, fail) = run_battery(
        &mut ctx,
        r#"
        var out = [];
        function E(label, got, want) {
            if (got === want) out.push(label + " PASS");
            else out.push(label + " FAIL:got=" + JSON.stringify(got) + " want=" + JSON.stringify(want));
        }
        var strip = require('util').stripVTControlCharacters;
        E("sgrr basic", strip("\x1B[31mred\x1B[39m"), "red");
        E("empty", strip(""), "");
        E("pure seqs", strip("\x1B[31m\x1B[39m"), "");
        E("plain", strip("plain"), "plain");
        E("mixed text", strip("a\x1B[1;32mb\x1B[0mc"), "abc");
        E("osc bel", strip("\x1B]0;title\x07hello"), "hello");
        E("erase seqs", strip("\x1B[2K\x1B[1G\x1B[1A erased"), " erased");
        E("c1 csi", strip("\x9B31mCSI-utf\x9B0m"), "CSI-utf");
        E("unterminated ris", strip("no\x1Bclose"), "nolose");
        E("bare bel bs kept", strip("\x07\x08bell-back"), "\x07\x08bell-back");
        E("sgr rgb", strip("\x1B[38;2;255;100;0mrgb\x1B[0m"), "rgb");
        E("osc st terminator", strip("\x1B]0;t\x1B\\x"), "x");
        E("c1 st terminator", strip("\x1B]0;t\x9Cend"), "end");
        // node v24.19.0 oracle (2026-10-23, local binary, re-run over the
        // full 21-case table): the ansi regex's digit run is capped at 4 and
        // the final byte 'z' (lowercase) is NOT in the final-byte class, so
        // the greedy digit match "31" fails at 'z' and the engine consumes
        // "\x1B[3" only (final byte = the digit '1'), leaving "z". The
        // former pin ("1z") encoded a backtracking semantics node's regex
        // does not have.
        E("digit munch backtrack", strip("\x1B[31z"), "z");
        E("digit run cap", strip("\x1B[12345mX"), "mX");
        // node v24.19.0 oracle (2026-10-23): the unterminated OSC arm's
        // `[[(\]()#;?]*` prefix stops at '0' (not in the class), then the
        // numeric arm consumes "0" + ";t" as its `;\d{0,4}` group with 't'
        // as a valid final byte — the match is exactly "\x1B]0;t", leaving
        // "itle\x1BX". The former pin ("]0;t") inverted kept/removed.
        E("failed outer inner rescan", strip("\x1B]0;title\x1BX"), "itle\u001bX");
        E("cursor private", strip("\x1B[?25lcursor"), "cursor");
        E("charset sel", strip("\x1B(Bcharset"), "charset");
        E("dec line char", strip("\x1B#8dec"), "dec");
        E("bare esc kept", strip("\x1B"), "\x1B");
        E("bare csi kept", strip("\x1B["), "\x1B[");
        out.join("|");
        "#,
    );
    assert_eq!(fail, 0, "stripVT oracle table drifted");
    assert!(pass >= 21, "expected >=21 stripVT pins, got {}", pass);
}

#[test]
fn test_strip_vt_invalid_arg_type() {
    let mut ctx = fresh_ctx();
    let results = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function T(label, fn, wantMsg, wantCode) {
            try { fn(); out.push(label + " FAIL:no-throw"); }
            catch (e) {
                var okMsg = wantMsg === null || (e.message && e.message.indexOf(wantMsg) !== -1);
                var okCode = wantCode === null || e.code === wantCode;
                if (okMsg && okCode) out.push(label + " PASS");
                else out.push(label + " FAIL:code=" + e.code + " msg=" + e.message);
            }
        }
        var strip = require('util').stripVTControlCharacters;
        T("null", function() { strip(null); }, 'The "str" argument must be of type string. Received null', "ERR_INVALID_ARG_TYPE");
        T("number", function() { strip(123); }, 'The "str" argument must be of type string. Received type number (123)', "ERR_INVALID_ARG_TYPE");
        T("undefined", function() { strip(undefined); }, 'must be of type string. Received undefined', "ERR_INVALID_ARG_TYPE");
        out.join("|");
        "#,
    );
    for item in results.split('|') {
        if !item.is_empty() && item.contains(" FAIL") {
            panic!("stripVT validation drift: {}", item);
        }
    }
}

#[test]
fn test_get_call_sites_shape() {
    let mut ctx = fresh_ctx();
    // Structural pins (position-independent): the CallSite object family is
    // null-prototype with exactly the six node v24 keys; frames are the
    // caller chain at current execution points with 1-based lines/columns.
    let (pass, fail) = run_battery(
        &mut ctx,
        r#"
        var out = [];
        function C(label, cond) {
            if (cond) out.push(label + " PASS");
            else out.push(label + " FAIL");
        }
        var util = require('util');
        function inner() { return util.getCallSites(2); }
        var sites = inner();
        C("array", Array.isArray(sites));
        C("len2", sites.length === 2);
        var s0 = sites[0];
        C("proto null", Object.getPrototypeOf(s0) === null);
        C("keys", Object.getOwnPropertyNames(s0).sort().join(",") === "column,columnNumber,functionName,lineNumber,scriptId,scriptName");
        C("fn name", s0.functionName === "inner");
        C("fn name str", typeof s0.functionName === "string");
        C("caller fn empty", sites[1].functionName === "");
        C("line 1-based", s0.lineNumber >= 1);
        C("col 1-based", s0.columnNumber >= 1);
        C("column mirrors", s0.column === s0.columnNumber);
        C("scriptName str", typeof s0.scriptName === "string");
        C("scriptId numeric", typeof s0.scriptId === "string" && /^\d+$/.test(s0.scriptId));
        C("mutable", (s0.extraProp = 1, s0.extraProp === 1));
        C("count upper bound ok", Array.isArray(util.getCallSites(200)));
        C("opts overload slot0", Array.isArray(util.getCallSites({ sourceMap: true })));
        out.join("|");
        "#,
    );
    assert_eq!(fail, 0, "getCallSites CallSite shape drifted");
    assert!(pass >= 15, "expected >=15 shape pins, got {}", pass);
}

#[test]
fn test_get_call_sites_validation() {
    let mut ctx = fresh_ctx();
    let results = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function T(label, fn, wantMsg, wantCode) {
            try { fn(); out.push(label + " FAIL:no-throw"); }
            catch (e) {
                var okMsg = wantMsg === null || (e.message && e.message.indexOf(wantMsg) !== -1);
                var okCode = wantCode === null || e.code === wantCode;
                if (okMsg && okCode) out.push(label + " PASS");
                else out.push(label + " FAIL:code=" + e.code + " msg=" + e.message);
            }
        }
        var util = require('util');
        T("count 0", function() { util.getCallSites(0); }, 'It must be >= 1 && <= 200. Received 0', "ERR_OUT_OF_RANGE");
        T("count -1", function() { util.getCallSites(-1); }, 'It must be >= 1 && <= 200. Received -1', "ERR_OUT_OF_RANGE");
        T("count 201", function() { util.getCallSites(201); }, 'It must be >= 1 && <= 200. Received 201', "ERR_OUT_OF_RANGE");
        T("count 1.5", function() { util.getCallSites(1.5); }, 'It must be an integer. Received 1.5', "ERR_OUT_OF_RANGE");
        T("count NaN", function() { util.getCallSites(NaN); }, 'It must be an integer. Received NaN', "ERR_OUT_OF_RANGE");
        T("count Infinity", function() { util.getCallSites(Infinity); }, 'It must be an integer. Received Infinity', "ERR_OUT_OF_RANGE");
        T("count str", function() { util.getCallSites("x"); }, 'The "frameCount" argument must be of type number. Received type string (\'x\')', "ERR_INVALID_ARG_TYPE");
        T("count null shifts to options", function() { util.getCallSites(null); }, 'The "options" argument must be of type object. Received null', "ERR_INVALID_ARG_TYPE");
        T("opts number", function() { util.getCallSites(undefined, 5); }, 'The "options" argument must be of type object. Received type number (5)', "ERR_INVALID_ARG_TYPE");
        T("opts null", function() { util.getCallSites(undefined, null); }, 'The "options" argument must be of type object. Received null', "ERR_INVALID_ARG_TYPE");
        T("opts sourceMap str", function() { util.getCallSites(undefined, { sourceMap: "yes" }); }, 'The "options.sourceMap" property must be of type boolean. Received type string (\'yes\')', "ERR_INVALID_ARG_TYPE");
        out.join("|");
        "#,
    );
    for item in results.split('|') {
        if !item.is_empty() && item.contains(" FAIL") {
            panic!("getCallSites validation drift: {}", item);
        }
    }
}

#[test]
fn test_inspect_cycle_folding() {
    let mut ctx = fresh_ctx();
    // Exact-string pins — node v24 oracle cycle semantics. Layout note: the
    // native jsval_inspect renders single-line (its long-standing contract);
    // node wraps containers beyond breakLength 80 — the "two cycles" and
    // "nested shared cycle" pins therefore pin bao's single-line form of
    // node's multiline oracle output; the `[Circular *N]` / `<ref *N>`
    // markers and their numbering are node-pinned byte-for-byte:
    //   self-cycle   → <ref *1> { name: 'a', self: [Circular *1] }
    //   3-cycle root → <ref *1> { b: { c: { a: [Circular *1] } } }  (only the
    //                  back-edge TARGET is numbered; middle nodes are not —
    //                  the H4 lazy-numbering discriminator)
    //   two cycles   → <ref *1> ... <ref *2> in first-back-edge order
    //   shared       → rendered twice, no [Circular]
    //   array/map/set self-cycles keep their container heads.
    let (pass, fail) = run_battery(
        &mut ctx,
        r#"
        var out = [];
        function E(label, got, want) {
            if (got === want) out.push(label + " PASS");
            else out.push(label + " FAIL:got=" + JSON.stringify(got) + " want=" + JSON.stringify(want));
        }
        var inspect = require('util').inspect;
        var a = { name: "a" }; a.self = a;
        E("self cycle", inspect(a), "<ref *1> { name: 'a', self: [Circular *1] }");
        var a2 = {}; a2.b = {}; a2.b.c = {}; a2.b.c.a = a2;
        E("3cycle root numbered", inspect(a2), "<ref *1> { b: { c: { a: [Circular *1] } } }");
        var p = {}; var q = {}; p.q = q; q.p = p;
        var r = {}; var s = {}; r.s = s; s.r = r;
        E("two cycles order", inspect({ p: p, r: r }), "{ p: <ref *1> { q: { p: [Circular *1] } }, r: <ref *2> { s: { r: [Circular *2] } } }");
        var shared = { v: 1 };
        E("shared non-cyclic", inspect({ x: shared, y: shared }), "{ x: { v: 1 }, y: { v: 1 } }");
        var arr = [1]; arr.push(arr);
        E("array self", inspect(arr), "<ref *1> [ 1, [Circular *1] ]");
        var m = new Map(); m.set("k", m);
        E("map self", inspect(m), "<ref *1> Map(1) { 'k' => [Circular *1] }");
        var st = new Set(); st.add(st);
        E("set self", inspect(st), "<ref *1> Set(1) { [Circular *1] }");
        var z = {}; z.self = z;
        E("depth0 cycle full", inspect(z, { depth: 0 }), "<ref *1> { self: [Circular *1] }");
        var z2 = {}; z2.self = z2;
        E("depth0 wrapper truncates", inspect({ w: z2 }, { depth: 0 }), "{ w: [Object] }");
        E("plain intact", inspect({ a: 1 }), "{ a: 1 }");
        E("string quoted", inspect("x"), "'x'");
        E("nested shared cycle re-render", (function() {
            var y = {}; y.s = y;
            return inspect({ a: y, b: { c: y } });
        })(), "{ a: <ref *1> { s: [Circular *1] }, b: { c: <ref *1> { s: [Circular *1] } } }");
        out.join("|");
        "#,
    );
    assert_eq!(fail, 0, "native inspect cycle folding drifted from node v24");
    assert!(pass >= 12, "expected >=12 cycle pins, got {}", pass);
}

#[test]
fn test_inspect_depth_infinity_semantics() {
    let mut ctx = fresh_ctx();
    // R7 §8 pins:
    //   * depth: Infinity on a 300-deep ACYCLIC chain renders FULLY
    //     (node oracle: 300 levels, no truncation tag) — the retired
    //     clamp-at-100 would cut it at 100.
    //   * a chain past the native budget interrupts with node's exact tag
    //     (node interrupts at its own engine stack bound the same way).
    //   * a cycle-heavy fan-out graph completes fast (the R7 exponential
    //     back-edge explosion is folded; without the seen stack this battery
    //     would not terminate in budget).
    let (pass, fail) = run_battery(
        &mut ctx,
        r#"
        var out = [];
        function C(label, cond) {
            if (cond) out.push(label + " PASS");
            else out.push(label + " FAIL");
        }
        var inspect = require('util').inspect;
        var cur = {}; var root = cur;
        for (var i = 0; i < 300; i++) { cur.n = {}; cur = cur.n; }
        var full = inspect(root, { depth: Infinity });
        C("acyclic 300 full render", full.split("n: {").length - 1 === 300);
        C("acyclic 300 no truncation tag", full.indexOf("[Object]") === -1);
        C("acyclic 300 no interruption", full.indexOf("Inspection interrupted prematurely") === -1);
        var cur2 = {}; var deep = cur2;
        for (var j = 0; j < 5000; j++) { deep.n = {}; deep = deep.n; }
        var cut = inspect(cur2, { depth: Infinity });
        C("deep chain interrupts with node tag", cut.indexOf("[Object: Inspection interrupted prematurely. Maximum call stack size exceeded.]") !== -1);
        var fan = {}; var last = fan;
        for (var k = 0; k < 200; k++) { last.next = { circular: fan, last: last, obj: { a: 1, b: 2, c: true } }; last = last.next; fan[k] = last; }
        var t0 = Date.now();
        var folded = inspect(fan, { depth: Infinity });
        var ms = Date.now() - t0;
        C("fan-out completes", folded.length > 0);
        C("fan-out folds cycles", folded.indexOf("[Circular *1]") !== -1);
        C("fan-out bounded time", ms < 10000);
        out.join("|");
        "#,
    );
    assert_eq!(fail, 0, "depth:Infinity semantics drifted (R7 §8 regression)");
    assert!(pass >= 6, "expected >=6 depth pins, got {}", pass);
}

#[test]
fn test_format_bigint_arms_still_inspect() {
    let mut ctx = fresh_ctx();
    // The util.format %d/%i BigInt arms now route through the fresh-context
    // inspect helper — pin the pre-existing outputs stayed byte-identical.
    let (pass, fail) = run_battery(
        &mut ctx,
        r#"
        var out = [];
        function E(label, got, want) {
            if (got === want) out.push(label + " PASS");
            else out.push(label + " FAIL:got=" + JSON.stringify(got) + " want=" + JSON.stringify(want));
        }
        var util = require('util');
        E("percent d bigint", util.format("%d", 9n), "9n");
        E("percent i bigint", util.format("%i", 9n), "9n");
        E("percent d int", util.format("%d", 42), "42");
        out.join("|");
        "#,
    );
    assert_eq!(fail, 0, "util.format BigInt arms regressed");
    assert!(pass >= 3, "expected >=3 format pins, got {}", pass);
}
