// @trace TEST-ENG-013 [req:REQ-ENG-013] [level:integration]
//
// REQ-ENG-013 TLSSocket wrap upgrade locks (C1/C2/C3), each against a real
// STARTTLS fixture: a Rust thread accepts a plaintext TCP connection,
// answers the plaintext greeting, then upgrades the SAME stream to a TLS
// server (bao_boringssl_bridge TlsServer — the exact engine the JS side
// wraps with). The JS side drives `net.connect` + `new tls.TLSSocket(sock,
// options)` through the shared bao-tls-driver (the tls.connect machinery).
//
//   C1  wrap_c1_upgrade_over_existing_conn — the wrap handshake runs on the
//       EXISTING connection (server thread sees one stream: greeting, ack,
//       ClientHello, TLS data) and secureConnect reports a real protocol.
//   C2  wrap_c2_secure_connect_authorized — authorized=true with ca+default
//       rejectUnauthorized; authorized=false with rejectUnauthorized:false
//       (handshake still completes). Also locks the ctor's loud error faces
//       (non-net.Socket object / isServer:true) leaving the socket live.
//   C3  wrap_c3_plaintext_phase_buffered — plaintext-phase RX (paused JS
//       drain) is re-delivered as the FIRST 'data' event BEFORE
//       secureConnect, and a write issued BEFORE the handshake completes is
//       parked and delivered over the TLS channel (server-side proof).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsServer, generate_self_signed_pem};
use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;

fn make_ctx() -> JsContext {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// One pump pass: realm-entered timer drain (dispatches the net poll
/// chain's setTimeout callbacks — pumping timers from bare Rust outside any
/// entered realm silently drops them, net_echo_e2e_tests) + MiniEventLoop
/// tick (ConcurrentTask dispatch for the TLS tasklet) + microjobs. Same
/// combined shape as net_tls_fetch_retention_tests::pump_until_settled —
/// this suite needs BOTH faces (net timers + tls tasklets).
fn pump_once(ctx: &mut JsContext) {
    let cx_raw = ctx.raw_cx();
    {
        let mut cxm = ctx.cx();
        let global = bao_engine::context::thread_realm_global();
        if let Some(g) = global {
            rooted!(&in(cxm) let g_root = g);
            let mut realm = AutoRealm::new_from_handle(&mut cxm, g_root.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            bun_runtime::timers::drain_and_check(realm_cx);
        } else {
            bun_runtime::timers::drain_and_check(&mut cxm);
        }
    }
    bun_runtime::timers::with_event_loop(|loop_| {
        loop_.tick_without_idle(std::ptr::null_mut());
    });
    unsafe {
        mozjs_sys::jsapi::js::RunJobs(cx_raw);
    }
    std::thread::sleep(Duration::from_millis(2));
}

/// Deadline-based pump until `cond` holds (bounded wait, never infinite).
fn pump_until(ctx: &mut JsContext, timeout: Duration, cond: impl Fn(&mut JsContext) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while !cond(ctx) {
        if Instant::now() >= deadline {
            return false;
        }
        pump_once(ctx);
    }
    true
}

/// Evaluate a JS expression that must yield a string.
fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

/// Evaluate a JS expression that must yield a boolean.
fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    matches!(ctx.eval(source, "<test>"), Ok(JsValue::Bool(true)))
}

/// Escape a string for embedding in a JS double-quoted string literal.
fn js_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('"', "\\\"")
}

// ── STARTTLS fixture protocol markers ──────────────────────────────────
const PRE_HELLO: &[u8] = b"PRE-HELLO";
const PLAIN_ACK: &[u8] = b"PLAIN-ACK";
const POST_UPGRADE: &[u8] = b"POST-UPGRADE";
const TLS_ECHO: &[u8] = b"TLS-ECHO";

