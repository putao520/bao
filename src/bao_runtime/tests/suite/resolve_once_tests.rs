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

/// e138 (REQ-ENG-001): require() of an ESM module must join the SAME module
/// registry the import hook uses (Node ≥22 require(esm) semantics: require
/// and import share one module registry; same URL ⇒ one module record).
/// Pre-fix anatomy (e136's reported adjacent surface): `load_esm_module`
/// compiled the entry OUTSIDE the module cache and keyed only the require
/// cache, so the import side (dynamic `import()` or a static import through
/// any dependent) missed the registry and compiled a SECOND instance from
/// disk — the body evaluated twice and the two namespaces diverged (a
/// require-vs-import observable fingerprint vs Node). Three pins:
///
/// 1. require-first, dual spelling: `require("./sub/../esm.mjs")` then
///    `import("./esm.mjs")` → one evaluation, shared namespace identity;
/// 2. import-first: `import("./esm.mjs")` then `require("./esm.mjs")` →
///    require must serve the registered record's namespace, not compile
///    a second instance;
/// 3. back-import through a dependent during the require-driven graph load
///    (require entry → sibling → entry static cycle) → each body exactly
///    once — the require-side twin of e136's eval-entry pin.

/// Require-first, dual spelling. Phase 1 requires the ESM under a
/// `sub/../` spelling (require resolver); phase 2 dynamically imports it
/// under the plain spelling (import hook) from a separate probe entry whose
/// continuation settles in eval_module_in_realm's job drain. The canonical
/// key must collapse the two spellings: one evaluation, and the import
/// namespace must be IDENTICAL to the require-returned namespace (SM hands
/// out one namespace object per module record).
#[test]
fn require_esm_then_import_single_instance() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("sub")).expect("mkdir sub");
    std::fs::write(
        root.join("esm.mjs"),
        "globalThis.__hits = (globalThis.__hits | 0) + 1;\n\
         export const hits = globalThis.__hits;\n\
         export const marker = {};\n",
    )
    .expect("write esm.mjs");

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    // The require cache keys by REQUIRE_DIR-relative resolution: point it
    // at the temp dir so the relative spelling resolves there.
    bun_runtime::require::set_require_dir(root.to_path_buf());

    // Phase 1: require() compiles the entry (pre-fix: outside the module
    // registry — only the require cache learns about it).
    ctx.eval(
        "globalThis.__reqNs = require(\"./sub/../esm.mjs\");",
        "<e138-require-first>",
    )
    .expect("require(esm) phase must load");

    // Phase 2: import() under a DIFFERENT spelling must resolve to the SAME
    // record (canonicalized key), not compile a second instance from disk.
    let probe_src = concat!(
        "import(\"./esm.mjs\").then((ns) => {\n",
        "  globalThis.__importHits = ns.hits;\n",
        "  globalThis.__importSame = (ns === globalThis.__reqNs);\n",
        "  globalThis.__importMarkerSame = (ns.marker === globalThis.__reqNs.marker);\n",
        "  globalThis.__done = true;\n",
        "});\n",
    );
    let probe_path = root.join("probe.mjs").to_string_lossy().into_owned();
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);
    ModuleLoader::eval_module_in_realm(&mut cx, probe_src, &probe_path, None, global.handle())
        .expect("probe entry evaluates");

    assert!(
        bool_probe(&mut ctx, "globalThis.__done === true"),
        "import continuation did not settle in the job drain"
    );
    let hits = num_probe(&mut ctx, "globalThis.__hits | 0");
    assert_eq!(
        hits, 1.0,
        "esm body must evaluate exactly once across require+import (got {})",
        hits
    );
    let import_hits = num_probe(&mut ctx, "globalThis.__importHits | 0");
    assert_eq!(
        import_hits, 1.0,
        "import namespace must be the require-side instance (ns.hits={}, expected 1)",
        import_hits
    );
    assert!(
        bool_probe(&mut ctx, "globalThis.__importSame === true"),
        "import namespace must be identical to the require-returned namespace \
         (ns !== require exports => second instance was compiled)"
    );
    assert!(
        bool_probe(&mut ctx, "globalThis.__importMarkerSame === true"),
        "binding-cell identity must match across require/import (second instance)"
    );
}

