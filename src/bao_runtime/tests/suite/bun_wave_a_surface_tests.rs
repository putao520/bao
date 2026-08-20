// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// Final-wave surface tests: Bun.$ Promise face, Bun.file URL/fd forms,
// Bun.Mime, CryptoHasher/SHA constructors, Bun.serve custom response
// headers, Bun.serve WebSocket handshake, zlib sync aliases, semver.order.
//
// All checks run in ONE #[test] fn (JSContext per-thread singleton) with a
// bounded drain hook that ticks the event loop (uWS + microtasks) — the
// same pattern as child_process_spawn_events_tests.rs.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::TcpStream;

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

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

fn eval_str(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<wave-a>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => {
            // integral numbers must not print as "1.0"
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", n as i64)
            } else {
                format!("{}", n)
            }
        }
        Ok(JsValue::Bool(b)) => {
            if b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Ok(_) => String::new(),
        Err(_) => "<eval-error>".to_string(),
    }
}

fn eval_ok(ctx: &mut JsContext, source: &str) -> bool {
    ctx.eval(source, "<wave-a>").is_ok()
}

fn wait_until(ctx: &mut JsContext, js_condition: &str, budget: usize) -> bool {
    for _ in 0..120 {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return true;
        }
    }
    false
}

/// One event-loop tick: each ctx.eval triggers the post-eval drain hook,
/// which ticks the uWS loop (accepts + processes inbound sockets).
fn tick(ctx: &mut JsContext) {
    HOOK_BUDGET.with(|b| b.set(50));
    let _ = eval_str(ctx, "'t'");
}

/// Pump the loop while polling the socket until `done(buf)` holds or the
/// iteration budget runs out. Returns everything read.
fn tick_read_until<F: Fn(&[u8]) -> bool>(
    ctx: &mut JsContext,
    s: &mut TcpStream,
    done: F,
    max_ticks: usize,
) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    for _ in 0..max_ticks {
        tick(ctx);
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if done(&buf) {
                    break;
                }
            }
            Err(_) => {
                // WouldBlock/TimedOut — keep pumping the loop.
            }
        }
    }
    buf
}

fn http_get(ctx: &mut JsContext, port: u16, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect serve");
    s.set_read_timeout(Some(std::time::Duration::from_millis(50))).ok();
    s.set_nonblocking(false).ok();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    s.write_all(req.as_bytes()).unwrap();
    let buf = tick_read_until(ctx, &mut s, |_| false, 200);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Raw RFC 6455 opening handshake + one masked text frame; returns
/// (handshake_response, post-upgrade bytes read after sending the frame).
fn ws_upgrade_and_frame(
    ctx: &mut JsContext,
    port: u16,
    payload: &str,
) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect ws serve");
    s.set_read_timeout(Some(std::time::Duration::from_millis(50))).ok();
    let key = "dGhlIHNhbXBsZSBub25jZQ=="; // 24 chars, RFC 6455 sample key
    let req = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
        port, key
    );
    s.write_all(req.as_bytes()).unwrap();

    let handshake_raw = tick_read_until(
        ctx,
        &mut s,
        |b| b.windows(4).any(|w| w == b"\r\n\r\n"),
        200,
    );
    // Split at the handshake terminator: early frames (the open-handler
    // greeting) often coalesce into the same TCP segment — keep them for
    // the post-upgrade buffer.
    let split_at = handshake_raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n".as_slice())
        .map(|i| i + 4)
        .unwrap_or(handshake_raw.len());
    let handshake = handshake_raw[..split_at].to_vec();
    let mut early_frames = handshake_raw[split_at..].to_vec();

    // Send one masked text frame.
    let payload_bytes = payload.as_bytes();
    let mask = [0x11u8, 0x22, 0x33, 0x44];
    let mut masked = Vec::with_capacity(payload_bytes.len());
    for (i, b) in payload_bytes.iter().enumerate() {
        masked.push(b ^ mask[i % 4]);
    }
    let mut frame = vec![0x81u8, 0x80 | payload_bytes.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend_from_slice(&masked);
    s.write_all(&frame).unwrap();

    let mut rest = tick_read_until(ctx, &mut s, |_| false, 200);
    early_frames.extend_from_slice(&rest);
    (String::from_utf8_lossy(&handshake).into_owned(), early_frames)
}

