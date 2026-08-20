// @trace TEST-E2E-HTTP-CLIENT [req:REQ-ENG-006,REQ-ENG-007,REQ-ENG-010] [level:system]
// @trace REQ-ENG-006 [level:system]
// @trace REQ-ENG-007 [level:system]
// @trace REQ-ENG-010 [level:system]
// P0 regression tests (v-surface audit wave): the Node CLIENT forms of
// http/https/tls and the node:http WebSocket-upgrade crash class.
//
//   1. http.get(url, cb) — the Node callback form must deliver the real
//      response (callback dropped = the silent-death class; the server saw
//      the request, the client never saw the answer).
//   2. http.request(...) + req.on('response') — ClientRequest face, the
//      transport fires on .end() (nothing before).
//   3. node:http + WebSocket upgrade request — an http.Server with no
//      'upgrade' handler must answer 426 Upgrade Required, not abort()
//      (uWS "returning without responding" std::terminate → SIGSEGV).
//   4. tls.connect(opts, cb) — Node callback form: returns a synchronous
//      TLSSocket, cb fires on secureConnect, .on('data')/.write() do real
//      I/O; `.then` (legacy promise shape) still settles with the socket.
//   5. tls.connect to a refused port — the promise rejects / 'error' fires
//      (previously: silent hang forever).
//   6. https.get error path — a failed request surfaces a real error
//      (previously: JSON.parse("[object Promise]") → statusCode 0 fake).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use bao_boringssl_bridge::generate_self_signed_pem;
use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<p0-client-test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

/// Pump the unified event loop (uWS sockets + timers + jobs + microtasks).
fn pump(ctx: &mut JsContext, passes: usize) {
    for _ in 0..passes {
        let cx_raw = ctx.raw_cx();
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        let mut cxm = ctx.cx();
        bun_runtime::timers::drain_and_check(&mut cxm);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn drive_until(ctx: &mut JsContext, timeout: Duration, cond: impl Fn(&mut JsContext) -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !cond(ctx) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        pump(ctx, 5);
    }
    true
}

// ─── 1. http.get(url, cb) Node callback roundtrip ──────────────────────

#[test]
fn http_get_callback_form_real_roundtrip() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let port_str = eval_str(
        &mut ctx,
        r#"
        var http = require('http');
        globalThis.got = null;
        globalThis.srvHits = 0;
        var srv = http.createServer(function (req, res) {
            globalThis.srvHits++;
            res.writeHead(200, { 'x-probe': 'yes' });
            res.end('http-srv-body');
        });
        srv.listen(0, '127.0.0.1');
        var port = srv.address().port;
        var req = http.get('http://127.0.0.1:' + port + '/x', function (r) {
            var b = '';
            r.on('data', function (c) { b += c; });
            r.on('end', function () {
                globalThis.got = r.statusCode + '|' + r.headers['x-probe'] + '|' + b;
            });
        });
        req.on('error', function (e) { globalThis.got = 'ERR:' + e.message; });
        String(port)
    "#,
    );
    let port: u16 = port_str.parse().unwrap_or_else(|_| {
        panic!("http server listen(0) failed: {}", port_str)
    });

    let ok = drive_until(&mut ctx, Duration::from_secs(8), |ctx| {
        eval_str(ctx, "globalThis.got !== null ? 'y' : 'n'") == "y"
    });
    assert!(ok, "http.get callback form never settled (silent death class)");
    let got = eval_str(&mut ctx, "String(globalThis.got)");
    assert_eq!(got, "200|yes|http-srv-body", "callback must deliver the real response");
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.srvHits)"),
        "1",
        "exactly one server hit"
    );
}

// ─── 2. http.request ClientRequest face, transport fires on .end() ─────

