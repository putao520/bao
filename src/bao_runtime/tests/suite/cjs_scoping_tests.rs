// @trace TEST-ENG-048 [req:REQ-ENG-048] [level:integration]
//! REQ-ENG-048 CJS module-body scoping root-fix — regression pins.
//! Grounding: PRD-REQ-NODE-COMPAT-SUITE (Node.js 官方测试套件为 Node 兼容
//! GOLDEN oracle) → REQ-ENG-048 (SPEC/10-REQUIREMENTS).
//!
//! Node v24 oracle: `require("module").wrapper === ["(function (exports,
//! require, module, __filename, __dirname) { ", "\n});"]` — every CommonJS
//! body is function-local. Pre-fix bao evaluated require()-loaded module
//! bodies AND plain-script entries as GLOBAL scripts, so top-level
//! const/let/class were interned into the realm's persistent global lexical
//! environment (the same name declared by any later script = SyntaxError =
//! "Failed to load module") and top-level var/function leaked across modules
//! as global properties.

fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bao-cjs-scope-{}-{}", std::process::id(), tag));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn write_module(dir: &std::path::Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("write fixture module");
    path.to_string_lossy().into_owned()
}

/// Evaluate `code` and return its string result.
fn eval_str(rt: &mut bun_runtime::NodeRuntime, code: &str) -> String {
    rt.eval(code, "<cjs-scope-probe>")
        .expect("probe eval must succeed")
        .as_string()
        .expect("probe must return a string")
        .to_string()
}

/// The report.md collision shape: two modules in one process both declare a
/// top-level `const` with the same name (historically the shim's
/// `const path` + the test file's `const path`) — pre-fix the second load
/// died with SyntaxError → "Failed to load module".
#[test]
fn cjs_same_name_top_level_const_across_modules_both_load() {
    let dir = scratch_dir("const-collision");
    let a = write_module(
        &dir,
        "a.js",
        "const path = 'module-a-path';\nexports.pathName = path;\n",
    );
    let b = write_module(
        &dir,
        "b.js",
        "const path = 'module-b-path';\nmodule.exports = { own: path };\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "JSON.stringify([require({a:?}).pathName, require({b:?}).own])",
        a = a,
        b = b
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"["module-a-path","module-b-path"]"#,
        "both modules must load and keep their own top-level const"
    );
}