/// Serve one STARTTLS connection on its accepted stream: read the plaintext
/// greeting, send the plaintext ack, then run the TLS server handshake on
/// the SAME stream, receive POST-UPGRADE over TLS (record it — the C3
/// outgoing proof), and echo TLS-ECHO back.
fn starttls_serve_conn(mut stream: TcpStream, cert: &str, key: &str, got: Arc<Mutex<Vec<String>>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    // 1. plaintext phase: wait for the greeting.
    let mut plain = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !plain.windows(PRE_HELLO.len()).any(|w| w == PRE_HELLO) {
        if Instant::now() >= deadline {
            return;
        }
        let mut buf = [0u8; 4096];
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => plain.extend_from_slice(&buf[..n]),
            Err(_) => continue,
        }
    }
    // 2. plaintext ack (the C3 incoming payload).
    if stream.write_all(PLAIN_ACK).is_err() {
        return;
    }
    // 3. TLS upgrade on the same stream.
    let Ok(server) = TlsServer::new(cert, key) else { return };
    let Ok(mut tls) = server.accept() else { return };
    let mut decrypted = Vec::new();
    let mut saw_upgrade = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Instant::now() >= deadline {
            return;
        }
        match tls.process() {
            Ok(res) => {
                for chunk in &res.plaintext {
                    decrypted.extend_from_slice(chunk);
                }
                if !saw_upgrade && decrypted.windows(POST_UPGRADE.len()).any(|w| w == POST_UPGRADE)
                {
                    saw_upgrade = true;
                    // The parked pre-handshake write arrived decrypted over
                    // the TLS channel — record it.
                    got.lock().unwrap().push("POST-UPGRADE".to_string());
                    let _ = tls.write(TLS_ECHO);
                }
            }
            Err(_) => return,
        }
        let out = tls.take_outgoing();
        if !out.is_empty() && stream.write_all(&out).is_err() {
            return;
        }
        if saw_upgrade && !tls.is_handshaking() {
            return; // echo flushed, connection served
        }
        let mut buf = [0u8; 16 * 1024];
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => tls.feed(&buf[..n]),
            Err(_) => {}
        }
    }
}

/// Spawn the STARTTLS fixture (accept loop + per-connection threads).
/// Returns (port, cert PEM the server serves, received-markers log). The
/// cert MUST be the one the JS side anchors as `ca` — the caller embeds it
/// in the prelude. (A second, unrelated generate call at the call sites was
/// the C1-C3 red root cause: the client anchored a different cert than the
/// one the server presented, so every verified handshake aborted with
/// SSL_ERROR_SSL.)
fn spawn_starttls_server() -> (u16, String, Arc<Mutex<Vec<String>>>) {
    let (cert, key) = generate_self_signed_pem("localhost", 365).expect("fixture cert");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let port = listener.local_addr().unwrap().port();
    let got: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let got_loop = Arc::clone(&got);
    let cert_out = cert.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            let cert = cert.clone();
            let key = key.clone();
            let got = Arc::clone(&got_loop);
            std::thread::spawn(move || starttls_serve_conn(stream, &cert, &key, got));
        }
    });
    (port, cert_out, got)
}

/// Shared JS prelude: result slots + the CA-anchored wrap helper inputs.
/// `__dbg` records every observable step so a stall turns into an
/// assert-visible trace instead of a silent timeout.
fn js_prelude(port: u16, cert_pem: &str) -> String {
    format!(
        r#"
        globalThis.__wrapDone = false;
        globalThis.__wrapRes = "";
        globalThis.__dbg = [];
        globalThis.__port = {port};
        globalThis.__ca = "{ca}";
        globalThis.__note = function (m) {{ globalThis.__dbg.push(m); }};
        "#,
        port = port,
        ca = js_str(cert_pem),
    )
}

// ─── C1: the wrap handshake runs on the EXISTING connection ────────────

