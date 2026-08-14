// @trace TEST-ENG-006-WS-ASYNC [req:REQ-ENG-006] [req:REQ-STL-001] [level:integration]
// Async page-WebSocket root fix: `new WebSocket(..)` must NOT connect on the
// JS thread (background worker + drain pump) and the wss:// handshake must
// apply the thread's StealthProfile (same application path as fetch()).
//
// Coverage:
//   1. ws:// echo round-trip: constructor returns immediately, onopen fires
//      from the drain pump, send→onmessage("ECHO:hello"), close→onclose.
//   2. Constructor non-blocking proof: a server that accepts TCP but never
//      completes the WS handshake leaves readyState===0 while a 10ms timer
//      still fires on the JS thread (the old code blocked the JS/Script
//      thread inside the constructor for the full handshake window).
//   3. wss:// echo round-trip against a real BoringSSL TLS server with a
//      self-signed cert. The client handshake runs the full stealth path
//      (`stealth_profile_to_ssl_config` → `configure_http_client_with_alpn`:
//      Firefox cipher list / TLS1.3 suites / curves / sigalgs + ALPN), so a
//      green test proves the stealth-configured ClientHello is accepted by a
//      real TLS peer — functional stealth-path evidence.
//   4. Connect failure surfaces explicitly: refused port → onerror (with the
//      reason) + onclose, readyState===3. Never a silent swallow.

use bao_boringssl_bridge::connection::{TlsConnection, TlsState};
use bao_boringssl_bridge::server::TlsServer;
use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_uws::ws_codec::{apply_mask, FrameDecoder, FrameEncoder};
use bun_uws::ws_handshake::server_handshake;
use mozjs::rooted;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
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

