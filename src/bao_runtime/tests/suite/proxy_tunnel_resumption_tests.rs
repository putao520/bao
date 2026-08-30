// @trace TEST-STL-001-RESUME [req:REQ-STL-001] [level:integration]
//
// CONNECT-proxy e2e for the ProxyTunnel session-resumption wiring: two
// https requests through a local CONNECT proxy to the same TLS target
// origin. The first tunnel performs a full inner-TLS handshake and its
// new-session callback must populate the process-wide cache (tunnel ctx
// `enable_client` + `offer_session` key stash); the second tunnel — forced
// fresh because the target closes each connection after responding — must
// be offered (and accept) the cached session.
//
// Topology per request:
//
//   AsyncHTTP ── plain TCP ──> CONNECT proxy ── TCP ──> TLS target
//              (http_proxy)     (200 tunnel)           (inner TLS)

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bao_boringssl_bridge::connection::TlsState;
use bao_boringssl_bridge::{TlsConnection, TlsServer, generate_self_signed_pem};
use bun_core::MutableString;
use bun_http::header_builder::HeaderBuilder;
use bun_http::{AsyncHTTP, FetchRedirect, Method};
use bun_url::URL;

// Link leg: bun_uws_sys's quic.c (pulled in via the bun_http h3 surface
// behind AsyncHTTP) references liblsquic C symbols; the native archive only
// contributes link directives when its owning rlib is referenced — anchor
// the force_link no-ops the same way bun_runtime's binary force-link chain
// does.
#[used]
static LSQUIC_FORCE_LINK: fn() = bun_lsquic_sys::force_link;
#[used]
static LSHPACK_FORCE_LINK: fn() = bun_lsquic_sys::force_link_lshpack;
// Same rationale: uSockets context.c needs the bao_uloop addrinfo seam
// (`Bun__addrinfo_*`) once the bun_http client surface pulls it in.
#[used]
static ULOOP_FORCE_LINK: fn() = bao_uloop::force_link;
// Same rationale: bun_core's StackCheck C seam
// (`Bun__StackCheck__initialize`) lives in bun_runtime's own
// product_native_symbols compilation unit.
#[used]
static PRODUCT_NATIVE_SYMBOLS_FORCE_LINK: fn() =
    bun_runtime::product_native_symbols::force_link_product_native_symbols;

/// Server-side TLS stream adapter over the raw TCP socket (mirror of the
/// client-side path; same drive contract: flush flights before blocking).
struct ServerTlsIo {
    tcp: TcpStream,
    tls: TlsConnection,
    /// Decrypted plaintext not yet consumed. The client's Finished and its
    /// first application-data record routinely coalesce into one TCP
    /// delivery; the `process()` pass that completes the handshake then
    /// also returns that plaintext, so it must be stashed here instead of
    /// being dropped by the handshake loop.
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl ServerTlsIo {
    fn handshake(tcp: &mut TcpStream, tls: &mut TlsConnection, pending: &mut Vec<u8>) -> std::io::Result<()> {
        loop {
            let res = tls
                .process()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            for chunk in res.plaintext {
                pending.extend_from_slice(&chunk);
            }
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

    /// Read until the full HTTP/1.1 request head (`\r\n\r\n`) is buffered.
    fn read_http_head(&mut self) -> std::io::Result<String> {
        let mut buf = [0u8; 4096];
        let mut head = Vec::new();
        loop {
            let n = self.read(&mut buf)?;
            head.extend_from_slice(&buf[..n]);
            if head.windows(4).any(|w| w == b"\r\n\r\n") {
                return Ok(String::from_utf8_lossy(&head).into_owned());
            }
        }
    }
}

impl Read for ServerTlsIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending_off < self.pending_plain.len() {
            let avail = &self.pending_plain[self.pending_off..];
            let n = avail.len().min(buf.len());
            buf[..n].copy_from_slice(&avail[..n]);
            self.pending_off += n;
            return Ok(n);
        }
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
                let n = joined.len().min(buf.len());
                buf[..n].copy_from_slice(&joined[..n]);
                return Ok(n);
            }
            let mut raw = [0u8; 16_384];
            match self.tcp.read(&mut raw) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "tls peer closed",
                    ))
                }
                Ok(n) => self.tls.feed(&raw[..n]),
                Err(e) => return Err(e),
            }
        }
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

/// Serve one TLS target connection: handshake (recording whether it
/// resumed), answer the GET, close (`Connection: close` — each request
/// gets a fresh tunnel so resumption, not tunnel pooling, is exercised).
fn serve_target_connection(
    mut tcp: TcpStream,
    server: &TlsServer,
    resumed: &Arc<Mutex<Vec<bool>>>,
) {
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut tls = match server.accept() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[tgt-server] accept failed: {}", e);
            return;
        }
    };
    let mut pending = Vec::new();
    if let Err(e) = ServerTlsIo::handshake(&mut tcp, &mut tls, &mut pending) {
        eprintln!("[tgt-server] tls handshake failed: {:?}", e);
        return;
    }
    resumed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(bao_boringssl_bridge::session_cache::session_reused(tls.ssl_ptr()));
    let mut io = ServerTlsIo {
        tcp,
        tls,
        pending_plain: pending,
        pending_off: 0,
    };
    let head = match io.read_http_head() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[tgt-server] request head read failed: {:?}", e);
            return;
        }
    };
    assert!(head.starts_with("GET /"), "expected GET request, got: {}", head);
    let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi";
    let _ = io.write_all(response.as_bytes());
    let _ = io.flush();
    // Drop closes the TCP connection (Connection: close semantics).
}

