// @trace REQ-ENG-006 [api:node:http2] [level:integration]
//
// C4 line-coverage wave for src/node_http2.rs. The existing
// node_http2_server_e2e_tests pin the compat roundtrip class; this file
// drives the remaining branch surface per the file read:
//   - res header API natives and every error arm (writeHead double-fire,
//     setHeader after-send/argc/name/value, removeHeader after-send,
//     write-after-end) with Node's exact message strings;
//   - session-shape stream methods (respond endStream / no-args /
//     after-end no-op, end, close) plus the throwing-'stream'-listener
//     exception-clear arm;
//   - listen/close lifecycle: host and no-host listen, fractional-port
//     parse, EADDRINUSE bind-failure arm ('error' + cb(err)), close
//     idempotence, close of never-listened servers, native validation arms;
//   - secure server ctor forms, handler-missing arm, close idempotence, and
//     a real SSL bind with a generated PEM;
//   - client session surface + __http2_fetch roundtrip against the local
//     capture server (request body string/Buffer, response/data/end events,
//     body-validation throws);
//   - pure-JS Http2Session/Http2Stream surface, settings pack/unpack
//     roundtrip, sensitiveHeaders, ctor forms, constants;
//   - mid-body abort notification (h2_on_aborted).

use bao_engine::context::JsContext;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
#[path = "common/mod.rs"]
mod common;

use common::eval_str;

/// Pump the unified event loop (uWS sockets + timers + jobs).
fn pump(ctx: &mut JsContext, passes: usize) {
    for _ in 0..passes {
        let mut cxm = ctx.cx();
        bun_runtime::timers::drain_and_check(&mut cxm);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Send a raw request and read until the response body is complete
/// (Content-Length framing; keep-alive responses never close).
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

/// Embed a Rust string as a JS single-quoted literal (PEM bodies carry
/// newlines but no single quotes).
fn js_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// res header API full surface: setHeader (string / int32 / double values),
/// getHeader (present + missing + non-string), getHeaders, hasHeader (both
/// arms), removeHeader, writeHead(status, statusText, headers) with the
/// store-override merge, write with string/Buffer/Uint8Array chunks, end;
/// 'finish'/'close' lifecycle events on the response.
#[test]
fn h2_res_header_api_full_surface() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            globalThis.__evts = [];
            globalThis.srv = null;
            var srv = http2.createServer(function (req, res) {{
              res.on('finish', function () {{ globalThis.__evts.push('finish'); }});
              res.on('close', function () {{ globalThis.__evts.push('close'); }});

              res.setHeader('x-store', 'from-store');
              res.setHeader('x-num', 42);
              res.setHeader('x-keep', 2.25);
              res.setHeader('x-frac', 3.5);

              globalThis.__g_present = res.getHeader('x-store');
              globalThis.__g_missing = res.getHeader('nope');
              globalThis.__g_nonstring = res.getHeader(42);
              globalThis.__has_yes = res.hasHeader('x-store');
              globalThis.__has_no = res.hasHeader('nope');
              globalThis.__has_nonstring = res.hasHeader(42);
              globalThis.__gk = Object.keys(res.getHeaders()).sort().join(',');

              res.removeHeader('x-frac');
              globalThis.__has_after_rm = res.hasHeader('x-frac');

              res.writeHead(201, 'Created', {{
                'content-type': 'text/plain',
                'x-store': 'from-obj',
                'x-only': 'obj'
              }});
              globalThis.__statuscode = res.statusCode;

              res.write('S|');
              res.write(Buffer.from([1, 2, 3, 4]));
              res.write(new Uint8Array([9, 8]));
              res.end('|tail');
            }});
            globalThis.srv = srv;
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok", "setup eval failed: {}", setup);

    let resp = raw_roundtrip_full(
        &mut ctx,
        port,
        b"GET /hdr HTTP/1.1\r\nHost: c4\r\nConnection: close\r\n\r\n",
    );
    let head = head_str(&resp);
    assert!(
        head.starts_with("HTTP/1.1 201"),
        "writeHead(201) status must reach the wire; got: {:?}",
        head.split("\r\n").next().unwrap_or("")
    );
    let lower = head.to_ascii_lowercase();
    assert!(
        lower.contains("x-only: obj"),
        "writeHead-object header must reach the wire; head: {:?}",
        head
    );
    assert!(
        lower.contains("x-num: 42"),
        "setHeader int32 value must stringify onto the wire; head: {:?}",
        head
    );
    assert!(
        lower.contains("x-keep: 2.25"),
        "setHeader double value must stringify onto the wire; head: {:?}",
        head
    );
    assert!(
        !lower.contains("x-frac"),
        "removed header must be absent from the wire; head: {:?}",
        head
    );
    // Store value overridden by the writeHead object — exactly one wire hit,
    // carrying the object's value (store flush skips overridden keys).
    assert!(
        lower.matches("x-store").count() == 1 && lower.contains("x-store: from-obj"),
        "writeHead object must override the setHeader store exactly once; head: {:?}",
        head
    );
    let body = body_bytes(&resp);
    let mut want = b"S|".to_vec();
    want.extend_from_slice(&[1, 2, 3, 4]);
    want.extend_from_slice(&[9, 8]);
    want.extend_from_slice(b"|tail");
    assert_eq!(
        body,
        &want[..],
        "string + Buffer + Uint8Array chunks must concatenate byte-exact"
    );

    // JS-side recorded values (the getter truth inside the handler).
    assert_eq!(eval_str(&mut ctx, "globalThis.__g_present"), "from-store");
    assert_eq!(eval_str(&mut ctx, "String(globalThis.__g_missing)"), "undefined");
    assert_eq!(eval_str(&mut ctx, "String(globalThis.__g_nonstring)"), "undefined");
    assert_eq!(eval_str(&mut ctx, "globalThis.__has_yes"), "true");
    assert_eq!(eval_str(&mut ctx, "globalThis.__has_no"), "false");
    assert_eq!(eval_str(&mut ctx, "globalThis.__has_nonstring"), "false");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__gk"),
        "x-frac,x-keep,x-num,x-store"
    );
    assert_eq!(eval_str(&mut ctx, "globalThis.__has_after_rm"), "false");
    assert_eq!(eval_str(&mut ctx, "globalThis.__statuscode"), "201");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__evts.join(',')"),
        "finish,close",
        "res end() must emit 'finish' then 'close'"
    );

    srv_close(&mut ctx, "srv");
}

