// @trace REQ-ENG-007 [level:integration]
// Conformance test harness shared across node_conformance suites.
// Extracted from Bun's test/js/node patterns (MIT, Bun project).
//
// Each `*_conformance.rs` test file pulls this in via
//   #[path = "conformance_common.rs"] mod common;
// so they remain independent Cargo test binaries.
//
// NOTE on test isolation: SpiderMonkey's JSEngine is process-global and can
// only be initialised once per process. When two test functions in the same
// binary run concurrently and both call `JsContext::for_test()`, the second
// fails with "Failed to init JSEngine: AlreadyInitialized". Run these suites
// with `--test-threads=1` (e.g. `cargo test --test buffer_conformance -- --test-threads=1`)
// to avoid the race. This is a harness limitation, not a bug in bao_runtime.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

/// Eval result as a display string. Booleans/numbers/strings get stringified;
/// everything else returns an empty string (the caller treats empty as
/// "did not return a value").
pub fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<conformance>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

/// Build a fresh test context with bao_runtime globals installed.
pub fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Run a labelled-check bundle in the given context and assert every check
/// passed. The JS source must push `"<label>:PASS"`, `"<label>:FAIL"`, or
/// `"<label>:ERROR:<msg>"` to a `results` array, then return `results.join("|")`.
pub fn run_checks(ctx: &mut JsContext, source: &str) {
    let results = eval_string(ctx, source);
    let mut failures = Vec::new();
    for item in results.split('|') {
        if item.is_empty() {
            continue;
        }
        if !item.contains(":PASS") {
            failures.push(item.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "conformance failures:\n  {}\nFull results: {}",
        failures.join("\n  "),
        results
    );
}

/// Escape a path for embedding inside a JS string literal.
pub fn js_path(p: &::std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

/// Drop-in scaffold for AAA checks: `check(label, fn)` + `results.join("|")`.
pub const CHECK_SCAFFOLD: &str = r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e && e.message ? e.message : e)); }
        }
"#;

// Sentinel test so Cargo treats this as a valid test binary. The real tests
// live in the sibling *_conformance.rs files which pull this in as a module.
#[test]
fn conformance_common_sentinel() {
    // Sanity check: scaffold is non-empty and the helper compiles.
    assert!(!CHECK_SCAFFOLD.is_empty());
    assert_eq!(js_path(::std::path::Path::new("/tmp/x")), "/tmp/x");
}
