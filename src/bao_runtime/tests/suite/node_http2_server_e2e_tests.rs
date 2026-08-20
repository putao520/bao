// @trace REQ-ENG-006 [api:node:http2] [level:integration]
//
// node:http2 server response path — the v-final6 crash repro class.
// createServer + listen used to bind but never fire the listen callback,
// and any inbound request hit a handler whose (req, res) surface did not
// exist (no writeHead/end/data-end wiring) → "Returning from a request
// handler without responding" → uWS std::terminate → mozalloc_abort →
// SIGSEGV (6/6 deterministic). These tests pin the wired behavior:
// compat (req, res) roundtrips (GET + POST binary), the session-shape
// surface the http_te_parity tests also pin, explicit-500 crash-class
// guards, listen-callback firing, and request concurrency.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

/// Pump the unified event loop (uWS sockets + timers + jobs) so the
/// in-process server accepts, parses, streams bodies and responds.
fn pump(ctx: &mut JsContext, passes: usize) {
    for _ in 0..passes {
        let mut cxm = ctx.cx();
        bun_runtime::timers::drain_and_check(&mut cxm);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Send a raw request and read until the response BODY is complete
/// (Content-Length framing; keep-alive connections never close, so
/// head-completion alone is not enough for body assertions).
fn raw_roundtrip_full(ctx: &mut JsContext, port: u16, request: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("read timeout");
    stream.write_all(request).expect("write request");

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    for _ in 0..100 {
        pump(ctx, 2);
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                // Complete once the declared body has arrived.
                if let Some(cl) = content_length(&buf) {
                    if let Some(head_end) = find_head_end(&buf) {
                        if buf.len() - head_end >= cl {
                            break;
                        }
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
    buf
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
}

fn content_length(buf: &[u8]) -> Option<usize> {
    let head_end = find_head_end(buf)?;
    let head = String::from_utf8_lossy(&buf[..head_end]);
    for line in head.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            return v.trim().parse::<usize>().ok();
        }
    }
    None
}

fn head_str(resp: &[u8]) -> String {
    let end = find_head_end(resp).unwrap_or(resp.len());
    String::from_utf8_lossy(&resp[..end]).into_owned()
}

fn body_bytes(resp: &[u8]) -> &[u8] {
    match find_head_end(resp) {
        Some(e) => &resp[e..],
        None => &[],
    }
}

/// Reserve a free port in Rust (h2 address() echoes the requested port, no
/// ephemeral surfacing — same constraint as http_te_parity).
fn free_port() -> u16 {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve free port");
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    port
}

fn fresh_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// The exact v-final6 repro shape: compat (req, res) handler, POST with a
/// 256-byte binary body accumulated through req 'data'/'end', answered with
/// res.writeHead + res.end(Buffer). Byte-exact roundtrip both directions.
#[test]
fn test_h2_server_post_binary_roundtrip() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            var body256 = Buffer.alloc(256);
            for (var i = 0; i < 256; i++) body256[i] = i;
            globalThis.__srvGotLen = -1;
            globalThis.__srvGotEq = false;
            var srv = http2.createServer(function (req, res) {{
              var chunks = [];
              req.on('data', function (c) {{ chunks.push(c); }});
              req.on('end', function () {{
                var got = Buffer.concat(chunks);
                globalThis.__srvGotLen = got.length;
                globalThis.__srvGotEq = got.equals(body256);
                res.setHeader('x-bao-echo', 'yes');
                res.writeHead(200, {{ 'content-type': 'application/octet-stream' }});
                res.end(body256);
              }});
            }});
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok", "setup eval failed: {}", setup);

    let mut body256 = vec![0u8; 256];
    for (i, b) in body256.iter_mut().enumerate() {
        *b = i as u8;
    }
    let mut request = format!(
        "POST /bin HTTP/1.1\r\nHost: x\r\nContent-Length: 256\r\n\r\n"
    )
    .into_bytes();
    request.extend_from_slice(&body256);

    let resp = raw_roundtrip_full(&mut ctx, port, &request);

    let head = head_str(&resp);
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "h2 server must answer 200; got: {:?}",
        head.split("\r\n").next().unwrap_or("")
    );
    assert!(
        head.to_ascii_lowercase().contains("x-bao-echo: yes"),
        "setHeader header must reach the wire; head: {:?}",
        head
    );
    assert_eq!(
        content_length(&resp),
        Some(256),
        "binary body must declare CL 256; head: {:?}",
        head
    );
    let body = body_bytes(&resp);
    assert_eq!(body.len(), 256, "body must carry exactly 256 bytes");
    assert_eq!(
        body.to_vec(),
        body256,
        "binary body must roundtrip byte-exact (0..255)"
    );

    // JS-side accumulation truth (what the handler actually received).
    let got_len = eval_str(&mut ctx, "globalThis.__srvGotLen");
    let got_eq = eval_str(&mut ctx, "globalThis.__srvGotEq");
    assert_eq!(got_len, "256", "handler must receive all 256 body bytes");
    assert_eq!(got_eq, "true", "handler body must equal the sent bytes");
}

/// GET roundtrip through the compat surface: writeHead(headers) + end(body)
/// with a custom header surviving to the wire.
#[test]
fn test_h2_server_get_roundtrip() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            var srv = http2.createServer(function (req, res) {{
              res.writeHead(200, {{ 'content-type': 'text/plain', 'x-bao-mark': 'get' }});
              res.end('hello-h2');
            }});
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    let resp = raw_roundtrip_full(
        &mut ctx,
        port,
        b"GET /a HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    let head = head_str(&resp);
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "GET must answer 200; got: {:?}",
        head.split("\r\n").next().unwrap_or("")
    );
    assert!(
        head.to_ascii_lowercase().contains("x-bao-mark: get"),
        "writeHead custom header missing; head: {:?}",
        head
    );
    assert_eq!(body_bytes(&resp), b"hello-h2", "GET body mismatch");
}