/// Every res error arm is reported with Node's exact message and the request
/// still completes normally (errors are caught inside the handler).
#[test]
fn h2_res_error_arms_are_reported() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            globalThis.__errs = [];
            globalThis.__h2srv = null;
            function grab(label, fn) {{
              try {{ fn(); globalThis.__errs.push(label + ':no-throw'); }}
              catch (e) {{ globalThis.__errs.push(label + ':' + e.message); }}
            }}
            var srv = http2.createServer(function (req, res) {{
              grab('setHeader_noargs', function () {{ res.setHeader('x'); }});
              grab('setHeader_badname', function () {{ res.setHeader(42, 'v'); }});
              grab('setHeader_badvalue', function () {{ res.setHeader('x-bad', {{}}); }});

              res.writeHead(200, {{ 'content-type': 'text/plain' }});
              grab('writeHead_double', function () {{ res.writeHead(200); }});
              grab('setHeader_aftersend', function () {{ res.setHeader('x-late', 'v'); }});
              grab('removeHeader_aftersend', function () {{ res.removeHeader('x-late'); }});

              res.end('errors-done');
              grab('write_after_end', function () {{ res.write('late'); }});
              res.end('dropped'); // no-op second end
              res.setTimeout(5, function () {{ globalThis.__timeoutFired = true; }});
              res.setTimeout(5); // no-callback no-op arm
              res.setTimeout('not-a-number', function () {{ globalThis.__timeoutNonInt = true; }});
            }});
            globalThis.__h2srv = srv;
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    let resp = raw_roundtrip_full(
        &mut ctx,
        port,
        b"GET /err HTTP/1.1\r\nHost: c4\r\nConnection: close\r\n\r\n",
    );
    assert!(
        head_str(&resp).starts_with("HTTP/1.1 200"),
        "caught error arms must not poison the response; got: {:?}",
        head_str(&resp).split("\r\n").next().unwrap_or("")
    );
    assert_eq!(body_bytes(&resp), b"errors-done", "single end() body only");

    // Let the 5ms res.setTimeout callback land.
    for _ in 0..30 {
        pump(&mut ctx, 2);
        if eval_str(&mut ctx, "String(globalThis.__timeoutFired)") == "true" {
            break;
        }
    }

    let errs = eval_str(&mut ctx, "globalThis.__errs.join('|')");
    assert!(
        errs.contains("setHeader_noargs:res.setHeader(name, value) requires 2 arguments"),
        "setHeader argc arm message mismatch: {}",
        errs
    );
    assert!(
        errs.contains("setHeader_badname:res.setHeader name must be a string"),
        "setHeader name arm message mismatch: {}",
        errs
    );
    assert!(
        errs.contains("setHeader_badvalue:res.setHeader value must be a string or number"),
        "setHeader value arm message mismatch: {}",
        errs
    );
    assert!(
        errs.contains("writeHead_double:Headers already sent"),
        "double writeHead arm message mismatch: {}",
        errs
    );
    assert!(
        errs.contains("setHeader_aftersend:Cannot set headers after they are sent to the client"),
        "setHeader-after-send arm message mismatch: {}",
        errs
    );
    assert!(
        errs.contains(
            "removeHeader_aftersend:Cannot remove headers after they are sent to the client"
        ),
        "removeHeader-after-send arm message mismatch: {}",
        errs
    );
    assert!(
        errs.contains("write_after_end:write after end"),
        "write-after-end arm message mismatch: {}",
        errs
    );
    assert!(
        !errs.contains(":no-throw"),
        "every probed arm must throw, some did not: {}",
        errs
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__timeoutFired)"),
        "true",
        "res.setTimeout callback must fire through the timer wheel"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__timeoutNonInt)"),
        "true",
        "non-int32 msecs must still arm the callback (0ms arm)"
    );

    srv_close(&mut ctx, "__h2srv");
}

