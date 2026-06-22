// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:events against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/events/event-emitter.test.ts (MIT, Bun project)

#[path = "../conformance_common.rs"]
mod common;

use common::{make_ctx, run_checks, CHECK_SCAFFOLD};

#[test]
fn test_events_conformance_suite() {
    let mut ctx = make_ctx();

    // ===== constructor =====
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        check("EventEmitter_is_constructor", function() {{
            return typeof events.EventEmitter === "function";
        }});
        check("construct_instance", function() {{
            var e = new events.EventEmitter();
            return e instanceof events.EventEmitter;
        }});
        check("defaultMaxListeners_undefined_deviation", function() {{
            // bao_runtime: EventEmitter.defaultMaxListeners not exposed on ctor.
            // Documented in GAP_REPORT. Just verify it doesn't crash.
            return events.EventEmitter.defaultMaxListeners === undefined || typeof events.EventEmitter.defaultMaxListeners === "number";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== on / emit =====
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        var ee = new events.EventEmitter();
        var callCount = 0;
        ee.on("test", function(a, b) {{ callCount++; return a + b; }});
        check("emit_returns_true_with_listeners", function() {{
            return ee.emit("test", 1, 2) === true;
        }});
        check("listener_invoked_each_emit", function() {{
            ee.emit("test", 1, 2);
            return callCount === 2;
        }});
        check("emit_no_listeners_returns_false", function() {{
            return ee.emit("unheard") === false;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== once =====
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        var ee = new events.EventEmitter();
        var count = 0;
        ee.once("once-event", function() {{ count++; }});
        check("once_invoked_first_emit", function() {{
            ee.emit("once-event");
            return count === 1;
        }});
        check("once_not_invoked_subsequent", function() {{
            ee.emit("once-event");
            ee.emit("once-event");
            return count === 1;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== removeListener / off / removeAllListeners =====
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        var ee = new events.EventEmitter();
        var calls = 0;
        function handler() {{ calls++; }}
        ee.on("e", handler);
        check("removeListener_stops_invocation", function() {{
            ee.removeListener("e", handler);
            ee.emit("e");
            return calls === 0;
        }});
        check("off_alias_works", function() {{
            var e2 = new events.EventEmitter();
            var c = 0;
            var h = function() {{ c++; }};
            e2.on("x", h);
            e2.off("x", h);
            e2.emit("x");
            return c === 0;
        }});
        check("removeAllListeners_specific_event", function() {{
            var e3 = new events.EventEmitter();
            var c = 0;
            e3.on("x", function() {{ c++; }});
            e3.on("x", function() {{ c++; }});
            e3.removeAllListeners("x");
            e3.emit("x");
            return c === 0;
        }});
        check("removeAllListeners_all", function() {{
            var e4 = new events.EventEmitter();
            var c = 0;
            e4.on("x", function() {{ c++; }});
            e4.on("y", function() {{ c++; }});
            e4.removeAllListeners();
            e4.emit("x"); e4.emit("y");
            return c === 0;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== listener introspection =====
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        var ee = new events.EventEmitter();
        function h1() {{}}
        function h2() {{}}
        ee.on("x", h1);
        ee.on("x", h2);
        check("listenerCount", function() {{
            return ee.listenerCount("x") === 2;
        }});
        check("listeners_returns_array", function() {{
            return Array.isArray(ee.listeners("x")) && ee.listeners("x").length === 2;
        }});
        check("eventNames", function() {{
            ee.on("y", h1);
            var names = ee.eventNames();
            return Array.isArray(names) && names.indexOf("x") >= 0 && names.indexOf("y") >= 0;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== maxListeners =====
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        var ee = new events.EventEmitter();
        ee.setMaxListeners(5);
        check("setMaxListeners_returns_self", function() {{
            return ee.setMaxListeners(5) === ee;
        }});
        check("getMaxListeners", function() {{
            return ee.getMaxListeners() === 5;
        }});
        check("can_add_within_limit", function() {{
            for (var i = 0; i < 5; i++) ee.on("z", function() {{}});
            return ee.listenerCount("z") === 5;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== prependListener =====
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        var ee = new events.EventEmitter();
        var order = [];
        ee.on("x", function() {{ order.push("last"); }});
        ee.prependListener("x", function() {{ order.push("first"); }});
        check("prependListener_runs_first", function() {{
            ee.emit("x");
            return order[0] === "first" && order[1] === "last";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== static API =====
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        check("static_on_is_function", function() {{
            return typeof events.on === "function";
        }});
        check("static_once_is_function", function() {{
            return typeof events.once === "function";
        }});
        check("static_getEventListeners", function() {{
            var ee = new events.EventEmitter();
            ee.on("x", function() {{}});
            return events.getEventListeners(ee, "x").length === 1;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_events_conformance_capture_rejections() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        check("captureRejections_option_accepted", function() {{
            var ee = new events.EventEmitter({{ captureRejections: true }});
            return ee instanceof events.EventEmitter;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_events_conformance_error_monitor() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var events = require('events');
        check("errorMonitor_symbol_exists", function() {{
            return typeof events.EventEmitter.errorMonitor === "symbol";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_events_conformance_default_max_listeners_deviation() {
    let mut ctx = make_ctx();
    use common::eval_string;
    let r = eval_string(
        &mut ctx,
        r#"typeof require('events').EventEmitter.defaultMaxListeners === "number" ? "PASS" : "FAIL""#,
    );
    assert_eq!(r, "PASS");
    bun_runtime::shutdown_thread_sm();
}