/// Serve one CONNECT proxy connection: answer the tunnel request, then
/// blind-pipe bytes between client and target in both directions.
fn serve_proxy_connection(mut client: TcpStream, target_port: u16) {
    client.set_read_timeout(Some(Duration::from_secs(10))).ok();
    // Read the CONNECT request head.
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match client.read(&mut buf) {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }
        head.extend_from_slice(&buf[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head_str = String::from_utf8_lossy(&head);
    assert!(
        head_str.starts_with(&format!("CONNECT 127.0.0.1:{target_port} ")),
        "expected CONNECT to target, got: {}",
        head_str
    );
    let mut target = match TcpStream::connect(("127.0.0.1", target_port)) {
        Ok(t) => t,
        Err(_) => return,
    };
    let _ = client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
    let _ = client.flush();
    // The pipe lives until either side reaches EOF — no read timeouts here
    // (a timeout surfaces as WouldBlock and would kill the pipe early).
    client.set_read_timeout(None).ok();
    target.set_read_timeout(None).ok();
    // Bidirectional pipe until either side reaches EOF.
    let mut c2 = match client.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut t2 = match target.try_clone() {
        Ok(t) => t,
        Err(_) => return,
    };
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut target);
    });
    let _ = std::io::copy(&mut t2, &mut client);
    let _ = up.join();
}

/// One proxied https GET via `AsyncHTTP::init_sync` with the CONNECT proxy
/// (mirrors `bun_runtime::http_client::http_request`, plus the proxy URL;
/// `reject_unauthorized=false` because the target's cert is self-signed).
fn proxied_get(target_port: u16, proxy_port: u16) -> Result<u32, String> {
    let url_str = format!("https://127.0.0.1:{target_port}/");
    let proxy_str = format!("http://127.0.0.1:{proxy_port}");
    let url = URL::parse(url_str.as_bytes());
    let proxy = URL::parse(proxy_str.as_bytes());
    let mut hb = HeaderBuilder::default();
    if let Err(e) = hb.allocate() {
        return Err(format!("header allocation failed: {:?}", e));
    }
    let entry_list = hb.entries;
    let headers_buf: &[u8] = unsafe {
        if let Some(ptr) = hb.content.ptr {
            std::slice::from_raw_parts(ptr.as_ptr(), hb.content.len)
        } else {
            &[]
        }
    };
    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut async_http = AsyncHTTP::init_sync(
        Method::GET,
        url,
        entry_list,
        headers_buf,
        response_buffer,
        b"",
        Some(proxy),
        FetchRedirect::Follow,
    );
    async_http.client.flags.reject_unauthorized = false;
    let result = async_http.send_sync().map_err(|e| format!("{:?}", e))?;
    let status = result.status_code;
    unsafe {
        drop(Box::from_raw(response_buffer));
    }
    Ok(status)
}

#[test]
fn proxy_tunnel_second_connection_resumes_session() {
    // Initialize Output streams before the HTTPThread spawns — its
    // `configure_named_thread` asserts `STDOUT_STREAM_SET` (same contract
    // as fetch_e2e_tests).
    bun_core::output::init_test();

    // TLS target: self-signed cert, one request per connection, then close.
    let (cert, key) =
        generate_self_signed_pem("127.0.0.1", 365).expect("self-signed cert");
    let target = Arc::new(TlsServer::new(&cert, &key).expect("TlsServer"));
    let target_listener = TcpListener::bind("127.0.0.1:0").expect("bind target");
    let target_port = target_listener.local_addr().unwrap().port();
    let resumed: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let target = target.clone();
        let resumed = resumed.clone();
        std::thread::spawn(move || {
            for stream in target_listener.incoming() {
                match stream {
                    Ok(s) => serve_target_connection(s, &target, &resumed),
                    Err(_) => return,
                }
            }
        });
    }

    // CONNECT proxy.
    let proxy_listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in proxy_listener.incoming() {
            match stream {
                Ok(s) => serve_proxy_connection(s, target_port),
                Err(_) => return,
            }
        }
    });

    let cache = bao_boringssl_bridge::session_cache::global();
    let origin_key =
        bao_boringssl_bridge::session_cache::origin_key("127.0.0.1", target_port, 0);
    let had_session_before = cache.contains_key(&origin_key);

    // First request: full inner handshake; its new-session callback must
    // populate the cache under the TARGET origin key (tunnel ctx
    // enable_client + offer_session key stash both proved by this hit).
    let status = proxied_get(target_port, proxy_port).expect("first proxied GET");
    assert_eq!(status, 200, "first proxied GET must succeed");
    assert!(
        cache.contains_key(&origin_key),
        "tunnel new-session callback must populate the cache for the target origin"
    );

    // Second request (fresh tunnel — target closed after the first): must
    // resume the cached session (asserted server-side per connection).
    let status = proxied_get(target_port, proxy_port).expect("second proxied GET");
    assert_eq!(status, 200, "second proxied GET must succeed");

    let flags = resumed.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(flags.len(), 2, "exactly two inner TLS connections expected");
    assert!(!flags[0], "first tunnel must be a full handshake");
    assert!(
        flags[1],
        "second tunnel to the same target origin must resume the cached session"
    );
    if !had_session_before {
        cache.clear();
    }
}