/// Session-shape stream methods on the union request object: respond with
/// endStream, respond with no args, close auto-200, respond-after-end no-op —
/// plus a throwing session-style 'stream' listener whose exception the native
/// emitter clears (response still produced).
#[test]
fn h2_stream_shape_methods_and_throwing_stream_listener() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            globalThis.__streamEvtFired = false;
            globalThis.__h2srv = null;
            var srv = http2.createServer(function (req, res) {{
              var stream = req.stream;
              if (req.url === '/r1') {{
                stream.respond({{ ':status': 200, 'content-type': 'text/plain' }}, {{ endStream: true }});
              }} else if (req.url === '/r2') {{
                stream.respond();
                stream.end('r2-body');
              }} else if (req.url === '/r3') {{
                stream.close();
              }} else {{
                stream.end('r4');
                stream.respond({{ ':status': 500 }}); // after end — no-op
              }}
            }});
            srv.on('stream', function (stream, headers, flags) {{
              globalThis.__streamEvtFired = true;
              throw new Error('listener bomb'); // must be cleared, not fatal
            }});
            globalThis.__h2srv = srv;
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    let get = |ctx: &mut JsContext, path: &str| {
        raw_roundtrip_full(
            ctx,
            port,
            format!("GET {} HTTP/1.1\r\nHost: c4\r\nConnection: close\r\n\r\n", path).into_bytes()
                .as_slice(),
        )
    };

    // /r1: respond({endStream:true}) — empty 200.
    let r1 = get(&mut ctx, "/r1");
    assert!(
        head_str(&r1).starts_with("HTTP/1.1 200"),
        "endStream respond must answer 200; got: {:?}",
        head_str(&r1).split("\r\n").next().unwrap_or("")
    );
    assert_eq!(body_bytes(&r1), b"", "endStream respond must carry no body");

    // /r2: respond() with no args then end('r2-body').
    let r2 = get(&mut ctx, "/r2");
    assert!(
        head_str(&r2).starts_with("HTTP/1.1 200"),
        "no-arg respond must default the status; got: {:?}",
        head_str(&r2).split("\r\n").next().unwrap_or("")
    );
    assert_eq!(body_bytes(&r2), b"r2-body");

    // /r3: stream.close() auto-200 empty.
    let r3 = get(&mut ctx, "/r3");
    assert!(
        head_str(&r3).starts_with("HTTP/1.1 200"),
        "stream.close() must auto-200; got: {:?}",
        head_str(&r3).split("\r\n").next().unwrap_or("")
    );
    assert_eq!(body_bytes(&r3), b"");

    // /r4: respond after end is a no-op — wire stays 200 with the body.
    let r4 = get(&mut ctx, "/r4");
    assert!(
        head_str(&r4).starts_with("HTTP/1.1 200"),
        "respond-after-end must be a no-op (not a 500); got: {:?}",
        head_str(&r4).split("\r\n").next().unwrap_or("")
    );
    assert_eq!(body_bytes(&r4), b"r4");

    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__streamEvtFired)"),
        "true",
        "session-style 'stream' event must fire alongside the compat handler"
    );

    srv_close(&mut ctx, "__h2srv");
}

/// Listen lifecycle: callback + 'listening' on bind, the EADDRINUSE
/// bind-failure arm ('error' event + cb(err) with code), fractional-port
/// listen, close of bound / failed-bind / never-listened servers, and the
/// native listen/close validation arms.
#[test]
fn h2_listen_lifecycle_eaddrinuse_and_close_arms() {
    let mut ctx = fresh_ctx();
    let port = free_port();
    let frac_port = port as u32 + 1000;

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            globalThis.__ev = [];
            var srv = http2.createServer(function (req, res) {{ res.end('ok'); }});
            srv.on('listening', function () {{ globalThis.__ev.push('listening'); }});
            srv.on('error', function (e) {{ globalThis.__ev.push('error:' + (e && e.code)); }});
            srv.listen({port}, '127.0.0.1', function () {{ globalThis.__ev.push('cb'); }});
            globalThis.__mainSrv = srv;

            // Fractional-port form: the native parses the double arm and
            // binds the truncated integer port (verified from Rust by an
            // actual connect — the wire is the truth source).
            globalThis.__dblFired = false;
            var dbl = http2.createServer(function (req, res) {{ res.end('d'); }});
            dbl.listen({frac_port} + 0.9, '127.0.0.1', function () {{
              globalThis.__dblFired = true;
            }});
            globalThis.__dblSrv = dbl;

            // Second server on the SAME port → bind-failure arm.
            globalThis.__errEvt = null;
            globalThis.__errCbArg = null;
            var clash = http2.createServer(function (req, res) {{ res.end('c'); }});
            clash.on('error', function (e) {{ globalThis.__errEvt = e && e.code; }});
            clash.listen({port}, '127.0.0.1', function (err) {{
              globalThis.__errCbArg = err && err.code;
            }});
            globalThis.__clashSrv = clash;

            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    // Drive the loop until all three listen outcomes have landed.
    for _ in 0..100 {
        pump(&mut ctx, 2);
        let done = eval_str(&mut ctx, "String(globalThis.__errCbArg !== null)");
        if done == "true" {
            break;
        }
    }

    assert_eq!(
        eval_str(&mut ctx, "globalThis.__ev.join(',')"),
        "listening,cb",
        "bind success must fire 'listening' then the user callback"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__dblFired)"),
        "true",
        "fractional-port listen callback must fire"
    );
    {
        let frac_port = port as u32 + 1000;
        let probe = TcpStream::connect(("127.0.0.1", frac_port as u16));
        assert!(
            probe.is_ok(),
            "fractional port {} must bind at its truncated integer value (wire probe)",
            frac_port
        );
    }
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__errEvt)"),
        "EADDRINUSE",
        "bind failure must emit 'error' with the EADDRINUSE code"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__errCbArg)"),
        "EADDRINUSE",
        "bind failure must hand the error carrier to the listen callback"
    );

    // Close every bound/failed server: callback fires per close; a second
    // close is an idempotent no-op (null app/ud arms).
    let closes = eval_str(
        &mut ctx,
        r#"
        (function() {
          var out = [];
          globalThis.__mainSrv.close(function () { out.push('m'); });
          globalThis.__mainSrv.close(function () { out.push('m2'); });
          globalThis.__dblSrv.close(function () { out.push('d'); });
          globalThis.__clashSrv.close(function () { out.push('c'); });
          return out.join(',');
        })()
        "#,
    );
    assert_eq!(
        closes, "m,m2,d,c",
        "close must fire its callback once per server and stay idempotent"
    );

    // Close of a never-listened server (both pointer arms null) + a fresh
    // bind/close cycle.
    let close_out = eval_str(
        &mut ctx,
        &format!(
            r#"
            (function() {{
              var http2 = require('http2');
              var out = [];
              var never = http2.createServer(function (req, res) {{ res.end('x'); }});
              never.close(function () {{ out.push('neverCb'); }});

              var srv = http2.createServer(function (req, res) {{ res.end('x'); }});
              srv.listen({port} + 2, '127.0.0.1');
              srv.close(function () {{ out.push('closeCb'); }});
              srv.close(function () {{ out.push('closeCb2'); }});
              out.push('listening:' + srv.listening);
              return out.join(',');
            }})()
            "#
        ),
    );
    assert_eq!(
        close_out, "neverCb,closeCb,closeCb2,listening:false",
        "never-listened close + fresh bind/close cycle mismatch"
    );

    // Native listen/close validation arms.
    let native_arms = eval_str(
        &mut ctx,
        r#"
        (function() {
          var http2 = require('http2');
          var out = [];
          out.push('nobj:' + String(__http2_server_listen('not-an-object', 30000)));
          out.push('noargs:' + String(__http2_server_listen()));
          out.push('closeNoargs:' + String(__http2_server_close()));
          try { __http2_server_listen({}, 30000); out.push('nohandler:miss'); }
          catch (e) { out.push('nohandler:' + e.message); }
          return out.join('|');
        })()
        "#,
    );
    assert_eq!(
        native_arms,
        "nobj:undefined|noargs:undefined|closeNoargs:undefined|\
         nohandler:http2.createServer requires a stream handler",
        "native listen/close validation arms mismatch: {}",
        native_arms
    );
}

