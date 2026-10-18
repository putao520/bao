// @trace REQ-ENG-27 [level:integration]
// node:repl module surface (REQ-ENG-27) — observes the shipped behavior of
// `src/bao_runtime/src/node_repl.rs` (a Bun-parity stub: the module object
// exposes start/REPLServer/builtinModules so importers do not crash; the
// interactive REPL itself is not implemented — `start()` throws).
//
// @trace REQ-ENG-006 [api:node:repl]

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

#[test]
fn test_node_repl_surface() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).toString().substring(0, 80)); }
        }

        // === C1: require('repl') does not throw ===
        var repl;
        check("require_repl_no_throw", function() {
            repl = require('repl');
            return repl !== null && typeof repl === 'object';
        });
        check("require_node_prefix", function() {
            var r2 = require('node:repl');
            return typeof r2 === 'object' && r2 !== null;
        });

        // === surface shape (mirrors Bun repl.ts stub) ===
        check("repl_start_function", function() { return typeof repl.start === 'function'; });
        check("repl_REPLServer_function", function() { return typeof repl.REPLServer === 'function'; });
        check("repl_builtinModules_array", function() {
            return Array.isArray(repl.builtinModules) && repl.builtinModules.length > 0;
        });
        check("repl_builtinModules_has_repl", function() {
            return repl.builtinModules.indexOf('repl') !== -1;
        });

        // === honest stub behavior: start() throws not-implemented ===
        // (Bao does not ship an interactive REPL; the module exists for
        // import-compatibility, like Bun's stub.)
        check("repl_start_throws_not_implemented", function() {
            try { repl.start(); return false; }
            catch(e) {
                var msg = (e.message || e).toString();
                return msg.indexOf('not implemented') !== -1 || msg.indexOf('REPL') !== -1;
            }
        });

        results.join("|")
    "#,
    );

    let mut pass = 0;
    let mut fail = 0;
    // Fake-green guard: the eval helper maps a thrown script or a non-string
    // return to "", so an empty battery must fail loudly instead of letting
    // the split loop below pass vacuously.
    assert!(
        !results.is_empty(),
        "battery probe produced no results (uncaught throw or non-string return)"
    );
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "node:repl surface tests had {} failures", fail);
    assert!(pass >= 6, "Expected at least 6 passes, got {}", pass);

    bun_runtime::shutdown_thread_sm();
}
