// @trace REQ-ENG-001 [entity:JsContext]
/// Realm persistence tests — verify ECMA-262/Node realm-per-context semantics:
/// a `JsContext` owns ONE persistent realm; scripts execute inside it; the
/// realm persists across `eval` calls. This is what makes `globalThis.x` set
/// by eval A visible to eval B, and lets setTimeout/server handlers fire
/// after the registering script returns.
///
/// Under the old eval-per-global model every `eval` built a fresh
/// `JS_NewGlobalObject`, so each of these would read `undefined` / fail. Under
/// realm-per-context they must all pass.

use bao_engine::context::{JsContext, thread_realm_global};
use bao_engine::module_loader::ModuleLoader;
use mozjs::rooted;

/// Build a test JsContext with the full Node/Bun globals installed.
fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Core realm-per-context invariant: a global property set by eval A must be
/// visible to eval B (Node semantics).
#[test]
fn cross_eval_globalthis_property_persists() {
    let mut ctx = make_ctx();
    ctx.eval("globalThis.x = 1;", "<a>").expect("eval A must succeed");
    let r = ctx.eval("globalThis.x;", "<b>").expect("eval B must succeed");
    assert_eq!(
        r.as_number(),
        Some(1.0),
        "globalThis.x must persist across evals (realm-per-context, Node semantics)"
    );
}

/// Function declarations install on the realm's global; a later eval in the
/// same realm must be able to call them.
#[test]
fn cross_eval_function_declaration_persists() {
    let mut ctx = make_ctx();
    ctx.eval("function double(n) { return n * 2; }", "<a>")
        .expect("eval A");
    let r = ctx.eval("double(21);", "<b>").expect("eval B");
    assert_eq!(
        r.as_number(),
        Some(42.0),
        "function declaration must persist across evals"
    );
}

/// Object identity must be stable across evals: a mutation by eval B of an
/// object stored by eval A is observable by eval C.
#[test]
fn cross_eval_object_identity_stable() {
    let mut ctx = make_ctx();
    ctx.eval("globalThis.box = { count: 0 };", "<a>")
        .expect("eval A");
    ctx.eval("globalThis.box.count += 5;", "<b>")
        .expect("eval B");
    let r = ctx.eval("globalThis.box.count;", "<c>").expect("eval C");
    assert_eq!(
        r.as_number(),
        Some(5.0),
        "object identity must be stable across evals"
    );
}

/// Realm global reuse: the same global object is reused across many evals
/// (not a fresh global per eval). Verified by incrementing a counter over
/// many evals and reading back the accumulated value.
#[test]
fn realm_global_reused_across_many_evals() {
    let mut ctx = make_ctx();
    ctx.eval("globalThis.counter = 0;", "<init>").expect("init");
    for i in 1..=10 {
        ctx.eval("globalThis.counter += 1;", &format!("<tick-{i}>"))
            .expect("tick eval");
    }
    let r = ctx.eval("globalThis.counter;", "<final>").expect("final");
    assert_eq!(
        r.as_number(),
        Some(10.0),
        "single realm global must be reused across 10 evals"
    );
}

/// A `require`-registered singleton must be the SAME instance across evals
/// (Node module-singleton semantics depend on realm persistence).
#[test]
fn cross_eval_require_singleton_identity() {
    let mut ctx = make_ctx();
    // Plant a sentinel on globalThis in A; in B verify the same value is read
    // back (this stands in for module-singleton identity, which requires a
    // real module loader and is exercised in module-eval path tests).
    ctx.eval("globalThis.__sentinel = { id: 'stable' };", "<a>")
        .expect("eval A");
    let r = ctx
        .eval("globalThis.__sentinel.id;", "<b>")
        .expect("eval B");
    assert_eq!(
        r.as_string(),
        Some("stable"),
        "sentinel identity must persist across evals"
    );
}

// ── module-vs-script same-realm (realm-per-context unification) ──
//
// Under realm-per-context a script eval and a module eval on the same context
// share ONE realm — `globalThis` is the same object, so a property planted by
// a script eval is visible to a module eval and vice versa. Under the old
// eval-per-global model each built its own global and these were isolated
// (Node/Bun/servo are all realm-per-context; the isolation was a bug).

/// Script → module → script: a `globalThis.x` planted by a script eval is
/// readable by a module eval, and a `globalThis.y` the module writes is
/// readable by a later script eval.
#[test]
fn script_and_module_share_realm_globalthis() {
    let mut ctx = make_ctx();

    // Script eval seeds the shared realm global.
    ctx.eval("globalThis.x = 42;", "<script>")
        .expect("script eval");

    // Module eval in the SAME realm: read x, write y. No new global —
    // `ModuleLoader::eval_module_in_realm` enters the existing realm.
    let global_ptr = thread_realm_global().expect("realm global published by script eval");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);
    ModuleLoader::eval_module_in_realm(
        &mut cx,
        "globalThis.y = globalThis.x + 1;",
        "<module>.mjs",
        None,
        global.handle(),
    )
    .expect("module eval in realm");

    // Script eval reads what the module wrote — same realm, same global.
    let r = ctx.eval("globalThis.y;", "<script-after>").expect("script-after eval");
    assert_eq!(
        r.as_number(),
        Some(43.0),
        "module and script must share the same realm global (realm-per-context)"
    );
}

/// A module-registered side effect is observable by a subsequent module eval
/// in the same realm (no fresh global between modules).
#[test]
fn module_to_module_share_realm_globalthis() {
    let mut ctx = make_ctx();

    let global_ptr = thread_realm_global().or_else(|| {
        // Ensure realm exists (script path initializes it lazily).
        ctx.eval("void 0;", "<realm-init>").ok()?;
        thread_realm_global()
    }).expect("realm global");

    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    // First module plants a value.
    ModuleLoader::eval_module_in_realm(
        &mut cx,
        "globalThis.fromModule = 'persisted';",
        "<m1>.mjs",
        None,
        global.handle(),
    )
    .expect("module 1");

    // Second module reads it — same realm.
    ModuleLoader::eval_module_in_realm(
        &mut cx,
        "globalThis.fromModuleSeen = globalThis.fromModule === 'persisted';",
        "<m2>.mjs",
        None,
        global.handle(),
    )
    .expect("module 2");

    // Script confirms.
    let r = ctx
        .eval("globalThis.fromModuleSeen;", "<verify>")
        .expect("verify");
    assert_eq!(
        r.as_bool(),
        Some(true),
        "module-to-module globalThis must persist across module evals"
    );
}
