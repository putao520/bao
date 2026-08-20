// @trace REQ-STL-001 [api:WebSocket] [level:e2e] — page wss over the bao
// TLS stack.
//
// Proves the servo page WebSocket path end-to-end after the migration off
// the servo connector's BoringsslTlsStream: a servo page's
// `new WebSocket("wss://...")` runs TLS through
// `bun_http::websocket_http_client::WsTlsStream` (BoringSSL bridge +
// stealth per-connection fingerprint + process-wide session-cache offer)
// and completes a real message roundtrip against a local TLS WS server.
//
// Harness notes (same contract as realworld_full_stack_tests.rs):
//   - single #[test] (mozjs Runtime / servo Opts are per-process
//     singletons);
//   - data: URL page origin — WebSocket is not CORS-gated, so a data:
//     origin may open wss:// to loopback;
//   - the wss server is Rust-native (bao_boringssl_bridge::TlsServer +
//     bun_uws codec/handshake driven over the same pub TlsIoStream the
//     client stack uses), on an OS-assigned port in a background thread.
#![allow(dead_code)]

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool};
use bun_http::websocket_http_client::TlsIoStream;
use bun_uws::ws_codec::{FrameDecoder, FrameEncoder, Opcode};

/// Drive one TLS-WS echo connection: TLS accept + handshake, RFC 6455
/// server handshake, then echo every text frame back until the peer closes
/// or the socket times out. Returns the number of echoed messages.
fn serve_wss_echo(tcp: TcpStream, tls_server: &bao_boringssl_bridge::TlsServer) -> usize {
    let mut io = TlsIoStream::new(tcp, tls_server.accept().expect("tls accept"));
    io.drive_handshake().expect("server tls handshake");
    bun_uws::ws_handshake::server_handshake(&mut io).expect("server ws handshake");

    let mut decoder = FrameDecoder::new();
    let mut encoder = FrameEncoder::new();
    let mut echoed = 0;
    loop {
        let header = match decoder.decode_frame(&mut io) {
            Ok(Some(h)) => h,
            _ => break,
        };
        let payload = if header.mask {
            let key = decoder.take_mask();
            let mut p = decoder.take_payload(&header);
            bun_uws::ws_codec::apply_mask(&mut p, &key);
            p
        } else {
            decoder.take_payload(&header)
        };
        match header.opcode {
            Opcode::Text | Opcode::Binary => {
                let reply = encoder
                    .encode_frame(header.opcode, &payload, None)
                    .to_vec();
                if io.write_all(&reply).is_err() {
                    break;
                }
                echoed += 1;
            },
            Opcode::Close => {
                let reply = encoder.encode_close(1000, "").to_vec();
                let _ = io.write_all(&reply);
                break;
            },
            _ => {},
        }
    }
    echoed
}

/// Spawn a one-connection wss echo server on an OS-assigned port.
fn spawn_wss_echo_server() -> (u16, Arc<AtomicBool>) {
    let (cert_pem, key_pem) =
        bao_boringssl_bridge::generate_self_signed_pem("localhost", 1).expect("self-signed pem");
    let tls_server =
        bao_boringssl_bridge::TlsServer::new(&cert_pem, &key_pem).expect("TlsServer::new");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind wss");
    let port = listener.local_addr().unwrap().port();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_c = Arc::clone(&shutdown);
    let _ = listener.set_nonblocking(true);

    std::thread::Builder::new()
        .name("wss-echo-fixture".into())
        .spawn(move || {
            while !shutdown_c.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((tcp, _)) => {
                        let _ = tcp.set_read_timeout(Some(Duration::from_secs(30)));
                        let _ = tcp.set_nonblocking(false);
                        let _ = serve_wss_echo(tcp, &tls_server);
                        return;
                    },
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    },
                    Err(_) => return,
                }
            }
        })
        .expect("spawn wss fixture");
    (port, shutdown)
}

/// Percent-encode the characters reserved by the data: URL scheme (same
/// minimal escaping as realworld_full_stack_tests::html_escape_minimal).
fn data_url_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'#' | b'%' | b'&' | b'?' | b'<' | b'>' | b'"' | b'\\' | b'^' | b'`' | b'{' | b'}'
            | b'|' => out.push_str(&format!("%{:02X}", b)),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[test]
fn page_wss_roundtrip_over_bao_tls() {
    let (wss_port, wss_shutdown) = spawn_wss_echo_server();

    let config = BaoConfig::default();
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => panic!("BaoRuntime::new failed: {}", e),
    };
    let pool: &PagePool = runtime.page_pool();

    let page_js = format!(
        r#"
window.__wsState = 'init';
try {{
  var ws = new WebSocket('wss://127.0.0.1:{port}/echo');
  window.__wsState = 'connecting';
  ws.onopen = function() {{ window.__wsState = 'open'; ws.send('ping'); }};
  ws.onmessage = function(ev) {{
    window.__wsState = (ev.data === 'ping') ? 'done' : ('wrong:' + ev.data);
  }};
  ws.onerror = function() {{
    if (window.__wsState !== 'done') window.__wsState = 'error';
  }};
  ws.onclose = function() {{
    if (window.__wsState !== 'done') window.__wsState += '+closed';
  }};
}} catch (e) {{
  window.__wsState = 'throw:' + e;
}}
'connecting';
"#,
        port = wss_port
    );
    let html = format!(
        "<!DOCTYPE html><html><head><title>wss e2e</title></head>\
         <body><p id=\"t\">wss</p><script>{}</script></body></html>",
        page_js
    );
    let url = format!("data:text/html;charset=utf-8,{}", data_url_escape(&html));

    // First pipeline start (mozjs + servo init) can exceed create_page's
    // internal 10s readiness timeout on a loaded machine — retry; later
    // attempts ride an already-warm servo.
    let mut page = None;
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(url.clone()),
            ..Default::default()
        }) {
            Ok(p) => {
                page = Some(p);
                break;
            },
            Err(e) => {
                eprintln!("page creation failed (retrying): {}", e);
                std::thread::sleep(Duration::from_secs(3));
            },
        }
    }
    let page = match page {
        Some(p) => p,
        None => panic!("page creation failed after retries"),
    };

    // Poll the page until the WS state resolves (drive servo's loop with
    // empty evaluates, same as wait_for_load in the full-stack harness).
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut final_state = String::from("timeout");
    while std::time::Instant::now() < deadline {
        match page.evaluate_js("window.__wsState") {
            Ok(s) => {
                let s = s.trim().trim_matches('"').to_string();
                if s.starts_with("done") || s.starts_with("error") || s.starts_with("throw") {
                    final_state = s;
                    break;
                }
            },
            Err(_) => {},
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    wss_shutdown.store(true, Ordering::SeqCst);
    let _ = page.close();
    pool.close_all();

    assert_eq!(final_state, "done", "page wss did not roundtrip: {}", final_state);
}
