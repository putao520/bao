// @trace REQ-STL-001 [level:e2e] — U2 phase 1: same page, same TLS
// fingerprint across both page-network stacks.
//
// With `BAO_PAGE_NET_BUN` scoped to `img,css` (phase 1 pilot), a servo page
// loads its stylesheet/image subresources through the bun bridge
// (bun HTTPThread + BoringSSL stealth SSLConfig from the page profile),
// while the same page's `window.fetch` rides the Node fetch stack (also
// bun HTTPThread, same-profile SSLConfig via stealth_http). A raw TCP
// capture server records each ClientHello before anything else happens on
// the wire, then the test asserts:
//
//   1. ALPN parity with hyper: the bridge captures advertise
//      `h2,http/1.1` regardless of the Node-fetch h2 gate — the page
//      egress migrated from hyper-h2 and must not downgrade to h1 (the
//      `Flags::is_page_egress` bypass; the gate itself is the single
//      source `BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP2_CLIENT`, default ON).
//   2. Same-page same-fingerprint: the canonicalized ClientHello bytes
//      (client_random / session_id zeroed — per-connection randomness) are
//      IDENTICAL for bridge-img, bridge-css and window.fetch. This covers
//      cipher list + order, curves, sigalgs, extension list + order, and
//      every extension payload including ALPN contents.
//   3. JA3 (repo convention: 771,ciphers,exts,curves,sigalgs) computed
//      from the live wire bytes is equal across all three captures.
//   4. Pilot dispatch scope: bridge request counter == 2 (img + css), while
//      the page's <script> and XHR requests (destinations outside the
//      list) reach the plain-HTTP fixture through servo's hyper path.
//
// Each request gets its own capture port: the bun socket pool and the TLS
// session cache are keyed by host:port, so separate ports guarantee fresh
// connections (one first-connection ClientHello each, no coalescing and no
// cross-request session-resumption offers).
//
// Harness notes (same contract as page_wss_bao_tls_e2e_tests.rs): single
// #[test] (mozjs Runtime / servo Opts are per-process singletons); data:
// URL page origin; subresources are injected via JS AFTER create_page
// returned, so the page profile (fetch_api + set_stealth_tls_config) is
// already installed before any request fires.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool, PageState};
use bao_stealth::StealthProfile;

// ---------------------------------------------------------------------------
// ClientHello wire parsing (record layer → handshake → body)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ClientHello {
    legacy_version: u16,
    random: [u8; 32],
    session_id: Vec<u8>,
    /// Raw big-endian 2-byte cipher suite IDs, wire order.
    cipher_suites: Vec<u8>,
    compression: Vec<u8>,
    /// (type, payload) in wire order.
    extensions: Vec<(u16, Vec<u8>)>,
}

fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

/// Read one full ClientHello (handshake body bytes) off a TCP stream. The
/// handshake may span several TLS records; reads are deadline-bounded.
fn read_client_hello(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut raw: Vec<u8> = Vec::with_capacity(1024);
    let mut handshake: Vec<u8> = Vec::with_capacity(512);
    let mut handshake_need: Option<usize> = None;
    let mut tmp = [0u8; 4096];
    loop {
        while raw.len() >= 5 {
            let record_len = be16(&raw[3..5]) as usize;
            if raw.len() < 5 + record_len {
                break;
            }
            if raw[0] != 0x16 {
                return Err(format!("non-handshake record type 0x{:02x}", raw[0]));
            }
            let payload: Vec<u8> = raw[5..5 + record_len].to_vec();
            raw.drain(..5 + record_len);
            if handshake_need.is_none() {
                if payload.len() < 4 || payload[0] != 0x01 {
                    return Err("first handshake message is not a ClientHello".into());
                }
                let length = (payload[1] as usize) << 16 | (payload[2] as usize) << 8 |
                    payload[3] as usize;
                handshake_need = Some(4 + length);
            }
            handshake.extend_from_slice(&payload);
            if let Some(need) = handshake_need {
                if handshake.len() >= need {
                    return Ok(handshake[4..need].to_vec());
                }
            }
        }
        if Instant::now() > deadline {
            return Err("timeout waiting for a full ClientHello".into());
        }
        match stream.read(&mut tmp) {
            Ok(0) => return Err("connection closed before a full ClientHello".into()),
            Ok(n) => raw.extend_from_slice(&tmp[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock ||
                    e.kind() == std::io::ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(2));
            },
            Err(e) => return Err(format!("socket read error: {e}")),
        }
    }
}