#[test]
fn wrap_c1_upgrade_over_existing_conn() {
    let (port, cert, got) = spawn_starttls_server();

    let mut ctx = make_ctx();
    let _ = eval_string(&mut ctx, &js_prelude(port, &cert));
    // Happy path: greeting → plain ack read → wrap → handshake on the same
    // conn → parked write → TLS echo.
    let _ = eval_string(
        &mut ctx,
        r#"
        var net = require('net'), tls = require('tls');
        var sock = net.connect(globalThis.__port, '127.0.0.1');
        __note('connected');
        sock.on('data', function (chunk) {
            __note('plain-data');
            if (String(chunk).indexOf('PLAIN-ACK') === -1) return;
            try {
                var t = new tls.TLSSocket(sock, {
                    servername: 'localhost',
                    ca: globalThis.__ca,
                });
                __note('wrapped');
                t.on('error', function (e) { __note('tls-error:' + (e && e.message || e)); });
                t.on('close', function () { __note('closed'); });
                t.on('secureConnect', function () { __note('secureConnect'); t.write('POST-UPGRADE'); });
                t.on('data', function (c) {
                    // TLSSocket 'data' delivers ArrayBuffer payloads (native
                    // tls_bytes_to_array_buffer) — decode before matching.
                    if (Buffer.from(c).toString().indexOf('TLS-ECHO') !== -1) {
                        globalThis.__wrapRes = t.getProtocol() || '';
                        globalThis.__wrapDone = true;
                    }
                });
            } catch (e) {
                __note('ctor-throw:' + (e.message || e));
                globalThis.__wrapDone = true;
            }
        });
        sock.write('PRE-HELLO');
        "#,
    );
    let done = pump_until(&mut ctx, Duration::from_secs(10), |c| {
        eval_bool(c, "globalThis.__wrapDone")
    });
    let dbg = eval_string(&mut ctx, "globalThis.__dbg.join(' > ')");
    assert!(done, "C1: upgrade never completed (trace: {:?})", dbg);
    // secureConnect fired with a real negotiated protocol (C1+C2 face).
    let protocol = eval_string(&mut ctx, "globalThis.__wrapRes");
    assert!(
        protocol.starts_with("TLS"),
        "C1: no negotiated protocol, got {:?} (trace: {:?})",
        protocol,
        dbg
    );
    // Server-side proof: the SAME accepted stream carried the TLS-decrypted
    // parked write — the handshake really ran over the existing conn.
    let received = got.lock().unwrap().clone();
    assert!(
        received.iter().any(|m| m == "POST-UPGRADE"),
        "C1: fixture never received the TLS-channel payload (got {:?}, trace: {:?})",
        received,
        dbg
    );
}

// ─── C2: secureConnect + authorized observable ──────────────────────────

#[test]
fn wrap_c2_secure_connect_authorized() {
    let (port, cert, _got) = spawn_starttls_server();

    let mut ctx = make_ctx();
    let _ = eval_string(&mut ctx, &js_prelude(port, &cert));
    let _ = eval_string(
        &mut ctx,
        r#"
        var net = require('net'), tls = require('tls');

        // Loud error faces (must throw, must not disturb a live socket).
        try { new tls.TLSSocket({ notASocket: true }); __note('NO_THROW_OBJ'); }
        catch (e) { __note('threw-nonsocket'); }

        function upgrade(rejectFlag) {
            var sock = net.connect(globalThis.__port, '127.0.0.1');
            sock.on('data', function (chunk) {
                if (String(chunk).indexOf('PLAIN-ACK') === -1) return;
                // isServer:true must throw BEFORE touching the socket, and
                // the socket must stay usable for the real wrap below.
                try {
                    new tls.TLSSocket(sock, { isServer: true });
                    __note('NO_THROW_ISSERVER');
                } catch (e) { __note('threw-isserver'); }
                try {
                    var t = new tls.TLSSocket(sock, {
                        servername: 'localhost',
                        ca: globalThis.__ca,
                        rejectUnauthorized: rejectFlag,
                    });
                    t.on('error', function (e) { __note('tls-error:' + (e && e.message || e)); });
                    t.on('close', function () { __note('closed'); });
                    t.on('secureConnect', function () {
                        t.write('POST-UPGRADE');
                        globalThis.__res += (globalThis.__res ? ';' : '') +
                            'authorized=' + t.authorized + ',proto=' + (t.getProtocol() || 'none');
                        t.destroy();
                        globalThis.__done += 1;
                    });
                } catch (e) { __note('ctor-throw:' + (e.message || e)); }
            });
            sock.write('PRE-HELLO');
        }
        globalThis.__res = '';
        globalThis.__done = 0;
        upgrade(true);
        upgrade(false);
        "#,
    );
    let done = pump_until(&mut ctx, Duration::from_secs(10), |c| {
        eval_bool(c, "globalThis.__done >= 2")
    });
    let dbg = eval_string(&mut ctx, "globalThis.__dbg.join(' > ')");
    assert!(done, "C2: both upgrades never completed (trace: {:?})", dbg);
    assert!(
        dbg.contains("threw-nonsocket"),
        "C2: wrapping a non-socket object must throw (trace: {:?})",
        dbg
    );
    assert!(
        dbg.contains("threw-isserver") && !dbg.contains("NO_THROW_ISSERVER"),
        "C2: isServer:true must throw (trace: {:?})",
        dbg
    );
    let res = eval_string(&mut ctx, "globalThis.__res");
    // Two secureConnect observations: verified (ca + default reject) is
    // authorized=true; rejectUnauthorized:false is authorized=false with
    // the handshake still completing.
    let mut verified = false;
    let mut unverified = false;
    for part in res.split(';') {
        if part.contains("authorized=true") && part.contains("proto=TLS") {
            verified = true;
        }
        if part.contains("authorized=false") && part.contains("proto=TLS") {
            unverified = true;
        }
    }
    assert!(
        verified,
        "C2: ca+default-reject wrap must observe authorized=true + protocol (got {:?}, trace: {:?})",
        res, dbg
    );
    assert!(
        unverified,
        "C2: rejectUnauthorized:false wrap must observe authorized=false + protocol (got {:?}, trace: {:?})",
        res, dbg
    );
}

