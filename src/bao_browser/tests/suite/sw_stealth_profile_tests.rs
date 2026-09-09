// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:19] [level:integration]
// REQ-BRW-004 C19 three-subclause live coverage (V-batch v47 ③: the SPEC
// subclauses beyond the already-green mediation/controller mechanics had
// ZERO tests — this file lands one live test per subclause):
//
//   ① `c19_sub1_…` — "SW 拦截并转发的 fetch 仍走主页同一 stealth TLS(JA3/JA4)
//      +HTTP2(AKAMAI) profile (不绕过反指纹)": a raw-TCP ClientHello capture
//      server records the wire fingerprint of BOTH the page's own
//      subresource egress AND the SW realm's forwarded fetch
//      (`respondWith(fetch('https://…'))`); the canonicalized ClientHello
//      bytes and the JA3 string must be IDENTICAL, ALPN must offer
//      `h2,http/1.1`, and the global h2 fingerprint snapshot both stacks
//      read must be the page profile's (AKAMAI pseudo-header order +
//      preface PRIORITY frames).
//
//   ② `c19_sub2_…` — "CDP Network 域可观测 SW 发起的请求/响应": a live
//      CdpServer (the production BaoWsRegistry wiring) + WS client enables
//      Network, the page drives an SW-intercepted fetch (respondWith 201),
//      and the client asserts Network.requestWillBeSent /
//      responseReceived events carrying the intercepted URL arrive on the
//      WS event stream.
//
//   ③ `c19_sub3_…` — "SW 持久生命周期(跨页存活)下 profile 继承注册页且
//      terminate 后正确注销": the SW realm's navigator.userAgent (read at
//      fetch-handling time inside respondWith) must equal the REGISTERING
//      page's profile UA; a SECOND page in the same scope must be mediated
//      by the SAME live worker (cross-page survival); and after
//      getRegistration().unregister() (the vendor terminate path — the
//      manager drops the registration and terminates the worker thread
//      before resolving, per serviceworker_controller_tests ④) new requests
//      must receive the NATIVE response (mediation deregistered).
//
//   ④ `sw_scope_injector_starvation_…` — SW scope injector starvation: a
//      same-page DEDICATED Worker created BEFORE `serviceWorker.register`
//      consumes the consume-once worker-scope queue, so the later-registered
//      SW scope drained an EMPTY one-shot queue and (before the per-Worker
//      injector tier was extended to the SW path) ran with ZERO embedder
//      injection — a bare fingerprintable SW realm. The SW scope must still
//      be FULLY injected in that timing: engine getter (ua === page profile
//      UA, permanent accessor) AND the W1a JS hooks (audio getChannelData /
//      webgl getParameter, delivered at the SW's own post-define point) with
//      the double-define guards holding (e36 gate; hooks blob exactly once).
//
// Contract discipline (V-batch release item): these tests assert the SPEC
// subclauses as written. A subclause that does not hold on the live path
// goes RED here with the gap named — product code is NOT touched from this
// file.
//
// Environment gating (same as serviceworker_mediation_tests): real servo
// rendering requires DISPLAY (Xvfb) and network I/O.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(sw_stealth_profile)'

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
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

fn wait_for<F: Fn() -> Option<T>, T>(poll: F, timeout: Duration, what: &str) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(v) = poll() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    eprintln!("[timeout] {what}");
    None
}

/// The servo JS bridge may return a JS string value as `"..."` (quoted).
/// Strip one outer quote pair if present (same helper as
/// worker_multi_injection_tests).
fn unquote_bridge(mut s: String) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        s = inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    s
}

/// Page-realm probe via SYNCHRONOUS XHR (the verdict transport proven by
/// serviceworker_mediation_tests): returns `status=… body=…`.
fn one_probe(page: &bao_browser::PageHandle, path: &str) -> String {
    let js = format!(
        "var __x = new XMLHttpRequest(); \
         __x.open('GET', '{path}', false); \
         var __o = '{path} '; \
         try {{ \
           __x.send(null); \
           __o += 'status=' + __x.status + ' body=' + __x.responseText; \
         }} catch (e) {{ __o += 'THREW:' + e; }} \
         __o;"
    );
    match page.evaluate_js_web(&js) {
        Ok(verdict) => verdict,
        Err(e) => format!("{path} EVAL-ERR:{e}"),
    }
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
/// outcome in `window.__swReg`; poll-read afterwards (same shape as the
/// mediation/controller tests — the dispatch JS resets the sink, so the
/// settlement is observed by a SEPARATE read-only evaluate).
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
    wait_for(
        || match page.evaluate_js_web("window.__swReg") {
            Ok(s) if s.contains("pending") => None,
            Ok(s) => Some(s),
            Err(_) => None,
        },
        Duration::from_secs(20),
        "serviceWorker.register promise settlement",
    )
    .unwrap_or_else(|| "<no settlement>".to_string())
}

// ---------------------------------------------------------------------------
// Plain-HTTP fixture: `/` → page HTML, `/sw.js` → templated SW script,
// `/api/*` → native marker bodies, everything else recorded.
// (Same shape as serviceworker_mediation_tests' fixture.)
// ---------------------------------------------------------------------------

struct SwC19HttpFixture {
    shutdown: Arc<AtomicBool>,
    paths: Arc<Mutex<Vec<String>>>,
    sw_script: Arc<Mutex<Option<String>>>,
    port: u16,
}

