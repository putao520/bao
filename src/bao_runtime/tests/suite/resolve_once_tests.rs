// @trace REQ-ENG-005 [level:integration]
/// resolve-once module identity locks — absorb of upstream bun 272ff4350c
/// (#44473, "Resolve a module specifier once: fix a segfault in require()
/// of an ES module, and require() with a plugin's namespace").
///
/// Upstream anatomy: a module specifier was resolved up to three times,
/// each time from the previous answer (`Module._resolveFilename`
/// overrides, symlinks, plugin `onResolve` answers are not idempotent).
/// `require()` of an ES module then looked the record up under the
/// ORIGINAL key, found the first (never linked) record, and fetched its
/// namespace → segfault at 0x18.
///
/// Bao's SpiderMonkey loader resolves each import request exactly once
/// (`load_module_record_sync` → `resolve_specifier`) and keys the module
/// registry AND the require cache by the CANONICALIZED path, so `./x`,
/// `./sub/../x` and `/abs/./x` spellings of one file collapse to a single
/// module instance — the post-fix upstream semantics. These tests pin
/// that contract on the real filesystem:
///
/// 1. three import spellings of one file (two direct + one through a
///    sub-directory module that also proves importer-relative resolution)
///    → the module body evaluates exactly ONCE;
/// 2. `require()` of an ESM module loads and returns the live namespace
///    (the upstream segfault scenario, bao-adapted) and the require cache
///    serves the same exports object across all three spellings.

use bao_engine::context::{JsContext, thread_realm_global};
use bao_engine::module_loader::ModuleLoader;
use mozjs::rooted;

/// Build a test JsContext with the full Node/Bun globals installed
/// (same harness as module_eval_error_tests).
fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Three spellings of one module (`./target.mjs`, `./sub/../target.mjs`,
/// and `../target.mjs` from a module inside `sub/`) must load as ONE
/// module instance: the target body runs exactly once. A loader that
/// re-resolved an already-resolved key with a different spelling (the
/// upstream pre-fix behavior) would evaluate it 2-3 times and the
/// module-internal asserts would throw.
#[test]
fn import_spellings_collapse_to_single_module_instance() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("sub")).expect("mkdir sub");
    std::fs::write(
        root.join("target.mjs"),
        "globalThis.__hits = (globalThis.__hits | 0) + 1;\n\
         export const hits = globalThis.__hits;\n",
    )
    .expect("write target.mjs");
    std::fs::write(
        root.join("sub").join("inner.mjs"),
        // `../target.mjs` resolves against inner.mjs's own directory
        // (importer-relative); the process cwd never has a `../target.mjs`.
        "import \"../target.mjs\";\n\
         export const innerHits = globalThis.__hits;\n",
    )
    .expect("write sub/inner.mjs");

    let main_src = "import \"./target.mjs\";\n\
                    import \"./sub/../target.mjs\";\n\
                    import { innerHits } from \"./sub/inner.mjs\";\n\
                    if (innerHits !== 1) throw new Error(\"innerHits=\" + innerHits);\n\
                    if (globalThis.__hits !== 1) {\n\
                      throw new Error(\"target evaluated \" + globalThis.__hits + \" times\");\n\
                    }\n";
    let main_path = root.join("main.mjs").to_string_lossy().into_owned();

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(&mut cx, main_src, &main_path, None, global.handle())
        .expect("three spellings must load as one module, evaluated once");
}

/// `require()` of an ES module is the upstream segfault trigger
/// (`Module._resolveFilename` returning `dir + "/./esm.mjs"` etc.). In bao
/// it must load, return the live namespace, and the require cache key
/// (canonicalized path) must unify the relative, `sub/../` and `/./`
/// spellings into one cached exports object with a single evaluation.
#[test]
fn require_of_esm_module_loads_and_cache_unifies_spellings() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("sub")).expect("mkdir sub");
    std::fs::write(
        root.join("esm.mjs"),
        "globalThis.__esm_hits = (globalThis.__esm_hits | 0) + 1;\n\
         export const who = \"esm\";\n",
    )
    .expect("write esm.mjs");

    let mut ctx = make_ctx();
    let root_str = root.to_string_lossy().into_owned();
    // No `format!` on the body: the JS braces would need escaping. Substitute
    // the root through a placeholder instead.
    let src = r#"
        var m1 = require("./esm.mjs");
        var m2 = require("./sub/../esm.mjs");
        var m3 = require("@ROOT@/./esm.mjs");
        if (m1.who !== "esm") throw new Error("who=" + m1.who);
        if (m1 !== m2 || m2 !== m3) {
            throw new Error("require cache identity broken across spellings");
        }
        if (globalThis.__esm_hits !== 1) {
            throw new Error("esm evaluated " + globalThis.__esm_hits + " times");
        }
        "require-esm-ok"
        "#
    .replace("@ROOT@", &root_str);

    // The require cache keys by REQUIRE_DIR-relative resolution: point it
    // at the temp dir so the relative spellings resolve there.
    bun_runtime::require::set_require_dir(root.to_path_buf());

    let r = ctx.eval(&src, "<require-esm>").expect(
        "require() of an ES module must load (no segfault class) and the \
         require cache must unify path spellings",
    );
    match r {
        bao_engine::value::JsValue::String(s) => assert_eq!(s, "require-esm-ok"),
        other => panic!("expected ok marker string, got {:?}", other),
    }
}