// ─── C3: upgrade-window plaintext buffering, both directions ────────────

#[test]
fn wrap_c3_plaintext_phase_buffered() {
    let (port, cert, got) = spawn_starttls_server();

    let mut ctx = make_ctx();
    let _ = eval_string(&mut ctx, &js_prelude(port, &cert));
    // The JS never drains the plain socket (paused poll chain): PLAIN-ACK
    // stays in the native RX buffer until the ctor's takeover drains and
    // re-delivers it through the TLSSocket. The write BEFORE handshake
    // completion exercises the parked-write path.
    let _ = eval_string(
        &mut ctx,
        r#"
        var net = require('net'), tls = require('tls');
        var sock = net.connect(globalThis.__port, '127.0.0.1');
        sock.pause(); // halt the JS drain chain — the upgrade window model
        sock.write('PRE-HELLO');
        setTimeout(function () {
            var order = [];
            try {
                var t = new tls.TLSSocket(sock, {
                    servername: 'localhost',
                    ca: globalThis.__ca,
                });
                __note('wrapped');
                t.on('error', function (e) { __note('tls-error:' + (e && e.message || e)); });
                t.on('close', function () { __note('closed'); });
                t.write('POST-UPGRADE'); // pre-handshake write: must park
                t.on('data', function (c) {
                    if (Buffer.from(c).toString().indexOf('PLAIN-ACK') !== -1) { __note('plain-redelivered'); order.push('plain'); }
                });
                t.on('secureConnect', function () { __note('secureConnect'); order.push('secure'); });
                t.on('data', function (c) {
                    if (Buffer.from(c).toString().indexOf('TLS-ECHO') !== -1) {
                        globalThis.__order = order.join(',');
                        globalThis.__wrapDone = true;
                    }
                });
            } catch (e) {
                __note('ctor-throw:' + (e.message || e));
                globalThis.__wrapDone = true;
            }
        }, 250);
        "#,
    );
    let done = pump_until(&mut ctx, Duration::from_secs(10), |c| {
        eval_bool(c, "globalThis.__wrapDone")
    });
    let dbg = eval_string(&mut ctx, "globalThis.__dbg.join(' > ')");
    assert!(done, "C3: buffered-upgrade flow never completed (trace: {:?})", dbg);
    let order = eval_string(&mut ctx, "globalThis.__order");
    // The plaintext-phase payload is re-delivered BEFORE secureConnect
    // (FIFO through the same event channel — nothing lost, order kept).
    assert_eq!(
        order, "plain,secure",
        "C3: PLAIN-ACK must precede secureConnect, got {:?} (trace: {:?})",
        order, dbg
    );
    // Server-side proof: the parked pre-handshake write traveled the TLS
    // channel (the fixture only records it after decrypting it).
    let received = got.lock().unwrap().clone();
    assert!(
        received.iter().any(|m| m == "POST-UPGRADE"),
        "C3: parked write never arrived over the TLS channel (got {:?}, trace: {:?})",
        received,
        dbg
    );
}