/// Pump the event loop (timers + jobs + the WebSocket drain pump) until
/// `probe` (a JS expression) evaluates truthy, or the budget runs out.
/// Enters the thread's persistent realm first — the CLI eval loop invokes
/// its drain hook inside the realm, and timer callbacks resolve through
/// CurrentGlobalOrNull.
fn pump_until(ctx: &mut JsContext, probe: &str, budget_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(budget_ms);
    while std::time::Instant::now() < deadline {
        let mut cxm = ctx.cx();
        let global = bao_engine::context::thread_realm_global();
        if let Some(g) = global {
            rooted!(&in(cxm) let g_root = g);
            let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            bun_runtime::timers::drain_and_check(realm_cx);
        } else {
            bun_runtime::timers::drain_and_check(&mut cxm);
        }
        if eval_str(ctx, &format!("Boolean({})", probe)) == "true" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

// ── Test servers ──────────────────────────────────────────────────────────

/// Encode a server→client text frame (no mask, server side).
fn encode_server_text(payload: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(payload.len() + 2);
    buf.push(0x81); // FIN + Text
    buf.push(payload.len() as u8); // server frames are unmasked
    buf.extend_from_slice(payload.as_bytes());
    buf
}

fn encode_server_close() -> Vec<u8> {
    vec![0x88, 0x02, 0x03, 0xE8] // FIN + Close, code 1000
}

/// Serve one plain ws:// connection: handshake, echo text frames as
/// "ECHO:<text>", reply to close frames.
fn serve_plain_connection(mut stream: TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    if server_handshake(&mut stream).is_err() {
        eprintln!("[ws-server] handshake failed");
        return;
    }
    let _ = stream.write_all(&encode_server_text("OPEN"));
    let _ = stream.flush();
    let mut decoder = FrameDecoder::new();
    loop {
        let header = match decoder.decode_frame(&mut stream) {
            Ok(Some(h)) => h,
            Ok(None) => {
                eprintln!("[ws-server] eof");
                return;
            }
            Err(e) => {
                eprintln!("[ws-server] decode error: {:?}", e.kind());
                return;
            }
        };
        let payload = if header.mask {
            let mask = decoder.take_mask();
            let mut p = decoder.take_payload(&header);
            apply_mask(&mut p, &mask);
            p
        } else {
            decoder.take_payload(&header)
        };
        match header.opcode {
            bun_uws::ws_codec::Opcode::Text => {
                let text = String::from_utf8_lossy(&payload).into_owned();
                let _ = stream.write_all(&encode_server_text(&format!("ECHO:{}", text)));
                let _ = stream.flush();
            }
            bun_uws::ws_codec::Opcode::Close => {
                let _ = stream.write_all(&encode_server_close());
                let _ = stream.flush();
                return;
            }
            _ => {
                eprintln!("[ws-server] other opcode: {:?}", header.opcode);
            }
        }
    }
}

/// TLS stream adapter for the test server: drives the server-side
/// BoringSSL state machine over the raw TCP socket (mirror of the client's
/// private `TlsStream` in web_api.rs).
struct ServerTlsIo {
    tcp: TcpStream,
    tls: TlsConnection,
    /// Decrypted plaintext not yet consumed (byte-at-a-time readers — see
    /// the client-side BCE-20260814-WS-TLS note).
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl ServerTlsIo {
    fn handshake(tcp: &mut TcpStream, tls: &mut TlsConnection) -> std::io::Result<()> {
        loop {
            let res = tls
                .process()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            // Flush the flight (ServerHello/cert/Finished) BEFORE blocking
            // on read — same ordering contract as the client loop.
            loop {
                let outgoing = tls.take_outgoing();
                if outgoing.is_empty() {
                    break;
                }
                tcp.write_all(&outgoing)?;
            }
            if res.state == TlsState::Active || res.state == TlsState::PeerClosed {
                return Ok(());
            }
            let mut buf = [0u8; 16_384];
            match tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "peer closed during tls handshake",
                    ))
                }
                Ok(n) => tls.feed(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    }

    fn read_plaintext(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            let outgoing = self.tls.take_outgoing();
            if !outgoing.is_empty() {
                self.tcp.write_all(&outgoing)?;
            }
            let res = self
                .tls
                .process()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            if !res.plaintext.is_empty() {
                let mut joined = Vec::new();
                for chunk in res.plaintext {
                    joined.extend_from_slice(&chunk);
                }
                return Ok(joined);
            }
            let mut buf = [0u8; 16_384];
            match self.tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "tls peer closed",
                    ))
                }
                Ok(n) => self.tls.feed(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    }
}

impl Read for ServerTlsIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending_off >= self.pending_plain.len() {
            self.pending_plain = self.read_plaintext()?;
            self.pending_off = 0;
        }
        let avail = &self.pending_plain[self.pending_off..];
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.pending_off += n;
        Ok(n)
    }
}

impl Write for ServerTlsIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self
            .tls
            .write(buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let outgoing = self.tls.take_outgoing();
        if !outgoing.is_empty() {
            self.tcp.write_all(&outgoing)?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

/// Serve one wss:// connection: TLS handshake (self-signed), WS handshake,
/// echo text frames, reply to close frames. Runs the full server-side TLS
/// state machine — the client's stealth-configured ClientHello must be
/// acceptable to this peer for the test to pass.
fn serve_tls_connection(mut tcp: TcpStream, server: &TlsServer) {
    tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut tls = match server.accept() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[wss-server] accept failed: {}", e);
            return;
        }
    };
    if let Err(e) = ServerTlsIo::handshake(&mut tcp, &mut tls) {
        eprintln!("[wss-server] tls handshake failed: {:?}", e);
        return;
    }
    let mut io = ServerTlsIo {
        tcp,
        tls,
        pending_plain: Vec::new(),
        pending_off: 0,
    };
    if server_handshake(&mut io).is_err() {
        return;
    }
    let _ = io.write_all(&encode_server_text("OPEN"));
    let _ = io.flush();
    let mut decoder = FrameDecoder::new();
    loop {
        let header = match decoder.decode_frame(&mut io) {
            Ok(Some(h)) => h,
            _ => return,
        };
        let payload = if header.mask {
            let mask = decoder.take_mask();
            let mut p = decoder.take_payload(&header);
            apply_mask(&mut p, &mask);
            p
        } else {
            decoder.take_payload(&header)
        };
        match header.opcode {
            bun_uws::ws_codec::Opcode::Text => {
                let text = String::from_utf8_lossy(&payload).into_owned();
                let _ = io.write_all(&encode_server_text(&format!("ECHO:{}", text)));
                let _ = io.flush();
            }
            bun_uws::ws_codec::Opcode::Close => {
                let mut enc = FrameEncoder::new();
                let _ = io.write_all(enc.encode_close(1000, ""));
                let _ = io.flush();
                return;
            }
            _ => {}
        }
    }
}

