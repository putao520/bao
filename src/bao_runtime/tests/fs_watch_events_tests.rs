// @trace TEST-ENG-007-FS-WATCH [req:REQ-ENG-007] [level:e2e]
// Real-event regression for fs.watch / fs.watchFile (BCE: silent-fake
// eradication). The former implementation returned an EventEmitter-shaped
// object with NO backend — registration succeeded and every file/directory
// write silently produced zero events. These tests assert REAL kernel-event
// delivery (inotify for fs.watch, stat polling for watchFile) through the
// production pump path (post_eval_hook → timers::drain_and_check →
// node_fs::fs_watch_pump_all), with real file writes in tempdirs.

use std::cell::Cell;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Ok(_) => "[other]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Bounded post-eval drain hook — the production pump path (timers +
/// fs.watch events ride the same drain_and_check tick).
fn bounded_drain_hook(cx: &mut mozjs::context::JSContext) -> bool {
    let exhausted = HOOK_BUDGET.with(|b| {
        let n = b.get();
        if n == 0 {
            return true;
        }
        b.set(n - 1);
        false
    });
    if exhausted {
        return false;
    }
    bun_runtime::timers::drain_and_check(cx)
}

/// Pump the loop until `js_condition` yields 'y' or the deadline passes.
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

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

fn js_escape(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

/// Per-process unique scratch dir: two invocations of this binary (e.g. two
/// terminals, or a rerun racing a still-running run) must not delete each
/// other's watched directory — a colliding cleanup surfaces as IN_DELETE_SELF
/// + IN_IGNORED ("rename:null") and kills the watch mid-test.
fn scratch_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{}_{}", name, std::process::id()))
}

