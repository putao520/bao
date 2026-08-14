// @trace TEST-ENG-007-SNI [req:REQ-ENG-007] [level:integration]
//
// Integration tests for the node:tls server SNICallback root-cure:
//   1. sni_two_domains — SNICallback dispatch selects the cert per SNI name
//   2. static_cert_regression — no SNICallback → static cert still serves
//   3. write_from_sni_callback_first — a socket.write() issued from inside
//      the SNICallback is parked (ssl_in_use semantics) and hits the wire
//      BEFORE anything written from the secureConnection listener
//   4. sni_callback_error — cb(err) fails the handshake loudly
//      (client handshake error + tlsClientError event)
//
// The TLS server runs on the bao-tls-driver thread; JS callbacks (the user
// SNICallback, event emissions) are dispatched as ConcurrentTasks on the JS
// thread's MiniEventLoop, which the test pumps via drive_event_loop (same
// harness as fetch_e2e_tests.rs).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bao_boringssl_bridge::{TlsClient, TlsConnection, generate_self_signed_pem, pem_parse_certs};
use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Drive the JS thread's MiniEventLoop for a few iterations (ConcurrentTask
/// dispatch), yielding between iterations.
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

/// Escape a PEM string for embedding in a JS double-quoted string literal.
fn js_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('"', "\\\"")
}

/// Result of a client TLS session: the peer's leaf cert DER + all decrypted
/// application data received (in order) until the stream idled.
struct ClientOutcome {
    peer_cert_der: Vec<u8>,
    plaintext: Vec<u8>,
}

/// Minimal blocking TLS client against the local server, sending SNI
/// `servername`. Drives the memory-BIO TlsConnection over a TcpStream.
/// After the handshake, keeps reading until `want_bytes` appears in the
/// decrypted stream (or a 2s idle timeout) when `want_bytes` is given.
fn tls_client_session(
    port: u16,
    servername: &str,
    want_bytes: Option<&[u8]>,
    trusted_ders: &[Vec<u8>],
) -> Result<ClientOutcome, String> {
    let client = TlsClient::new().map_err(|e| format!("TlsClient::new: {}", e))?;
    // The BoringSSL client verifies by default — anchor the test certs.
    for der in trusted_ders {
        if !client.add_trusted_der(der) {
            return Err("add_trusted_der failed".to_string());
        }
    }
    let mut conn = TlsConnection::new_client(&client, servername)
        .map_err(|e| format!("new_client: {}", e))?;
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {}", e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("set_read_timeout: {}", e))?;

    let mut plaintext = Vec::new();
    loop {
        let res = conn.process().map_err(|e| format!("process: {}", e))?;
        let out = conn.take_outgoing();
        if !out.is_empty() {
            stream.write_all(&out).map_err(|e| format!("write: {}", e))?;
        }
        for chunk in &res.plaintext {
            plaintext.extend_from_slice(chunk);
        }
        if !conn.is_handshaking() {
            let peer = conn
                .peer_certificate_der()
                .ok_or_else(|| "no peer certificate".to_string())?;
            // Post-handshake application-data phase: short idle timeout so
            // the session returns promptly once the expected marker (or
            // nothing more) arrives.
            stream
                .set_read_timeout(Some(Duration::from_millis(300)))
                .map_err(|e| format!("set_read_timeout(2): {}", e))?;
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if let Some(want) = want_bytes {
                    if plaintext.windows(want.len()).any(|w| w == want) {
                        break;
                    }
                }
                let mut buf = [0u8; 16 * 1024];
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        conn.feed(&buf[..n]);
                        let res = conn.process().map_err(|e| format!("post-process: {}", e))?;
                        let out = conn.take_outgoing();
                        if !out.is_empty() {
                            let _ = stream.write_all(&out);
                        }
                        for chunk in &res.plaintext {
                            plaintext.extend_from_slice(chunk);
                        }
                    }
                    Err(_) => {
                        if want_bytes.is_none() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
            return Ok(ClientOutcome {
                peer_cert_der: peer,
                plaintext,
            });
        }
        // Still handshaking: read more ciphertext.
        let mut buf = [0u8; 16 * 1024];
        match stream.read(&mut buf) {
            Ok(0) => return Err("EOF during handshake".to_string()),
            Ok(n) => conn.feed(&buf[..n]),
            Err(e) => return Err(format!("read: {}", e)),
        }
    }
}

