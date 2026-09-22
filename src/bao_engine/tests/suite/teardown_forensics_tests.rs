// @trace TEST-ENG-003-TEARDOWN [req:REQ-ENG-003] [level:integration]
// Teardown forensics: isolate which JS surface leaves a dangling exact stack
// root that AVs in JS::RootingContext::traceStackRoots at the destroy-time
// GC (Windows real machine, W8 深修② residual).
//
// Method: each step runs one isolated `JsContext::for_test()` session whose
// eval surface is a single candidate, then forces a full `JS_GC` before
// teardown — a stale stack root crashes at the FIRST collection, not only at
// destroyRuntime, so the per-step log pins the culprit to one eval. Each
// `#[test]` runs in its own nextest process; the step log is appended to
// `tdf.log` in the process CWD (C:\bao-win-test on the probe host).

use bao_engine::context::JsContext;
use mozjs::jsapi::{GCReason, JS_GC};

fn tdf_log(step: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("tdf.log")
    {
        let _ = writeln!(f, "{step}");
    }
}

/// One isolated session: fresh for_test context, single eval, forced GC,
/// explicit runtime teardown. Returns the eval result string for asserts.
fn session(name: &str, source: &str) -> String {
    tdf_log(&format!("[{name}] begin"));
    let mut ctx = JsContext::for_test().expect("for_test");
    let out = match ctx.eval(source, "<tdf>") {
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("ERR:{}", e.message),
    };
    tdf_log(&format!("[{name}] eval -> {out}"));
    unsafe {
        let raw = ctx.raw_cx();
        JS_GC(raw, GCReason::API);
    }
    tdf_log(&format!("[{name}] gc ok"));
    JsContext::shutdown_thread_sm();
    tdf_log(&format!("[{name}] teardown ok"));
    out
}

#[test]
fn tdf_00_control_no_native() {
    let out = session("00-control", "1 + 1");
    assert!(out.contains("2"), "{out}");
}

#[test]
fn tdf_01_typeof_console() {
    let out = session("01-typeof", "typeof console.log");
    assert_eq!(out.trim(), "String(\"function\")", "{out}");
}

#[test]
fn tdf_02_console_time() {
    // console.time writes nothing and calls zero SM APIs — pure Rust TLS.
    let out = session("02-time", "console.time('t'); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_03_console_time_end() {
    let out = session("03-timeEnd", "console.time('t'); console.timeEnd('t'); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_04_console_count() {
    let out = session("04-count", "console.count('c'); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_05_console_log_string() {
    let out = session("05-log", "console.log('hello'); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_06_console_trace() {
    let out = session("06-trace", "console.trace('tr'); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_07_console_dir_object() {
    let out = session("07-dir", "console.dir({a: 1}); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_08_console_assert() {
    let out = session("08-assert", "console.assert(false, 'x'); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_09_console_clear() {
    let out = session("09-clear", "console.clear(); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_10_console_table() {
    let out = session("10-table", "console.table([{n: 'a'}]); 'ok'");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn tdf_11_for_of() {
    let out = session("11-forof", "var s = 0; for (var x of [1,2,3]) s += x; s");
    assert!(out.contains("6"), "{out}");
}

#[test]
fn tdf_12_promise_catch() {
    let out = session("12-promise", "typeof Promise.reject('e').catch(function(e){ return e; })");
    assert!(out.contains("object"), "{out}");
}

#[test]
fn tdf_13_throw_catch() {
    let out = session("13-throw", "try { throw new Error('e'); } catch (ex) { ex.message; }");
    assert!(out.contains("e"), "{out}");
}

#[test]
fn tdf_14_nested_eval() {
    let out = session("14-nested-eval", r#"try { eval("function("); } catch (e) { e.name; }"#);
    assert!(out.contains("SyntaxError"), "{out}");
}

/// Full sequence in ONE context (mirrors test_host_fn_all's single-session
/// shape): if individual sessions are green but this crashes, the leak is
/// cumulative across evals in one realm.
#[test]
fn tdf_15_all_in_one_context() {
    tdf_log("[15-all] begin");
    let mut ctx = JsContext::for_test().expect("for_test");
    let srcs: [&str; 12] = [
        "1 + 1",
        "typeof console.log",
        "console.time('t'); console.timeEnd('t'); 'ok'",
        "console.count('c'); 'ok'",
        "console.log('hello'); 'ok'",
        "console.trace('tr'); 'ok'",
        "console.dir({a: 1}); 'ok'",
        "console.assert(false, 'x'); 'ok'",
        "console.clear(); 'ok'",
        "console.table([{n: 'a'}]); 'ok'",
        "var s = 0; for (var x of [1,2,3]) s += x; s",
        "try { throw new Error('e'); } catch (ex) { ex.message; }",
    ];
    for (i, src) in srcs.iter().enumerate() {
        match ctx.eval(src, "<tdf>") {
            Ok(v) => tdf_log(&format!("[15-all] step{i} -> {v:?}")),
            Err(e) => tdf_log(&format!("[15-all] step{i} -> ERR:{}", e.message)),
        }
        unsafe { JS_GC(ctx.raw_cx(), GCReason::API) };
        tdf_log(&format!("[15-all] step{i} gc ok"));
    }
    JsContext::shutdown_thread_sm();
    tdf_log("[15-all] teardown ok");
}

/// With-diagnostics GC sweep: run each candidate TWICE (two contexts) to
/// expose single-context vs. recycled-TLS differences.
#[test]
fn tdf_16_repeat_log() {
    for i in 0..3 {
        let out = session("16-log", "console.log('hello'); 'ok'");
        tdf_log(&format!("[16-repeat] round{i} -> {out}"));
        assert!(out.contains("ok"), "{out}");
    }
}