/// fs.watch(directory, listener): create + append + rename on real files must
/// deliver eventType + filename events (inotify → pump → listener).
#[test]
fn test_fs_watch_directory_events_fire() {
    let mut ctx = setup_ctx();
    let dir = scratch_dir("bao_fswatch_dir_events");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = js_escape(&dir);

    let script = format!(
        r#"
var fs = require('fs');
globalThis.__ev = [];
var w = fs.watch("{d}", function (eventType, filename) {{
  __ev.push(eventType + ':' + filename);
}});
fs.writeFileSync("{d}/one.txt", "hello");
setTimeout(function () {{ fs.appendFileSync("{d}/one.txt", "-more"); }}, 80);
setTimeout(function () {{ fs.renameSync("{d}/one.txt", "{d}/two.txt"); }}, 160);
// Deadline gate = the full expected event set (NOT a fixed count): a count
// gate (>= 3) races under load — the append's modify event is the 3rd, so
// the gate can release before the 160ms rename's two.txt event is pumped.
globalThis.__done = function () {{
  return __ev.some(function (e) {{ return e === 'rename:one.txt'; }}) &&
         __ev.some(function (e) {{ return e === 'change:one.txt'; }}) &&
         __ev.some(function (e) {{ return e.indexOf('two.txt') !== -1; }});
}};
globalThis.__close = function () {{ w.close(); }};
'ok'
"#,
        d = d
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");

    let got_events = wait_until(
        &mut ctx,
        "(globalThis.__done && globalThis.__done()) ? 'y' : 'n'",
        80,
    );
    let events = eval_string(&mut ctx, "JSON.stringify(globalThis.__ev)");
    let _ = eval_string(&mut ctx, "(globalThis.__close && globalThis.__close(), 'closed')");
    assert!(
        got_events,
        "fs.watch(dir) must deliver create/modify/rename events, got: {}",
        events
    );
    // Node semantics: IN_CREATE → 'rename', IN_MODIFY → 'change'.
    assert!(
        events.contains("\"rename:one.txt\""),
        "create must surface as rename:<name>: {}",
        events
    );
    assert!(
        events.contains("\"change:one.txt\""),
        "append must surface as change:<name>: {}",
        events
    );
    assert!(
        events.contains("two.txt"),
        "rename must surface the new name: {}",
        events
    );
    let _ = std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}

/// fs.watch(file, listener): appends to the watched file must fire 'change'
/// with the file's basename.
#[test]
fn test_fs_watch_single_file_events_fire() {
    let mut ctx = setup_ctx();
    let dir = scratch_dir("bao_fswatch_file_events");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("watched.log");
    std::fs::write(&file, "seed").unwrap();
    let f = js_escape(&file);

    let script = format!(
        r#"
var fs = require('fs');
globalThis.__ev = [];
var w = fs.watch("{f}", function (eventType, filename) {{
  __ev.push(eventType + ':' + filename);
}});
setTimeout(function () {{ fs.appendFileSync("{f}", "appended"); }}, 60);
setTimeout(function () {{ fs.appendFileSync("{f}", "-again"); }}, 140);
// Deadline gate = the expected event itself (count gates race with the
// assertion set under load; see directory-events test above).
globalThis.__done = function () {{
  return __ev.some(function (e) {{ return e === 'change:watched.log'; }});
}};
globalThis.__close = function () {{ w.close(); }};
'ok'
"#,
        f = f
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");

    let got = wait_until(
        &mut ctx,
        "(globalThis.__done && globalThis.__done()) ? 'y' : 'n'",
        80,
    );
    let events = eval_string(&mut ctx, "JSON.stringify(globalThis.__ev)");
    let _ = eval_string(&mut ctx, "(globalThis.__close && globalThis.__close(), 'closed')");
    assert!(
        got && events.contains("change:watched.log"),
        "fs.watch(file) append must fire change:watched.log, got: {}",
        events
    );
    let _ = std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}

/// fs.watchFile: a real rewrite must call listener(curr, prev) with Stats
/// objects whose sizes reflect the before/after content.
#[test]
fn test_fs_watch_file_stat_polling_fires() {
    let mut ctx = setup_ctx();
    let dir = scratch_dir("bao_fswatch_watchfile");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("polled.txt");
    std::fs::write(&file, "0123456789").unwrap(); // 10 bytes
    let f = js_escape(&file);

    let script = format!(
        r#"
var fs = require('fs');
globalThis.__deltas = [];
fs.watchFile("{f}", {{ interval: 60 }}, function (curr, prev) {{
  __deltas.push(prev.size + '->' + curr.size);
}});
setTimeout(function () {{ fs.writeFileSync("{f}", "0123456789ABC"); }}, 120);
// Deadline gate = the expected delta itself: a stat poll landing mid-write
// can deliver "10->0" first, so a count gate (>= 1) would race the 10->13
// assertion under load.
globalThis.__done = function () {{ return __deltas.indexOf('10->13') !== -1; }};
'ok'
"#,
        f = f
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");

    let got = wait_until(
        &mut ctx,
        "(globalThis.__done && globalThis.__done()) ? 'y' : 'n'",
        80,
    );
    let deltas = eval_string(&mut ctx, "JSON.stringify(globalThis.__deltas)");
    let _ = eval_string(&mut ctx, &format!("fs.unwatchFile('{}')", f));
    assert!(
        got && deltas.contains("10->13"),
        "watchFile must deliver prev.size=10 -> curr.size=13, got: {}",
        deltas
    );
    let _ = std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}

/// w.close() must stop delivery: events after close never reach the listener.
#[test]
fn test_fs_watch_close_stops_events() {
    let mut ctx = setup_ctx();
    let dir = scratch_dir("bao_fswatch_close");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = js_escape(&dir);

    let script = format!(
        r#"
var fs = require('fs');
globalThis.__ev = [];
var w = fs.watch("{d}", function (eventType, filename) {{
  __ev.push(eventType + ':' + filename);
}});
w.close();
fs.writeFileSync("{d}/after-close.txt", "x");
setTimeout(function () {{ fs.writeFileSync("{d}/after-close-2.txt", "y"); }}, 120);
'ok'
"#,
        d = d
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");
    // Give the (closed) watcher plenty of pump iterations to prove silence.
    std::thread::sleep(Duration::from_millis(400));
    HOOK_BUDGET.with(|b| b.set(50));
    let _ = eval_string(&mut ctx, "(function(){ for(var i=0;i<20;i++) setTimeout(function(){}, 1); return 'pumped'; })()");
    std::thread::sleep(Duration::from_millis(200));
    let events = eval_string(&mut ctx, "JSON.stringify(globalThis.__ev)");
    assert_eq!(
        events, "[]",
        "no events may arrive after watcher.close(): {}",
        events
    );
    let _ = std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}

/// fs.watch on a missing path must throw (explicit ENOENT — never a silent
/// fake watcher), and recursive:true must throw explicitly (registered
/// limitation: no recursive inotify support).
#[test]
fn test_fs_watch_error_surfaces() {
    let mut ctx = setup_ctx();
    let missing = js_escape(&std::env::temp_dir().join("bao_fswatch_no_such_dir_xyz"));

    let enoent = eval_string(
        &mut ctx,
        &format!(
            r#"(function () {{ try {{ fs_watch_test_require(); }} catch (e) {{}}
var fs = require('fs');
try {{ fs.watch("{m}"); return 'no-throw'; }} catch (e) {{ return (e && e.message) || String(e); }} }})()"#,
            m = missing
        ),
    );
    assert!(
        enoent.contains("ENOENT") || enoent.contains("watch"),
        "fs.watch on a missing path must throw, got: {}",
        enoent
    );

    let recursive = eval_string(
        &mut ctx,
        r#"(function () {
var fs = require('fs');
try { fs.watch(require('os').tmpdir(), { recursive: true }); return 'no-throw'; }
catch (e) { return 'throw:' + String(e && e.message).slice(0, 60); } })()"#,
    );
    assert!(
        recursive.starts_with("throw:"),
        "recursive:true must throw explicitly (registered limitation), got: {}",
        recursive
    );
    bun_runtime::shutdown_thread_sm();
}

