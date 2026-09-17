// @trace TEST-ENG-005-RESOLVER-ROOT [req:REQ-ENG-005] [level:integration]
//
// B1 census row 27 (用户裁决 2026-09-18 B1「全部处理掉」): the resolver root
// (top_level_dir) is per-runtime. It used to be a process-global written once
// by the FIRST runtime's resolver install, so a second BaoRuntime created in
// a different directory read the first runtime's root through every
// consumption site (`bun_paths::fs::FileSystem::instance().top_level_dir()`
// → resolve_path relative* joins, Path::init_top_level_dir, dotenv's
// node/ccache lookup). The fix is a thread-local overlay in `bun_core`
// (read order: overlay → process-global), seeded per `BaoRuntime::new()` by
// `resolver_bridge::install_runtime_root()` and retired at drop with
// clear-if-same (an older runtime's drop must not erase a newer runtime's
// root — same parasitic-runtime shape as the CURRENT_RUNTIME_TOKEN discipline).
//
// Assertions are behavioral, through the real call chains:
//   T1: dual runtime (different cwds, each with its own require target):
//       the overlay follows the LATEST runtime on the thread; interleaved
//       `require('./probe.js')` resolves each runtime's probe under the cwd
//       its root represents; dropping the newer runtime (B) does NOT erase
//       the older one's root (A still serves its own probe + the bun_paths
//       delegation layer still reads A's root via the process-global
//       fallback); dropping A retires the last claim cleanly.
//   T2: sequential lifecycles — after a runtime drops, a fresh runtime in a
//       different directory re-seeds the overlay with ITS own root (never
//       inherits the previous runtime's), resolves from it, and its own drop
//       retires the overlay (raw `bun_core::top_level_dir()` back to the
//       process global `b"."` — runtime flows never write the global, so the
//       CLI/bundler fallback contract is untouched).

use std::fs;
use std::path::Path;

use bao_engine::value::JsValue;
use bun_runtime::BaoRuntime;

fn eval_str(rt: &mut BaoRuntime, code: &str) -> String {
    match rt.eval(code, "<resolver-root-test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {}", e.message),
    }
}

/// CommonJS probe module whose export names the root it lives under.
fn write_probe(dir: &Path, marker: &str) {
    fs::write(
        dir.join("probe.js"),
        format!("module.exports = {:?};\n", marker),
    )
    .expect("write probe.js");
}

fn root_string() -> String {
    String::from_utf8_lossy(bun_core::top_level_dir()).into_owned()
}

/// The bun_paths delegation layer — the single read point all four
/// consumption sites (resolve_path.rs, Path.rs, dotenv/env_loader.rs) go
/// through. With the overlay live it must equal it; overlay-cleared it falls
/// back to the first-init snapshot.
fn paths_fs_root_string() -> String {
    String::from_utf8_lossy(bun_paths::fs::FileSystem::instance().top_level_dir()).into_owned()
}

fn dir_string(dir: &Path) -> String {
    dir.to_str().expect("utf8 tmpdir").to_string()
}

