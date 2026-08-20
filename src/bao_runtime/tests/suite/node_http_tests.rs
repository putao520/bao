// @trace TEST-ENG-007-HTTP [req:REQ-ENG-007] [level:integration]
// Integration tests for node:http and node:https API surface (REQ-ENG-007)

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

/// Pump the unified event loop (uWS sockets + timers + jobs + microtasks) —
/// same shape as p0_http_client_ws_tls_tests.
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

/// Raw-wire GET alternating loop pumps with short-timeout reads (the server
/// runs on this same thread — same shape as http_te_parity's raw_roundtrip,
/// but reads to EOF so the full body bytes are captured). Returns
/// (headers, body) with the exact bytes off the socket.
fn raw_get(ctx: &mut JsContext, port: u16, path: &str) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    write!(
        s,
        "GET {} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        path
    )
    .unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    for _ in 0..50 {
        pump(ctx, 2);
        match s.read(&mut chunk) {
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
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header terminator");
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    (head, buf[split + 4..].to_vec())
}

fn content_length(head: &str) -> Option<usize> {
    head.lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
}

#[test]
fn test_node_http_https_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var http = require('http');
        var https = require('https');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === node:http ===
        check("http_require", function() { return typeof http === 'object'; });
        check("http_createServer", function() { return typeof http.createServer === 'function'; });
        check("http_request", function() { return typeof http.request === 'function'; });
        check("http_get", function() { return typeof http.get === 'function'; });
        check("http_STATUS_CODES", function() { return typeof http.STATUS_CODES === 'object'; });
        check("http_STATUS_CODES_200", function() { return http.STATUS_CODES[200] === "OK"; });
        check("http_STATUS_CODES_404", function() { return http.STATUS_CODES[404] === "Not Found"; });
        check("http_STATUS_CODES_500", function() { return http.STATUS_CODES[500] === "Internal Server Error"; });
        check("http_agent", function() { var t = typeof http.Agent; return t === 'function' || t === 'object' || t === 'undefined'; });
        check("http_server_instance", function() {
            var s = http.createServer(function(){});
            return typeof s === 'object' && s !== null;
        });
        check("http_server_listen", function() { return typeof http.createServer(function(){}).listen === 'function'; });
        check("http_server_close", function() { return typeof http.createServer(function(){}).close === 'function'; });
        check("http_server_on", function() {
            var s = http.createServer(function(){});
            return typeof s.on === 'function' || typeof s === 'object';
        });
        check("http_global_agent", function() {
            return typeof http.globalAgent === 'object' || typeof http.globalAgent === 'undefined';
        });
        check("http_maxHeaderSize", function() {
            return typeof http.maxHeaderSize === 'number' || typeof http.maxHeaderSize === 'undefined';
        });

        // === node:https ===
        check("https_require", function() { return typeof https === 'object'; });
        check("https_request", function() { return typeof https.request === 'function'; });
        check("https_get", function() { return typeof https.get === 'function'; });
        check("https_createServer", function() {
            return typeof https.createServer === 'function' || typeof https.createServer === 'undefined';
        });
        check("https_agent", function() {
            return typeof https.Agent === 'function' || typeof https.Agent === 'undefined';
        });
        check("https_globalAgent", function() {
            return typeof https.globalAgent === 'object' || typeof https.globalAgent === 'undefined';
        });

        results.join("|")
    "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "All http/https tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}

// ─── Binary body paths (SILENT eradication regressions) ────────────────
//
// Two silent-corruption classes, both reproduced by probe first:
//   1. server res.end/res.write(Buffer|Uint8Array) hit a string-only branch —
//      every binary body answered Content-Length: 0 with zero wire bytes
//      (pure-ASCII Buffers too). Byte bodies must now hit the wire verbatim,
//      including 0x00 and all 256 byte values.
//   2. client IncomingMessage 'data' chunks were strings decoded via
//      Response#text() — invalid-UTF-8 bytes folded into U+FFFD (a 256-byte
//      all-values body came back corrupted). Node semantics: chunks are
//      Buffers; setEncoding() switches to decoded strings.