#[test]
fn test_bun_wave_a_surface_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    // ═══ 1. Bun.$ Promise face (then/await resolve, catch reject) ═══
    assert!(
        eval_str(
            &mut ctx,
            "typeof Bun.$`echo hi`.then === 'function' && typeof Bun.$`echo hi`.catch === 'function' && typeof Bun.$`echo hi`.finally === 'function' ? 'y' : 'n'"
        ) == "y",
        "Bun.$ result must carry then/catch/finally"
    );
    eval_ok(
        &mut ctx,
        r#"var t1 = ''; Bun.$`echo wave-a-then`.then(function(r){ t1 = r.stdout.trim(); }); 'k'"#,
    );
    assert!(
        wait_until(&mut ctx, "t1 === 'wave-a-then' ? 'y' : 'n'", 50),
        "then(onFulfilled) must receive ShellOutput with stdout, got: {}",
        eval_str(&mut ctx, "t1")
    );
    // lines() reachable through the then-resolution
    eval_ok(
        &mut ctx,
        r#"var t1b = 0; Bun.$`printf 'a\nb\n'`.then(function(r){ t1b = r.lines().filter(function(x){return x;}).length; }); 'k'"#,
    );
    assert!(
        wait_until(&mut ctx, "t1b === 2 ? 'y' : 'n'", 50),
        "resolved ShellOutput.lines() must split stdout, got: {}",
        eval_str(&mut ctx, "String(t1b)")
    );
    // await assimilation (thenable → resolved value)
    eval_ok(
        &mut ctx,
        r#"var t2 = ''; (async function(){ t2 = (await Bun.$`echo await-ok`).stdout.trim(); })(); 'k'"#,
    );
    assert!(
        wait_until(&mut ctx, "t2 === 'await-ok' ? 'y' : 'n'", 50),
        "await Bun.$ must resolve the ShellOutput, got: {}",
        eval_str(&mut ctx, "t2")
    );
    // failure → reject with ShellError carrying exitCode/stdout/stderr
    eval_ok(
        &mut ctx,
        r#"var t3 = ''; Bun.$`sh -c 'echo boom >&2; exit 42'`.then(function(r){ t3 = 'unexpected-resolve:' + r.exitCode; }, function(e){ t3 = [e.name, e.exitCode, e.stderr.trim(), String(e.success)].join('|'); }); 'k'"#,
    );
    assert!(
        wait_until(&mut ctx, "t3 === 'ShellError|42|boom|false' ? 'y' : 'n'", 50),
        "non-zero exit must reject with ShellError, got: {}",
        eval_str(&mut ctx, "t3")
    );
    // catch() recovers; finally() runs on both paths (separate flags —
    // microtask interleaving makes a shared counter order-dependent)
    eval_ok(
        &mut ctx,
        r#"var t4 = ''; var t5a = 0; var t5b = 0; Bun.$`exit 7`.catch(function(e){ t4 = e.exitCode; }).finally(function(){ t5a = 99; }); Bun.$`echo ok2`.finally(function(){ t5b = 1; }); 'k'"#,
    );
    assert!(
        wait_until(&mut ctx, "t4 === 7 && t5a === 99 && t5b === 1 ? 'y' : 'n'", 50),
        "catch/finally faces must behave (t4={}, t5a={}, t5b={})",
        eval_str(&mut ctx, "String(t4)"),
        eval_str(&mut ctx, "String(t5a)"),
        eval_str(&mut ctx, "String(t5b)")
    );

    // ═══ 2. Bun.file object forms (URL / fd) ═══
    assert!(
        eval_str(
            &mut ctx,
            "typeof Bun.file(new URL('file:///nonexistent-wave-a/x')).exists === 'function' ? 'y' : 'n'"
        ) == "y",
        "Bun.file(URL) must return a BunFile with exists()"
    );
    eval_ok(
        &mut ctx,
        r#"var f1 = ''; Bun.file(new URL('file:///nonexistent-wave-a/x')).exists().then(function(v){ f1 = String(v); }); 'k'"#,
    );
    assert!(
        wait_until(&mut ctx, "f1 === 'false' ? 'y' : 'n'", 50),
        "Bun.file(file:// URL of missing file).exists() must resolve false, got: {}",
        eval_str(&mut ctx, "f1")
    );
    // URL of an existing file → true, and .path is the decoded pathname
    let tmp = std::env::temp_dir().join("bao_wave_a_exists.txt");
    std::fs::write(&tmp, b"x").unwrap();
    let tmp_js = tmp.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    eval_ok(
        &mut ctx,
        &format!(
            r#"var f2 = ''; var fu = new URL('file://{}'); Bun.file(fu).exists().then(function(v){{ f2 = String(v) + ':' + Bun.file(fu).path; }}); 'k'"#,
            tmp_js
        ),
    );
    let f2 = eval_str(&mut ctx, "f2");
    assert!(
        wait_until(&mut ctx, &format!("f2 === 'true:{}' ? 'y' : 'n'", tmp_js), 50),
        "Bun.file(file:// URL of existing file).exists() must resolve true with decoded path, got: {}",
        f2
    );
    // fd form
    assert!(
        eval_str(
            &mut ctx,
            "typeof Bun.file(1).exists === 'function' && typeof Bun.stdin.exists === 'function' && Bun.file(1).fd === 1 && Bun.stdout.writable === true ? 'y' : 'n'"
        ) == "y",
        "Bun.file(fd) and Bun.stdin/stdout must expose the fd + exists faces"
    );
    eval_ok(
        &mut ctx,
        r#"var f3 = ''; Bun.file(2).exists().then(function(v){ f3 = String(v); }); 'k'"#,
    );
    assert!(
        wait_until(&mut ctx, "f3 === 'true' ? 'y' : 'n'", 50),
        "Bun.file(2).exists() must resolve true (std stream), got: {}",
        eval_str(&mut ctx, "f3")
    );
    // non-file URL must be an explicit error, not a silent fake path
    assert!(
        !eval_ok(&mut ctx, "Bun.file(new URL('https://example.com/x'))"),
        "Bun.file(https URL) must throw (unsupported), not fake a path"
    );
    // string form unchanged: exists() false for missing, true for existing
    eval_ok(
        &mut ctx,
        r#"var f4 = ''; Bun.file('/nonexistent-wave-a/plain').exists().then(function(v){ f4 = String(v); }); 'k'"#,
    );
    assert!(
        wait_until(&mut ctx, "f4 === 'false' ? 'y' : 'n'", 50),
        "Bun.file(string missing).exists() must stay false"
    );

    // ═══ 3. Bun.Mime.getType coverage (server table parity) ═══
    assert!(
        eval_str(
            &mut ctx,
            r#"
            ['html','htm','js','mjs','cjs','json','css','png','jpg','jpeg','gif','svg','webp','ico','txt','xml','pdf','zip','gz','tar','mp4','webm','mp3','wav','wasm','woff','woff2','ttf','otf','csv','ts','tsx','jsx','yaml','yml','md','webmanifest','xhtml','avif','heic','flac','m4a','aac']
              .every(function(e){ return typeof Bun.Mime.getType(e) === 'string' && Bun.Mime.getType(e).indexOf('/') > 0; }) ? 'y' : 'n'
        "#
        ) == "y",
        "common MIME extensions must all resolve via Bun.Mime.getType"
    );
    assert_eq!(eval_str(&mut ctx, "Bun.Mime.getType('x.html')"), "text/html");
    assert_eq!(
        eval_str(&mut ctx, "String(Bun.Mime.getType('does-not-exist-xyz'))"),
        "null",
        "unknown extensions must stay null (npm-mime semantics)"
    );
    assert_eq!(eval_str(&mut ctx, "Bun.Mime.getExtension('text/html')"), "html");
    assert_eq!(
        eval_str(&mut ctx, "new Bun.Mime().getType('html')"),
        "text/html",
        "Mime instances must expose getType (npm-mime instance face)"
    );
    assert_eq!(
        eval_str(&mut ctx, "new Bun.Mime().getExtension('text/html')"),
        "html"
    );

    // ═══ 4. CryptoHasher / SHA constructors ═══
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"(function(){ var h = new Bun.CryptoHasher('sha256'); h.update('abc'); return h.digest('hex'); })()"#
        ),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "new Bun.CryptoHasher('sha256') digest must match SHA-256('abc')"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"(function(){ var h = new Bun.CryptoHasher('sha1'); h.update('abc'); return h.digest(); })()"#
        ),
        "a9993e364706816aba3e25717850c26c9cd0d89d",
        "new CryptoHasher('sha1') default hex digest must match SHA-1('abc')"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"(function(){ var h = new Bun.SHA('sha256'); h.update('abc'); return h.digest('hex').length; })()"#
        ),
        "64",
        "new Bun.SHA(...) must construct and digest (alias of CryptoHasher)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(new Bun.CryptoHasher('md5').update('abc').digest('hex'))"),
        "900150983cd24fb0d6963f7d28e17f72",
        "chained update() must return this and MD5('abc') must match"
    );
    // non-new call form still works
    assert_eq!(
        eval_str(&mut ctx, "typeof Bun.CryptoHasher('sha256').digest === 'function' ? 'y' : 'n'"),
        "y"
    );

    // ═══ 5. Bun.serve custom response headers ═══
    let setup5 = eval_str(
        &mut ctx,
        r#"
        globalThis.__srv5 = Bun.serve({
            port: 19375,
            fetch: function(req) {
                return new Response('hdr-body', { headers: { 'X-Custom-Bao': 'wave-a', 'Content-Type': 'text/x-bao' } });
            },
        });
        'up'
    "#,
    );
    assert_eq!(setup5, "up", "serve 5 must start");
    let resp5 = http_get(&mut ctx, 19375, "/");
    // Header names arrive lowercased (the web Headers map lowercases keys;
    // HTTP names are case-insensitive) — compare case-insensitively.
    let resp5lc = resp5.to_ascii_lowercase();
    assert!(
        resp5lc.contains("x-custom-bao: wave-a"),
        "custom response header must survive serialization:\n{}",
        resp5
    );
    assert!(
        resp5lc.contains("content-type: text/x-bao"),
        "custom Content-Type must survive serialization:\n{}",
        resp5
    );
    assert!(
        resp5.contains("hdr-body"),
        "body must round-trip:\n{}",
        resp5
    );
    assert_eq!(
        resp5lc.matches("content-length:").count(),
        1,
        "exactly one Content-Length (uWS end() owns it):\n{}",
        resp5
    );
    eval_ok(&mut ctx, "globalThis.__srv5.stop(); 'stopped'");

    // ═══ 6. Bun.serve WebSocket handshake (handler present → 101 + echo) ═══
    let setup6 = eval_str(
        &mut ctx,
        r#"
        globalThis.__echo6 = [];
        globalThis.__srv6 = Bun.serve({
            port: 19376,
            fetch: function(req) { return new Response('plain'); },
            websocket: {
                open: function(ws) { ws.send('opened'); },
                message: function(ws, msg) { globalThis.__echo6.push(String(msg)); ws.send('echo:' + msg); },
                close: function(ws) {},
            },
        });
        'up'
    "#,
    );
    assert_eq!(setup6, "up", "serve 6 must start");
    // Prime the loop so the listen socket is accepting before the raw client hits it.
    assert!(
        wait_until(&mut ctx, "globalThis.__srv6.port === 19376 ? 'y' : 'n'", 30),
        "serve 6 must report its port"
    );

    let (handshake, post) = ws_upgrade_and_frame(&mut ctx, 19376, "ping-x");
    assert!(
        handshake.starts_with("HTTP/1.1 101"),
        "WS upgrade must complete the 101 handshake, got:\n{}",
        handshake
    );
    assert!(
        handshake
            .to_ascii_lowercase()
            .contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "Sec-WebSocket-Accept must be the RFC 6455 SHA1 of the key, got:\n{}",
        handshake
    );
    let post_text = String::from_utf8_lossy(&post);
    assert!(
        post_text.contains("opened"),
        "open handler greeting frame must arrive after upgrade, got: {:?}",
        post_text
    );
    assert!(
        post_text.contains("echo:ping-x"),
        "message handler must echo the sent frame (real WS data round-trip), got: {:?}",
        post_text
    );
    // The fetch handler keeps serving plain HTTP alongside the WS route.
    let resp6 = http_get(&mut ctx, 19376, "/");
    assert!(
        resp6.contains("HTTP/1.1 200") && resp6.contains("plain"),
        "plain fetch path must still work with a websocket handler registered:\n{}",
        resp6
    );
    eval_ok(&mut ctx, "globalThis.__srv6.stop(); 'stopped'");

    // ═══ 7. Bun.gzipSync/gunzipSync/deflateSync/inflateSync aliases ═══
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            (function() {
                var enc = new TextEncoder(); var dec = new TextDecoder();
                var gz = Bun.gzipSync(enc.encode('wave-a-gzip'));
                var back = new TextDecoder().decode(Bun.gunzipSync(gz));
                var df = Bun.deflateSync(enc.encode('wave-a-deflate'));
                var back2 = new TextDecoder().decode(Bun.inflateSync(df));
                return back + '|' + back2;
            })()
        "#
        ),
        "wave-a-gzip|wave-a-deflate",
        "zlib sync aliases must round-trip real gzip/deflate streams"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(Bun.gzipSync(new TextEncoder().encode('x')).length > 0)"),
        "true",
        "gzipSync must produce real compressed bytes"
    );

    // ═══ 8. Bun.semver.order ═══
    assert_eq!(eval_str(&mut ctx, "Bun.semver.order('1.2.3', '1.2.4')"), "-1");
    assert_eq!(eval_str(&mut ctx, "Bun.semver.order('1.2.4', '1.2.3')"), "1");
    assert_eq!(eval_str(&mut ctx, "Bun.semver.order('1.2.3', '1.2.3')"), "0");
    assert_eq!(
        eval_str(&mut ctx, "Bun.semver.order('1.0.0', '1.0.0-alpha')"),
        "1",
        "release outranks prerelease (SemVer 2.0)"
    );
    assert_eq!(eval_str(&mut ctx, "Bun.semver.order('1.0.0-alpha', '1.0.0-beta')"), "-1");
    assert_eq!(
        eval_str(&mut ctx, "Bun.semver.order('1.0.0-alpha.1', '1.0.0-alpha.beta')"),
        "-1",
        "numeric identifier ranks below alphanumeric at same position"
    );
    assert_eq!(
        eval_str(&mut ctx, "Bun.semver.order('1.0.0+build.1', '1.0.0+build.2')"),
        "0",
        "build metadata is ignored for precedence"
    );
    assert_eq!(
        eval_str(&mut ctx, "Bun.semver.order('2.0.0', '10.0.0')"),
        "-1",
        "numeric (not lexical) comparison of major"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.semver.order('not-a-version', '1.0.0')"),
        "order() must throw on invalid versions"
    );
    // bun_semver-crate semantics mirrored from upstream SemverObject.order
    assert_eq!(
        eval_str(&mut ctx, "Bun.semver.order('1.\u{e9}.0', '1.0.0')"),
        "0",
        "non-ASCII input short-circuits to 0 (upstream behavior)"
    );
    assert_eq!(
        eval_str(&mut ctx, "Bun.semver.order('v1.2.3', '1.2.4')"),
        "-1",
        "v-prefix accepted (crate parser semantics)"
    );
    assert_eq!(
        eval_str(&mut ctx, "Bun.semver.order('1.2', '1.2.3')"),
        "1",
        "partial version canonicalizes via max(): 1.2 → 1.2.MAX outranks 1.2.3"
    );
}
