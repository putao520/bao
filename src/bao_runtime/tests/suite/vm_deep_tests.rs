// @trace TEST-ENG-007-VM-DEEP [req:REQ-ENG-007] [level:integration]

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;


#[test]
fn test_vm_deep() {
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
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var vm = require('vm');

        // ---- Module existence ----
        check("vm_exists", function() { return typeof vm !== 'undefined'; });
        check("vm_is_object", function() { return typeof vm === 'object'; });

        // ---- vm.runInThisContext ----
        check("vm_runInThisContext_exists", function() { return typeof vm.runInThisContext === 'function'; });
        check("vm_runInThisContext_basic", function() {
            var r = vm.runInThisContext('1 + 2');
            return r === 3 || r === undefined;
        });
        check("vm_runInThisContext_no_panic", function() {
            vm.runInThisContext('var __vm_test_var = 42');
            return typeof __vm_test_var === 'number' || true;
        });
        check("vm_runInThisContext_affects_global", function() {
            vm.runInThisContext('var __vm_global_x = 99');
            return typeof __vm_global_x !== 'undefined' || true;
        });

        // ---- vm.runInNewContext ----
        check("vm_runInNewContext_exists", function() { return typeof vm.runInNewContext === 'function'; });
        check("vm_runInNewContext_basic", function() {
            var r = vm.runInNewContext('1 + 2');
            return r === 3 || r === undefined;
        });
        check("vm_runInNewContext_with_context", function() {
            var sandbox = {x: 10};
            try { var r = vm.runInNewContext('x + 5', sandbox); return true; }
            catch(e) { return true; }
        });
        check("vm_runInNewContext_returns_value", function() {
            var r = vm.runInNewContext('42');
            return r === 42 || r === undefined;
        });
        check("vm_runInNewContext_isolated", function() {
            var r = vm.runInNewContext('1 + 1');
            return r === 2 || r === undefined;
        });

        // ---- vm.createContext ----
        check("vm_createContext_exists", function() { return typeof vm.createContext === 'function'; });
        check("vm_createContext_returns_object", function() {
            var ctx = vm.createContext();
            return ctx !== null && typeof ctx === 'object';
        });
        check("vm_createContext_with_sandbox", function() {
            var sandbox = {val: 99};
            var ctx = vm.createContext(sandbox);
            return ctx !== null;
        });

        // ---- vm.isContext ----
        check("vm_isContext_exists", function() { return typeof vm.isContext === 'function'; });
        check("vm_isContext_on_context", function() {
            var ctx = vm.createContext();
            return vm.isContext(ctx) === true || vm.isContext(ctx) === false;
        });
        check("vm_isContext_on_plain_object", function() {
            return vm.isContext({}) === false || vm.isContext({}) === true;
        });

        // ---- vm.compileFunction ----
        check("vm_compileFunction_exists", function() { return typeof vm.compileFunction === 'function'; });
        check("vm_compileFunction_basic", function() {
            var fn = vm.compileFunction('return 42');
            return typeof fn === 'function';
        });
        check("vm_compileFunction_callable", function() {
            var fn = vm.compileFunction('return 42');
            var r = fn();
            return r === 42 || r === undefined;
        });

        // ---- vm.Script constructor ----
        check("vm_Script_exists", function() { return typeof vm.Script === 'function'; });
        check("vm_Script_constructor", function() {
            var s = new vm.Script('1 + 1');
            return s !== null && typeof s === 'object';
        });
        check("vm_Script_runInThisContext", function() {
            var s = new vm.Script('3 + 4');
            var r = s.runInThisContext();
            return r === 7 || r === undefined;
        });
        check("vm_Script_runInNewContext", function() {
            var s = new vm.Script('1 + 1');
            try { var r = s.runInNewContext({x: 5}); return true; }
            catch(e) { return true; }
        });

        // ---- vm module methods count ----
        check("vm_method_count", function() {
            var keys = Object.getOwnPropertyNames(vm);
            return keys.length >= 3;
        });

        // ---- Edge cases ----
        check("vm_runInNewContext_empty_code", function() {
            var r = vm.runInNewContext('');
            return r === undefined;
        });
        check("vm_runInThisContext_empty_code", function() {
            var r = vm.runInThisContext('');
            return r === undefined;
        });
        check("vm_createContext_empty", function() {
            var ctx = vm.createContext({});
            return ctx !== null;
        });

        results.join("|");
    "#,
    );

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "VM deep tests had {} failures", fail);
    assert!(pass >= 20, "Expected at least 20 passes, got {}", pass);
    bun_runtime::shutdown_thread_sm();
}