/// Import-first. Phase 1 statically imports the ESM from a probe entry —
/// the import hook compiles AND registers the record in the module cache,
/// and the graph machinery evaluates it. Phase 2 requires the same path:
/// require must serve the registered record's namespace (identity-equal to
/// the import-side namespace object) and the body must still have run
/// exactly once. (Dynamic `import()` of a fresh module — the adjacent
/// surface this test reported — was fixed in e139: the load hook now
/// drives the graph load for dynamic payloads before handing the record
/// to SM's ContinueDynamicImport; see the e139 pins below.)
#[test]
fn import_then_require_esm_single_instance() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::write(
        root.join("esm.mjs"),
        "globalThis.__hits = (globalThis.__hits | 0) + 1;\n\
         export const hits = globalThis.__hits;\n",
    )
    .expect("write esm.mjs");

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");

    // Phase 1: static import first — the hook registers the record and the
    // graph machinery evaluates it as part of the probe's graph.
    let probe_src = concat!(
        "import * as ns from \"./esm.mjs\";\n",
        "globalThis.__importNs = ns;\n",
    );
    let probe_path = root.join("probe.mjs").to_string_lossy().into_owned();
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);
    ModuleLoader::eval_module_in_realm(&mut cx, probe_src, &probe_path, None, global.handle())
        .expect("probe entry evaluates");

    // Phase 2: require() must hit the registry, not compile anew.
    bun_runtime::require::set_require_dir(root.to_path_buf());
    let src = r#"
        var m = require("./esm.mjs");
        if (m !== globalThis.__importNs) {
            throw new Error("require(esm) served a second instance (namespace identity broken)");
        }
        if (globalThis.__hits !== 1) {
            throw new Error("esm body evaluated " + globalThis.__hits + " times");
        }
        "import-then-require-ok"
        "#;
    let r = ctx
        .eval(src, "<e138-require-second>")
        .expect("require after import must serve the registered record");
    match r {
        bao_engine::value::JsValue::String(s) => assert_eq!(s, "import-then-require-ok"),
        other => panic!("expected ok marker string, got {:?}", other),
    }
}

/// Back-import through a dependent during the require-driven graph load:
/// the require entry imports ./sibling.mjs, sibling imports the entry back.
/// The require-compiled entry must be registered in the module cache BEFORE
/// graph load, so sibling's back-import resolves to THIS instance — the
/// entry body evaluates exactly once. Pre-fix, the back-import compiled a
/// second entry instance from disk (the disk copy evaluated first as
/// sibling's dependency, then the require-side body ran again).
#[test]
fn require_esm_back_import_from_dependency_single_instance() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::write(
        root.join("sibling.mjs"),
        "import \"./main.mjs\";\n\
         globalThis.__siblingRan = (globalThis.__siblingRan | 0) + 1;\n",
    )
    .expect("write sibling.mjs");
    std::fs::write(
        root.join("main.mjs"),
        "import \"./sibling.mjs\";\n\
         globalThis.__hits = (globalThis.__hits | 0) + 1;\n",
    )
    .expect("write main.mjs");

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    bun_runtime::require::set_require_dir(root.to_path_buf());

    ctx.eval("require(\"./main.mjs\");", "<e138-cycle>").expect(
        "cyclic require entry graph must load, link and evaluate",
    );

    let sibling_ran = num_probe(&mut ctx, "globalThis.__siblingRan | 0");
    assert_eq!(
        sibling_ran, 1.0,
        "sibling body must evaluate exactly once (got {} evaluations)",
        sibling_ran
    );
    let hits = num_probe(&mut ctx, "globalThis.__hits | 0");
    assert_eq!(
        hits, 1.0,
        "require entry body must evaluate exactly once across the cycle (got {} evaluations)",
        hits
    );
}

/// e139 (REQ-ENG-001): dynamic `import()` of a FRESH module (never
/// statically imported, required or evaluated — not in the module cache)
/// must resolve and evaluate the module, including its static dependency
/// graph. SM153 contract (Modules.cpp `ContinueDynamicImport`, the
/// "Step 3" comment: "The module dependencies has been loaded in the host
/// layer, so we only need to do _linkAndEvaluate_ part defined in the
/// spec"): when the host hands a module to
/// `JS::FinishLoadingImportedModule` with a promise payload, the record's
/// graph load must ALREADY be complete — `LinkAndEvaluateDynamicImport`
/// goes straight to `JS::ModuleLink`, which throws JSMSG_BAD_MODULE_STATUS
/// on a status=New record (`ModuleLink`'s step-1 status gate). Pre-fix
/// anatomy: `host_load_imported_module` compiled the fresh record (status
/// New) and passed it directly to FinishLoadingImportedModule, so every
/// dynamic import of an uncached module rejected; the existing suite's
/// only dynamic-import pins (e136/e138) all hit already-Evaluated cache
/// entries and never exercised the fresh path. The fix drives the same
/// graph load the eval entry paths use (`load_requested_modules_sync`,
/// JS::LoadRequestedModules) on the dynamic payload before handing the
/// record to the engine.