/// Run a client session on a worker thread while the test (JS) thread pumps
/// the event loop. Returns the session result.
fn run_client_with_pump(
    ctx: &mut JsContext,
    port: u16,
    servername: &str,
    want_bytes: Option<&[u8]>,
    trusted_ders: &[Vec<u8>],
) -> Result<ClientOutcome, String> {
    let done = Arc::new(AtomicBool::new(false));
    let result: Arc<Mutex<Option<Result<ClientOutcome, String>>>> = Arc::new(Mutex::new(None));
    let servername_owned = servername.to_string();
    let want_owned = want_bytes.map(|w| w.to_vec());
    let trusted_owned = trusted_ders.to_vec();
    {
        let done = Arc::clone(&done);
        let result = Arc::clone(&result);
        std::thread::spawn(move || {
            let r = tls_client_session(
                port,
                &servername_owned,
                want_owned.as_deref(),
                &trusted_owned,
            );
            *result.lock().unwrap() = Some(r);
            done.store(true, Ordering::Release);
        });
    }
    while !done.load(Ordering::Acquire) {
        drive_event_loop(ctx, 5);
    }
    // Extra pump so the JS side settles (secureConnection listeners etc).
    drive_event_loop(ctx, 5);
    result.lock().unwrap().take().expect("client result")
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<tls-sni-test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

// ─── 1. SNICallback selects the certificate per SNI name ───────────────

#[test]
fn sni_two_domains_select_cert_per_name() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (foo_cert, foo_key) = generate_self_signed_pem("foo.com", 365).expect("foo cert");
    let (bar_cert, bar_key) = generate_self_signed_pem("bar.com", 365).expect("bar cert");
    let (default_cert, default_key) =
        generate_self_signed_pem("default.local", 365).expect("default cert");
    let foo_der = pem_parse_certs(&foo_cert).into_iter().next().expect("foo DER");
    let bar_der = pem_parse_certs(&bar_cert).into_iter().next().expect("bar DER");

    let mut ctx = make_ctx();
    let setup = format!(
        r#"
        var tls = require('tls');
        globalThis.port = 0;
        globalThis.secureCount = 0;
        globalThis.sniSeen = [];
        var server = tls.createServer({{
            key: "{default_key}",
            cert: "{default_cert}",
            SNICallback: function(servername, cb) {{
                globalThis.sniSeen.push(servername);
                if (servername === 'foo.com') cb(null, {{ key: "{foo_key}", cert: "{foo_cert}" }});
                else if (servername === 'bar.com') cb(null, {{ key: "{bar_key}", cert: "{bar_cert}" }});
                else cb(new Error('unknown servername ' + servername));
            }}
        }});
        server.on('secureConnection', function(s) {{ globalThis.secureCount++; }});
        server.listen(0, '127.0.0.1', function() {{
            globalThis.port = server.address().port;
        }});
        "ok"
        "#,
        default_key = js_str(&default_key),
        default_cert = js_str(&default_cert),
        foo_key = js_str(&foo_key),
        foo_cert = js_str(&foo_cert),
        bar_key = js_str(&bar_key),
        bar_cert = js_str(&bar_cert),
    );
    let r = eval_string(&mut ctx, &setup);
    assert_eq!(r, "ok", "server setup eval failed: {}", r);
    drive_event_loop(&mut ctx, 3);
    let port = match ctx.eval("globalThis.port;", "<p>") {
        Ok(JsValue::Number(n)) => n as u16,
        _ => panic!("port not captured after listen"),
    };
    assert!(port > 0, "listen(0) must bind an ephemeral port");

    // foo.com → foo cert.
    let trusted = vec![foo_der.clone(), bar_der.clone()];
    let out = run_client_with_pump(&mut ctx, port, "foo.com", None, &trusted)
        .expect("foo.com handshake must succeed");
    assert_eq!(
        out.peer_cert_der, foo_der,
        "SNI foo.com must serve the foo.com certificate"
    );

    // bar.com → bar cert (same listener, one IP, two certificates).
    let out = run_client_with_pump(&mut ctx, port, "bar.com", None, &trusted)
        .expect("bar.com handshake must succeed");
    assert_eq!(
        out.peer_cert_der, bar_der,
        "SNI bar.com must serve the bar.com certificate"
    );

    // The JS SNICallback really ran, once per connection, with the wire name.
    let seen = eval_string(&mut ctx, "globalThis.sniSeen.join(',')");
    assert_eq!(seen, "foo.com,bar.com", "SNICallback dispatch log");
    let secure = eval_string(&mut ctx, "String(globalThis.secureCount)");
    assert_eq!(secure, "2", "secureConnection must fire per connection");

    let close = eval_string(&mut ctx, "server.close(function(){ globalThis.closed = true; }); 'closing'");
    assert_eq!(close, "closing");
    drive_event_loop(&mut ctx, 10);
    let closed = eval_string(&mut ctx, "String(globalThis.closed === true)");
    assert_eq!(closed, "true", "close() callback must run after teardown");
}

