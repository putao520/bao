// @trace TEST-ENG-6-WS-PROTOCOL [req:REQ-BRW-012,REQ-ENG-006] [level:integration]
//
// RFC 6455 §5.2/§5.4 protocol semantics (node v24.19.0 oracle:
// /tmp/ws-oracle/probe.mjs) for the shared web_api WebSocket read loop:
//
//   - reserved opcodes (0x3..0x7 / 0xB..0xF) FAIL the connection with a
//     protocol error — never a clean Close (node: error + close 1006, later
//     frames never delivered). Plain path pinned here; the TLS path is
//     pinned by c10_ws_reserved_opcode_frame_fails_connection.
//   - a bare Continuation frame (no fragmentation in progress) is a protocol
//     error on BOTH paths (node: first message delivered, then error+close,
//     the orphan payload never delivered).
//   - a fragmented message (first frame FIN=0 Text/Binary, Continuation
//     frames until FIN=1) is reassembled into ONE message dispatch, with
//     control frames (Ping) allowed to interleave (node reassembles the same
//     way; interleaved ping does not break the message).
//   - a new data frame mid-fragment is a protocol error.
//
// Every server is REAL loopback code (std TcpListener / BoringSSL TlsServer)
// and every frame is hand-built RFC 6455 wire truth.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use bao_boringssl_bridge::connection::{TlsConnection, TlsState};
use bao_boringssl_bridge::server::TlsServer;
use bao_engine::context::JsContext;
use bun_uws::ws_codec::{FrameDecoder, Opcode, apply_mask};
use bun_uws::ws_handshake::server_handshake;

#[path = "common/mod.rs"]
mod common;

use common::eval_string;

// ── test-context + pump plumbing ─────────────────────────────────────────

fn new_test_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// @trace REQ-ENG-010
/// Settle-pump delegate: unified suite pump (`common::pump_until`) evaluating
/// `Boolean(probe)` until truthy (the file's settle shape) or budget expiry.
fn pump_until(ctx: &mut JsContext, probe: &str, budget_ms: u64) -> bool {
    common::pump_until(
        ctx,
        &format!("Boolean({})", probe),
        Duration::from_millis(budget_ms),
        common::eval_string,
        |p| p == "true",
    ) == "true"
}

/// Hand-built server→client frame (unmasked per RFC 6455 §5.1). `fin=false`
/// opens/continues a fragmented message (§5.4).
fn frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 126, "test frames stay in the short form");
    let mut buf = vec![
        (if fin { 0x80 } else { 0x00 }) | opcode,
        payload.len() as u8,
    ];
    buf.extend_from_slice(payload);
    buf
}

/// Read client frames until a Close lands, then answer it (clean close
/// handshake). Any other frame is consumed silently.
fn serve_close_handshake<R: Read + Write>(io: &mut R) {
    let mut dec = FrameDecoder::new();
    loop {
        let header = match dec.decode_frame(io) {
            Ok(Some(h)) => h,
            _ => return,
        };
        let _payload = if header.mask {
            let key = dec.take_mask();
            let mut p = dec.take_payload(&header);
            apply_mask(&mut p, &key);
            p
        } else {
            dec.take_payload(&header)
        };
        if header.opcode == Opcode::Close {
            let _ = io.write_all(&[0x88, 0x02, 0x03, 0xE8]);
            let _ = io.flush();
            return;
        }
    }
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

// ── TLS server plumbing (mirrored from the c10 carrier) ──────────────────

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
                    ));
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
                    ));
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