/// Secure server: ctor argument forms, the handler-missing listen arm,
/// close-on-never-listened idempotence, and a real SSL bind with a generated
/// self-signed cert (skipped visibly when openssl is unavailable).
#[test]
fn h2_secure_server_surface_and_arms() {
    let mut ctx = fresh_ctx();

    let surface = eval_str(
        &mut ctx,
        r#"
        (function() {
          var http2 = require('http2');
          var out = [];

          // Single-arg form: the function lands in the handler slot.
          var s1 = http2.createSecureServer(function (req, res) { res.end('s'); });
          out.push('form1:' + (s1 instanceof http2.Http2SecureServer));
          out.push('handler:' + (typeof s1._onStreamHandler));

          // Two-arg form.
          var s2 = http2.createSecureServer({ key: 'k', cert: 'c' }, function (req, res) {});
          out.push('form2:' + (s2 instanceof http2.Http2SecureServer));
          out.push('opts:' + (s2._options.key === 'k' && s2._options.cert === 'c'));

          // Pre-listen address + setTimeout surface.
          var a = s1.address();
          out.push('addr:' + (a.port === 0 && a.family === 'IPv4'));
          s1.setTimeout(10, function () { out.push('timeoutCb'); });

          // Handler-missing listen arm on the native bridge.
          try { __http2_secure_server_listen({}, 30000); out.push('nohandler:miss'); }
          catch (e) { out.push('nohandler:' + e.message); }

          // argc / non-object arms.
          out.push('nobj:' + String(__http2_secure_server_listen('x', 30000)));
          out.push('noargs:' + String(__http2_secure_server_listen()));
          out.push('closeNoargs:' + String(__http2_secure_server_close()));

          // Close of a never-listened secure server: null arms + callback.
          var closed = [];
          s1.close(function () { closed.push('cb'); });
          s1.close(function () { closed.push('cb2'); });
          out.push('closed:' + closed.join(','));

          return out.join('|');
        })()
        "#,
    );
    assert_eq!(
        surface,
        "form1:true|handler:function|form2:true|opts:true|addr:true|timeoutCb|\
         nohandler:http2.createSecureServer requires a stream handler|\
         nobj:undefined|noargs:undefined|closeNoargs:undefined|closed:cb,cb2",
        "secure server surface mismatch: {}",
        surface
    );

    // Real SSL bind with a generated self-signed cert.
    let dir = std::env::temp_dir().join(format!("bao-c4-h2-secure-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let key_path = dir.join("key.pem");
    let cert_path = dir.join("cert.pem");
    let gen_out = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            key_path.to_str().unwrap(),
            "-out",
            cert_path.to_str().unwrap(),
            "-subj",
            "/CN=localhost",
            "-days",
            "2",
        ])
        .output();
    let generated = match gen_out {
        Ok(o) => o.status.success(),
        Err(e) => {
            eprintln!("[skip] openssl unavailable ({}) — real SSL bind not exercised", e);
            false
        }
    };
    if !generated {
        eprintln!("[skip] openssl req failed — real SSL bind not exercised");
    } else {
        let key = std::fs::read_to_string(&key_path).expect("read key.pem");
        let cert = std::fs::read_to_string(&cert_path).expect("read cert.pem");
        let port = free_port();
        let setup = eval_str(
            &mut ctx,
            &format!(
                r#"
                var http2 = require('http2');
                globalThis.__sev = [];
                globalThis.__seSrv = null;
                var srv = http2.createSecureServer({{
                  key: {},
                  cert: {}
                }}, function (req, res) {{ res.end('tls'); }});
                srv.on('listening', function () {{ globalThis.__sev.push('listening'); }});
                srv.listen({port}, '127.0.0.1', function () {{
                  globalThis.__sev.push('cb:' + srv.address().port);
                }});
                globalThis.__seSrv = srv;
                'setup-ok'
                "#,
                js_string_literal(&key),
                js_string_literal(&cert),
                port = port
            ),
        );
        assert_eq!(setup, "setup-ok");

        for _ in 0..100 {
            pump(&mut ctx, 2);
            if eval_str(&mut ctx, "globalThis.__sev.length") != "0" {
                break;
            }
        }
        let sev = eval_str(&mut ctx, "globalThis.__sev.join(',')").replace(&port.to_string(), "PORT");
        assert_eq!(
            sev, "listening,cb:PORT",
            "secure server must bind with a real PEM pair; got: {}",
            sev
        );

        let closed = eval_str(
            &mut ctx,
            r#"
            (function() {
              var out = [];
              globalThis.__seSrv.close(function () { out.push('cb'); });
              globalThis.__seSrv.close(function () { out.push('cb2'); });
              globalThis.__seSrv.listening = false;
              return out.join(',');
            })()
            "#,
        );
        assert_eq!(closed, "cb,cb2", "secure close must fire and stay idempotent");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Client session surface + __http2_fetch roundtrip against the local
/// capture server: response/data/end delivery, string and Buffer request
/// bodies, the body-validation throw arms, and the session control surface.
#[test]
fn h2_client_session_and_fetch_roundtrip() {
    let (port, captured) = common::capture_server::start_capture_server();
    let mut ctx = fresh_ctx();

    // Connect a client session and stage three requests (POST string body,
    // POST Buffer body, GET). ':scheme' + ':authority' build the URL.
    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            var sess = http2.connect('http://127.0.0.1:{port}');
            globalThis.__sess = sess;
            globalThis.__out = [];

            function stage(label, headers, options) {{
              var stream = sess.request(headers, options);
              var got = {{ label: label, status: null, body: '', err: null }};
              stream.on('response', function (h) {{ got.status = h[':status']; }});
              stream.on('data', function (c) {{ got.body += Buffer.from(c).toString('utf8'); }});
              stream.on('end', function () {{ globalThis.__out.push(JSON.stringify(got)); }});
              stream.on('error', function (e) {{
                got.err = String(e && e.message);
                globalThis.__out.push(JSON.stringify(got));
              }});
              return stream;
            }}

            stage('post-str', {{
              ':method': 'POST', ':scheme': 'http', ':authority': '127.0.0.1:{port}',
              ':path': '/post-str', 'x-c4': 'v1'
            }}, {{ body: 'raw-post-body-c4' }});

            stage('post-buf', {{
              ':method': 'POST', ':scheme': 'http', ':authority': '127.0.0.1:{port}',
              ':path': '/post-buf'
            }}, {{ body: Buffer.from([1, 2, 3]) }});

            stage('get', {{
              ':method': 'GET', ':scheme': 'http', ':authority': '127.0.0.1:{port}',
              ':path': '/get-c4'
            }}, {{}});

            // Session surface probes.
            globalThis.__sessSurface = [
              sess.encrypted === false,
              sess.alpnProtocol === 'h2',
              Array.isArray(sess.originSet) && sess.originSet.length === 1,
              sess.localSettings.headerTableSize === 4096,
              sess.remoteSettings.initialWindowSize === 65535,
              typeof sess.state.nextStreamID === 'number',
              sess.pendingSettingsAck === false,
              sess.closed === false,
              sess.destroyed === false
            ].join(',');

            // Control surface.
            var pingArgs = null;
            globalThis.__pingRet = sess.ping(function (err, dur, payload) {{
              pingArgs = [err === null, dur === 0, payload && payload.length === 8];
            }}) === true;
            globalThis.__pingArgs = pingArgs;

            sess.settings = {{ headerTableSize: 8192 }};
            globalThis.__localAfterSettings = sess.localSettings.headerTableSize;
            sess.sendSettings({{ maxFrameSize: 9999 }});
            globalThis.__afterSend = sess.localSettings.maxFrameSize;
            sess.setLocalWindowSize(123456);
            globalThis.__afterWindow = sess.localSettings.initialWindowSize;

            // Direct __http2_fetch promise shape (bypasses the stage wiring):
            // the bridge returns a real Promise resolving to a Response with
            // status + arrayBuffer — asserted after the settle wait.
            globalThis.__probe2 = 'pending';
            var p2 = __http2_fetch('http://127.0.0.1:{port}/probe2', 'GET', '{{}}', null);
            globalThis.__probe2IsPromise = (p2 && typeof p2.then === 'function');
            p2.then(function (resp) {{
              globalThis.__probe2 = 'resp:' + resp.status + ':ab=' + typeof resp.arrayBuffer;
              return resp.arrayBuffer().then(function (ab) {{
                globalThis.__probe2 += ':len=' + ab.byteLength;
              }});
            }}, function (e) {{ globalThis.__probe2 = 'err:' + (e && e.message); }});

            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok", "client session setup failed");

    // Pump until all three staged requests settle (or budget out). The
    // proven settle shape (fetch_headers_e2e): RunJobs + a real uWS loop tick
    // via with_event_loop — drain_and_check alone starves the lazy pump.
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            {
                let cx_raw = ctx.raw_cx();
                unsafe {
                    mozjs_sys::jsapi::js::RunJobs(cx_raw);
                }
            }
            bun_runtime::timers::with_event_loop(|loop_| {
                loop_.tick_without_idle(std::ptr::null_mut());
            });
            std::thread::sleep(Duration::from_millis(2));
            // Both settle markers must land: the three staged streams AND the
            // direct __http2_fetch probe (its promise chain can lag the staged
            // streams by a microtask hop — break only when both are done).
            let n = eval_str(&mut ctx, "globalThis.__out.length");
            let probe2 = eval_str(&mut ctx, "String(globalThis.__probe2)");
            if n == "3" && probe2 != "pending" {
                break;
            }
        }
    }

    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__probe2IsPromise)"),
        "true",
        "__http2_fetch must return a thenable"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__probe2)"),
        "resp:200:ab=function:len=2",
        "__http2_fetch promise must resolve to a 200 Response with arrayBuffer"
    );

    let out_raw = eval_str(&mut ctx, "globalThis.__out.join('|')");
    assert_eq!(
        out_raw.matches("|").count() + 1,
        3,
        "all three staged requests must settle; got: {}",
        out_raw
    );
    for entry in out_raw.split('|') {
        assert!(
            entry.contains("\"status\":\"200\""),
            "every staged request must answer 200; got: {}",
            entry
        );
        assert!(
            entry.contains("\"body\":\"ok\""),
            "capture server body must be delivered as the data chunk; got: {}",
            entry
        );
        assert!(
            entry.contains("\"err\":null"),
            "no staged request may error; got: {}",
            entry
        );
    }

    // The wire saw the POST bodies and headers (capture server truth).
    let post_str = common::capture_server::request_for(&captured, "/post-str");
    assert!(
        post_str.contains("post /post-str"),
        "POST method must reach the wire; captured: {}",
        post_str
    );
    assert!(
        post_str.contains("x-c4: v1"),
        "custom request header must reach the wire; captured: {}",
        post_str
    );
    assert!(
        post_str.contains("raw-post-body-c4"),
        "string request body must reach the wire byte-exact; captured: {}",
        post_str
    );
    let post_buf = common::capture_server::request_for(&captured, "/post-buf");
    assert!(
        post_buf.contains("content-length: 3"),
        "Buffer body must declare its byte length; captured: {}",
        post_buf
    );

    // Session surface + control probes.
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__sessSurface"),
        "true,true,true,true,true,true,true,true,true",
        "session property surface mismatch"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__pingRet)"),
        "true",
        "ping must return true on the open session"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__pingArgs.join(',')"),
        "true,true,true",
        "ping cb must carry (null, 0, 8-byte payload)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__localAfterSettings)"),
        "8192",
        "settings setter must update localSettings"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__afterSend)"),
        "9999",
        "sendSettings must update localSettings"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__afterWindow)"),
        "123456",
        "setLocalWindowSize must update initialWindowSize"
    );
    // Session lifecycle AFTER the staged fetches settled: goaway closes the
    // session and every open stream; then the closed-session arms.
    let lifecycle = eval_str(
        &mut ctx,
        r#"
        (function() {
          var sess = globalThis.__sess;
          var out = [];
          var goawaySeen = 0;
          sess.on('goaway', function () { goawaySeen++; });
          sess.goaway(2, 1, null);
          out.push('goaway:' + goawaySeen);
          out.push('closed:' + sess.closed);
          try { sess.request({ ':path': '/' }); out.push('req-after-close:miss'); }
          catch (e) { out.push('req-after-close:' + e.message); }
          var closeCb = false;
          sess.close(function () { closeCb = true; }); // already closed — immediate cb
          out.push('close-cb:' + closeCb);
          sess.destroy(new Error('boom'), function () {});
          out.push('destroy-closed:' + sess.destroyed);
          return out.join('|');
        })()
        "#,
    );
    assert!(
        lifecycle.contains("goaway:1")
            && lifecycle.contains("closed:true")
            && lifecycle.contains("req-after-close:Session is closed")
            && lifecycle.contains("close-cb:true")
            && lifecycle.contains("destroy-closed:true"),
        "session lifecycle sequence mismatch: {}",
        lifecycle
    );

    // Unsupported surfaces + http2_fetch validation throws.
    let closed_arms = eval_str(
        &mut ctx,
        r#"
        (function() {
          var http2 = require('http2');
          var sess = globalThis.__sess;
          var out = [];
          try { sess.request({ ':path': '/' }); out.push('req-after-close:miss'); }
          catch (e) { out.push('req-after-close:' + e.message); }
          var closeCb = false;
          sess.close(function () { closeCb = true; }); // already closed — immediate cb
          out.push('close-cb:' + closeCb);
          sess.destroy(new Error('boom'), function () {});
          out.push('destroy-closed:' + sess.destroyed);
          return out.join('|');
        })()
        "#,
    );
    assert!(
        closed_arms.contains("req-after-close:Session is closed"),
        "request on a closed session must throw; got: {}",
        closed_arms
    );
    assert!(
        closed_arms.contains("close-cb:true"),
        "close on an already-closed session must invoke the callback; got: {}",
        closed_arms
    );
    assert!(
        closed_arms.contains("destroy-closed:true"),
        "destroy must mark the session destroyed; got: {}",
        closed_arms
    );

    // http2_fetch validation throws (no network involved).
    let fetch_arms = eval_str(
        &mut ctx,
        r#"
        (function() {
          var out = [];
          try { __http2_fetch('http://127.0.0.1:9/x', 'GET', '{}', 123); out.push('num:miss'); }
          catch (e) { out.push('num:' + e.message); }
          try { __http2_fetch('http://127.0.0.1:9/x', 'GET', '{}', { nope: 1 }); out.push('obj:miss'); }
          catch (e) { out.push('obj:' + e.message); }
          return out.join('|');
        })()
        "#,
    );
    assert_eq!(
        fetch_arms,
        "num:http2: request body must be a string, Buffer, TypedArray or ArrayBuffer|\
         obj:http2: request body must be a string, Buffer, TypedArray or ArrayBuffer",
        "http2_fetch body validation arms mismatch: {}",
        fetch_arms
    );

    // Unsupported surfaces throw their documented errors; the pushStream
    // callback receives the not-supported error.
    let unsupported = eval_str(
        &mut ctx,
        r#"
        (function() {
          var http2 = require('http2');
          var sess = globalThis.__sess;
          var out = [];
          try { sess.respondWithFile('/x'); out.push('rwf:miss'); }
          catch (e) { out.push('rwf:' + e.message); }
          try { sess.respondWithFD(3); out.push('rfd:miss'); }
          catch (e) { out.push('rfd:' + e.message); }
          return out.join('|');
        })()
        "#,
    );
    assert_eq!(
        unsupported,
        "rwf:http2.respondWithFile is not supported|rfd:http2.respondWithFD is not supported",
        "unsupported-surface arms mismatch: {}",
        unsupported
    );
}

