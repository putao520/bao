// @trace TEST-ENG-011-EDGE [req:REQ-ENG-011] [level:integration]
//
// C8b line-coverage wave: node_vm.rs uncovered branches — Node validateObject
// ERR_INVALID_ARG_TYPE faces on every entry point, string-shorthand filename
// options, Script constructor (syntax-error-at-compile, options validation,
// without-new fallback), Script.runIn* methods, compileFunction (params /
// string name / options validation / syntax error), codeGeneration policy
// (strings / wasm), context idempotency, and both write-through directions.

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full;

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn run_checks(ctx: &mut JsContext, body: &str) -> String {
    let wrapped = format!(
        r#"
        var __results = [];
        function check(label, fn) {{
            try {{ var ok = fn(); __results.push(label + (ok ? " PASS" : " FAIL")); }}
            catch(e) {{ __results.push(label + " ERR:" + (e.message || e).toString().substring(0, 90)); }}
        }}
        {body}
        __results.join("\n");
        "#
    );
    eval_string_full(ctx, &wrapped)
}

fn assert_all_pass(results: &str) {
    assert!(
        !results.contains("FAIL") && !results.contains("ERR:"),
        "vm edge checks failed:\n{}",
        results
    );
}

#[test]
fn vm_options_validation_matrix() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var vm = require('vm');
        function catchErr(fn) { try { fn(); return null; } catch (e) { return e; } }

        // Node validateObject: null / arrays / functions / primitives throw
        // ERR_INVALID_ARG_TYPE; a string is the filename shorthand (ok).
        check("runInThisContext_null_options_throws", function() {
            var e = catchErr(function() { vm.runInThisContext('1', null); });
            return e instanceof TypeError && e.code === 'ERR_INVALID_ARG_TYPE'
                && e.message.indexOf('options') !== -1
                && e.message.indexOf('Received null') !== -1;
        });
        check("runInThisContext_array_options_throws", function() {
            var e = catchErr(function() { vm.runInThisContext('1', [1]); });
            return e.code === 'ERR_INVALID_ARG_TYPE'
                && e.message.indexOf('an instance of Array') !== -1;
        });
        check("runInThisContext_function_options_throws", function() {
            var e = catchErr(function() { vm.runInThisContext('1', function myOpt() {}); });
            return e.code === 'ERR_INVALID_ARG_TYPE'
                && e.message.indexOf('function myOpt') !== -1;
        });
        check("runInThisContext_number_options_throws", function() {
            var e = catchErr(function() { vm.runInThisContext('1', 5); });
            return e.code === 'ERR_INVALID_ARG_TYPE'
                && e.message.indexOf('type number (5)') !== -1;
        });
        check("runInThisContext_bool_options_throws", function() {
            var e = catchErr(function() { vm.runInThisContext('1', true); });
            return e.code === 'ERR_INVALID_ARG_TYPE'
                && e.message.indexOf('type boolean (true)') !== -1;
        });
        check("runInThisContext_string_options_filename_shorthand", function() {
            return vm.runInThisContext('2 + 3', 'shorthand.js') === 5;
        });
        check("runInThisContext_undefined_options_defaults", function() {
            return vm.runInThisContext('6 * 7', undefined) === 42;
        });
        check("runInThisContext_object_filename_in_stack", function() {
            var e = catchErr(function() { vm.runInThisContext('null.x', {filename: 'edge-vm-file.js'}); });
            return e instanceof TypeError
                && String(e.stack).indexOf('edge-vm-file.js') !== -1;
        });

        // runInNewContext spreads non-plain options (array/function ok) and
        // takes the string shorthand; only createContext-level validation is
        // bypassed by the spread.
        check("runInNewContext_array_options_ok", function() {
            return vm.runInNewContext('4 + 4', {}, [1]) === 8;
        });
        check("runInNewContext_function_options_ok", function() {
            return vm.runInNewContext('5 + 5', {}, function opt() {}) === 10;
        });
        check("runInNewContext_string_options_ok", function() {
            return vm.runInNewContext('1 + 1', {}, 's.js') === 2;
        });
        check("runInNewContext_filename_in_stack", function() {
            var e = catchErr(function() { vm.runInNewContext('null.y', {}, {filename: 'rinc-file.js'}); });
            return String(e.stack).indexOf('rinc-file.js') !== -1;
        });

        // Non-string code argument rejects on every entry point.
        check("runInThisContext_non_string_code_throws", function() {
            var e = catchErr(function() { vm.runInThisContext(5); });
            return e !== null && String(e.message).indexOf('code string') !== -1;
        });
        check("runInNewContext_non_string_code_throws", function() {
            var e = catchErr(function() { vm.runInNewContext(5, {}); });
            return e !== null && String(e.message).indexOf('code string') !== -1;
        });
        check("runInContext_non_string_code_throws", function() {
            var e = catchErr(function() { vm.runInContext(5, {}); });
            return e !== null && String(e.message).indexOf('code string') !== -1;
        });
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn vm_script_constructor_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var vm = require('vm');
        function catchErr(fn) { try { fn(); return null; } catch (e) { return e; } }

        // Syntax errors throw at CONSTRUCTION time (Node contract), with the
        // options filename on the stack.
        check("script_syntax_error_at_ctor", function() {
            var e = catchErr(function() { new vm.Script('1 +'); });
            return e instanceof SyntaxError;
        });
        check("script_ctor_parse_error_is_syntaxerror", function() {
            // The ctor compiles with the options filename in effect; SM does
            // not surface the filename on a parse error's .stack (runtime
            // errors do — see the flows_to_runInNewContext check below).
            var e = catchErr(function() { new vm.Script('2 *', {filename: 'ctor-file.js'}); });
            return e instanceof SyntaxError;
        });
        check("script_ctor_non_string_code_throws", function() {
            var e = catchErr(function() { new vm.Script(5); });
            return String(e.message).indexOf('code string') !== -1;
        });
        check("script_ctor_invalid_options_throws", function() {
            var e = catchErr(function() { new vm.Script('1', 5); });
            return e.code === 'ERR_INVALID_ARG_TYPE';
        });
        check("script_ctor_ok_with_filename", function() {
            var s = new vm.Script('3 + 4', {filename: 'ok.js'});
            return typeof s === 'object' && s !== null;
        });

        // Script methods.
        check("script_runInThisContext_value", function() {
            return new vm.Script('10 + 10').runInThisContext() === 20;
        });
        check("script_runInThisContext_options_validation", function() {
            var s = new vm.Script('1');
            var e = catchErr(function() { s.runInThisContext(5); });
            return e.code === 'ERR_INVALID_ARG_TYPE';
        });
        check("script_runInContext_write_through", function() {
            var sb = {};
            vm.createContext(sb);
            var s = new vm.Script('svar = 11; svar + 1');
            var v = s.runInContext(sb);
            return v === 12 && sb.svar === 11;
        });
        check("script_runInContext_non_contextified_throws", function() {
            var s = new vm.Script('1');
            var e = catchErr(function() { s.runInContext({}); });
            return String(e.message).indexOf('not a contextified object') !== -1;
        });
        check("script_runInContext_missing_sandbox_throws", function() {
            var s = new vm.Script('1');
            var e = catchErr(function() { s.runInContext(); });
            return String(e.message).indexOf('contextified sandbox') !== -1;
        });
        check("script_runInContext_options_validation", function() {
            var sb = {};
            vm.createContext(sb);
            var s = new vm.Script('1');
            var e = catchErr(function() { s.runInContext(sb, 5); });
            return e.code === 'ERR_INVALID_ARG_TYPE';
        });
        check("script_runInNewContext_value_and_writeback", function() {
            var sb = {};
            var s = new vm.Script('nvar = 21; nvar');
            var v = s.runInNewContext(sb);
            return v === 21 && sb.nvar === 21;
        });
        check("script_runInNewContext_filename_from_ctor_options", function() {
            // The eval filename comes from the Script INSTANCE's ctor-time
            // `__filename`, not the per-run options (divergence vs node —
            // registered in the C8b report): assert the ctor default.
            var e = catchErr(function() {
                new vm.Script('null.z').runInNewContext({}, {filename: 'ignored.js'});
            });
            var stack = String(e && e.stack || '');
            if (stack.indexOf('vm.js') === -1) throw new Error('GOT-STACK:' + stack);
            return true;
        });
        check("script_ctor_filename_flows_to_runInNewContext", function() {
            var e = catchErr(function() {
                new vm.Script('null.z', {filename: 'ctor-opt.js'}).runInNewContext({});
            });
            var stack = String(e && e.stack || '');
            if (stack.indexOf('ctor-opt.js') === -1) throw new Error('GOT-STACK:' + stack);
            return true;
        });
        check("script_runInNewContext_options_validation", function() {
            var s = new vm.Script('1');
            var e = catchErr(function() { s.runInNewContext({}, 5); });
            return e.code === 'ERR_INVALID_ARG_TYPE';
        });

        // (The ctor's without-`new` fallback arms are dead from JS: Spider-
        // Monkey supplies the global object as `this` for sloppy constructor
        // calls, so `this_val.is_object()` always holds. Honest boundary —
        // registered in the C8b report.)
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn vm_compile_function_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var vm = require('vm');
        function catchErr(fn) { try { fn(); return null; } catch (e) { return e; } }

        check("compileFunction_params_array", function() {
            var f = vm.compileFunction('return a + b', ['a', 'b']);
            return typeof f === 'function' && f(1, 2) === 3;
        });
        check("compileFunction_string_name_extension", function() {
            var f = vm.compileFunction('return 1', 'myName');
            return f.name === 'myName' && f() === 1;
        });
        check("compileFunction_anonymous_default", function() {
            var f = vm.compileFunction('return 2');
            return f() === 2 && (f.name === 'anonymous' || f.name === '');
        });
        check("compileFunction_syntax_error_throws", function() {
            var e = catchErr(function() { vm.compileFunction('1 +', []); });
            return e instanceof SyntaxError;
        });
        check("compileFunction_invalid_options_throws", function() {
            var e = catchErr(function() { vm.compileFunction('return 1', [], 5); });
            return e.code === 'ERR_INVALID_ARG_TYPE';
        });
        check("compileFunction_undefined_options_ok", function() {
            var f = vm.compileFunction('return 3', [], undefined);
            return f() === 3;
        });
        check("compileFunction_non_string_code_throws", function() {
            var e = catchErr(function() { vm.compileFunction(5); });
            return String(e.message).indexOf('code string') !== -1;
        });
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn vm_code_generation_policy() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var vm = require('vm');
        function catchErr(fn) { try { fn(); return null; } catch (e) { return e; } }

        // strings: false — eval / new Function / Function are blocked. The
        // throwing stub reports via JS_ReportErrorUTF8, which produces a
        // plain `Error` (node throws EvalError — type divergence registered
        // in the C8b report); the message is the contract here.
        var noStr = vm.createContext({}, {codeGeneration: {strings: false, wasm: true}});
        check("codegen_eval_blocked", function() {
            var e = catchErr(function() { vm.runInContext('eval("1 + 1")', noStr); });
            var m = String(e && e.message || '');
            if (m.indexOf('Code generation from strings disallowed') === -1) {
                throw new Error('GOT:' + (e && (e.name + ': ' + m) || 'no-throw'));
            }
            return true;
        });
        check("codegen_new_function_blocked", function() {
            var e = catchErr(function() { vm.runInContext('new Function("return 1")', noStr); });
            return e !== null;
        });
        check("codegen_function_call_blocked", function() {
            var e = catchErr(function() { vm.runInContext('Function("return 1")', noStr); });
            return e !== null;
        });
        check("codegen_plain_code_still_runs", function() {
            return vm.runInContext('2 + 2', noStr) === 4;
        });

        // wasm: false — the WebAssembly global is removed from the realm.
        var noWasm = vm.createContext({}, {codeGeneration: {strings: true, wasm: false}});
        check("codegen_wasm_removed", function() {
            return vm.runInContext('typeof WebAssembly', noWasm) === 'undefined';
        });
        check("codegen_permissive_keeps_wasm", function() {
            return vm.runInContext('typeof WebAssembly', vm.createContext({})) !== 'undefined';
        });

        // Non-boolean / absent sub-fields fall back to allowed.
        var partial = vm.createContext({}, {codeGeneration: {}});
        check("codegen_empty_codegen_object_permissive", function() {
            return vm.runInContext('eval("5")', partial) === 5;
        });
        // codeGeneration not an object → permissive default.
        var badCg = vm.createContext({}, {codeGeneration: 'nope'});
        check("codegen_non_object_codegen_permissive", function() {
            return vm.runInContext('eval("6")', badCg) === 6;
        });
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn vm_context_lifecycle_and_writethrough() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var vm = require('vm');
        function catchErr(fn) { try { fn(); return null; } catch (e) { return e; } }

        // createContext is idempotent on an already-contextified sandbox —
        // and the early return happens BEFORE options validation.
        check("createContext_idempotent_returns_same", function() {
            var sb = {v: 1};
            var c1 = vm.createContext(sb);
            var c2 = vm.createContext(sb);
            return c1 === sb && c2 === sb;
        });
        check("createContext_idempotent_skips_options_validation", function() {
            var sb = {};
            vm.createContext(sb);
            return vm.createContext(sb, 5) === sb;
        });
        check("createContext_invalid_options_throws", function() {
            var e = catchErr(function() { vm.createContext({}, 5); });
            return e.code === 'ERR_INVALID_ARG_TYPE';
        });
        check("createContext_empty_sandbox_ok", function() {
            return typeof vm.createContext() === 'object';
        });

        // isContext.
        check("isContext_faces", function() {
            return vm.isContext({}) === false
                && vm.isContext(vm.createContext({})) === true
                && vm.isContext(5) === false
                && vm.isContext() === false;
        });

        // Write-through, both directions, including properties added to the
        // sandbox AFTER createContext.
        var sb = {seed: 40};
        vm.createContext(sb);
        check("writethrough_sandbox_to_global_late_props", function() {
            sb.late = 2;
            return vm.runInContext('seed + late', sb) === 42;
        });
        check("writethrough_global_to_sandbox", function() {
            vm.runInContext('gvar = 7', sb);
            return sb.gvar === 7;
        });
        check("writethrough_repeated_runs", function() {
            vm.runInContext('counter = (typeof counter === "undefined" ? 0 : counter) + 1', sb);
            vm.runInContext('counter = counter + 1', sb);
            return sb.counter === 2;
        });

        // runInContext against a non-contextified plain object fails loudly.
        check("runInContext_plain_object_throws", function() {
            var e = catchErr(function() { vm.runInContext('1', {}); });
            return String(e.message).indexOf('not a contextified object') !== -1;
        });
        // And against a primitive.
        check("runInContext_primitive_sandbox_throws", function() {
            var e = catchErr(function() { vm.runInContext('1', 5); });
            return String(e.message).indexOf('contextified sandbox') !== -1;
        });
        "#
    );
    assert_all_pass(&results);
}
