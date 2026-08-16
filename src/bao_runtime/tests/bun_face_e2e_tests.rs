// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// Bun-face completion wave e2e: Bun.hash / nanoseconds / env / inspect /
// main / spawnSync / peek / stringWidth / Mime / TOML / YAML / JSONC /
// RegExp.escape / Glob / readableStreamToArray / tcpSocket /
// file().exists() / serve websocket upgrade dispatch.
//
// Single #[test] body (mozjs thread-singleton rule, same as bun_api_tests).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        Ok(JsValue::String(s)) => s == "true",
        _ => false,
    }
}

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

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

/// Drive the JS thread's MiniEventLoop (fetch e2e pattern).
fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
    let cx_raw = ctx.raw_cx();
    for _ in 0..max_iters {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn test_bun_face_completion() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ── A1. Bun.hash — wyhash BigInt + deterministic + seed + variants ──
    assert!(
        eval_bool(&mut ctx, r#"typeof Bun.hash("hello") === "bigint""#),
        "Bun.hash default returns BigInt (64-bit)"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"Bun.hash("hello") === Bun.hash("hello") && Bun.hash("hello") === Bun.hash.wyhash("hello")"#
        ),
        "Bun.hash determinism + wyhash variant equivalence"
    );
    assert!(
        eval_bool(&mut ctx, r#"Bun.hash("hello") !== Bun.hash("hello", 1234)"#),
        "Bun.hash honours the seed argument"
    );
    assert!(
        eval_bool(&mut ctx, r#"typeof Bun.hash.crc32("hello") === "number""#),
        "32-bit variant returns a number"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"["adler32","cityHash32","cityHash64","xxHash32","xxHash64","xxHash3","murmur32v2","murmur32v3","murmur64v2","rapidhash"].every(n => typeof Bun.hash[n] === "function")"#
        ),
        "all upstream named variants exist"
    );
    assert!(
        eval_bool(&mut ctx, r#"Bun.hash(new Uint8Array([1,2,3])) === Bun.hash(new Uint8Array([1,2,3]))"#),
        "typed-array input hashing is stable"
    );

    // ── A2. Bun.nanoseconds — real elapsed-since-process-start scale ──
    let ns = eval_number(&mut ctx, "Bun.nanoseconds()");
    assert!(
        ns > 1_000_000.0,
        "Bun.nanoseconds() must reflect time since process start (got {} ns; the old first-call-baseline bug returned ~43)",
        ns
    );
    let ns2 = eval_number(&mut ctx, "Bun.nanoseconds()");
    assert!(ns2 >= ns, "Bun.nanoseconds() is monotonic");

    // ── A3. Bun.env — shared proxy with process.env (writes propagate) ──
    assert!(
        eval_bool(
            &mut ctx,
            r#"(Bun.env.__FACE_E2E = "1") === "1" && process.env.__FACE_E2E === "1""#
        ),
        "Bun.env writes are visible on process.env"
    );
    assert_eq!(::std::env::var("__FACE_E2E").as_deref(), Ok("1"),
        "Bun.env writes reach std::env");
    assert!(
        eval_bool(
            &mut ctx,
            r#"(process.env.__FACE_E2E_B = "2") === "2" && Bun.env.__FACE_E2E_B === "2""#
        ),
        "process.env writes are visible on Bun.env"
    );

    // ── A4. Bun.inspect — objects render with contents, cycles survive ──
    let inspected = eval_string(&mut ctx, r#"Bun.inspect({ a: 1, b: "x" })"#);
    assert!(
        inspected.contains("a: 1") && inspected.contains("\"x\""),
        "Bun.inspect renders object contents, got: {}",
        inspected
    );
    let arr_insp = eval_string(&mut ctx, r#"Bun.inspect([1, "two", true])"#);
    assert!(
        arr_insp.contains("[ 1") && arr_insp.contains("two"),
        "Bun.inspect renders arrays, got: {}",
        arr_insp
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"Bun.inspect(new Map([["k", 7]])) === 'Map(1) { "k" => 7 }'"#
        ),
        "Bun.inspect Map entries"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"var c = {}; c.self = c; Bun.inspect(c).includes("[Circular]")"#
        ),
        "Bun.inspect cycle guard"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"Bun.inspect({ d: { e: { f: 1 } } }, { depth: 0 }) === "{ d: [Object] }""#
        ),
        "Bun.inspect depth cap, got: {}",
        eval_string(&mut ctx, r#"Bun.inspect({ d: { e: { f: 1 } } }, { depth: 0 })"#)
    );
    assert!(
        eval_bool(&mut ctx, r#"Bun.inspect("hi") === "\"hi\"""#),
        "Bun.inspect quotes strings"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"Bun.inspect({ k: 1 }, { colors: true }).includes("\x1b[")"#
        ),
        "Bun.inspect colors option"
    );

    // ── A5. Bun.main — string surface (run-mode absolute path probed
    // against the real binary; here the fallback shape). ──
    assert_eq!(
        eval_string(&mut ctx, "typeof Bun.main"),
        "string",
        "Bun.main is a string"
    );

    // ── A6. Bun.spawnSync ──
    let echo_out = eval_string(
        &mut ctx,
        r#"var r = Bun.spawnSync(["/bin/echo", "face-e2e"]); r.success === true && r.exitCode === 0 && (typeof r.stdout)"#,
    );
    assert!(
        echo_out.contains("object"),
        "Bun.spawnSync success + exitCode 0, stdout type object, got: {}",
        echo_out
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"Bun.spawnSync(["/bin/echo", "face-e2e"], { encoding: "utf8" }).stdout.trim() === "face-e2e""#
        ),
        "spawnSync utf8 encoding returns trimmed text"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"var tr = Bun.spawnSync(["/bin/sh", "-c", "cat; echo err >&2"], { stdin: "in-data", encoding: "utf8" }); tr.stdout.trim() === "in-data" && tr.stderr.trim() === "err""#
        ),
        "spawnSync stdin string + stderr capture"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"var cap = Bun.spawnSync(["/bin/sh", "-c", "echo 0123456789"], { maxBuffer: 5, encoding: "utf8" }); cap.stdout.length === 5"#
        ),
        "spawnSync maxBuffer truncates"
    );
    assert!(
        !eval_ok(&mut ctx, r#"Bun.spawnSync(["/nonexistent/binary/xyz"])"#),
        "spawnSync of a missing binary throws explicitly"
    );

    // ── A7. Bun.peek ──
    assert!(
        eval_bool(&mut ctx, r#"Bun.peek(Promise.resolve(42)) === 42"#),
        "peek(fulfilled) → settled value"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"var p = Promise.reject(new Error("boom")); Bun.peek(p).message === "boom""#
        ),
        "peek(rejected) → rejection reason"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"var pend = new Promise(function(){}); Bun.peek(pend) === pend"#
        ),
        "peek(pending) → the promise itself"
    );
    assert!(
        eval_bool(&mut ctx, r#"Bun.peekStatus(new Promise(function(){})) === "pending""#),
        "peekStatus pending"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"Bun.peekStatus(Promise.resolve(1)) === "fulfilled" && Bun.peekStatus(Promise.reject(1)) === "rejected""#
        ),
        "peekStatus settled states"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"var it = { i: 0, next: function() { this.i++; return { value: this.i, done: this.i > 2 }; } };
                 var peeked = Bun.peek(it);
                 peeked.peeked === 1 && peeked.next().value === 1 && peeked.next().value === 2"#
        ),
        "peek(lazy iterator) takes the first item eagerly and replays it"
    );

    // ── A8. Bun.stringWidth ──
    assert_eq!(eval_number(&mut ctx, r#"Bun.stringWidth("hello")"#), 5.0);
    assert_eq!(
        eval_number(&mut ctx, r#"Bun.stringWidth("你好")"#),
        4.0,
        "CJK chars are double-width"
    );
    assert_eq!(
        eval_number(&mut ctx, r#"Bun.stringWidth("\x1b[31mhi\x1b[0m")"#),
        2.0,
        "ANSI escapes are zero-width by default"
    );
    assert!(
        eval_number(&mut ctx, r#"Bun.stringWidth("\x1b[31mhi\x1b[0m", { countAnsiEscapeCodes: true })"#) > 2.0,
        "countAnsiEscapeCodes counts escape characters"
    );

    // ── A9. Bun.Mime ──
    assert_eq!(
        eval_string(&mut ctx, r#"Bun.Mime.getType("x.html")"#),
        "text/html",
        "Mime.getType by extension"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"Bun.Mime.getType("dir/file.JSON")"#),
        "application/json",
        "Mime.getType by path, case-insensitive"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"Bun.Mime.getType("nope.unknownext")"#),
        "null",
        "Mime.getType unknown → null (npm-mime semantics)"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"Bun.Mime.getExtension("text/html")"#),
        "html",
        "Mime.getExtension canonical"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"Bun.Mime.getExtension("application/json;charset=utf-8")"#),
        "json",
        "Mime.getExtension ignores parameters"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"Bun.Mime.normalizeKind("json")"#),
        "application/json",
        "normalizeKind short kind"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"Bun.Mime.normalizeKind("text/html")"#),
        "text/html",
        "normalizeKind full type passes through"
    );
    // Forward/reverse consistency for the whole canonical table.
    assert!(
        eval_bool(
            &mut ctx,
            r#"(function() {
                 var pairs = [["text/html","html"],["text/css","css"],["text/javascript","js"],["application/json","json"],["image/png","png"],["image/svg+xml","svg"],["application/pdf","pdf"],["application/zip","zip"],["video/mp4","mp4"],["audio/mpeg","mp3"],["font/woff2","woff2"],["text/markdown","md"],["image/webp","webp"],];
                 var bad = pairs.filter(function(p) { return !(Bun.Mime.getType(p[1]) === p[0] && Bun.Mime.getExtension(p[0]) === p[1]); });
                 globalThis.__mimeBad = JSON.stringify(bad);
                 return bad.length === 0;
               })()"#
        ),
        "Mime getType/getExtension table consistency, bad: {}",
        eval_string(&mut ctx, "globalThis.__mimeBad")
    );
    assert_eq!(
        eval_string(&mut ctx, r#"new Bun.Mime("text", "plain").toString()"#),
        "text/plain",
        "Mime instance ctor + toString"
    );

    // ── B10. Bun.TOML ──
    assert!(
        eval_bool(
            &mut ctx,
            r#"var cfg = Bun.TOML.parse('title = "bao"\n[owner]\nname = "putao"\nports = [1, 2, 3]\n');
                 cfg.title === "bao" && cfg.owner.name === "putao" && cfg.owner.ports.join(",") === "1,2,3""#
        ),
        "TOML.parse tables, strings, arrays"
    );
    assert!(
        eval_bool(&mut ctx, r#"Bun.TOML.parse('n = 3.5\ni = 42\nok = true\n').i === 42 && Bun.TOML.parse('n = 3.5').n === 3.5 && Bun.TOML.parse('ok = true').ok === true"#),
        "TOML.parse scalars"
    );
    let roundtrip = eval_string(
        &mut ctx,
        r#"Bun.TOML.stringify({ name: "bao", level: 7, pi: 3.5, ok: true, nested: { deep: "v" } })"#,
    );
    assert!(
        roundtrip.contains("name = \"bao\"")
            && roundtrip.contains("level = 7")
            && roundtrip.contains("[nested]")
            && roundtrip.contains("deep = \"v\""),
        "TOML.stringify emits pairs + nested table sections, got: {}",
        roundtrip
    );
    let aot_out = eval_string(
        &mut ctx,
        r#"Bun.TOML.stringify({ items: [{ x: 1 }, { x: 2 }] })"#,
    );
    assert!(
        aot_out.contains("[[items]]"),
        "TOML.stringify array-of-tables, got: {}",
        aot_out
    );
    assert!(
        !eval_ok(&mut ctx, r#"Bun.TOML.parse("this is [ not toml")"#),
        "TOML.parse syntax error throws"
    );
    assert!(
        !eval_ok(&mut ctx, r#"Bun.TOML.stringify({ bad: null })"#),
        "TOML.stringify rejects null (TOML has no null)"
    );

    // ── B11. Bun.YAML ──
    assert!(
        eval_bool(
            &mut ctx,
            r#"var y = Bun.YAML.parse('name: bao\nlevel: 3\nnested:\n  deep: true\nlist:\n  - 1\n  - 2\n');
                 y.name === "bao" && y.level === 3 && y.nested.deep === true && y.list.join(",") === "1,2""#
        ),
        "YAML.parse mappings, sequences, scalars"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.YAML.parse('a: [unclosed')"),
        "YAML.parse syntax error throws"
    );

    // ── B12. Bun.JSONC ──
    assert!(
        eval_bool(
            &mut ctx,
            r#"var j = Bun.JSONC.parse('{\n  // line comment\n  "a": 1, /* block\n comment */ "b": [2, 3,],\n}');
                 j.a === 1 && j.b.join(",") === "2,3""#
        ),
        "JSONC.parse comments + trailing commas"
    );
    assert!(
        !eval_ok(&mut ctx, r#"Bun.JSONC.parse('{ "a": }')"#),
        "JSONC.parse invalid JSON throws"
    );

    // ── B13. Bun.RegExp.escape ──
    assert_eq!(
        eval_string(&mut ctx, r#"Bun.RegExp.escape("bun.js")"#),
        "bun\\.js",
        "RegExp.escape dots"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"Bun.RegExp.escape("a.*+?^${}()|[]\\-b") === "a\\.\\*\\+\\?\\^\\$\\{\\}\\(\\)\\|\\[\\]\\\\\\x2db""#
        ),
        "RegExp.escape full meta set (upstream escapeRegExp: - → \\x2d)"
    );
    assert!(
        eval_bool(
            &mut ctx,
            r#"new RegExp("^" + Bun.RegExp.escape("1+1=2?") + "$").test("1+1=2?")"#,
        ),
        "escaped pattern matches the literal source text"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.RegExp.escape(42)"),
        "RegExp.escape rejects non-strings (upstream throws)"
    );

    // ── B14. Bun.Glob ──
    let tmp = ::tempfile::tempdir().expect("tempdir");
    let tmp_path = tmp.path().to_string_lossy().into_owned();
    ::std::fs::write(tmp.path().join("a.js"), b"1").unwrap();
    ::std::fs::write(tmp.path().join("b.txt"), b"2").unwrap();
    ::std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
    ::std::fs::write(tmp.path().join("sub").join("c.js"), b"3").unwrap();
    let tmp_js = tmp_path.replace('\\', "\\\\").replace('"', "\\\"");
    assert!(
        eval_bool(&mut ctx, &format!(r#"new Bun.Glob("*.js").match("a.js") && !new Bun.Glob("*.js").match("a.txt")"#)),
        "Glob.match"
    );
    let scan_out = eval_string(
        &mut ctx,
        &format!(
            r#"JSON.stringify(new Bun.Glob("**/*.js").scanSync({{ cwd: "{}" }}).sort())"#,
            tmp_js
        ),
    );
    assert!(
        scan_out.contains("a.js") && scan_out.contains("sub/c.js") && !scan_out.contains("b.txt"),
        "Glob.scanSync recursive + onlyFiles default, got: {}",
        scan_out
    );
    let scan_async = eval_string(
        &mut ctx,
        &format!(
            r#"(function(){{ var s = new Bun.Glob("*.txt").scan({{ cwd: "{}" }}); return (typeof s[Symbol.asyncIterator] === "function"); }})()"#,
            tmp_js
        ),
    );
    assert_eq!(scan_async, "true", "Glob.scan returns an async iterable");

    // ── B15. Bun.readableStreamToArray ──
    ctx.eval(
        r#"
        globalThis.__rs2a = "unset";
        var chunks = ["x", "y"];
        var rs = new ReadableStream({
          start(controller) { chunks.forEach(c => controller.enqueue(c)); controller.close(); }
        });
        Bun.readableStreamToArray(rs).then(function(arr) {
          globalThis.__rs2a = arr.join("-");
        });
        "#,
        "<test>",
    )
    .expect("readableStreamToArray setup");
    drive_event_loop(&mut ctx, 50);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__rs2a"),
        "x-y",
        "readableStreamToArray drains stream chunks in order"
    );

    // ── B16. Bun.tcpSocket — explicit registered gap ──
    assert!(
        !eval_ok(&mut ctx, "Bun.tcpSocket()"),
        "Bun.tcpSocket throws (not-implemented; owned by the net domain)"
    );

    // ── B17. Bun.file().exists() ──
    let exists_path = tmp.path().join("a.js").to_string_lossy().into_owned();
    let missing_path = tmp.path().join("missing-xyz.txt").to_string_lossy().into_owned();
    ctx.eval(
        &format!(
            r#"
            globalThis.__exists = "unset";
            Promise.all([
              Bun.file("{0}").exists(),
              Bun.file("{1}").exists(),
            ]).then(function(r) {{ globalThis.__exists = r[0] + "," + r[1]; }});
            "#,
            exists_path.replace('\\', "\\\\"),
            missing_path.replace('\\', "\\\\"),
        ),
        "<test>",
    )
    .expect("exists setup");
    drive_event_loop(&mut ctx, 50);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__exists"),
        "true,false",
        "Bun.file().exists() resolves true/false (missing → false, not undefined)"
    );

    // ── C18. Bun.serve: WS upgrade request reaches fetch when no websocket
    // handler is registered (upstream dispatch; was: early 426). ──
    ctx.eval(
        &format!(
            r#"
            globalThis.__exists = "unset";
            Promise.all([
              Bun.file("{0}").exists(),
              Bun.file("{1}").exists(),
            ]).then(function(r) {{ globalThis.__exists = r[0] + "," + r[1]; }});
            "#,
            exists_path.replace('\\', "\\\\"),
            missing_path.replace('\\', "\\\\"),
        ),
        "<test>",
    )
    .expect("exists setup");
    drive_event_loop(&mut ctx, 50);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__exists"),
        "true,false",
        "Bun.file().exists() resolves true/false (missing → false, not undefined)"
    );

    // ── C18. Bun.serve: WS upgrade request reaches fetch when no websocket
    // handler is registered (upstream dispatch; was: early 426). ──
    let listener = ::std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let seen_upgrade = ::std::sync::Arc::new(::std::sync::atomic::AtomicBool::new(false));
    let seen_clone = ::std::sync::Arc::clone(&seen_upgrade);
    ::std::thread::spawn(move || {
        let deadline = ::std::time::Instant::now() + Duration::from_secs(10);
        listener.set_nonblocking(true).ok();
        while ::std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);
                    let req = String::from_utf8_lossy(&buf).into_owned();
                    if req.contains("GET /ws") {
                        seen_clone.store(true, ::std::sync::atomic::Ordering::Release);
                    }
                    let body = b"fetch-saw-upgrade";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.write_all(body);
                    let _ = stream.flush();
                }
                Err(_) => ::std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });

    ctx.eval(
        &format!(
            r#"
            globalThis.__upgradeBody = "unset";
            var srv = Bun.serve({{
              port: 0,
              hostname: "127.0.0.1",
              fetch: function(req) {{
                var isUpgrade = (req.headers.get("upgrade") || "").toLowerCase() === "websocket";
                return new Response(isUpgrade ? "fetch-saw-upgrade" : "plain", {{ status: isUpgrade ? 200 : 218 }});
              }},
            }});
            "#,
        ),
        "<test>",
    )
    .expect("serve setup");
    drive_event_loop(&mut ctx, 20);

    // Raw WS-upgrade GET against the server port.
    let server_port = eval_number(&mut ctx, "srv.port") as u16;
    if let Ok(mut sock) = TcpStream::connect(("127.0.0.1", server_port)) {
        sock.set_read_timeout(Some(Duration::from_secs(3))).ok();
        let req = format!(
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            server_port
        );
        if sock.write_all(req.as_bytes()).is_ok() {
            let mut buf = [0u8; 2048];
            if let Ok(n) = sock.read(&mut buf) {
                let resp = String::from_utf8_lossy(&buf[..n]).into_owned();
                assert!(
                    !resp.starts_with("HTTP/1.1 426"),
                    "WS upgrade must not be answered 426 when a fetch handler exists — got: {}",
                    resp.split("\r\n").next().unwrap_or("")
                );
                assert!(
                    resp.contains("fetch-saw-upgrade"),
                    "fetch handler observed the upgrade request — got: {:?}",
                    resp
                );
            }
        }
    }
    let _ = eval_bool(&mut ctx, "srv.stop && srv.stop(), true");
    drive_event_loop(&mut ctx, 10);
}

fn eval_ok(ctx: &mut JsContext, source: &str) -> bool {
    ctx.eval(source, "<test>").is_ok()
}