/// T1: per-runtime root isolation with interleaved require, plus drop-B
/// (newer) survival of A's root.
#[test]
fn dual_runtime_resolver_root_isolated_and_survives_newer_drop() {
    let original_cwd = std::env::current_dir().expect("original cwd");
    let dir_a = tempfile::tempdir().expect("tmpdir a");
    let dir_b = tempfile::tempdir().expect("tmpdir b");
    write_probe(dir_a.path(), "A_PROBE");
    write_probe(dir_b.path(), "B_PROBE");

    // Runtime A installs under root a.
    std::env::set_current_dir(dir_a.path()).expect("chdir a");
    let mut rt_a = BaoRuntime::new().expect("runtime A");
    // Single-runtime zero-delta: the overlay equals the creating cwd and the
    // bun_paths delegation layer serves the same root.
    assert_eq!(root_string(), dir_string(dir_a.path()), "overlay must follow runtime A's cwd");
    assert_eq!(
        paths_fs_root_string(),
        dir_string(dir_a.path()),
        "bun_paths delegation must serve runtime A's root"
    );

    // Runtime B installs under root b — the overlay must move to B (the bug:
    // the process-global kept A's root for every later runtime).
    std::env::set_current_dir(dir_b.path()).expect("chdir b");
    let mut rt_b = BaoRuntime::new().expect("runtime B");
    assert_eq!(root_string(), dir_string(dir_b.path()), "overlay must follow runtime B's cwd");
    assert_eq!(
        paths_fs_root_string(),
        dir_string(dir_b.path()),
        "bun_paths delegation must serve runtime B's root while B is latest"
    );

    // Interleaved require: each runtime's require chain resolves ./probe.js
    // under the cwd its root represents (require from a bare eval anchors on
    // the process cwd), alternating between the two runtimes.
    std::env::set_current_dir(dir_a.path()).expect("chdir a");
    assert_eq!(eval_str(&mut rt_a, "require('./probe.js')"), "A_PROBE");
    std::env::set_current_dir(dir_b.path()).expect("chdir b");
    assert_eq!(eval_str(&mut rt_b, "require('./probe.js')"), "B_PROBE");
    std::env::set_current_dir(dir_a.path()).expect("chdir a");
    assert_eq!(eval_str(&mut rt_a, "require('./probe.js')"), "A_PROBE");

    // Drop the NEWER runtime: only B's claim may retire. A's root stays
    // served — through the bun_core global fallback ("." — runtime flows
    // never write the global) the bun_paths layer falls back to the
    // first-init snapshot, which is A's cwd.
    drop(rt_b);
    assert_eq!(root_string(), ".", "overlay must retire with B's drop (clear-if-same)");
    assert_eq!(
        paths_fs_root_string(),
        dir_string(dir_a.path()),
        "dropping B must not erase A's root"
    );
    std::env::set_current_dir(dir_a.path()).expect("chdir a");
    assert_eq!(eval_str(&mut rt_a, "require('./probe.js')"), "A_PROBE");

    // Dropping the last runtime retires its claim cleanly; the snapshot root
    // remains what the process-global fallback serves.
    drop(rt_a);
    assert_eq!(root_string(), ".");
    assert_eq!(paths_fs_root_string(), dir_string(dir_a.path()));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
}

/// T2: sequential lifecycles — a fresh runtime must re-seed the overlay with
/// ITS own root, never inherit the previous runtime's.
///
/// (Drop-order note: the first runtime on a thread owns the process
/// JSContext — later runtimes parasitize it, per the same constraint the
/// dgram/worker/child cleanup tests respect by always dropping the newer
/// runtime first. Dropping the owner while a parasite is alive destroys the
/// shared context under it, so the owner-drop-while-parasite-live form of the
/// clear-if-same check is exercised at the `bun_core` unit level, not here.)
#[test]
fn runtime_reinstall_seeds_fresh_root_after_previous_drop() {
    let original_cwd = std::env::current_dir().expect("original cwd");
    let dir_a = tempfile::tempdir().expect("tmpdir a");
    let dir_b = tempfile::tempdir().expect("tmpdir b");
    write_probe(dir_a.path(), "A_PROBE_T2");
    write_probe(dir_b.path(), "B_PROBE_T2");

    // First lifecycle: root a live, probe resolves under it.
    std::env::set_current_dir(dir_a.path()).expect("chdir a");
    let mut rt_a = BaoRuntime::new().expect("runtime A");
    assert_eq!(root_string(), dir_string(dir_a.path()));
    assert_eq!(eval_str(&mut rt_a, "require('./probe.js')"), "A_PROBE_T2");
    drop(rt_a);
    assert_eq!(root_string(), ".", "A's drop must retire its overlay claim");

    // Second lifecycle in a different directory: the overlay must be
    // re-seeded with b, not inherited from a.
    std::env::set_current_dir(dir_b.path()).expect("chdir b");
    let mut rt_b = BaoRuntime::new().expect("runtime B");
    assert_eq!(
        root_string(),
        dir_string(dir_b.path()),
        "a fresh runtime must seed its own root, not inherit the previous runtime's"
    );
    assert_eq!(
        paths_fs_root_string(),
        dir_string(dir_b.path()),
        "bun_paths delegation must serve the new runtime's root"
    );
    assert_eq!(eval_str(&mut rt_b, "require('./probe.js')"), "B_PROBE_T2");

    drop(rt_b);
    assert_eq!(root_string(), ".", "last drop retires the overlay");
    // The bun_paths snapshot still holds the FIRST init's root — the
    // process-global fallback contract is untouched by both lifecycles.
    assert_eq!(paths_fs_root_string(), dir_string(dir_a.path()));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
}
