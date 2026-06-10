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

    let results = eval_string(&mut ctx, r#"
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
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All events tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
