// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// Bun.serve response-body byte forms (SILENT: string-only body read):
// serve_write_response_object used to read ONLY `_bodyText`, but the
// Request/Response constructors store ArrayBuffer / typed-array / Buffer
// bodies in `_bodyBytes` — so `return new Response(uint8Array)` produced a
// 200 with a silently EMPTY body. The writer now reads `_bodyText` →
// `_bodyBytes` (collect_byte_view → wire verbatim), mirroring fetch_fn's
// Request-side slot precedence.
//
// Byte-exact roundtrips over raw TCP against a real Bun.serve listener:
// Uint8Array (with NUL + 0xFF markers), Buffer, plain ArrayBuffer, a
// byteOffset≠0 subarray view, and the legacy string body — each compared
// byte-for-byte, plus status/Content-Length framing.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

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

/// One raw HTTP/1.1 GET, pumping the JS-thread MiniEventLoop between read
/// attempts (the uWS dispatch only runs when the loop ticks — a blocking
/// read on this thread would starve it and time out).
fn http_get(ctx: &mut JsContext, port: u16, path: &str) -> Option<Vec<u8>> {
    let mut sock = TcpStream::connect(("127.0.0.1", port)).ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(25))).ok();
    sock.set_nonblocking(false).ok();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    sock.write_all(req.as_bytes()).ok()?;
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        drive_event_loop(ctx, 3);
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
        // Complete responses end with connection close (end(…, close=true))
        // — but stop early once headers say Content-Length is satisfied.
        if let Some(sep) = out.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&out[..sep]).to_ascii_lowercase();
            if let Some(cl) = head.lines().find_map(|l| {
                l.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            }) {
                if out.len() >= sep + 4 + cl {
                    break;
                }
            }
        }
    }
    Some(out)
}

/// Split a raw response into (status_line, headers_block, body_bytes) using
/// Content-Length framing when present (falling back to after-`\r\n\r\n`).
fn split_response(raw: &[u8]) -> (String, Vec<u8>) {
    let sep = b"\r\n\r\n";
    let pos = raw
        .windows(sep.len())
        .position(|w| w == sep)
        .expect("response must contain header/body separator");
    let head = String::from_utf8_lossy(&raw[..pos]).into_owned();
    let body = raw[pos + sep.len()..].to_vec();
    (head, body)
}

#[test]
fn test_serve_response_body_byte_forms_roundtrip() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let setup = eval_string(
        &mut ctx,
        r#"
        var u8 = new Uint8Array([104, 105, 0, 255, 33]);   // 'h','i',NUL,0xFF,'!'
        var buf = Buffer.from([98, 117, 102, 0, 102, 101, 114]); // 'buf' NUL 'fer'
        var ab = new Uint8Array([1, 2, 3, 4, 254]).buffer;
        var big = new Uint8Array([9, 9, 9, 65, 66, 67, 68, 205, 204]);
        var sub = big.subarray(3); // byteOffset=3 view: 'A','B','C','D',0xCD,0xCC
        var srv = Bun.serve({
          port: 0,
          hostname: "127.0.0.1",
          fetch: function(req) {
            // req.url is the relative form (path + query), per uWS.
            var p = req.url.split("?")[0];
            if (p === "/u8") return new Response(u8, { status: 201 });
            if (p === "/buf") return new Response(buf, { status: 202 });
            if (p === "/ab") return new Response(ab, { status: 203 });
            if (p === "/sub") return new Response(sub, { status: 204 });
            if (p === "/text") return new Response("plain-text-body", { status: 200 });
            return new Response("nope", { status: 404 });
          },
        });
        globalThis.__srv = srv;
        "ok"
    "#,
    );
    assert_eq!(setup, "ok", "serve setup must eval");
    drive_event_loop(&mut ctx, 30);

    let port = eval_number(&mut ctx, "globalThis.__srv.port") as u16;
    assert!(port > 0, "serve must bind an ephemeral port, got {port}");
    drive_event_loop(&mut ctx, 10);

    // (path, expected status, expected body bytes)
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        ("/u8", "201", vec![104, 105, 0, 255, 33]),
        ("/buf", "202", vec![98, 117, 102, 0, 102, 101, 114]),
        ("/ab", "203", vec![1, 2, 3, 4, 254]),
        (
            "/sub",
            "204",
            vec![65, 66, 67, 68, 205, 204],
        ),
        ("/text", "200", b"plain-text-body".to_vec()),
    ];

    for (path, want_status, want_body) in cases {
        let raw = http_get(&mut ctx, port, path)
            .unwrap_or_else(|| panic!("request {path} must get a response"));
        let (head, body) = split_response(&raw);
        let status_line = head.lines().next().unwrap_or("");
        assert!(
            status_line.starts_with(&format!("HTTP/1.1 {want_status}")),
            "{path}: status must be {want_status}, got: {status_line}"
        );
        let cl = head
            .lines()
            .find_map(|l| {
                let lower = l.to_ascii_lowercase();
                if lower.starts_with("content-length:") {
                    lower["content-length:".len()..].trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(usize::MAX);
        assert_eq!(
            cl, want_body.len(),
            "{path}: Content-Length must equal the body byte length"
        );
        assert_eq!(
            body, want_body,
            "{path}: body bytes must roundtrip exactly (got {} bytes: {:?})",
            body.len(),
            body
        );
    }

    let _ = eval_string(&mut ctx, "globalThis.__srv.stop(), 'stopped'");
    drive_event_loop(&mut ctx, 10);
}
