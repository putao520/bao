// @trace TEST-ENG-7-EVREG [req:REQ-ENG-007] [level:integration]
// E55 delivery-face discriminants for the EMITTER_REGISTRY registry carrier
// (node_events.rs). The `\x00__ee_state` hidden-property carrier was retired
// in favour of a thread-local Rust-side registry keyed by the rooted object
// identity; these tests pin the observable JS-facing contract of that change.
//
// Group map (all six E55 discriminant groups):
//   1. baseline          — real EventEmitter on/emit/off/listenerCount
//   2. foreign_this      — EE.prototype.on.call(plainObj, ...) tolerance
//   3. forgery_inert     — forged `\x00__ee_state` double is a no-op
//   4. enumeration       — getOwnPropertyNames / getOwnPropertyDescriptor
//   5. prototype_chain   — Object.create(EE.prototype) production path
//   6. multi_instance    — two emitters do not share state
//
// GC reclamation face (sweep_dead_emitters) is not exercised here: the suite
// has no major-GC trigger idiom; that face stays soak-observed per the E55
// briefing.

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

#[test]
fn test_node_events_emitter_registry_discriminants() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var events = require('events');
        var EE = events.EventEmitter;
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        // === 1. Baseline: real EventEmitter instance ===
        check("baseline_on_emit", function() {
            var ee = new EE();
            var received = null;
            ee.on("test", function(v) { received = v; });
            ee.emit("test", 42);
            return received === 42;
        });
        check("baseline_off", function() {
            var ee = new EE();
            var count = 0;
            var fn = function() { count++; };
            ee.on("x", fn);
            ee.off("x", fn);
            ee.emit("x");
            return count === 0;
        });
        check("baseline_listenerCount", function() {
            var ee = new EE();
            ee.on("ev", function() {});
            ee.on("ev", function() {});
            return ee.listenerCount("ev") === 2;
        });
        check("baseline_emit_returns", function() {
            var ee = new EE();
            ee.on("x", function() {});
            return ee.emit("x") === true;
        });

        // === 2. Foreign-this tolerance (node oracle shape) ===
        check("foreign_on_returns_this", function() {
            var plain = {};
            var ret = EE.prototype.on.call(plain, "x", function() {});
            return ret === plain;
        });
        check("foreign_on_no_throw", function() {
            var plain = {};
            try { EE.prototype.on.call(plain, "y", function() {}); return true; }
            catch(e) { return false; }
        });
        check("foreign_listenerCount", function() {
            var plain = {};
            EE.prototype.on.call(plain, "z", function() {});
            return EE.prototype.listenerCount.call(plain, "z") === 1;
        });
        check("foreign_emit_false", function() {
            var plain = {};
            return EE.prototype.emit.call(plain, "nothing") === false;
        });

        // === 3. Forged-property inertness (registry does not read the prop) ===
        check("forgery_inert_on_emit", function() {
            var ee = new EE();
            var fired = false;
            ee.on("x", function() { fired = true; });
            ee["\x00__ee_state"] = 999.5;
            ee.emit("x");
            return fired === true;
        });
        check("forgery_inert_listenerCount", function() {
            var ee = new EE();
            ee.on("y", function() {});
            ee["\x00__ee_state"] = 12345.678;
            return ee.listenerCount("y") === 1;
        });
        check("forgery_inert_foreign", function() {
            var o = {};
            o["\x00__ee_state"] = 1.5;
            EE.prototype.on.call(o, "z", function() {});
            return EE.prototype.emit.call(o, "z") === true;
        });

        // === 4. Enumeration surface (prop carrier is gone) ===
        check("no_hidden_prop_names", function() {
            var ee = new EE();
            ee.on("x", function() {});
            var names = Object.getOwnPropertyNames(ee);
            return names.indexOf("\x00__ee_state") === -1;
        });
        check("no_hidden_prop_descriptor", function() {
            var ee = new EE();
            ee.on("x", function() {});
            var desc = Object.getOwnPropertyDescriptor(ee, "\x00__ee_state");
            return desc === undefined;
        });
        check("no_hidden_prop_foreign", function() {
            var plain = {};
            EE.prototype.on.call(plain, "x", function() {});
            var names = Object.getOwnPropertyNames(plain);
            return names.indexOf("\x00__ee_state") === -1;
        });

        // === 5. Prototype-chain form (node_net production path) ===
        check("proto_chain_on_emit", function() {
            var proto = Object.create(EE.prototype);
            var fired = false;
            EE.prototype.on.call(proto, "test", function(v) { fired = v; });
            var result = proto.emit("test", true);
            return result === true && fired === true;
        });
        check("proto_chain_listenerCount", function() {
            var proto = Object.create(EE.prototype);
            EE.prototype.on.call(proto, "ev", function() {});
            return proto.listenerCount("ev") === 1;
        });
        check("proto_chain_off", function() {
            var proto = Object.create(EE.prototype);
            var count = 0;
            var fn = function() { count++; };
            EE.prototype.on.call(proto, "x", fn);
            proto.off("x", fn);
            proto.emit("x");
            return count === 0;
        });

        // === 6. Multi-instance isolation ===
        check("multi_instance_isolated", function() {
            var ee1 = new EE();
            var ee2 = new EE();
            var count1 = 0, count2 = 0;
            ee1.on("ev", function() { count1++; });
            ee2.on("ev", function() { count2++; });
            ee1.emit("ev");
            return count1 === 1 && count2 === 0;
        });
        check("multi_instance_remove_isolated", function() {
            var ee1 = new EE();
            var ee2 = new EE();
            var fn1 = function() {};
            var fn2 = function() {};
            ee1.on("x", fn1);
            ee2.on("x", fn2);
            ee1.off("x", fn1);
            return ee1.listenerCount("x") === 0 && ee2.listenerCount("x") === 1;
        });

        results.join("|")
    "#,
    );

    let mut all_passed = true;
    // Fake-green guard: the eval helper maps a thrown script or a non-string
    // return to "", so an empty battery must fail loudly instead of letting
    // the split loop below pass vacuously.
    assert!(
        !results.is_empty(),
        "battery probe produced no results (uncaught throw or non-string return)"
    );
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "All emitter-registry discriminant tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}