fn spawn_tls_ws_server(serve: impl FnOnce(&mut ServerTlsIo) + Send + 'static) -> u16 {
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
                Err(_) => return,
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

// ═════════════════════════════════════════════════════════════════════════
// 1. Reserved opcode on the PLAIN path: RFC 6455 §5.2 fail-the-connection.
//    (The TLS half is c10_ws_reserved_opcode_frame_fails_connection.)
#[test]
fn ws_reserved_opcode_fails_plain_connection() {
    let port = spawn_plain_ws_server(move |stream| {
        let _ = stream.write_all(&frame(0x1, true, b"before"));
        let _ = stream.write_all(&[0x83, 0x00]); // FIN + reserved opcode 0x3
        let _ = stream.flush();
        std::thread::sleep(Duration::from_millis(250));
        let _ = stream.write_all(&frame(0x1, true, b"after"));
        let _ = stream.flush();
        serve_close_handshake(stream);
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("ws://127.0.0.1:{port}/reserved");
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
    assert!(
        done,
        "plain reserved-opcode failure did not surface: {}",
        log
    );
    assert!(
        log.starts_with("open|t:before|error:") && log.ends_with("|close:3"),
        "plain reserved opcode must fail the connection: {}",
        log
    );
    assert!(
        log.contains("reserved websocket opcode 0x3"),
        "the protocol-error reason must name the reserved opcode: {}",
        log
    );
    assert!(
        !bun_runtime::web_api::ws_has_pending(),
        "failed socket must deregister"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 2. Bare Continuation on the PLAIN path: protocol error (node oracle:
//    the earlier message delivers, then error + close, orphan payload never
//    delivers). The WS_EDGE carrier pinned the old Binary-delivery behavior.
#[test]
fn ws_bare_continuation_fails_plain_connection() {
    let port = spawn_plain_ws_server(move |stream| {
        let _ = stream.write_all(&frame(0x1, true, b"first"));
        let _ = stream.write_all(&frame(0x0, true, b"orphan"));
        let _ = stream.flush();
        std::thread::sleep(Duration::from_millis(250));
        let _ = stream.write_all(&frame(0x1, true, b"after"));
        let _ = stream.flush();
        serve_close_handshake(stream);
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("ws://127.0.0.1:{port}/orphan-cont");
        ws.onopen = function () {{ __log.push("open"); }};
        ws.onmessage = function (ev) {{
            if (typeof ev.data === "string") {{ __log.push("t:" + ev.data); }}
            else {{ __log.push("b"); }}
        }};
        ws.onerror = function (ev) {{ __log.push("error:" + (ev.data || "?")); }};
        ws.onclose = function () {{ __log.push("close:" + ws.readyState); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(
        done,
        "plain bare-continuation failure did not surface: {}",
        log
    );
    assert!(
        log.starts_with("open|t:first|error:") && log.ends_with("|close:3"),
        "bare continuation must fail the connection (orphan payload dropped): {}",
        log
    );
    assert!(
        log.contains("continuation frame without a preceding fragmented message"),
        "the protocol-error reason must identify the orphan continuation: {}",
        log
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 3. Bare Continuation on the TLS path: identical failure semantics (the
//    former TLS arm silently skipped continuation frames).
#[test]
fn ws_bare_continuation_fails_tls_connection() {
    let port = spawn_tls_ws_server(move |io| {
        let _ = io.write_all(&frame(0x1, true, b"first"));
        let _ = io.write_all(&frame(0x0, true, b"orphan"));
        let _ = io.flush();
        std::thread::sleep(Duration::from_millis(250));
        let _ = io.write_all(&frame(0x1, true, b"after"));
        let _ = io.flush();
        serve_close_handshake(io);
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("wss://127.0.0.1:{port}/orphan-cont-tls");
        ws.onopen = function () {{ __log.push("open"); }};
        ws.onmessage = function (ev) {{
            if (typeof ev.data === "string") {{ __log.push("t:" + ev.data); }}
            else {{ __log.push("b"); }}
        }};
        ws.onerror = function (ev) {{ __log.push("error:" + (ev.data || "?")); }};
        ws.onclose = function () {{ __log.push("close:" + ws.readyState); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(
        done,
        "TLS bare-continuation failure did not surface: {}",
        log
    );
    assert!(
        log.starts_with("open|t:first|error:") && log.ends_with("|close:3"),
        "TLS bare continuation must fail the connection: {}",
        log
    );
    assert!(
        log.contains("continuation frame without a preceding fragmented message"),
        "TLS protocol-error reason must identify the orphan continuation: {}",
        log
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 4. Fragmented TEXT reassembly on the PLAIN path: three frames (FIN=0 Text,
//    FIN=0 Continuation, FIN=1 Continuation) deliver as ONE "Hello world"
//    message — node oracle reassembles the same way. The former plain path
//    delivered each frame as a separate message (continuation as binary).
#[test]
fn ws_fragmented_text_reassembles_single_message_plain() {
    let port = spawn_plain_ws_server(move |stream| {
        let _ = stream.write_all(&frame(0x1, false, b"Hel"));
        let _ = stream.write_all(&frame(0x0, false, b"lo "));
        let _ = stream.write_all(&frame(0x0, true, b"world"));
        let _ = stream.flush();
        serve_close_handshake(stream);
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("ws://127.0.0.1:{port}/frag-text");
        ws.onopen = function () {{ __log.push("open"); }};
        ws.onmessage = function (ev) {{
            __log.push((typeof ev.data === "string" ? "t:" : "b:") + ev.data);
            ws.close();
        }};
        ws.onerror = function (ev) {{ __log.push("error:" + (ev.data || "?")); }};
        ws.onclose = function () {{ __log.push("close:" + ws.readyState); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(done, "fragmented text did not finish: {}", log);
    assert_eq!(
        log, "open|t:Hello world|close:3",
        "a fragmented text message must reassemble into ONE dispatch: {}",
        log
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 5. Fragmented BINARY reassembly over TLS with an interleaved Ping
//    (RFC 6455 §5.4 allows control frames between fragments): the message
//    delivers as one 1,2,3,4 Uint8Array and the ping is still ponged — the
//    reassembly state survives the control frame.
#[test]
fn ws_fragmented_binary_reassembles_across_interleaved_ping_tls() {
    let pongs: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let pong_sink = pongs.clone();
    let port = spawn_tls_ws_server(move |io| {
        let _ = io.write_all(&frame(0x2, false, &[1, 2]));
        let _ = io.write_all(&frame(0x9, true, b"q")); // Ping mid-fragment
        let _ = io.write_all(&frame(0x0, true, &[3, 4]));
        let _ = io.flush();
        // Record the client's Pong, then answer the close handshake.
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
                Opcode::Pong => pong_sink
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&payload).into_owned()),
                Opcode::Close => {
                    let _ = io.write_all(&[0x88, 0x02, 0x03, 0xE8]);
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
        var ws = new WebSocket("wss://127.0.0.1:{port}/frag-bin");
        ws.onopen = function () {{ __log.push("open"); }};
        ws.onmessage = function (ev) {{
            var a = new Uint8Array(ev.data), s = "";
            for (var i = 0; i < a.length; i++) s += a[i] + ",";
            __log.push("b:" + s);
            ws.close();
        }};
        ws.onerror = function (ev) {{ __log.push("error:" + (ev.data || "?")); }};
        ws.onclose = function () {{ __log.push("close:" + ws.readyState); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(done, "fragmented binary did not finish: {}", log);
    assert_eq!(
        log, "open|b:1,2,3,4,|close:3",
        "a fragmented binary message must reassemble as ONE dispatch: {}",
        log
    );
    assert_eq!(
        pongs.lock().unwrap().clone(),
        vec!["q".to_string()],
        "the interleaved ping must still be ponged during reassembly"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 6. A NEW data frame mid-fragment is a protocol error (RFC 6455 §5.4: a
//    message in progress may only be continued by Continuation frames).
#[test]
fn ws_data_frame_mid_fragment_fails_connection() {
    let port = spawn_plain_ws_server(move |stream| {
        let _ = stream.write_all(&frame(0x1, false, b"a")); // opens a fragment
        let _ = stream.write_all(&frame(0x1, true, b"b")); // illegal: new data frame
        let _ = stream.flush();
        std::thread::sleep(Duration::from_millis(250));
        serve_close_handshake(stream);
    });

    let mut ctx = new_test_ctx();
    let setup = format!(
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        var ws = new WebSocket("ws://127.0.0.1:{port}/mid-frag");
        ws.onopen = function () {{ __log.push("open"); }};
        ws.onmessage = function (ev) {{ __log.push("msg"); }};
        ws.onerror = function (ev) {{ __log.push("error:" + (ev.data || "?")); }};
        ws.onclose = function () {{ __log.push("close:" + ws.readyState); __done = true; }};
        "setup-ok";
        "#,
        port = port
    );
    assert_eq!(eval_string(&mut ctx, &setup), "setup-ok");

    let done = pump_until(&mut ctx, "__done", 12_000);
    let log = eval_string(&mut ctx, "__log.join('|')");
    assert!(done, "mid-fragment data frame did not fail: {}", log);
    assert!(
        log.starts_with("open|error:") && log.ends_with("|close:3"),
        "a data frame mid-fragment must fail the connection: {}",
        log
    );
    assert!(
        log.contains("data frame while a fragmented message is in progress"),
        "the protocol-error reason must identify the mid-fragment data frame: {}",
        log
    );
}
