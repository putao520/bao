// @trace TEST-ENG-006-WS-C10 [req:REQ-BRW-12,REQ-ENG-006] [level:integration]
//
// C10 coverage-convergence wave: the remaining JS-reachable error/boundary
// paths of web_api.rs (REQ-BRW-12 WebSocket API + the web-API gap faces
// registered in .plans/coverage-governance-20261017.md) that the ws_edge /
// ws_async / subtle_edge carriers never reach:
//
//   web_api.rs WebSocket face (REQ-BRW-12 event + close contract):
//   - wss:// (Tls) read_message arms never driven by the async happy path:
//     masked server frame (take_mask arm), Ping→pong echo written back over
//     TLS (push_masked_len + apply_mask + TlsStream::write), unsolicited
//     Pong / Continuation recursive skip arms
//   - abrupt TLS transport EOF: pump_inbound Ok(0) → UnexpectedEof →
//     read_message EOF→Close arm → clean onclose without onerror
//   - connect_tls failure arms: EOF mid-handshake ("tls handshake stalled"),
//     garbage bytes ("tls handshake: <ssl error>"), unresolvable/invalid
//     host + wss default-port arm ("invalid address: ...")
//   - illegal-frame semantics: a decoder-unknown opcode (0x3, reserved per
//     RFC 6455 §5.2) FAILS the connection with a protocol error on both the
//     plain and TLS paths — onerror(reason) + onclose, later frames never
//     delivered (node v24 oracle: error + close 1006)
//   - scheme-less URL fallback (constructor + WsConn::connect else-arms) and
//     empty text/binary frames (zero-length Uint8Array dispatch arm)
//   - ws_trigger_event handler-throw BCE path → uncaughtException routing
//     (the pump keeps running), and non-function handler slots silently
//     skipped (is_object false + JS_ObjectIsFunction false arms)
//
//   localStorage guards (corrupt-file load, non-string value filter, arity /
//   index guards) and the addrinfo seam's V6 completed-hit entry arm +
//   cancel-after-completed contract (the C-ABI face the usockets C callers
//   drive, same carrier form as uloop_edge_tests).
//
// Every server is REAL loopback code (std TcpListener / BoringSSL TlsServer);
// every frame on the wire is RFC 6455 truth via bun_uws codec primitives.
// Env-gated early returns are forbidden in this suite (Goal A discipline):
// all tests run unconditionally.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use bao_boringssl_bridge::connection::{TlsConnection, TlsState};
use bao_boringssl_bridge::server::TlsServer;
use bao_engine::context::JsContext;
use bun_uws::ws_codec::{apply_mask, FrameDecoder, FrameEncoder, Opcode};
use bun_uws::ws_handshake::server_handshake;
use mozjs::rooted;

#[path = "common/mod.rs"]
mod common;

use common::eval_string;

// ── test-context + pump plumbing (mirrors web_socket_async_tests.rs) ─────

fn new_test_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Drive the JS thread (jobs + timers + the WebSocket drain pump) until
/// `probe` evaluates truthy, or the budget runs out.
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
        if eval_string(ctx, &format!("Boolean({})", probe)) == "true" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

/// Server→client text frame (FIN + Text, unmasked — server side).
fn server_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0x81u8];
    assert!(payload.len() < 126, "test frames stay in the short form");
    buf.push(payload.len() as u8);
    buf.extend_from_slice(payload);
    buf
}

// ── TLS server plumbing (server-side BoringSSL state machine, mirrored from
//    web_socket_async_tests.rs; the client's private TlsStream counterpart) ──