// ─── 2. No SNICallback → static certificate (default contract branch) ──

#[test]
fn static_cert_regression_without_sni_callback() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (cert, key) = generate_self_signed_pem("static.local", 365).expect("cert");
    let static_der = pem_parse_certs(&cert).into_iter().next().expect("DER");

    let mut ctx = make_ctx();
    let setup = format!(
        r#"
        var tls = require('tls');
        globalThis.port = 0;
        globalThis.secureCount = 0;
        var server = tls.createServer({{
            key: "{key}",
            cert: "{cert}"
        }});
        server.on('secureConnection', function(s) {{ globalThis.secureCount++; }});
        server.listen(0, '127.0.0.1', function() {{ globalThis.port = server.address().port; }});
        "ok"
        "#,
        key = js_str(&key),
        cert = js_str(&cert),
    );
    let r = eval_string(&mut ctx, &setup);
    assert_eq!(r, "ok", "static server setup failed: {}", r);
    drive_event_loop(&mut ctx, 3);
    let port = match ctx.eval("globalThis.port;", "<p>") {
        Ok(JsValue::Number(n)) => n as u16,
        _ => panic!("port not captured after listen"),
    };
    assert!(port > 0);

    // Even WITH an SNI extension in the ClientHello, the static cert serves.
    let out = run_client_with_pump(&mut ctx, port, "anything.example", None, &[static_der.clone()])
        .expect("static-cert handshake must succeed");
    assert_eq!(
        out.peer_cert_der, static_der,
        "no SNICallback → static certificate for every connection"
    );
    let secure = eval_string(&mut ctx, "String(globalThis.secureCount)");
    assert_eq!(secure, "1", "secureConnection must fire");

    eval_string(&mut ctx, "server.close();");
    drive_event_loop(&mut ctx, 5);
}

// ─── 3. write() inside SNICallback: parked, delivered first ─────────────

