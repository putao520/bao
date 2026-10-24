// @trace TEST-ENG-7-PEMFORMS [req:REQ-ENG-007] [level:integration]
//
// node:tls PEM input-shape conformance (root cure 2026-10-16 — Windows
// red-ledger "tls.createServer().listen() callback never fires"):
//
// node documents `cert`/`key` (and `ca`) as `string | Buffer | string[] |
// Buffer[]`. The original implementation read `is_string()` only, so the
// canonical real-world shape — `fs.readFileSync()` returning a
// Buffer/Uint8Array — was silently dropped and `listen()` failed its
// cert/key precondition ("called without cert/key") BEFORE the listening
// callback ever ran. Platform-independent (repro'd on Linux with the same
// probe; the Windows red entry was this same gap observed on the .200
// machine).
//
// Faces (each drives a REAL handshake via the blocking TlsClient harness,
// same shape as tls_sni_server_tests.rs):
//   1. buffer_cert_key_listen_fires        — Buffer.from(pem) for both fields
//   2. uint8array_cert_key_listen_fires    — TextEncoder bytes (readFileSync shape)
//   3. cert_chain_array_serves_leaf        — cert: [leaf, extra] chain semantics
//   4. secure_context_setters_accept_bytes — setKey/setCert/setCA byte views

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bao_boringssl_bridge::{TlsClient, TlsConnection, generate_self_signed_pem, pem_parse_certs};
use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
#[path = "common/mod.rs"]
mod common;

use common::drive_event_loop;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    common::eval_string_full_named(ctx, source, "<tls-pem-forms-test>")
}

fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Deadline-based pump: drive the JS thread's MiniEventLoop until `cond`
/// holds or `timeout` elapses (bounded wait, never infinite).
fn drive_event_loop_until(
    ctx: &mut JsContext,
    timeout: Duration,
    cond: impl Fn(&mut JsContext) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !cond(ctx) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        drive_event_loop(ctx, 5);
    }
    true
}

/// Escape a PEM string for embedding in a JS double-quoted string literal.
fn js_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('"', "\\\"")
}

struct ClientOutcome {
    peer_cert_der: Vec<u8>,
}

/// Minimal blocking TLS client: handshake against the local server and
/// return the peer's leaf cert DER. (Same shape as tls_sni_server_tests.rs.)
fn tls_client_session(
    port: u16,
    servername: &str,
    trusted_ders: &[Vec<u8>],
) -> Result<ClientOutcome, String> {
    let client = TlsClient::new().map_err(|e| format!("TlsClient::new: {}", e))?;
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

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while conn.is_handshaking() {
        if std::time::Instant::now() >= deadline {
            return Err("handshake deadline".to_string());
        }
        let res = conn.process().map_err(|e| format!("process: {}", e))?;
        let out = conn.take_outgoing();
        if !out.is_empty() {
            stream.write_all(&out).map_err(|e| format!("write: {}", e))?;
        }
        let _ = &res.plaintext;
        if !conn.is_handshaking() {
            break;
        }
        let mut buf = [0u8; 16384];
        match stream.read(&mut buf) {
            Ok(0) => return Err("server closed during handshake".to_string()),
            Ok(n) => conn.feed(&buf[..n]),
            Err(e) => return Err(format!("read: {}", e)),
        }
    }
    let peer = conn
        .peer_certificate_der()
        .ok_or_else(|| "no peer certificate".to_string())?;
    Ok(ClientOutcome { peer_cert_der: peer })
}

/// Client on its own thread; JS thread pumped until `settled` holds.
fn run_client_with_pump(
    ctx: &mut JsContext,
    port: u16,
    servername: &str,
    trusted_ders: &[Vec<u8>],
    settled: impl Fn(&mut JsContext) -> bool,
) -> Result<ClientOutcome, String> {
    let done = Arc::new(AtomicBool::new(false));
    let result: Arc<Mutex<Option<Result<ClientOutcome, String>>>> = Arc::new(Mutex::new(None));
    let servername_owned = servername.to_string();
    let trusted_owned = trusted_ders.to_vec();
    {
        let done = Arc::clone(&done);
        let result = Arc::clone(&result);
        std::thread::spawn(move || {
            let r = tls_client_session(port, &servername_owned, &trusted_owned);
            *result.lock().unwrap() = Some(r);
            done.store(true, Ordering::Release);
        });
    }
    while !done.load(Ordering::Acquire) {
        drive_event_loop(ctx, 5);
    }
    drive_event_loop_until(ctx, Duration::from_secs(2), settled);
    result.lock().unwrap().take().expect("client result")
}

/// Start a tls.createServer() with the caller's options literal and return
/// the bound port. Fails the test if the listening callback never fires —
/// the red-ledger symptom this suite pins.
fn setup_server(ctx: &mut JsContext, opts_literal: &str) -> u16 {
    let setup = format!(
        r#"
        var tls = require('tls');
        globalThis.pemPort = 0;
        globalThis.pemServed = 0;
        var server = tls.createServer({opts});
        server.on('secureConnection', function(s) {{ globalThis.pemServed++; }});
        server.listen(0, '127.0.0.1', function() {{ globalThis.pemPort = server.address().port; }});
        "ok"
        "#,
        opts = opts_literal,
    );
    let r = eval_string(ctx, &setup);
    assert_eq!(r, "ok", "server setup failed: {}", r);
    assert!(
        drive_event_loop_until(ctx, Duration::from_secs(3), |ctx| {
            eval_string(ctx, "String(globalThis.pemPort > 0)") == "true"
        }),
        "listen callback must fire (cert/key option shape rejected?)"
    );
    match ctx.eval("globalThis.pemPort;", "<p>") {
        Ok(JsValue::Number(n)) => n as u16,
        other => panic!("port not captured after listen: {:?}", other),
    }
}