#[test]
fn http_request_clientrequest_face_fires_on_end() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let port_str = eval_str(
        &mut ctx,
        r#"
        var http = require('http');
        globalThis.result = null;
        globalThis.srvHits = 0;
        var srv = http.createServer(function (req, res) {
            globalThis.srvHits++;
            res.writeHead(200, { 'x-echo-method': req.method });
            res.end('done');
        });
        srv.listen(0, '127.0.0.1');
        var port = srv.address().port;
        var req = http.request(
            { host: '127.0.0.1', port: port, method: 'POST', path: '/post', headers: { 'x-t': '1' } },
            function (r) {
                var b = '';
                r.on('data', function (c) { b += c; });
                r.on('end', function () { globalThis.result = r.statusCode + ':' + r.headers['x-echo-method'] + ':' + b; });
            }
        );
        req.on('error', function (e) { globalThis.result = 'ERR:' + e.message; });
        // ClientRequest contract: nothing is sent before .end().
        globalThis.hitsBeforeEnd = globalThis.srvHits;
        req.write('pay');
        req.end('load');
        String(port)
    "#,
    );
    assert!(port_str.parse::<u16>().is_ok(), "listen: {}", port_str);

    let ok = drive_until(&mut ctx, Duration::from_secs(8), |ctx| {
        eval_str(ctx, "globalThis.result !== null ? 'y' : 'n'") == "y"
    });
    assert!(ok, "http.request POST never settled");
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.hitsBeforeEnd)"),
        "0",
        "transport must not fire before .end()"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.result)"),
        "200:POST:done",
        "POST request must round-trip through the real transport"
    );
}

// ─── 3. node:http + WS upgrade → 426, no abort/SIGSEGV ─────────────────