impl SwC19HttpFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind sw c19 fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sw_script: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let shutdown_c = Arc::clone(&shutdown);
        let paths_c = Arc::clone(&paths);
        let script_c = Arc::clone(&sw_script);
        std::thread::Builder::new()
            .name("sw-c19-fixture".into())
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
                            paths_c.lock().unwrap().push(path.clone());
                            let sw_script_body = script_c.lock().unwrap().clone();
                            let (content_type, body): (&str, String) =
                                if path.starts_with("/sw.js") {
                                    match sw_script_body {
                                        Some(script) => ("application/javascript", script),
                                        None => ("text/plain", "sw script not set".into()),
                                    }
                                } else if path.starts_with("/api/forward") {
                                    ("text/plain", "NATIVE_FORWARD_BODY".into())
                                } else if path.starts_with("/api/data") {
                                    ("text/plain", "NATIVE_DATA_BODY".into())
                                } else if path.starts_with("/api/inherit") {
                                    ("text/plain", "NATIVE_INHERIT_BODY".into())
                                } else {
                                    (
                                        "text/html",
                                        "<html><body>sw-c19 fixture</body></html>".into(),
                                    )
                                };
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
                                ct = content_type,
                                len = body.len()
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
            .expect("spawn sw c19 fixture thread");
        SwC19HttpFixture {
            shutdown,
            paths,
            sw_script,
            port,
        }
    }

    fn set_script(&self, script: String) {
        *self.sw_script.lock().unwrap() = Some(script);
    }

    fn recorded_paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
}

impl Drop for SwC19HttpFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// ClientHello wire capture (record layer → handshake → body), copied from
// page_net_bun_fingerprint_e2e_tests (suite-local there; duplicated here
// rather than widening this wave's test-file scope).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ClientHello {
    legacy_version: u16,
    #[allow(dead_code)]
    random: [u8; 32],
    session_id: Vec<u8>,
    cipher_suites: Vec<u8>,
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
    /// per-connection random fields zeroed but length-preserved
    /// (client_random, session_id, key_share and pre_shared_key payloads).
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

/// Accepts TLS connections, records each ClientHello, never replies (the
/// ClientHello is already captured when the handshake fails).
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