/// Dynamic import of a fresh module with a static dependency: the promise
/// must RESOLVE (no rejection), the fresh body and its dependency body
/// each evaluate exactly once, and the resolved namespace carries the
/// module's exports. The rejection handler records the error so a pre-fix
/// RED names the actual engine error (JSMSG_BAD_MODULE_STATUS).
#[test]
fn dynamic_import_fresh_module_resolves_and_evaluates() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::write(
        root.join("leaf.mjs"),
        "globalThis.__leafHits = (globalThis.__leafHits | 0) + 1;\n\
         export const leaf = 42;\n",
    )
    .expect("write leaf.mjs");
    std::fs::write(
        root.join("fresh.mjs"),
        "import { leaf } from \"./leaf.mjs\";\n\
         globalThis.__freshHits = (globalThis.__freshHits | 0) + 1;\n\
         export const hits = globalThis.__freshHits;\n\
         export const leafSum = leaf + 1;\n",
    )
    .expect("write fresh.mjs");

    // The probe entry only fires the dynamic import; fresh.mjs is loaded
    // exclusively through the import() path (fresh — not in the cache).
    let probe_src = concat!(
        "import(\"./fresh.mjs\").then((ns) => {\n",
        "  globalThis.__nsHits = ns.hits;\n",
        "  globalThis.__nsLeafSum = ns.leafSum;\n",
        "  globalThis.__done = true;\n",
        "}, (e) => {\n",
        "  globalThis.__err = String(e);\n",
        "  globalThis.__done = true;\n",
        "});\n",
    );
    let probe_path = root.join("probe.mjs").to_string_lossy().into_owned();

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(&mut cx, probe_src, &probe_path, None, global.handle())
        .expect("probe entry evaluates");

    assert!(
        bool_probe(&mut ctx, "globalThis.__done === true"),
        "dynamic import continuation did not settle in the job drain"
    );
    assert!(
        bool_probe(&mut ctx, "globalThis.__err === undefined"),
        "dynamic import of a fresh module must resolve, got rejection: {:?}",
        ctx.eval("globalThis.__err", "<e139-err>")
            .ok()
            .map(|v| format!("{:?}", v))
            .unwrap_or_else(|| "<unreadable>".into())
    );
    let fresh_hits = num_probe(&mut ctx, "globalThis.__freshHits | 0");
    assert_eq!(
        fresh_hits, 1.0,
        "fresh body must evaluate exactly once (got {} evaluations)",
        fresh_hits
    );
    let leaf_hits = num_probe(&mut ctx, "globalThis.__leafHits | 0");
    assert_eq!(
        leaf_hits, 1.0,
        "fresh module's static dependency must evaluate exactly once (got {})",
        leaf_hits
    );
    let ns_hits = num_probe(&mut ctx, "globalThis.__nsHits | 0");
    assert_eq!(
        ns_hits, 1.0,
        "resolved namespace must be the fresh record (ns.hits={}, expected 1)",
        ns_hits
    );
    let ns_leaf = num_probe(&mut ctx, "globalThis.__nsLeafSum | 0");
    assert_eq!(
        ns_leaf, 43.0,
        "namespace must carry the module exports through its dependency (ns.leafSum={}, expected 43)",
        ns_leaf
    );
}

/// Two dynamic imports of the same fresh path from one probe: the first
/// loads the record (graph-driven), the second must hit the module cache
/// (same record instance) — the body evaluates exactly once and both
/// promises resolve to the IDENTICAL namespace object (Node/spec: one URL
/// ⇒ one module record).
#[test]
fn dynamic_import_fresh_module_twice_single_instance() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::write(
        root.join("fresh.mjs"),
        "globalThis.__hits = (globalThis.__hits | 0) + 1;\n\
         export const hits = globalThis.__hits;\n\
         export const marker = {};\n",
    )
    .expect("write fresh.mjs");

    let probe_src = concat!(
        "globalThis.__count = 0;\n",
        "const p1 = import(\"./fresh.mjs\");\n",
        "const p2 = import(\"./fresh.mjs\");\n",
        "Promise.all([p1, p2]).then(([ns1, ns2]) => {\n",
        "  globalThis.__nsSame = (ns1 === ns2);\n",
        "  globalThis.__markerSame = (ns1.marker === ns2.marker);\n",
        "  globalThis.__done = true;\n",
        "}, (e) => {\n",
        "  globalThis.__err = String(e);\n",
        "  globalThis.__done = true;\n",
        "});\n",
    );
    let probe_path = root.join("probe.mjs").to_string_lossy().into_owned();

    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(&mut cx, probe_src, &probe_path, None, global.handle())
        .expect("probe entry evaluates");

    assert!(
        bool_probe(&mut ctx, "globalThis.__done === true"),
        "dynamic import continuations did not settle in the job drain"
    );
    assert!(
        bool_probe(&mut ctx, "globalThis.__err === undefined"),
        "both dynamic imports must resolve, got rejection: {:?}",
        ctx.eval("globalThis.__err", "<e139-err>")
            .ok()
            .map(|v| format!("{:?}", v))
            .unwrap_or_else(|| "<unreadable>".into())
    );
    let hits = num_probe(&mut ctx, "globalThis.__hits | 0");
    assert_eq!(
        hits, 1.0,
        "fresh body must evaluate exactly once across both imports (got {})",
        hits
    );
    assert!(
        bool_probe(&mut ctx, "globalThis.__nsSame === true"),
        "both dynamic imports must resolve to the same namespace object \
         (second import compiled a second instance)"
    );
    assert!(
        bool_probe(&mut ctx, "globalThis.__markerSame === true"),
        "export binding cells must be shared across both imports (second instance)"
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