#[test]
fn node_http_ws_upgrade_answered_426_not_abort() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let port_str = eval_str(
        &mut ctx,
        r#"
        var http = require('http');
        globalThis.srvHits = 0;
        var srv = http.createServer(function (req, res) {
            globalThis.srvHits++;
            res.end('plain');
        });
        srv.listen(0, '127.0.0.1');
        String(srv.address().port)
    "#,
    );
    let port: u16 = port_str.parse().unwrap_or_else(|_| {
        panic!("listen(0) failed: {}", port_str)
    });

    // Real WebSocket upgrade request (the same shape a browser WS client
    // sends). The server has no 'upgrade' handler: pre-fix this hit uWS's
    // "returning from a request handler without responding" std::terminate
    // → abort() → SIGSEGV (process death, handler never entered).
    let req = format!(
        "GET /path HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("read timeout");
    stream.write_all(req.as_bytes()).expect("write upgrade request");

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    for _ in 0..50 {
        pump(&mut ctx, 2);
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
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
    let resp = String::from_utf8_lossy(&buf).into_owned();
    assert!(
        resp.starts_with("HTTP/1.1 426"),
        "WS upgrade must be answered 426 Upgrade Required, got: {:?}",
        resp.split("\r\n").next().unwrap_or("")
    );
    // And the server survives: the plain request handler still answers.
    let plain = "GET /plain HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    let mut s2 = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect 2");
    s2.set_read_timeout(Some(Duration::from_millis(200))).expect("timeout");
    s2.write_all(plain.as_bytes()).expect("write plain");
    let mut b2: Vec<u8> = Vec::new();
    for _ in 0..50 {
        pump(&mut ctx, 2);
        match s2.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                b2.extend_from_slice(&chunk[..n]);
                if b2.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
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
    let resp2 = String::from_utf8_lossy(&b2).into_owned();
    assert!(
        resp2.starts_with("HTTP/1.1 200") && resp2.ends_with("plain"),
        "server must still serve plain HTTP after a refused upgrade; got: {:?}",
        resp2.split("\r\n").next().unwrap_or("")
    );
}

// ─── 4. tls.connect(opts, cb) callback form + promise compat ───────────

#[test]
fn tls_connect_callback_form_and_promise_compatible() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (cert, key) = generate_self_signed_pem("server.local", 365).expect("cert");

    let mut ctx = make_ctx();
    let setup = format!(
        r#"
        var tls = require('tls');
        globalThis.port = 0;
        globalThis.cbFired = 'no';
        globalThis.gotData = '';
        globalThis.promiseResult = 'pending';
        globalThis.hasOn = 'unknown';
        var server = tls.createServer({{
            key: "{key}",
            cert: "{cert}"
        }});
        server.on('secureConnection', function (s) {{
            s.on('data', function (d) {{ s.write('echo:' + String.fromCharCode.apply(null, new Uint8Array(d))); }});
        }});
        server.listen(0, '127.0.0.1', function () {{ globalThis.port = server.address().port; }});
        'setup'
        "#,
        key = key.replace('\\', "\\\\").replace('\n', "\\n").replace('"', "\\\""),
        cert = cert.replace('\\', "\\\\").replace('\n', "\\n").replace('"', "\\\""),
    );
    assert_eq!(eval_str(&mut ctx, &setup), "setup", "server setup");

    assert!(
        drive_until(&mut ctx, Duration::from_secs(2), |ctx| {
            eval_str(ctx, "String(globalThis.port > 0)") == "true"
        }),
        "tls server listen callback must fire"
    );
    let port: u16 = eval_str(&mut ctx, "String(globalThis.port)").parse().expect("port");

    // ONE eval: Node callback form + synchronous-socket contract + promise
    // compat on the SAME connect call.
    let connect = format!(
        r#"
        var c = tls.connect({{ host: '127.0.0.1', port: {port}, rejectUnauthorized: false }}, function () {{
            globalThis.cbFired = 'yes';
        }});
        // Node shape: the return value is a socket with EE methods NOW
        // (pre-fix: a Promise with no .on → TypeError, silently swallowed).
        globalThis.hasOn = typeof c.on === 'function' ? 'yes' : 'no';
        c.on('data', function (d) {{
            globalThis.gotData = String.fromCharCode.apply(null, new Uint8Array(d));
        }});
        c.on('error', function (e) {{ globalThis.gotData = 'ERR:' + e.message; }});
        // Legacy promise shape on the same socket.
        c.then(function (sock) {{
            globalThis.promiseResult = sock === c ? 'resolved-self' : 'resolved-other';
            sock.write('tls-ping');
        }}, function (e) {{
            globalThis.promiseResult = 'rejected:' + e.message;
        }});
        'connecting'
        "#,
        port = port,
    );
    assert_eq!(eval_str(&mut ctx, &connect), "connecting", "connect eval");

    assert!(
        drive_until(&mut ctx, Duration::from_secs(8), |ctx| {
            eval_str(ctx, "globalThis.gotData !== '' ? 'y' : 'n'") == "y"
        }),
        "tls callback-form connect never delivered echo data"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.hasOn)"),
        "yes",
        "tls.connect must return a synchronous TLSSocket (Node shape)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.cbFired)"),
        "yes",
        "secureConnect callback must fire"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.promiseResult)"),
        "resolved-self",
        "legacy .then must settle with the same socket object"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.gotData)"),
        "echo:tls-ping",
        "echo must flow over the real TLS session"
    );
}

// ─── 5. tls.connect refused port → rejects (no silent hang) ────────────