fn spawn_plain_ws_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => serve_plain_connection(s),
                Err(_) => return,
            }
        }
    });
    port
}

fn spawn_tls_ws_server() -> u16 {
    let (cert, key) =
        bao_boringssl_bridge::generate_self_signed_pem("localhost", 365).expect("self-signed cert");
    let server = std::sync::Arc::new(TlsServer::new(&cert, &key).expect("TlsServer"));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => serve_tls_connection(s, &server),
                Err(_) => return,
            }
        }
    });
    port
}

/// Accept one TCP connection and never speak (the non-blocking proof server).
fn spawn_silent_tcp_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            // Hold the connection open (accepted, no WS handshake) so the
            // client stays CONNECTING. Drop after 8s to bound the test.
            std::thread::sleep(Duration::from_secs(8));
            drop(stream);
        }
    });
    port
}

// ── Tests ─────────────────────────────────────────────────────────────────

fn new_test_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// ws:// echo round-trip through the async model: constructor returns with
/// readyState 0, the drain pump delivers onopen/onmessage/onclose.
#[test]
fn ws_async_open_echo_close_roundtrip() {
    let port = spawn_plain_ws_server();
    let mut ctx = new_test_ctx();

    let setup = format!(
        r#"
        var wsLog = [];
        var ws = new WebSocket("ws://127.0.0.1:{}/test");
        wsLog.push("ctor:" + ws.readyState);
        ws.onopen = function() {{ wsLog.push("open:" + ws.readyState); ws.send("hello"); }};
        ws.onmessage = function(ev) {{ wsLog.push("msg:" + ev.data); ws.close(); }};
        ws.onerror = function(ev) {{ wsLog.push("error:" + (ev.data || "?")); }};
        ws.onclose = function() {{ wsLog.push("close:" + ws.readyState); }};
        "done"
        "#,
        port
    );
    assert_eq!(
        eval_str(&mut ctx, &setup),
        "done",
        "constructor eval failed"
    );

    // Constructor must be non-instantly CONNECTING (not yet open).
    assert_eq!(
        eval_str(&mut ctx, "wsLog[0]"),
        "ctor:0",
        "constructor must return while CONNECTING (readyState 0), got: {}",
        eval_str(&mut ctx, "wsLog.join(',')")
    );

    let opened = pump_until(
        &mut ctx,
        "wsLog.some(function(l){{return l==='close:3'}})",
        8_000,
    );
    let log = eval_str(&mut ctx, "wsLog.join('|')");
    assert!(
        opened,
        "WS round-trip did not finish in budget; log: {}",
        log
    );
    assert!(
        log.contains("ctor:0"),
        "readyState must be 0 right after constructor: {}",
        log
    );
    assert!(
        log.contains("open:1"),
        "onopen must fire with readyState 1: {}",
        log
    );
    assert!(
        log.contains("msg:ECHO:hello"),
        "onmessage must deliver the echo: {}",
        log
    );
    assert!(
        log.contains("close:3"),
        "onclose must fire with readyState 3: {}",
        log
    );
    assert!(
        !log.contains("error"),
        "no onerror expected in this flow: {}",
        log
    );
}

