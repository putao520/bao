// @trace TEST-ENG-006/007 [req:REQ-ENG-006 REQ-ENG-007] [level:integration]
//
// 终审 3 个可判定偏差的 polish 回归:
//   A. node:stream push(string) 字节模式 → 'data'/read() 投递 Buffer
//      (BCE-20260817-STREAM-STRCHUNK;Node readableAddChunk 语义:默认 utf8 /
//      显式 enc 参数字节断言;setEncoding 与 objectMode 例外保持 string;
//      非法 enc fail-closed TypeError)。
//   B. Bun.Glob scan/scanSync 字符串实参 = cwd 简写(上游 runtime/api/
//      glob.zig ScanOpts.fromJS → parseCWD;BCE-20260817-GLOB-STRCWD),
//      绝对 pattern 直枚举 + 非法实参/非法 cwd fail-closed 上游报错文案。
//   C. bun:sqlite backup 返回目标路径字符串(终审 probe 同款钉死;
//      与 wave_b_final_tests item 5 互补,防报告层再漏)。

use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use mozjs::rooted;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<polish3>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Microtask + timer + event-loop pump (stream_p0_fix_tests pattern verbatim):
/// the Readable flow start, async generators and promise callbacks all settle
/// through the job queue, so a realm-entered drain_and_check + tick + RunJobs
/// per iteration is required.
fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
    let cx_raw = ctx.raw_cx();
    for _ in 0..max_iters {
        {
            let mut cxm = ctx.cx();
            let global = bao_engine::context::thread_realm_global();
            if let Some(g) = global {
                rooted!(&in(cxm) let g_root = g);
                let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
                let realm_cx: &mut mozjs::context::JSContext = &mut realm;
                bun_runtime::timers::drain_and_check(realm_cx);
            } else {
                bun_runtime::timers::drain_and_check(&mut cxm);
            }
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ══════════════════════════════════════════════════════════════════════════
// A — stream push(string) delivers Buffers in byte mode
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_stream_push_string_delivers_buffer() {
    let mut ctx = setup_ctx();

    // A1. data events: default utf8, explicit enc arg, and hex enc all land
    //     as Buffers with exact byte payloads ('héllo' = 68 c3 a9 6c 6c 6f).
    ctx.eval(
        r#"
        var stream = require('stream');
        var r = new stream.Readable({ read() {} });
        r.push('héllo');
        r.push('x', 'utf8');
        r.push('deadbeef', 'hex');
        r.push(null);
        var seen = [];
        globalThis.__s1 = 'pending';
        r.on('data', function (c) { seen.push(c); });
        r.on('end', function () {
          function d(i) {
            var c = seen[i];
            return Buffer.isBuffer(c) ? 'B:' + c.toString('hex') : 'S:' + String(c);
          }
          globalThis.__s1 = seen.length + '|' + d(0) + '|' + d(1) + '|' + d(2);
        });
    "#,
        "<polish3>",
    )
    .expect("s1 setup");
    drive_event_loop(&mut ctx, 60);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__s1"),
        "3|B:68c3a96c6c6f|B:78|B:deadbeef",
        "byte-mode string pushes must deliver Buffers with exact encoded bytes"
    );

    // A2. read()/unshift() path: unshifted and pushed strings both surface
    //     as Buffers (synchronous — no pump needed).
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"
            var r2 = new stream.Readable({ read() {} });
            r2.push('b');
            r2.unshift('a');
            var u = r2.read();
            var v = r2.read();
            (Buffer.isBuffer(u) && Buffer.isBuffer(v)) + '|' + u.toString() + v.toString();
        "#
        ),
        "true|ab",
        "read()/unshift() must hand out Buffers for string chunks"
    );

    // A3. setEncoding stream: chunks stay strings (Node emits decoded
    //     strings on setEncoding streams).
    ctx.eval(
        r#"
        var r3 = new stream.Readable({ read() {} });
        r3.setEncoding('utf8');
        r3.push('abc');
        r3.push(null);
        var s3 = [];
        globalThis.__s3 = 'pending';
        r3.on('data', function (c) { s3.push(c); });
        r3.on('end', function () { globalThis.__s3 = (typeof s3[0]) + ':' + s3[0]; });
    "#,
        "<polish3>",
    )
    .expect("s3 setup");
    drive_event_loop(&mut ctx, 60);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__s3"),
        "string:abc",
        "setEncoding streams must keep emitting strings"
    );

    // A4. objectMode: strings pass through untouched.
    ctx.eval(
        r#"
        var r4 = new stream.Readable({ objectMode: true, read() {} });
        r4.push('plain');
        r4.push(null);
        var s4 = [];
        globalThis.__s4 = 'pending';
        r4.on('data', function (c) { s4.push(c); });
        r4.on('end', function () { globalThis.__s4 = (typeof s4[0]) + ':' + s4[0]; });
    "#,
        "<polish3>",
    )
    .expect("s4 setup");
    drive_event_loop(&mut ctx, 60);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__s4"),
        "string:plain",
        "objectMode pushes must not be converted to Buffers"
    );

    // A5. Transform cb(null, string): the pushed output becomes a Buffer.
    ctx.eval(
        r#"
        var t = new stream.Transform({ transform(c, e, cb) { cb(null, 'OUT'); } });
        var s5 = [];
        globalThis.__s5 = 'pending';
        t.on('data', function (c) { s5.push(c); });
        t.on('finish', function () {
          var c = s5[0];
          globalThis.__s5 = (Buffer.isBuffer(c) ? 'B:' + c.toString() : 'S:' + String(c));
        });
        t.end('q');
    "#,
        "<polish3>",
    )
    .expect("s5 setup");
    drive_event_loop(&mut ctx, 60);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__s5"),
        "B:OUT",
        "Transform string pushes must surface as Buffers"
    );

    // A6. Invalid encoding fails closed (Buffer.from TypeError propagates
    //     out of push — no silent fallback).
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"
            var r6 = new stream.Readable({ read() {} });
            try { r6.push('a', 'bogus-enc'); 'NO-THROW'; }
            catch (e) { e.constructor.name; }
        "#
        ),
        "TypeError",
        "push(str, bogusEncoding) must throw TypeError"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// B — Bun.Glob: string-arg cwd shorthand + absolute patterns + fail-closed
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_glob_string_cwd_and_absolute_patterns() {
    let mut ctx = setup_ctx();

    let tmp = ::tempfile::tempdir().expect("tempdir");
    let tmp_abs = tmp.path().to_string_lossy().into_owned();
    ::std::fs::write(tmp.path().join("a.js"), b"1").unwrap();
    ::std::fs::write(tmp.path().join("b.txt"), b"2").unwrap();
    ::std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
    ::std::fs::write(tmp.path().join("sub").join("c.js"), b"3").unwrap();
    ::std::fs::write(tmp.path().join(".hidden.js"), b"4").unwrap();
    let tmp_js = js_escape(&tmp_abs);

    // B1. String argument = cwd shorthand (upstream ScanOpts.fromJS): the
    //     scan must enumerate inside the passed directory, not the process
    //     cwd (the pre-fix scan ignored the string entirely).
    assert_eq!(
        eval_string(
            &mut ctx,
            &format!(r#"JSON.stringify(new Bun.Glob("*.js").scanSync("{}").sort())"#, tmp_js)
        ),
        r#"["a.js"]"#,
        "scanSync(cwdString) must scan the passed directory"
    );
    assert_eq!(
        eval_string(
            &mut ctx,
            &format!(
                r#"JSON.stringify(new Bun.Glob("**/*.js").scanSync("{}").sort())"#,
                tmp_js
            )
        ),
        r#"["a.js","sub/c.js"]"#,
        "scanSync(cwdString) recursive form"
    );

    // B2. Object form unchanged: cwd + dot filter.
    assert_eq!(
        eval_string(
            &mut ctx,
            &format!(
                r#"JSON.stringify(new Bun.Glob("**/*.js").scanSync({{ cwd: "{}", dot: true }}).sort())"#,
                tmp_js
            )
        ),
        r#"[".hidden.js","a.js","sub/c.js"]"#,
        "object form with dot:true must include dotfiles"
    );

    // B3. Absolute pattern string enumerates real paths (audit's literal
    //     probe form: pattern itself absolute).
    assert_eq!(
        eval_string(
            &mut ctx,
            &format!(
                r#"JSON.stringify(new Bun.Glob("{}/**/*.js").scanSync().sort())"#,
                tmp_js
            )
        ),
        format!(r#"["{}/a.js","{}/sub/c.js"]"#, tmp_js, tmp_js),
        "absolute pattern must enumerate absolute paths"
    );

    // B4. scan() async form with a string cwd, consumed end-to-end.
    ctx.eval(
        &format!(
            r#"
            globalThis.__g4 = 'pending';
            (async function() {{
              var acc = [];
              try {{
                for await (var p of new Bun.Glob("**/*.js").scan("{}")) acc.push(p);
                globalThis.__g4 = 'OK:' + acc.sort().join(',');
              }} catch (e) {{
                globalThis.__g4 = 'THREW:' + e.message;
              }}
            }})();
        "#,
            tmp_js
        ),
        "<polish3>",
    )
    .expect("g4 setup");
    drive_event_loop(&mut ctx, 80);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__g4"),
        "OK:a.js,sub/c.js",
        "scan(cwdString) async iteration must enumerate the directory"
    );

    // B5. Fail-closed argument validation (upstream messages): non-object/
    //     non-string arg and truthy non-string cwd both throw.
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"
            var e1 = 'NO-THROW', e2 = 'NO-THROW', e3 = 'NO-THROW';
            try { new Bun.Glob('*').scanSync(42); } catch (e) { e1 = e.name + ':' + e.message; }
            try { new Bun.Glob('*').scan(42); } catch (e) { e2 = e.name + ':' + e.message; }
            try { new Bun.Glob('*').scanSync({ cwd: 5 }); } catch (e) { e3 = e.message; }
            e1 + '|' + e2 + '|' + e3;
        "#
        ),
        "Error:scanSync: expected first argument to be an object|Error:scan: expected first argument to be an object|scanSync: invalid `cwd`, not a string",
        "invalid scan arguments must fail closed with upstream messages"
    );

    // B6. Nonexistent cwd shorthand yields empty (no throw, no fake hits).
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"JSON.stringify(new Bun.Glob("*.js").scanSync("/nonexistent-dir-polish3"))"#
        ),
        "[]",
        "nonexistent cwd string must yield empty"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// C — bun:sqlite backup returns the destination path (audit probe pin)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_sqlite_backup_returns_path_string() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var { Database } = require('bun:sqlite');
        var db = new Database(':memory:');
        db.exec('CREATE TABLE t(x); INSERT INTO t VALUES (42);');
        var path = require('os').tmpdir() + '/polish3-backup.db';
        try { require('fs').rmSync(path); } catch (e) {}
        var ret = db.backup(path);
        var reopened = 'unset';
        var db2 = new Database(path);
        reopened = db2.prepare('SELECT x FROM t').get().x;
        var dup;
        try { db.backup(path); dup = 'NO-THROW'; }
        catch (e) { dup = 'THREW:' + (e.message.indexOf('already exists') >= 0); }
        (typeof ret) + '|' + (ret === path) + '|' + reopened + '|' + dup;
    "#,
    );
    assert_eq!(
        out, "string|true|42|THREW:true",
        "backup must return the destination path string, write a real snapshot, and fail closed on duplicates"
    );
}