impl Drop for CaptureServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// ① Sub-clause 1 — SW-forwarded fetch rides the page's stealth TLS+H2 profile
// ═══════════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:19] SW-forwarded fetch stealth TLS/H2
/// profile equivalence (live wire capture)
///
/// The SW intercepts `/api/forward` and answers
/// `respondWith(fetch('https://127.0.0.1:<sw_capture>/sw_forwarded'))` —
/// the SW realm's OWN fetch. A direct page subresource (`<img>`) to a
/// second capture server provides the main-page-path baseline. The SPEC
/// subclause holds iff the SW-forwarded ClientHello is byte-identical
/// (canonical form: random/key_share zeroed) to the page-path ClientHello,
/// with JA3 equal and ALPN offering h2 — i.e. the SW-forwarded fetch did
/// NOT bypass the page's stealth TLS profile. The H2(AKAMAI) side is pinned
/// by the global h2 fingerprint snapshot (pseudo-header order + preface
/// PRIORITY frames), the single source both stacks' h2 encoding reads.
///
/// The capture servers never complete the TLS handshake: the SW sub-fetch's
/// settlement may wedge (documented pre-existing engine defect) but its
/// EGRESS — the ClientHello — is on the wire, which is the entire
/// assertion surface here (same egress-not-settlement discipline as the
/// mediation test's ③).
#[test]
fn c19_sub1_sw_forwarded_fetch_rides_page_tls_h2_profile_live() {
    if !common::run_isolated("sw_stealth_profile_tests::c19_sub1_sw_forwarded_fetch_rides_page_tls_h2_profile_live") {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    // The bun bridge HTTPThread's per-thread configure asserts an output
    // sink (same leg as page_net_bun_fingerprint_e2e_tests).
    bun_core::Output::init_test();

    let fixture = SwC19HttpFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    let direct_capture = CaptureServer::spawn();
    let sw_capture = CaptureServer::spawn();
    // The SW's forwarded fetch target: an HTTPS capture server. Templated
    // AFTER the port is known (the accept thread serves it before any
    // /sw.js request can arrive).
    fixture.set_script(format!(
        "self.addEventListener('fetch', function (e) {{ \
           var u = String(e.request.url); \
           if (u.indexOf('/api/forward') !== -1) {{ \
             e.respondWith(fetch('https://127.0.0.1:{port}/sw_forwarded')); \
           }} \
         }});",
        port = sw_capture.port
    ));

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    // Let the page load settle before dispatching probes.
    pump(&page, 500);

    // Register the SW; the register promise must resolve on the live path.
    let reg = register_sw(&page, "/sw.js");
    eprintln!("[c19-sub1] register outcome = {reg}");
    assert!(
        reg.contains("ok"),
        "① serviceWorker.register must resolve ok (live-path prerequisite), got: {reg}"
    );

    // Baseline: the PAGE's own subresource path — an <img> to the direct
    // capture (the proven egress shape from the fingerprint e2e; img/css/
    // fetch/xhr all ride the same bridge + stealth SSLConfig).
    let img_js = format!(
        "(function() {{ var im = document.createElement('img'); \
           im.onerror = function(){{}}; im.onload = function(){{}}; \
           document.body.appendChild(im); \
           im.src = 'https://127.0.0.1:{}/direct_page_probe'; }})()",
        direct_capture.port
    );
    page.evaluate_js_web(&img_js)
        .expect("direct baseline img dispatch must not fail");
    let direct_arrived = {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if direct_capture.count() >= 1 {
                break true;
            }
            if Instant::now() > deadline {
                break false;
            }
            pump(&page, 100);
        }
    };
    assert!(
        direct_arrived,
        "① no ClientHello captured for the page's direct subresource path: errors={:?}",
        direct_capture.errors()
    );
    let direct_hello = direct_capture
        .parsed()
        .into_iter()
        .next()
        .expect("direct ClientHello parsed");
    eprintln!(
        "[c19-sub1] direct-path ClientHello captured: ja3={}",
        direct_hello.ja3_string()
    );

    // Trigger the SW-forwarded fetch: an async page XHR to /api/forward
    // (in scope) → net-layer mediation → SW realm FetchEvent →
    // respondWith(fetch('https://…/sw_forwarded')) → SW sub-fetch egress.
    // Retry-bounded: activation is asynchronous, and pre-activation
    // attempts pass through to the native fixture body (harmless).
    let sw_arrived = {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let xhr_js = "(function() { try { \
                 var x = new XMLHttpRequest(); \
                 x.open('GET', '/api/forward'); x.send(); \
               } catch (e) {} })()";
            let _ = page.evaluate_js_web(xhr_js);
            let inner_deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if sw_capture.count() >= 1 {
                    break;
                }
                if Instant::now() > inner_deadline || Instant::now() > deadline {
                    break;
                }
                pump(&page, 100);
            }
            if sw_capture.count() >= 1 || Instant::now() > deadline {
                break sw_capture.count() >= 1;
            }
        }
    };
    assert!(
        sw_arrived,
        "① no ClientHello captured for the SW realm's forwarded fetch — the SW \
         sub-fetch never egressed (mediation activation or SW fetch dispatch \
         broken): sw_capture errors={:?}, fixture paths={:?}",
        sw_capture.errors(),
        fixture.recorded_paths()
    );
    let sw_hello = sw_capture
        .parsed()
        .into_iter()
        .next()
        .expect("SW-forwarded ClientHello parsed");
    eprintln!(
        "[c19-sub1] SW-forwarded ClientHello captured: ja3={}",
        sw_hello.ja3_string()
    );

    // THE subclause assertion: same page → same stealth TLS profile. The
    // SW-forwarded fetch must present the IDENTICAL ClientHello
    // (cipher list + order, curves, sigalgs, extension list + order +
    // payloads incl. ALPN) as the page's own subresource path.
    assert_eq!(
        direct_hello.canonical_bytes(),
        sw_hello.canonical_bytes(),
        "① STEALTH BOUNDARY VIOLATION: the SW-forwarded fetch's ClientHello \
         differs from the page-path ClientHello (canonical bytes; random/\
         key_share zeroed) — SW fetch is NOT riding the page's stealth TLS \
         profile. direct ja3={} vs sw ja3={}",
        direct_hello.ja3_string(),
        sw_hello.ja3_string()
    );
    assert_eq!(
        direct_hello.ja3_string(),
        sw_hello.ja3_string(),
        "① SW-forwarded fetch JA3 must equal the page-path JA3"
    );
    assert!(
        sw_hello.ja3_string().starts_with("771,"),
        "① live SW-path JA3 malformed: {}",
        sw_hello.ja3_string()
    );
    // H2 offer parity: both paths must advertise `h2,http/1.1` (the
    // stealth ALPN order; http/1.1-only would be a downgrade bypass).
    let h2_h1: Vec<Vec<u8>> = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    assert_eq!(
        direct_hello.alpn_protocols(),
        h2_h1,
        "① page-path ALPN must be h2,http/1.1"
    );
    assert_eq!(
        sw_hello.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "① SW-forwarded ALPN must be h2,http/1.1 (stealth H2 profile, not an \
         h1 downgrade bypass)"
    );

    // H2(AKAMAI) profile wiring: both stacks' h2 encoding reads the single
    // global snapshot installed from the page profile — pin that the
    // snapshot IS the page profile's fingerprint (pseudo-header order +
    // preface PRIORITY frames; same cross-check as the fingerprint e2e).
    let profile = StealthProfile::firefox_default();
    let h2fp = bao_stealth::global_http2_fingerprint()
        .expect("page profile must install the global h2 fingerprint snapshot");
    assert_eq!(
        h2fp.pseudo_header_order, profile.http2.pseudo_header_order,
        "① global h2 snapshot pseudo-header order must be the page profile's \
         (Firefox AKAMAI order)"
    );
    assert_eq!(
        h2fp.priority_frames.len(),
        profile.http2.priority_frames.len(),
        "① global h2 snapshot must carry the profile's preface PRIORITY frames"
    );

    direct_capture.shutdown.store(true, Ordering::SeqCst);
    sw_capture.shutdown.store(true, Ordering::SeqCst);
    eprintln!("[c19-sub1] === SUBCLAUSE ① GREEN: SW-forwarded fetch rides the page TLS+H2 profile ===");
}

// ═══════════════════════════════════════════════════════════════════════════
// ② Sub-clause 2 — CDP Network domain observability of the SW-intercepted fetch
// ═══════════════════════════════════════════════════════════════════════════