/// Non-blocking proof: with a server that never completes the WS handshake,
/// the constructor returns immediately and the JS thread stays responsive
/// (a 10ms timer fires while the WS is still CONNECTING). The pre-fix code
/// performed the blocking connect inside the constructor on the JS thread.
#[test]
fn ws_constructor_does_not_block_js_thread() {
    let port = spawn_silent_tcp_server();
    let mut ctx = new_test_ctx();

    let setup = format!(
        r#"
        var wsLog = [];
        var ws = new WebSocket("ws://127.0.0.1:{}/hang");
        // If the constructor blocked (old behavior), these two statements
        // only run after the ~10s connect window and timerFired stays false.
        var ctorReturnedAt = Date.now();
        var timerFired = false;
        setTimeout(function() {{ timerFired = true; }}, 10);
        "done"
        "#,
        port
    );

    let started = std::time::Instant::now();
    assert_eq!(
        eval_str(&mut ctx, &setup),
        "done",
        "constructor eval failed"
    );
    let ctor_elapsed = started.elapsed();
    assert!(
        ctor_elapsed < Duration::from_secs(2),
        "WebSocket constructor blocked the JS thread for {:?} (must return immediately)",
        ctor_elapsed
    );

    let fired = pump_until(&mut ctx, "timerFired", 3_000);
    assert!(fired, "timer must fire while WS connect is pending");
    assert_eq!(
        eval_str(&mut ctx, "ws.readyState"),
        "0",
        "WS must still be CONNECTING (handshake deliberately unanswered)"
    );
}

/// wss:// echo round-trip: the client TLS handshake runs the full stealth
/// application path (Firefox cipher list / TLS1.3 suites / curves / sigalgs
/// via configure_http_client_with_alpn) against a real BoringSSL server —
/// green test proves the stealth-configured ClientHello is accepted.
#[test]
fn wss_async_roundtrip_with_stealth_profile() {
    let port = spawn_tls_ws_server();
    let mut ctx = new_test_ctx();

    // install_all installs the default Firefox stealth profile — the exact
    // production configuration a page gets.
    let setup = format!(
        r#"
        var wsLog = [];
        var ws = new WebSocket("wss://127.0.0.1:{}/secure");
        ws.onopen = function() {{ wsLog.push("open"); ws.send("ping"); }};
        ws.onmessage = function(ev) {{ wsLog.push("msg:" + ev.data); if (ev.data.indexOf("ECHO:") === 0) {{ ws.close(); }} }};
        ws.onerror = function(ev) {{ wsLog.push("error:" + (ev.data || "?")); }};
        ws.onclose = function() {{ wsLog.push("close"); }};
        "done"
        "#,
        port
    );
    assert_eq!(
        eval_str(&mut ctx, &setup),
        "done",
        "constructor eval failed"
    );

    let done = pump_until(&mut ctx, "wsLog.indexOf('close') >= 0", 8_000);
    let log = eval_str(&mut ctx, "wsLog.join('|')");
    assert!(
        done,
        "wss round-trip did not finish in budget; log: {}",
        log
    );
    assert!(log.contains("open"), "wss onopen must fire: {}", log);
    assert!(
        log.contains("msg:ECHO:ping"),
        "wss onmessage must deliver the echo: {}",
        log
    );
    assert!(!log.contains("error"), "no onerror expected: {}", log);
}