// ─── 1. Buffer.from(pem) — node's documented first-class Buffer shape ────

#[test]
fn buffer_cert_key_listen_fires() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (cert, key) = generate_self_signed_pem("buffer.local", 365).expect("fixture cert");
    let cert_der = pem_parse_certs(&cert).into_iter().next().expect("DER");

    let mut ctx = make_ctx();
    let port = setup_server(
        &mut ctx,
        &format!(
            r#"{{ key: Buffer.from("{}"), cert: Buffer.from("{}") }}"#,
            js_str(&key),
            js_str(&cert),
        ),
    );
    assert!(port > 0);

    let out = run_client_with_pump(
        &mut ctx,
        port,
        "buffer.local",
        &[cert_der.clone()],
        |ctx| eval_string(ctx, "String(globalThis.pemServed || '0')") == "1",
    )
    .expect("handshake with Buffer-form cert/key must succeed");
    assert_eq!(out.peer_cert_der, cert_der, "Buffer-form cert must be the served leaf");
    assert_eq!(eval_string(&mut ctx, "server.close(); \"closed\""), "closed");
    drive_event_loop(&mut ctx, 5);
}

// ─── 2. Uint8Array (fs.readFileSync shape on bao) ─────────────────────────

#[test]
fn uint8array_cert_key_listen_fires() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (cert, key) = generate_self_signed_pem("u8.local", 365).expect("fixture cert");
    let cert_der = pem_parse_certs(&cert).into_iter().next().expect("DER");

    let mut ctx = make_ctx();
    let port = setup_server(
        &mut ctx,
        &format!(
            r#"{{ key: new TextEncoder().encode("{}"), cert: new TextEncoder().encode("{}") }}"#,
            js_str(&key),
            js_str(&cert),
        ),
    );
    assert!(port > 0);

    let out = run_client_with_pump(
        &mut ctx,
        port,
        "u8.local",
        &[cert_der.clone()],
        |ctx| eval_string(ctx, "String(globalThis.pemServed || '0')") == "1",
    )
    .expect("handshake with Uint8Array-form cert/key must succeed");
    assert_eq!(out.peer_cert_der, cert_der);
    assert_eq!(eval_string(&mut ctx, "server.close(); \"closed\""), "closed");
    drive_event_loop(&mut ctx, 5);
}

// ─── 3. cert: [leaf, extra] — array is a chain, not just its last entry ──

#[test]
fn cert_chain_array_serves_leaf() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (leaf, key) = generate_self_signed_pem("chain.local", 365).expect("leaf");
    let (extra, _) = generate_self_signed_pem("extra.local", 365).expect("extra");
    let leaf_der = pem_parse_certs(&leaf).into_iter().next().expect("leaf DER");

    let mut ctx = make_ctx();
    let port = setup_server(
        &mut ctx,
        &format!(
            r#"{{ key: Buffer.from("{}"), cert: [Buffer.from("{}"), Buffer.from("{}")] }}"#,
            js_str(&key),
            js_str(&leaf),
            js_str(&extra),
        ),
    );
    assert!(port > 0);

    let out = run_client_with_pump(
        &mut ctx,
        port,
        "chain.local",
        &[leaf_der.clone()],
        |ctx| eval_string(ctx, "String(globalThis.pemServed || '0')") == "1",
    )
    .expect("handshake with chain-array cert must succeed");
    assert_eq!(
        out.peer_cert_der, leaf_der,
        "cert: [leaf, extra] — the FIRST entry is the served leaf (chain order)"
    );
    assert_eq!(eval_string(&mut ctx, "server.close(); \"closed\""), "closed");
    drive_event_loop(&mut ctx, 5);
}

// ─── 4. SecureContext setters accept byte views ───────────────────────────

#[test]
fn secure_context_setters_accept_byte_views() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();

    let (cert, key) = generate_self_signed_pem("sc.local", 365).expect("fixture cert");
    let cert_der = pem_parse_certs(&cert).into_iter().next().expect("DER");

    let mut ctx = make_ctx();
    let setup = format!(
        r#"
        var tls = require('tls');
        globalThis.scPort = 0;
        globalThis.pemServed = 0;
        var sc = tls.createSecureContext();
        sc.setKey(Buffer.from("{key}"));
        sc.setCert(new TextEncoder().encode("{cert}"));
        var server = tls.createServer(sc);
        server.on('secureConnection', function(s) {{ globalThis.pemServed++; }});
        server.listen(0, '127.0.0.1', function() {{ globalThis.scPort = server.address().port; }});
        "ok"
        "#,
        key = js_str(&key),
        cert = js_str(&cert),
    );
    let r = eval_string(&mut ctx, &setup);
    assert_eq!(r, "ok", "sc setup failed: {}", r);
    assert!(
        drive_event_loop_until(&mut ctx, Duration::from_secs(3), |ctx| {
            eval_string(ctx, "String(globalThis.scPort > 0)") == "true"
        }),
        "listen callback must fire with SecureContext byte-view setters"
    );
    let port = match ctx.eval("globalThis.scPort;", "<p>") {
        Ok(JsValue::Number(n)) => n as u16,
        other => panic!("port not captured: {:?}", other),
    };

    let out = run_client_with_pump(
        &mut ctx,
        port,
        "sc.local",
        &[cert_der.clone()],
        |ctx| eval_string(ctx, "String(globalThis.pemServed || '0')") == "1",
    )
    .expect("handshake with SecureContext byte-view material must succeed");
    assert_eq!(out.peer_cert_der, cert_der);
    assert_eq!(eval_string(&mut ctx, "server.close(); \"closed\""), "closed");
    drive_event_loop(&mut ctx, 5);
}