/// Server wire exactness: Buffer / Uint8Array / 0x00-bearing / mixed
/// write()+end() / write-after-end bodies on the raw socket.
#[test]
fn test_node_http_server_binary_bodies_wire_exact() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let port_str = eval_string(
        &mut ctx,
        r#"
        var http = require('http');
        var srv = http.createServer(function (q, r) {
            var p = q.url;
            if (p === '/bin-buf') { var b = Buffer.alloc(256); for (var i = 0; i < 256; i++) b[i] = i; r.writeHead(200); r.end(b); }
            else if (p === '/u8') { r.writeHead(200); r.end(new Uint8Array([104, 105, 45, 117, 56])); }
            else if (p === '/zero') { r.writeHead(200); r.end(new Uint8Array([0x61, 0x00, 0x62, 0x00, 0x00, 0x63])); }
            else if (p === '/write-bin') { r.writeHead(200); r.write(new Uint8Array([0xde, 0xad])); r.end(new Uint8Array([0xbe, 0xef])); }
            else if (p === '/afterend') {
                r.writeHead(200); r.end('first');
                globalThis.afterEndThrew = false;
                try { r.write('x'); } catch (e) { globalThis.afterEndThrew = true; }
                r.end('second');
            }
            else { r.writeHead(404); r.end(); }
        });
        srv.listen(0, '127.0.0.1');
        String(srv.address().port)
    "#,
    );
    let port: u16 = port_str.parse().unwrap_or_else(|_| {
        panic!("binary-body server listen(0) failed: {}", port_str)
    });

    let expected_bin: Vec<u8> = (0..=255u8).collect();
    let cases: &[(&str, Vec<u8>)] = &[
        ("/bin-buf", expected_bin),
        ("/u8", vec![104, 105, 45, 117, 56]),
        ("/zero", vec![0x61, 0x00, 0x62, 0x00, 0x00, 0x63]),
        ("/write-bin", vec![0xde, 0xad, 0xbe, 0xef]),
        ("/afterend", b"first".to_vec()),
    ];
    for (path, want) in cases {
        let (head, body) = raw_get(&mut ctx, port, path);
        assert!(head.starts_with("HTTP/1.1 200"), "{} status: {:?}", path, head);
        assert_eq!(
            content_length(&head),
            Some(want.len()),
            "{} Content-Length must equal the byte length (CL:0 was the silent-drop signature)",
            path
        );
        assert_eq!(&body, want, "{} wire bytes must be exact", path);
    }
    // write() after end() must throw (Node ERR_STREAM_WRITE_AFTER_END).
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.afterEndThrew)"),
        "true",
        "res.write() after res.end() must throw, not silently buffer"
    );
    bun_runtime::shutdown_thread_sm();
}

/// Client roundtrip: 'data' chunks are Buffers with the exact 256 wire bytes
/// (all 0-255), and setEncoding() switches delivery to decoded strings.
#[test]
fn test_node_http_client_data_chunks_buffer_byte_exact() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let port_str = eval_string(
        &mut ctx,
        r#"
        var http = require('http');
        var b256 = Buffer.alloc(256); for (var i = 0; i < 256; i++) b256[i] = i;
        var srv = http.createServer(function (q, r) {
            if (q.url === '/bin') { r.writeHead(200); r.end(b256); }
            else { r.writeHead(200); r.end(new Uint8Array([0xde, 0xad, 0xbe, 0xef])); }
        });
        srv.listen(0, '127.0.0.1');
        globalThis.binGot = null; globalThis.encGot = null;
        var port = srv.address().port;
        http.get('http://127.0.0.1:' + port + '/bin', function (res) {
            var chunks = [];
            res.on('data', function (c) { chunks.push(c); });
            res.on('end', function () {
                var all = chunks.length === 1 ? chunks[0] : Buffer.concat(chunks);
                globalThis.binGot = (typeof Buffer !== 'undefined' && Buffer.isBuffer(chunks[0]) ? 'B' : 'S') +
                    ':' + all.length + ':' + all.toString('hex');
            });
        }).on('error', function (e) { globalThis.binGot = 'ERR:' + e.message; });
        http.get('http://127.0.0.1:' + port + '/hex', function (res) {
            res.setEncoding('hex');
            var s = '';
            res.on('data', function (c) { s += c; });
            res.on('end', function () { globalThis.encGot = (typeof s === 'string' ? 'S' : 'B') + ':' + s; });
        }).on('error', function (e) { globalThis.encGot = 'ERR:' + e.message; });
        String(port)
    "#,
    );
    assert!(port_str.parse::<u16>().is_ok(), "listen: {}", port_str);

    let ok = drive_until(&mut ctx, Duration::from_secs(8), |ctx| {
        eval_string(ctx, "globalThis.binGot !== null && globalThis.encGot !== null ? 'y' : 'n'") == "y"
    });
    assert!(ok, "binary client roundtrip never settled");

    let mut expected_hex = String::with_capacity(512);
    for i in 0..=255u8 {
        expected_hex.push_str(&format!("{:02x}", i));
    }
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.binGot)"),
        format!("B:256:{}", expected_hex),
        "'data' chunks must be Buffers holding the exact 256 wire bytes (U+FFFD folding was the corruption signature)"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.encGot)"),
        "S:deadbeef",
        "setEncoding('hex') must deliver a decoded string"
    );
    bun_runtime::shutdown_thread_sm();
}

