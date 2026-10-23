// @trace REQ-ENG-005 [level:integration]
/// Node CJS directory-index correction pins — the require() of a directory
/// containing both `index.js` (pure CJS) and `index.mjs` (ESM facade) must
/// resolve to the CJS `index.js` per Node LOAD_AS_DIRECTORY semantics
/// (`.js` > `.json` > `.node`; `.mjs` is never a directory-index result).
///
/// Root cause: bun_resolver (ImportKind::Stmt, ESM-first ordering) resolves
/// the directory to `index.mjs`, is_esm_module then flags it, and the ESM
/// loader fails with "Failed to load ESM module". Node v24 oracle
/// (2026-10-23 /tmp/resprobe):
///   index.js + index.mjs → require('./common') picks index.js
///   only index.mjs       → require('./common') MODULE_NOT_FOUND
///   explicit './common/index.mjs' → ESM namespace (require(esm) allowed)
use bao_engine::context::JsContext;

fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Grid 1: directory contains both index.js (CJS) and index.mjs (ESM).
/// require() of the directory must return the CJS module.exports — not
/// attempt the ESM facade and not throw "Failed to load ESM module".
#[test]
fn require_directory_prefers_index_js_over_index_mjs() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let pkg = dir.path().join("common");
    std::fs::create_dir_all(&pkg).expect("mkdir common");

    // Pure CJS index.js — the directory-index target per Node semantics.
    std::fs::write(
        pkg.join("index.js"),
        "module.exports = { tag: 'js', value: 42 };\n",
    )
    .expect("write index.js");

    // ESM facade — must NOT be selected by directory-index require().
    std::fs::write(
        pkg.join("index.mjs"),
        "export const tag = 'mjs';\nexport default 42;\n",
    )
    .expect("write index.mjs");

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    bun_runtime::require::set_require_dir(dir.path().to_path_buf());

    let result = ctx.eval("JSON.stringify(require('./common'))", "<require-dir-cjs>");

    match result {
        Ok(bao_engine::value::JsValue::String(s)) => {
            assert_eq!(
                s, r#"{"tag":"js","value":42}"#,
                "require('./common') must return the CJS index.js exports, \
                 not the ESM facade (got: {})",
                s
            );
        }
        Ok(other) => panic!("expected string JSON, got {:?}", other),
        Err(e) => panic!(
            "require('./common') must not throw when index.js exists \
             alongside index.mjs: {}",
            e
        ),
    }
}

/// Grid 2: directory contains ONLY index.mjs (no CJS index sibling).
/// Node semantics → MODULE_NOT_FOUND; the require must NOT fall through to
/// the ESM loader ("Failed to load ESM module" is the wrong error class).
#[test]
fn require_directory_only_index_mjs_is_module_not_found() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let pkg = dir.path().join("common");
    std::fs::create_dir_all(&pkg).expect("mkdir common");

    std::fs::write(pkg.join("index.mjs"), "export const tag = 'mjs';\n").expect("write index.mjs");

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    bun_runtime::require::set_require_dir(dir.path().to_path_buf());

    let result = ctx.eval("require('./common')", "<require-dir-mjs-only>");

    match result {
        Ok(other) => panic!(
            "require('./common') with only index.mjs must throw \
             MODULE_NOT_FOUND, not return {:?}",
            other
        ),
        Err(e) => {
            let msg = format!("{}", e);
            assert!(
                msg.contains("Cannot find module"),
                "error must be MODULE_NOT_FOUND form \
                 (\"Cannot find module ...\"), got: {}",
                msg
            );
            assert!(
                !msg.contains("Failed to load ESM module"),
                "error must NOT be the ESM loader failure surface \
                 (directory-index .mjs is not require(esm)), got: {}",
                msg
            );
        }
    }
}

/// Grid 3: explicit `require('./common/index.mjs')` — the caller named the
/// exact `.mjs` file. Node ≥22 require(esm) semantics apply: the ESM module
/// loads and its namespace is returned. The correction must NOT redirect to
/// a CJS sibling.
#[test]
fn require_explicit_index_mjs_loads_esm() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let pkg = dir.path().join("common");
    std::fs::create_dir_all(&pkg).expect("mkdir common");

    // Both files present — but the caller asked for index.mjs explicitly.
    std::fs::write(pkg.join("index.js"), "module.exports = { tag: 'js' };\n")
        .expect("write index.js");
    std::fs::write(
        pkg.join("index.mjs"),
        "globalThis.__esmLoaded = (globalThis.__esmLoaded | 0) + 1;\n\
         export const tag = 'mjs';\n",
    )
    .expect("write index.mjs");

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    bun_runtime::require::set_require_dir(dir.path().to_path_buf());

    // The ESM namespace must be returned — the CJS sibling must NOT
    // shadow the explicitly named .mjs file.
    let result = ctx.eval(
        "var ns = require('./common/index.mjs'); ns.tag",
        "<require-explicit-mjs>",
    );

    match result {
        Ok(bao_engine::value::JsValue::String(s)) => {
            assert_eq!(
                s, "mjs",
                "explicit require('./common/index.mjs') must load the ESM \
                 namespace (tag='mjs'), not redirect to index.js (got: {})",
                s
            );
        }
        Ok(other) => panic!("expected string 'mjs', got {:?}", other),
        Err(e) => panic!(
            "explicit require('./common/index.mjs') must not throw: {}",
            e
        ),
    }
}
