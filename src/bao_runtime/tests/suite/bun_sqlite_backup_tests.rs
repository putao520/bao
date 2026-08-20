// @trace TEST-ENG-008-SQLITE [req:REQ-ENG-008] [level:integration]
// Database.backup(path) Promise contract tests — Bun truth:
//   backup(destination) → Promise<string>, resolving with the destination
//   path once the VACUUM INTO snapshot is written; runtime failures reject.
// The old implementation returned the path STRING synchronously (callers
// doing .then got a function-less string) and threw synchronously on failure.

use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<sqlite-backup>") {
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

/// Drive the JS thread's MiniEventLoop so already-settled promise .then jobs
/// run (RunJobs flushes the microtask queue; fetch e2e pattern).
fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
    let cx_raw = ctx.raw_cx();
    for _ in 0..max_iters {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn escape_path(p: &str) -> String {
    p.replace('\\', "\\\\").replace('"', "\\\"")
}

#[test]
fn test_sqlite_backup_promise_contract() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let uniq = std::process::id();
    let db_path = std::env::temp_dir().join(format!("bao_backup_src_{}.db", uniq));
    let backup_path = std::env::temp_dir().join(format!("bao_backup_dst_{}.db", uniq));
    let existing_path = std::env::temp_dir().join(format!("bao_backup_exists_{}.db", uniq));
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(&backup_path);
    // VACUUM INTO refuses existing targets — the failure-path fixture.
    std::fs::write(&existing_path, b"already here").unwrap();

    let db_s = escape_path(&db_path.to_string_lossy());
    let dst_s = escape_path(&backup_path.to_string_lossy());
    let exists_s = escape_path(&existing_path.to_string_lossy());

    eval_string(
        &mut ctx,
        &format!(
            r#"
globalThis.__r = {{}};
var {{ Database }} = require('bun:sqlite');
var db = new Database("{db_s}");
db.exec('CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, v TEXT)');
db.exec("INSERT INTO t (v) VALUES ('before-backup')");

// ── success: Promise<string> resolving with the destination path ──
var p = db.backup("{dst_s}");
// NOTE: not `p instanceof Promise` — node_async_hooks replaces the global
// Promise with a JS subclass, so engine-native promises (every native
// promise-returning API) fail instanceof against that binding. The honest
// observable contract: thenable + the real [object Promise] class tag.
__r.isPromise = (typeof p.then === 'function') + ':' +
                Object.prototype.toString.call(p) + ':' + (p.constructor && p.constructor.name);
p.then(
  function(resolved) {{ __r.ok = (typeof resolved === 'string') + ':' + resolved; }},
  function(e) {{ __r.ok = 'REJ:' + (e && e.message); }}
);

// ── failure: existing destination → rejected promise (not a sync throw) ──
var threw = false;
var p2;
try {{
  p2 = db.backup("{exists_s}");
}} catch (e) {{
  threw = true;
}}
__r.syncThrow = '' + threw;
if (p2) {{
  p2.then(
    function() {{ __r.fail = 'RESOLVED'; }},
    function(e) {{ __r.fail = 'REJ:' + (e instanceof Error) + ':' + (e && e.message); }}
  );
}}
"#
        ),
    );
    drive_event_loop(&mut ctx, 10);

    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.isPromise"),
        "true:[object Promise]:Promise",
        "backup() must return a real Promise (thenable + [object Promise] tag)"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.ok"),
        format!("true:{}", backup_path.to_string_lossy()),
        "backup() must resolve with the destination path string"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.syncThrow"),
        "false",
        "backup() to an existing file must not throw synchronously"
    );
    let fail = eval_string(&mut ctx, "globalThis.__r.fail");
    assert!(
        fail.starts_with("REJ:true:"),
        "backup() failure must reject with an Error, got: {}",
        fail
    );

    // External truth: the snapshot file exists on disk and is a real SQLite
    // database carrying the row (read through a direct rusqlite connection).
    assert!(backup_path.exists(), "snapshot file must exist");
    {
        let conn = rusqlite::Connection::open(&backup_path).expect("open snapshot");
        let v: String = conn
            .query_row("SELECT v FROM t ORDER BY id LIMIT 1", [], |row| row.get(0))
            .expect("query snapshot");
        assert_eq!(v, "before-backup", "snapshot must carry the source rows");
    }

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(&backup_path);
    let _ = std::fs::remove_file(&existing_path);
}