struct ServerTlsIo {
    tcp: TcpStream,
    tls: TlsConnection,
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl ServerTlsIo {
    fn handshake(tcp: &mut TcpStream, tls: &mut TlsConnection) -> std::io::Result<()> {
        loop {
            let res = tls
                .process()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
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

/// Accept + TLS-handshake one connection, hand the plaintext stream to
/// `serve`, and answer the WS upgrade first (every C10 TLS server speaks
/// RFC 6455 immediately after TLS completes).
fn spawn_tls_ws_server(
    serve: impl FnOnce(&mut ServerTlsIo) + Send + 'static,
) -> u16 {
    let (cert, key) =
        bao_boringssl_bridge::generate_self_signed_pem("localhost", 365).expect("self-signed cert");
    let server = std::sync::Arc::new(TlsServer::new(&cert, &key).expect("TlsServer"));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut tcp = match listener.incoming().next() {
            Some(Ok(s)) => s,
            _ => return,
        };
        {
            tcp.set_read_timeout(Some(Duration::from_secs(8))).ok();
            let mut tls = match server.accept() {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[c10-wss] tls accept failed: {}", e);
                    return;
                }
            };
            if ServerTlsIo::handshake(&mut tcp, &mut tls).is_err() {
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
            serve(&mut io);
        }
    });
    port
}

// ── plain-ws server plumbing ─────────────────────────────────────────────

fn spawn_plain_ws_server(serve: impl FnOnce(&mut TcpStream) + Send + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut stream = match listener.incoming().next() {
            Some(Ok(s)) => s,
            _ => return,
        };
        {
            stream.set_read_timeout(Some(Duration::from_secs(8))).ok();
            if server_handshake(&mut stream).is_err() {
                return;
            }
            serve(&mut stream);
        }
    });
    port
}

