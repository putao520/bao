// @trace TEST-ENG-023-ZONE [req:REQ-ENG-001] [level:integration]
//
// ISSUE #23 closeout: real Zone/Realm topology measurement under realm churn
// (S0-3 row #4 — every `ModuleLoader::eval_module` call creates a fresh
// global = fresh compartment+zone) plus multi-runtime isolation data
// (one Runtime+cx+zone per engine thread).
//
// Numbers produced here are the "Zone 实测数据" the #23 DoD row asks for —
// they are run, not documented. Zone strategy itself stays at the S0-4
// ruling (现状维持) until these numbers say otherwise.
//
// Single-#[test] discipline (ENGINE_HANDLE is thread-local), same as
// memory_stats_tests / execution_control_tests. Inner threads each own a
// Runtime via for_test's thread-local parasitism/new-creation.

use bao_engine::context::JsContext;
use bao_engine::memory_stats::collect_runtime_stats;
use mozjs::jsapi::{CurrentGlobalOrNull, JS_GetProperty};
use mozjs::rooted;
#[path = "common/mod.rs"]
mod common;

fn eval_string(cx: &mut JsContext, source: &str) -> String {
    common::eval_string_debug_named(cx, source, "<zone_topology>")
}
use bun_sm::ModuleLoader;

fn collect(cx: &mut JsContext) -> bao_engine::memory_stats::EngineMemoryStats {
    // SAFETY: cx.raw_cx() is this thread's live context; no JS executing.
    unsafe { collect_runtime_stats(cx.raw_cx()) }.expect("CollectRuntimeStats must succeed")
}

