// @trace TEST-ENG-007-HTTP-TE-PARITY [req:REQ-ENG-007] [level:integration]
// Behavioral parity split for HTTP/1.0 + Transfer-Encoding (RFC 9112 6.1,
// upstream Bun bdb738222): Bun.serve/Bun.listen reject the request outright
// with 400 INVALID_TRANSFER_ENCODING; node:http keeps Node llhttp semantics
// (dispatch the request, close the connection after — the HTTP/1.0 request
// already marks the connection close via isAncient). Both paths share the
// same uWS HttpParser; the split is carried by HttpFlags::isNodeHttp, set
// only by node_http::server_listen.

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

/// Pump the unified event loop (uWS sockets + timers + jobs) a few passes so
/// the in-process server accepts, parses and responds on our thread.
fn pump(ctx: &mut JsContext, passes: usize) {
    for _ in 0..passes {
        let mut cxm = ctx.cx();
        bun_runtime::timers::drain_and_check(&mut cxm);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Send a raw request over a real TCP connection and collect the response,
/// alternating loop pumps with short-timeout reads (server and client share
/// this thread). Returns once the response head is complete or the peer
/// closes / budget is exhausted.
fn raw_roundtrip(ctx: &mut JsContext, port: u16, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("read timeout");
    stream.write_all(request).expect("write request");

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    for _ in 0..50 {
        pump(ctx, 2);
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.starts_with(b"HTTP/") && buf.windows(4).any(|w| w == b"\r\n\r\n") {
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
    String::from_utf8_lossy(&buf).into_owned()
}

/// POST with `Transfer-Encoding: chunked` on an HTTP/1.0 request line — the
/// smuggling shape rejected by RFC 9112 6.1.
const SMUGGLED_10_TE: &[u8] =
    b"POST /a HTTP/1.0\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";

/// Like `raw_roundtrip`, but keeps reading past the response head until the
/// peer closes or the budget is exhausted — used where the response BODY is
/// the assertion target (real JS handler output vs default echo).
fn raw_roundtrip_full(ctx: &mut JsContext, port: u16, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("read timeout");
    stream.write_all(request).expect("write request");

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    for _ in 0..50 {
        pump(ctx, 2);
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// node:http `listen(0)` must surface the OS-assigned ephemeral port through
/// `address()` (mirrors Bun.serve BCE-005 `actual_port`). This is the
/// runnable half of the node:http parity story: without it the ignored
/// dispatch test below cannot even address the server.
#[test]
fn test_node_http_listen_ephemeral_port() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let port_str = eval_str(
        &mut ctx,
        r#"
        var http = require('http');
        var srv = http.createServer(function(req, res) {});
        srv.listen(0, '127.0.0.1');
        var addr = srv.address();
        (addr && typeof addr.port === 'number' && addr.port > 0) ? String(addr.port) : 'noaddr'
    "#,
    );
    let port: u16 = port_str.parse().unwrap_or_else(|_| {
        panic!("node:http listen(0) did not report the bound ephemeral port: {}", port_str)
    });

    // The reported port must be a real listening socket.
    let stream = TcpStream::connect_timeout(
        &::std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(1000),
    );
    assert!(stream.is_ok(), "connect to reported port {} refused", port);
    // No srv.close(): JsContext::eval is realm-per-call, so the setup eval's
    // `srv` is not reachable from a later eval. The server dies with the test
    // binary (same pattern as bug353 T7 leaving Bun.serve running).
}

/// node:http keeps llhttp framing semantics: an HTTP/1.0 request bearing
/// Transfer-Encoding is PARSED AND ROUTED (no 400) — the isNodeHttp=false
/// rejection never fires on this path.
///
/// Dispatch-after-eval is REAL here: the request is pumped through
/// `drain_and_check` AFTER the setup eval returned (its realm is popped),
/// so this exercises the persistent-rooted GcStore + AutoRealm dispatch
/// path end-to-end (BCE: handler used to be unresolvable via
/// CurrentGlobalOrNull → route handler returned without responding →
/// uWS std::terminate).
#[test]
fn test_node_http_dispatches_http10_transfer_encoding_llhttp_parity() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let port_str = eval_str(
        &mut ctx,
        r#"
        var http = require('http');
        var srv = http.createServer(function(req, res) {
            res.statusCode = 200;
            res.end('ok');
        });
        srv.listen(0, '127.0.0.1');
        var addr = srv.address();
        (addr && typeof addr.port === 'number' && addr.port > 0) ? String(addr.port) : 'noaddr'
    "#,
    );
    let port: u16 = port_str.parse().unwrap_or_else(|_| {
        panic!("node:http listen(0) did not report the bound ephemeral port: {}", port_str)
    });

    let response = raw_roundtrip_full(&mut ctx, port, SMUGGLED_10_TE);
    assert!(
        !response.is_empty(),
        "node:http server produced no response for HTTP/1.0+TE"
    );
    assert!(
        !response.starts_with("HTTP/1.1 400") && !response.starts_with("HTTP/1.0 400"),
        "node:http must keep llhttp parity (dispatch, not 400); got: {:?}",
        response.split("\r\n").next().unwrap_or("")
    );
    assert!(
        response.starts_with("HTTP/"),
        "node:http response missing status line: {:?}",
        response
    );
    // The JS handler's body is the handler-hit proof: dispatch happened
    // after the eval returned (realm popped), so 'ok' can only come from the
    // real `res.end('ok')` call inside the registered JS handler.
    assert!(
        response.ends_with("ok") || response.contains("\r\n\r\nok"),
        "node:http JS handler must have run and ended with 'ok' (dispatch-after-eval); got: {:?}",
        response
    );
}

#[test]
fn test_bun_serve_rejects_http10_transfer_encoding() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let port_str = eval_str(
        &mut ctx,
        r#"
        var s = Bun.serve({
            port: 0,
            fetch: function(req) { return new Response('hello'); }
        });
        String(s.port)
    "#,
    );
    assert!(port_str.parse::<u16>().is_ok(), "Bun.serve port: {}", port_str);
    let port: u16 = port_str.parse().unwrap();

    let response = raw_roundtrip(&mut ctx, port, SMUGGLED_10_TE);
    assert!(
        response.starts_with("HTTP/1.1 400"),
        "Bun.serve must reject HTTP/1.0+TE with 400 (RFC 9112 6.1); got: {:?}",
        response.split("\r\n").next().unwrap_or("")
    );

    // Handler never invoked: JsContext::eval creates a fresh realm per call,
    // so the setup eval's JS state is not readable afterwards. The 400 status
    // line is the authoritative signal — a routed request always completes as
    // 200 (worst case via Bun.serve's default-response fallback), so a 400 can
    // only come from the parser rejecting before routing.
}

/// Control: the same chunked request on HTTP/1.1 is valid framing and must
/// flow through Bun.serve untouched (guards against an over-broad guard).
///
/// The body assertion carries the dispatch-after-eval verdict: the request
/// is pumped via `drain_and_check` AFTER the setup eval returned (realm
/// popped), so a 200 with the handler's real body ("hello") proves the
/// persistent-rooted GcStore + AutoRealm dispatch reached the JS fetch
/// handler — the default echo (`{"method":...}`) is now reserved for
/// servers with NO registered handler and must not appear here.
#[test]
fn test_bun_serve_accepts_http11_chunked_control() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let port_str = eval_str(
        &mut ctx,
        r#"
        var s = Bun.serve({
            port: 0,
            fetch: function(req) { return new Response('hello'); }
        });
        String(s.port)
    "#,
    );
    let port: u16 = port_str.parse().unwrap_or_else(|_| {
        panic!("Bun.serve did not report the bound ephemeral port: {}", port_str)
    });

    let response = raw_roundtrip_full(
        &mut ctx,
        port,
        b"POST /a HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    );
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "HTTP/1.1 + chunked is valid framing and must be served; got: {:?}",
        response.split("\r\n").next().unwrap_or("")
    );
    assert!(
        response.ends_with("hello") || response.contains("\r\n\r\nhello"),
        "real JS fetch handler must have produced the body 'hello' (dispatch-after-eval); got: {:?}",
        response
    );
    assert!(
        !response.contains("\"method\""),
        "default echo response must not impersonate the JS handler; got: {:?}",
        response
    );
}