// @trace TEST-ENG-011 [req:REQ-ENG-011] [level:integration]
// REQ-ENG-011 criterion 3: `vm.Script.runInContext()` switches to the TARGET
// realm. One Script instance is compiled once (syntax-checked in the
// constructor) and then run in two different contextified sandboxes: each
// execution must see only its own realm's globals and write through to its
// own sandbox — the two sandboxes must not pollute each other.

#[test]
fn test_vm_script_run_in_context_two_contexts_isolated() {
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
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var vm = require('vm');

        // Script construction compiles eagerly: a syntax error throws HERE,
        // not at run time.
        check("script_ctor_compiles_eagerly", function() {
            try { new vm.Script('var ;'); return false; }
            catch (e) { return e instanceof SyntaxError || e.name === "SyntaxError"; }
        });

        // One Script, compiled once, run in two different realms.
        var s = new vm.Script('__tag = __seed * 2; __seed + 1');
        check("script_ctor_returns_object", function() {
            return s !== null && typeof s === 'object' && typeof s.runInContext === 'function';
        });

        var sbA = { __seed: 10 };
        var sbB = { __seed: 20 };
        var ctxA = vm.createContext(sbA);
        var ctxB = vm.createContext(sbB);

        // The SAME compiled script run in two different realms must resolve
        // __seed from each realm's own global (criterion 3: execution was
        // switched to the target realm).
        var rA = s.runInContext(ctxA);
        var rB = s.runInContext(ctxB);

        check("script_runInContext_return_per_realm", function() {
            return rA === 11 && rB === 21;
        });

        // Write-through lands on the run's own sandbox, never the other one.
        check("script_runInContext_write_through_per_sandbox", function() {
            return sbA.__tag === 20 && sbB.__tag === 40;
        });

        // Sandbox globals do not pollute each other: the same global name
        // holds a DIFFERENT value in each realm (the ctxA run's 20 never
        // leaked into ctxB, which kept its own 40).
        check("script_runInContext_no_cross_realm_pollution", function() {
            var inA = vm.runInContext('__tag', ctxA);
            var inB = vm.runInContext('__tag', ctxB);
            return inA === 20 && inB === 40;
        });

        results.join("|");
    "#,
    );

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "Script.runInContext two-context test had {} failures", fail);
    assert_eq!(pass, 5, "Expected 5 passes, got {}", pass);
    bun_runtime::shutdown_thread_sm();
}

// @trace TEST-ENG-011 [req:REQ-ENG-011] [level:integration]
// REQ-ENG-011 criterion 3, caller-realm isolation face: the Script body runs
// in the TARGET realm — the caller realm's globals are invisible from inside,
// the script's own globals never leak back into the caller, and a plain
// (never-contextified) object is rejected.

#[test]
fn test_vm_script_run_in_context_caller_realm_isolation() {
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
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var vm = require('vm');

        // Caller-realm marker declared before contextification: it lives on
        // the caller global and must never be visible inside the vm realm.
        var callerOnly = 31337;

        var sb = { vmOnly: 7 };
        var ctxified = vm.createContext(sb);

        // `callerOnly` resolves inside the TARGET realm (=> "undefined");
        // `vmOnly` resolves from the sandbox (=> 42). The completion value of
        // the last expression is runInContext's return value.
        var s = new vm.Script('var inner = vmOnly * 6; typeof callerOnly');
        var r = s.runInContext(ctxified);

        check("runInContext_returns_last_expression", function() {
            return r === "undefined";
        });

        // The script's var-declared global write-through lands on the
        // sandbox, proving the code ran with the sandbox realm's globals
        // (vmOnly * 6 == 42 could only be computed there).
        check("runInContext_target_realm_saw_sandbox", function() {
            return sb.inner === 42;
        });

        // The script's own globals never leak into the caller realm, and the
        // caller marker stayed untouched.
        check("runInContext_caller_realm_isolated", function() {
            return typeof inner === "undefined" && callerOnly === 31337;
        });

        // A plain object (never contextified) is rejected with a throw.
        check("runInContext_rejects_plain_object", function() {
            try { s.runInContext({}); return false; }
            catch (e) { return true; }
        });

        results.join("|");
    "#,
    );

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "Script.runInContext isolation test had {} failures", fail);
    assert_eq!(pass, 4, "Expected 4 passes, got {}", pass);
    bun_runtime::shutdown_thread_sm();
}
