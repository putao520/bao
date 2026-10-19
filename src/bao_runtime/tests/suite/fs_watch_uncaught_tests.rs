// @trace REQ-ENG-007 [req:REQ-ENG-007] [level:integration]
//
// fs.watchFile / fs.watch callback throws route through the unified
// uncaught-exception router (node_fs.rs:3902 fsw_emit_on_entry →
// route_uncaught_exception). The node v24 oracle four-quadrant table
// (/tmp/throw-research/report.md §4) confirms bao behaviour matches node:
//   - no uncaughtException handler → default report (stderr stack, captured
//     here via uncaught::begin_capture) + exit requested + exit code 1
//   - handler registered → handler receives the thrown Error; the process
//     keeps running (exit untouched, later watch events still dispatch)
//
// Oracle evidence (node v24.19.0, /tmp/throw-research/oracle_*.js):
//   watchFile no-handler:  exit=1, stderr carries "watchfile-boom" + stack
//   watchFile with-handler: exit=0, handler called per throw, process alive
//   fs.watch no-handler:   exit=1, stderr carries "fs-watch-boom" + stack
//   fs.watch with-handler: exit=0, handler called per throw, process alive
//
// Pump contract: identical to fs_watch_events_tests (setup_ctx_bounded +
// bounded_drain_hook post-eval pumping). The pump MUST run inside ctx.eval
// via the post_eval_hook — the stat-poll dispatch (build_stats_object →
// JS_NewPlainObject) dereferences cx->global(), which is only valid while
// a realm is entered. A bare drain_and_check outside eval runs realm-less.

use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::setup_ctx_bounded as setup_ctx;
use common::HOOK_BUDGET;
use common::eval_string_dbg as eval_string;

/// Pump the loop until `js_condition` yields 'y' or the deadline passes.
/// Arms HOOK_BUDGET before each eval so the post_eval_hook drains timers
/// (including the fs-watch stat-poll pump) in the realm-entered context.
fn wait_until(ctx: &mut JsContext, js_condition: &str, per_eval_budget: usize) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        HOOK_BUDGET.with(|b| b.set(per_eval_budget));
        if eval_string(ctx, js_condition) == "y" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    false
}

/// Per-process unique scratch file: two invocations of this binary must not
/// delete each other's watched file mid-test.
fn scratch_file(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{}_{}.txt", name, std::process::id()))
}