/// Same-path multi-watcher independence (Node semantics: every fs.watch
/// watcher on a path receives its own events). inotify returns the SAME wd
/// for repeated add_watch on one path within one fd — a wd→single-id map made
/// the FIRST registrant silently lose all events to the overwrite, and an
/// unrefcounted close of one watcher tore down the shared kernel watch. This
/// test covers BOTH registration orders, per-watcher event delivery, and
/// close independence (closed watcher goes silent, survivor keeps receiving).
#[test]
fn test_fs_watch_multi_watcher_same_path_independent() {
    let mut ctx = setup_ctx();
    let dir = scratch_dir("bao_fswatch_multi");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("multi.txt");
    std::fs::write(&file, "init").unwrap();
    let f = js_escape(&file);

    // Order 1: listener-only first, then a second listener-only watcher.
    let script = format!(
        r#"
var fs = require('fs');
globalThis.__a = []; globalThis.__b = []; globalThis.__c = []; globalThis.__d = [];
globalThis.__w1 = fs.watch("{f}", function (et) {{ __a.push(et); }});
globalThis.__w2 = fs.watch("{f}", function (et) {{ __b.push(et); }});
'ok'
"#,
        f = f
    );
    assert_eq!(eval_string(&mut ctx, &script), "ok");

    // Phase 1: one append must reach BOTH watchers (>= 1 each).
    let _ = eval_string(
        &mut ctx,
        &format!(r#"require('fs').appendFileSync("{}", "-p1"); 'wrote'"#, f),
    );
    let both = wait_until(
        &mut ctx,
        "(globalThis.__a.length >= 1 && globalThis.__b.length >= 1) ? 'y' : 'n'",
        80,
    );
    let a1 = eval_string(&mut ctx, "JSON.stringify(globalThis.__a)");
    let b1 = eval_string(&mut ctx, "JSON.stringify(globalThis.__b)");
    assert!(
        both && a1.contains("change") && b1.contains("change"),
        "same-path multi-watch order1: BOTH watchers must receive the append, first={} second={}",
        a1,
        b1
    );

    // Phase 2: close independence — w1 closes, w2 must keep receiving while
    // w1 stays silent (refcounted kernel-watch teardown).
    let _ = eval_string(
        &mut ctx,
        &format!(
            r#"(function () {{
globalThis.__aF = globalThis.__a.length;
globalThis.__bF = globalThis.__b.length;
globalThis.__w1.close();
require('fs').appendFileSync("{f}", "-p2");
return 'closed'; }})()"#,
            f = f
        ),
    );
    let survivor = wait_until(
        &mut ctx,
        "(globalThis.__b.length > globalThis.__bF) ? 'y' : 'n'",
        80,
    );
    // Silence window for the closed watcher: pump timers, then assert frozen.
    std::thread::sleep(Duration::from_millis(300));
    HOOK_BUDGET.with(|b| b.set(50));
    let _ = eval_string(
        &mut ctx,
        "(function(){ for(var i=0;i<20;i++) setTimeout(function(){}, 1); return 'pumped'; })()",
    );
    std::thread::sleep(Duration::from_millis(200));
    let frozen = eval_string(
        &mut ctx,
        "globalThis.__a.length + '/' + globalThis.__aF + '|' + globalThis.__b.length + '/' + globalThis.__bF",
    );
    // "a/aF|b/bF": closed w1 frozen (a == aF), survivor w2 grew (b > bF).
    let parts: Vec<&str> = frozen.split('|').collect();
    let closed_silent = parts.len() == 2
        && parts[0].split('/').count() == 2
        && parts[0].split('/').nth(0) == parts[0].split('/').nth(1);
    assert!(
        survivor && closed_silent,
        "close independence: closed watcher must stay silent (a==aF) while the survivor keeps receiving: {}",
        frozen
    );

    // Phase 3: reverse order — options-object watcher FIRST, listener-only
    // second. The first registrant must not lose events here either.
    let _ = eval_string(
        &mut ctx,
        &format!(
            r#"(function () {{
globalThis.__w3 = require('fs').watch("{f}", {{}}, function (et) {{ __c.push(et); }});
globalThis.__w4 = require('fs').watch("{f}", function (et) {{ __d.push(et); }});
require('fs').appendFileSync("{f}", "-p3");
return 'registered'; }})()"#,
            f = f
        ),
    );
    let both_rev = wait_until(
        &mut ctx,
        "(globalThis.__c.length >= 1 && globalThis.__d.length >= 1) ? 'y' : 'n'",
        80,
    );
    let c1 = eval_string(&mut ctx, "JSON.stringify(globalThis.__c)");
    let d1 = eval_string(&mut ctx, "JSON.stringify(globalThis.__d)");
    assert!(
        both_rev && c1.contains("change") && d1.contains("change"),
        "same-path multi-watch reverse order: BOTH watchers must receive, optsFirst={} listenerSecond={}",
        c1,
        d1
    );

    let _ = eval_string(
        &mut ctx,
        "(function(){ [globalThis.__w2, globalThis.__w3, globalThis.__w4].forEach(function (w) { try { w.close(); } catch (e) {} }); return 'closed'; })()",
    );
    let _ = std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}

/// The watcher object surface: EventEmitter methods + close; watchFile
/// returns a StatWatcher with close(); unwatchFile stops polling.
#[test]
fn test_fs_watch_surface_shapes() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"(function () {
var fs = require('fs');
var os = require('os');
var path = require('path');
var dir = fs.mkdtempSync(path.join(os.tmpdir(), 'fsw-surface-'));
var w = fs.watch(dir);
var shape = ['on','once','off','removeListener','addListener','emit','close']
  .every(function (m) { return typeof w[m] === 'function'; });
var sw = fs.watchFile(path.join(dir, 'any.txt'), { interval: 50 }, function () {});
var swShape = typeof sw.on === 'function' && typeof sw.close === 'function';
w.close();
fs.unwatchFile(path.join(dir, 'any.txt'));
return 'shape=' + shape + ' statwatcher=' + swShape;
})()"#,
    );
    assert_eq!(out, "shape=true statwatcher=true", "watcher surface: {}", out);
    bun_runtime::shutdown_thread_sm();
}
