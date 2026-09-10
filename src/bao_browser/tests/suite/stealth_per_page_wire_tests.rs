// @trace REQ-STL-001 [req:REQ-STL-001] [level:integration]
// BUN-EVOLUTION R53-A net-face live coverage: per-WebViewId stealth wire
// configuration (TLS + H2). Before R53-A the TLS wire config
// (`connector.rs STEALTH_TLS_CONFIG`) and the h2 fingerprint snapshot
// (`bao_stealth GLOBAL_HTTP2_FINGERPRINT`) were process-global
// LAST-WRITE-WINS: with two pages carrying different
// `PageConfig.stealth_profile`s, BOTH pages' egress rode the LAST-created
// page's fingerprint — silent cross-contamination inside a single runtime.
//
//   ① `per_page_divergent_wire_profiles_live` — two pages (Firefox / Chrome
//      profiles) in ONE runtime; each page's own subresource egress must
//      present ITS profile's ClientHello (profile-derived supported-groups
//      anchor — Firefox keeps P-521=25, Chrome stops at P-384=24) and the
//      two pages' JA3 strings must DIFFER. Before the fix both presented
//      the last page's profile (RED symptom: identical JA3 / wrong anchor).
//
//   ② `sw_egress_rides_host_page_profile_under_divergence_live` — the
//      SW-realm forwarded fetch (`respondWith(fetch(https://…))`) must ride
//      the REGISTERING page's profile even while a second page runs a
//      different profile (the R53-A ownership ruling: SW/worker-realm
//      egress belongs to its host page, the ScopeThings webview_id
//      inheritance made explicit on the net Request). Asserts the SW
//      ClientHello matches the host page's Firefox anchor and NOT the
//      coexisting Chrome page's.
//
// Environment gating (same as sw_stealth_profile_tests): real servo
// rendering requires DISPLAY (Xvfb) and network I/O.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(stealth_per_page)'

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use bao_stealth::StealthProfile;

/// Serializes servo-touching phases inside one isolated test process.
static TEST_SERIALIZER: Mutex<()> = Mutex::new(());

fn should_skip() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1");
        return true;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return true;
    }
    false
}