/// Crash-class guard (node:http 4c933019 pattern): a handler that returns
/// without responding gets an explicit 500 — never uWS std::terminate /
/// mozalloc_abort. The server must stay alive for the next request.
#[test]
fn test_h2_server_unresponded_handler_gets_500() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            var srv = http2.createServer(function (req, res) {{
              if (req.url === '/bad') return; // silently unresponsive
              res.writeHead(200);
              res.end('alive');
            }});
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    let resp = raw_roundtrip_full(
        &mut ctx,
        port,
        b"GET /bad HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    let head = head_str(&resp);
    assert!(
        head.starts_with("HTTP/1.1 500"),
        "unresponded handler must yield explicit 500 (crash-class guard); got: {:?}",
        head.split("\r\n").next().unwrap_or("")
    );
    assert!(
        body_bytes(&resp).starts_with(b"handler did not respond"),
        "500 body must name the failure; got: {:?}",
        String::from_utf8_lossy(body_bytes(&resp))
    );

    // Process survived: the server answers the next request normally.
    let resp2 = raw_roundtrip_full(
        &mut ctx,
        port,
        b"GET /good HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        head_str(&resp2).starts_with("HTTP/1.1 200"),
        "server must stay alive after the 500 guard; got: {:?}",
        head_str(&resp2).split("\r\n").next().unwrap_or("")
    );
    assert_eq!(body_bytes(&resp2), b"alive");
}

