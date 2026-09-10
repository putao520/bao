// @trace REQ-ENG-011 [api:vm.createContext / vm.runInContext] [level:integration]
//
// GC rooting regression (SM-EVOLUTION #29; BCE class of the R2 opt-profile
// SIGSEVP / bao_browser `trace_node_realm_roots`): the vm context registry
// used to hold the sandbox realm's global as a raw `*mut JSObject` in a
// thread-local Vec — plain Rust memory invisible to SpiderMonkey's tracer.
// A full GC between `vm.createContext` and the next `vm.runInContext` could
// sweep the whole NewCompartmentAndZone realm; the next AutoRealm then
// dereferenced a freed zone (dev survives only by GC-timing luck, the
// opt-only SIGSEGV family). node_vm now carries the GC edge as a
// registered-symbol property on the sandbox object.
//
// This test drives the REAL production path across forced FULL GCs
// (JS_GC API reason), asserting: realms survive, realm identity persists
// (state set before the GC is readable after it), write-through still
// lands, fresh objects never false-positive isContext, and dropping the
// last sandbox reference keeps later contexts healthy (no permanent pin
// crash on collection).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<vm-gc-test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(other) => panic!("unexpected eval result: {:?}", other),
        Err(e) => panic!("eval failed: {} ({}:{}:{})", e.message, e.filename, e.line, e.column),
    }
}

/// Force a FULL GC on the context's runtime (mark + sweep across zones).
fn force_full_gc(ctx: &mut JsContext) {
    unsafe { mozjs::jsapi::JS_GC(ctx.raw_cx(), mozjs::jsapi::JS::GCReason::API) };
}

#[test]
fn test_vm_context_survives_full_gc() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Phase 1: create three contexts and stamp realm-side state.
    eval_string(
        &mut ctx,
        r#"
        var vm = require('vm');
        globalThis.sbs = [{a:1}, {b:2}, {c:3}];
        for (var sb of globalThis.sbs) vm.createContext(sb);
        vm.runInContext('globalThis.marker = "alive"; 1', globalThis.sbs[1]);
        "#,
    );

    // The hazard window: full GCs with the sandboxes referenced only from
    // JS (globalThis.sbs) and the realm global referenced only through the
    // context edge. Without the edge, the realm zone was swept here.
    force_full_gc(&mut ctx);
    force_full_gc(&mut ctx);

    // Phase 2: realms alive AND identical (marker survives) — a swept or
    // re-created realm could not answer this.
    let out = eval_string(
        &mut ctx,
        r#"
        var ids = [
            vm.runInContext('globalThis.marker', globalThis.sbs[1]),
            vm.runInContext('a+10', globalThis.sbs[0]),
            vm.runInContext('b+20', globalThis.sbs[1]),
            vm.runInContext('c+30', globalThis.sbs[2]),
        ];
        ids.join(',')
        "#,
    );
    assert_eq!(out, "alive,11,22,33", "realm state must survive full GC");

    // Phase 3: write-through (global → sandbox) still lands post-GC.
    let out = eval_string(
        &mut ctx,
        r#"
        vm.runInContext('var w = 7', globalThis.sbs[0]);
        globalThis.sbs[0].w
        "#,
    );
    assert_eq!(out, "7", "write-through must reach the sandbox after GC");

    // Phase 4: sandbox → global sync still lands post-GC (properties added
    // to the sandbox after createContext become realm globals).
    let out = eval_string(
        &mut ctx,
        r#"
        globalThis.sbs[2].late = 5;
        vm.runInContext('late * 2', globalThis.sbs[2])
        "#,
    );
    assert_eq!(out, "10", "sandbox→global sync must work after GC");

    // Phase 5: fresh objects never false-positive isContext (the registry
    // key used to be a raw address that dead entries could shadow).
    let out = eval_string(
        &mut ctx,
        r#"
        var junk = [];
        for (var i = 0; i < 64; i++) junk.push({});
        junk.every(function (o) { return !vm.isContext(o); })
        "#,
    );
    assert_eq!(out, "true", "plain objects must not be reported as contexts");

    // Phase 6: dropping the last sandbox reference releases the realms
    // (Node vm lifetime: context lives as long as the sandbox). Later
    // contexts stay healthy — collection must not corrupt the edge or the
    // runtime.
    let out = eval_string(
        &mut ctx,
        r#"
        globalThis.sbs = null;
        globalThis.junk = null;
        var fresh = {};
        vm.createContext(fresh);
        vm.runInContext('41 + 1', fresh)
        "#,
    );
    assert_eq!(out, "42", "fresh context after dropping dead ones must work");

    // Final full GCs: collected realms must not crash tracing or later eval.
    force_full_gc(&mut ctx);
    force_full_gc(&mut ctx);
    let out = eval_string(&mut ctx, "'post-gc:' + (6 * 7)");
    assert_eq!(out, "post-gc:42");
}

#[test]
fn test_vm_gc_churn_many_contexts() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Churn: interleaved create/use/GC over many contexts — the shape a
    // long-running process hits (per-request sandboxes). Every context
    // must stay usable across the GCs that follow its creation.
    for round in 0..4 {
        let src = format!(
            r#"
            var vm = require('vm');
            var round = {};
            var sbs = [];
            for (var i = 0; i < 8; i++) {{
                var sb = {{ n: round * 100 + i }};
                vm.createContext(sb);
                sbs.push(sb);
            }}
            var acc = [];
            for (var i = 0; i < sbs.length; i++)
                acc.push(vm.runInContext('n + 1', sbs[i]));
            acc.join(',')
            "#,
            round
        );
        let expected: Vec<String> = (0..8).map(|i| format!("{}", round * 100 + i + 1)).collect();
        let out = eval_string(&mut ctx, &src);
        assert_eq!(out, expected.join(","), "round {} pre-GC", round);
        // The eval script's `var` bindings are script-scoped and gone with
        // it — both the sandboxes and their realms are now unreachable, so
        // this full GC collects all 8 realms (lifetime = sandbox lifetime,
        // no permanent pin).
        force_full_gc(&mut ctx);
        let out = eval_string(&mut ctx, &format!("'r{}:' + {}", round, round + 1));
        assert_eq!(out, format!("r{}:{}", round, round + 1));
    }
}