/// Minimal WS CDP client (same shape as cdp_ws_command_face_tests' WsCdp,
/// plus an event collector).
struct WsCdpCollector {
    client: bun_uws::ws_client::WebSocketClient,
    next_id: i64,
    /// Every event frame received (method, params).
    events: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

impl WsCdpCollector {
    fn connect(url: &str) -> Self {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut client = None;
        while Instant::now() < deadline {
            match bun_uws::ws_client::WebSocketClient::connect(url) {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        let mut client = client.expect("ws connect (bounded 10s retry exhausted)");
        client.set_read_timeout(Duration::from_millis(200));
        WsCdpCollector {
            client,
            next_id: 1,
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Send a command and wait for the matching response id, banking every
    /// event frame seen along the way into `self.events`.
    fn send(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = serde_json::json!({ "id": id, "method": method });
        if !params.is_null() {
            msg["params"] = params;
        }
        self.client
            .send_text(&serde_json::to_string(&msg).unwrap())
            .expect("ws send");
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                bun_uws::ws_client::RecvOutcome::Message(_op, payload) => {
                    let v: serde_json::Value =
                        serde_json::from_slice(&payload).expect("valid json frame");
                    if v.get("id").and_then(|i| i.as_i64()) == Some(id) {
                        return v;
                    }
                    if let Some(m) = v.get("method").and_then(|m| m.as_str()) {
                        self.events
                            .lock()
                            .unwrap()
                            .push((m.to_string(), v.get("params").cloned().unwrap_or_default()));
                    }
                },
                bun_uws::ws_client::RecvOutcome::Timeout => continue,
                bun_uws::ws_client::RecvOutcome::Closed => {
                    panic!("ws closed waiting for {method} response")
                },
            }
        }
        panic!("timeout waiting for {method} response");
    }

    /// Drain the socket for `dur`, banking event frames.
    fn collect_events(&mut self, dur: Duration) {
        let deadline = Instant::now() + dur;
        while Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                bun_uws::ws_client::RecvOutcome::Message(_op, payload) => {
                    let v: serde_json::Value =
                        serde_json::from_slice(&payload).expect("valid json frame");
                    if let Some(m) = v.get("method").and_then(|m| m.as_str()) {
                        self.events
                            .lock()
                            .unwrap()
                            .push((m.to_string(), v.get("params").cloned().unwrap_or_default()));
                    }
                },
                bun_uws::ws_client::RecvOutcome::Timeout => continue,
                bun_uws::ws_client::RecvOutcome::Closed => return,
            }
        }
    }
}

/// Extract a URL from Network event params (requestWillBeSent carries
/// params.request.url; responseReceived carries params.response.url).
fn network_event_url(params: &serde_json::Value) -> String {
    params
        .get("request")
        .and_then(|r| r.get("url"))
        .and_then(|u| u.as_str())
        .or_else(|| {
            params
                .get("response")
                .and_then(|r| r.get("url"))
                .and_then(|u| u.as_str())
        })
        .unwrap_or("")
        .to_string()
}

/// @trace REQ-BRW-004 [criterion:19] CDP Network domain observability of the
/// SW-intercepted fetch (live WS face)
///
/// Live CdpServer (the production BaoWsRegistry wiring used by
/// run_browser / cdp_ws_command_face_tests) + WS client: Network.enable,
/// then the page registers the SW (intercepts `/api/data` with
/// respondWith(new Response(..., {status: 201}))) and drives a mediated
/// fetch to a verified 201 verdict. The subclause holds iff
/// Network.requestWillBeSent and Network.responseReceived for the
/// intercepted URL arrive on the WS event stream (status 201 on the
/// response — the MEDIATED response, proving the observation covers the
/// interception, not just the native egress).
#[test]
fn c19_sub2_cdp_network_observability_of_sw_intercepted_fetch_live() {
    if !common::run_isolated(
        "sw_stealth_profile_tests::c19_sub2_cdp_network_observability_of_sw_intercepted_fetch_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    // The SW intercepts /api/data with a synthetic 201 (the mediation
    // test's ① shape).
    let fixture = SwC19HttpFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    fixture.set_script(
        "self.addEventListener('fetch', function (e) { \
           var u = String(e.request.url); \
           if (u.indexOf('/api/data') !== -1) { \
             e.respondWith(new Response('CDP_OBSERVABLE_BODY', { \
               status: 201, \
               statusText: 'Made By SW', \
               headers: { 'Content-Type': 'text/plain', 'X-Sw-Intercepted': 'yes' } \
             })); \
           } \
         });"
        .to_string(),
    );

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    pump(&page, 500);

    // CDP server wiring — the production shape from cdp_ws_command_face.
    use bao_browser::{handle_bridge_command, BaoWsRegistry};
    use bao_cdp::domains::ServoTargetProvider;
    use bao_cdp::servo_bridge::bridge_channel;
    use cdp_server::{CdpServer, EventSender, ServerConfig};

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));
    let (event_subscriber, servo_event_rx) = bao_cdp_client::bridge::EventSubscriber::new();
    runtime.set_event_channel(event_subscriber.sender());

