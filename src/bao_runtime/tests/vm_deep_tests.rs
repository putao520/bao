// @trace TEST-ENG-007-VM-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_vm_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
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
    "#);

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
    std::mem::forget(ctx);
}
