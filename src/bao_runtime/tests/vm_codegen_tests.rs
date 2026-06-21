// @trace TEST-ENG-011 [req:REQ-ENG-011] [level:integration]
// REQ-ENG-011 criterion 8: codeGeneration option disables eval/function compilation.
// Also re-verifies criteria 5 (sandbox injection) and 6 (sandbox isolation)
// because the codeGeneration path shares the createContext/runInNewContext
// surface and must not regress the core sandbox semantics.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "object".to_string(),
        Err(e) => format!("ERR:{}", e),
    }
}

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        _ => false,
    }
}

#[test]
fn test_vm_codegen_strings_disabled() {
    // REQ-ENG-011 criterion 8: codeGeneration: { strings: false } blocks eval/Function.
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // eval should throw when strings:false.
    let eval_blocked = eval_string(&mut ctx, r#"
        var vm = require('vm');
        var blocked = false;
        try {
            vm.runInNewContext('eval("1+1")', {}, { codeGeneration: { strings: false, wasm: false } });
        } catch (e) {
            blocked = true;
        }
        blocked ? "EVAL_BLOCKED" : "EVAL_ALLOWED"
    "#);
    assert!(
        eval_blocked.contains("EVAL_BLOCKED"),
        "eval should be blocked when codeGeneration.strings=false, got: {}",
        eval_blocked
    );

    // new Function should also throw when strings:false.
    let fn_blocked = eval_string(&mut ctx, r#"
        var vm = require('vm');
        var blocked = false;
        try {
            vm.runInNewContext('new Function("return 1")()', {}, { codeGeneration: { strings: false } });
        } catch (e) {
            blocked = true;
        }
        blocked ? "FN_BLOCKED" : "FN_ALLOWED"
    "#);
    assert!(
        fn_blocked.contains("FN_BLOCKED"),
        "new Function should be blocked when codeGeneration.strings=false, got: {}",
        fn_blocked
    );

    // Function(...) direct call should also throw.
    let fn_direct_blocked = eval_string(&mut ctx, r#"
        var vm = require('vm');
        var blocked = false;
        try {
            vm.runInNewContext('Function("return 1")()', {}, { codeGeneration: { strings: false } });
        } catch (e) {
            blocked = true;
        }
        blocked ? "FN_DIRECT_BLOCKED" : "FN_DIRECT_ALLOWED"
    "#);
    assert!(
        fn_direct_blocked.contains("FN_DIRECT_BLOCKED"),
        "Function() direct call should be blocked, got: {}",
        fn_direct_blocked
    );

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_vm_codegen_strings_allowed_by_default() {
    // REQ-ENG-011 criterion 8: without codeGeneration restriction, eval/Function work.
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let eval_result = eval_string(&mut ctx, r#"
        var vm = require('vm');
        var r = vm.runInNewContext('eval("2+3")', {}, {});
        String(r)
    "#);
    assert!(
        eval_result == "5",
        "eval should work without restriction, got: {}",
        eval_result
    );

    // Explicitly allowed.
    let allowed = eval_string(&mut ctx, r#"
        var vm = require('vm');
        var ok = false;
        try {
            var r = vm.runInNewContext('eval("10*2")', {}, { codeGeneration: { strings: true } });
            ok = (r === 20);
        } catch (e) { ok = false; }
        ok ? "OK" : "FAIL"
    "#);
    assert!(
        allowed.contains("OK"),
        "eval should work when strings=true, got: {}",
        allowed
    );

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_vm_sandbox_injection_criterion5() {
    // REQ-ENG-011 criterion 5: sandbox variable injection.
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let result = eval_string(&mut ctx, r#"
        var vm = require('vm');
        var r = vm.runInNewContext('typeof x === "number" ? "sandbox_ok" : "sandbox_fail"', { x: 42 });
        String(r)
    "#);
    assert_eq!(
        result, "sandbox_ok",
        "criterion 5: sandbox injection should make x available, got: {}",
        result
    );

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_vm_sandbox_isolation_criterion6() {
    // REQ-ENG-011 criterion 6: variables defined in runInNewContext
    // do NOT leak into the caller's realm.
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let isolated = eval_bool(&mut ctx, r#"
        var vm = require('vm');
        // Define a global in the sandbox realm.
        vm.runInNewContext('__bao_sandbox_leak = 999', {});
        // In the caller realm, the leak must NOT be visible.
        (typeof __bao_sandbox_leak === 'undefined')
    "#);
    assert!(
        isolated,
        "criterion 6: sandbox global must not leak into caller realm"
    );

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_vm_create_context_then_run_in_context_codegen() {
    // REQ-ENG-011 criterion 8 via createContext + Script.runInContext:
    // restrictions persist on the contextified sandbox.
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let result = eval_string(&mut ctx, r#"
        var vm = require('vm');
        // Create a context with strings disabled.
        var sandbox = vm.createContext({}, { codeGeneration: { strings: false } });
        var blocked = false;
        try {
            var s = new vm.Script('eval("1+1")');
            s.runInContext(sandbox);
        } catch (e) {
            blocked = true;
        }
        blocked ? "PERSISTENT_BLOCK" : "NO_BLOCK"
    "#);
    assert!(
        result.contains("PERSISTENT_BLOCK"),
        "criterion 8: codeGeneration restriction must persist via Script.runInContext, got: {}",
        result
    );

    bun_runtime::shutdown_thread_sm();
}