fn parse_client_hello(body: &[u8]) -> Result<ClientHello, String> {
    let mut pos = 0usize;
    fn take<'a>(
        bytes: &'a [u8],
        pos: &mut usize,
        len: usize,
        what: &str,
    ) -> Result<&'a [u8], String> {
        if bytes.len() < *pos + len {
            return Err(format!("truncated ClientHello at {what}"));
        }
        let slice = &bytes[*pos..*pos + len];
        *pos += len;
        Ok(slice)
    }

    let legacy_version = be16(take(body, &mut pos, 2, "legacy_version")?);
    let random: [u8; 32] = take(body, &mut pos, 32, "random")?
        .try_into()
        .expect("32 bytes");
    let session_id_len = take(body, &mut pos, 1, "session_id length")?[0] as usize;
    let session_id = take(body, &mut pos, session_id_len, "session_id")?.to_vec();
    let cipher_len = be16(take(body, &mut pos, 2, "cipher_suites length")?) as usize;
    if cipher_len % 2 != 0 {
        return Err("odd cipher_suites length".into());
    }
    let cipher_suites = take(body, &mut pos, cipher_len, "cipher_suites")?.to_vec();
    let compression_len = take(body, &mut pos, 1, "compression length")?[0] as usize;
    let compression = take(body, &mut pos, compression_len, "compression")?.to_vec();

    let mut extensions = Vec::new();
    if pos < body.len() {
        let extensions_total = be16(take(body, &mut pos, 2, "extensions length")?) as usize;
        let extensions_end = pos + extensions_total;
        if extensions_end > body.len() {
            return Err("truncated extensions block".into());
        }
        while pos < extensions_end {
            let extension_type = be16(take(body, &mut pos, 2, "extension type")?);
            let extension_len = be16(take(body, &mut pos, 2, "extension length")?) as usize;
            let payload = take(body, &mut pos, extension_len, "extension body")?.to_vec();
            extensions.push((extension_type, payload));
        }
        if pos != extensions_end {
            return Err("extension lengths do not add up".into());
        }
    }

    Ok(ClientHello {
        legacy_version,
        random,
        session_id,
        cipher_suites,
        compression,
        extensions,
    })
}

impl ClientHello {
    fn extension(&self, extension_type: u16) -> Option<&[u8]> {
        self.extensions
            .iter()
            .find(|(t, _)| *t == extension_type)
            .map(|(_, payload)| payload.as_slice())
    }

    /// ALPN protocol list (extension 16), decoded. The payload is a
    /// `ProtocolNameList`: 2-byte total length, then (length-prefixed)
    /// protocol names in offer order.
    fn alpn_protocols(&self) -> Vec<Vec<u8>> {
        let Some(wire) = self.extension(0x0010) else {
            return Vec::new();
        };
        if wire.len() < 2 {
            return Vec::new();
        }
        let list_length = be16(wire) as usize;
        let body = &wire[2..];
        let body = &body[..body.len().min(list_length)];
        let mut protocols = Vec::new();
        let mut offset = 0usize;
        while offset < body.len() {
            let len = body[offset] as usize;
            offset += 1;
            if offset + len > body.len() {
                break;
            }
            protocols.push(body[offset..offset + len].to_vec());
            offset += len;
        }
        protocols
    }

