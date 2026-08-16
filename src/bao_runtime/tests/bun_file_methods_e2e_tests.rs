// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// Bun.file BunFile read-method family e2e: text()/json()/arrayBuffer()/
// slice() over REAL temp files — roundtrips (UTF-8 multibyte, exact bytes),
// missing-file ENOENT rejections (promise reject + sync slice throw), the fd
// form (reads the descriptor, cursor-preserving), and bad-JSON SyntaxError
// rejection. Every check drives the real FS path and asserts observable
// behavior (no typeof-only checks).
//
// Single #[test] body (mozjs thread-singleton rule, same as bun_api_tests).

use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<bunfile>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Drive the JS thread's MiniEventLoop so already-settled promise .then jobs
/// run (fetch e2e pattern — RunJobs flushes the microtask queue).
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
fn test_bun_file_method_family() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let tmp = std::env::temp_dir();
    let uniq = std::process::id();
    let text_path = tmp.join(format!("bao_bunfile_text_{}.txt", uniq));
    let json_path = tmp.join(format!("bao_bunfile_json_{}.json", uniq));
    let bin_path = tmp.join(format!("bao_bunfile_bin_{}.bin", uniq));
    let badjson_path = tmp.join(format!("bao_bunfile_badjson_{}.json", uniq));
    let missing_path = tmp.join(format!("bao_bunfile_missing_{}.nope", uniq));

    // "0123456789ABC包" — 13 ASCII bytes + 3 UTF-8 bytes for 包 = 16 bytes.
    // Byte range [2,7) = "23456" (ASCII-clean for the slice decode check);
    // the last 3 bytes are exactly 包.
    let text_content = "0123456789ABC包";
    std::fs::write(&text_path, text_content.as_bytes()).unwrap();
    std::fs::write(&json_path, "{\"name\":\"包子\",\"n\":42,\"ok\":true}".as_bytes()).unwrap();
    std::fs::write(&bin_path, [0x00u8, 0x01, 0xFE, 0xFF, 0x41, 0x80]).unwrap();
    std::fs::write(&badjson_path, b"{not json").unwrap();
    let _ = std::fs::remove_file(&missing_path);

    let t = escape_path(&text_path.to_string_lossy());
    let j = escape_path(&json_path.to_string_lossy());
    let b = escape_path(&bin_path.to_string_lossy());
    let bj = escape_path(&badjson_path.to_string_lossy());
    let m = escape_path(&missing_path.to_string_lossy());

    // ── text(): exact UTF-8 string (multibyte content survives) ──
    eval_string(
        &mut ctx,
        &format!(
            r#"
        globalThis.__r = {{}};
        Bun.file("{t}").text().then(
          function(s) {{ __r.text = s; }},
          function(e) {{ __r.text = 'REJ:' + e.code; }}
        );
    "#
        ),
    );
    drive_event_loop(&mut ctx, 10);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.text"),
        text_content,
        "text() must resolve the exact file content (UTF-8 multibyte intact)"
    );

    // ── json(): parsed object roundtrip ──
    eval_string(
        &mut ctx,
        &format!(
            r#"
        Bun.file("{j}").json().then(
          function(o) {{ __r.json = JSON.stringify(o); }},
          function(e) {{ __r.json = 'REJ:' + e.code; }}
        );
    "#
        ),
    );
    drive_event_loop(&mut ctx, 10);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.json"),
        r#"{"name":"包子","n":42,"ok":true}"#,
        "json() must resolve the parsed object"
    );

    // ── arrayBuffer(): byte-exact Uint8Array (high bytes unmangled) ──
    eval_string(
        &mut ctx,
        &format!(
            r#"
        Bun.file("{b}").arrayBuffer().then(
          function(u) {{
            __r.ab = (u instanceof Uint8Array) + ':' + u.length + ':' +
                     Array.prototype.join.call(u, ',');
          }},
          function(e) {{ __r.ab = 'REJ:' + e.code; }}
        );
    "#
        ),
    );
    drive_event_loop(&mut ctx, 10);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.ab"),
        "true:6:0,1,254,255,65,128",
        "arrayBuffer() must resolve a byte-exact Uint8Array (0xFE/0xFF/0x80 survive)"
    );

    // ── slice(): real Blob, range + negative clamp + contentType ──
    let slice_shape = eval_string(
        &mut ctx,
        &format!(
            r#"
        var b1 = Bun.file("{t}").slice(2, 7, 'text/plain');
        var b2 = Bun.file("{t}").slice(-3);
        var b3 = Bun.file("{t}").slice(0, 4);
        b1.text().then(function(s) {{ __r.sliceText = s; }});
        (b1 instanceof Blob) + ':' + b1.size + ':' + b1.type + '|' +
        (b2 instanceof Blob) + ':' + b2.size + '|' +
        b3.type;
    "#
        ),
    );
    assert_eq!(
        slice_shape,
        "true:5:text/plain|true:3|",
        "slice() returns real Blobs with clamped sizes; contentType honored, default empty"
    );
    drive_event_loop(&mut ctx, 10);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.sliceText"),
        "23456",
        "slice(2,7) content must be the exact byte range decoded as UTF-8"
    );

    // ── missing file: reject ENOENT (consistent with exists() → false) ──
    eval_string(
        &mut ctx,
        &format!(
            r#"
        var miss = '{{';
        Promise.all([
          Bun.file("{m}").text().then(
            function() {{ return 'RESOLVED'; }},
            function(e) {{ return e.code + ':' + (e instanceof Error); }}
          ),
          Bun.file("{m}").json().then(
            function() {{ return 'RESOLVED'; }},
            function(e) {{ return e.code; }}
          ),
          Bun.file("{m}").arrayBuffer().then(
            function() {{ return 'RESOLVED'; }},
            function(e) {{ return e.code; }}
          ),
        ]).then(function(r) {{ __r.miss = r.join('|'); }});
        try {{
          Bun.file("{m}").slice(0, 2);
          __r.missSlice = 'NO-THROW';
        }} catch (e) {{
          __r.missSlice = e.code + ':' + (e instanceof Error);
        }}
        Bun.file("{m}").exists().then(function(v) {{ __r.missExists = '' + v; }});
    "#
        ),
    );
    drive_event_loop(&mut ctx, 10);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.miss"),
        "ENOENT:true|ENOENT|ENOENT",
        "text()/json()/arrayBuffer() on a missing file must reject with coded ENOENT errors"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.missSlice"),
        "ENOENT:true",
        "slice() on a missing file must throw a coded ENOENT (no fake empty Blob)"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.missExists"),
        "false",
        "exists() face stays consistent (false, never a throw)"
    );

    // ── bad JSON: json() rejects with the SyntaxError ──
    eval_string(
        &mut ctx,
        &format!(
            r#"
        Bun.file("{bj}").json().then(
          function() {{ __r.badjson = 'RESOLVED'; }},
          function(e) {{ __r.badjson = e.name; }}
        );
    "#
        ),
    );
    drive_event_loop(&mut ctx, 10);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.badjson"),
        "SyntaxError",
        "json() on invalid JSON must reject with a SyntaxError"
    );

    // ── fd form: reads the descriptor itself, cursor-preserving ──
    eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('node:fs');
        var fd = fs.openSync("{t}", 'r');
        var f = Bun.file(fd);
        var shape = (f.fd === fd) + '';
        f.text().then(function(s) {{ __r.fdText1 = s; }});
        f.text().then(function(s) {{ __r.fdText2 = s; }});
        f.arrayBuffer().then(function(u) {{ __r.fdAb = '' + u.length; }});
        f.json().then(
          function() {{ __r.fdJson = 'RESOLVED'; }},
          function(e) {{ __r.fdJson = 'REJ:' + e.name; }}
        );
        fs.closeSync(fd);
        __r.fdShape = shape;
    "#
        ),
    );
    drive_event_loop(&mut ctx, 10);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.fdShape"),
        "true",
        "Bun.file(fd) object must carry the fd"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.fdText1"),
        text_content,
        "fd-form text() must read the descriptor's whole file"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.fdText2"),
        text_content,
        "fd-form reads are idempotent (pread from 0 — the fd cursor is never consumed)"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.fdAb"),
        "16",
        "fd-form arrayBuffer() must see all 16 bytes"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.fdJson"),
        "REJ:SyntaxError",
        "fd-form json() on non-JSON content rejects with SyntaxError"
    );

    // fd-form slice: same Blob face off the descriptor.
    let fd_slice = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs2 = require('node:fs');
        var fd2 = fs2.openSync("{t}", 'r');
        var bl = Bun.file(fd2).slice(0, 5);
        fs2.closeSync(fd2);
        (bl instanceof Blob) + ':' + bl.size;
    "#
        ),
    );
    assert_eq!(
        fd_slice, "true:5",
        "fd-form slice() must return a real Blob of the requested range"
    );

    // ── receiver without path/fd: hard error, never a dead promise ──
    let bad_recv = eval_string(
        &mut ctx,
        r#"
        try {
          var real = Bun.file("/etc/hostname");
          var t = real.text;
          t.call({}); // a `this` with neither fd nor path
          'NO-THROW';
        } catch (e) { 'THREW:' + (e instanceof Error); }
    "#,
    );
    assert_eq!(
        bad_recv, "THREW:true",
        "text() on an object without path/fd must throw a real error"
    );

    // Cleanup.
    for p in [&text_path, &json_path, &bin_path, &badjson_path] {
        let _ = std::fs::remove_file(p);
    }
}
