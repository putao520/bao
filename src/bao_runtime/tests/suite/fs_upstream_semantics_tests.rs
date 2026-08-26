// @trace TEST-ENG-007-FS-UPSTREAM [req:REQ-ENG-007] [level:integration]
// Regression lock for two absorbed upstream node:fs semantics:
//
//  1. bun 4c815c11a5 — the truncate family treats an explicit `undefined`
//     len exactly like an absent one: truncate to 0 bytes (Node parity:
//     `if (len === undefined) len = 0`). Entry points locked here:
//     truncateSync / promises.truncate / ftruncateSync.
//
//  2. bun 83350172b9 — a path too long for ANY syscall rejects the promise
//     (and errors the callback) instead of throwing synchronously; the sync
//     forms still throw. On every face the error is identifiable:
//     code === 'ENAMETOOLONG' and err.path === the over-long input.

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

/// 4c815c11a5: fs.truncateSync(path, undefined) truncates to 0 — the same as
/// an absent len (parity between the two forms is asserted on a fresh file).
#[test]
fn truncate_sync_undefined_len_is_zero() {
    let mut ctx = setup_ctx();

    let dir = std::env::temp_dir().join("bao_fs_upstream_trunc_sync");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f1 = dir.join("undefined_len.txt");
    let f2 = dir.join("absent_len.txt");

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        fs.writeFileSync("{}", "hello world");
        fs.writeFileSync("{}", "hello world");
        var r1 = '', r2 = '';
        try {{ fs.truncateSync("{}", undefined); r1 = String(fs.statSync("{}").size); }}
        catch(e) {{ r1 = 'THREW:' + ((e && e.code) || e.message); }}
        try {{ fs.truncateSync("{}"); r2 = String(fs.statSync("{}").size); }}
        catch(e) {{ r2 = 'THREW:' + ((e && e.code) || e.message); }}
        r1 + ',' + r2
    "#,
            js_escape(&f1),
            js_escape(&f2),
            js_escape(&f1),
            js_escape(&f1),
            js_escape(&f2),
            js_escape(&f2)
        ),
    );
    assert_eq!(out, "0,0", "truncateSync undefined-len and absent-len must both truncate to 0");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 4c815c11a5: fs.promises.truncate(path, undefined) resolves after
/// truncating to 0 — never throws ERR_INVALID_ARG_TYPE synchronously.
#[test]
fn promises_truncate_undefined_len_is_zero() {
    let mut ctx = setup_ctx();

    let dir = std::env::temp_dir().join("bao_fs_upstream_trunc_prom");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f1 = dir.join("undefined_len.txt");

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        fs.writeFileSync("{}", "hello world");
        globalThis.__pt = {{ threw: false, settled: false, size: -1, code: null }};
        try {{
            fs.promises.truncate("{}", undefined).then(function() {{
                globalThis.__pt.settled = true;
                globalThis.__pt.size = fs.statSync("{}").size;
            }}, function(e) {{
                globalThis.__pt.settled = true;
                globalThis.__pt.code = (e && e.code) || String(e);
            }});
        }} catch(e) {{ globalThis.__pt.threw = true; globalThis.__pt.code = (e && e.code) || e.message; }}
        'scheduled'
    "#,
            js_escape(&f1),
            js_escape(&f1),
            js_escape(&f1)
        ),
    );
    assert_eq!(out, "scheduled");

    let state = eval_string(&mut ctx, "JSON.stringify(globalThis.__pt)");
    assert!(
        state.contains("\"threw\":false"),
        "promises.truncate(path, undefined) must not throw synchronously (state: {})",
        state
    );
    assert!(
        state.contains("\"settled\":true"),
        "promises.truncate promise never settled (state: {})",
        state
    );
    assert!(
        state.contains("\"size\":0"),
        "promises.truncate(path, undefined) must truncate to 0 (state: {})",
        state
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 4c815c11a5: fs.ftruncateSync(fd, undefined) truncates to 0.
#[test]
fn ftruncate_sync_undefined_len_is_zero() {
    let mut ctx = setup_ctx();

    let dir = std::env::temp_dir().join("bao_fs_upstream_ftrunc_sync");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f1 = dir.join("undefined_len.txt");

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        fs.writeFileSync("{}", "hello world");
        var r = '';
        var fd = fs.openSync("{}", 'r+');
        try {{ fs.ftruncateSync(fd, undefined); }}
        catch(e) {{ r = 'THREW:' + ((e && e.code) || e.message); }}
        fs.closeSync(fd);
        if (r === '') {{ r = String(fs.statSync("{}").size); }}
        r
    "#,
            js_escape(&f1),
            js_escape(&f1),
            js_escape(&f1)
        ),
    );
    assert_eq!(out, "0", "ftruncateSync(fd, undefined) must truncate to 0");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 83350172b9: fs.promises.stat(tooLong) REJECTS with code ENAMETOOLONG and