#[test]
fn test_zone_topology_measurement_all() {
    // ------------------------------------------------------------------
    // Face A — single-runtime realm churn: baseline → K fresh module
    // realms → collect. Each eval_module call is S0-3 row #4: fresh
    // global = fresh compartment+zone.
    // ------------------------------------------------------------------
    let mut cx = JsContext::for_test().expect("for_test");

    let before = collect(&mut cx);
    println!(
        "[zone-measure] baseline: zone_count={} realm_count={} zone_live_gc_things={} realm_live_gc_things={}",
        before.zone_count, before.realm_count, before.zone_live_gc_things, before.realm_live_gc_things
    );

    const CHURN: usize = 8;
    for i in 0..CHURN {
        let src = format!(
            "globalThis.__churn_epoch = {i}; export const churnEpoch = {i};"
        );
        let mut mcx = cx.cx();
        ModuleLoader::eval_module(&mut mcx, &src, &format!("zone_churn_{i}.mjs"), None, None)
            .unwrap_or_else(|e| panic!("churn module {i} must evaluate: {e:?}"));
    }

    let after = collect(&mut cx);
    println!(
        "[zone-measure] after {CHURN} module realms: zone_count={} realm_count={} zone_live_gc_things={} realm_live_gc_things={}",
        after.zone_count, after.realm_count, after.zone_live_gc_things, after.realm_live_gc_things
    );
    println!(
        "[zone-measure] churn delta: zone_delta=+{} realm_delta=+{}",
        after.zone_count.saturating_sub(before.zone_count),
        after.realm_count.saturating_sub(before.realm_count)
    );

    // Structural invariants (must hold regardless of zone sweeping policy):
    // the live runtime still meters, and the counts never regressed to zero.
    assert!(after.zone_count >= 1, "zone_count must stay >= 1");
    assert!(after.realm_count >= 1, "realm_count must stay >= 1");
    assert!(
        after.realm_count >= before.realm_count.min(1),
        "realm count must not go below the baseline floor"
    );

    // ------------------------------------------------------------------
    // Face B — capability isolation across module realms (persistent-side
    // observable faces): a module realm is a distinct global, so (negative)
    // module globals never appear in the persistent realm and (positive)
    // writes on a module realm's own global never land on the persistent
    // one either.
    // ------------------------------------------------------------------
    // Persistent realm marks itself first (the isolation positive anchor).
    cx.eval(
        "globalThis.__persistent_marker = 'persistent';",
        "mark.js",
    )
    .expect("persistent marker eval");

    let mut mcx = cx.cx();
    ModuleLoader::eval_module(
        &mut mcx,
        "globalThis.__module_marker = 'module';",
        "zone_cap_isolation.mjs",
        None,
        None,
    )
    .expect("capability module must evaluate");
    let persistent_view = eval_string(&mut cx, "typeof globalThis.__module_marker;");
    assert_eq!(
        persistent_view, "undefined",
        "module globals must not leak into the persistent realm"
    );

    let mut mcx = cx.cx();
    ModuleLoader::eval_module(
        &mut mcx,
        "globalThis.__probe_saw = (typeof __persistent_marker !== 'undefined');",
        "zone_cap_probe.mjs",
        None,
        None,
    )
    .expect("probe module must evaluate");
    let probe_view = eval_string(&mut cx, "typeof globalThis.__probe_saw;");
    assert_eq!(
        probe_view, "undefined",
        "a module realm's own-global write must not leak into the persistent realm"
    );

    let persistent_marker = eval_string(&mut cx, "globalThis.__persistent_marker;");
    assert_eq!(
        persistent_marker, "persistent",
        "persistent realm marker must stay live"
    );

    // ------------------------------------------------------------------
    // Face C — stale-object navigation analogue at engine level: after the
    // churn, an OLD module realm's global must not be reachable from the
    // persistent realm (stale handle negative), while the persistent realm
    // itself stays fully live (its marker still readable).
    // ------------------------------------------------------------------
    let stale_check = eval_string(
        &mut cx,
        "JSON.stringify([globalThis.__churn_epoch, globalThis.__module_marker, \
          globalThis.__persistent_marker])",
    );
    assert_eq!(
        stale_check, r#"[null,null,"persistent"]"#,
        "churned module globals must be stale (unreachable from the \
         persistent realm); persistent marker must stay live"
    );

    // ------------------------------------------------------------------
    // Face D — multi-runtime isolation: one Runtime+cx+zone per engine
    // thread; each thread meters its own topology independently.
    // ------------------------------------------------------------------
    const RUNTIMES: usize = 4;
    let mut handles = Vec::new();
    for t in 0..RUNTIMES {
        handles.push(std::thread::spawn(move || {
            let mut cx = JsContext::for_test().expect("thread for_test");
            cx.eval(
                &format!("globalThis.__rt = 'rt-{t}'; for (let i = 0; i < 1000; i++) {{ globalThis['k'+i] = i; }}"),
                "rt_load.js",
            )
            .expect("thread load eval");
            let stats = collect(&mut cx);
            println!(
                "[zone-measure] runtime-{t}: zone_count={} realm_count={} zone_live_gc_things={}",
                stats.zone_count, stats.realm_count, stats.zone_live_gc_things
            );
            assert!(stats.zone_count >= 1, "runtime-{t} must own >= 1 zone");
            assert!(stats.realm_count >= 1, "runtime-{t} must own >= 1 realm");
            (t, stats.zone_count, stats.realm_count)
        }));
    }
    for h in handles {
        let (t, zones, realms) = h.join().expect("runtime thread must not panic");
        println!("[zone-measure] runtime-{t} joined: zones={zones} realms={realms}");
    }

    // ------------------------------------------------------------------
    // Recorded summary (the deliverable numbers live in this output and in
    // the terminal report; they are run, not documented).
    // ------------------------------------------------------------------
    let final_stats = collect(&mut cx);
    println!(
        "[zone-measure] final: zone_count={} realm_count={} zone_live_gc_things={} gc_heap_gc_things={}",
        final_stats.zone_count,
        final_stats.realm_count,
        final_stats.zone_live_gc_things,
        final_stats.gc_heap_gc_things
    );
}