/// The task-header minimal shape: module A exports name X while module B
/// declares a local X — cross require stays independent, and neither
/// module-local leaks to the global object.
#[test]
fn cjs_export_name_and_local_variable_same_name_no_conflict() {
    let dir = scratch_dir("export-vs-local");
    write_module(&dir, "a.js", "var X = 'a-local';\nexports.X = X;\n");
    let b = write_module(
        &dir,
        "b.js",
        "const a = require('./a.js');\nvar X = 'b-local-' + a.X;\nmodule.exports = X;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let b_path = dir.join("b.js");
    let code = format!(
        "JSON.stringify([require({b:?}), typeof globalThis.X])",
        b = b_path.to_string_lossy()
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"["b-local-a-local","undefined"]"#,
        "local X resolves per module and never leaks to globalThis"
    );
}

/// Module top-level var no longer becomes a global property (pre-fix:
/// `var helperValue` in a required module was observable from later evals).
#[test]
fn cjs_module_top_level_var_does_not_leak_to_global() {
    let dir = scratch_dir("var-leak");
    let a = write_module(
        &dir,
        "leaky.js",
        "var helperValue = 'leaked';\nexports.tag = 1;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "require({a:?}); JSON.stringify([typeof globalThis.helperValue])",
        a = a
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"["undefined"]"#,
        "module-local var must stay module-local"
    );
}

/// Entry files (bao run / bao test plain-script arm) get the same wrapper:
/// an entry-level `const` no longer enters the global lexical environment,
/// and an entry + required module declaring the same top-level const both
/// load (the golden-node shim+test collision, entry side).
#[test]
fn cjs_entry_top_level_const_stays_local_and_module_collision_loads() {
    let dir = scratch_dir("entry-scope");
    let c = write_module(
        &dir,
        "c.js",
        "const path = 'c-local';\nmodule.exports = { own: path };\n",
    );
    let entry = write_module(
        &dir,
        "entry.js",
        &format!(
            "const entryToken = 'entry-local';\nconst path = require('path');\nconst c = require({c:?});\nglobalThis.__entryProbe = JSON.stringify({{ own: c.own, tok: entryToken }});\n",
            c = c
        ),
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    rt.run_file(&entry)
        .expect("entry with const colliding with required module must load");
    assert_eq!(
        eval_str(&mut rt, "globalThis.__entryProbe"),
        r#"{"own":"c-local","tok":"entry-local"}"#,
        "entry locals and module locals stay independent"
    );
    assert_eq!(
        eval_str(&mut rt, "typeof globalThis.entryToken"),
        "undefined",
        "entry top-level const must not leak to globalThis"
    );
}

/// Wrapper argument parity (node contract): exports object, require
/// function, module with exports alias, __filename/__dirname strings,
/// `this === exports`.
#[test]
fn cjs_wrapper_arguments_node_parity() {
    let dir = scratch_dir("wrapper-args");
    let p = write_module(
        &dir,
        "p.js",
        r#"
var out = {};
out.thisIsExports = (this === exports);
out.exportsIsObject = (typeof exports === 'object' && exports !== null);
out.moduleExportsAlias = (module.exports === exports);
out.filenameOk = (typeof __filename === 'string' && __filename.slice(-4) === 'p.js');
out.dirnameOk = (typeof __dirname === 'string' && __dirname.length > 0);
out.requireFn = (typeof require === 'function');
out.exportsWrite = ((exports.marker = 42), true);
module.exports = out;
"#,
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "var m = require({p:?}); JSON.stringify([m.thisIsExports, m.exportsIsObject, m.moduleExportsAlias, m.filenameOk, m.dirnameOk, m.requireFn, m.exportsWrite, m.marker === undefined])",
        p = p
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"[true,true,true,true,true,true,true,true]"#,
        "all wrapper arguments must match node's contract; the marker write \
         landed on the pre-rebind exports alias, so it must be detached after \
         `module.exports = out` (node semantics)"
    );
}

/// `module.exports` rebinding is the module's public value; later writes
/// through the detached `exports` alias stay invisible (node semantics).
#[test]
fn cjs_module_exports_rebind_wins() {
    let dir = scratch_dir("exports-rebind");
    let r = write_module(
        &dir,
        "r.js",
        "module.exports = { rebound: true };\nexports.hidden = 1;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "var m = require({r:?}); JSON.stringify([m.rebound, m.hidden === undefined])",
        r = r
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"[true,true]"#,
        "module.exports rebind must win and exports alias writes must detach"
    );
}

/// Top-level `return` is legal in a CommonJS body (function wrapper) —
/// pre-fix the global-script evaluation rejected it as a SyntaxError.
#[test]
fn cjs_top_level_return_is_legal() {
    let dir = scratch_dir("top-return");
    let ret = write_module(
        &dir,
        "ret.js",
        "exports.before = 1;\nreturn;\nexports.after = 2;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "var m = require({ret:?}); JSON.stringify([m.before, m.after === undefined])",
        ret = ret
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"[1,true]"#,
        "top-level return must end the module body like in node (before=1, after never assigned)"
    );
}

/// A leading shebang is normalized to a comment before wrapping — modules
/// with `#!` keep loading (a HashbangComment is only valid at the very start
/// of a Script source, so the wrapper needs the normalization).
#[test]
fn cjs_shebang_module_loads() {
    let dir = scratch_dir("shebang");
    let s = write_module(
        &dir,
        "she.js",
        "#!/usr/bin/env bao\nexports.shebang = true;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!("JSON.stringify([require({s:?}).shebang])", s = s);
    assert_eq!(eval_str(&mut rt, &code), r#"[true]"#);
}

/// Nested require during a module body still works through the wrapper call
/// (REQUIRE_DIR save/restore must survive the new execution shape), and the
/// require cache still serves the identical exports object.
#[test]
fn cjs_nested_require_and_cache_unchanged() {
    let dir = scratch_dir("nested");
    write_module(&dir, "child.js", "module.exports = { ok: true };\n");
    let parent = write_module(
        &dir,
        "parent.js",
        "var c = require('./child.js');\nexports.childSaw = c.ok;\nexports.done = true;\n",
    );

    let mut rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let code = format!(
        "var p1 = require({parent:?}); var p2 = require({parent:?}); JSON.stringify([p1.childSaw, p1.done, p1 === p2])",
        parent = parent
    );
    assert_eq!(
        eval_str(&mut rt, &code),
        r#"[true,true,true]"#,
        "nested require works and the cache serves the identical object"
    );
}