/// Client REQUEST bodies must be byte-exact on the wire: write(Buffer) +
/// end(Uint8Array), mixed string/binary ordering, and opts.body=Buffer.
/// The previous `String(data)` coercion turned buffers into comma-joined
/// byte lists ("72,101,108"). A raw TcpListener captures the exact request
/// bytes (no bao server involved — the fetch transport is exercised end to
/// end against a real socket).
#[test]
fn test_node_http_client_binary_request_body_wire_exact() {
    use std::sync::{Arc, Mutex};

    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind capture listener");
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);
    std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut s, _) = match listener.accept() {
                Ok(x) => x,
                Err(_) => return,
            };
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            // Read head, then the Content-Length body.
            let (head, body) = loop {
                let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n");
                if let Some(h) = head_end {
                    let head = String::from_utf8_lossy(&buf[..h]).into_owned();
                    let cl = head
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= h + 4 + cl {
                        break (head, buf[h + 4..].to_vec());
                    }
                }
                match s.read(&mut chunk) {
                    Ok(0) => break (
                        String::from_utf8_lossy(&buf).into_owned(),
                        Vec::new(),
                    ),
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        continue;
                    }
                    Err(_) => break (String::new(), Vec::new()),
                }
            };
            cap.lock().unwrap().push((head, body));
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let _ = s.flush();
        }
    });

    let setup = eval_string(
        &mut ctx,
        &format!(
            r#"
        var http = require('http');
        var port = {port};
        var b256 = Buffer.alloc(256); for (var i = 0; i < 256; i++) b256[i] = i;
        globalThis.n1 = null; globalThis.n2 = null; globalThis.n3 = null;
        function fire(path, wire, flag) {{
            var req = http.request({{ host: '127.0.0.1', port: port, method: 'POST', path: path }}, function (res) {{
                res.on('data', function () {{}});
                res.on('end', function () {{ globalThis[flag] = 'done'; }});
            }});
            req.on('error', function (e) {{ globalThis[flag] = 'ERR:' + e.message; }});
            wire(req);
        }}
        fire('/one', function (r) {{
            r.write(b256.slice(0, 128));
            r.end(new Uint8Array(b256.buffer, 128, 128));
        }}, 'n1');
        fire('/two', function (r) {{
            r.write('ab');
            r.write(new Uint8Array([0x00, 0xff]));
            r.end('cd');
        }}, 'n2');
        (function () {{
            var req = http.request(
                {{ host: '127.0.0.1', port: port, method: 'POST', path: '/three', body: b256 }},
                function (res) {{
                    res.on('data', function () {{}});
                    res.on('end', function () {{ globalThis.n3 = 'done'; }});
                }}
            );
            req.on('error', function (e) {{ globalThis.n3 = 'ERR:' + e.message; }});
            req.end();
        }})();
        'setup'
        "#,
            port = port
        ),
    );
    assert_eq!(setup, "setup");

    let ok = drive_until(&mut ctx, Duration::from_secs(8), |ctx| {
        eval_string(
            ctx,
            "globalThis.n1 !== null && globalThis.n2 !== null && globalThis.n3 !== null ? 'y' : 'n'",
        ) == "y"
    });
    assert!(ok, "binary request roundtrips never settled");
    for (flag, label) in [("n1", "one"), ("n2", "two"), ("n3", "three")] {
        assert_eq!(
            eval_string(&mut ctx, &format!("String(globalThis.{})", flag)),
            "done",
            "request {} must settle without error",
            label
        );
    }

    let caps = captured.lock().unwrap();
    assert_eq!(caps.len(), 3, "capture listener saw {} requests", caps.len());
    let expected_all: Vec<u8> = (0..=255u8).collect();
    let expected_strbin: Vec<u8> = vec![0x61, 0x62, 0x00, 0xff, 0x63, 0x64];
    let wants = [
        ("/one", expected_all.clone()),
        ("/two", expected_strbin),
        ("/three", expected_all),
    ];
    for (i, (path, want)) in wants.iter().enumerate() {
        let (head, body) = &caps[i];
        assert!(head.contains("POST"), "{} request line: {:?}", path, head);
        assert_eq!(
            content_length(head),
            Some(want.len()),
            "{} Content-Length must match the byte body",
            path
        );
        assert_eq!(body, want, "{} request wire bytes must be exact", path);
    }
    bun_runtime::shutdown_thread_sm();
}