/// Pure-JS surface: Http2Session/Http2Stream construction, stream method
/// registry eviction, settings pack/unpack roundtrip, sensitiveHeaders,
/// createServer argument forms, and the module constants.
#[test]
fn h2_pure_js_session_stream_and_settings_utils() {
    let mut ctx = fresh_ctx();
    let battery = eval_str(
        &mut ctx,
        r#"
        (function() {
          var http2 = require('http2');
          function h2_noop() {}
          var out = [];

          // Direct session construction: server-mode sessions do not fetch.
          var sess = new http2.Http2Session('server', 'auth.example');
          out.push('type-server:' + (sess.type === 1));
          var st1 = sess.request({ ':path': '/a' });
          var st2 = sess.request({ ':path': '/b' });
          out.push('ids-odd:' + (st1.id % 2 === 1 && st2.id === st1.id + 2));
          out.push('registered:' + (Object.keys(sess._streams).length === 2));
          out.push('session-getter:' + (st1.session === sess));

          // Client-mode session property: encrypted by https authority.
          var csess = new http2.Http2Session('client', 'https://secure.example');
          out.push('client-encrypted:' + (csess.encrypted === true));
          out.push('client-type:' + (csess.type === 0));

          // Stream surface.
          st1.respond({ ':status': 200 });
          out.push('respond-chainable:' + (st1.respond({}) === st1));
          out.push('sendTrailers:' + (st1.sendTrailers({ 'x-t': '1' }) === st1));
          out.push('priority-noop:' + (st1.priority({ weight: 1 }) === st1));
          var pushErr = null;
          st1.pushStream({}, null, function (err) { pushErr = err; });
          out.push('push-err:' + (pushErr && pushErr.message === 'HTTP/2 server push is not supported'));
          var stTimeout = false;
          out.push('stream-timeout:' + ((st1.setTimeout(5, function () { stTimeout = true; }) === st1) && stTimeout));
          out.push('state-open:' + (st1.state.state === 0 && st1.ended === false && st1.closed === false));

          // end() closes the writable half; the registry eviction happens on
          // close/destroy (release-on-close), not on the half-close.
          var endCbRan = false;
          st1.end('payload', function () { endCbRan = true; });
          out.push('after-end:' + (st1.ended === true && st1.writable === false &&
                                   st1.state.state === 4 && endCbRan));
          out.push('end-no-evict:' + (Object.keys(sess._streams).length === 2));
          out.push('end-idempotent:' + (st1.end('again') === st1));

          // close() evicts; second close is a no-op.
          var closeCb = false;
          st2.close(0, function () { closeCb = true; });
          out.push('close-evict:' + (Object.keys(sess._streams).length === 1 && closeCb &&
                                     st2.closed === true && st2.readable === false));
          out.push('close-idempotent:' + (st2.close() === st2));

          // Session close destroys remaining open streams.
          var s3 = new http2.Http2Session('server', 'x');
          s3.request({ ':path': '/x' });
          s3.request({ ':path': '/y' });
          var closeEvt = 0;
          s3.on('close', function () { closeEvt++; });
          s3.close(function () {});
          out.push('session-close-evict:' + (Object.keys(s3._streams).length === 0));
          out.push('session-close-evt:' + (closeEvt === 1));

          // destroy(error) emits 'error' then 'close'.
          var s4 = new http2.Http2Session('server', 'x');
          var seen = [];
          s4.on('error', function (e) { seen.push('err:' + e.message); });
          s4.on('close', function () { seen.push('closed'); });
          s4.destroy(new Error('fatal'), function () {});
          out.push('destroy-seq:' + (seen.join(',') === 'err:fatal,closed' && s4.destroyed));
          var destroyCb = false;
          s4.destroy(null, function () { destroyCb = true; }); // idempotent
          out.push('destroy-idempotent:' + destroyCb);

          // ref/unref/setLocalWindowSize chainable.
          out.push('chainables:' + (s4.ref() === s4 && s4.unref() === s4 &&
                                    s4.setLocalWindowSize(4096) === s4));

          // Settings pack/unpack roundtrip.
          var defaults = http2.getDefaultSettings();
          out.push('defaults:' + (defaults.headerTableSize === 4096 &&
                                  defaults.initialWindowSize === 65535 &&
                                  defaults.maxFrameSize === 16384 &&
                                  defaults.maxConcurrentStreams === 100 &&
                                  defaults.maxHeaderListSize === 65535 &&
                                  defaults.enablePush === false));
          var packed = http2.getPackedSettings();
          out.push('packed-len:' + (packed.length === 36));
          out.push('packed-first-id:' + (packed[0] === 0 && packed[1] === 1));
          var unpacked = http2.getUnpackedSettings(packed);
          out.push('roundtrip:' + (unpacked.headerTableSize === 4096 &&
                                   !unpacked.enablePush &&
                                   unpacked.maxConcurrentStreams === 100 &&
                                   unpacked.initialWindowSize === 65535 &&
                                   unpacked.maxFrameSize === 16384 &&
                                   unpacked.maxHeaderListSize === 65535));
          var custom = http2.getPackedSettings({ headerTableSize: 100 });
          out.push('custom-packed:' + (custom[2] === 0 && custom[3] === 0 &&
                                       custom[4] === 0 && custom[5] === 100));
          out.push('unpack-empty-defaults:' +
                   (http2.getUnpackedSettings(null).headerTableSize === 4096));

          // sensitiveHeaders marks a non-enumerable flag and returns the object.
          var hdrs = { 'x-a': '1' };
          var ret = http2.sensitiveHeaders(hdrs);
          var desc = Object.getOwnPropertyDescriptor(hdrs, '_sensitive');
          out.push('sensitive:' + (ret === hdrs && desc.value === true &&
                                   desc.enumerable === false));

          // createServer argument forms.
          var f1 = http2.createServer();
          var f2 = http2.createServer(function () {});
          var f3 = http2.createServer({}, function () {});
          out.push('ctor-forms:' + (f1 instanceof http2.Http2Server &&
                                    f2 instanceof http2.Http2Server &&
                                    f3 instanceof http2.Http2Server));
          out.push('no-handler-form:' + (f1._onStreamHandler === undefined));
          out.push('handler-form:' + (typeof f2._onStreamHandler === 'function'));
          var f1addr = f1.address();
          out.push('pre-listen-addr:' + (f1addr.port === 0 && f1addr.family === 'IPv4' &&
                                         f1addr.address === '0.0.0.0'));
          var srvTimeout = false;
          f1.setTimeout(5, function () { srvTimeout = true; });
          out.push('server-timeout:' + srvTimeout);

          // performance stubs.
          out.push('perf-timerify:' + (http2.performance.timerify(h2_noop) === h2_noop));
          var elu = http2.performance.eventLoopUtilization();
          out.push('perf-elu:' + (elu.idle === 0 && elu.active === 0 && elu.utilization === 0));

          // Constants surface.
          out.push('constants:' + (http2.HTTP2_HEADER_STATUS === 0x01 &&
                                   http2.HTTP2_HEADER_METHOD === 0x02 &&
                                   http2.NGHTTP2_NO_ERROR === 0 &&
                                   http2.NGHTTP2_CANCEL === 8 &&
                                   http2.NGHTTP2_FLAG_END_STREAM === 1 &&
                                   http2.DEFAULT_SETTINGS_HEADER_TABLE_SIZE === 4096 &&
                                   http2.NGHTTP2_STREAM_STATE_CLOSED === 6 &&
                                   http2.NGHTTP2_GOAWAY === 7));

          return out.join('|');
        })()
        "#,
    );

    let failures: Vec<&str> = battery.split('|').filter(|s| !s.contains(":true")).collect();
    assert!(
        failures.is_empty(),
        "pure-JS surface failures: {:?}\nfull: {}",
        failures,
        battery
    );
}