#[test]
fn tls_connect_refused_rejects_with_error() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        globalThis.outcome = 'pending';
        globalThis.errEvt = 'none';
        // Port 9 (discard) on loopback: nothing listens → connect refused.
        var c = tls_connect_probe();
        function tls_connect_probe() {
            var tls = require('tls');
            return tls.connect({ host: '127.0.0.1', port: 9, rejectUnauthorized: false }, function () {
                globalThis.outcome = 'connected(unexpected)';
            });
        }
        c.on('error', function (e) { globalThis.errEvt = String(e && e.message).substring(0, 40); });
        c.then(function () { globalThis.outcome = 'resolved(unexpected)'; },
               function (e) { globalThis.outcome = 'rejected'; });
        'issued'
        "#,
    );
    assert_eq!(r, "issued", "connect eval");

    let ok = drive_until(&mut ctx, Duration::from_secs(6), |ctx| {
        eval_str(ctx, "globalThis.outcome !== 'pending' ? 'y' : 'n'") == "y"
    });
    assert!(ok, "tls.connect to a refused port must settle (was: silent hang forever)");
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.outcome)"),
        "rejected",
        "refused connect must reject the promise"
    );
    let err = eval_str(&mut ctx, "String(globalThis.errEvt)");
    assert!(
        err.starts_with("tls:"),
        "socket 'error' must fire with the real failure message, got: {}",
        err
    );
}

// ─── 6. https.get: real verification error + rejectUnauthorized:false ───
//
// Against a LIVE self-signed TLS server: default verification must surface
// a real TLS error (pre-fix: the shim JSON.parsed a Promise object and
// delivered a fake statusCode-0 callback), and rejectUnauthorized:false
// must ride the real transport to a 200.

#[test]
fn https_get_real_error_and_optout_roundtrip() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (cert, key) = generate_self_signed_pem("server.local", 365).expect("cert");
    let mut ctx = make_ctx();
    let setup = format!(
        r#"
        var tls = require('tls');
        var https = require('https');
        globalThis.port = 0;
        globalThis.strictResult = 'pending';
        globalThis.optoutResult = 'pending';
        var server = tls.createServer({{
            key: "{key}",
            cert: "{cert}"
        }}, function (s) {{
            s.on('data', function (d) {{
                s.write('HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi');
                s.end();
            }});
        }});
        server.listen(0, '127.0.0.1', function () {{ globalThis.port = server.address().port; }});
        'setup'
        "#,
        key = key.replace('\\', "\\\\").replace('\n', "\\n").replace('"', "\\\""),
        cert = cert.replace('\\', "\\\\").replace('\n', "\\n").replace('"', "\\\""),
    );
    assert_eq!(eval_str(&mut ctx, &setup), "setup", "tls server setup");
    assert!(
        drive_until(&mut ctx, Duration::from_secs(2), |ctx| {
            eval_str(ctx, "String(globalThis.port > 0)") == "true"
        }),
        "tls server listen callback must fire"
    );
    let port: u16 = eval_str(&mut ctx, "String(globalThis.port)").parse().expect("port");

    let client = format!(
        r#"
        var req = https.get('https://127.0.0.1:{port}/x', function (res) {{
            globalThis.strictResult = 'FAKE-CALLBACK:' + res.statusCode;
        }});
        req.on('error', function (e) {{ globalThis.strictResult = 'real-error'; }});
        var req2 = https.get(
            {{ hostname: '127.0.0.1', port: {port}, path: '/', rejectUnauthorized: false }},
            function (res) {{
                var b = '';
                res.on('data', function (c) {{ b += c; }});
                res.on('end', function () {{ globalThis.optoutResult = 'ok:' + res.statusCode + ':' + b; }});
            }}
        );
        req2.on('error', function (e) {{ globalThis.optoutResult = 'ERR:' + (e && e.message); }});
        'issued'
        "#,
        port = port,
    );
    assert_eq!(eval_str(&mut ctx, &client), "issued", "client eval");

    let ok = drive_until(&mut ctx, Duration::from_secs(10), |ctx| {
        eval_str(
            ctx,
            "(globalThis.strictResult !== 'pending' && globalThis.optoutResult !== 'pending') ? 'y' : 'n'",
        ) == "y"
    });
    assert!(ok, "both https.get calls must settle");
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.strictResult)"),
        "real-error",
        "untrusted cert must surface a real error (not a statusCode-0 fake callback)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.optoutResult)"),
        "ok:200:hi",
        "rejectUnauthorized:false must round-trip over the real TLS transport"
    );
}
