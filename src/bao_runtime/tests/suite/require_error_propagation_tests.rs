// @trace TEST-ENG-048 [req:REQ-ENG-048] [level:integration]
//! REQ-ENG-048 — Node v24 oracle: `require()` rethrows the loaded module's
//! own evaluation error VERBATIM.
//!
//! Node v24 oracle (`/tmp` fixture, node v24.19.0):
//!   thrower.js: `const e = new Error('boom-from-module'); e.code = 'E_BOOM';
//!                e.custom = {k: 42}; throw e;`
//!   require('./thrower.js') inside try/catch yields
//!   `{isError:true, name:"Error", message:"boom-from-module",
//!     code:"E_BOOM", custom:42, keys:["code","custom"]}` and
//!   `e === <the object the module threw>`; a nested require preserves the
//!   same identity. A non-Error throw propagates as the raw thrown value.
//!   A syntactically broken module yields the engine SyntaxError with the
//!   module filename/line — never a loader-authored replacement.
//!
//! Pre-fix bao cleared the pending exception in the CJS wrapper call arm and
//! reported `Error("Failed to load module '<specifier>'")`, erasing the
//! original error object (no cause/code/keys/stack) — the first-error family
//! in the Node official-suite failmap.

use std::path::PathBuf;

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bao-require-err-{}-{}", std::process::id(), tag));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn write_module(dir: &std::path::Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("write fixture module");
    path.to_string_lossy().into_owned()
}

fn eval_str(rt: &mut bun_runtime::NodeRuntime, code: &str) -> String {
    rt.eval(code, "<require-error-probe>")
        .expect("probe eval must succeed")
        .as_string()
        .expect("probe must return a string")
        .to_string()
}

/// Node parity (oracle above): the caught value is the module's Error object
/// with all own properties intact — name/message/code/custom/keys.
#[test]
fn require_propagates_module_thrown_error_with_own_properties() {
    let dir = scratch_dir("props");
    let module = write_module(
        &dir,
        "thrower.js",
        "const e = new Error('boom-from-module');\n\
         e.code = 'E_BOOM';\n\
         e.custom = { k: 42 };\n\
         throw e;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "let caught = null;\n\
         try {{ require({module:?}); }} catch (err) {{ caught = err; }}\n\
         JSON.stringify({{\n\
           isError: caught instanceof Error,\n\
           name: caught && caught.name,\n\
           message: caught && caught.message,\n\
           code: caught && caught.code,\n\
           custom: caught && caught.custom && caught.custom.k,\n\
           keys: caught ? Object.keys(caught) : [],\n\
           cause: caught ? caught.cause : undefined,\n\
         }})",
        module = module
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"{"isError":true,"name":"Error","message":"boom-from-module","code":"E_BOOM","custom":42,"keys":["code","custom"]}"#,
        "require() must rethrow the module's own Error, own properties intact"
    );
}

/// Node parity: error IDENTITY is preserved — the caller catches the exact
/// object the module threw, not a wrapper/copy.
#[test]
fn require_preserves_thrown_error_identity() {
    let dir = scratch_dir("identity");
    let module = write_module(
        &dir,
        "thrower.js",
        "globalThis.__bao_throw_identity = new Error('identity-probe');\n\
         throw globalThis.__bao_throw_identity;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "try {{ require({module:?}); }} catch (e) {{\n\
           JSON.stringify([e === globalThis.__bao_throw_identity, e.message])\n\
         }}",
        module = module
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"[true,"identity-probe"]"#,
        "the caught value must be the literal thrown object"
    );
}

/// Node parity: propagation crosses nested require boundaries unchanged.
#[test]
fn require_nested_module_throw_propagates_identity_to_outer_caller() {
    let dir = scratch_dir("nested");
    write_module(
        &dir,
        "thrower.js",
        "globalThis.__bao_nested_err = new Error('nested-boom');\n\
         globalThis.__bao_nested_err.code = 'E_NESTED';\n\
         throw globalThis.__bao_nested_err;\n",
    );
    let nested = write_module(&dir, "nested.js", "require('./thrower.js');\n");

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "try {{ require({module:?}); }} catch (e) {{\n\
           JSON.stringify([e === globalThis.__bao_nested_err, e.code, e.message])\n\
         }}",
        module = nested
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"[true,"E_NESTED","nested-boom"]"#,
        "a throw in a nested require must reach the outer catch as the same object"
    );
}