    let registry = Arc::new(BaoWsRegistry::new(bridge_tx.clone()));
    let port = {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    };
    let server_config = ServerConfig::builder()
        .host("127.0.0.1")
        .port(port)
        .build();
    let mut server = CdpServer::with_registry(server_config, registry);
    server.set_target_provider(Arc::new(ServoTargetProvider::new(
        bridge_tx,
        page.id().to_string(),
        "127.0.0.1".into(),
        port,
    )));
    let broadcaster = server.broadcaster();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page.id());

    // Coordination flags between the WS client thread and the main
    // (servo-driving) thread.
    let net_enabled = Arc::new(AtomicBool::new(false));
    let fetch_done = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));

    // ── WS client phase ──────────────────────────────────────────────────
    let client = {
        let ws_url = ws_url.clone();
        let net_enabled = Arc::clone(&net_enabled);
        let fetch_done = Arc::clone(&fetch_done);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut cdp = WsCdpCollector::connect(&ws_url);
            let resp = cdp.send("Network.enable", serde_json::json!({}));
            assert!(
                resp.get("error").is_none(),
                "② Network.enable must succeed on the WS face: {resp}"
            );
            net_enabled.store(true, Ordering::Relaxed);

            // Collect while the main thread drives the SW-mediated fetch,
            // then a bounded grace window after the fetch verb'd 201.
            let deadline = Instant::now() + Duration::from_secs(75);
            while Instant::now() < deadline && !fetch_done.load(Ordering::Relaxed) {
                cdp.collect_events(Duration::from_millis(200));
            }
            cdp.collect_events(Duration::from_secs(10));

            let events = cdp.events.lock().unwrap().clone();
            let all_methods: Vec<String> =
                events.iter().map(|(m, _)| m.clone()).collect();
            let channel_marker_seen = events.iter().any(|(m, p)| {
                m == "Log.entryAdded" &&
                    p.to_string().contains("c19-sub2-channel-marker")
            });
            eprintln!(
                "[c19-sub2] collected {} events: {:?} (console control marker seen: {channel_marker_seen})",
                events.len(),
                all_methods
            );

            let rws = events.iter().find(|(m, p)| {
                m == "Network.requestWillBeSent" &&
                    network_event_url(p).contains("/api/data")
            });
            let rrs = events.iter().find(|(m, p)| {
                m == "Network.responseReceived" &&
                    network_event_url(p).contains("/api/data")
            });

            assert!(
                rws.is_some(),
                "② SPEC SUBCLAUSE GAP: no Network.requestWillBeSent observed for \
                 the SW-intercepted /api/data fetch on the live CDP WS face \
                 (Network.enable acknowledged; the mediated 201 itself reached \
                 the page). Collected events: {all_methods:?}; console control \
                 marker seen: {channel_marker_seen}. Gap layer: no live \
                 Network-event emitter — bao_browser cdp_handler's NetworkEnable \
                 is a no-op ok (src/bao_browser/src/cdp_handler.rs) and the \
                 delegate event path emits only Console/PageError/Frame* \
                 ServoEvents (src/bao_browser/src/delegate.rs); the SW fetch \
                 path (and page fetches generally) have no net-layer event tap \
                 emitting Network.requestWillBeSent/responseReceived."
            );
            let (m, p) = rws.expect("checked above");
            let url = network_event_url(p);
            assert!(
                url.contains("/api/data"),
                "② requestWillBeSent URL must identify the intercepted request: {url}"
            );
            let _ = m;
            assert!(
                rrs.is_some(),
                "② SPEC SUBCLAUSE GAP: requestWillBeSent observed but no \
                 Network.responseReceived for /api/data — response-side \
                 observability missing. Collected events: {all_methods:?}"
            );
            let (_m2, p2) = rrs.expect("checked above");
            let status = p2
                .get("response")
                .and_then(|r| r.get("status"))
                .and_then(|s| s.as_i64());
            assert_eq!(
                status,
                Some(201),
                "② responseReceived must carry the MEDIATED 201 status (the SW's \
                 respondWith answer), proving the observation covers the \
                 interception: params={p2}"
            );
            done.store(true, Ordering::Relaxed);
        })
    };

    // ── Main thread: servo spin + bridge drain + event translation, and
    //    the page driving once Network.enable is acknowledged ────────────
    use bao_cdp_client::bridge::translate;
    let spin_once = |collect: bool| {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        if collect {
            while let Ok(servo_event) = servo_event_rx.try_recv() {
                for cdp_event in translate(servo_event) {
                    broadcaster.send_event(&cdp_event.method, cdp_event.params);
                }
            }
        }
    };

    // Wait for the client's Network.enable (bounded), spinning.
    {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !net_enabled.load(Ordering::Relaxed) {
            if Instant::now() > deadline {
                panic!("② WS client never acknowledged Network.enable");
            }
            spin_once(true);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // Drive the SW registration + mediated fetch to a verified 201 (the
    // mediation test's ① retry shape — activation is asynchronous).
    let reg = register_sw(&page, "/sw.js");
    eprintln!("[c19-sub2] register outcome = {reg}");
    assert!(
        reg.contains("ok"),
        "② serviceWorker.register must resolve ok (live-path prerequisite), got: {reg}"
    );
    let mediated = {
        let mut verdict = String::new();
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            spin_once(true);
            verdict = one_probe(&page, "/api/data");
            eprintln!("[c19-sub2] attempt = {verdict}");
            if verdict.contains("status=201") || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        verdict
    };
    assert!(
        mediated.contains("/api/data status=201"),
        "② the SW-mediated fetch itself must reach the page as 201 (live \
         prerequisite for CDP observability): {mediated}"
    );
    assert!(
        mediated.contains("CDP_OBSERVABLE_BODY"),
        "② the mediated body must be the SW's synthetic response: {mediated}"
    );
    // Control probe: a page console.log marker. If Log.entryAdded for it
    // reaches the WS client, the event channel (delegate → EventSubscriber
    // → translate → broadcaster → WS) is PROVEN live, isolating the red to
    // the missing Network emitter rather than a dead event channel.
    let _ = page.evaluate_js_web("console.log('c19-sub2-channel-marker')");
    for _ in 0..20 {
        spin_once(true);
        std::thread::sleep(Duration::from_millis(50));
    }
    fetch_done.store(true, Ordering::Relaxed);

    // Spin until the client finishes (its collection + assertions).
    let deadline = Instant::now() + Duration::from_secs(90);
    while !done.load(Ordering::Relaxed) && Instant::now() < deadline {
        spin_once(true);
        std::thread::yield_now();
    }
    client.join().expect("② client phase must not panic");
    assert!(
        done.load(Ordering::Relaxed),
        "② client phase must have completed all assertions"
    );
    eprintln!("[c19-sub2] === SUBCLAUSE ② GREEN: CDP Network observed the SW-intercepted fetch ===");
}

// ═══════════════════════════════════════════════════════════════════════════
// ③ Sub-clause 3 — cross-page SW liveness + profile inheritance +
//    terminate(unregister) deregistration
// ═══════════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:19] SW cross-page liveness, profile
/// inheritance and terminate deregistration (live)
///
/// The SW intercepts `/api/inherit` answering
/// `respondWith(new Response('SWUA=' + navigator.userAgent, {status: 201}))`
/// — the UA is read INSIDE the SW realm at fetch-handling time, so the
/// mediated body is a live probe of the SW realm's stealth profile. Asserts:
///   1. INHERITANCE: the SW-realm UA === the REGISTERING page's profile UA
///      (Firefox profile marker present).
///   2. CROSS-PAGE: a SECOND page of the same origin (in scope) is mediated
///      by the SAME live worker with the SAME inherited UA.
///   3. DEREGISTRATION: after getRegistration().unregister() resolves true
///      (the vendor terminate path — the manager drops the registration and
///      terminates the worker thread before resolving), new requests must
///      receive the NATIVE fixture response (mediation gone).
#[test]
fn c19_sub3_sw_cross_page_inheritance_and_terminate_deregistration_live() {
    if !common::run_isolated(
        "sw_stealth_profile_tests::c19_sub3_sw_cross_page_inheritance_and_terminate_deregistration_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    let fixture = SwC19HttpFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    fixture.set_script(
        "self.addEventListener('fetch', function (e) { \
           var u = String(e.request.url); \
           if (u.indexOf('/api/inherit') !== -1) { \
             e.respondWith(new Response('SWUA=' + navigator.userAgent, { \
               status: 201, \
               statusText: 'SW Inherit', \
               headers: { 'Content-Type': 'text/plain' } \
             })); \
           } \
         });"
        .to_string(),
    );

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page1 = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page 1 must succeed");
    pump(&page1, 500);

    // The REGISTERING page's own profile UA — the SW realm must match it.
    let page1_ua = unquote_bridge(
        page1
            .evaluate_js_web("navigator.userAgent")
            .expect("page1 navigator.userAgent read must succeed")
            .trim()
            .to_string(),
    );
    assert!(
        page1_ua.contains("Firefox"),
        "③ page1 must run the Firefox stealth profile, got UA: {page1_ua}"
    );

    // Register + wait for the mediated verdict (retry: activation async).
    let reg = register_sw(&page1, "/sw.js");
    eprintln!("[c19-sub3] register outcome = {reg}");
    assert!(
        reg.contains("ok"),
        "③ serviceWorker.register must resolve ok (live-path prerequisite), got: {reg}"
    );
    let page1_verdict = {
        let mut verdict = String::new();
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            verdict = one_probe(&page1, "/api/inherit");
            eprintln!("[c19-sub3] page1 attempt = {verdict}");
            if verdict.contains("status=201") || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        verdict
    };
    assert!(
        page1_verdict.contains("/api/inherit status=201"),
        "③ 1. INHERITANCE prerequisite: page1's /api/inherit must be mediated \
         to the SW's 201: {page1_verdict}"
    );
    assert!(
        page1_verdict.contains("SWUA="),
        "③ 1. INHERITANCE: the mediated body must carry the SW-realm UA \
         (SWUA=…), got: {page1_verdict}"
    );
    let sw_ua = page1_verdict
        .split("SWUA=")
        .nth(1)
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(
        sw_ua.contains("Firefox"),
        "③ 1. INHERITANCE: the SW realm's navigator.userAgent must serve the \
         stealth profile (Firefox marker), got: {sw_ua} — a bare/native SW \
         realm means the SW scope drain (S1) did not install the page profile"
    );
    assert_eq!(
        sw_ua, page1_ua,
        "③ 1. INHERITANCE: SW-realm UA must EXACTLY equal the registering \
         page's profile UA (DF-WK-10): sw={sw_ua} vs page={page1_ua}"
    );

    // 2. CROSS-PAGE — a second page of the same origin (in scope) must be
    // mediated by the SAME live worker with the SAME inherited UA.
    let page2 = runtime
        .create_page(&PageConfig {
            url: Some(format!("{origin}page2")),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page 2 must succeed");
    pump(&page2, 500);
    let page2_verdict = {
        let mut verdict = String::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            verdict = one_probe(&page2, "/api/inherit");
            eprintln!("[c19-sub3] page2 attempt = {verdict}");
            if verdict.contains("status=201") || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        verdict
    };
    assert!(
        page2_verdict.contains("/api/inherit status=201"),
        "③ 2. CROSS-PAGE: page2 (same origin, in scope) must be mediated by \
         the SAME live SW (persistent lifecycle): {page2_verdict}"
    );
    assert!(
        page2_verdict.contains(&format!("SWUA={page1_ua}"))
            || page2_verdict.contains(&sw_ua),
        "③ 2. CROSS-PAGE: page2's mediated body must carry the SAME inherited \
         SW-realm UA: {page2_verdict} (expected SWUA={sw_ua})"
    );

    // 3. DEREGISTRATION — unregister (the vendor terminate path; the manager
    // terminates the worker thread before resolving). New requests must get
    // the NATIVE fixture body: mediation is deregistered.
    let unregister_js = "window.__swUn = 'pending'; \
         navigator.serviceWorker.getRegistration().then( \
           function (r) { \
             if (!r) { window.__swUn = 'noreg'; return; } \
             r.unregister().then( \
               function (v) { window.__swUn = 'un:' + v; }, \
               function (e) { window.__swUn = 'err:' + e; }); \
           }, \
           function (e) { window.__swUn = 'err:' + e; }); \
         window.__swUn";
    let _ = page1.evaluate_js_web(unregister_js);
    let un_outcome = wait_for(
        || match page1.evaluate_js_web("window.__swUn") {
            Ok(s) if s.contains("pending") => None,
            Ok(s) => Some(s),
            Err(_) => None,
        },
        Duration::from_secs(20),
        "getRegistration + unregister settlement",
    )
    .unwrap_or_else(|| "<no settlement>".to_string());
    eprintln!("[c19-sub3] unregister = {un_outcome}");
    assert!(
        un_outcome.contains("un:true"),
        "③ 3. DEREGISTRATION prerequisite: unregister must resolve true, got: {un_outcome}"
    );

    // Post-terminate probe: the mediation must be GONE — /api/inherit now
    // returns the native 200 body (bounded retry: the manager may drop the
    // SwManagers entry asynchronously; in-flight bounded-wait mediations
    // degrade to pass-through).
    let post_verdict = {
        let mut verdict = String::new();
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            verdict = one_probe(&page2, "/api/inherit");
            eprintln!("[c19-sub3] post-terminate attempt = {verdict}");
            if verdict.contains("NATIVE_INHERIT_BODY") || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        verdict
    };
    assert!(
        post_verdict.contains("/api/inherit status=200") &&
            post_verdict.contains("NATIVE_INHERIT_BODY"),
        "③ 3. DEREGISTRATION: after terminate(unregister) the SW must NO \
         LONGER mediate — /api/inherit must return the native fixture body. \
         Got: {post_verdict}"
    );
    assert!(
        !post_verdict.contains("SWUA="),
        "③ 3. DEREGISTRATION: the SW's mediated body must never appear after \
         terminate: {post_verdict}"
    );
    eprintln!("[c19-sub3] === SUBCLAUSE ③ GREEN: cross-page liveness + inheritance + deregistration ===");
}

// ═══════════════════════════════════════════════════════════════════════════
// ④ SW scope injector starvation — same-page DedicatedWorker created BEFORE
//    the SW registration must not starve the SW scope of embedder injection
// ═══════════════════════════════════════════════════════════════════════════

/// URL-encode a JS worker body for a data: URL (the worker_multi_injection
/// percent-encoding form — proves on this page's webview).
fn encode_js_body(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            },
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            },
        }
    }
    out
}