/// err.path === input — never a synchronous throw. Also pins the interplay
/// with 4c815c11a5: promises.truncate(tooLong, undefined) rejects the same
/// way instead of throwing.
#[test]
fn promises_toolong_path_rejects_not_throws() {
    let mut ctx = setup_ctx();

    // > PATH_MAX on every supported platform (4096 Linux / 1024 macOS) and a
    // single component > NAME_MAX(255): no syscall can accept this path.
    let too_long = format!("/tmp/{}", "a".repeat(5000));

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        var tooLong = "{}";
        globalThis.__tl = {{
            statThrew: false, statRejected: false, statCode: null, statPathOk: false,
            truncThrew: false, truncRejected: false, truncCode: null,
        }};
        try {{
            fs.promises.stat(tooLong).then(function() {{
                globalThis.__tl.statRejected = 'resolved';
            }}, function(e) {{
                globalThis.__tl.statRejected = true;
                globalThis.__tl.statCode = (e && e.code) || null;
                globalThis.__tl.statPathOk = (e && e.path === tooLong);
            }});
        }} catch(e) {{ globalThis.__tl.statThrew = true; globalThis.__tl.statCode = (e && e.code) || e.message; }}
        try {{
            fs.promises.truncate(tooLong, undefined).then(function() {{
                globalThis.__tl.truncRejected = 'resolved';
            }}, function(e) {{
                globalThis.__tl.truncRejected = true;
                globalThis.__tl.truncCode = (e && e.code) || null;
            }});
        }} catch(e) {{ globalThis.__tl.truncThrew = true; globalThis.__tl.truncCode = (e && e.code) || e.message; }}
        'scheduled'
    "#,
            too_long
        ),
    );
    assert_eq!(out, "scheduled");

    let state = eval_string(&mut ctx, "JSON.stringify(globalThis.__tl)");
    assert!(
        state.contains("\"statThrew\":false"),
        "fs.promises.stat(tooLong) must reject, not throw synchronously (state: {})",
        state
    );
    assert!(
        state.contains("\"statRejected\":true"),
        "fs.promises.stat(tooLong) promise never rejected (state: {})",
        state
    );
    assert!(
        state.contains("\"statCode\":\"ENAMETOOLONG\""),
        "rejection must carry code ENAMETOOLONG (state: {})",
        state
    );
    assert!(
        state.contains("\"statPathOk\":true"),
        "rejection must carry err.path === input (state: {})",
        state
    );
    assert!(
        state.contains("\"truncThrew\":false"),
        "fs.promises.truncate(tooLong, undefined) must reject, not throw (state: {})",
        state
    );
    assert!(
        state.contains("\"truncRejected\":true"),
        "fs.promises.truncate(tooLong, undefined) promise never rejected (state: {})",
        state
    );
    assert!(
        state.contains("\"truncCode\":\"ENAMETOOLONG\""),
        "truncate rejection must carry code ENAMETOOLONG (state: {})",
        state
    );
}

/// 83350172b9: the callback form delivers ENAMETOOLONG TO THE CALLBACK —
/// the call itself must not throw synchronously.
#[test]
fn callback_toolong_path_errors_callback_not_throw() {
    let mut ctx = setup_ctx();

    let too_long = format!("/tmp/{}", "b".repeat(5000));

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        globalThis.__cb = {{ threw: false, fired: false, code: null }};
        try {{
            fs.stat("{}", function(err, st) {{
                globalThis.__cb.fired = true;
                globalThis.__cb.code = err ? String((err && err.code) || err.message || err) : null;
            }});
        }} catch(e) {{ globalThis.__cb.threw = true; globalThis.__cb.code = (e && e.code) || e.message; }}
        'scheduled'
    "#,
            too_long
        ),
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    let state = eval_string(&mut ctx, "JSON.stringify(globalThis.__cb)");
    assert!(
        state.contains("\"threw\":false"),
        "fs.stat(tooLong, cb) must not throw synchronously (state: {})",
        state
    );
    assert!(
        state.contains("\"fired\":true"),
        "fs.stat(tooLong, cb) callback never delivered (state: {})",
        state
    );
    assert!(
        state.contains("\"code\":\"ENAMETOOLONG\""),
        "callback must receive code ENAMETOOLONG (state: {})",
        state
    );
}

/// 83350172b9: the SYNC form keeps throwing (node parity), now with an
/// identifiable code (ENAMETOOLONG) and err.path === input.
#[test]
fn stat_sync_toolong_throws_with_code_and_path() {
    let mut ctx = setup_ctx();

    let too_long = format!("/tmp/{}", "c".repeat(5000));

    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        var tooLong = "{}";
        var r = 'NOTHROW';
        try {{ fs.statSync(tooLong); }}
        catch(e) {{
            r = ((e && e.code) || 'nocode') + '|' + ((e && e.path === tooLong) ? 'pathok' : 'pathbad');
        }}
        r
    "#,
            too_long
        ),
    );
    assert_eq!(
        out, "ENAMETOOLONG|pathok",
        "statSync(tooLong) must throw code ENAMETOOLONG with err.path set"
    );
}