/// Node parity: non-Error thrown values propagate raw (oracle: caught value
/// is the string itself).
#[test]
fn require_propagates_non_error_thrown_value_raw() {
    let dir = scratch_dir("primitive");
    let module = write_module(&dir, "prim.js", "throw 'string-thrown';\n");

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "try {{ require({module:?}); }} catch (e) {{\n\
           JSON.stringify([e, typeof e])\n\
         }}",
        module = module
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"["string-thrown","string"]"#,
        "a thrown primitive must not be wrapped into an Error"
    );
}

/// Node parity: a syntactically broken module surfaces the engine
/// SyntaxError (module filename/line), not the loader's opaque replacement.
#[test]
fn require_syntax_error_module_surfaces_engine_syntax_error() {
    let dir = scratch_dir("syntax");
    let module = write_module(&dir, "badsyntax.js", "const x = ;\n");

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "let caught = null;\n\
         try {{ require({module:?}); }} catch (err) {{ caught = err; }}\n\
         JSON.stringify([\n\
           caught instanceof SyntaxError,\n\
           caught && caught.name,\n\
           !/Failed to load module/.test(String(caught && caught.message)),\n\
         ])",
        module = module
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"[true,"SyntaxError",true]"#,
        "syntax errors must surface as SyntaxError, not a loader-authored Error"
    );

    // Uncaught path: the engine's JsError carries the module file/line.
    let err = rt
        .eval(&format!("require({module:?})"), "<require-error-probe>")
        .expect_err("require() of a broken module must throw");
    assert!(
        !err.message.contains("Failed to load module"),
        "uncaught require error must not be the loader replacement: {err}"
    );
    assert!(
        err.filename.ends_with("badsyntax.js"),
        "uncaught syntax error must point at the module file, got {err}"
    );
    assert_eq!(
        err.line, 1,
        "wrapper prefix adds no newline — body line 1 must report as line 1: {err}"
    );
}

/// Node parity: a module that throws is NOT cached — the next require()
/// re-executes it (oracle: both attempts execute and throw).
#[test]
fn require_throwing_module_is_not_cached_and_reexecutes() {
    let dir = scratch_dir("no-cache");
    let module = write_module(
        &dir,
        "always-throw.js",
        "globalThis.__bao_load_count = (globalThis.__bao_load_count || 0) + 1;\n\
         throw new Error('always-' + globalThis.__bao_load_count);\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "const messages = [];\n\
         for (let i = 0; i < 2; i++) {{\n\
           try {{ require({module:?}); }} catch (e) {{ messages.push(e.message); }}\n\
         }}\n\
         JSON.stringify(messages)",
        module = module
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"["always-1","always-2"]"#,
        "a throwing module must not enter require.cache"
    );
}

/// Uncaught path: the module's message/stack survive the native require
/// boundary (Node prints `Error: boom-from-module` at the module line).
#[test]
fn uncaught_require_error_keeps_module_message_and_stack_frame() {
    let dir = scratch_dir("uncaught");
    let module = write_module(
        &dir,
        "thrower.js",
        "const e = new Error('uncaught-boom');\n\
         throw e;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let err = rt
        .eval(&format!("require({module:?})"), "<require-error-probe>")
        .expect_err("uncaught require must propagate the module error");
    assert!(
        err.message.contains("uncaught-boom"),
        "module message must survive: {err}"
    );
    assert!(
        !err.message.contains("Failed to load module"),
        "loader replacement must not mask the module error: {err}"
    );
    assert!(
        err.filename.ends_with("thrower.js"),
        "error location must be the module file: {err}"
    );

    // In-JS stack usable by the official suite diagnostics.
    let module2 = write_module(
        &dir,
        "stacked.js",
        "function inner() { throw new Error('stack-boom'); }\n\
         inner();\n",
    );
    let code = format!(
        "try {{ require({module:?}); }} catch (e) {{\n\
           JSON.stringify([String(e.stack).includes('stacked.js'),\n\
                           String(e.stack).includes('inner')])\n\
         }}",
        module = module2
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"[true,true]"#,
        "the module's own stack frames must be observable after require()"
    );
}