/// Connect failure must surface explicitly: refused port → onerror (with the
/// reason) + onclose, readyState 3. Never a constructor throw, never silence.
#[test]
fn ws_connect_failure_fires_onerror_and_onclose() {
    // Port 1 on loopback: connection refused (nothing listens there).
    let mut ctx = new_test_ctx();

    let setup = r#"
        var wsLog = [];
        var ws = new WebSocket("ws://127.0.0.1:1/refused");
        ws.onopen = function() { wsLog.push("open"); };
        ws.onerror = function(ev) { wsLog.push("error:" + (ev.data || "no-reason")); };
        ws.onclose = function() { wsLog.push("close:" + ws.readyState); };
        "done"
        "#;
    assert_eq!(eval_str(&mut ctx, setup), "done", "constructor eval failed");

    let done = pump_until(
        &mut ctx,
        "wsLog.some(function(l){return l.indexOf('close:')===0})",
        8_000,
    );
    let log = eval_str(&mut ctx, "wsLog.join('|')");
    assert!(done, "failure did not surface in budget; log: {}", log);
    assert!(
        log.starts_with("error:"),
        "onerror must fire FIRST with the failure reason, got: {}",
        log
    );
    assert!(
        !log.contains("error:no-reason") && !log.contains("error:?"),
        "onerror must carry the reason (no silent failure), got: {}",
        log
    );
    assert!(
        log.contains("close:3"),
        "onclose must fire with readyState 3: {}",
        log
    );
    assert!(
        !log.contains("open"),
        "onopen must not fire for a refused connect: {}",
        log
    );
    assert_eq!(
        eval_str(&mut ctx, "ws.readyState"),
        "3",
        "final readyState must be CLOSED"
    );
}

/// Raw TLS handshake isolation (no JS): drive a bao_boringssl_bridge client
/// against the test TLS server, with and without the stealth config, to
/// isolate which leg of the wss handshake stalls.
#[test]
fn wss_raw_handshake_isolation() {
    use bao_boringssl_bridge::client::TlsClient;
    use bao_boringssl_bridge::connection::TlsConnection;
    use std::net::TcpStream;

    fn try_handshake(port: u16, stealth: bool) -> Result<(), String> {
        let mut tcp = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        tcp.set_read_timeout(Some(Duration::from_secs(3))).ok();
        let tls_client = TlsClient::new().map_err(|e| e.to_string())?;
        let mut tls =
            TlsConnection::new_client(&tls_client, "127.0.0.1").map_err(|e| e.to_string())?;
        if stealth {
            let profile = Some(bao_stealth::StealthProfile::firefox_default());
            let cfg = bun_runtime::stealth_http::stealth_profile_to_ssl_config(&profile);
            let host_c = std::ffi::CString::new("127.0.0.1").unwrap();
            let ssl = tls.ssl_ptr();
            bun_http::configure_http_client_with_alpn(
                unsafe { &mut *ssl },
                host_c.as_ptr(),
                bun_http::AlpnOffer::H1,
                Some(&cfg),
            );
        }
        loop {
            let res = tls.process().map_err(|e| format!("process: {}", e))?;
            // Flush every flight before blocking on read (BCE-20260814-WS-TLS
            // ordering contract).
            loop {
                let outgoing = tls.take_outgoing();
                if outgoing.is_empty() {
                    break;
                }
                tcp.write_all(&outgoing).map_err(|e| e.to_string())?;
            }
            if matches!(res.state, TlsState::Active | TlsState::PeerClosed) {
                return Ok(());
            }
            let mut buf = [0u8; 16_384];
            match tcp.read(&mut buf) {
                Ok(0) => return Err("eof".into()),
                Ok(n) => tls.feed(&buf[..n]),
                Err(e) => return Err(format!("read: {}", e)),
            }
        }
    }

    let port = spawn_tls_ws_server();
    let plain = try_handshake(port, false);
    eprintln!("[raw] no-stealth result: {:?}", plain);
    let stealth = try_handshake(port, true);
    eprintln!("[raw] stealth result: {:?}", stealth);
    assert!(plain.is_ok(), "no-stealth handshake must work: {:?}", plain);
    assert!(
        stealth.is_ok(),
        "stealth handshake must work: {:?}",
        stealth
    );
}
