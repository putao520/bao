// @trace TEST-ENG-007 [req:REQ-ENG-007] [level:integration]
// fs/crypto async callback DELIVERY tests — the A' ConcurrentTask carrier
// root-cure (user-adjudicated 2026-08-21).
//
// Pre-cure reality (three-fold break, analyze-defer-gap briefing):
//   1. fs/crypto worker completions called `uws_loop_defer` on the WORKER
//      thread's private uWS::Loop instance (Loop.h thread-local lazy init)
//      — never the JS thread's loop, so the defer never reached a pump.
//   2. Even on the right loop, the C tick early-returns when
//      num_polls == 0 (epoll_kqueue.c:355) — a JS thread with no sockets
//      never drains the defer queue.
//   3. The pump's liveness verdict (timers.rs drain_and_check) did not
//      include fs/crypto pending work — the eval loop broke before any
//      late completion could be delivered.
//
// These tests assert the USER-VISIBLE contract: a callback passed to
// fs.readFile / crypto.pbkdf2 actually RUNS (sets observable state) after
// the event loop is pumped. Effect-based filesystem polling (as in
// upstream_bun_semantic_port_tests) cannot see this — the worker performs
// the I/O regardless of callback delivery.

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
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Production-shaped pump: keep calling `timers::drain_and_check` while the
/// loop reports liveness (this is the eval loop's keep-alive contract). Once
/// the verdict is false the production loop would exit — every callback that
/// has not fired by then is lost, which is exactly the pre-cure bug.
fn pump_until_quiescent(ctx: &mut JsContext, deadline_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    while std::time::Instant::now() < deadline {
        let mut cxm = ctx.cx();
        if !bun_runtime::timers::drain_and_check(&mut cxm) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn js_escape(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

/// fs.readFile(path, 'utf8', cb): the callback must be DELIVERED on the JS
/// thread (fired flag) with the file content and no error.
#[test]
fn fs_async_callback_readfile_utf8_delivered() {
    let mut ctx = setup_ctx();

    let dir = std::env::temp_dir().join("bao_fs_async_cb_readfile");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hello.txt");
    std::fs::write(&path, "hello fs async").unwrap();

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        globalThis.__rf = {{ fired: false, err: null, data: null }};
        fs.readFile("{}", "utf8", function(err, data) {{
            globalThis.__rf.fired = true;
            globalThis.__rf.err = err ? String((err && err.code) || err.message || err) : null;
            globalThis.__rf.data = (typeof data === 'string') ? data : '<nonstring:' + typeof data + '>';
        }});
        'scheduled'
    "#,
            js_escape(&path)
        ),
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    let state = eval_string(&mut ctx, "JSON.stringify(globalThis.__rf)");
    assert!(
        state.contains("\"fired\":true"),
        "fs.readFile callback never delivered (state: {})",
        state
    );
    assert!(
        state.contains("\"err\":null"),
        "fs.readFile callback delivered an error (state: {})",
        state
    );
    assert!(
        state.contains("hello fs async"),
        "fs.readFile callback delivered wrong content (state: {})",
        state
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// fs.readFile on a missing path: the ERROR callback must be delivered with
/// err.code === 'ENOENT' (the Err arm of the carrier, not a silent drop).
#[test]
fn fs_async_callback_readfile_enoent_error_delivered() {
    let mut ctx = setup_ctx();

    let missing = std::env::temp_dir().join("bao_fs_async_cb_missing_definitely_absent.txt");
    let _ = std::fs::remove_file(&missing);

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        globalThis.__re = {{ fired: false, code: null }};
        fs.readFile("{}", function(err, data) {{
            globalThis.__re.fired = true;
            globalThis.__re.code = err ? String((err && err.code) || err.message || err) : null;
        }});
        'scheduled'
    "#,
            js_escape(&missing)
        ),
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    let state = eval_string(&mut ctx, "JSON.stringify(globalThis.__re)");
    assert!(
        state.contains("\"fired\":true"),
        "fs.readFile error callback never delivered (state: {})",
        state
    );
    assert!(
        state.contains("\"code\":\"ENOENT\""),
        "fs.readFile error callback must carry ENOENT (state: {})",
        state
    );
}

/// fs.promises.readFile regression (comparison): the promises face resolves
/// through its synchronous bun_fs::read path — the carrier change must not
/// disturb it. The then-reaction runs during the first eval's post-script
/// job drain; state is asserted from the follow-up eval.
#[test]
fn fs_async_callback_promise_readfile_regression() {
    let mut ctx = setup_ctx();

    let dir = std::env::temp_dir().join("bao_fs_async_cb_promises");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("promises.txt");
    std::fs::write(&path, "promise payload").unwrap();

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        globalThis.__pv = 'unset';
        fs.promises.readFile("{}", "utf8").then(
            function(d) {{ globalThis.__pv = d; }},
            function(e) {{ globalThis.__pv = 'ERR:' + ((e && e.message) || e); }}
        );
        'scheduled'
    "#,
            js_escape(&path)
        ),
    );
    assert_eq!(out, "scheduled");

    let state = eval_string(&mut ctx, "globalThis.__pv");
    assert_eq!(
        state, "promise payload",
        "fs.promises.readFile must still resolve with file content"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Sync-path regression (comparison): readFileSync must be untouched by the
/// carrier change.
#[test]
fn fs_async_callback_readfilesync_regression() {
    let mut ctx = setup_ctx();

    let dir = std::env::temp_dir().join("bao_fs_async_cb_sync");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sync.txt");
    std::fs::write(&path, "sync body").unwrap();

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"require('fs').readFileSync("{}", "utf8")"#,
            js_escape(&path)
        ),
    );
    assert_eq!(out, "sync body");

    let _ = std::fs::remove_dir_all(&dir);
}

/// crypto.pbkdf2(password, salt, iterations, keylen, digest, cb): the async
/// crypto carrier (same ConcurrentTask shape as fs) must deliver the
/// callback on the JS thread with a 32-byte derived key.
#[test]
fn fs_async_callback_crypto_pbkdf2_delivered() {
    let mut ctx = setup_ctx();

    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        globalThis.__pk = { fired: false, err: null, len: 0 };
        crypto.pbkdf2("password", "salt", 1000, 32, "sha256", function(err, key) {
            globalThis.__pk.fired = true;
            globalThis.__pk.err = err ? String((err && err.message) || err) : null;
            globalThis.__pk.len = (key && key.length) ? key.length : 0;
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    let state = eval_string(&mut ctx, "JSON.stringify(globalThis.__pk)");
    assert!(
        state.contains("\"fired\":true"),
        "crypto.pbkdf2 callback never delivered (state: {})",
        state
    );
    assert!(
        state.contains("\"err\":null"),
        "crypto.pbkdf2 callback delivered an error (state: {})",
        state
    );
    assert!(
        state.contains("\"len\":32"),
        "crypto.pbkdf2 callback must deliver a 32-byte key (state: {})",
        state
    );
}
