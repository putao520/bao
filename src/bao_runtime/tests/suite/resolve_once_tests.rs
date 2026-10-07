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

/// e136 (REQ-ENG-001): entry modules compiled directly by the eval_module*
/// entries must join the module cache under the SAME canonicalized-path key
/// the import hook uses. Two pins below:
///
/// 1. `import()` of the entry's own path from inside the entry body must
///    resolve to THE SAME module record — the entry body evaluates exactly
///    once and the self-import namespace identity matches (same export
///    binding cells), instead of compiling a second instance from disk.
///    Node/spec: one URL ⇒ one module instance.
///
/// 2. a static back-import through a dependent (entry → sibling → entry)
///    during graph load must hit the same cache — the entry body still
///    evaluates exactly once.
///
/// Pre-fix anatomy: `eval_module_in_realm` compiled the entry from the
/// caller's source but never registered it in the module cache, so any
/// by-path import of the entry (self or back) missed the cache and compiled
/// a SECOND instance from disk — the body ran twice and the two namespaces
/// diverged (an import-detectable fingerprint vs Node).

/// Probe helper: evaluate a boolean expression in the persistent realm.
fn bool_probe(ctx: &mut JsContext, expr: &str) -> bool {
    match ctx.eval(expr, "<e136-probe>").expect("probe eval") {
        bao_engine::value::JsValue::Bool(b) => b,
        other => panic!("probe `{}` returned {:?}, expected bool", expr, other),
    }
}

/// Probe helper: evaluate a numeric expression in the persistent realm.
fn num_probe(ctx: &mut JsContext, expr: &str) -> f64 {
    match ctx.eval(expr, "<e136-probe>").expect("probe eval") {
        bao_engine::value::JsValue::Number(n) => n,
        other => panic!("probe `{}` returned {:?}, expected number", expr, other),
    }
}

/// Dynamic `import()` of the entry's own path, fired from the entry body:
/// the continuation must settle inside the post-evaluation job drain, the
/// entry body must have run exactly ONCE, and the resolved namespace must
/// be the entry's own record (ns.hits === 1, ns.marker === marker). A
/// second disk-compiled instance would run the body again (hits === 2) and
/// hand back its own fresh `marker` object (identity mismatch).
#[test]
fn entry_dynamic_self_import_single_instance() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    // The import hook reads from DISK, so the entry must also exist as a
    // file — exactly the `bao run main.mjs` shape (source passed in, same
    // bytes on disk).
    let main_src = concat!(
        "globalThis.__hits = (globalThis.__hits | 0) + 1;\n",
        "export const hits = globalThis.__hits;\n",
        "export const marker = {};\n",
        "import(\"./main.mjs\").then((ns) => {\n",
        "  globalThis.__nsHits = ns.hits;\n",
        "  globalThis.__nsMarkerSame = (ns.marker === marker);\n",
        "  globalThis.__done = true;\n",
        "});\n",
    );
    std::fs::write(root.join("main.mjs"), main_src).expect("write main.mjs");
    let main_path = root.join("main.mjs").to_string_lossy().into_owned();

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(&mut cx, main_src, &main_path, None, global.handle())
        .expect("entry module evaluates");

    assert!(
        bool_probe(&mut ctx, "globalThis.__done === true"),
        "dynamic import continuation did not settle in the job drain"
    );
    let hits = num_probe(&mut ctx, "globalThis.__hits | 0");
    assert_eq!(
        hits, 1.0,
        "entry body must evaluate exactly once (got {} evaluations)",
        hits
    );
    let ns_hits = num_probe(&mut ctx, "globalThis.__nsHits | 0");
    assert_eq!(
        ns_hits, 1.0,
        "self-import namespace must be the entry instance (ns.hits={}, expected 1)",
        ns_hits
    );
    assert!(
        bool_probe(&mut ctx, "globalThis.__nsMarkerSame === true"),
        "self-import namespace identity must match the entry record \
         (ns.marker !== marker => second instance was compiled)"
    );
}

/// Static back-import through a dependent: entry imports ./sibling.mjs,
/// sibling imports ./entry back. Side-effect-only module bodies keep the
/// cycle free of TDZ hazards (the entry's top level runs after sibling in
/// the cycle). Assertions stay on the Rust side: the entry body must run
/// exactly ONCE. Pre-fix, sibling's back-import compiled a second entry
/// instance from disk — the disk copy evaluated first (as sibling's
/// dependency), then the real entry body ran again (2 evaluations).
#[test]
fn entry_back_import_from_dependency_single_instance() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::write(
        root.join("sibling.mjs"),
        "import \"./main.mjs\";\n\
         globalThis.__siblingRan = (globalThis.__siblingRan | 0) + 1;\n",
    )
    .expect("write sibling.mjs");
    let main_src = concat!(
        "import \"./sibling.mjs\";\n",
        "globalThis.__hits = (globalThis.__hits | 0) + 1;\n",
    );
    // The entry must exist on disk for the (pre-fix) disk-compile path to
    // be exercisable at all — same bytes as the in-memory source.
    std::fs::write(root.join("main.mjs"), main_src).expect("write main.mjs");
    let main_path = root.join("main.mjs").to_string_lossy().into_owned();

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(&mut cx, main_src, &main_path, None, global.handle())
        .expect("cyclic entry graph loads, links and evaluates");

    let sibling_ran = num_probe(&mut ctx, "globalThis.__siblingRan | 0");
    assert_eq!(
        sibling_ran, 1.0,
        "sibling body must evaluate exactly once (got {} evaluations)",
        sibling_ran
    );
    let hits = num_probe(&mut ctx, "globalThis.__hits | 0");
    assert_eq!(
        hits, 1.0,
        "entry body must evaluate exactly once across the cycle (got {} evaluations)",
        hits
    );
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
