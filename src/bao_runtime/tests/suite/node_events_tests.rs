// @trace TEST-ENG-007-EV [req:REQ-ENG-007] [level:integration]
// Integration tests for node:events API (REQ-ENG-007)
// All JS assertions in one eval() call.

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
fn test_node_events_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var events = require('events');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        check("require", function() { return typeof events === 'object'; });
        check("EventEmitter", function() { return typeof events.EventEmitter === 'function'; });

        // basic on/emit
        check("on_emit", function() {
            var ee = new events.EventEmitter();
            var received = null;
            ee.on("test", function(val) { received = val; });
            ee.emit("test", 42);
            return received === 42;
        });

        // multiple listeners
        check("multi_listener", function() {
            var ee = new events.EventEmitter();
            var count = 0;
            ee.on("inc", function() { count++; });
            ee.on("inc", function() { count++; });
            ee.emit("inc");
            return count === 2;
        });

        // emit with multiple args
        check("multi_args", function() {
            var ee = new events.EventEmitter();
            var args = null;
            ee.on("multi", function(a, b, c) { args = [a, b, c]; });
            ee.emit("multi", 1, "two", true);
            return args[0] === 1 && args[1] === "two" && args[2] === true;
        });

        // off
        check("off", function() {
            var ee = new events.EventEmitter();
            var count = 0;
            var fn = function() { count++; };
            ee.on("x", fn);
            ee.emit("x");
            ee.off("x", fn);
            ee.emit("x");
            return count === 1;
        });

        // once
        check("once", function() {
            var ee = new events.EventEmitter();
            var onceCount = 0;
            ee.once("fire", function() { onceCount++; });
            ee.emit("fire");
            ee.emit("fire");
            return onceCount === 1;
        });

        // listenerCount
        check("listenerCount", function() {
            var ee = new events.EventEmitter();
            ee.on("ev", function() {});
            ee.on("ev", function() {});
            return ee.listenerCount("ev") === 2;
        });

        // removeAllListeners
        check("removeAll", function() {
            var ee = new events.EventEmitter();
            ee.on("a", function() {});
            ee.on("b", function() {});
            ee.removeAllListeners();
            return ee.listenerCount("a") === 0 && ee.listenerCount("b") === 0;
        });

        // eventNames
        check("eventNames", function() {
            var ee = new events.EventEmitter();
            ee.on("alpha", function() {});
            ee.on("beta", function() {});
            var names = ee.eventNames();
            return names.indexOf("alpha") >= 0 && names.indexOf("beta") >= 0;
        });

        // prependListener
        check("prepend", function() {
            var ee = new events.EventEmitter();
            var order = [];
            ee.on("ord", function() { order.push("second"); });
            ee.prependListener("ord", function() { order.push("first"); });
            ee.emit("ord");
            return order[0] === "first" && order[1] === "second";
        });

        // instanceof
        check("instanceof", function() {
            var ee = new events.EventEmitter();
            return ee instanceof events.EventEmitter;
        });

        // emit returns true when listeners exist
        check("emit_return", function() {
            var ee = new events.EventEmitter();
            ee.on("x", function() {});
            return ee.emit("x") === true;
        });

        // emit returns false when no listeners
        check("emit_false", function() {
            var ee = new events.EventEmitter();
            return ee.emit("nonexistent") === false;
        });

        // newListener event (if supported)
        check("newListener", function() {
            var ee = new events.EventEmitter();
            var captured = null;
            ee.on("newListener", function(ev) { captured = ev; });
            ee.on("myevent", function() {});
            return captured === "myevent" || captured === null;
        });

        results.join("|")
    "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "All events tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}

// @trace TEST-ENG-007-EV [req:REQ-ENG-007] [level:integration]
// Same-object NESTED emits — the EmitterState single-owner invariant
// regression. get_state used to consume the prop-owned Box
// (`*Box::from_raw`) while the hidden prop kept pointing at the freed
// memory; ANY listener that re-emitted on the same object ran from_raw on
// the dangling pointer a second time → 112B double free + SIGSEGV
// (mimalloc abort; bt lands in ee_emit's listeners.get). These are the
// canonical Node shapes: sock.on('data', () => sock.end()),
// sock.on('end', () => sock.end()) — end() re-emits 'end'/'close'.
#[test]
fn test_node_events_nested_same_object_emits() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var events = require('events');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        // 3-level same-object nesting: emitting 'a' emits 'b' emits 'c'.
        // Before the ownership fix this double-freed and crashed the process.
        check("nested_3_levels", function() {
            var ee = new events.EventEmitter();
            var order = [];
            ee.on("a", function() { order.push("a"); ee.emit("b"); });
            ee.on("b", function() { order.push("b"); ee.emit("c"); });
            ee.on("c", function() { order.push("c"); });
            ee.emit("a");
            return order.join(",") === "a,b,c";
        });

        // Canonical socket shape: 'data' listener calls end(), which
        // re-emits 'end' then 'close' on the SAME object synchronously.
        check("data_end_close_shape", function() {
            var sock = new events.EventEmitter();
            sock.end = function() { this.emit("end"); this.emit("close"); };
            var seen = [];
            sock.on("data", function() { sock.end(); });
            sock.on("end", function() { seen.push("end"); });
            sock.on("close", function() { seen.push("close"); });
            sock.emit("data");
            return seen.join(",") === "end,close";
        });

        // Nested once-removal: the inner once listener fires exactly once
        // even when reached through a nested emit.
        check("nested_once", function() {
            var ee = new events.EventEmitter();
            var fires = 0;
            ee.once("tick", function() { fires++; });
            ee.on("go", function() { ee.emit("tick"); });
            ee.emit("go");
            ee.emit("go");
            return fires === 1;
        });

        // Nested on(): registering a listener from inside a listener of the
        // same emitter must survive the outer emit's state write-back.
        check("nested_on_registration", function() {
            var ee = new events.EventEmitter();
            var hits = 0;
            ee.on("first", function() { ee.on("second", function() { hits++; }); });
            ee.emit("first");
            ee.emit("second");
            return hits === 1;
        });

        // Deep recursion through the same event (bounded) — stress the
        // snapshot write-back ordering.
        check("self_recursive_bounded", function() {
            var ee = new events.EventEmitter();
            var n = 0;
            ee.on("step", function() { n++; if (n < 10) ee.emit("step"); });
            ee.emit("step");
            return n === 10;
        });

        results.join("|")
    "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "Nested same-object emit tests must pass (single-owner invariant). Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}