// ═════════════════════════════════════════════════════════════════════════
// 1. wss:// read_message arms the happy path never drives: masked server
//    frame (client take_mask arm), Ping→pong echo written back over TLS
//    (asserted server-side), unsolicited Pong skip.
#[test]
fn c10_ws_tls_probe_frames_ping_pong_continuation_masked() {
    let pong_seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let pong_sink = pong_seen.clone();
    let port = spawn_tls_ws_server(move |io| {
        // ── probe script: masked text, binary, ping, unsolicited pong
        //    (each drives a distinct client arm; a bare continuation is a
        //    protocol error now — pinned by ws_reserved_opcode_tests) ──
        let mut enc = FrameEncoder::new();
        let masked = enc.encode_frame(Opcode::Text, b"tls-masked", Some([0x5a, 0xa5, 0x11, 0x99]));
        io.write_all(&masked).expect("masked text");
        let bin = enc.encode_binary(&[0x01, 0xff, 0x00, 0x80]);
        io.write_all(&bin).expect("binary");
        let ping = enc.encode_frame(Opcode::Ping, b"pin", None);
        io.write_all(&ping).expect("ping");
        let pong = enc.encode_frame(Opcode::Pong, b"p", None);
        io.write_all(&pong).expect("pong");
        let _ = io.flush();

        // ── echo loop: client text → "echo:<t>"; record the client's Pong
        //    echo (proof the TLS Ping arm masked + wrote a real frame) ──
        let mut dec = FrameDecoder::new();
        loop {
            let header = match dec.decode_frame(io) {
                Ok(Some(h)) => h,
                _ => return,
            };
            let payload = if header.mask {
                let key = dec.take_mask();
                let mut p = dec.take_payload(&header);
                apply_mask(&mut p, &key);
                p
            } else {
                dec.take_payload(&header)
            };
            match header.opcode {
                Opcode::Text => {
                    let echoed = format!("echo:{}", String::from_utf8_lossy(&payload));
                    let _ = io.write_all(&server_text_frame(echoed.as_bytes()));
                    let _ = io.flush();
                }
                Opcode::Pong => {
                    pong_sink
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&payload).into_owned());
                }
                Opcode::Close => {
                    let _ = io.write_all(&enc.encode_close(1000, ""));
                    let _ = io.flush();
                    return;
                }
                _ => {}
            }
        }
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("wss://127.0.0.1:{port}/probe");
        ws.onopen = function () {{ __log.push("open"); ws.send("hi"); }};
        ws.onmessage = function (ev) {{
            if (typeof ev.data === "string") {{ __log.push("t:" + ev.data); }}
            else {{
                var a = new Uint8Array(ev.data), s = "";
                for (var i = 0; i < a.length; i++) s += a[i].toString(16) + ",";
                __log.push("b:" + s);
            }}
            if (__log.length >= 4 && !ws.__closing) {{ ws.__closing = true; ws.close(); }}
        }};
        ws.onerror = function () {{ __log.push("unexpected-error"); }};
        ws.onclose = function () {{ __log.push("close"); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(done, "wss probe transcript did not finish: {}", log);

    // Exact transcript: masked text (client-side unmask arm), binary, then —
    // ping consumed by the shared Ping arm (pong echoed back) and the
    // unsolicited pong skipped — then the echo, no error.
    assert_eq!(
        log, "open|t:tls-masked|b:1,ff,0,80,|t:echo:hi|close",
        "TLS probe-frame transcript diverged"
    );

    // Server-side proof of the Ping→pong echo: the client masked a Pong
    // frame carrying the ping's payload back over the TLS stream.
    let pongs = pong_seen.lock().unwrap().clone();
    assert_eq!(
        pongs,
        vec!["pin".to_string()],
        "client must echo the server ping as a masked pong over TLS"
    );
    assert!(
        !bun_runtime::web_api::ws_has_pending(),
        "closed wss socket must deregister from the drain-pump registry"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 2. Abrupt TLS transport EOF after traffic: the server drops the TCP
//    connection without a WS close frame. pump_inbound's Ok(0) arm surfaces
//    as UnexpectedEof, read_message maps it to a clean Close → onclose with
//    NO onerror (EOF is not a transport error — it is the peer going away).
#[test]
fn c10_ws_tls_abrupt_eof_is_clean_close_without_error() {
    let port = spawn_tls_ws_server(move |io| {
        let _ = io.write_all(&server_text_frame(b"bye-soon"));
        let _ = io.flush();
        // Give the client's non-blocking pump time to deliver the message
        // before the transport vanishes.
        std::thread::sleep(Duration::from_millis(400));
        // Drop: no close frame, just TCP FIN (io + tcp dropped here).
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("wss://127.0.0.1:{port}/eof");
        ws.onopen = function () {{ __log.push("open"); }};
        ws.onmessage = function (ev) {{ __log.push("t:" + ev.data); }};
        ws.onerror = function (ev) {{ __log.push("error:" + (ev.data || "?")); }};
        ws.onclose = function () {{ __log.push("close:" + ws.readyState); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(done, "abrupt EOF did not surface as a close: {}", log);
    assert_eq!(
        log, "open|t:bye-soon|close:3",
        "abrupt TLS EOF must deliver the queued message then close cleanly"
    );
    assert_eq!(eval_string(&mut ctx, "String(ws.readyState)"), "3");
    assert!(
        !bun_runtime::web_api::ws_has_pending(),
        "EOF-closed socket must deregister"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 3. connect_tls failure arms, each surfacing as onerror(reason) + onclose
//    with the REAL reason carried in event.data:
//      a. server accepts TLS TCP then vanishes → "tls handshake stalled"
//         (the read-leg `_` arm after the ClientHello flight is flushed)
//      b. server feeds garbage bytes → "tls handshake: <ssl error>"
//      c. invalid numeric host, no port → wss default-port arm + the
//         "invalid address:" resolution arm (fails offline, deterministically)
#[test]
fn c10_ws_tls_handshake_failures_carry_reasons() {
    // (a) accept + immediate drop.
    let stall_port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((s, _)) = listener.accept() {
                drop(s); // vanish before any TLS byte is answered
            }
        });
        port
    };
    // (b) accept + garbage bytes (never a valid TLS record).
    let garbage_port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = s.write_all(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0x01, 0x02, 0x03]);
                let _ = s.flush();
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        port
    };

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__stall = [];
        globalThis.__garbage = [];
        globalThis.__badhost = [];
        globalThis.__all = 0;
        function watch(log, url) {{
            var ws = new WebSocket(url);
            ws.onopen = function () {{ log.push("UNEXPECTED-OPEN"); }};
            ws.onerror = function (ev) {{ log.push("err:" + (ev.data || "?")); }};
            ws.onclose = function () {{ log.push("close"); __all++; }};
            return ws;
        }}
        watch(__stall, "wss://127.0.0.1:{stall_port}/stall");
        watch(__garbage, "wss://127.0.0.1:{garbage_port}/garbage");
        watch(__badhost, "wss://256.256.256.256/noport");
        "setup-ok";
        "#,
        stall_port = stall_port,
        garbage_port = garbage_port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__all === 3", 20_000);
    let stall = eval_string(&mut ctx, "__stall.join('|')");
    let garbage = eval_string(&mut ctx, "__garbage.join('|')");
    let badhost = eval_string(&mut ctx, "__badhost.join('|')");
    assert!(
        done,
        "not every TLS failure surfaced; stall={} garbage={} badhost={}",
        stall, garbage, badhost
    );

    // (a) EOF during the handshake read leg → the "stalled" arm.
    assert!(
        stall.starts_with("err:tls handshake stalled|close"),
        "stalled server must surface the handshake-stall reason: {}",
        stall
    );
    // (b) garbage bytes → BoringSSL rejects inside process() → the
    //     "tls handshake: <error>" arm.
    assert!(
        garbage.starts_with("err:tls handshake: ") && garbage.ends_with("|close"),
        "garbage server must surface the ssl handshake error: {}",
        garbage
    );
    // (c) invalid numeric host + no explicit port → default-port arm then the
    //     "invalid address:" resolution arm.
    assert!(
        badhost.starts_with("err:invalid address: ") && badhost.ends_with("|close"),
        "invalid host must surface the resolution error: {}",
        badhost
    );
    assert!(
        !bun_runtime::web_api::ws_has_pending(),
        "all failed wss connects must deregister"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 4. Illegal-frame semantics (RFC 6455 §5.2, node v24 oracle): a reserved
//    opcode (0x3 — decoder-unknown) FAILS the connection with a protocol
//    error — onerror(reason) + onclose, wasClean=false in node terms — and
//    frames after it are never delivered. (The former decoder conflated a
//    reserved frame to a clean peer close; that conflation was the defect.)
#[test]
fn c10_ws_reserved_opcode_frame_fails_connection() {
    let port = spawn_tls_ws_server(move |io| {
        let _ = io.write_all(&server_text_frame(b"before"));
        // FIN + reserved opcode 3, empty payload: decoder-rejected.
        let _ = io.write_all(&[0x83, 0x00]);
        let _ = io.flush();
        std::thread::sleep(Duration::from_millis(300));
        let _ = io.write_all(&server_text_frame(b"after"));
        let _ = io.flush();
        std::thread::sleep(Duration::from_millis(200));
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("wss://127.0.0.1:{port}/opcode3");
        ws.onopen = function () {{ __log.push("open"); }};
        ws.onmessage = function (ev) {{ __log.push("t:" + ev.data); }};
        ws.onerror = function (ev) {{ __log.push("error:" + (ev.data || "?")); }};
        ws.onclose = function () {{ __log.push("close:" + ws.readyState); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(done, "reserved-opcode failure did not surface: {}", log);
    // BEFORE is delivered; the reserved frame fails the connection with the
    // decoder's protocol-error reason; AFTER never arrives.
    assert!(
        log.starts_with("open|t:before|error:") && log.ends_with("|close:3"),
        "reserved opcode 0x3 must fail the connection: {}",
        log
    );
    assert!(
        log.contains("reserved websocket opcode 0x3"),
        "the protocol-error reason must name the reserved opcode: {}",
        log
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 5. Scheme-less URL fallback + empty frames: `new WebSocket("host:port/p")`
//    rides the ("ws", url-as-is) fallback arm and the ws:// canonical
//    reconstruction; empty text / empty binary frames drive the zero-length
//    Uint8Array dispatch arm (JS_NewUint8Array(0), no copy).
#[test]
fn c10_ws_schemeless_url_fallback_and_empty_frames() {
    let port = spawn_plain_ws_server(move |stream| {
        // Empty text + empty binary right after the handshake.
        let _ = stream.write_all(&[0x81, 0x00]);
        let _ = stream.write_all(&[0x82, 0x00]);
        let _ = stream.flush();
        let mut dec = FrameDecoder::new();
        loop {
            let header = match dec.decode_frame(stream) {
                Ok(Some(h)) => h,
                _ => return,
            };
            let payload = if header.mask {
                let key = dec.take_mask();
                let mut p = dec.take_payload(&header);
                apply_mask(&mut p, &key);
                p
            } else {
                dec.take_payload(&header)
            };
            match header.opcode {
                Opcode::Text => {
                    let echoed = format!("echo:{}", String::from_utf8_lossy(&payload));
                    let _ = stream.write_all(&server_text_frame(echoed.as_bytes()));
                    let _ = stream.flush();
                }
                Opcode::Close => {
                    let _ = stream.write_all(&[0x88, 0x02, 0x03, 0xE8]);
                    let _ = stream.flush();
                    return;
                }
                _ => {}
            }
        }
    });

    let mut ctx = new_test_ctx();
    // NOTE: no ws:// scheme — the fallback face under test.
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("127.0.0.1:{port}/noscheme");
        ws.onopen = function () {{
            __log.push("open:url=" + ws.url);
            ws.send("");
        }};
        ws.onmessage = function (ev) {{
            if (typeof ev.data === "string") {{ __log.push("t:" + ev.data); }}
            else {{
                var a = new Uint8Array(ev.data);
                __log.push("b:" + (a instanceof Uint8Array) + ":" + a.length);
            }}
            if (__log.length >= 3 && !ws.__closing) {{ ws.__closing = true; ws.close(); }}
        }};
        ws.onerror = function () {{ __log.push("unexpected-error"); }};
        ws.onclose = function () {{ __log.push("close"); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(done, "scheme-less round-trip did not finish: {}", log);
    assert!(
        log.starts_with("open:url=127.0.0.1:"),
        "constructor must accept the scheme-less URL and echo it back: {}",
        log
    );
    assert_eq!(
        log,
        format!(
            "open:url=127.0.0.1:{port}/noscheme|t:|b:true:0|t:echo:|close"
        ),
        "scheme-less fallback + empty-frame transcript diverged: {}",
        log
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 6. Handler-throw BCE path: a throwing onmessage is captured by
//    ws_trigger_event, cleared, and routed through uncaughtException — the
//    process keeps running and the pump keeps delivering (Node semantics).
#[test]
fn c10_ws_throwing_handler_routes_to_uncaughtexception_and_pump_continues() {
    let port = spawn_plain_ws_server(move |stream| {
        for m in ["one", "two", "three"] {
            let _ = stream.write_all(&server_text_frame(m.as_bytes()));
            let _ = stream.flush();
            std::thread::sleep(Duration::from_millis(60));
        }
        // Wait for the client to swap in a real handler and ask for "four".
        let mut dec = FrameDecoder::new();
        loop {
            let header = match dec.decode_frame(stream) {
                Ok(Some(h)) => h,
                _ => return,
            };
            let payload = if header.mask {
                let key = dec.take_mask();
                let mut p = dec.take_payload(&header);
                apply_mask(&mut p, &key);
                p
            } else {
                dec.take_payload(&header)
            };
            match header.opcode {
                Opcode::Text => {
                    let echoed = format!("echo:{}", String::from_utf8_lossy(&payload));
                    let _ = stream.write_all(&server_text_frame(echoed.as_bytes()));
                    let _ = stream.flush();
                }
                Opcode::Close => {
                    let _ = stream.write_all(&[0x88, 0x02, 0x03, 0xE8]);
                    let _ = stream.flush();
                    return;
                }
                _ => {}
            }
        }
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__uncaught = [];
        process.on("uncaughtException", function (e) {{
            __uncaught.push(String(e && e.message || e));
        }});
        globalThis.__log = [];
        globalThis.__gotfour = false;
        globalThis.__done = false;
        var ws = new WebSocket("ws://127.0.0.1:{port}/throw");
        ws.onopen = function () {{ __log.push("open"); }};
        ws.onmessage = function () {{ throw new Error("msg-handler-boom"); }};
        ws.onerror = function () {{ __log.push("unexpected-error"); }};
        ws.onclose = function () {{ __log.push("close"); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    // All three throws must land in the uncaughtException handler.
    let threw = pump_until(&mut ctx, "__uncaught.length === 3", 12_000);
    let uncaught = eval_string(&mut ctx, "__uncaught.join('|')");
    assert!(threw, "throws did not route to uncaughtException: {}", uncaught);
    assert_eq!(
        uncaught, "msg-handler-boom|msg-handler-boom|msg-handler-boom",
        "every message handler throw must be routed verbatim"
    );

    // The pump survives: swap in a real collector and the next message is
    // delivered normally (the handler-throw path must not wedge the entry).
    let healed = eval_string(
        &mut ctx,
        r#"
        (function () {
            __gotfour = false;
            ws.onmessage = function (ev) { if (ev.data === "echo:go") __gotfour = true; };
            ws.send("go");
            return "swapped";
        })();
        "#,
    );
    assert_eq!(healed, "swapped");
    let healed_ok = pump_until(&mut ctx, "__gotfour === true", 12_000);
    assert!(healed_ok, "pump must keep delivering after handler throws");

    // Close cleanly and drain.
    let _ = eval_string(&mut ctx, "ws.close()");
    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(done, "close did not surface: {}", log);
    assert!(
        log.contains("close") && !log.contains("unexpected-error"),
        "throwing handler must not degrade the connection lifecycle: {}",
        log
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 7. Non-function handler slots: `ws.onopen = {}` (object, not callable) and
//    `ws.onmessage = 42` (not even an object) are silently skipped by
//    ws_trigger_event — no throw, no uncaught, dispatch continues.
#[test]
fn c10_ws_non_function_handlers_are_silently_skipped() {
    let port = spawn_plain_ws_server(move |stream| {
        let _ = stream.write_all(&server_text_frame(b"m1-skipped"));
        let _ = stream.flush();
        // Echo loop: the client's "go" comes back as "echo:go" once the real
        // collector handler is installed.
        let mut dec = FrameDecoder::new();
        loop {
            let header = match dec.decode_frame(stream) {
                Ok(Some(h)) => h,
                _ => return,
            };
            let payload = if header.mask {
                let key = dec.take_mask();
                let mut p = dec.take_payload(&header);
                apply_mask(&mut p, &key);
                p
            } else {
                dec.take_payload(&header)
            };
            match header.opcode {
                Opcode::Text => {
                    let echoed = format!("echo:{}", String::from_utf8_lossy(&payload));
                    let _ = stream.write_all(&server_text_frame(echoed.as_bytes()));
                    let _ = stream.flush();
                }
                Opcode::Close => {
                    let _ = stream.write_all(&[0x88, 0x02, 0x03, 0xE8]);
                    let _ = stream.flush();
                    return;
                }
                _ => {}
            }
        }
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__uncaught = [];
        process.on("uncaughtException", function (e) {{ __uncaught.push(String(e)); }});
        globalThis.__log = [];
        globalThis.__got = false;
        globalThis.__done = false;
        var ws = new WebSocket("ws://127.0.0.1:{port}/nonfn");
        ws.onopen = {{ notAFunction: true }};   // object, not callable → skipped
        ws.onmessage = 42;                      // not an object → skipped
        ws.onclose = function () {{ __log.push("close"); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    // Drive until the connection is OPEN (the object-onopen must not crash
    // dispatch) — probe the readyState through the pump.
    let opened = pump_until(&mut ctx, "ws.readyState === 1", 12_000);
    assert!(opened, "non-callable onopen must not wedge dispatch");

    // Install a real collector, then request one message.
    let swapped = eval_string(
        &mut ctx,
        r#"
        (function () {
            ws.onmessage = function (ev) { if (ev.data === "echo:go") __got = true; };
            ws.send("go");
            return "swapped";
        })();
        "#,
    );
    assert_eq!(swapped, "swapped");
    let got = pump_until(&mut ctx, "__got === true", 12_000);
    assert!(got, "real handler must receive after non-function slots: {}", eval_string(&mut ctx, "String(__got)"));

    let _ = eval_string(&mut ctx, "ws.close()");
    let done = pump_until(&mut ctx, "__done", 12_000);
    assert!(done, "close did not surface");
    assert!(
        eval_string(&mut ctx, "__uncaught.length") == "0",
        "non-function handler slots must be silent, not routed as uncaught"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 8. localStorage guard faces: corrupt-file load (serde fail → empty store,
//    next write re-persists valid JSON) and the non-string value filter, plus
//    the arity/index guards of getItem/setItem/key/removeItem. HOME is
//    isolated per test (nextest gives each test its own process).
fn c10_ls_isolate_home(tag: &str) -> std::path::PathBuf {
    let tmp_home = std::env::temp_dir().join(format!("bao-c10-ls-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_home);
    std::fs::create_dir_all(tmp_home.join(".bao")).expect("create isolated HOME");
    // SAFETY: single-threaded test setup, before any JS context exists.
    unsafe { std::env::set_var("HOME", tmp_home.to_string_lossy().to_string()) };
    tmp_home
}

#[test]
fn c10_localstorage_corrupt_file_loads_empty_and_rewrites_valid() {
    let home = c10_ls_isolate_home("corrupt");
    std::fs::write(home.join(".bao/localstorage.json"), "{{{not json at all").expect("seed corrupt");

    let mut ctx = new_test_ctx();
    let report = eval_string(
        &mut ctx,
        r#"
        (function () {
            var out = [];
            out.push("len0=" + localStorage.length);          // corrupt load → empty store
            out.push("probe=" + localStorage.getItem("anything"));
            localStorage.setItem("probe", "survives");        // rewrite path
            out.push("len1=" + localStorage.length);
            out.push("get=" + localStorage.getItem("probe"));
            return out.join("|");
        })();
        "#,
    );
    assert_eq!(
        report, "len0=0|probe=null|len1=1|get=survives",
        "corrupt localstorage.json must load as an empty store, not crash"
    );
    // The next mutation re-persisted valid JSON over the corrupt file.
    let on_disk = std::fs::read_to_string(home.join(".bao/localstorage.json")).expect("reread");
    let parsed: serde_json::Value = serde_json::from_str(&on_disk).expect("file must be valid JSON now");
    assert_eq!(parsed["probe"], "survives", "persisted store must carry the rewrite");
}

#[test]
fn c10_localstorage_non_string_values_filtered_and_arity_guards() {
    let home = c10_ls_isolate_home("filter");
    std::fs::write(
        home.join(".bao/localstorage.json"),
        r#"{"keep":"v","obj":{"a":1},"arr":[1,2],"num":5}"#,
    )
    .expect("seed mixed-type store");

    let mut ctx = new_test_ctx();
    let report = eval_string(
        &mut ctx,
        r#"
        (function () {
            var out = [];
            out.push("keep=" + localStorage.getItem("keep"));   // string value loads
            out.push("obj=" + localStorage.getItem("obj"));     // non-string value filtered
            out.push("num=" + localStorage.getItem("num"));
            out.push("len=" + localStorage.length);
            // ── arity / type guards ──
            try { localStorage.setItem("only-one"); out.push("set=NO-THROW"); }
            catch (e) { out.push("set=THREW:" + e.message); }
            out.push("get-nonstr=" + localStorage.getItem(42)); // non-string key → undefined
            out.push("key-neg=" + localStorage.key(-1));        // negative → null
            out.push("key-str=" + localStorage.key("x"));       // non-int → null
            out.push("key-oob=" + localStorage.key(50));        // out of range → null
            out.push("rm-noarg=" + localStorage.removeItem());  // arity guard → undefined
            return out.join("|");
        })();
        "#,
    );
    assert_eq!(
        report,
        "keep=v|obj=null|num=null|len=1|\
         set=THREW:localStorage.setItem(key, value) requires 2 arguments|\
         get-nonstr=undefined|key-neg=null|key-str=null|key-oob=null|rm-noarg=undefined",
        "localStorage guard faces diverged"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 10. Encoding guard faces the existing carriers don't pin: the TextDecoder
//     streaming BOM-hold arm (a strict BOM prefix held across calls, then
//     stripped when completed), and the atob/btoa input guards (arity /
//     non-string → undefined, len%4==1 + non-alphabet reject, >U+00FF btoa
//     reject).
#[test]
fn c10_text_decoder_bom_hold_and_atob_btoa_guards() {
    let mut ctx = new_test_ctx();
    let report = eval_string(
        &mut ctx,
        r#"
        (function () {
            var out = [];
            // ── streaming BOM: strict prefixes are HELD, the final chunk
            //    completes the BOM and strips it ──
            var d = new TextDecoder();
            out.push("h1=" + JSON.stringify(d.decode(new Uint8Array([0xEF]), {stream:true})));
            out.push("h2=" + JSON.stringify(d.decode(new Uint8Array([0xBB]), {stream:true})));
            out.push("h3=" + JSON.stringify(d.decode(new Uint8Array([0xBF]))));
            out.push("body=" + JSON.stringify(d.decode(new Uint8Array([0x41]))));
            // A non-BOM first byte kills the hold immediately (streaming).
            var d2 = new TextDecoder();
            out.push("nb=" + JSON.stringify(d2.decode(new Uint8Array([0x41]), {stream:true})));
            // ── atob guards ──
            out.push("atob-noarg=" + typeof atob());
            out.push("atob-nonstr=" + typeof atob(42));
            try { atob("abcde"); out.push("len5=NO-THROW"); }          // len%4==1
            catch (e) { out.push("len5=THREW"); }
            try { atob("a@bc"); out.push("alpha=NO-THROW"); }          // non-alphabet
            catch (e) { out.push("alpha=THREW"); }
            out.push("atob-pad=" + atob("YQ==").charCodeAt(0));        // padding strip arm
            // ── btoa guards ──
            out.push("btoa-noarg=" + typeof btoa());
            try { btoa("€"); out.push("u20ac=NO-THROW"); }        // > U+00FF
            catch (e) { out.push("u20ac=THREW"); }
            out.push("btoa-latin1=" + btoa("\xE0"));                   // single-octet face
            return out.join("|");
        })();
        "#,
    );
    assert_eq!(
        report,
        "h1=\"\"|h2=\"\"|h3=\"\"|body=\"A\"|nb=\"A\"|\
         atob-noarg=undefined|atob-nonstr=undefined|len5=THREW|alpha=THREW|atob-pad=97|\
         btoa-noarg=undefined|u20ac=THREW|btoa-latin1=4A==",
        "TextDecoder BOM-hold / atob / btoa guard faces diverged"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 9. addrinfo seam arms uloop_edge never drives: a completed hit built from
//    a V6 cache entry (entry_from_ip's AF_INET6 arm — family, addrlen, port),
//    and Bun__addrinfo_cancel's completed arm (0 — the callback has fired or
//    is queued; the C caller keeps the socket until after_resolve drains it).
#[test]
fn c10_addrinfo_v6_completed_hit_and_cancel_completed_contract() {
    use bun_dns::cache::IpAddr;

    #[repr(C)]
    struct AddrInfoResultView {
        entries: *mut AddrInfoEntryView,
        error: core::ffi::c_int,
    }
    #[repr(C)]
    struct AddrInfoEntryView {
        info: libc::addrinfo,
        _storage: [u8; 128],
    }

    unsafe extern "C" {
        fn Bun__addrinfo_get(
            loop_: *mut core::ffi::c_void,
            host: *const core::ffi::c_char,
            port: u16,
            out: *mut *mut core::ffi::c_void,
        ) -> core::ffi::c_int;
        fn Bun__addrinfo_getRequestResult(req: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn Bun__addrinfo_cancel(req: *mut core::ffi::c_void, socket: *mut core::ffi::c_void)
            -> core::ffi::c_int;
        fn Bun__addrinfo_freeRequest(req: *mut core::ffi::c_void, err: core::ffi::c_int);
    }

    // V6 cache entry → the completed-hit construction path's AF_INET6 arm.
    let v6_bytes = [0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 42];
    bun_dns::cache::insert(b"c10-v6-hit.test", vec![IpAddr::V6(v6_bytes)], Some(60));

    let host = std::ffi::CString::new("c10-v6-hit.test").unwrap();
    let mut req: *mut core::ffi::c_void = std::ptr::null_mut();
    let rc = unsafe { Bun__addrinfo_get(std::ptr::null_mut(), host.as_ptr(), 8443, &mut req) };
    assert_eq!(rc, 0, "cache hit must report completed (0)");
    assert!(!req.is_null());

    // SAFETY: result points at the request's result_c (stable C-layout view).
    let result = unsafe { Bun__addrinfo_getRequestResult(req) };
    assert!(!result.is_null());
    // SAFETY: C-layout view matches addrinfo_result on this platform.
    let view = unsafe { &*(result as *const AddrInfoResultView) };
    assert_eq!(view.error, 0);
    assert!(!view.entries.is_null());
    // SAFETY: single-entry chain built by Request::completed + link_chain.
    let entry = unsafe { &*view.entries };
    assert_eq!(entry.info.ai_family, libc::AF_INET6);
    assert_eq!(
        entry.info.ai_addrlen as usize,
        std::mem::size_of::<libc::sockaddr_in6>(),
        "V6 entry must carry the sockaddr_in6 addrlen"
    );
    assert!(entry.info.ai_next.is_null(), "single-entry chain");
    // SAFETY: ai_addr points at entry.storage (link_chain contract).
    let sa6 = unsafe { &*(entry.info.ai_addr as *const libc::sockaddr_in6) };
    assert_eq!(sa6.sin6_family, libc::AF_INET6 as u16);
    assert_eq!(sa6.sin6_port, 8443u16.to_be(), "port applied to the entry");
    assert_eq!(sa6.sin6_addr.s6_addr, v6_bytes);

    // cancel-after-completed: 0 — no removal possible (callback fired/queued).
    let fake_socket = 0xC10AB1Eusize as *mut core::ffi::c_void;
    assert_eq!(
        unsafe { Bun__addrinfo_cancel(req, fake_socket) },
        0,
        "cancel on a completed request must report 0 (already fired/queued)"
    );
    // SAFETY: release the completed-hit handout (single ref → freed).
    unsafe { Bun__addrinfo_freeRequest(req, 0) };
}