/// Drive servo's ScriptThread with a no-op evaluate (the pump pattern every
/// live harness here uses) so page-realm async dispatches progress.
fn pump(page: &bao_browser::PageHandle, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Dispatch `navigator.serviceWorker.register(path)` recording the promise
/// outcome in `window.__swReg`; poll-read afterwards (same shape as
/// sw_stealth_profile_tests).
fn register_sw(page: &bao_browser::PageHandle, path: &str) -> String {
    let js = format!(
        "window.__swReg = 'pending'; \
         try {{ \
           navigator.serviceWorker.register('{path}').then( \
             function () {{ window.__swReg = 'ok'; }}, \
             function (e) {{ window.__swReg = 'error:' + e; }}); \
         }} catch (err) {{ window.__swReg = 'threw:' + err; }} \
         window.__swReg"
    );
    let _ = page.evaluate_js_web(&js);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let settled = match page.evaluate_js_web("window.__swReg") {
            Ok(s) if s.contains("pending") => None,
            Ok(s) => Some(s),
            Err(_) => None,
        };
        if let Some(s) = settled {
            return s;
        }
        if Instant::now() > deadline {
            return "<no settlement>".to_string();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Expected JA3 supported-groups field for a profile: the profile's group
/// ids minus FFDHE (0x0100..=0x010D), which the shared BoringSSL builder
/// drops (no BoringSSL implementation — connector.rs notes). Firefox keeps
/// secp521r1 (25) → "8-29-23-24-25"; Chrome stops at secp384r1 →
/// "6-29-23-24" (the leading entry is the extension's 2-byte list length
/// parsed as a group id — the suite-wide `ja3_string` convention, which
/// cancels out in equality comparisons; here it is mirrored explicitly so
/// absolute anchors work).
fn expected_curves_field(profile: &StealthProfile) -> String {
    let groups: Vec<String> = profile
        .tls
        .supported_groups
        .iter()
        .copied()
        .filter(|id| !(0x0100..=0x010D).contains(id))
        .map(|id| id.to_string())
        .collect();
    format!("{}-{}", groups.len() * 2, groups.join("-"))
}

// ---------------------------------------------------------------------------
// ClientHello wire capture (record layer → handshake → body), copied from
// sw_stealth_profile_tests (suite-local there; duplicated here rather than
// widening this wave's test-file scope).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ClientHello {
    #[allow(dead_code)]
    legacy_version: u16,
    #[allow(dead_code)]
    random: [u8; 32],
    #[allow(dead_code)]
    session_id: Vec<u8>,
    #[allow(dead_code)]
    cipher_suites: Vec<u8>,
    #[allow(dead_code)]
    compression: Vec<u8>,
    extensions: Vec<(u16, Vec<u8>)>,
}

fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

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

    /// ALPN protocol list (extension 16), decoded, in offer order.
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

    /// JA3 string in the repo convention: `771,ciphers,extensions,curves,
    /// sigalgs`, wire order.
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

    /// The JA3 supported-groups field (`8-29-23-24-25` vs `6-29-23-24`) — the
    /// per-profile identity anchor for these tests (Firefox keeps P-521).
    fn curves_field(&self) -> String {
        self.ja3_string().split(',').nth(3).unwrap_or("").to_string()
    }
}

/// Accepts TLS connections, records each ClientHello, never replies (the
/// ClientHello is already captured when the handshake fails).
struct CaptureServer {
    #[allow(dead_code)]
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

    fn count(&self) -> usize {
        *self.signal.0.lock().unwrap()
    }

    fn first_parsed(&self) -> Option<ClientHello> {
        self.hellos
            .lock()
            .unwrap()
            .iter()
            .find_map(|r| r.clone().ok())
    }

    fn errors(&self) -> Vec<String> {
        self.hellos
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| r.clone().err())
            .collect()
    }

    fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Dispatch an `<img>` subresource to `https://127.0.0.1:<port>/<path>` on
/// the page (the proven egress shape — img/css/fetch/xhr all ride the same
/// bridge + stealth SSLConfig).
fn dispatch_img(page: &bao_browser::PageHandle, port: u16, path: &str) {
    let js = format!(
        "(function() {{ var im = document.createElement('img'); \
           im.onerror = function(){{}}; im.onload = function(){{}}; \
           document.body.appendChild(im); \
           im.src = 'https://127.0.0.1:{port}/{path}'; }})()"
    );
    let _ = page.evaluate_js_web(&js);
}

/// Wait (bounded) for `capture` to record at least one ClientHello, pumping
/// the page meanwhile.
fn await_hello(
    page: &bao_browser::PageHandle,
    capture: &CaptureServer,
    what: &str,
) -> ClientHello {
    let deadline = Instant::now() + Duration::from_secs(20);
    while capture.count() == 0 {
        assert!(
            Instant::now() <= deadline,
            "① no ClientHello captured for {what}: errors={:?}",
            capture.errors()
        );
        pump(page, 100);
    }
    capture
        .first_parsed()
        .unwrap_or_else(|| panic!("{what}: ClientHello captured but failed to parse"))
}

// ═══════════════════════════════════════════════════════════════════════════
// ① Two pages, divergent profiles — each page's egress rides ITS profile
// ═══════════════════════════════════════════════════════════════════════════

/// @trace REQ-STL-001 [criterion:REQ-STL-001] per-page stealth wire profile
/// isolation (R53-A net face, live wire capture)
///
/// ONE runtime, TWO pages: page F carries the Firefox profile, page C the
/// Chrome profile. Each page dispatches an https subresource to its own
/// capture server. The R53-A contract holds iff:
///   1. page F's ClientHello carries the FIREFOX supported-groups anchor
///      ("8-29-23-24-25" — P-521 present),
///   2. page C's carries the CHROME anchor ("6-29-23-24" — no P-521),
///   3. the two JA3 strings differ,
///   4. both ALPN lists offer `h2,http/1.1` (no h1 downgrade on either).
/// Before the fix both pages rode the LAST-installed profile (identical
/// JA3, wrong anchor on one side) — that is the RED this test pins.
#[test]
fn per_page_divergent_wire_profiles_live() {
    if !common::run_isolated(
        "stealth_per_page_wire_tests::per_page_divergent_wire_profiles_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    // The bun bridge HTTPThread's per-thread configure asserts an output
    // sink (same leg as page_net_bun_fingerprint_e2e_tests).
    bun_core::Output::init_test();

    let capture_f = CaptureServer::spawn();
    let capture_c = CaptureServer::spawn();

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");

    // Page F FIRST: its install must not be overwritten by page C's.
    let page_f = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page F must succeed");
    pump(&page_f, 300);

    let page_c = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::chrome_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page C must succeed");
    pump(&page_c, 300);

    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();
    let ff_curves = expected_curves_field(&firefox);
    let ch_curves = expected_curves_field(&chrome);
    assert_ne!(
        ff_curves, ch_curves,
        "test precondition: Firefox/Chrome profiles must diverge on the wire \
         (supported groups)"
    );

    dispatch_img(&page_f, capture_f.port, "page_f_probe");
    let hello_f = await_hello(&page_f, &capture_f, "page F (Firefox profile)");
    eprintln!(
        "[per-page] page F ja3={} curves={}",
        hello_f.ja3_string(),
        hello_f.curves_field()
    );

    dispatch_img(&page_c, capture_c.port, "page_c_probe");
    let hello_c = await_hello(&page_c, &capture_c, "page C (Chrome profile)");
    eprintln!(
        "[per-page] page C ja3={} curves={}",
        hello_c.ja3_string(),
        hello_c.curves_field()
    );

    // THE per-page assertions (R53-A): each page rides ITS OWN profile.
    assert_eq!(
        hello_f.curves_field(),
        ff_curves,
        "① R53-A VIOLATION: page F (installed FIRST, Firefox profile) must \
         present the Firefox supported groups ({ff_curves}), got \
         {} — a later Chrome page's install overwrote it (process-global \
         last-write-wins)",
        hello_f.curves_field()
    );
    assert_eq!(
        hello_c.curves_field(),
        ch_curves,
        "① R53-A VIOLATION: page C (installed LAST, Chrome profile) must \
         present the Chrome supported groups ({ch_curves}), got {}",
        hello_c.curves_field()
    );
    assert_ne!(
        hello_f.ja3_string(),
        hello_c.ja3_string(),
        "① R53-A VIOLATION: two divergent-profile pages must present \
         different JA3 fingerprints — identical JA3 means one process-global \
         wire config served both"
    );
    let h2_h1: Vec<Vec<u8>> = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    assert_eq!(hello_f.alpn_protocols(), h2_h1, "① page F ALPN must be h2,http/1.1");
    assert_eq!(
        hello_c.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "① page C ALPN must be h2,http/1.1"
    );

    capture_f.stop();
    capture_c.stop();
    eprintln!("[per-page] === ① GREEN: per-page divergent wire profiles isolated ===");
}

// ═══════════════════════════════════════════════════════════════════════════
// ② SW-realm forwarded fetch rides the REGISTERING page's profile while a
//    divergent-profile page coexists
// ═══════════════════════════════════════════════════════════════════════════

/// Minimal plain-HTTP fixture for the SW origin: `/` → page HTML,
/// `/sw.js` → the templated SW script (same shape as
/// sw_stealth_profile_tests' fixture, trimmed to the two paths used here).
struct SwOriginFixture {
    shutdown: Arc<AtomicBool>,
    sw_script: Arc<Mutex<Option<String>>>,
    port: u16,
}

impl SwOriginFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind sw origin fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let sw_script: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let shutdown_c = Arc::clone(&shutdown);
        let script_c = Arc::clone(&sw_script);
        std::thread::Builder::new()
            .name("sw-origin-fixture".into())
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
                            let body: String = if path.starts_with("/sw.js") {
                                script_c.lock().unwrap().clone().unwrap_or_default()
                            } else {
                                "<html><body>sw origin fixture</body></html>".into()
                            };
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                if path.starts_with("/sw.js") {
                                    "application/javascript"
                                } else {
                                    "text/html"
                                },
                                body.len()
                            );
                            let _ = tcp.write_all(response.as_bytes());
                            let _ = tcp.write_all(body.as_bytes());
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn sw origin fixture");
        SwOriginFixture {
            shutdown,
            sw_script,
            port,
        }
    }

    fn set_script(&self, script: String) {
        *self.sw_script.lock().unwrap() = Some(script);
    }
}