#[test]
fn write_from_sni_callback_delivered_before_secure_connection_writes() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (foo_cert, foo_key) = generate_self_signed_pem("foo.com", 365).expect("cert");
    let (def_cert, def_key) = generate_self_signed_pem("default.local", 365).expect("default");

    let mut ctx = make_ctx();
    let setup = format!(
        r#"
        var tls = require('tls');
        globalThis.port = 0;
        globalThis.connSock = null;
        var server = tls.createServer({{
            key: "{def_key}",
            cert: "{def_cert}",
            SNICallback: function(servername, cb) {{
                // Write issued from INSIDE the SNICallback, while the
                // handshake is mid-flight (the ssl_in_use trigger).
                globalThis.connSock.write("from-callback;");
                cb(null, {{ key: "{foo_key}", cert: "{foo_cert}" }});
            }}
        }});
        server.on('connection', function(s) {{ globalThis.connSock = s; }});
        server.on('secureConnection', function(s) {{ s.write("from-secure;"); }});
        server.listen(0, '127.0.0.1', function() {{ globalThis.port = server.address().port; }});
        "ok"
        "#,
        def_key = js_str(&def_key),
        def_cert = js_str(&def_cert),
        foo_key = js_str(&foo_key),
        foo_cert = js_str(&foo_cert),
    );
    let r = eval_string(&mut ctx, &setup);
    assert_eq!(r, "ok", "setup failed: {}", r);
    drive_event_loop(&mut ctx, 3);
    let port = match ctx.eval("globalThis.port;", "<p>") {
        Ok(JsValue::Number(n)) => n as u16,
        _ => panic!("port not captured after listen"),
    };
    assert!(port > 0);

    let foo_der = pem_parse_certs(&foo_cert).into_iter().next().expect("foo DER");
    let out = run_client_with_pump(&mut ctx, port, "foo.com", Some(b"from-secure;"), &[foo_der])
        .expect("handshake with in-callback write must succeed");
    assert!(
        out.plaintext.starts_with(b"from-callback;"),
        "parked callback write must be delivered right after the handshake, got: {:?}",
        String::from_utf8_lossy(&out.plaintext)
    );
    assert!(
        out.plaintext.windows(b"from-secure;".len()).any(|w| w == b"from-secure;"),
        "secureConnection write must follow the parked callback write, got: {:?}",
        String::from_utf8_lossy(&out.plaintext)
    );
    let cb_first = out
        .plaintext
        .windows(b"from-callback;".len())
        .position(|w| w == b"from-callback;")
        .unwrap_or(usize::MAX);
    let secure_pos = out
        .plaintext
        .windows(b"from-secure;".len())
        .position(|w| w == b"from-secure;")
        .unwrap_or(usize::MAX);
    assert!(
        cb_first < secure_pos,
        "callback write must precede secureConnection write on the wire"
    );

    eval_string(&mut ctx, "server.close();");
    drive_event_loop(&mut ctx, 5);
}

// ─── 4. cb(err) fails the handshake loudly ─────────────────────────────

#[test]
fn sni_callback_error_fails_handshake_with_tls_client_error() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (cert, key) = generate_self_signed_pem("default.local", 365).expect("cert");

    let mut ctx = make_ctx();
    let setup = format!(
        r#"
        var tls = require('tls');
        globalThis.port = 0;
        globalThis.clientErrors = [];
        var server = tls.createServer({{
            key: "{key}",
            cert: "{cert}",
            SNICallback: function(servername, cb) {{
                cb(new Error('no certificate configured for ' + servername));
            }}
        }});
        server.on('tlsClientError', function(err) {{
            globalThis.clientErrors.push(String(err && err.message || err));
        }});
        server.listen(0, '127.0.0.1', function() {{ globalThis.port = server.address().port; }});
        "ok"
        "#,
        key = js_str(&key),
        cert = js_str(&cert),
    );
    let r = eval_string(&mut ctx, &setup);
    assert_eq!(r, "ok", "setup failed: {}", r);
    drive_event_loop(&mut ctx, 3);
    let port = match ctx.eval("globalThis.port;", "<p>") {
        Ok(JsValue::Number(n)) => n as u16,
        _ => panic!("port not captured after listen"),
    };
    assert!(port > 0);

    let result = run_client_with_pump(&mut ctx, port, "foo.com", None, &[]);
    assert!(
        result.is_err(),
        "client handshake must fail when SNICallback errors (got {:?})",
        result.map(|o| o.plaintext)
    );

    let errors = eval_string(&mut ctx, "globalThis.clientErrors.join('|')");
    assert!(
        errors.contains("no certificate configured"),
        "tlsClientError must surface the SNICallback error message, got: {}",
        errors
    );

    eval_string(&mut ctx, "server.close();");
    drive_event_loop(&mut ctx, 5);
}