fn js_escape(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

// ---------------------------------------------------------------------------
// Quadrant 1: fs.watchFile callback throw, NO uncaughtException handler
// ---------------------------------------------------------------------------

/// Oracle: node exits 1, stderr carries the throw message + stack.
#[test]
fn watch_file_throw_without_handler_prints_and_exits_1() {
    let mut ctx = setup_ctx();
    bun_runtime::clear_exit();

    let file = scratch_file("bao_watchfile_uncaught_no_handler");
    let _ = std::fs::remove_file(&file);
    std::fs::write(&file, "seed").unwrap();
    let f = js_escape(&file);

    let script = format!(
        r#"
var fs = require('fs');
globalThis.__fired = 0;
fs.watchFile("{f}", {{ interval: 60 }}, function (curr, prev) {{
  globalThis.__fired++;
  throw new Error("watchfile-no-handler-boom");
}});
setTimeout(function () {{ fs.writeFileSync("{f}", "changed-content-here"); }}, 120);
'ok'
"#,
        f = f
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");

    bun_runtime::uncaught::begin_capture();
    // Pump via the post_eval_hook (bounded_drain_hook fires inside ctx.eval,
    // realm-entered — the stat-poll dispatch needs a live realm global).
    let deadline = Instant::now() + Duration::from_secs(15);
    while !bun_runtime::should_exit() && Instant::now() < deadline {
        HOOK_BUDGET.with(|b| b.set(4));
        let _ = eval_string(&mut ctx, "'tick'");
    }
    let cap = bun_runtime::uncaught::take_capture();

    assert!(
        eval_string(&mut ctx, "String(globalThis.__fired)") != "0",
        "watchFile callback must have fired (and thrown)"
    );
    assert!(
        cap.contains("watchfile-no-handler-boom"),
        "default report must carry the watchFile callback's error message, got: {cap}"
    );
    assert!(
        cap.contains("Error:"),
        "default report must carry the Error framing, got: {cap}"
    );
    assert!(
        bun_runtime::should_exit(),
        "watchFile callback throw without handler must request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "watchFile callback throw without handler must exit 1"
    );

    let _ = std::fs::remove_file(&file);
    bun_runtime::shutdown_thread_sm();
}

// ---------------------------------------------------------------------------
// Quadrant 2: fs.watchFile callback throw, WITH uncaughtException handler
// ---------------------------------------------------------------------------

/// Oracle: handler called per throw, process continues, exit code stays 0.
#[test]
fn watch_file_throw_with_handler_receives_error_and_loop_continues() {
    let mut ctx = setup_ctx();
    bun_runtime::clear_exit();

    let file = scratch_file("bao_watchfile_uncaught_with_handler");
    let _ = std::fs::remove_file(&file);
    std::fs::write(&file, "seed").unwrap();
    let f = js_escape(&file);

    let script = format!(
        r#"
var fs = require('fs');
globalThis.__fired = 0;
globalThis.__handlerCalls = 0;
globalThis.__got = null;
process.on('uncaughtException', function (e) {{
  globalThis.__handlerCalls++;
  globalThis.__got = e && e.message;
}});
fs.watchFile("{f}", {{ interval: 60 }}, function (curr, prev) {{
  globalThis.__fired++;
  throw new Error("watchfile-handled-boom");
}});
setTimeout(function () {{ fs.writeFileSync("{f}", "change-number-one"); }}, 120);
setTimeout(function () {{ fs.writeFileSync("{f}", "change-number-two"); }}, 400);
'ok'
"#,
        f = f
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");

    // Pump until both writes have been observed (or deadline). Each eval's
    // post_eval_hook drains the stat-poll events realm-entered.
    let both_fired = wait_until(
        &mut ctx,
        "String(globalThis.__fired) === '2' ? 'y' : 'n'",
        80,
    );

    assert!(
        both_fired,
        "both watchFile writes must dispatch (and throw), fired={}",
        eval_string(&mut ctx, "String(globalThis.__fired)")
    );
    let handler_calls: u32 = eval_string(&mut ctx, "String(globalThis.__handlerCalls)")
        .parse()
        .unwrap_or(0);
    assert!(
        handler_calls >= 1,
        "uncaughtException handler must be called at least once, got {handler_calls}"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__got"),
        "watchfile-handled-boom",
        "uncaughtException handler must receive the thrown Error message"
    );
    assert!(
        !bun_runtime::should_exit(),
        "handled watchFile callback throw must not request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        0,
        "handled watchFile callback throw must leave exit code untouched"
    );

    let _ = eval_string(&mut ctx, &format!("require('fs').unwatchFile(\"{}\")", f));
    let _ = std::fs::remove_file(&file);
    bun_runtime::shutdown_thread_sm();
}

// ---------------------------------------------------------------------------
// Quadrant 3: fs.watch callback throw, NO uncaughtException handler
// ---------------------------------------------------------------------------

/// Oracle: node exits 1, stderr carries the throw message + stack.
#[test]
fn fs_watch_throw_without_handler_prints_and_exits_1() {
    let mut ctx = setup_ctx();
    bun_runtime::clear_exit();

    let file = scratch_file("bao_fswatch_uncaught_no_handler");
    let _ = std::fs::remove_file(&file);
    std::fs::write(&file, "seed").unwrap();
    let f = js_escape(&file);

    let script = format!(
        r#"
var fs = require('fs');
globalThis.__fired = 0;
globalThis.__w = fs.watch("{f}", function (eventType, filename) {{
  globalThis.__fired++;
  throw new Error("fs-watch-no-handler-boom");
}});
setTimeout(function () {{ fs.appendFileSync("{f}", "-more-data"); }}, 120);
'ok'
"#,
        f = f
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");

    bun_runtime::uncaught::begin_capture();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !bun_runtime::should_exit() && Instant::now() < deadline {
        HOOK_BUDGET.with(|b| b.set(4));
        let _ = eval_string(&mut ctx, "'tick'");
    }
    let cap = bun_runtime::uncaught::take_capture();

    assert!(
        eval_string(&mut ctx, "String(globalThis.__fired)") != "0",
        "fs.watch callback must have fired (and thrown)"
    );
    assert!(
        cap.contains("fs-watch-no-handler-boom"),
        "default report must carry the fs.watch callback's error message, got: {cap}"
    );
    assert!(
        cap.contains("Error:"),
        "default report must carry the Error framing, got: {cap}"
    );
    assert!(
        bun_runtime::should_exit(),
        "fs.watch callback throw without handler must request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "fs.watch callback throw without handler must exit 1"
    );

    let _ = eval_string(
        &mut ctx,
        "(function(){ try { globalThis.__w.close(); } catch(e) {} return 'closed'; })()",
    );
    let _ = std::fs::remove_file(&file);
    bun_runtime::shutdown_thread_sm();
}

// ---------------------------------------------------------------------------
// Quadrant 4: fs.watch callback throw, WITH uncaughtException handler
// ---------------------------------------------------------------------------

/// Oracle: handler called per throw, process continues, exit code stays 0.
#[test]
fn fs_watch_throw_with_handler_receives_error_and_loop_continues() {
    let mut ctx = setup_ctx();
    bun_runtime::clear_exit();

    let file = scratch_file("bao_fswatch_uncaught_with_handler");
    let _ = std::fs::remove_file(&file);
    std::fs::write(&file, "seed").unwrap();
    let f = js_escape(&file);

    let script = format!(
        r#"
var fs = require('fs');
globalThis.__fired = 0;
globalThis.__handlerCalls = 0;
globalThis.__got = null;
process.on('uncaughtException', function (e) {{
  globalThis.__handlerCalls++;
  globalThis.__got = e && e.message;
}});
globalThis.__w = fs.watch("{f}", function (eventType, filename) {{
  globalThis.__fired++;
  throw new Error("fs-watch-handled-boom");
}});
setTimeout(function () {{ fs.appendFileSync("{f}", "-first"); }}, 120);
setTimeout(function () {{ fs.appendFileSync("{f}", "-second"); }}, 400);
'ok'
"#,
        f = f
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");

    // Pump until both appends have been observed (or deadline).
    let both_fired = wait_until(
        &mut ctx,
        "String(globalThis.__fired) === '2' ? 'y' : 'n'",
        80,
    );

    assert!(
        both_fired,
        "both fs.watch appends must dispatch (and throw), fired={}",
        eval_string(&mut ctx, "String(globalThis.__fired)")
    );
    let handler_calls: u32 = eval_string(&mut ctx, "String(globalThis.__handlerCalls)")
        .parse()
        .unwrap_or(0);
    assert!(
        handler_calls >= 1,
        "uncaughtException handler must be called at least once, got {handler_calls}"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__got"),
        "fs-watch-handled-boom",
        "uncaughtException handler must receive the thrown Error message"
    );
    assert!(
        !bun_runtime::should_exit(),
        "handled fs.watch callback throw must not request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        0,
        "handled fs.watch callback throw must leave exit code untouched"
    );

    let _ = eval_string(
        &mut ctx,
        "(function(){ try { globalThis.__w.close(); } catch(e) {} return 'closed'; })()",
    );
    let _ = std::fs::remove_file(&file);
    bun_runtime::shutdown_thread_sm();
}