/// Extract a `|`-separated marker field (worker_multi_injection form).
fn marker_field<'a>(r: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    r.split('|').find_map(|f| f.strip_prefix(&prefix))
}

/// Dedicated-Worker probe body (the starvation setup AND the exhaustion
/// proof): worker #1 of this webview drains the consume-once one-shot scope
/// queue; its own marker proves it ran and was fully injected (the one-shot
/// + injector double delivery, worker-#1 semantics).
const STARVE_WORKER_PROBE: &str = r#"
var __r = (function () {
  try {
    var ua = String(navigator.userAgent);
    var audioHooked = typeof AudioBuffer !== 'undefined'
      && String(AudioBuffer.prototype.getChannelData).indexOf('detNoise') !== -1;
    return 'DEDRAN|ua=' + ua + '|audiohooked=' + (audioHooked ? 1 : 0);
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
self.postMessage(__r);
"#;

/// @trace REQ-BRW-004 [criterion:12..17] SW scope injector starvation
///
/// The v49/arch-closeout-③ defect: the SW scope's only embedder injection
/// was the consume-once one-shot drain (S1 f77faf8b). A page that created a
/// Dedicated Worker BEFORE registering its SW had already exhausted that
/// queue, so the SW scope ran with ZERO injection — bare native
/// navigator.userAgent, no W1a audio/webgl JS hooks — a fingerprintable SW
/// realm (the exact e43 multi-worker starvation shape, on the SW path).
///
/// Fix under test: the per-Worker NON-consuming injector tier
/// (5627cf8e) delivered on the SW path at both points — after the SW's
/// one-shot drain (engine getters) and after the SW's own
/// `define_all_exposed_interfaces` (W1a JS hooks; the SW path never reaches
/// `WorkerGlobalScope::on_complete`, so without this delivery point the
/// hooks NEVER landed on any SW realm).
///
/// Timing under test: worker FIRST (exhausts one-shot), SW registration
/// SECOND. The SW-realm probe (computed inside the fetch handler, returned
/// via respondWith — the sub3 verdict transport) must carry the FULL
/// injection state: ua === page profile UA (engine getter, permanent
/// accessor), audiohooked=1 + wglhooked=1 (W1a hooks), orignative=1 (e36
/// gate) — and each marker exactly once (hooks blob single delivery, no
/// double-define).
#[test]
fn sw_scope_injector_starvation_after_dedicated_worker_live() {
    if !common::run_isolated(
        "sw_stealth_profile_tests::sw_scope_injector_starvation_after_dedicated_worker_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    let fixture = SwC19HttpFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    // The SW answers /api/starve with the FULL injection probe computed in
    // the SW realm (the worker_multi_injection probe form, returned through
    // the sub3 respondWith transport).
    fixture.set_script(
        "self.addEventListener('fetch', function (e) { \
           var u = String(e.request.url); \
           if (u.indexOf('/api/starve') !== -1) { \
             var ua = String(navigator.userAgent); \
             var audioHooked = typeof AudioBuffer !== 'undefined' \
               && String(AudioBuffer.prototype.getChannelData).indexOf('detNoise') !== -1; \
             var wglHooked = 'n/a'; \
             var origNative = 'n/a'; \
             if (typeof WebGLRenderingContext !== 'undefined') { \
               wglHooked = String(WebGLRenderingContext.prototype.getParameter).indexOf('dbgRenderer') !== -1 ? 1 : 0; \
               var orig = WebGLRenderingContext.prototype.__originalGetParameter__; \
               origNative = (typeof orig === 'function' && String(orig).indexOf('dbgRenderer') === -1) ? 1 : 0; \
             } \
             var d = Object.getOwnPropertyDescriptor(navigator, 'userAgent'); \
             var permGetter = d && d.configurable === false && typeof d.get === 'function' ? 1 : 0; \
             e.respondWith(new Response( \
               'SWPROBE|ua=' + ua + '|audiohooked=' + (audioHooked ? 1 : 0) \
               + '|wglhooked=' + wglHooked + '|orignative=' + origNative \
               + '|permgetter=' + permGetter, \
               { status: 201, statusText: 'SW Starve', \
                 headers: { 'Content-Type': 'text/plain' } })); \
           } \
         });"
        .to_string(),
    );

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    pump(&page, 500);

    // The page's OWN profile UA — the SW realm must match it exactly.
    let page_ua = unquote_bridge(
        page.evaluate_js_web("navigator.userAgent")
            .expect("page navigator.userAgent read must succeed")
            .trim()
            .to_string(),
    );
    assert!(
        page_ua.contains("Firefox"),
        "④ page must run the Firefox stealth profile, got UA: {page_ua}"
    );

    // ── 1. DEDICATED WORKER FIRST — the starvation setup: worker #1 of this
    //        webview drains the consume-once one-shot scope queue. Its marker
    //        is the exhaustion proof (a fully-injected worker ran).
    let _ = page.evaluate_js_web("window.__swStarveW = null;");
    let worker_js = format!(
        "(function () {{ try {{ \
           var w = new Worker('data:text/javascript,{}'); \
           w.onmessage = function (e) {{ window.__swStarveW = String(e.data); }}; \
           w.onerror = function (ev) {{ \
             window.__swStarveW = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown'); \
             return true; \
           }}; \
           return 'worker-created'; \
         }} catch (e) {{ window.__swStarveW = 'CREATE-ERROR:' + String(e); }} }})()",
        encode_js_body(STARVE_WORKER_PROBE)
    );
    let created = page.evaluate_js_web(&worker_js);
    match created {
        Ok(s) => assert!(
            s.contains("worker-created"),
            "④ dedicated Worker creation must succeed (starvation setup), got: {s}"
        ),
        Err(e) => panic!("④ dedicated Worker dispatch failed: {e}"),
    }
    let worker_res = wait_for(
        || match page.evaluate_js_web("window.__swStarveW") {
            Ok(s) => {
                let t = s.trim();
                let t = unquote_bridge(t.to_string());
                if t.is_empty() || t == "null" || t == "undefined" {
                    None
                } else {
                    Some(t)
                }
            },
            Err(_) => None,
        },
        Duration::from_secs(45),
        "dedicated worker (starvation setup) marker",
    )
    .unwrap_or_else(|| "<no marker>".to_string());
    eprintln!("[sw-starve] dedicated worker #1: {worker_res}");
    assert!(
        worker_res.starts_with("DEDRAN|"),
        "④ dedicated worker probe must complete (the one-shot exhaustion \
         setup), got: {worker_res}"
    );
    let w_ua = marker_field(&worker_res, "ua").unwrap_or("");
    assert_eq!(
        w_ua, page_ua,
        "④ worker #1 must carry the page profile UA (one-shot consumed by a \
         FULLY-INJECTED worker — this is what starves a later SW without the \
         injector tier): worker={w_ua} vs page={page_ua}"
    );
    assert!(
        worker_res.contains("|audiohooked=1|") || worker_res.ends_with("|audiohooked=1"),
        "④ worker #1 must carry the W1a audio hook (injection machinery live \
         on this webview), got: {worker_res}"
    );

    // ── 2. NOW register the SW — the starved timing (one-shot already
    //        consumed by the dedicated worker above).
    let reg = register_sw(&page, "/sw.js");
    eprintln!("[sw-starve] register outcome = {reg}");
    assert!(
        reg.contains("ok"),
        "④ serviceWorker.register must resolve ok (live-path prerequisite), got: {reg}"
    );

    // ── 3. The SW-realm full-injection probe (retry: activation async).
    let verdict = {
        let mut verdict = String::new();
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            verdict = one_probe(&page, "/api/starve");
            eprintln!("[sw-starve] attempt = {verdict}");
            if verdict.contains("status=201") || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        verdict
    };
    assert!(
        verdict.contains("/api/starve status=201"),
        "④ the SW-mediated probe must reach the page as 201 (live \
         prerequisite), got: {verdict}"
    );
    let body = verdict
        .split("body=")
        .nth(1)
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(
        body.starts_with("SWPROBE|"),
        "④ the mediated body must be the SW-realm probe marker, got: {body}"
    );
    eprintln!("[sw-starve] SW-realm probe: {body}");

    // THE starvation assertion: engine getter — the SW realm's UA must
    // EXACTLY equal the page profile UA even though the one-shot queue was
    // already consumed (before the fix this was the servo native UA).
    let sw_ua = marker_field(&body, "ua").unwrap_or("");
    assert_eq!(
        sw_ua, page_ua,
        "④ SW INJECTION STARVATION: the SW realm's navigator.userAgent must \
         serve the page profile UA even when a Dedicated Worker consumed the \
         one-shot queue first — a native UA means the SW scope ran with ZERO \
         embedder injection (bare fingerprintable SW realm): sw={sw_ua} vs \
         page={page_ua}"
    );
    // W1a JS hooks — landed at the SW's own post-define delivery point.
    assert!(
        body.contains("|audiohooked=1|") || body.ends_with("|audiohooked=1"),
        "④ SW realm must carry the W1a audio getChannelData JS hook (the SW \
         path never reaches on_complete — without the SW post-define delivery \
         this hook NEVER landed on a SW realm), got: {body}"
    );
    assert!(
        body.contains("|wglhooked=1|") || body.ends_with("|wglhooked=1"),
        "④ SW realm must carry the W1a webgl getParameter JS hook, got: {body}"
    );
    // Double-define guards — single delivery, no re-save loop (completion ③:
    // the SW scope injected exactly once).
    assert!(
        body.contains("|orignative=1|") || body.ends_with("|orignative=1"),
        "④ e36 gate — __originalGetParameter__ must stay the servo native \
         (no double-define loop on the SW path), got: {body}"
    );
    assert!(
        body.contains("|permgetter=1|") || body.ends_with("|permgetter=1"),
        "④ navigator.userAgent must be a non-configurable accessor getter in \
         the SW realm (define_permanent_getter prior-install arm held), got: \
         {body}"
    );
    assert_eq!(
        body.matches("SWPROBE|").count(),
        1,
        "④ the SW probe marker must appear exactly once (single mediated \
         response, no double delivery of the probe itself): {body}"
    );
    eprintln!("[sw-starve] === ④ GREEN: SW scope fully injected in the starved timing ===");
}