/// listen(port, host, callback): the callback fires when the socket
/// actually binds (host no longer swallows the callback slot), and
/// 'listening' is emitted on the server.
#[test]
fn test_h2_server_listen_callback_fires() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            globalThis.__h2listening = 0;
            globalThis.__h2event = 0;
            var srv = http2.createServer(function (req, res) {{
              res.end('ok');
            }});
            srv.on('listening', function () {{ globalThis.__h2event = 1; }});
            srv.listen({port}, '127.0.0.1', function () {{
              globalThis.__h2listening = srv.address().port;
            }});
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    // Drive the loop until the bind lands (or budget out).
    for _ in 0..50 {
        pump(&mut ctx, 2);
        if eval_str(&mut ctx, "globalThis.__h2listening") != "0" {
            break;
        }
    }
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__h2listening"),
        format!("{}", port),
        "listen callback must fire with the bound port"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__h2event"),
        "1",
        "server 'listening' event must fire"
    );
}

/// Concurrency/serialization: several requests across separate connections
/// all complete on one server (per-request state must not leak across
/// requests — the finish/responder bookkeeping is per-request).
#[test]
fn test_h2_server_multiple_requests() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            globalThis.__h2count = 0;
            var srv = http2.createServer(function (req, res) {{
              globalThis.__h2count++;
              res.writeHead(200);
              res.end('n=' + globalThis.__h2count);
            }});
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    for expected in 1..=3 {
        let resp = raw_roundtrip_full(
            &mut ctx,
            port,
            b"GET /c HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert!(
            head_str(&resp).starts_with("HTTP/1.1 200"),
            "request {} must answer 200; got: {:?}",
            expected,
            head_str(&resp).split("\r\n").next().unwrap_or("")
        );
        assert_eq!(
            body_bytes(&resp),
            format!("n={}", expected).as_bytes(),
            "request {} body mismatch",
            expected
        );
    }
}

/// res.write() + res.end() concatenation and double-end idempotence: the
/// second end() is a no-op (a second uWS end() on the same response is the
/// use-after-answer crash class), and the connection still yields exactly
/// one response.
#[test]
fn test_h2_server_write_chunks_and_double_end() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            var srv = http2.createServer(function (req, res) {{
              res.writeHead(200, {{ 'content-type': 'text/plain' }});
              res.write('chunk-');
              res.write('plus;');
              res.end('tail');
              res.end('dropped'); // no-op — must not double-answer
            }});
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    let resp = raw_roundtrip_full(
        &mut ctx,
        port,
        b"GET /w HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        head_str(&resp).starts_with("HTTP/1.1 200"),
        "chunked-write handler must answer 200; got: {:?}",
        head_str(&resp).split("\r\n").next().unwrap_or("")
    );
    assert_eq!(
        body_bytes(&resp),
        b"chunk-plus;tail",
        "write()+write()+end() body must concatenate exactly once"
    );
}

/// Session-shape surface on arg1 stays intact (the http_te_parity tests pin
/// the same shape; this guards the union object carrying both): a handler
/// written against (stream, headers) using stream.respond/stream.end keeps
/// working, and the server 'stream' event fires with (stream, headers).
#[test]
fn test_h2_server_session_surface_and_stream_event() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            globalThis.__streamEvt = '';
            var srv = http2.createServer(function (stream, headers) {{
              stream.respond({{ ':status': 200 }}, {{ endStream: false }});
              stream.end('sess-ok');
            }});
            srv.on('stream', function (stream, headers, flags) {{
              globalThis.__streamEvt = (typeof stream.respond) + '/' + headers.host;
            }});
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    let resp = raw_roundtrip_full(
        &mut ctx,
        port,
        b"GET /s HTTP/1.1\r\nHost: unit-tests\r\nConnection: close\r\n\r\n",
    );
    assert!(
        head_str(&resp).starts_with("HTTP/1.1 200"),
        "session-shape handler must answer 200; got: {:?}",
        head_str(&resp).split("\r\n").next().unwrap_or("")
    );
    assert!(
        resp.ends_with(b"sess-ok") || body_bytes(&resp) == b"sess-ok",
        "session end() body mismatch: {:?}",
        String::from_utf8_lossy(&resp)
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__streamEvt"),
        "function/unit-tests",
        "server 'stream' event must carry (stream-with-respond, headers)"
    );
}