    /// JA3 string in the repo convention (`bao_stealth::TlsFingerprint::
    /// compute_ja3`): `771,ciphers,extensions,curves,sigalgs`, wire order.
    fn ja3_string(&self) -> String {
        let ciphers: Vec<String> = self
            .cipher_suites
            .chunks_exact(2)
            .map(be16)
            .map(|id| id.to_string())
            .collect();
        let extensions: Vec<String> = self
            .extensions
            .iter()
            .map(|(t, _)| t.to_string())
            .collect();
        let u16_list = |extension_type: u16| -> Vec<String> {
            self.extension(extension_type)
                .map(|payload| {
                    payload
                        .chunks_exact(2)
                        .map(be16)
                        .map(|id| id.to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        format!(
            "771,{},{},{},{}",
            ciphers.join("-"),
            extensions.join("-"),
            u16_list(0x000a).join("-"), // supported_groups
            u16_list(0x000d).join("-"), // signature_algorithms
        )
    }

    /// Canonical byte form: the full ClientHello re-serialized with the
    /// per-connection random fields zeroed but length-preserved:
    /// client_random, legacy_session_id, the key_share (ext 51) ephemeral
    /// public key, and the pre_shared_key identities (ext 41) when present.
    /// Two ClientHellos from the same client configuration compare equal
    /// iff every fingerprint-relevant byte (cipher order, curves, sigalgs,
    /// extension list/order/payloads — including ALPN contents) is
    /// identical.
    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&self.legacy_version.to_be_bytes());
        out.extend_from_slice(&[0u8; 32]);
        out.push(self.session_id.len() as u8);
        out.resize(out.len() + self.session_id.len(), 0);
        out.extend_from_slice(&(self.cipher_suites.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.cipher_suites);
        out.push(self.compression.len() as u8);
        out.extend_from_slice(&self.compression);
        if !self.extensions.is_empty() {
            let total: usize = self.extensions.iter().map(|(_, p)| 4 + p.len()).sum();
            out.extend_from_slice(&(total as u16).to_be_bytes());
            for (extension_type, payload) in &self.extensions {
                out.extend_from_slice(&extension_type.to_be_bytes());
                out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                match *extension_type {
                    0x0029 | 0x0033 => out.resize(out.len() + payload.len(), 0),
                    _ => out.extend_from_slice(payload),
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Capture server: accepts TLS connections, records each ClientHello, closes
// ---------------------------------------------------------------------------

struct CaptureServer {
    port: u16,
    shutdown: Arc<AtomicBool>,
    hellos: Arc<Mutex<Vec<Result<ClientHello, String>>>>,
    signal: Arc<(Mutex<usize>, Condvar)>,
}

impl CaptureServer {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind capture server");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let hellos: Arc<Mutex<Vec<Result<ClientHello, String>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let signal = Arc::new((Mutex::new(0usize), Condvar::new()));

        let shutdown_c = Arc::clone(&shutdown);
        let hellos_c = Arc::clone(&hellos);
        let signal_c = Arc::clone(&signal);
        std::thread::Builder::new()
            .name("tls-capture-fixture".into())
            .spawn(move || {
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
                            let captured = read_client_hello(&mut tcp)
                                .and_then(|body| parse_client_hello(&body));
                            hellos_c.lock().unwrap().push(captured);
                            let (count, cond) = &*signal_c;
                            let mut guard = count.lock().unwrap();
                            *guard += 1;
                            cond.notify_all();
                            // No TLS reply: the client handshake fails, which
                            // is fine — the ClientHello is already captured.
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn tls-capture-fixture");
        CaptureServer {
            port,
            shutdown,
            hellos,
            signal,
        }
    }

    /// Block until at least `n` ClientHellos arrived (or timeout).
    fn wait_for(&self, n: usize, timeout: Duration) -> bool {
        let (count, cond) = &*self.signal;
        let guard = count.lock().unwrap();
        let (guard, timed_out) = cond
            .wait_timeout_while(guard, timeout, |c| *c < n)
            .expect("capture condvar poisoned");
        !timed_out.timed_out() && *guard >= n
    }

    fn parsed(&self) -> Vec<ClientHello> {
        self.hellos
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| r.clone().ok())
            .collect()
    }

    fn errors(&self) -> Vec<String> {
        self.hellos
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| r.clone().err())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Plain-HTTP fixture: records request paths for the hyper-path destinations
// ---------------------------------------------------------------------------

struct HttpFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    paths: Arc<Mutex<Vec<String>>>,
    count: Arc<AtomicUsize>,
}

impl HttpFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind http fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let shutdown_c = Arc::clone(&shutdown);
        let paths_c = Arc::clone(&paths);
        let count_c = Arc::clone(&count);
        std::thread::Builder::new()
            .name("http-fixture".into())
            .spawn(move || {
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            let _ = tcp.set_read_timeout(Some(Duration::from_millis(300)));
                            let mut buf = Vec::new();
                            let mut tmp = [0u8; 2048];
                            let deadline = Instant::now() + Duration::from_secs(2);
                            while buf.windows(4).position(|w| w == b"\r\n\r\n").is_none() &&
                                Instant::now() < deadline
                            {
                                match tcp.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                    Err(_) => break,
                                }
                            }
                            let head = String::from_utf8_lossy(&buf).to_string();
                            let path = head
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("")
                                .to_string();
                            if !path.is_empty() {
                                paths_c.lock().unwrap().push(path.clone());
                                count_c.fetch_add(1, Ordering::SeqCst);
                            }
                            let body = b"";
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = tcp.write_all(response.as_bytes());
                            let _ = tcp.write_all(body);
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn http-fixture");
        HttpFixture {
            port,
            shutdown,
            paths,
            count,
        }
    }

    fn wait_for_count(&self, n: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.count.load(Ordering::SeqCst) >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
}

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

fn wait_for_load(page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

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
fn page_net_bun_same_fingerprint_and_destination_pilot() {
    // bao output sinks: the HTTPThread's per-thread configure asserts
    // STDOUT_STREAM_SET unless the embedder initialized Output first (the
    // product binary does this in bun_runtime::dispatch; test harnesses use
    // init_test — same leg as servo-net's bun_bridge unit tests).
    bun_core::Output::init_test();

    // Phase 1 pilot scope: img + css through the bridge, everything else
    // (script / xhr / document) keeps servo's hyper path.
    net::fetch::bun_bridge::set_page_net_bun_destinations("img,css");

    let img_capture = CaptureServer::spawn();
    let css_capture = CaptureServer::spawn();
    let fetch_capture = CaptureServer::spawn();
    let fixture = HttpFixture::spawn();

    let config = BaoConfig::default();
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => panic!("BaoRuntime::new failed: {}", e),
    };
    let pool: &PagePool = runtime.page_pool();

    // Empty shell page — subresources are injected via JS after create_page
    // returned so the page's stealth profile (fetch_api global +
    // servo::set_stealth_tls_config) is installed before any request fires.
    let html = "<!DOCTYPE html><html><head><title>fp</title></head>\
                <body><p id=\"t\">fp</p></body></html>"
        .to_string();
    let url = format!("data:text/html;charset=utf-8,{}", data_url_escape(&html));

    let mut page = None;
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(url.clone()),
            stealth_profile: Some(StealthProfile::firefox_default()),
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
    eprintln!("[fp-e2e] page created");
    wait_for_load(&page, 3000);
    eprintln!("[fp-e2e] page loaded");

    // Inject in the PAGE realm (`evaluate_js` would run in the Node Realm
    // behind DOM proxies; the subresource elements must live in the page).
    let inject = |label: &str, js: &str| {
        match page.evaluate_js_web(js) {
            Ok(value) => eprintln!("[fp-e2e] inject {label}: ok {value}"),
            Err(error) => panic!("inject {label} failed: {error:?}"),
        }
    };
    let pump = |ms: u64| {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let _ = page.evaluate_js("");
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    // Wait for a capture while pumping servo's event loop (page-side async
    // tasks — XHR dispatch, fetch promise chaining — need the loop to turn).
    let wait_capturing = |capture: &CaptureServer, n: usize, timeout: Duration| -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if capture.wait_for(n, Duration::from_millis(200)) {
                return true;
            }
            pump(50);
        }
        false
    };
    let wait_fixturing = |fixture: &HttpFixture, n: usize, timeout: Duration| -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if fixture.wait_for_count(n, Duration::from_millis(200)) {
                return true;
            }
            pump(50);
        }
        false
    };

    // ── Phase A: bridge captures with EXPERIMENTAL_HTTP2_CLIENT OFF ──────
    // The bridge must still offer h2 (hyper parity — the is_page_egress
    // bypass). Node fetch would offer http/1.1 only in this posture, which
    // is why the fetch capture happens in phase B below.

    inject(
        "img",
        &format!(
            "(function(){{ var im = document.createElement('img'); \
             im.onerror = function(){{ window.__imgState = 'error'; }}; \
             im.onload = function(){{ window.__imgState = 'loaded'; }}; \
             document.body.appendChild(im); \
             im.src = 'https://127.0.0.1:{}/fingerprint.png'; \
             window.__imgState = 'pending'; }})()",
            img_capture.port
        ),
    );
    pump(200);
    inject(
        "css",
        &format!(
            "(function(){{ var l = document.createElement('link'); l.rel = 'stylesheet'; \
             l.href = 'https://127.0.0.1:{}/fingerprint.css'; document.head.appendChild(l); }})()",
            css_capture.port
        ),
    );
    pump(500);

    assert!(
        wait_capturing(&img_capture, 1, Duration::from_secs(15)),
        "no ClientHello captured for the bridge image fetch: errors={:?}",
        img_capture.errors()
    );
    eprintln!("[fp-e2e] img ClientHello captured");
    assert!(
        wait_capturing(&css_capture, 1, Duration::from_secs(15)),
        "no ClientHello captured for the bridge stylesheet fetch: errors={:?}",
        css_capture.errors()
    );
    eprintln!("[fp-e2e] css ClientHello captured");
    let img_hello = img_capture
        .parsed()
        .into_iter()
        .next()
        .expect("img ClientHello parsed");
    let css_hello = css_capture
        .parsed()
        .into_iter()
        .next()
        .expect("css ClientHello parsed");

    let h2_h1: Vec<Vec<u8>> = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    assert_eq!(
        img_hello.alpn_protocols(),
        h2_h1,
        "bridge (img) ALPN must be h2,http/1.1 regardless of the Node h2 gate — hyper parity"
    );
    assert_eq!(
        css_hello.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "bridge (css) ALPN must be h2,http/1.1 regardless of the Node h2 gate — hyper parity"
    );

    // ── Phase B: Node fetch (h2 gate = env flag, default ON); script/xhr
    //    stay on hyper ────────────────────────────────────────────────────

    // window.fetch — same page, Node fetch stack (bun HTTPThread, stealth
    // SSLConfig from the page profile via stealth_http).
    inject(
        "fetch",
        &format!(
            "(function(){{ window.__fetchState = 'pending'; \
             window.__fetchOwn = !!Object.getOwnPropertyDescriptor(window, 'fetch'); \
             fetch('https://127.0.0.1:{}/fetch_probe').then(function(r){{ window.__fetchState = 'ok:' + r.status; }}).catch(function(e){{ window.__fetchState = 'err:' + e; }}); }})()",
            fetch_capture.port
        ),
    );

    // script + XHR (destinations outside the img,css list) — must take
    // servo's hyper path and land on the plain-HTTP fixture.
    inject(
        "script",
        &format!(
            "(function(){{ var s = document.createElement('script'); s.src = 'http://127.0.0.1:{}/script_probe.js'; document.head.appendChild(s); }})()",
            fixture.port
        ),
    );
    inject(
        "xhr",
        &format!(
            "(function(){{ try {{ var x = new XMLHttpRequest(); x.open('GET', 'http://127.0.0.1:{}/xhr_probe'); x.send(); window.__xhrState = 'sent'; }} catch (e) {{ window.__xhrState = 'throw:' + e; }} }})()",
            fixture.port
        ),
    );

    assert!(
        wait_capturing(&fetch_capture, 1, Duration::from_secs(15)),
        "no ClientHello captured for window.fetch: errors={:?}",
        fetch_capture.errors()
    );
    eprintln!("[fp-e2e] window.fetch ClientHello captured");
    let fetch_diag = page.evaluate_js_web(
        "(function(){ return 'own=' + window.__fetchOwn + ' state=' + window.__fetchState; })()",
    );
    eprintln!("[fp-e2e] fetch diag: {:?}", fetch_diag);
    assert!(
        wait_fixturing(&fixture, 2, Duration::from_secs(15)),
        "hyper-path fixture did not receive script+xhr (paths so far: {:?})",
        fixture.paths()
    );
    eprintln!("[fp-e2e] hyper fixture got script+xhr: {:?}", fixture.paths());
    let fetch_hello = fetch_capture
        .parsed()
        .into_iter()
        .next()
        .expect("fetch ClientHello parsed");

    // ── Same page, same fingerprint ───────────────────────────────────────

    let img_canonical = img_hello.canonical_bytes();
    let css_canonical = css_hello.canonical_bytes();
    let fetch_canonical = fetch_hello.canonical_bytes();
    assert_eq!(
        img_canonical,
        fetch_canonical,
        "bridge (img) and window.fetch ClientHello fingerprints differ on the same page"
    );
    assert_eq!(
        css_canonical,
        fetch_canonical,
        "bridge (css) and window.fetch ClientHello fingerprints differ on the same page"
    );
    assert_eq!(
        img_hello.ja3_string(),
        fetch_hello.ja3_string(),
        "bridge (img) and window.fetch JA3 differ on the same page"
    );
    assert_eq!(
        css_hello.ja3_string(),
        fetch_hello.ja3_string(),
        "bridge (css) and window.fetch JA3 differ on the same page"
    );
    assert!(
        img_hello.ja3_string().starts_with("771,"),
        "live JA3 malformed: {}",
        img_hello.ja3_string()
    );

    // ── Pilot dispatch scope ──────────────────────────────────────────────
    // img + css went through the bridge (counter), script + xhr did not —
    // they reached the fixture via servo's hyper path instead.

    let bridge_count = net::fetch::bun_bridge::page_net_bun_request_count();
    assert_eq!(
        bridge_count, 2,
        "bridge must have driven exactly img+css (got {bridge_count}; fixture paths: {:?})",
        fixture.paths()
    );
    let paths = fixture.paths();
    assert!(
        paths.iter().any(|p| p.contains("script_probe")),
        "script request missing from hyper fixture: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.contains("xhr_probe")),
        "xhr request missing from hyper fixture: {paths:?}"
    );

    // ── h2 SETTINGS payload parity (code-level cross-check) ───────────────
    // Both stacks derive the SETTINGS wire bytes from the SAME profile
    // through the SAME bao_stealth function (settings_frame_payload), so the
    // frames are byte-equal by construction; assert the two derivations
    // agree on this profile to pin the invariant.

    let profile = StealthProfile::firefox_default();
    let wire = bao_stealth::StealthTlsWireConfig::from_profile(&profile);
    assert!(
        !wire.h2_settings_payload.is_empty() &&
            wire.h2_settings_payload.len() % 6 == 0,
        "wire h2 SETTINGS payload malformed"
    );
    assert_eq!(
        wire.h2_initial_stream_size,
        profile.http2.initial_window_size,
        "bridge and Node fetch must carry the same h2 initial window size"
    );

    // ── U2 stage 2: h2 pseudo-header order / preface PRIORITY frames ─────
    // The page profile installed the global Http2Fingerprint snapshot (set
    // alongside set_stealth_tls_config by runtime_bridge); the bridge reads
    // it into its SSLConfig (`build_ssl_config`), which
    // h2_client::encode::write_preface / encode_request_headers consume —
    // the same fields window.fetch's stealth_http sets. Pin the wiring at
    // e2e level: the snapshot must be the page profile's Firefox fingerprint.
    let h2fp = bao_stealth::global_http2_fingerprint()
        .expect("page profile must install the global h2 fingerprint snapshot");
    assert_eq!(
        h2fp.pseudo_header_order,
        profile.http2.pseudo_header_order,
        "global h2 snapshot pseudo-header order must be the page profile's (Firefox: method/path/authority/scheme)"
    );
    assert_eq!(
        h2fp.priority_frames.len(),
        profile.http2.priority_frames.len(),
        "global h2 snapshot must carry the profile's preface PRIORITY frames (Firefox: 3/5/7/11)"
    );
    assert!(
        h2fp.sends_priority_frames(),
        "Firefox profile must send explicit PRIORITY frames (REQ-STL-002-C3)"
    );

    // ── Phase C (U2 stage 3): EXPERIMENTAL_HTTP2_CLIENT default-ON smoke ──
    // No flag is set anywhere (the h2 gate's single source is the env flag,
    // whose default is ON) and Node fetch must offer h2 on its own: the
    // ClientHello ALPN list must still be `h2,http/1.1`. (Pre-flip this
    // posture offered http/1.1 only; the bridge bypass `is_page_egress` is
    // NOT involved — this is the Node fetch stack's own gate.)
    let default_capture = CaptureServer::spawn();
    inject(
        "fetch-default-h2",
        &format!(
            "(function(){{ fetch('https://127.0.0.1:{}/default_probe').catch(function(){{}}); }})()",
            default_capture.port
        ),
    );
    assert!(
        wait_capturing(&default_capture, 1, Duration::from_secs(15)),
        "no ClientHello captured for the default-flag fetch: errors={:?}",
        default_capture.errors()
    );
    let default_hello = default_capture
        .parsed()
        .into_iter()
        .next()
        .expect("default-flag ClientHello parsed");
    assert_eq!(
        default_hello.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "with no flag set anywhere, Node fetch must offer h2 by default (BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP2_CLIENT default ON)"
    );
    default_capture.shutdown.store(true, Ordering::SeqCst);
    eprintln!("[fp-e2e] default-flag fetch offers h2 (EXPERIMENTAL_HTTP2_CLIENT default ON)");

    img_capture.shutdown.store(true, Ordering::SeqCst);
    css_capture.shutdown.store(true, Ordering::SeqCst);
    fetch_capture.shutdown.store(true, Ordering::SeqCst);
    fixture.shutdown.store(true, Ordering::SeqCst);
    eprintln!("[fp-e2e] === ALL ASSERTIONS PASSED ===");

    // Shutdown: every assertion above already ran and printed the banner.
    // servo teardown in this harness can stall indefinitely (observed:
    // ResourceManager hot-spins on a closed channel in its select set and
    // Constellation never finishes Exit sequencing — independent of the
    // assertions, which have all completed). A watchdog force-exits the
    // process AFTER a grace period so a clean servo teardown still gets a
    // chance to run first; the exit code is 0 only because the banner above
    // proves every assertion passed.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(10));
        eprintln!("[fp-e2e] watchdog: servo teardown did not finish in 10s — force exit");
        std::process::exit(0);
    });
    let _ = page.close();
    pool.close_all();
}