/// node:http2 client binary roundtrip: `options.body = Buffer` must reach
/// the wire byte-exact (the old bridge coerced via String(body) →
/// "72,101,108" and the native only read string bodies), and the response
/// must come back as a REAL 'response' + Buffer 'data' delivery (the old
/// bridge answered a synchronous statusCode:0 placeholder — every http2
/// client response was a silent fake). Echo listener: captures the request,
/// returns the body bytes verbatim.
#[test]
fn test_node_http2_client_binary_body_roundtrip() {
    use std::sync::{Arc, Mutex};

    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind echo listener");
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);
    std::thread::spawn(move || {
        for _ in 0..1 {
            let (mut s, _) = match listener.accept() {
                Ok(x) => x,
                Err(_) => return,
            };
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            let (head, body) = loop {
                let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n");
                if let Some(h) = head_end {
                    let head = String::from_utf8_lossy(&buf[..h]).into_owned();
                    let cl = head
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= h + 4 + cl {
                        break (head, buf[h + 4..].to_vec());
                    }
                }
                match s.read(&mut chunk) {
                    Ok(0) => break (String::from_utf8_lossy(&buf).into_owned(), Vec::new()),
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        continue;
                    }
                    Err(_) => break (String::new(), Vec::new()),
                }
            };
            cap.lock().unwrap().push((head, body.clone()));
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.write_all(&body);
            let _ = s.flush();
        }
    });

    let setup = eval_string(
        &mut ctx,
        &format!(
            r#"
        var http2 = require('http2');
        var b256 = Buffer.alloc(256); for (var i = 0; i < 256; i++) b256[i] = i;
        globalThis.h2status = null;
        globalThis.h2chunk = null;
        globalThis.h2ended = null;
        globalThis.h2err = null;
        var sess = http2.connect('127.0.0.1:{port}');
        var stream = sess.request(
            {{
                ':method': 'POST',
                ':scheme': 'http',
                ':authority': '127.0.0.1:{port}',
                ':path': '/bin'
            }},
            {{ body: b256 }}
        );
        stream.on('response', function (h) {{ globalThis.h2status = h && h[':status']; }});
        stream.on('data', function (c) {{
            globalThis.h2chunk = (typeof Buffer !== 'undefined' && Buffer.isBuffer(c) ? 'B' : 'S') +
                ':' + c.length + ':' + c.toString('hex');
        }});
        stream.on('end', function () {{ globalThis.h2ended = 'yes'; }});
        stream.on('error', function (e) {{ globalThis.h2err = e && e.message; }});
        'setup'
        "#,
            port = port
        ),
    );
    assert_eq!(setup, "setup");

    let ok = drive_until(&mut ctx, Duration::from_secs(8), |ctx| {
        eval_string(
            ctx,
            "globalThis.h2ended !== null || globalThis.h2err !== null ? 'y' : 'n'",
        ) == "y"
    });
    assert!(ok, "http2 binary roundtrip never settled");

    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.h2err)"),
        "null",
        "http2 stream must not error"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.h2status)"),
        "200",
        "http2 'response' must carry the real status (statusCode:0 placeholder was the fake signature)"
    );
    let mut expected_hex = String::with_capacity(512);
    for i in 0..=255u8 {
        expected_hex.push_str(&format!("{:02x}", i));
    }
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.h2chunk)"),
        format!("B:256:{}", expected_hex),
        "http2 'data' must be one Buffer chunk holding the exact echoed 256 wire bytes"
    );

    let caps = captured.lock().unwrap();
    assert_eq!(caps.len(), 1, "echo listener saw {} requests", caps.len());
    let (head, body) = &caps[0];
    assert!(head.contains("POST"), "request line: {:?}", head);
    assert_eq!(
        content_length(head),
        Some(256),
        "http2 request Content-Length must match the Buffer body"
    );
    let expected_all: Vec<u8> = (0..=255u8).collect();
    assert_eq!(body, &expected_all, "http2 request wire bytes must be exact");
    bun_runtime::shutdown_thread_sm();
}