/// @trace REQ-BRW-004 [criterion:19] [level:integration] SW-realm egress
/// rides the REGISTERING page's stealth profile under multi-page profile
/// divergence (R53-A ownership ruling, live wire capture)
///
/// Page F (Firefox, SW origin) registers a SW whose fetch handler forwards
/// `/api/forward` to an https capture server. Page C (Chrome, about:blank)
/// coexists in the same runtime — created AFTER page F so the pre-R53
/// process-global fallback bucket holds the Chrome profile. The ruling
/// holds iff the SW-realm ClientHello carries the REGISTERING page's
/// Firefox anchor ("8-29-23-24-25"), NOT the Chrome one.
#[test]
fn sw_egress_rides_host_page_profile_under_divergence_live() {
    if !common::run_isolated(
        "stealth_per_page_wire_tests::sw_egress_rides_host_page_profile_under_divergence_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    let fixture = SwOriginFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    let capture_sw = CaptureServer::spawn();
    fixture.set_script(format!(
        "self.addEventListener('fetch', function (e) {{ \
           var u = String(e.request.url); \
           if (u.indexOf('/api/forward') !== -1) {{ \
             e.respondWith(fetch('https://127.0.0.1:{port}/sw_forwarded')); \
           }} \
         }});",
        port = capture_sw.port
    ));

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");

    // Host page FIRST (Firefox) — the SW's owner.
    let page_f = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page F must succeed");
    pump(&page_f, 500);

    // Divergent page C (Chrome) AFTER — its install is the process-global
    // fallback, exactly the contamination source R53-A removes.
    let page_c = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::chrome_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page C must succeed");
    pump(&page_c, 300);

    let reg = register_sw(&page_f, "/sw.js");
    eprintln!("[sw-egress] register outcome = {reg}");
    assert!(
        reg.contains("ok"),
        "② serviceWorker.register must resolve ok (live-path prerequisite), got: {reg}"
    );

    // Trigger the SW-forwarded fetch (retry-bounded: activation is
    // asynchronous and pre-activation attempts pass through natively).
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let xhr_js = "(function() { try { \
             var x = new XMLHttpRequest(); \
             x.open('GET', '/api/forward'); x.send(); \
           } catch (e) {} })()";
        let _ = page_f.evaluate_js_web(xhr_js);
        let inner_deadline = Instant::now() + Duration::from_secs(2);
        while capture_sw.count() == 0 && Instant::now() < inner_deadline && Instant::now() < deadline
        {
            pump(&page_f, 100);
        }
        if capture_sw.count() > 0 || Instant::now() > deadline {
            break;
        }
    }
    assert!(
        capture_sw.count() > 0,
        "② no ClientHello captured for the SW realm's forwarded fetch — the \
         SW sub-fetch never egressed: errors={:?}",
        capture_sw.errors()
    );
    let sw_hello = capture_sw
        .first_parsed()
        .expect("SW-forwarded ClientHello parsed");
    eprintln!(
        "[sw-egress] SW-forwarded ja3={} curves={}",
        sw_hello.ja3_string(),
        sw_hello.curves_field()
    );

    let firefox = StealthProfile::firefox_default();
    let ff_curves = expected_curves_field(&firefox);
    let ch_curves = expected_curves_field(&StealthProfile::chrome_default());

    // THE ownership assertion: the SW realm rides the REGISTERING page's
    // (Firefox) wire profile, not the coexisting Chrome page's and not a
    // stealth-free ClientHello.
    assert_eq!(
        sw_hello.curves_field(),
        ff_curves,
        "② R53-A OWNERSHIP VIOLATION: the SW-realm forwarded fetch must ride \
         the REGISTERING page's Firefox profile ({ff_curves}), got {} — \
         riding the coexisting Chrome page's profile or no stealth at all \
         means the SW egress did not resolve to its host page",
        sw_hello.curves_field()
    );
    assert_ne!(
        sw_hello.curves_field(),
        ch_curves,
        "② SW egress must NOT carry the Chrome profile of a page that never \
         registered this worker"
    );
    assert_eq!(
        sw_hello.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "② SW-forwarded ALPN must be h2,http/1.1 (stealth H2 profile, not an \
         h1 downgrade bypass)"
    );

    let _ = page_c.evaluate_js("");
    capture_sw.stop();
    eprintln!("[sw-egress] === ② GREEN: SW egress rides the host page profile under divergence ===");
}