/// onAborted: a client that disconnects mid-body gets the request stream's
/// 'aborted' event, the res 'close' event, and no body completion — the
/// dead-response late-write crash class guard.
#[test]
fn h2_abort_mid_body_notifies_and_marks_dead() {
    let mut ctx = fresh_ctx();
    let port = free_port();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var http2 = require('http2');
            globalThis.__aborted = 0;
            globalThis.__resClose = 0;
            globalThis.__bodyCompleted = undefined;
            globalThis.__resFinish = undefined;
            var srv = http2.createServer(function (req, res) {{
              req.on('aborted', function () {{ globalThis.__aborted++; }});
              req.on('data', function () {{}});
              req.on('end', function () {{
                // Fires only if the body completed — it must not for this test.
                globalThis.__bodyCompleted = true;
              }});
              res.on('close', function () {{ globalThis.__resClose++; }});
              res.on('finish', function () {{ globalThis.__resFinish = true; }});
            }});
            srv.listen({port}, '127.0.0.1');
            'setup-ok'
            "#
        ),
    );
    assert_eq!(setup, "setup-ok");

    // Connect, send a partial POST (declares 64 bytes, sends 8), disconnect.
    {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .write_all(b"POST /cut HTTP/1.1\r\nHost: c4\r\nContent-Length: 64\r\n\r\n12345678")
            .expect("write partial body");
        let _ = stream.flush();
        // Abrupt shutdown — the connection dies before the body ends.
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }

    for _ in 0..100 {
        pump(&mut ctx, 2);
        if eval_str(&mut ctx, "String(globalThis.__aborted)") != "0" {
            break;
        }
    }

    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__aborted)"),
        "1",
        "mid-body disconnect must fire the request 'aborted' event exactly once"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__resClose)"),
        "1",
        "abort must emit 'close' on the response object"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__bodyCompleted)"),
        "undefined",
        "the request body must not complete for a cut-off upload"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__resFinish)"),
        "undefined",
        "an aborted request must never produce a 'finish'"
    );
}

/// Close a stored server reference by global name (`srv` or
/// `globalThis.__h2srv` style handles created in the setup eval).
fn srv_close(ctx: &mut JsContext, global_name: &str) {
    let out = eval_str(
        ctx,
        &format!(
            "(function() {{ var out = []; globalThis.{n}.close(function () {{ out.push('cb'); }}); \
             globalThis.{n}.close(function () {{ out.push('cb2'); }}); return out.join(','); }})()",
            n = global_name
        ),
    );
    assert_eq!(out, "cb,cb2", "server close must fire its callback and be idempotent");
}
